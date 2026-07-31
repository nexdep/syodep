//! The application core: glues config, input, layout, rendering and storage.
//!
//! The UI shell drives this type exclusively through:
//!
//! - lifecycle: [`App::new`], [`App::open_document`]
//! - input: [`App::handle_key`], [`App::scroll_by_px`], [`App::set_viewport_size`]
//! - output: [`App::visible_pages`], [`App::render_page`], [`App::status_text`]
//!
//! [`App::handle_key`] returns [`Effects`] describing what the shell must do
//! (redraw, quit, show a file dialog). The shell never interprets keys or
//! touches document state itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use syodep_config::keys::Chord;
use syodep_config::Config;
use syodep_pdf::{
    Bitmap, CellKind, ContentLine, ContentObject, ContentOptions, FurnitureProfile,
    HighlightAnnotation, ObjectKind, PageContent, Rect,
};
use syodep_storage::{HighlightRect, Position, Storage};

use crate::caret::{
    column_index_of, column_ranges, continues_word_run, is_abbreviation, is_attached_number_suffix,
    is_exponent_sign, is_inside_dotted_token, is_inside_hyphenated_word, is_inside_number,
    is_inside_scientific_exponent, is_line_final_colon, is_number_suffix, is_numeric_separator,
    is_sentence_terminator, is_sentence_trailer, is_word_hyphen, is_word_target, link_span,
    nearest_cell_in_line, nearest_line_in_column, opens_a_sentence, page_span_rects,
    paragraph_segments, split_segments_at_objects, word_class, Caret, Dir, Landing, LineMark, Mode,
    ObjectId, ParagraphMark, PendingHighlight, Scope, SentenceMark, VisualAnchor, VisualSelection,
    WordClass, WordMark,
};
use crate::command::Command;
use crate::input::{InputState, KeyOutcome, Keymap, KeymapError};
use crate::layout::{DocumentLayout, PageSize, ScreenRect, View};
use crate::render_cache::RenderCache;

/// Hard bound on how many raw steps one atomic step may take to leave an
/// object. Only a runaway detection could ever approach it; it exists so the
/// loop terminates no matter what the per-scope steppers do.
const MAX_ATOMIC_STEPS: usize = 4096;

/// Hard bound on how many real-space-separated fragments [`App::link_at`]
/// may stitch into one address. The loop already terminates on its own —
/// each iteration consumes at least one more token, and a line is finite —
/// this exists only so a pathological line cannot make one caret query
/// visibly slow.
const MAX_LINK_FRAGMENTS: usize = 20;

/// Side effects the UI shell must perform after an input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effects {
    pub redraw: bool,
    pub quit: bool,
    /// The shell should show a native file-open dialog and call
    /// [`App::open_document`] with the result.
    pub open_file_dialog: bool,
    /// A partial key sequence is buffered. The shell should arm its pause
    /// timer and call [`App::handle_timeout`] when it fires; when this is
    /// false it should cancel any armed timer.
    pub pending_input: bool,
    /// The document was reloaded from disk, so any page bitmaps the shell is
    /// holding are stale even though the layout and zoom did not change. Only a
    /// save sets this today.
    pub reload: bool,
    /// Quitting would lose highlights not yet embedded in the PDF. The shell
    /// should ask (Save & Quit / Discard & Quit / Cancel) and call
    /// [`App::save_and_quit`] or [`App::quit_discarding_highlights`] with the
    /// answer, rather than quitting outright.
    pub confirm_quit: bool,
}

impl Effects {
    fn redraw() -> Self {
        Self {
            redraw: true,
            ..Self::default()
        }
    }

    /// Combine the effects of two commands run for one key press (see
    /// [`App::handle_key`]); every effect is a request, so they OR together.
    fn merge(self, other: Self) -> Self {
        Self {
            redraw: self.redraw || other.redraw,
            quit: self.quit || other.quit,
            open_file_dialog: self.open_file_dialog || other.open_file_dialog,
            reload: self.reload || other.reload,
            confirm_quit: self.confirm_quit || other.confirm_quit,
            // Not a request like the others: it describes the state left
            // behind, so the later value wins rather than OR-ing.
            pending_input: other.pending_input,
        }
    }
}

/// A page to draw, in canvas pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisiblePage {
    pub page: usize,
    pub rect: ScreenRect,
}

/// Where the reader was, carried across the reopen a save performs.
#[derive(Debug, Clone, Copy)]
struct SelectionState {
    mode: Mode,
    focus: Option<Caret>,
    focus_scope: Scope,
    visual: Option<VisualAnchor>,
    scroll: Option<(f32, f32)>,
    zoom: Option<f32>,
}

/// The built-in highlight colour, for the one case where the configured value
/// cannot be parsed. Unreachable in practice — the FFI validates the colour at
/// startup and warns — but a highlight must still get written on save.
fn default_highlight_rgb() -> (u8, u8, u8) {
    syodep_config::parse_hex_color(&syodep_config::ViewConfig::default().highlight_color)
        .expect("the built-in highlight colour is valid")
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Pdf(#[from] syodep_pdf::PdfError),
    #[error(transparent)]
    Storage(#[from] syodep_storage::StorageError),
    #[error("no document is open")]
    NoDocument,
    #[error("cannot write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// A stored highlight, held for the open document.
///
/// Geometry rather than a caret span, for the reason spelled out on
/// [`syodep_storage::StoredHighlight`]: rectangles are what the overlay, the PDF
/// writer and any future export all need, and they survive a reload without the
/// content layer being re-extracted.
#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    /// Row id in the database; `None` when persistence is disabled.
    pub id: Option<i64>,
    /// `#rrggbb`.
    pub color: String,
    /// The characters the highlight covers, for notes and export.
    pub text: String,
    /// One rectangle per covered line, in page points with the origin top left.
    pub rects: Vec<HighlightRect>,
}

struct Session {
    doc: syodep_pdf::Document,
    path: PathBuf,
    document_id: Option<i64>,
    view: View,
    cache: RenderCache,
    /// Lazily-extracted navigable content, per page. Text is cheap to keep, so
    /// every visited page stays cached for the life of the session.
    content: HashMap<usize, PageContent>,
    /// What this document repeats in its margins. Learned once, before the
    /// first page is extracted, so every cached page above was filtered
    /// against the same evidence — otherwise navigation would differ depending
    /// on which page happened to be visited first.
    furniture: Option<FurnitureProfile>,
}

/// Top-level application state. One instance per window.
pub struct App {
    config: Config,
    keymap: Keymap,
    /// Keymap used while in focus mode: the normal keymap plus the
    /// `[focus_keys]` overrides (so `hjkl`/`<Esc>` change meaning there). One
    /// keymap covers every scope, because the motion commands dispatch on the
    /// scope rather than being named after it.
    focus_keymap: Keymap,
    /// Keymap used while in visual mode: the normal keymap plus the
    /// `[visual_keys]` overrides.
    visual_keymap: Keymap,
    /// Keymap used while placing a highlight: the normal keymap plus the
    /// `[highlight_keys]` overrides. Its motions are bound to the *visual*
    /// commands, so there is one implementation of reshaping a selection.
    highlight_keymap: Keymap,
    input: InputState,
    storage: Option<Storage>,
    session: Option<Session>,
    viewport: (f32, f32),
    /// Whether `hjkl` scroll, move the focus, or grow a selection.
    mode: Mode,
    /// The live position, remembered across mode toggles. A single point: what
    /// is *highlighted* is derived from it by [`Self::scope_span`], so changing
    /// the scope cannot leave the highlight somewhere else.
    ///
    /// In visual mode this is also the end that motions move, which is why
    /// leaving visual by any route keeps your place.
    focus: Option<Caret>,
    /// Granularity the highlight snaps to, remembered across mode toggles. In
    /// visual mode this is the moving end's scope.
    focus_scope: Scope,
    /// The focus highlight resolved to a document-order cell range, recomputed
    /// after every change to `focus` or `focus_scope`. Cached for the same
    /// reason as `visual_span`: resolving it needs page content (`&mut self`)
    /// while the overlay getter the shell calls is `&self`.
    focus_span: Option<(Caret, Caret)>,
    /// Remembered goal column (page-space x) for vertical motion.
    focus_goal_x: f32,
    /// Remembered goal row (page-space y center) for line-scope column motion.
    focus_goal_y: f32,
    /// The anchored end, present only in visual mode. The *moving* end is
    /// `focus`/`focus_scope` above — there is no second position, so no pair of
    /// fields to drift apart.
    visual: Option<VisualAnchor>,
    /// The selection resolved to a document-order cell range, recomputed after
    /// every change to either end. Cached because resolving it needs page
    /// content (`&mut self`) while the overlay getter the shell calls is
    /// `&self`.
    visual_span: Option<(Caret, Caret)>,
    /// The highlight being placed, present only in [`Mode::Highlight`]. Its
    /// *extent* is not stored here — that is `visual_span`, because a pending
    /// highlight is a selection; this holds only what discarding must restore.
    pending: Option<PendingHighlight>,
    /// Highlights stored for the open document but not yet written into the PDF.
    /// Emptied by a successful save, after which the PDF renders them itself.
    highlights: Vec<Highlight>,
    /// Config/keymap problems collected at startup, for the UI to surface.
    startup_warnings: Vec<String>,
    last_error: Option<String>,
    /// One-off feedback for the status line (what a save did). Cleared by the
    /// next command, so it reads as a reply to the key just pressed.
    status_message: Option<String>,
}

impl App {
    /// Create the core with an already-loaded config and an optional storage
    /// handle. `storage = None` disables persistence (used by some tests and
    /// as graceful degradation when the database cannot be opened).
    pub fn new(config: Config, storage: Option<Storage>) -> Self {
        // The leader is parsed first, because every table below may use it. A
        // bad value degrades to the default plus a warning rather than costing
        // the user every `<leader>` binding they have.
        let mut leader_warning = None;
        let leader = match syodep_config::keys::parse_sequence(&config.input.leader) {
            Ok(chords) => chords,
            Err(e) => {
                let fallback = syodep_config::InputConfig::default().leader;
                leader_warning = Some(format!(
                    "invalid [input] leader: {e}; using the default {fallback:?}"
                ));
                syodep_config::keys::parse_sequence(&fallback)
                    .expect("the built-in default leader is valid")
            }
        };
        let entries = config.keys.iter().map(|(k, v)| (k.as_str(), v.as_str()));
        let (keymap, mut keymap_errors) = Keymap::from_entries(entries, &leader);
        // The focus keymap is the normal keymap with the focus overrides
        // applied, so every normal binding still works in focus mode and only
        // the overridden keys (hjkl/<Esc>) change meaning. Cloning then
        // overlaying avoids re-validating (and double-reporting) normal keys.
        let mut focus_keymap = keymap.clone();
        let focus_entries = config
            .focus_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(focus_keymap.overlay(focus_entries));
        // Visual mode's keymap, from `[visual_keys]`.
        let mut visual_keymap = keymap.clone();
        let visual_entries = config
            .visual_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(visual_keymap.overlay(visual_entries));
        // Highlight mode's keymap, from `[highlight_keys]`. Built the same way as
        // the other two, so `v`/`c` and every normal binding still reach it.
        let mut highlight_keymap = keymap.clone();
        let highlight_entries = config
            .highlight_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(highlight_keymap.overlay(highlight_entries));
        let startup_warnings = leader_warning
            .into_iter()
            .chain(keymap_errors.iter().map(KeymapError::to_string))
            .collect();
        Self {
            config,
            keymap,
            focus_keymap,
            visual_keymap,
            highlight_keymap,
            input: InputState::new(),
            storage,
            session: None,
            viewport: (800.0, 600.0),
            mode: Mode::Normal,
            focus: None,
            focus_scope: Scope::Char,
            focus_span: None,
            focus_goal_x: 0.0,
            focus_goal_y: 0.0,
            visual: None,
            visual_span: None,
            pending: None,
            highlights: Vec::new(),
            startup_warnings,
            last_error: None,
            status_message: None,
        }
    }

    pub fn startup_warnings(&self) -> &[String] {
        &self.startup_warnings
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn has_document(&self) -> bool {
        self.session.is_some()
    }

    pub fn document_path(&self) -> Option<&Path> {
        self.session.as_ref().map(|s| s.path.as_path())
    }

    /// Open a document, restoring its saved reading position if one exists.
    pub fn open_document(&mut self, path: &Path) -> Result<(), AppError> {
        let doc = syodep_pdf::Document::open(path)?;
        let sizes: Vec<PageSize> = doc
            .page_sizes()
            .iter()
            .map(|s| PageSize {
                width: s.width,
                height: s.height,
            })
            .collect();
        let layout = DocumentLayout::new(&sizes, self.config.view.page_gap);
        let mut view = View::new(layout, self.viewport.0, self.viewport.1);

        let mut document_id = None;
        let mut restored = false;
        if let Some(storage) = &self.storage {
            // Persistence failures must not prevent reading the document.
            match Self::lookup_position(storage, path) {
                Ok((id, position)) => {
                    document_id = Some(id);
                    if let Some(p) = position {
                        view.restore(p.scroll_x, p.scroll_y, p.zoom);
                        restored = true;
                    }
                }
                Err(e) => self
                    .startup_warnings
                    .push(format!("persistence disabled for this document: {e}")),
            }
        }
        if !restored {
            if self.config.view.fit_width_on_open {
                view.fit_width();
            } else {
                view.set_zoom(self.config.view.default_zoom);
            }
        }

        self.session = Some(Session {
            doc,
            path: path.to_owned(),
            document_id,
            view,
            cache: RenderCache::default(),
            content: HashMap::new(),
            furniture: None,
        });
        // Caret positions are document-specific; reset to normal mode.
        self.mode = Mode::Normal;
        self.focus = None;
        self.focus_scope = Scope::Char;
        self.focus_span = None;
        self.focus_goal_x = 0.0;
        self.focus_goal_y = 0.0;
        self.visual = None;
        self.visual_span = None;
        self.pending = None;
        self.last_error = None;
        // Stored highlights are geometry, so they can be drawn immediately —
        // nothing has to be extracted or resolved first.
        self.load_highlights();
        Ok(())
    }

    fn lookup_position(
        storage: &Storage,
        path: &Path,
    ) -> Result<(i64, Option<Position>), AppError> {
        let fingerprint = Storage::fingerprint_file(path)?;
        let id = storage.upsert_document(&fingerprint, &path.display().to_string())?;
        Ok((id, storage.load_position(id)?))
    }

    /// Persist the current reading position. Called automatically after
    /// navigation commands; safe to call at any time.
    pub fn save_position(&mut self) {
        let Some(session) = &self.session else { return };
        let (Some(storage), Some(id)) = (&self.storage, session.document_id) else {
            return;
        };
        let (scroll_x, scroll_y) = session.view.scroll();
        let result = storage.save_position(
            id,
            Position {
                scroll_x,
                scroll_y,
                zoom: session.view.zoom(),
            },
        );
        if let Err(e) = result {
            self.last_error = Some(format!("could not save position: {e}"));
        }
    }

    /// Whether quitting now would leave highlights not yet embedded in the
    /// PDF — either already committed (`self.highlights`) or still being
    /// placed. Read-only, unlike [`Command::Quit`]: never commits an
    /// in-progress highlight, so a caller (the window-close path) can ask
    /// "should I warn?" with no side effects.
    pub fn has_unsaved_highlights(&self) -> bool {
        !self.highlights.is_empty() || self.has_pending_highlight()
    }

    /// Quit without embedding unsaved highlights into the PDF. Nothing is
    /// deleted — they are already rows in `highlights`/SQLite, persisted as
    /// they were committed — so they simply reappear as pending overlays next
    /// time this document is opened. Used both when [`Command::Quit`] finds
    /// nothing to confirm, and as the "Discard & Quit" dialog answer.
    pub fn quit_discarding_highlights(&mut self) -> Effects {
        self.save_position();
        Effects {
            quit: true,
            ..Effects::default()
        }
    }

    /// Embed unsaved highlights into the PDF, then quit — but only if the
    /// save succeeds ([`App::save_document`]'s `reload: true` is the existing
    /// success signal). On failure this returns exactly what a failed save
    /// returns: `quit` stays false, the document stays open, `last_error` is
    /// set.
    pub fn save_and_quit(&mut self) -> Effects {
        let effects = self.save_document();
        if effects.reload {
            self.save_position();
            return Effects {
                quit: true,
                ..effects
            };
        }
        effects
    }

    /// Overwrite the open PDF with its highlights embedded as PDF annotations.
    ///
    /// The file is rewritten beside itself and renamed over the original, so an
    /// interrupted write can never leave a half-written PDF where the document
    /// was. Afterwards the document is reopened: its content hash has changed, so
    /// the annotations MuPDF now renders into the page bitmaps are the highlights
    /// and the overlay must stop drawing them.
    fn save_document(&mut self) -> Effects {
        // Saving keeps a highlight in progress rather than losing it, the same
        // way `a`, `v` and `c` do — and lands in focus, like `a` and `c`.
        if self.mode == Mode::Highlight {
            let _ = self.enter_focus(self.focus_scope);
        }
        match self.write_document() {
            Ok(count) => {
                self.status_message = Some(match count {
                    1 => "saved 1 highlight".to_owned(),
                    n => format!("saved {n} highlights"),
                });
                Effects {
                    reload: true,
                    ..Effects::redraw()
                }
            }
            Err(e) => {
                self.last_error = Some(format!("could not save: {e}"));
                Effects::redraw()
            }
        }
    }

    /// The save itself, split out so every failure path is one `?` away from
    /// leaving the document exactly as it was.
    fn write_document(&mut self) -> Result<usize, AppError> {
        let Some(session) = &self.session else {
            return Err(AppError::NoDocument);
        };
        let path = session.path.clone();
        let document_id = session.document_id;
        if self.highlights.is_empty() {
            return Ok(0);
        }
        let annotations = self.highlight_annotations();
        let count = self.highlights.len();

        // Beside the original, so the rename below stays within one filesystem
        // and is therefore atomic. A fixed name rather than a random one: a
        // leftover from a crashed save is then obvious, and simply overwritten.
        let temp = path.with_extension("pdf.syodep-tmp");
        if let Err(e) = syodep_pdf::write_highlights(&path, &temp, &annotations) {
            let _ = std::fs::remove_file(&temp);
            return Err(e.into());
        }

        // Close the document *before* the rename: on Windows, replacing a file
        // MuPDF still holds open fails with a sharing violation.
        let restore = self.take_selection_state();
        self.session = None;
        if let Err(source) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            // The original is untouched, so reopening restores the status quo.
            let _ = self.open_document(&path);
            self.restore_selection_state(restore);
            return Err(AppError::Write {
                path: path.display().to_string(),
                source,
            });
        }

        // The rewritten file hashes differently, so move the document row to the
        // new fingerprint before reopening — otherwise every save orphans the
        // reading position. Then forget the highlight rows: they live in the PDF
        // now, and drawing them as well would paint them twice.
        if let (Some(storage), Some(id)) = (&self.storage, document_id) {
            let outcome = Storage::fingerprint_file(&path).and_then(|fingerprint| {
                storage.rekey_document(id, &fingerprint, &path.display().to_string())?;
                storage.delete_highlights(id)
            });
            if let Err(e) = outcome {
                self.last_error = Some(format!("saved, but could not update the database: {e}"));
            }
        }
        self.open_document(&path)?;
        self.restore_selection_state(restore);
        Ok(count)
    }

    /// The stored highlights as one annotation per (highlight, page).
    ///
    /// Opacity comes from the *current* `[view] highlight_opacity`, the same
    /// place the live overlay reads it from, rather than being captured per
    /// highlight the way colour is: a PDF reader always paints a highlight
    /// with Multiply blending, so matching both blend mode and opacity
    /// between the overlay and the saved `/CA` is what keeps a highlight
    /// looking the same before and after saving. Config has no hot-reload
    /// yet, so within one session this is indistinguishable from capturing
    /// it at creation time.
    fn highlight_annotations(&self) -> Vec<HighlightAnnotation> {
        let opacity = self.config.view.highlight_opacity.clamp(0.0, 1.0);
        let mut out = Vec::new();
        for highlight in &self.highlights {
            let color = syodep_config::parse_hex_color(&highlight.color)
                .unwrap_or_else(default_highlight_rgb);
            // Grouped by page in one pass over rectangles that are already in
            // document order, so no sorting is needed.
            let mut current: Option<HighlightAnnotation> = None;
            for rect in &highlight.rects {
                let rect_out = Rect {
                    x0: rect.x0,
                    y0: rect.y0,
                    x1: rect.x1,
                    y1: rect.y1,
                };
                match &mut current {
                    Some(annotation) if annotation.page == rect.page => {
                        annotation.rects.push(rect_out)
                    }
                    _ => {
                        out.extend(current.take());
                        current = Some(HighlightAnnotation {
                            page: rect.page,
                            rects: vec![rect_out],
                            color,
                            opacity,
                        });
                    }
                }
            }
            out.extend(current);
        }
        out
    }

    /// Everything a reload must put back: reopening a document resets the mode
    /// and position by design, which is right for opening a *different* file and
    /// wrong for reopening the same one.
    fn take_selection_state(&mut self) -> SelectionState {
        SelectionState {
            mode: self.mode,
            focus: self.focus,
            focus_scope: self.focus_scope,
            visual: self.visual,
            scroll: self.session.as_ref().map(|s| s.view.scroll()),
            zoom: self.session.as_ref().map(|s| s.view.zoom()),
        }
    }

    fn restore_selection_state(&mut self, state: SelectionState) {
        if let (Some(session), Some((x, y)), Some(zoom)) =
            (&mut self.session, state.scroll, state.zoom)
        {
            session.view.restore(x, y, zoom);
        }
        self.mode = state.mode;
        self.focus = state.focus;
        self.focus_scope = state.focus_scope;
        self.visual = state.visual;
        self.refresh_focus_span();
        self.refresh_visual_span();
    }

    pub fn set_viewport_size(&mut self, width: f32, height: f32) {
        self.viewport = (width, height);
        if let Some(session) = &mut self.session {
            session.view.set_viewport_size(width, height);
        }
    }

    /// Direct pixel scrolling (mouse wheel / trackpad).
    pub fn scroll_by_px(&mut self, dx: f32, dy: f32) -> Effects {
        if let Some(session) = &mut self.session {
            session.view.scroll_by_px(dx, dy);
            self.save_position();
            Effects::redraw()
        } else {
            Effects::default()
        }
    }

    /// Feed one key press; returns the side effects for the shell.
    ///
    /// One press can resolve more than one command: a longest-prefix fallback
    /// (see `input.rs`) fires the prefix binding and replays the leftover
    /// chords. The keymap is re-selected on every iteration, so a
    /// mode-changing command applies to the chords that follow it.
    pub fn handle_key(&mut self, chord: Chord) -> Effects {
        // Scoped so the keymap borrow ends before `dispatch`. Borrowing the
        // field directly (rather than through a `&self` helper) keeps it
        // disjoint from `self.input`.
        let outcome = {
            let keymap = match self.mode {
                Mode::Normal => &self.keymap,
                Mode::Focus => &self.focus_keymap,
                Mode::Visual => &self.visual_keymap,
                Mode::Highlight => &self.highlight_keymap,
            };
            self.input.handle(keymap, chord)
        };
        self.dispatch(outcome)
    }

    /// End a pending sequence because the shell's pause timer fired.
    ///
    /// The core never reads a clock: the shell owns the timer and arms it when
    /// [`Effects::pending_input`] says a pause could resolve something.
    pub fn handle_timeout(&mut self) -> Effects {
        let outcome = {
            let keymap = match self.mode {
                Mode::Normal => &self.keymap,
                Mode::Focus => &self.focus_keymap,
                Mode::Visual => &self.visual_keymap,
                Mode::Highlight => &self.highlight_keymap,
            };
            self.input.timeout(keymap)
        };
        self.dispatch(outcome)
    }

    /// Run one input outcome and everything the replay queue produces after it.
    fn dispatch(&mut self, outcome: KeyOutcome) -> Effects {
        let mut effects = Effects::default();
        let mut next = Some(outcome);
        // The replay queue shrinks on every pass, so this always terminates;
        // the bound only guards against a future bug turning it into a spin.
        for _ in 0..32 {
            let Some(outcome) = next.take() else { break };
            effects = effects.merge(match outcome {
                // Redraw on pending input so the status line shows it.
                KeyOutcome::Pending => Effects::redraw(),
                KeyOutcome::Unmatched => Effects::redraw(),
                KeyOutcome::Command { command, count } => self.execute(command, count),
            });
            if effects.quit || effects.confirm_quit {
                break;
            }
            // Re-select the keymap each pass: the command just run may have
            // changed the mode, and the replayed chords must use the new one.
            next = match self.input.next_replay() {
                Some(chord) => {
                    let keymap = match self.mode {
                        Mode::Normal => &self.keymap,
                        Mode::Focus => &self.focus_keymap,
                        Mode::Visual => &self.visual_keymap,
                        Mode::Highlight => &self.highlight_keymap,
                    };
                    Some(self.input.handle(keymap, chord))
                }
                None => None,
            };
        }
        // A partial sequence survives on purpose -- that is what the pause
        // timer is for -- so only the replay queue is drained here.
        effects.pending_input = self.input.has_pending_sequence();
        effects
    }

    /// Execute a command. Public so a future command palette can reuse it.
    pub fn execute(&mut self, command: Command, count: Option<u32>) -> Effects {
        // Feedback belongs to the key that produced it, so the next command
        // clears it rather than leaving a stale "saved" sitting on the status
        // line.
        self.status_message = None;
        let n = count.unwrap_or(1).max(1);
        let step = self.config.view.scroll_step * n as f32;
        let hstep = self.config.view.horizontal_scroll_step * n as f32;
        let zoom_step = self.config.view.zoom_step;

        match command {
            Command::Quit => {
                // Quitting keeps a highlight in progress, same as save/`a`/`c`,
                // and lands in focus so the confirm prompt is never stranded in
                // highlight mode.
                if self.mode == Mode::Highlight {
                    let _ = self.enter_focus(self.focus_scope);
                }
                if self.highlights.is_empty() {
                    return self.quit_discarding_highlights();
                }
                return Effects {
                    confirm_quit: true,
                    ..Effects::redraw()
                };
            }
            Command::OpenFile => {
                return Effects {
                    open_file_dialog: true,
                    redraw: true,
                    ..Effects::default()
                };
            }
            Command::Cancel => return Effects::redraw(),
            Command::FocusEnter => return self.enter_focus(self.focus_scope),
            Command::FocusEnterChar => return self.enter_focus(Scope::Char),
            Command::FocusEnterWord => return self.enter_focus(Scope::Word),
            Command::FocusEnterLine => return self.enter_focus(Scope::Line),
            Command::FocusEnterSentence => return self.enter_focus(Scope::Sentence),
            Command::FocusEnterParagraph => return self.enter_focus(Scope::Paragraph),
            Command::FocusExit => {
                self.enter_normal_mode();
                return Effects::redraw();
            }
            Command::FocusLeft => return self.focus_move(Dir::Left, count),
            Command::FocusRight => return self.focus_move(Dir::Right, count),
            Command::FocusUp => return self.focus_move(Dir::Up, count),
            Command::FocusDown => return self.focus_move(Dir::Down, count),
            Command::FocusNextWord => {
                return self.focus_scope_motion(Scope::Word, Dir::Right, count)
            }
            Command::FocusPrevWord => {
                return self.focus_scope_motion(Scope::Word, Dir::Left, count)
            }
            Command::FocusNextLine => {
                return self.focus_scope_motion(Scope::Line, Dir::Down, count)
            }
            Command::FocusNextSentence => {
                return self.focus_scope_motion(Scope::Sentence, Dir::Down, count)
            }
            Command::FocusNextParagraph => {
                return self.focus_scope_motion(Scope::Paragraph, Dir::Down, count)
            }
            Command::VisualEnter => return self.enter_visual(None),
            Command::VisualEnterChar => return self.enter_visual(Some(Scope::Char)),
            Command::VisualEnterWord => return self.enter_visual(Some(Scope::Word)),
            Command::VisualEnterLine => return self.enter_visual(Some(Scope::Line)),
            Command::VisualEnterSentence => return self.enter_visual(Some(Scope::Sentence)),
            Command::VisualEnterParagraph => return self.enter_visual(Some(Scope::Paragraph)),
            Command::VisualExit => return self.exit_visual(),
            Command::VisualLeft => return self.visual_move(Dir::Left, count),
            Command::VisualRight => return self.visual_move(Dir::Right, count),
            Command::VisualUp => return self.visual_move(Dir::Up, count),
            Command::VisualDown => return self.visual_move(Dir::Down, count),
            Command::VisualNextWord => {
                return self.visual_scope_motion(Scope::Word, Dir::Right, count)
            }
            Command::VisualPrevWord => {
                return self.visual_scope_motion(Scope::Word, Dir::Left, count)
            }
            Command::VisualNextLine => {
                return self.visual_scope_motion(Scope::Line, Dir::Down, count)
            }
            Command::VisualNextSentence => {
                return self.visual_scope_motion(Scope::Sentence, Dir::Down, count)
            }
            Command::VisualNextParagraph => {
                return self.visual_scope_motion(Scope::Paragraph, Dir::Down, count)
            }
            Command::VisualSwapEnds => return self.visual_swap_ends(),
            Command::VisualScopeChar => return self.set_head_scope(Scope::Char, false),
            Command::VisualScopeWord => return self.set_head_scope(Scope::Word, false),
            Command::VisualScopeLine => return self.set_head_scope(Scope::Line, false),
            Command::VisualScopeSentence => return self.set_head_scope(Scope::Sentence, false),
            Command::VisualScopeParagraph => return self.set_head_scope(Scope::Paragraph, false),
            Command::VisualOtherChar => return self.set_head_scope(Scope::Char, true),
            Command::VisualOtherWord => return self.set_head_scope(Scope::Word, true),
            Command::VisualOtherLine => return self.set_head_scope(Scope::Line, true),
            Command::VisualOtherSentence => return self.set_head_scope(Scope::Sentence, true),
            Command::VisualOtherParagraph => return self.set_head_scope(Scope::Paragraph, true),
            Command::HighlightEnter => return self.enter_highlight(),
            Command::HighlightCommit => return self.commit_highlight(),
            Command::HighlightDiscard => return self.discard_highlight(),
            Command::SaveDocument => return self.save_document(),
            Command::CenterView => return self.center_view(),
            _ => {}
        }

        let Some(session) = &mut self.session else {
            return Effects::default();
        };
        let view = &mut session.view;
        let viewport_h = self.viewport.1;
        match command {
            Command::ScrollDown => view.scroll_by_px(0.0, step),
            Command::ScrollUp => view.scroll_by_px(0.0, -step),
            Command::ScrollLeft => view.scroll_by_px(-hstep, 0.0),
            Command::ScrollRight => view.scroll_by_px(hstep, 0.0),
            Command::ScrollHalfPageDown => view.scroll_by_px(0.0, viewport_h / 2.0 * n as f32),
            Command::ScrollHalfPageUp => view.scroll_by_px(0.0, -viewport_h / 2.0 * n as f32),
            Command::ScrollPageDown => view.scroll_by_px(0.0, viewport_h * n as f32),
            Command::ScrollPageUp => view.scroll_by_px(0.0, -viewport_h * n as f32),
            Command::NextPage => view.next_page(n as usize),
            Command::PrevPage => view.prev_page(n as usize),
            // `{count}gg` / `{count}G` jump to a 1-based page number, like Vim lines.
            Command::GotoFirstPage => match count {
                Some(page) => view.goto_page(page.saturating_sub(1) as usize),
                None => view.goto_page(0),
            },
            Command::GotoLastPage => match count {
                Some(page) => view.goto_page(page.saturating_sub(1) as usize),
                None => view.goto_page(view.layout().page_count().saturating_sub(1)),
            },
            Command::ZoomIn => view.zoom_by(zoom_step.powi(n as i32)),
            Command::ZoomOut => view.zoom_by(1.0 / zoom_step.powi(n as i32)),
            Command::FitWidth => view.fit_width(),
            Command::ZoomReset => view.set_zoom(1.0),
            Command::Quit
            | Command::OpenFile
            | Command::Cancel
            | Command::FocusEnter
            | Command::FocusEnterChar
            | Command::FocusEnterWord
            | Command::FocusEnterLine
            | Command::FocusEnterSentence
            | Command::FocusEnterParagraph
            | Command::FocusExit
            | Command::FocusLeft
            | Command::FocusRight
            | Command::FocusUp
            | Command::FocusDown
            | Command::FocusNextWord
            | Command::FocusPrevWord
            | Command::FocusNextLine
            | Command::FocusNextSentence
            | Command::FocusNextParagraph
            | Command::VisualEnter
            | Command::VisualEnterChar
            | Command::VisualEnterWord
            | Command::VisualEnterLine
            | Command::VisualEnterSentence
            | Command::VisualEnterParagraph
            | Command::VisualExit
            | Command::VisualLeft
            | Command::VisualRight
            | Command::VisualUp
            | Command::VisualDown
            | Command::VisualNextWord
            | Command::VisualPrevWord
            | Command::VisualNextLine
            | Command::VisualNextSentence
            | Command::VisualNextParagraph
            | Command::VisualSwapEnds
            | Command::VisualScopeChar
            | Command::VisualScopeWord
            | Command::VisualScopeLine
            | Command::VisualScopeSentence
            | Command::VisualScopeParagraph
            | Command::VisualOtherChar
            | Command::VisualOtherWord
            | Command::VisualOtherLine
            | Command::VisualOtherSentence
            | Command::VisualOtherParagraph
            | Command::HighlightEnter
            | Command::HighlightCommit
            | Command::HighlightDiscard
            | Command::SaveDocument
            | Command::CenterView => unreachable!("handled above"),
        }
        // In focus mode, scroll and page jumps carry the highlight to the newly
        // visible content; zoom commands leave it where it is.
        let moves_focus = matches!(
            command,
            Command::ScrollHalfPageDown
                | Command::ScrollHalfPageUp
                | Command::ScrollPageDown
                | Command::ScrollPageUp
                | Command::NextPage
                | Command::PrevPage
                | Command::GotoFirstPage
                | Command::GotoLastPage
        );
        if self.mode == Mode::Focus && moves_focus {
            self.reposition_focus_to_viewport();
        }
        self.save_position();
        Effects::redraw()
    }

    /// Pages currently intersecting the viewport, in canvas pixels.
    pub fn visible_pages(&self) -> Vec<VisiblePage> {
        match &self.session {
            Some(session) => session
                .view
                .visible_pages()
                .into_iter()
                .map(|(page, rect)| VisiblePage { page, rect })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Render (or fetch from cache) a page at the current zoom.
    pub fn render_page(&mut self, page: usize) -> Result<&Bitmap, AppError> {
        let session = self
            .session
            .as_mut()
            .expect("render_page called without an open document");
        let scale = session.view.zoom();
        let doc = &session.doc;
        let bitmap = session
            .cache
            .get_or_render(page, scale, || doc.render_page(page, scale))?;
        Ok(bitmap)
    }

    /// Plain text of a page (selection/search foundation, exposed for tests
    /// and upcoming features).
    pub fn page_text(&self, page: usize) -> Result<String, AppError> {
        let session = self
            .session
            .as_ref()
            .expect("page_text called without an open document");
        Ok(session.doc.page_text(page)?)
    }

    // ---- Content navigation primitives ---------------------------------

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Ensure page `page`'s navigable content is extracted and cached.
    /// Extraction failures are treated as "no content" so caret motion simply
    /// skips the page rather than erroring.
    fn ensure_content(&mut self, page: usize) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.content.contains_key(&page) {
            return;
        }
        // Learn the margins before extracting anything, so every page in this
        // session is filtered against the same evidence. Done here rather than
        // at open so that merely reading a document never pays for it — page
        // content is only ever extracted once the caret is used.
        if session.furniture.is_none() && self.config.view.skip_page_furniture {
            session.furniture = Some(session.doc.furniture_profile().unwrap_or_default());
        }
        let opts = ContentOptions {
            detect_tables: self.config.view.detect_tables,
            detect_headings: self.config.view.detect_headings,
            detect_equations: self.config.view.detect_equations,
            skip_page_furniture: self.config.view.skip_page_furniture,
            detect_footnotes: self.config.view.detect_footnotes,
        };
        let content = session
            .doc
            .page_content(page, opts, session.furniture.as_ref())
            .unwrap_or_default();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.content.insert(page, content);
    }

    /// Replace a page's extracted content, so navigation can be tested against
    /// an exact layout without depending on MuPDF's table-detection heuristic.
    #[cfg(test)]
    fn set_page_content(&mut self, page: usize, content: PageContent) {
        if let Some(session) = self.session.as_mut() {
            session.content.insert(page, content);
        }
    }

    /// Cached content lines for `page` (empty if absent/uncached).
    fn content(&self, page: usize) -> &[ContentLine] {
        self.session
            .as_ref()
            .and_then(|s| s.content.get(&page))
            .map(|c| c.lines.as_slice())
            .unwrap_or(&[])
    }

    /// Cached structural objects for `page` — every kind, atomic or not.
    fn objects(&self, page: usize) -> &[ContentObject] {
        self.session
            .as_ref()
            .and_then(|s| s.content.get(&page))
            .map(|c| c.objects.as_slice())
            .unwrap_or(&[])
    }

    /// The *region* containing `line` on `page`, extracting the page if
    /// needed: a table, an image or a heading.
    ///
    /// Regions bound sentence runs and split paragraphs. That is all a heading
    /// needs to be one step at those two scopes, and it is deliberately less
    /// than being atomic — see [`Self::atomic_object_at`].
    fn region_at(&mut self, page: usize, line: usize) -> Option<ContentObject> {
        self.ensure_content(page);
        self.objects(page)
            .iter()
            .find(|o| line >= o.start_line && line <= o.end_line)
            .copied()
    }

    /// The region containing `line`, but only if it is one stop at `scope`.
    ///
    /// The two categories differ at exactly one scope, which is the whole
    /// reason there are two. At word scope only an *atomic* kind counts, so
    /// `w` walks into a table's cells and a formula's terms while stepping
    /// over an image in one press. From line scope up a *block* counts too,
    /// so a table and an equation each become a single stop however many rows
    /// they run to. Char scope has no units at all — it is the escape hatch
    /// that reaches inside everything.
    ///
    /// Everything that moves or highlights by a unit goes through here, so
    /// this one function is where that distinction is made.
    fn unit_object_at(&mut self, page: usize, line: usize, scope: Scope) -> Option<ContentObject> {
        let object = self.region_at(page, line)?;
        let counts = match scope {
            Scope::Char => false,
            Scope::Word => object.kind.is_atomic(),
            Scope::Line | Scope::Sentence | Scope::Paragraph => object.kind.is_block(),
        };
        counts.then_some(object)
    }

    /// Identity of the region a caret sits in, for "are these two positions in
    /// the same run of content".
    fn region_id_at(&mut self, at: Caret) -> Option<ObjectId> {
        self.region_at(at.page, at.line).map(|o| ObjectId {
            page: at.page,
            start_line: o.start_line,
        })
    }

    /// Identity of the unit a caret sits in at `scope`, for "did we leave it
    /// yet" while stepping.
    fn unit_id_at(&mut self, at: Caret, scope: Scope) -> Option<ObjectId> {
        self.unit_object_at(at.page, at.line, scope)
            .map(|o| ObjectId {
                page: at.page,
                start_line: o.start_line,
            })
    }

    /// The canonical caret for an object: its first cell, or its last.
    fn object_landing(&mut self, page: usize, object: ContentObject, land: Landing) -> Caret {
        match land {
            Landing::Start => Caret {
                page,
                line: object.start_line,
                cell: 0,
            },
            Landing::End => Caret {
                page,
                line: object.end_line,
                cell: self
                    .line_cell_count(page, object.end_line)
                    .saturating_sub(1),
            },
        }
    }

    fn page_line_count(&mut self, page: usize) -> usize {
        self.ensure_content(page);
        self.content(page).len()
    }

    fn line_cell_count(&mut self, page: usize, line: usize) -> usize {
        self.ensure_content(page);
        self.content(page).get(line).map_or(0, |l| l.cells.len())
    }

    fn cell_rect(&mut self, page: usize, line: usize, cell: usize) -> Option<Rect> {
        self.ensure_content(page);
        self.content(page)
            .get(line)
            .and_then(|l| l.cells.get(cell))
            .map(|c| c.bbox)
    }

    fn nearest_cell(&mut self, page: usize, line: usize, goal_x: f32) -> usize {
        self.ensure_content(page);
        self.content(page)
            .get(line)
            .map_or(0, |l| nearest_cell_in_line(&l.cells, goal_x))
    }

    /// First page at or after `start` that has navigable content.
    fn content_page_from(&mut self, start: usize) -> Option<usize> {
        let count = self.session.as_ref()?.view.layout().page_count();
        (start..count).find(|&p| self.page_line_count(p) > 0)
    }

    /// First page strictly after `after` with content.
    fn next_content_page(&mut self, after: usize) -> Option<usize> {
        let count = self.session.as_ref()?.view.layout().page_count();
        ((after + 1)..count).find(|&p| self.page_line_count(p) > 0)
    }

    /// Last page strictly before `before` with content.
    fn prev_content_page(&mut self, before: usize) -> Option<usize> {
        (0..before).rev().find(|&p| self.page_line_count(p) > 0)
    }

    /// The first content line whose bottom edge is at or below `view_top`
    /// (document space), scanning from the page under the viewport top. Falls
    /// back to the last content line when the view is scrolled past all
    /// content; `None` only when the document has no content at all.
    fn topmost_visible_line(&mut self, view_top: f32) -> Option<(usize, usize)> {
        let page_count = self.session.as_ref()?.view.layout().page_count();
        let start = self.session.as_ref()?.view.layout().page_at_y(view_top);
        let mut fallback = None;
        for page in start..page_count {
            self.ensure_content(page);
            let page_top = self.session.as_ref()?.view.layout().page(page)?.y;
            let lines = self.content(page);
            for (line, content) in lines.iter().enumerate() {
                if content.cells.is_empty() {
                    continue;
                }
                fallback = Some((page, line));
                if page_top + content.bbox.y1 >= view_top {
                    return Some((page, line));
                }
            }
        }
        fallback
    }

    fn step_right(&mut self, caret: &mut Caret) -> bool {
        if caret.cell + 1 < self.line_cell_count(caret.page, caret.line) {
            caret.cell += 1;
            return true;
        }
        if caret.line + 1 < self.page_line_count(caret.page) {
            caret.line += 1;
            caret.cell = 0;
            return true;
        }
        if let Some(next) = self.next_content_page(caret.page) {
            *caret = Caret {
                page: next,
                line: 0,
                cell: 0,
            };
            return true;
        }
        false
    }

    fn step_left(&mut self, caret: &mut Caret) -> bool {
        if caret.cell > 0 {
            caret.cell -= 1;
            return true;
        }
        if caret.line > 0 {
            caret.line -= 1;
            caret.cell = self
                .line_cell_count(caret.page, caret.line)
                .saturating_sub(1);
            return true;
        }
        if let Some(prev) = self.prev_content_page(caret.page) {
            let line = self.page_line_count(prev).saturating_sub(1);
            caret.page = prev;
            caret.line = line;
            caret.cell = self.line_cell_count(prev, line).saturating_sub(1);
            return true;
        }
        false
    }

    fn step_down(&mut self, caret: &mut Caret, goal_x: f32) -> bool {
        if caret.line + 1 < self.page_line_count(caret.page) {
            caret.line += 1;
            caret.cell = self.nearest_cell(caret.page, caret.line, goal_x);
            return true;
        }
        if let Some(next) = self.next_content_page(caret.page) {
            caret.page = next;
            caret.line = 0;
            caret.cell = self.nearest_cell(next, 0, goal_x);
            return true;
        }
        false
    }

    fn step_up(&mut self, caret: &mut Caret, goal_x: f32) -> bool {
        if caret.line > 0 {
            caret.line -= 1;
            caret.cell = self.nearest_cell(caret.page, caret.line, goal_x);
            return true;
        }
        if let Some(prev) = self.prev_content_page(caret.page) {
            let line = self.page_line_count(prev).saturating_sub(1);
            caret.page = prev;
            caret.line = line;
            caret.cell = self.nearest_cell(prev, line, goal_x);
            return true;
        }
        false
    }

    fn word_class_at(&mut self, caret: Caret) -> Option<WordClass> {
        self.ensure_content(caret.page);
        self.content(caret.page)
            .get(caret.line)
            .and_then(|line| line.cells.get(caret.cell))
            .map(word_class)
    }

    fn next_cell(&mut self, caret: Caret) -> Option<Caret> {
        let mut next = caret;
        self.step_right(&mut next).then_some(next)
    }

    fn prev_cell(&mut self, caret: Caret) -> Option<Caret> {
        let mut prev = caret;
        self.step_left(&mut prev).then_some(prev)
    }

    fn same_word_run(&mut self, left: Caret, right: Caret) -> bool {
        let same_line = left.page == right.page && left.line == right.line;
        // A link is one word: `https://example.com/a?x=1` is a single stop, not
        // a dozen. Checked first because it is the most specific rule, and its
        // span is a hard edge in both directions, so the bracket before it and
        // the stop after it keep their own stops.
        if same_line {
            // `link_at(left)` alone is not enough once a link's own span
            // stitches across a real space (see `link_at`'s doc comment):
            // `word_run_end` steps onto that space cell as `left` on its way
            // through, and `token_span` -- correctly -- refuses to resolve a
            // whitespace cell to any token at all, so `link_at(left)` sees
            // nothing there. Falling back to `link_at(right)` picks up the
            // very same chain from the non-whitespace side instead.
            let span = self.link_at(left).or_else(|| self.link_at(right));
            if let Some(span) = span {
                let within = |c: Caret| c.cell >= span.0 && c.cell <= span.1;
                if within(left) || within(right) {
                    return within(left) && within(right);
                }
            }
        }
        // A number or dotted identifier is one word: the separator in `3.14`
        // or `VII.0` joins the alphanumeric sides, so `w` steps over the
        // whole token instead of stopping inside it.
        if same_line && (self.is_number_interior(left) || self.is_number_interior(right)) {
            return true;
        }
        // A sign written tight against a figure belongs to it: `45.5%` is one
        // word. Only the right-hand side is asked, so the sign joins backwards
        // to its number and never forwards into what follows it.
        if same_line && self.is_number_suffix_at(right) {
            return true;
        }
        // A hyphenated compound is one word: the hyphen in `well-known` joins
        // the words on either side of it, so `w` steps over the compound
        // instead of stopping at each half and at the hyphen.
        if same_line && (self.is_hyphen_interior(left) || self.is_hyphen_interior(right)) {
            return true;
        }
        // An abbreviation is one word, stops included: `e.g.` is a single stop
        // for `w`, not four. Its span is also a hard edge in both directions,
        // so the comma in `(e.g.,` is a stop of its own rather than being
        // swallowed by the punctuation run the closing stop would otherwise
        // start.
        if same_line {
            if let Some(span) = self.abbreviation_at(left) {
                let within = |c: Caret| c.cell >= span.0 && c.cell <= span.1;
                if within(left) || within(right) {
                    return within(left) && within(right);
                }
            }
        }
        // A synthetic space MuPDF guessed mid-token must not itself break a
        // number, dotted identifier or hyphenated compound: `9p4kxc2cvd.1`
        // stays one word even when MuPDF drew it as two runs with a gap
        // between them. Checked only against these specific constructs --
        // never the generic word-class fallback below -- so an ordinary
        // inter-word gap MuPDF happens to flag as synthetic still ends the
        // word exactly as a real space would. Links and abbreviations need
        // no equivalent bridge here: they compute a holistic span up front
        // via `link_at`/`abbreviation_at`, which already see through a
        // synthetic space (see `token_span`).
        if same_line {
            if self.is_synthetic_space_at(right) {
                if let Some(beyond) = self.next_cell_on_line(right) {
                    if self.is_number_interior(beyond)
                        || self.is_hyphen_interior(beyond)
                        || self.is_number_suffix_at(beyond)
                    {
                        return true;
                    }
                }
            }
            if self.is_synthetic_space_at(left) {
                if let Some(before) = self.prev_cell_on_line(left) {
                    if self.is_number_interior(before)
                        || self.is_hyphen_interior(before)
                        || self.is_number_suffix_at(before)
                    {
                        return true;
                    }
                }
            }
        }
        let Some(left_class) = self.word_class_at(left) else {
            return false;
        };
        let Some(right_class) = self.word_class_at(right) else {
            return false;
        };
        continues_word_run(left_class, right_class, same_line)
    }

    /// Whether the character at `c` is punctuation sitting inside a number or
    /// a dotted identifier: a decimal point, a grouping separator, a full stop
    /// in `VII.0` / `file.txt`, or the sign of an exponent.
    ///
    /// Only the same line counts: a figure is not carried across a line break,
    /// and treating one as though it were would join text that merely happens
    /// to end and begin with digits.
    ///
    /// Neighbours skip one synthetic-space cell so a DOI fragment drawn as
    /// `9p4kxc2cvd` + gap + `.1` still joins for word motion. Sentence
    /// boundaries use [`Self::is_number_interior_tight`] instead: any space
    /// after a stop — synthetic or authored — must end the sentence.
    fn is_number_interior(&mut self, c: Caret) -> bool {
        self.number_interior(c, true)
    }

    /// Like [`Self::is_number_interior`], but neighbours are the immediate
    /// same-line cells — a synthetic space is not peeked through.
    ///
    /// Used only by sentence-boundary detection. Looking through MuPDF's
    /// guessed gaps is correct for welding DOI fragments into one *word*, but
    /// turns an ordinary `reactor. Efficacy` into a false dotted token and
    /// swallows the sentence end.
    fn is_number_interior_tight(&mut self, c: Caret) -> bool {
        self.number_interior(c, false)
    }

    fn number_interior(&mut self, c: Caret, skip_synthetic: bool) -> bool {
        let Some(here) = self.char_at(c) else {
            return false;
        };
        if !(is_numeric_separator(here) || is_exponent_sign(here)) {
            return false;
        }
        let prev = if skip_synthetic {
            self.prev_real_cell_on_line(c)
        } else {
            self.prev_cell_on_line(c)
        };
        let before = prev.and_then(|p| self.char_at(p));
        let after = if skip_synthetic {
            self.next_real_cell_on_line(c)
        } else {
            self.next_cell_on_line(c)
        }
        .and_then(|n| self.char_at(n));
        if is_inside_number(before, here, after) || is_inside_dotted_token(before, here, after) {
            return true;
        }
        // `2.3E+5`: the sign needs the exponent marker behind it and a digit
        // behind that, which is what tells a number from `cache+1`.
        let before2 = prev
            .and_then(|p| {
                if skip_synthetic {
                    self.prev_real_cell_on_line(p)
                } else {
                    self.prev_cell_on_line(p)
                }
            })
            .and_then(|p| self.char_at(p));
        is_inside_scientific_exponent(before2, before, here, after)
    }

    /// Whether the character at `c` is a sign attached to the figure in front
    /// of it, as in `45.5%`.
    fn is_number_suffix_at(&mut self, c: Caret) -> bool {
        let Some(here) = self.char_at(c) else {
            return false;
        };
        if !is_number_suffix(here) {
            return false;
        }
        let before = self.prev_real_cell_on_line(c).and_then(|p| self.char_at(p));
        is_attached_number_suffix(before, here)
    }

    /// Whether the character at `c` is a hyphen joining a compound word.
    ///
    /// Same-line only, for the same reason as [`Self::is_number_interior`]: a
    /// word broken across a line break is still two stops, since the hyphen
    /// that broke it belongs to the typesetting, not to the word.
    fn is_hyphen_interior(&mut self, c: Caret) -> bool {
        let Some(here) = self.char_at(c) else {
            return false;
        };
        if !is_word_hyphen(here) {
            return false;
        }
        let before = self.prev_real_cell_on_line(c).and_then(|p| self.char_at(p));
        let after = self.next_real_cell_on_line(c).and_then(|n| self.char_at(n));
        is_inside_hyphenated_word(before, here, after)
    }

    /// The inclusive cell range of the whitespace-delimited token containing
    /// `c`, or `None` when `c` is itself whitespace.
    ///
    /// The unit both token-level rules work from: an abbreviation and a link
    /// are each recognised by looking at a whole token rather than at the
    /// characters beside one cell.
    ///
    /// Every whitespace cell ends a token here, synthetic or not — a
    /// synthetic space (MuPDF's own guess at a gap inside a URL/DOI drawn as
    /// separate positioning runs) is not treated as invisible at this stage.
    /// It was, once, and that was wrong: whichever ordinary word or
    /// punctuation happened to sit *before* a synthetic gap got welded onto
    /// the front of whatever came after it (`files:` glued straight onto
    /// `https://doi`, corrupting the very scheme `link_span` needs to see).
    /// [`Self::link_at`] is where a synthetic — or a real, authored — gap
    /// gets bridged instead, one token at a time, each step re-validated by
    /// an actual `link_span` match rather than assumed.
    fn token_span(&mut self, c: Caret) -> Option<(usize, usize)> {
        self.ensure_content(c.page);
        let cells = self.content(c.page).get(c.line)?.cells.as_slice();
        if c.cell >= cells.len() {
            return None;
        }
        let is_space =
            |cell: &syodep_pdf::Cell| matches!(cell.kind, CellKind::Char(ch) if ch.is_whitespace());
        if is_space(&cells[c.cell]) {
            return None;
        }
        let start = cells[..c.cell]
            .iter()
            .rposition(is_space)
            .map_or(0, |i| i + 1);
        let end = cells[c.cell..]
            .iter()
            .position(is_space)
            .map_or(cells.len() - 1, |n| c.cell + n - 1);
        Some((start, end))
    }

    /// The inclusive cell range of the link inside the token containing `c`, if
    /// there is one.
    ///
    /// Like [`Self::abbreviation_at`] this is a span the caller must test `c`
    /// against: the brackets and the sentence-ending stop around a link are not
    /// part of it. [`link_span`] does the recognising; the mapping back to cells
    /// goes through the characters actually present, so an image inside a token
    /// cannot shift the result.
    ///
    /// A recognised link is also extended across a document's own literal
    /// mid-address spacing: some PDFs draw a URL's path segments with a real,
    /// PDF-authored space character between them (not MuPDF's synthetic
    /// guess, which `token_span` already sees through) — a DOI rendered as
    /// `https://doi .org /10 .17632 /9p4kxc2cvd .1`, say, every one of those
    /// gaps a genuine space in the content stream, geometrically identical to
    /// an ordinary word space beside it, so neither `synthetic` nor width can
    /// tell them apart. [`Self::continuing_link_fragment`] is what keeps this
    /// safe: a following token only ever gets pulled in when it starts with
    /// `.` or `/` *and* the longer, concatenated string still recognises as
    /// one link reaching the same far end — so ordinary prose separated by
    /// real spaces is never at risk, only fragments shaped like a URL
    /// continuation that `link_span` itself agrees with.
    ///
    /// `c` may land in *any* fragment of a stitched chain, not just its
    /// first — `w` walking forward asks `same_word_run` about the boundary
    /// between `.org` and the space before `/10`, for instance, which needs
    /// the same answer as asking from `https://doi` itself. So the first
    /// phase below walks backward with [`Self::preceding_link_fragment`] (the
    /// mirror image of the forward one) to find the chain's true start before
    /// the forward extension runs from there — resolving to the same span no
    /// matter which fragment `c` was in.
    fn link_at(&mut self, c: Caret) -> Option<(usize, usize)> {
        let (mut start, mut end) = self.token_span(c)?;
        for _ in 0..MAX_LINK_FRAGMENTS {
            let Some((prev_start, prev_end)) = self.preceding_link_fragment(c.page, c.line, start)
            else {
                break;
            };
            start = prev_start;
            end = prev_end;
        }

        let mut best: Option<(usize, usize)> = None;
        for _ in 0..MAX_LINK_FRAGMENTS {
            self.ensure_content(c.page);
            let cells = self.content(c.page).get(c.line)?.cells.as_slice();
            // A synthetic space is dropped from the string handed to
            // `link_span` -- MuPDF's guessed gap, not an authored character,
            // and `is_dotted_host`/`is_url_scheme` reject whitespace
            // outright. So is a real space bridged in by the loop below: it
            // already passed the shape gate in `continuing_link_fragment`,
            // and dropping it here is what lets the concatenation still read
            // as one continuous address. The cell range returned still spans
            // every gap, so the recognised link's selection/highlight has no
            // hole in it.
            let indexed: Vec<(usize, char)> = (start..=end)
                .filter_map(|i| match cells[i].kind {
                    CellKind::Char(ch) if !ch.is_whitespace() => Some((i, ch)),
                    _ => None,
                })
                .collect();
            let token: String = indexed.iter().map(|(_, ch)| ch).collect();
            let Some((lo, hi)) = link_span(&token) else {
                break;
            };
            let span = (indexed[lo].0, indexed[hi].0);
            best = Some(span);
            // Only keep extending while the match reaches the very end of
            // what has been gathered so far -- trailing punctuation already
            // closed the link otherwise, and reaching past it would be wrong.
            if span.1 != end {
                break;
            }
            let Some((_, next_end)) = self.continuing_link_fragment(c.page, c.line, end) else {
                break;
            };
            end = next_end;
        }
        best
    }

    /// If a space (synthetic or real — either can sit between a URL's own
    /// segments, see [`Self::link_at`]) immediately follows the cell at
    /// `end`, and the token beyond it opens with `.` or `/` — the shape of
    /// every continuation seen in practice (`.com`, `/njoy`, `.17632`) and
    /// not of an ordinary next word — the span of that following token.
    /// Consulted only by [`Self::link_at`]'s extension loop, which is also
    /// what keeps this safe despite not caring *why* there is a gap here:
    /// the shape gate only ever admits a candidate, and `link_at` re-runs
    /// `link_span` on the result before trusting it.
    fn continuing_link_fragment(
        &mut self,
        page: usize,
        line: usize,
        end: usize,
    ) -> Option<(usize, usize)> {
        self.ensure_content(page);
        let cells = self.content(page).get(line)?.cells.as_slice();
        let is_space =
            |cell: &syodep_pdf::Cell| matches!(cell.kind, CellKind::Char(ch) if ch.is_whitespace());
        if !is_space(cells.get(end + 1)?) {
            return None;
        }
        let next_start = end + 2;
        let next = cells.get(next_start)?;
        if !matches!(next.kind, CellKind::Char('.') | CellKind::Char('/')) {
            return None;
        }
        let next_end = cells[next_start..]
            .iter()
            .position(is_space)
            .map_or(cells.len() - 1, |n| next_start + n - 1);
        Some((next_start, next_end))
    }

    /// Symmetric with [`Self::continuing_link_fragment`], searching
    /// backward: if the token starting at `start` itself opens with `.` or
    /// `/`, and a space immediately precedes it, the span of the token
    /// before that space. Consulted only by [`Self::link_at`]'s backward
    /// phase, which is what makes it resolve to the same span no matter
    /// which fragment of a stitched chain `c` lands in.
    fn preceding_link_fragment(
        &mut self,
        page: usize,
        line: usize,
        start: usize,
    ) -> Option<(usize, usize)> {
        self.ensure_content(page);
        let cells = self.content(page).get(line)?.cells.as_slice();
        if !matches!(
            cells.get(start)?.kind,
            CellKind::Char('.') | CellKind::Char('/')
        ) {
            return None;
        }
        let is_space =
            |cell: &syodep_pdf::Cell| matches!(cell.kind, CellKind::Char(ch) if ch.is_whitespace());
        let space_idx = start.checked_sub(1)?;
        if !is_space(cells.get(space_idx)?) {
            return None;
        }
        let prev_start = cells[..space_idx]
            .iter()
            .rposition(is_space)
            .map_or(0, |i| i + 1);
        Some((prev_start, space_idx - 1))
    }

    /// Whether `c` sits inside a link, where a stop is part of the address
    /// rather than the end of a sentence.
    fn is_inside_link(&mut self, c: Caret) -> bool {
        self.link_at(c)
            .is_some_and(|(start, end)| c.cell >= start && c.cell <= end)
    }

    /// The inclusive cell range of the abbreviation inside the
    /// whitespace-delimited token containing `c`, if there is one.
    ///
    /// This is what makes `e.g.` one word and one sentence: the same span
    /// answers both questions, so word runs and sentence runs cannot disagree
    /// about where the construct begins and ends.
    ///
    /// The span is the abbreviation itself, not the whole token: the brackets
    /// and commas around `(e.g.,` are punctuation the writer put *beside* the
    /// construct, so they are trimmed off before the token is recognised and
    /// they keep their own word stops afterwards. Note that `c` may sit outside
    /// the returned span — every caller checks.
    fn abbreviation_at(&mut self, c: Caret) -> Option<(usize, usize)> {
        let (mut start, mut end) = self.token_span(c)?;
        self.ensure_content(c.page);
        let cells = self.content(c.page).get(c.line)?.cells.as_slice();
        let char_of = |cell: &syodep_pdf::Cell| match cell.kind {
            CellKind::Char(ch) => Some(ch),
            CellKind::Image => None,
        };
        // Trim the punctuation wrapped around the construct. Leading: anything
        // that is not a word character, so `(`, `[` and quotes go. Trailing:
        // the same, except a full stop, which may be the abbreviation's own.
        let wrapper = |ch: char| !(ch.is_alphanumeric() || ch == '_');
        while start <= end && matches!(char_of(&cells[start]), Some(ch) if wrapper(ch)) {
            start += 1;
        }
        while end > start && matches!(char_of(&cells[end]), Some(ch) if wrapper(ch) && ch != '.') {
            end -= 1;
        }
        if start > end {
            return None;
        }
        let token: String = cells[start..=end].iter().filter_map(char_of).collect();
        is_abbreviation(&token).then_some((start, end))
    }

    /// Whether the stop at `c` closes an abbreviation without ending the
    /// sentence it sits in.
    fn is_abbreviation_stop(&mut self, c: Caret) -> bool {
        let Some((start, end)) = self.abbreviation_at(c) else {
            return false;
        };
        // Punctuation beside the construct is not part of it: the `)` closing
        // `(etc.)` ends the sentence exactly as it would anywhere else.
        if c.cell < start || c.cell > end {
            return false;
        }
        // A stop *inside* the construct never ends anything.
        if c.cell < end {
            return true;
        }
        // The closing stop does, but only when a new sentence follows it.
        let mut following = String::new();
        let mut cur = c;
        for _ in 0..8 {
            match self.next_cell_same_page(cur) {
                Some(next) => {
                    cur = next;
                    match self.char_at(cur) {
                        Some(ch) => following.push(ch),
                        None => break,
                    }
                }
                None => return false,
            }
        }
        !opens_a_sentence(&following)
    }

    fn next_cell_on_line(&mut self, c: Caret) -> Option<Caret> {
        self.next_cell(c)
            .filter(|n| n.page == c.page && n.line == c.line)
    }

    fn prev_cell_on_line(&mut self, c: Caret) -> Option<Caret> {
        self.prev_cell(c)
            .filter(|p| p.page == c.page && p.line == c.line)
    }

    /// Whether the cell at `c` is a space MuPDF guessed rather than an
    /// authored character — see [`syodep_pdf::Cell::synthetic`].
    fn is_synthetic_space_at(&mut self, c: Caret) -> bool {
        self.ensure_content(c.page);
        self.content(c.page)
            .get(c.line)
            .and_then(|l| l.cells.get(c.cell))
            .is_some_and(|cell| cell.synthetic)
    }

    /// Like [`Self::next_cell_on_line`], but a single synthetic-space cell is
    /// stepped past so the true next character is visible. This is what lets
    /// `9p4kxc2cvd` see the `.` two cells away in `9p4kxc2cvd<synthetic
    /// space>.1` — a gap MuPDF guessed while assembling a URL/DOI drawn as
    /// separate positioning runs, not a real word boundary.
    fn next_real_cell_on_line(&mut self, c: Caret) -> Option<Caret> {
        let n = self.next_cell_on_line(c)?;
        if self.is_synthetic_space_at(n) {
            self.next_cell_on_line(n)
        } else {
            Some(n)
        }
    }

    /// Symmetric with [`Self::next_real_cell_on_line`].
    fn prev_real_cell_on_line(&mut self, c: Caret) -> Option<Caret> {
        let p = self.prev_cell_on_line(c)?;
        if self.is_synthetic_space_at(p) {
            self.prev_cell_on_line(p)
        } else {
            Some(p)
        }
    }

    fn next_word_target_from(&mut self, mut caret: Caret) -> Option<Caret> {
        loop {
            if is_word_target(self.word_class_at(caret)?) {
                return Some(caret);
            }
            caret = self.next_cell(caret)?;
        }
    }

    fn prev_word_target_from(&mut self, mut caret: Caret) -> Option<Caret> {
        loop {
            if is_word_target(self.word_class_at(caret)?) {
                return Some(caret);
            }
            caret = self.prev_cell(caret)?;
        }
    }

    fn word_run_end(&mut self, mut caret: Caret) -> Caret {
        while let Some(next) = self.next_cell(caret) {
            if !self.same_word_run(caret, next) {
                break;
            }
            caret = next;
        }
        caret
    }

    fn word_run_start(&mut self, mut caret: Caret) -> Caret {
        while let Some(prev) = self.prev_cell(caret) {
            if !self.same_word_run(prev, caret) {
                break;
            }
            caret = prev;
        }
        caret
    }

    fn step_next_word_start(&mut self, caret: &mut Caret) -> bool {
        let Some(current_class) = self.word_class_at(*caret) else {
            return false;
        };
        let mut pos = *caret;
        if is_word_target(current_class) {
            loop {
                let Some(next) = self.next_cell(pos) else {
                    return false;
                };
                pos = next;
                if !self.same_word_run(*caret, pos) {
                    break;
                }
            }
        } else {
            let Some(next) = self.next_cell(pos) else {
                return false;
            };
            pos = next;
        }
        let Some(target) = self.next_word_target_from(pos) else {
            return false;
        };
        *caret = target;
        true
    }

    fn step_prev_word_start(&mut self, caret: &mut Caret) -> bool {
        let Some(current_class) = self.word_class_at(*caret) else {
            return false;
        };
        let target = if is_word_target(current_class) {
            let start = self.word_run_start(*caret);
            if start != *caret {
                *caret = start;
                return true;
            }
            let Some(prev) = self.prev_cell(*caret) else {
                return false;
            };
            self.prev_word_target_from(prev)
        } else {
            let Some(prev) = self.prev_cell(*caret) else {
                return false;
            };
            self.prev_word_target_from(prev)
        };
        let Some(target) = target else {
            return false;
        };
        *caret = self.word_run_start(target);
        true
    }

    // ---- Line motion ---------------------------------------------------

    /// Bounding box of a content line in page points (`None` if absent).
    fn line_bbox(&mut self, page: usize, line: usize) -> Option<Rect> {
        self.ensure_content(page);
        self.content(page).get(line).map(|l| l.bbox)
    }

    fn line_step_down(&mut self, mark: &mut LineMark) -> bool {
        if mark.line + 1 < self.page_line_count(mark.page) {
            mark.line += 1;
            return true;
        }
        if let Some(next) = self.next_content_page(mark.page) {
            mark.page = next;
            mark.line = 0;
            return true;
        }
        false
    }

    fn line_step_up(&mut self, mark: &mut LineMark) -> bool {
        if mark.line > 0 {
            mark.line -= 1;
            return true;
        }
        if let Some(prev) = self.prev_content_page(mark.page) {
            mark.page = prev;
            mark.line = self.page_line_count(prev).saturating_sub(1);
            return true;
        }
        false
    }

    /// Move the mark to the adjacent column on the same page, landing on the line
    /// nearest `goal_y`. A no-op (returns `false`) on single-column pages or when
    /// already in the edge column toward `forward`.
    fn line_step_column(&mut self, mark: &mut LineMark, goal_y: f32, forward: bool) -> bool {
        self.ensure_content(mark.page);
        let lines = self.content(mark.page);
        let cols = column_ranges(lines);
        if cols.len() < 2 {
            return false;
        }
        let Some(cur_box) = lines.get(mark.line).map(|l| l.bbox) else {
            return false;
        };
        let Some(cur_col) = column_index_of(&cols, cur_box.x0, cur_box.x1) else {
            return false;
        };
        let target = if forward {
            cur_col + 1
        } else {
            cur_col.checked_sub(1).unwrap_or(usize::MAX)
        };
        if target >= cols.len() {
            return false;
        }
        let candidates: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                !l.cells.is_empty() && column_index_of(&cols, l.bbox.x0, l.bbox.x1) == Some(target)
            })
            .map(|(i, _)| i)
            .collect();
        if candidates.is_empty() {
            return false;
        }
        mark.line = nearest_line_in_column(lines, &candidates, goal_y);
        true
    }

    // ---- Word motion ---------------------------------------------------

    /// Build a word-focus mark from a landed caret cell by expanding it to the
    /// full word run on that line (reusing the caret word-run helpers).
    fn word_mark_from_caret(&mut self, caret: Caret) -> WordMark {
        let start = self.word_run_start(caret);
        let end = self.word_run_end(caret);
        WordMark {
            page: caret.page,
            line: caret.line,
            start_cell: start.cell,
            end_cell: end.cell,
        }
    }

    /// A caret at the mark's first cell — the representative position used to
    /// drive the shared caret motion helpers.
    fn word_mark_caret(mark: WordMark) -> Caret {
        Caret {
            page: mark.page,
            line: mark.line,
            cell: mark.start_cell,
        }
    }

    /// Move the mark one line up or down, landing on the word nearest `goal_x`.
    fn word_step_vertical(&mut self, mark: &mut WordMark, goal_x: f32, down: bool) -> bool {
        let mut caret = Self::word_mark_caret(*mark);
        let moved = if down {
            self.step_down(&mut caret, goal_x)
        } else {
            self.step_up(&mut caret, goal_x)
        };
        if !moved {
            return false;
        }
        *mark = self.word_mark_from_caret(caret);
        true
    }

    // ---- Sentence motion -----------------------------------------------

    /// One cell forward/backward but only within the same page (sentences never
    /// straddle a page). Thin filters over [`Self::next_cell`]/[`Self::prev_cell`].
    fn next_cell_same_page(&mut self, c: Caret) -> Option<Caret> {
        self.next_cell(c).filter(|n| n.page == c.page)
    }

    fn prev_cell_same_page(&mut self, c: Caret) -> Option<Caret> {
        self.prev_cell(c).filter(|p| p.page == c.page)
    }

    /// The character at `c`, or `None` for an image or absent cell.
    fn char_at(&mut self, c: Caret) -> Option<char> {
        self.ensure_content(c.page);
        match self
            .content(c.page)
            .get(c.line)
            .and_then(|l| l.cells.get(c.cell))
            .map(|cell| cell.kind)
        {
            Some(CellKind::Char(ch)) => Some(ch),
            _ => None,
        }
    }

    /// Whether `c` is the last cell of a sentence-ending group — a maximal run
    /// of terminators (`. ! ?`) plus any trailing closing quotes/brackets. The
    /// group must contain at least one terminator, so a lone closing bracket is
    /// not a boundary. Analogue of [`Self::same_word_run`] for sentences.
    fn sentence_boundary_after(&mut self, c: Caret) -> bool {
        // A heading or an equation is exactly one sentence, whatever punctuation
        // it contains: `3.1. Methods` would otherwise be three and `f(x) = 0.`
        // two. The run still stops at their edges, because those are region
        // boundaries.
        if self.in_single_sentence_region(c) {
            return false;
        }
        // Nor inside a list item's own marker: `1. First point` is one
        // sentence, not an enumerator followed by a sentence.
        if self.in_list_marker(c) {
            return false;
        }
        // A colon that ends its line (optional trailing spaces only) is a
        // sentence boundary: "A lead-in:\nContinued." is two sentences. A
        // mid-line colon stays inert. Links keep their own rule below.
        if self.char_at(c) == Some(':') {
            self.ensure_content(c.page);
            let cells = self
                .content(c.page)
                .get(c.line)
                .map(|l| l.cells.as_slice())
                .unwrap_or(&[]);
            if is_line_final_colon(cells, c.cell) && !self.is_inside_link(c) {
                return true;
            }
        }
        let here = match self.char_at(c) {
            Some(ch) if is_sentence_terminator(ch) || is_sentence_trailer(ch) => ch,
            _ => return false,
        };
        // A full stop with alphanumeric sides is inside a number or dotted
        // identifier, not the end of anything: `3.14` and `ENDF/B-VII.0` sit
        // in the middle of a sentence. The stop in `costs 3.` still ends it,
        // because nothing follows. Immediate neighbours only — peeking through
        // a synthetic space would swallow `reactor. Efficacy` on IOP layouts.
        if self.is_number_interior_tight(c) {
            return false;
        }
        // Nor a stop belonging to `e.g.`, `Fig.` and friends. The stops inside
        // such a construct are inert; the one closing it ends a sentence only
        // when a new one visibly follows.
        if self.is_abbreviation_stop(c) {
            return false;
        }
        // Nor one inside a link, where it is part of the address:
        // `example.com/a.html` is not two sentences. The stop *after* a link is
        // outside its span and still ends one.
        if self.is_inside_link(c) {
            return false;
        }
        // The group must end at `c`: the next cell cannot continue it.
        if let Some(next) = self.next_cell_same_page(c) {
            if matches!(self.char_at(next), Some(ch) if is_sentence_terminator(ch) || is_sentence_trailer(ch))
            {
                return false;
            }
        }
        if is_sentence_terminator(here) {
            return true;
        }
        // `here` is a trailer: the group is only a boundary if a terminator
        // precedes it through the run of terminators/trailers.
        let mut cur = c;
        while let Some(prev) = self.prev_cell_same_page(cur) {
            match self.char_at(prev) {
                Some(ch) if is_sentence_terminator(ch) => return true,
                Some(ch) if is_sentence_trailer(ch) => cur = prev,
                _ => break,
            }
        }
        false
    }

    /// Advance over whitespace/empty cells to the first real content cell at or
    /// after `from`, staying on the page.
    fn skip_whitespace_forward(&mut self, from: Caret) -> Caret {
        let mut cur = from;
        loop {
            match self.word_class_at(cur) {
                Some(WordClass::Whitespace) | None => match self.next_cell_same_page(cur) {
                    Some(next) => cur = next,
                    None => return cur,
                },
                Some(_) => return cur,
            }
        }
    }

    /// Last cell of the sentence containing `from` (walk forward to a boundary
    /// or the page edge). Analogue of [`Self::word_run_end`].
    fn sentence_run_end(&mut self, from: Caret) -> Caret {
        let mut cur = from;
        loop {
            if self.sentence_boundary_after(cur) {
                return cur;
            }
            match self.next_cell_in_region(cur) {
                Some(next) => cur = next,
                None => return cur,
            }
        }
    }

    /// First (non-whitespace) cell of the sentence containing `from` (walk back
    /// until just after the previous boundary, then skip leading whitespace).
    /// Analogue of [`Self::word_run_start`].
    fn sentence_run_start(&mut self, from: Caret) -> Caret {
        let mut cur = from;
        while let Some(prev) = self.prev_cell_in_region(cur) {
            if self.sentence_boundary_after(prev) {
                break;
            }
            cur = prev;
        }
        self.skip_whitespace_forward(cur)
    }

    /// Whether two positions lie in the same atomic region — both outside every
    /// object, or both inside the same one.
    fn same_region(&mut self, a: Caret, b: Caret) -> bool {
        self.region_id_at(a) == self.region_id_at(b)
    }

    /// Whether `at` sits inside a region that is one sentence whatever
    /// punctuation it contains — a heading or an equation.
    fn in_single_sentence_region(&mut self, at: Caret) -> bool {
        self.region_at(at.page, at.line)
            .is_some_and(|o| o.kind.is_one_sentence())
    }

    /// Whether `at` sits inside a footnote block.
    ///
    /// Consulted only by the Sentence/Paragraph *auto-search* loops below
    /// (`step_next_sentence_start`, `step_prev_sentence_start`,
    /// `first_sentence_start_on_page`, `paragraph_step_next`,
    /// `paragraph_step_prev`), which treat a footnote as invisible — reading
    /// through a page's body prose with `s`/`p` never lands on one, the same
    /// as page furniture. It is never consulted by `sentence_run_start`/
    /// `sentence_run_end`, so a caret placed inside a footnote deliberately
    /// (word, char or line motion — a footnote stays in `content.lines`,
    /// unlike furniture) still expands and steps through its sentences
    /// normally once there.
    fn in_footnote(&mut self, at: Caret) -> bool {
        self.region_at(at.page, at.line)
            .is_some_and(|o| o.kind == ObjectKind::Footnote)
    }

    /// Whether `at` falls within the marker that opens a list item.
    ///
    /// The marker is exactly the line's first token: detection only accepts a
    /// marker that is followed by a space or is the whole line, so on a line it
    /// accepted, the first token *is* the marker. That means no second copy of
    /// the marker grammar has to live here, and no mapping between character
    /// and cell indices — which would diverge the moment a bullet were drawn as
    /// an image.
    fn in_list_marker(&mut self, at: Caret) -> bool {
        let starts_item = self
            .region_at(at.page, at.line)
            .is_some_and(|o| o.kind == ObjectKind::ListItem && at.line == o.start_line);
        if !starts_item {
            return false;
        }
        let cells = self
            .content(at.page)
            .get(at.line)
            .map(|l| l.cells.as_slice());
        let Some(cells) = cells else { return false };
        let is_space =
            |cell: &syodep_pdf::Cell| matches!(cell.kind, CellKind::Char(c) if c.is_whitespace());
        let first = cells.iter().position(|c| !is_space(c)).unwrap_or(0);
        let past = cells[first..]
            .iter()
            .position(is_space)
            .map_or(cells.len(), |n| first + n);
        at.cell < past
    }

    /// [`Self::next_cell_same_page`], additionally stopping at the edge of a
    /// table or image. A table cell rarely ends in `.`, so without this a
    /// sentence starting in the prose above a table would run right through it.
    /// Only the *expansion* of a sentence is bounded this way; searching for the
    /// next sentence still crosses freely, so a table simply becomes a sentence
    /// of its own.
    fn next_cell_in_region(&mut self, c: Caret) -> Option<Caret> {
        self.next_cell_same_page(c)
            .filter(|n| self.same_region(c, *n))
    }

    fn prev_cell_in_region(&mut self, c: Caret) -> Option<Caret> {
        self.prev_cell_same_page(c)
            .filter(|p| self.same_region(c, *p))
    }

    /// Build a [`SentenceMark`] for the sentence containing `caret`.
    fn sentence_mark_from_caret(&mut self, caret: Caret) -> SentenceMark {
        let start = self.sentence_run_start(caret);
        let end = self.sentence_run_end(caret);
        SentenceMark {
            page: caret.page,
            start_line: start.line,
            start_cell: start.cell,
            end_line: end.line,
            end_cell: end.cell,
        }
    }

    /// First content cell of `page` (skipping leading whitespace/empty lines).
    fn first_sentence_start_on_page(&mut self, page: usize) -> Option<Caret> {
        let mut cur = Caret {
            page,
            line: 0,
            cell: 0,
        };
        loop {
            if self.in_footnote(cur) {
                cur = self.next_cell_same_page(cur)?;
                continue;
            }
            match self.word_class_at(cur) {
                Some(WordClass::Whitespace) | None => {
                    cur = self.next_cell_same_page(cur)?;
                }
                Some(_) => return Some(cur),
            }
        }
    }

    /// Start cell of the next sentence on the same page, or `None` at the page
    /// edge (the caller then crosses to the next content page).
    fn step_next_sentence_start(&mut self, caret: Caret) -> Option<Caret> {
        let end = self.sentence_run_end(caret);
        let mut cur = self.next_cell_same_page(end)?;
        loop {
            if self.in_footnote(cur) {
                cur = self.next_cell_same_page(cur)?;
                continue;
            }
            match self.word_class_at(cur) {
                Some(WordClass::Whitespace) | None => {
                    cur = self.next_cell_same_page(cur)?;
                }
                Some(_) => return Some(cur),
            }
        }
    }

    /// Start cell of the previous sentence on the same page, or `None` at the
    /// page start.
    fn step_prev_sentence_start(&mut self, caret: Caret) -> Option<Caret> {
        let start = self.sentence_run_start(caret);
        let mut cur = self.prev_cell_same_page(start)?;
        // Skip whitespace back into the previous sentence, then expand it.
        // A footnote is skipped the same way -- invisible to this search,
        // even though it is real prose once a caret sits inside it.
        while matches!(self.word_class_at(cur), Some(WordClass::Whitespace) | None)
            || self.in_footnote(cur)
        {
            cur = self.prev_cell_same_page(cur)?;
        }
        Some(self.sentence_run_start(cur))
    }

    fn sentence_step_next(&mut self, mark: &mut SentenceMark) -> bool {
        let caret = Caret {
            page: mark.page,
            line: mark.start_line,
            cell: mark.start_cell,
        };
        if let Some(next) = self.step_next_sentence_start(caret) {
            *mark = self.sentence_mark_from_caret(next);
            return true;
        }
        if let Some(page) = self.next_content_page(mark.page) {
            if let Some(start) = self.first_sentence_start_on_page(page) {
                *mark = self.sentence_mark_from_caret(start);
                return true;
            }
        }
        false
    }

    fn sentence_step_prev(&mut self, mark: &mut SentenceMark) -> bool {
        let caret = Caret {
            page: mark.page,
            line: mark.start_line,
            cell: mark.start_cell,
        };
        if let Some(prev) = self.step_prev_sentence_start(caret) {
            *mark = self.sentence_mark_from_caret(prev);
            return true;
        }
        if let Some(page) = self.prev_content_page(mark.page) {
            // Last sentence of the previous page: expand from its last cell.
            let last_line = self.page_line_count(page).saturating_sub(1);
            let last_cell = self.line_cell_count(page, last_line).saturating_sub(1);
            let from = Caret {
                page,
                line: last_line,
                cell: last_cell,
            };
            *mark = self.sentence_mark_from_caret(from);
            return true;
        }
        false
    }

    // ---- Paragraph motion ----------------------------------------------

    /// A page's paragraphs, cut so that no paragraph straddles an object that
    /// stands alone as one.
    ///
    /// List items are the exception: a list is a single paragraph made of many
    /// items, so `p` skips the whole list while `s` walks it item by item.
    fn page_paragraphs(&mut self, page: usize) -> Vec<(usize, usize)> {
        self.ensure_content(page);
        let segs = paragraph_segments(self.content(page));
        let splitting: Vec<ContentObject> = self
            .objects(page)
            .iter()
            .filter(|o| o.kind.splits_paragraphs())
            .copied()
            .collect();
        split_segments_at_objects(&segs, &splitting)
    }

    /// The paragraph (segment of lines) that contains `line` on `page`.
    fn paragraph_mark_containing(&mut self, page: usize, line: usize) -> Option<ParagraphMark> {
        let segs = self.page_paragraphs(page);
        segs.iter()
            .find(|(s, e)| *s <= line && line <= *e)
            .or_else(|| segs.last())
            .map(|&(s, e)| ParagraphMark {
                page,
                start_line: s,
                end_line: e,
            })
    }

    fn paragraph_step_next(&mut self, mark: &mut ParagraphMark) -> bool {
        self.ensure_content(mark.page);
        let segs = self.page_paragraphs(mark.page);
        if let Some(i) = segs
            .iter()
            .position(|&(s, e)| s <= mark.start_line && mark.start_line <= e)
        {
            // A footnote's own segment is skipped entirely, never a stop of
            // its own — invisible to this search the same way a footnote is
            // invisible to sentence auto-search.
            for &(s, e) in &segs[i + 1..] {
                if self.in_footnote(Caret {
                    page: mark.page,
                    line: s,
                    cell: 0,
                }) {
                    continue;
                }
                *mark = ParagraphMark {
                    page: mark.page,
                    start_line: s,
                    end_line: e,
                };
                return true;
            }
        }
        let mut page = mark.page;
        while let Some(next_page) = self.next_content_page(page) {
            self.ensure_content(next_page);
            let segs = self.page_paragraphs(next_page);
            for &(s, e) in &segs {
                if self.in_footnote(Caret {
                    page: next_page,
                    line: s,
                    cell: 0,
                }) {
                    continue;
                }
                *mark = ParagraphMark {
                    page: next_page,
                    start_line: s,
                    end_line: e,
                };
                return true;
            }
            page = next_page;
        }
        false
    }

    fn paragraph_step_prev(&mut self, mark: &mut ParagraphMark) -> bool {
        self.ensure_content(mark.page);
        let segs = self.page_paragraphs(mark.page);
        if let Some(i) = segs
            .iter()
            .position(|&(s, e)| s <= mark.start_line && mark.start_line <= e)
        {
            for &(s, e) in segs[..i].iter().rev() {
                if self.in_footnote(Caret {
                    page: mark.page,
                    line: s,
                    cell: 0,
                }) {
                    continue;
                }
                *mark = ParagraphMark {
                    page: mark.page,
                    start_line: s,
                    end_line: e,
                };
                return true;
            }
        }
        let mut page = mark.page;
        while let Some(prev_page) = self.prev_content_page(page) {
            self.ensure_content(prev_page);
            let segs = self.page_paragraphs(prev_page);
            for &(s, e) in segs.iter().rev() {
                if self.in_footnote(Caret {
                    page: prev_page,
                    line: s,
                    cell: 0,
                }) {
                    continue;
                }
                *mark = ParagraphMark {
                    page: prev_page,
                    start_line: s,
                    end_line: e,
                };
                return true;
            }
            page = prev_page;
        }
        false
    }

    // ---- Shared scope machinery ----------------------------------------
    //
    // Focus and visual are the same idea at different arities: focus holds one
    // position, visual holds two. Everything that depends on the *scope* rather
    // than on how many positions there are lives here, so a scope cannot mean
    // one thing in one mode and something else in the other.

    /// The caret a highlight should start from when there is no remembered
    /// position: the top-most content line in the viewport below the scroll-off
    /// buffer, falling back to the first content line of the document.
    fn entry_caret(&mut self) -> Option<Caret> {
        let from_visible = if let Some(view_top) = self.viewport_content_top() {
            self.topmost_visible_line(view_top)
                .map(|(page, line)| Caret {
                    page,
                    line,
                    cell: 0,
                })
        } else {
            None
        };
        from_visible.or_else(|| {
            let start = self.current_page();
            self.content_page_from(start).map(|page| Caret {
                page,
                line: 0,
                cell: 0,
            })
        })
    }

    /// The inclusive cell range `at` occupies under `scope`.
    ///
    /// This is the whole of the per-scope highlight behavior. A focus highlight
    /// is the degenerate case of a selection — both ends at `at` — which is why
    /// one function serves both modes.
    fn scope_span(&mut self, at: Caret, scope: Scope) -> (Caret, Caret) {
        // A unit is covered whole, which is also what makes its highlight
        // collapse to one box rather than a strip per row.
        if let Some(object) = self.unit_object_at(at.page, at.line, scope) {
            return (
                self.object_landing(at.page, object, Landing::Start),
                self.object_landing(at.page, object, Landing::End),
            );
        }
        match scope {
            Scope::Char => (at, at),
            Scope::Word => (self.word_run_start(at), self.word_run_end(at)),
            Scope::Line => {
                let last = self.line_cell_count(at.page, at.line).saturating_sub(1);
                (Caret { cell: 0, ..at }, Caret { cell: last, ..at })
            }
            Scope::Sentence => (self.sentence_run_start(at), self.sentence_run_end(at)),
            Scope::Paragraph => match self.paragraph_mark_containing(at.page, at.line) {
                Some(p) => {
                    let last_cell = self.line_cell_count(at.page, p.end_line).saturating_sub(1);
                    (
                        Caret {
                            page: at.page,
                            line: p.start_line,
                            cell: 0,
                        },
                        Caret {
                            page: at.page,
                            line: p.end_line,
                            cell: last_cell,
                        },
                    )
                }
                None => (at, at),
            },
        }
    }

    /// Move `caret` to the canonical start of the unit it lies in under
    /// `scope`.
    ///
    /// Applied when a position crosses into a different scope, so a caret
    /// carried over from another granularity lands on a real unit rather than
    /// mid-word or on whitespace. Motion itself does not need this: the
    /// steppers already land on unit starts.
    fn snap_to_scope(&mut self, at: Caret, scope: Scope) -> Caret {
        // Guard first: the word arm below skips whitespace forward and would
        // otherwise walk straight out of the object.
        if let Some(object) = self.unit_object_at(at.page, at.line, scope) {
            return self.object_landing(at.page, object, Landing::Start);
        }
        match scope {
            Scope::Char => at,
            Scope::Word => {
                let target = self.next_word_target_from(at).unwrap_or(at);
                self.word_run_start(target)
            }
            Scope::Line => Caret { cell: 0, ..at },
            Scope::Sentence => self.sentence_run_start(at),
            Scope::Paragraph => match self.paragraph_mark_containing(at.page, at.line) {
                Some(p) => Caret {
                    page: at.page,
                    line: p.start_line,
                    cell: 0,
                },
                None => at,
            },
        }
    }

    /// Move `caret` one unit of `scope` in `dir`, counting whatever is a single
    /// unit at that scope — see [`Self::unit_object_at`] — as one step.
    ///
    /// This wraps [`Self::step_scope`] rather than changing it: the per-scope
    /// table stays the pure description of what a word, line, sentence or
    /// paragraph is, and unit-hood is one rule applied on top of all of them.
    /// Because it has the same signature, counts (`5w`) and every caller keep
    /// working unchanged, and a table costs exactly one repetition at the
    /// scopes where it is one unit.
    ///
    /// Char scope passes straight through — that is the escape hatch that
    /// keeps a single number inside a table selectable.
    fn step_scope_atomic(
        &mut self,
        caret: &mut Caret,
        scope: Scope,
        dir: Dir,
        goal_x: f32,
        goal_y: f32,
    ) -> bool {
        if scope == Scope::Char {
            return self.step_scope(caret, scope, dir, goal_x, goal_y);
        }
        let from = self.unit_id_at(*caret, scope);
        if !self.step_scope(caret, scope, dir, goal_x, goal_y) {
            self.land_on_object(caret, scope, Landing::Start);
            return false;
        }
        // Still inside the object we started in: keep going until we leave it,
        // so the whole object costs one step rather than one step per line.
        if from.is_some() {
            let mut guard = 0;
            while self.unit_id_at(*caret, scope) == from {
                guard += 1;
                // `step_scope` returning true does not guarantee document-order
                // progress (line scope's column jumps move sideways), so the
                // loop needs a hard bound to be provably terminating.
                if guard > MAX_ATOMIC_STEPS || !self.step_scope(caret, scope, dir, goal_x, goal_y) {
                    self.land_on_object(caret, scope, Landing::Start);
                    return false;
                }
            }
        }
        self.land_on_object(caret, scope, Landing::Start);
        true
    }

    /// If `caret` sits inside a unit of `scope`, move it to that unit's
    /// canonical position, so a caret never rests part-way through one.
    fn land_on_object(&mut self, caret: &mut Caret, scope: Scope, land: Landing) {
        if let Some(object) = self.unit_object_at(caret.page, caret.line, scope) {
            *caret = self.object_landing(caret.page, object, land);
        }
    }

    /// Move `caret` one unit of `scope` in `dir`. Returns false at a document
    /// edge, so callers can stop early on a repeated motion.
    ///
    /// This is the single per-scope motion table. One deliberate asymmetry:
    /// line, sentence and paragraph swap the axes on multi-column pages —
    /// `h`/`l` jump columns (keyed on the goal row) while `j`/`k` step the
    /// unit vertically. Char and word keep the ordinary reading axes.
    fn step_scope(
        &mut self,
        caret: &mut Caret,
        scope: Scope,
        dir: Dir,
        goal_x: f32,
        goal_y: f32,
    ) -> bool {
        let forward = matches!(dir, Dir::Right | Dir::Down);
        match scope {
            Scope::Char => match dir {
                Dir::Left => self.step_left(caret),
                Dir::Right => self.step_right(caret),
                Dir::Up => self.step_up(caret, goal_x),
                Dir::Down => self.step_down(caret, goal_x),
            },
            Scope::Word => match dir {
                Dir::Left => self.step_prev_word_start(caret),
                Dir::Right => self.step_next_word_start(caret),
                Dir::Up | Dir::Down => {
                    let mut mark = self.word_mark_from_caret(*caret);
                    let moved = self.word_step_vertical(&mut mark, goal_x, forward);
                    *caret = Self::word_mark_caret(mark);
                    moved
                }
            },
            Scope::Line => {
                let mut mark = LineMark {
                    page: caret.page,
                    line: caret.line,
                };
                let moved = match dir {
                    Dir::Down => self.line_step_down(&mut mark),
                    Dir::Up => self.line_step_up(&mut mark),
                    Dir::Left => self.line_step_column(&mut mark, goal_y, false),
                    Dir::Right => self.line_step_column(&mut mark, goal_y, true),
                };
                *caret = Caret {
                    page: mark.page,
                    line: mark.line,
                    cell: 0,
                };
                moved
            }
            Scope::Sentence => match dir {
                Dir::Left | Dir::Right => {
                    let mut mark = LineMark {
                        page: caret.page,
                        line: caret.line,
                    };
                    if !self.line_step_column(&mut mark, goal_y, forward) {
                        return false;
                    }
                    *caret = self.snap_to_scope(
                        Caret {
                            page: mark.page,
                            line: mark.line,
                            cell: 0,
                        },
                        Scope::Sentence,
                    );
                    true
                }
                Dir::Up | Dir::Down => {
                    let mut mark = self.sentence_mark_from_caret(*caret);
                    let moved = if forward {
                        self.sentence_step_next(&mut mark)
                    } else {
                        self.sentence_step_prev(&mut mark)
                    };
                    *caret = Caret {
                        page: mark.page,
                        line: mark.start_line,
                        cell: mark.start_cell,
                    };
                    moved
                }
            },
            Scope::Paragraph => match dir {
                Dir::Left | Dir::Right => {
                    let mut mark = LineMark {
                        page: caret.page,
                        line: caret.line,
                    };
                    if !self.line_step_column(&mut mark, goal_y, forward) {
                        return false;
                    }
                    *caret = self.snap_to_scope(
                        Caret {
                            page: mark.page,
                            line: mark.line,
                            cell: 0,
                        },
                        Scope::Paragraph,
                    );
                    true
                }
                Dir::Up | Dir::Down => {
                    let Some(mut mark) = self.paragraph_mark_containing(caret.page, caret.line)
                    else {
                        return false;
                    };
                    let moved = if forward {
                        self.paragraph_step_next(&mut mark)
                    } else {
                        self.paragraph_step_prev(&mut mark)
                    };
                    *caret = Caret {
                        page: mark.page,
                        line: mark.start_line,
                        cell: 0,
                    };
                    moved
                }
            },
        }
    }

    /// A span's bounding box in page points, for scrolling it into view.
    /// `None` when the span's page has no content extracted.
    ///
    /// Multi-line spans give one box covering every line, which is what
    /// `scroll_doc_rect_into_view` wants: it scrolls the minimum amount, so a
    /// span taller than the viewport simply pins its top edge.
    fn span_bbox(&mut self, start: Caret, end: Caret) -> Option<Rect> {
        // A whole block scrolls into view by its own bounds, so a table's
        // ruling lines and a formula's inter-row gaps come along with the
        // text. Keyed on the span covering the block rather than on the
        // scope, so it agrees with what `page_span_rects` decided to draw.
        if start.page == end.page {
            if let Some(object) = self
                .region_at(start.page, start.line)
                .filter(|o| o.kind.is_block())
            {
                if start.line == object.start_line && end.line == object.end_line {
                    return Some(object.bbox);
                }
            }
        }
        let first = self.cell_rect(start.page, start.line, start.cell)?;
        let last = self
            .cell_rect(end.page, end.line, end.cell)
            .unwrap_or(first);
        Some(Rect {
            x0: first.x0.min(last.x0),
            y0: first.y0.min(last.y0),
            x1: first.x1.max(last.x1),
            y1: first.y1.max(last.y1),
        })
    }

    /// Scroll so a page-space rectangle's center sits at the viewport center
    /// (both axes), then clamp. No scroll-off margin — true centering.
    fn center_page_rect(&mut self, page: usize, rect: Rect) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(page) = session.view.layout().page(page) else {
            return;
        };
        let cx = page.x + (rect.x0 + rect.x1) / 2.0;
        let cy = page.y + (rect.y0 + rect.y1) / 2.0;
        session.view.center_on_doc_point(cx, cy);
    }

    /// Scroll the minimum amount needed to bring a page-space rectangle on
    /// `page` into view, keeping `view.scroll_off` pixels of context above and
    /// below it.
    fn scroll_page_rect_into_view(&mut self, page: usize, rect: Rect) {
        let scroll_off = self.config.view.scroll_off;
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(page) = session.view.layout().page(page) else {
            return;
        };
        let (px, py) = (page.x, page.y);
        session.view.scroll_doc_rect_into_view(
            px + rect.x0,
            py + rect.y0,
            px + rect.x1,
            py + rect.y1,
            scroll_off,
        );
    }

    /// Top of the band the highlight is allowed to sit in, in document space:
    /// the viewport top pushed down by the scroll-off buffer.
    ///
    /// The buffer is capped by how far the view *could* still scroll up, so at
    /// the start of the document — where no amount of scrolling would create
    /// clearance — the first lines stay reachable. Landing the highlight here
    /// rather than at the raw viewport top is what stops it from being placed
    /// inside the buffer and immediately scrolling the view back.
    fn viewport_content_top(&self) -> Option<f32> {
        let session = self.session.as_ref()?;
        let scroll_y = session.view.scroll().1;
        let scroll_off = self.config.view.scroll_off.max(0.0) / session.view.zoom();
        Some(scroll_y + scroll_off.min(scroll_y.max(0.0)))
    }

    /// An inclusive cell range as one screen rectangle per spanned line,
    /// restricted to the pages currently on screen.
    ///
    /// Walking the viewport rather than the span keeps this O(visible lines)
    /// however long the span is, and avoids forcing content extraction for
    /// pages the reader cannot see. The per-line geometry itself is
    /// [`page_span_rects`], shared with [`Self::span_page_rects`] so drawing a
    /// span and storing one cannot disagree about its shape.
    fn span_screen_rects(&self, start: Caret, end: Caret) -> Option<Vec<ScreenRect>> {
        let session = self.session.as_ref()?;
        let mut rects = Vec::new();
        for (page, _) in session.view.visible_pages() {
            // Content is loaded lazily; a page we have not visited yet simply
            // has nothing to draw.
            let Some(content) = session.content.get(&page) else {
                continue;
            };
            for rect in page_span_rects(content, page, start, end) {
                if let Some(rect) = session
                    .view
                    .page_rect_to_screen(page, rect.x0, rect.y0, rect.x1, rect.y1)
                {
                    rects.push(rect);
                }
            }
        }
        if rects.is_empty() {
            None
        } else {
            Some(rects)
        }
    }

    /// An inclusive cell range as page-space rectangles, for *every* page it
    /// covers — extracting content where it has not been visited yet.
    ///
    /// The counterpart to [`Self::span_screen_rects`]: storing a highlight has to
    /// resolve the whole span, not just the part on screen, which is exactly the
    /// case decision 13 in `docs/architecture.md` left open.
    fn span_page_rects(&mut self, start: Caret, end: Caret) -> Vec<(usize, Vec<Rect>)> {
        let mut out = Vec::new();
        for page in start.page..=end.page {
            self.ensure_content(page);
            let Some(session) = self.session.as_ref() else {
                break;
            };
            let Some(content) = session.content.get(&page) else {
                continue;
            };
            let rects = page_span_rects(content, page, start, end);
            if !rects.is_empty() {
                out.push((page, rects));
            }
        }
        out
    }

    /// The characters an inclusive cell range covers.
    ///
    /// Images and table rules contribute nothing, so a highlight over a figure
    /// has empty text rather than a placeholder — the geometry is what makes it
    /// visible, and the text is only there for notes and export.
    fn span_text(&mut self, start: Caret, end: Caret) -> String {
        let mut out = String::new();
        for page in start.page..=end.page {
            self.ensure_content(page);
            let Some(session) = self.session.as_ref() else {
                break;
            };
            let Some(content) = session.content.get(&page) else {
                continue;
            };
            for (line_idx, line) in content.lines.iter().enumerate() {
                if page == start.page && line_idx < start.line {
                    continue;
                }
                if page == end.page && line_idx > end.line {
                    break;
                }
                let first = if page == start.page && line_idx == start.line {
                    start.cell
                } else {
                    0
                };
                let last = if page == end.page && line_idx == end.line {
                    end.cell
                } else {
                    line.cells.len().saturating_sub(1)
                };
                for cell in line.cells.iter().take(last + 1).skip(first) {
                    if let CellKind::Char(c) = cell.kind {
                        out.push(c);
                    }
                }
            }
        }
        out
    }

    // ---- Focus mode -----------------------------------------------------

    /// The focused position: one point, whatever the scope. What is highlighted
    /// is [`Self::focus_span`].
    pub fn focus_caret(&self) -> Option<Caret> {
        self.focus
    }

    /// The granularity the focus highlight snaps to.
    pub fn focus_scope(&self) -> Scope {
        self.focus_scope
    }

    /// The focus highlight as an inclusive, document-order cell range.
    pub fn focus_span(&self) -> Option<(Caret, Caret)> {
        self.focus_span
    }

    /// Recompute the cached span from the focused position and scope.
    fn refresh_focus_span(&mut self) {
        let Some(at) = self.focus else {
            self.focus_span = None;
            return;
        };
        self.focus_span = Some(self.scope_span(at, self.focus_scope));
    }

    /// Update the remembered goal column from the focused cell.
    fn update_focus_goal_x(&mut self, at: Caret) {
        if let Some(r) = self.cell_rect(at.page, at.line, at.cell) {
            self.focus_goal_x = (r.x0 + r.x1) / 2.0;
        }
    }

    /// Update the remembered goal row from the focused line, for
    /// line/sentence/paragraph column motion.
    fn update_focus_goal_y(&mut self, at: Caret) {
        if let Some(b) = self.line_bbox(at.page, at.line) {
            self.focus_goal_y = (b.y0 + b.y1) / 2.0;
        }
    }

    /// Enter focus mode at `scope`, or — when already focused — reinterpret the
    /// current position at the new scope without moving it.
    ///
    /// That second case is the point of holding the scope in a field: `cw` then
    /// `ce` keeps you exactly where you are, where five separate modes each
    /// carried their own stale mark.
    fn enter_focus(&mut self, scope: Scope) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        // A `c` chord out of highlight mode keeps the highlight, the same way `a`
        // and `v` do: the only way to throw one away is to ask for that.
        self.store_pending_highlight();
        self.mode = Mode::Focus;
        self.focus_scope = scope;
        // A focus highlight and a selection are mutually exclusive.
        self.visual = None;
        self.visual_span = None;
        let at = match self.focus {
            Some(at) => Some(at),
            None => self.entry_caret(),
        };
        if let Some(at) = at {
            let at = self.snap_to_scope(at, scope);
            self.focus = Some(at);
            self.update_focus_goal_x(at);
            self.update_focus_goal_y(at);
        }
        self.refresh_focus_span();
        self.ensure_focus_visible();
        self.save_position();
        Effects::redraw()
    }

    /// Move the focus by `count` units of the active scope.
    fn focus_move(&mut self, dir: Dir, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        // Nothing focused yet (e.g. entered on an empty document): try again.
        let Some(mut at) = self.focus else {
            return self.enter_focus(self.focus_scope);
        };
        let steps = count.unwrap_or(1).max(1);
        let goal_x = self.focus_goal_x;
        let goal_y = self.focus_goal_y;
        let scope = self.focus_scope;
        for _ in 0..steps {
            if !self.step_scope_atomic(&mut at, scope, dir, goal_x, goal_y) {
                break; // reached a document edge
            }
        }
        self.focus = Some(at);
        // Horizontal motion redefines the column vertical motion aims for --
        // except at line/sentence/paragraph, where the axes are swapped: there
        // `h`/`l` jump columns and `j`/`k` set the row those jumps aim at.
        if matches!(scope, Scope::Line | Scope::Sentence | Scope::Paragraph) {
            if matches!(dir, Dir::Up | Dir::Down) {
                self.update_focus_goal_y(at);
            }
        } else if matches!(dir, Dir::Left | Dir::Right) {
            self.update_focus_goal_x(at);
        }
        self.refresh_focus_span();
        self.ensure_focus_visible();
        self.save_position();
        Effects::redraw()
    }

    /// `w`/`b`/`e`/`s`/`p` move by their own named unit in *every* scope: a
    /// motion names its unit, and the highlight still snaps back out to the
    /// active scope afterwards. Move by `count` units of `scope`, *whatever
    /// the active scope is*.
    ///
    /// It is the same [`Self::step_scope`] table `hjkl` use — the only
    /// difference is that the scope comes from the command rather than from
    /// the mode. `e` (line) always lands at column 0, since a line's start
    /// *is* column 0, so the goal-column update below still does the right
    /// thing without a line-shaped exception.
    fn focus_scope_motion(&mut self, scope: Scope, dir: Dir, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        let Some(mut at) = self.focus else {
            return self.enter_focus(self.focus_scope);
        };
        let steps = count.unwrap_or(1).max(1);
        let goal_x = self.focus_goal_x;
        let goal_y = self.focus_goal_y;
        for _ in 0..steps {
            if !self.step_scope_atomic(&mut at, scope, dir, goal_x, goal_y) {
                break; // reached a document edge
            }
        }
        self.focus = Some(at);
        // Landing at a new unit's start redefines the column `j`/`k` aim at.
        self.update_focus_goal_x(at);
        self.refresh_focus_span();
        self.ensure_focus_visible();
        self.save_position();
        Effects::redraw()
    }

    /// After a scroll or page jump in focus mode, move the highlight to the
    /// top-most content line now visible below the scroll-off buffer, keeping
    /// its goal column. Unlike focus motion this does *not* scroll the view
    /// back, so the highlight follows the scroll rather than fighting it.
    fn reposition_focus_to_viewport(&mut self) {
        let Some(view_top) = self.viewport_content_top() else {
            return;
        };
        let goal_x = self.focus_goal_x;
        let scope = self.focus_scope;
        let Some((page, line)) = self.topmost_visible_line(view_top) else {
            return;
        };
        // Char and word scope keep the reader's column; the line-and-larger
        // scopes start at the beginning of the line, since their spans do.
        let cell = match scope {
            Scope::Char | Scope::Word => self.nearest_cell(page, line, goal_x),
            Scope::Line | Scope::Sentence | Scope::Paragraph => 0,
        };
        let at = self.snap_to_scope(Caret { page, line, cell }, scope);
        self.focus = Some(at);
        self.refresh_focus_span();
    }

    /// Scroll so the current highlight is centered in the viewport (Vim `zz`).
    ///
    /// Target by mode: focus span (or caret) in focus mode; visual span (or
    /// head) in visual/highlight; remembered focus in normal mode. No-op when
    /// there is nothing to center on. Uses true centering — no scroll-off.
    fn center_view(&mut self) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        let (start, end) = match self.mode {
            Mode::Focus => match self.focus_span {
                Some(span) => span,
                None => match self.focus {
                    Some(at) => (at, at),
                    None => return Effects::default(),
                },
            },
            Mode::Visual | Mode::Highlight => match self.visual_span {
                Some(span) => span,
                None => match self.focus {
                    Some(at) => (at, at),
                    None => return Effects::default(),
                },
            },
            Mode::Normal => match self.focus {
                Some(at) => self.focus_span.unwrap_or((at, at)),
                None => return Effects::default(),
            },
        };
        let Some(rect) = self.span_bbox(start, end) else {
            return Effects::default();
        };
        self.center_page_rect(start.page, rect);
        self.save_position();
        Effects::redraw()
    }

    /// Scroll the minimum amount needed to keep the focus highlight on screen.
    fn ensure_focus_visible(&mut self) {
        let Some((start, end)) = self.focus_span else {
            return;
        };
        let Some(rect) = self.span_bbox(start, end) else {
            return;
        };
        self.scroll_page_rect_into_view(start.page, rect);
    }

    /// The focus highlight as one screen rectangle per spanned line. `None`
    /// outside focus mode. The shell paints this overlay.
    pub fn focus_screen_rects(&self) -> Option<Vec<ScreenRect>> {
        if self.mode != Mode::Focus {
            return None;
        }
        let (start, end) = self.focus_span?;
        self.span_screen_rects(start, end)
    }

    // ---- Visual mode ----------------------------------------------------

    /// The selection as a two-ended view: the anchor as stored, the head read
    /// from the live position. Assembled on demand — the head is not stored
    /// twice, so this cannot disagree with what motions actually move.
    pub fn visual_selection(&self) -> Option<VisualSelection> {
        let a = self.visual?;
        Some(VisualSelection {
            anchor: a.anchor,
            anchor_scope: a.anchor_scope,
            head: self.focus?,
            head_scope: self.focus_scope,
            return_mode: a.return_mode,
        })
    }

    /// The selection resolved to an inclusive, document-order cell range.
    pub fn visual_span(&self) -> Option<(Caret, Caret)> {
        self.visual_span
    }

    /// Recompute the cached span from the two ends. Each end is expanded by its
    /// own scope, then the outermost edges win — so the ends crossing needs no
    /// special case, and swapping them cannot change what is drawn.
    fn refresh_visual_span(&mut self) {
        let (Some(a), Some(head)) = (self.visual, self.focus) else {
            self.visual_span = None;
            return;
        };
        let (a0, a1) = self.scope_span(a.anchor, a.anchor_scope);
        let (h0, h1) = self.scope_span(head, self.focus_scope);
        self.visual_span = Some((a0.min(h0), a1.max(h1)));
    }

    /// Where a fresh selection starts: the live position, else the topmost
    /// visible line.
    fn visual_entry_caret(&mut self) -> Option<Caret> {
        match self.focus {
            Some(at) => Some(at),
            None => self.entry_caret(),
        }
    }

    /// Enter visual mode. `scope = None` inherits the current scope (`v` from
    /// word focus selects word-wise).
    fn enter_visual(&mut self, scope: Option<Scope>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        // From highlight mode, `v` keeps the highlight and changes nothing else:
        // the two ends are the ones the user just shaped, so the collapse below
        // would throw the selection away. A scope specifier still applies, which
        // is what makes `vw` "keep it, carry on selecting by word".
        if self.mode == Mode::Highlight {
            self.store_pending_highlight();
            self.mode = Mode::Visual;
            return match scope {
                Some(scope) => self.set_head_scope(scope, false),
                None => {
                    self.refresh_visual_span();
                    Effects::redraw()
                }
            };
        }
        // Re-entering visual mode collapses the selection onto the head.
        let live = self.visual.filter(|_| self.mode == Mode::Visual);
        if let (Some(mut a), Some(head)) = (live, self.focus) {
            a.anchor = head;
            if let Some(scope) = scope {
                self.focus_scope = scope;
            }
            a.anchor_scope = self.focus_scope;
            self.visual = Some(a);
            self.refresh_visual_span();
            return Effects::redraw();
        }
        let scope = scope.unwrap_or(self.focus_scope);
        let Some(at) = self.visual_entry_caret() else {
            return Effects::default();
        };
        self.visual = Some(VisualAnchor {
            anchor: at,
            anchor_scope: scope,
            return_mode: self.mode,
        });
        self.focus = Some(at);
        self.focus_scope = scope;
        self.mode = Mode::Visual;
        self.update_focus_goal_x(at);
        self.refresh_visual_span();
        self.ensure_visual_head_visible();
        self.save_position();
        Effects::redraw()
    }

    /// Leave visual mode, restoring the mode it was entered from.
    ///
    /// Nothing is carried across: the head already *is* the live position, so
    /// dropping the anchor is the whole operation. Every other way out of
    /// visual mode (a `c` chord, say) is correct for free, for the same reason.
    fn exit_visual(&mut self) -> Effects {
        let Some(a) = self.visual else {
            self.enter_normal_mode();
            return Effects::redraw();
        };
        if a.return_mode == Mode::Normal {
            self.enter_normal_mode();
        } else {
            self.mode = a.return_mode;
            self.refresh_focus_span();
        }
        self.visual = None;
        self.visual_span = None;
        self.save_position();
        Effects::redraw()
    }

    /// Return to normal mode, resetting the scope.
    ///
    /// Normal mode has no granularity of its own, so it cannot sensibly
    /// *remember* one: a bare `v` from a clean normal mode would otherwise
    /// select by whatever unit you last happened to use. The position is kept
    /// -- only the scope resets.
    fn enter_normal_mode(&mut self) {
        self.mode = Mode::Normal;
        self.focus_scope = Scope::Char;
        self.refresh_focus_span();
    }

    /// Exchange the anchored end with the live one, scopes included. Keeps the
    /// invariant that the focus position is whichever end moves.
    fn swap_visual_ends(&mut self) {
        let (Some(mut a), Some(head)) = (self.visual, self.focus) else {
            return;
        };
        self.focus = Some(a.anchor);
        a.anchor = head;
        std::mem::swap(&mut self.focus_scope, &mut a.anchor_scope);
        self.visual = Some(a);
    }

    /// `o`: make the other end the one motions move. Purely a state change —
    /// the rendered selection is unaffected.
    fn visual_swap_ends(&mut self) -> Effects {
        if self.visual.is_none() {
            return Effects::default();
        }
        self.swap_visual_ends();
        // The goal column belongs to whichever end is moving, so it has to
        // follow the swap or the next `j`/`k` jumps to the old head's column.
        if let Some(head) = self.focus {
            self.update_focus_goal_x(head);
        }
        self.ensure_visual_head_visible();
        Effects::redraw()
    }

    /// Set the active end's granularity, optionally switching ends first
    /// (`vw` vs `ow`).
    fn set_head_scope(&mut self, scope: Scope, swap_first: bool) -> Effects {
        if self.visual.is_none() {
            return Effects::default();
        }
        if swap_first {
            self.swap_visual_ends();
        }
        self.focus_scope = scope;
        if swap_first {
            if let Some(head) = self.focus {
                self.update_focus_goal_x(head);
            }
        }
        self.refresh_visual_span();
        self.ensure_visual_head_visible();
        Effects::redraw()
    }

    fn visual_move(&mut self, dir: Dir, count: Option<u32>) -> Effects {
        let (Some(_), Some(mut head)) = (self.visual, self.focus) else {
            return Effects::default();
        };
        let steps = count.unwrap_or(1).max(1);
        let goal_x = self.focus_goal_x;
        let goal_y = self.focus_goal_y;
        let scope = self.focus_scope;
        for _ in 0..steps {
            if !self.step_scope_atomic(&mut head, scope, dir, goal_x, goal_y) {
                break;
            }
        }
        self.focus = Some(head);
        // Horizontal motion redefines the column vertical motion aims for --
        // except at line/sentence/paragraph, where the axes are swapped: there
        // `h`/`l` jump columns and `j`/`k` set the row those jumps aim at.
        if matches!(scope, Scope::Line | Scope::Sentence | Scope::Paragraph) {
            if matches!(dir, Dir::Up | Dir::Down) {
                self.update_focus_goal_y(head);
            }
        } else if matches!(dir, Dir::Left | Dir::Right) {
            self.update_focus_goal_x(head);
        }
        self.refresh_visual_span();
        self.ensure_visual_head_visible();
        self.save_position();
        Effects::redraw()
    }

    /// `w`/`b`/`e`/`s`/`p` move the head by their own named unit in *every*
    /// scope: see [`Self::focus_scope_motion`], which this mirrors for the
    /// selection's moving end.
    fn visual_scope_motion(&mut self, scope: Scope, dir: Dir, count: Option<u32>) -> Effects {
        let (Some(_), Some(mut head)) = (self.visual, self.focus) else {
            return Effects::default();
        };
        let steps = count.unwrap_or(1).max(1);
        let goal_x = self.focus_goal_x;
        let goal_y = self.focus_goal_y;
        for _ in 0..steps {
            if !self.step_scope_atomic(&mut head, scope, dir, goal_x, goal_y) {
                break;
            }
        }
        self.focus = Some(head);
        self.update_focus_goal_x(head);
        self.refresh_visual_span();
        self.ensure_visual_head_visible();
        self.save_position();
        Effects::redraw()
    }

    /// Scroll so the moving end stays on screen. Only the head is followed —
    /// the anchor may be arbitrarily far away.
    fn ensure_visual_head_visible(&mut self) {
        let Some(head) = self.focus else {
            return;
        };
        let Some(rect) = self.cell_rect(head.page, head.line, head.cell) else {
            return;
        };
        self.scroll_page_rect_into_view(head.page, rect);
    }

    /// The selection as one screen rectangle per spanned line. `None` outside
    /// visual mode. The shell paints this overlay.
    pub fn visual_screen_rects(&self) -> Option<Vec<ScreenRect>> {
        if self.mode != Mode::Visual {
            return None;
        }
        let (start, end) = self.visual_span?;
        self.span_screen_rects(start, end)
    }

    // ---- Highlight mode -------------------------------------------------

    /// The highlights stored for the open document.
    pub fn highlights(&self) -> &[Highlight] {
        &self.highlights
    }

    /// Whether a highlight is being placed.
    pub fn has_pending_highlight(&self) -> bool {
        self.pending.is_some()
    }

    /// `a`: turn the focus highlight or the selection into a pending highlight.
    ///
    /// Coming from focus mode, the second end is *synthesised* here — anchor and
    /// head coincide — which is what lets every visual motion reshape a highlight
    /// that started from a single focused word.
    fn enter_highlight(&mut self) -> Effects {
        if self.session.is_none() || self.mode == Mode::Highlight {
            return Effects::default();
        }
        if !matches!(self.mode, Mode::Focus | Mode::Visual) {
            return Effects::default();
        }
        let Some(focus) = self.focus else {
            return Effects::default();
        };
        self.pending = Some(PendingHighlight {
            return_mode: self.mode,
            return_focus: focus,
            return_scope: self.focus_scope,
            return_visual: self.visual,
            color: self.config.view.highlight_color.clone(),
        });
        if self.visual.is_none() {
            self.visual = Some(VisualAnchor {
                anchor: focus,
                anchor_scope: self.focus_scope,
                // Only consulted by `visual_exit`, which highlight mode never
                // reaches: `<Esc>` here discards instead. Recording where we came
                // from keeps it honest anyway.
                return_mode: self.mode,
            });
        }
        self.mode = Mode::Highlight;
        self.refresh_visual_span();
        Effects::redraw()
    }

    /// `a` again: keep the highlight and return to focus on the moving end.
    fn commit_highlight(&mut self) -> Effects {
        if self.pending.is_none() {
            return Effects::default();
        }
        // `enter_focus` stores the pending highlight and drops the selection.
        self.enter_focus(self.focus_scope)
    }

    /// `<Esc>` / `<BS>`: throw the highlight away and put back the mode and
    /// selection that were in effect when `a` was pressed.
    fn discard_highlight(&mut self) -> Effects {
        let Some(pending) = self.pending.take() else {
            return Effects::default();
        };
        self.mode = pending.return_mode;
        self.focus = Some(pending.return_focus);
        self.focus_scope = pending.return_scope;
        self.visual = pending.return_visual;
        self.refresh_focus_span();
        self.refresh_visual_span();
        Effects::redraw()
    }

    /// Store the pending highlight, if there is one, leaving the mode and the
    /// selection alone.
    ///
    /// Shared by every way out of highlight mode that keeps the highlight — `a`,
    /// `v`, `c`, and saving — so "which exits keep it" is one list in the
    /// bindings rather than a condition repeated in four places.
    fn store_pending_highlight(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let Some((start, end)) = self.visual_span else {
            return;
        };
        let rects: Vec<HighlightRect> = self
            .span_page_rects(start, end)
            .into_iter()
            .flat_map(|(page, rects)| {
                rects.into_iter().map(move |r| HighlightRect {
                    page,
                    x0: r.x0,
                    y0: r.y0,
                    x1: r.x1,
                    y1: r.y1,
                })
            })
            .collect();
        if rects.is_empty() {
            return;
        }
        let text = self.span_text(start, end);
        let id = self.persist_highlight(&pending.color, &text, &rects);
        self.highlights.push(Highlight {
            id,
            color: pending.color,
            text,
            rects,
        });
    }

    /// Write a highlight to the database, returning its row id.
    ///
    /// Failure is reported and otherwise ignored: the highlight still exists for
    /// this session and can still be saved into the PDF, which is a far better
    /// outcome than refusing to highlight because the database is unwritable.
    fn persist_highlight(
        &mut self,
        color: &str,
        text: &str,
        rects: &[HighlightRect],
    ) -> Option<i64> {
        let document_id = self.session.as_ref()?.document_id?;
        let storage = self.storage.as_ref()?;
        match storage.insert_highlight(document_id, color, text, rects) {
            Ok(id) => Some(id),
            Err(e) => {
                self.last_error = Some(format!("could not save highlight: {e}"));
                None
            }
        }
    }

    /// Load the document's stored highlights. Called on open, so highlights made
    /// in an earlier session are on screen before any page content is extracted.
    fn load_highlights(&mut self) {
        self.highlights.clear();
        let Some(document_id) = self.session.as_ref().and_then(|s| s.document_id) else {
            return;
        };
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        match storage.load_highlights(document_id) {
            Ok(stored) => {
                self.highlights = stored
                    .into_iter()
                    .map(|h| Highlight {
                        id: Some(h.id),
                        color: h.color,
                        text: h.text,
                        rects: h.rects,
                    })
                    .collect()
            }
            Err(e) => self.last_error = Some(format!("could not load highlights: {e}")),
        }
    }

    /// Every highlight to paint: the stored ones, plus the pending one while it
    /// is being placed.
    ///
    /// One overlay and one colour for both, so the shell's single merged fill
    /// cannot double-blend a pending highlight over the stored one it overlaps.
    pub fn highlight_screen_rects(&self) -> Option<Vec<ScreenRect>> {
        let session = self.session.as_ref()?;
        let mut rects = Vec::new();
        for (page, _) in session.view.visible_pages() {
            for highlight in &self.highlights {
                for rect in highlight.rects.iter().filter(|r| r.page == page) {
                    if let Some(rect) = session
                        .view
                        .page_rect_to_screen(page, rect.x0, rect.y0, rect.x1, rect.y1)
                    {
                        rects.push(rect);
                    }
                }
            }
        }
        if self.mode == Mode::Highlight {
            if let Some((start, end)) = self.visual_span {
                rects.extend(self.span_screen_rects(start, end).unwrap_or_default());
            }
        }
        if rects.is_empty() {
            None
        } else {
            Some(rects)
        }
    }

    /// One-line status text: file, current page, zoom, pending keys.
    pub fn status_text(&self) -> String {
        let mut out = String::new();
        match &self.session {
            Some(session) => {
                let name = session
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| session.path.display().to_string());
                out.push_str(&format!(
                    "{name}  [{}/{}]  {:.0}%",
                    session.view.current_page() + 1,
                    session.view.layout().page_count(),
                    session.view.zoom() * 100.0
                ));
            }
            None => out.push_str("no document - press <leader>o to open a PDF"),
        }
        if self.mode == Mode::Focus {
            // `-- FOCUS (word) --`, matching visual's `-- VISUAL (word) --`.
            out.push_str(&format!("  -- FOCUS ({}) --", self.focus_scope.name()));
            if let Some((start, end)) = self.focus_span {
                if start.line == end.line {
                    out.push_str(&format!("  Ln {}, Col {}", start.line + 1, start.cell + 1));
                } else {
                    out.push_str(&format!("  Ln {}-{}", start.line + 1, end.line + 1));
                }
            }
        }
        // One arm for both two-ended modes: they show the same thing, and the
        // only difference is the word — which is the point of the mode.
        if matches!(self.mode, Mode::Visual | Mode::Highlight) {
            let name = if self.mode == Mode::Visual {
                "VISUAL"
            } else {
                "HIGHLIGHT"
            };
            if let Some(a) = self.visual {
                // Both scopes are shown, head first, when the ends differ.
                let head_scope = self.focus_scope;
                if head_scope == a.anchor_scope {
                    out.push_str(&format!("  -- {name} ({}) --", head_scope.name()));
                } else {
                    out.push_str(&format!(
                        "  -- {name} ({}/{}) --",
                        head_scope.name(),
                        a.anchor_scope.name()
                    ));
                }
            } else {
                out.push_str(&format!("  -- {name} --"));
            }
            if let Some((start, end)) = self.visual_span {
                out.push_str(&format!("  Ln {}-{}", start.line + 1, end.line + 1));
            }
        }
        if self.input.has_pending() {
            out.push_str(&format!("  {}", self.input.pending_display()));
        }
        if let Some(message) = &self.status_message {
            out.push_str(&format!("  {message}"));
        }
        if let Some(error) = &self.last_error {
            out.push_str(&format!("  ERROR: {error}"));
        }
        out
    }

    pub fn current_page(&self) -> usize {
        self.session
            .as_ref()
            .map(|s| s.view.current_page())
            .unwrap_or(0)
    }

    pub fn zoom(&self) -> f32 {
        self.session.as_ref().map(|s| s.view.zoom()).unwrap_or(1.0)
    }

    /// Record an error for display in the status line (e.g. a failed open
    /// from the file dialog).
    pub fn report_error(&mut self, message: String) {
        self.last_error = Some(message);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.save_position();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syodep_config::keys::parse_sequence;
    use syodep_pdf::{
        test_support::{pdf_with_image, pdf_with_line_numbers, pdf_with_pages},
        CellKind,
    };

    /// The focus highlight read back in the shape of the per-scope marks the
    /// app used to store.
    ///
    /// These are *derivations*, not state: `focus_span` is the only source, so
    /// a test asserting `word_mark()` is asserting what is actually drawn. Each
    /// one is only meaningful at its own scope — `word_mark()` in line scope
    /// would report the whole line as one word — which is exactly how the tests
    /// use them.
    impl App {
        fn caret(&self) -> Option<Caret> {
            self.focus
        }

        fn line_mark(&self) -> Option<LineMark> {
            let (start, _) = self.focus_span?;
            Some(LineMark {
                page: start.page,
                line: start.line,
            })
        }

        fn word_mark(&self) -> Option<WordMark> {
            let (start, end) = self.focus_span?;
            Some(WordMark {
                page: start.page,
                line: start.line,
                start_cell: start.cell,
                end_cell: end.cell,
            })
        }

        fn sentence_mark(&self) -> Option<SentenceMark> {
            let (start, end) = self.focus_span?;
            Some(SentenceMark {
                page: start.page,
                start_line: start.line,
                start_cell: start.cell,
                end_line: end.line,
                end_cell: end.cell,
            })
        }

        fn paragraph_mark(&self) -> Option<ParagraphMark> {
            let (start, end) = self.focus_span?;
            Some(ParagraphMark {
                page: start.page,
                start_line: start.line,
                end_line: end.line,
            })
        }

        /// The overlay's first rectangle, with the page the span starts on —
        /// the shape the old single-rect getters returned.
        fn focus_screen_rect(&self) -> Option<(usize, ScreenRect)> {
            let (start, _) = self.focus_span?;
            let rects = self.focus_screen_rects()?;
            rects.first().map(|r| (start.page, *r))
        }
    }

    fn write_pdf_bytes(dir: &Path, name: &str, bytes: Vec<u8>) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn write_test_pdf(dir: &Path, pages: usize) -> PathBuf {
        let texts: Vec<String> = (1..=pages).map(|i| format!("Page {i} text")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        write_pdf_bytes(dir, "doc.pdf", pdf_with_pages(&refs))
    }

    fn app_with_doc(dir: &Path, pages: usize) -> App {
        let mut app = App::new(Config::default(), Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        app.open_document(&write_test_pdf(dir, pages)).unwrap();
        app
    }

    fn app_with_text_pages(dir: &Path, page_texts: &[&str]) -> App {
        let mut app = App::new(Config::default(), Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        let path = write_pdf_bytes(dir, "text.pdf", pdf_with_pages(page_texts));
        app.open_document(&path).unwrap();
        app
    }

    fn escape_pdf_text(text: &str) -> String {
        text.replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)")
    }

    fn pdf_with_two_lines(first: &str, second: &str) -> Vec<u8> {
        let total_objects = 5;
        let mut buf: Vec<u8> = b"%PDF-1.4\n".to_vec();
        let mut offsets: Vec<usize> = vec![0; total_objects + 1];

        let write_obj = |buf: &mut Vec<u8>, offsets: &mut Vec<usize>, num: usize, body: &[u8]| {
            offsets[num] = buf.len();
            buf.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
            buf.extend_from_slice(body);
            buf.extend_from_slice(b"\nendobj\n");
        };

        write_obj(
            &mut buf,
            &mut offsets,
            1,
            b"<< /Type /Catalog /Pages 2 0 R >>",
        );
        write_obj(
            &mut buf,
            &mut offsets,
            2,
            b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
        );
        write_obj(
            &mut buf,
            &mut offsets,
            3,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        );
        write_obj(
            &mut buf,
            &mut offsets,
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
              /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>",
        );
        let stream = format!(
            "BT /F1 24 Tf 72 750 Td ({}) Tj ET\nBT /F1 24 Tf 72 710 Td ({}) Tj ET",
            escape_pdf_text(first),
            escape_pdf_text(second)
        );
        write_obj(
            &mut buf,
            &mut offsets,
            5,
            format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            )
            .as_bytes(),
        );

        let xref_offset = buf.len();
        buf.extend_from_slice(format!("xref\n0 {}\n", total_objects + 1).as_bytes());
        buf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets[1..] {
            buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        buf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
                total_objects + 1
            )
            .as_bytes(),
        );
        buf
    }

    fn app_with_two_line_pdf(dir: &Path, first: &str, second: &str) -> App {
        let mut app = App::new(Config::default(), Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        let path = write_pdf_bytes(dir, "two-lines.pdf", pdf_with_two_lines(first, second));
        app.open_document(&path).unwrap();
        app
    }

    fn press(app: &mut App, sequence: &str) -> Effects {
        let mut effects = Effects::default();
        for chord in parse_sequence(sequence).unwrap() {
            effects = app.handle_key(chord);
        }
        effects
    }

    fn assert_caret(app: &App, page: usize, line: usize, cell: usize) {
        let caret = app.caret().unwrap();
        assert_eq!((caret.page, caret.line, caret.cell), (page, line, cell));
    }

    #[test]
    fn open_document_lays_out_all_pages() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_with_doc(dir.path(), 3);
        assert!(app.has_document());
        assert_eq!(app.current_page(), 0);
        // fit_width_on_open with viewport width == page width => zoom 1.0.
        assert!((app.zoom() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn keyboard_navigation_drives_the_view() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);

        assert_eq!(press(&mut app, "J"), Effects::redraw());
        assert_eq!(app.current_page(), 1);
        press(&mut app, "K");
        assert_eq!(app.current_page(), 0);

        press(&mut app, "G");
        assert_eq!(app.current_page(), 4);
        press(&mut app, "gg");
        assert_eq!(app.current_page(), 0);

        // Count-prefixed page jump: 3G goes to 1-based page 3.
        press(&mut app, "3G");
        assert_eq!(app.current_page(), 2);
        // 2J advances two pages.
        press(&mut app, "2J");
        assert_eq!(app.current_page(), 4);
    }

    #[test]
    fn scroll_commands_move() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        let y0 = 0.0;
        press(&mut app, "j");
        let scrolled = app.session.as_ref().unwrap().view.scroll().1;
        assert!(scrolled > y0);
        press(&mut app, "5k");
        assert_eq!(app.session.as_ref().unwrap().view.scroll().1, 0.0);
    }

    #[test]
    fn zoom_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 2);
        let z0 = app.zoom();
        press(&mut app, "z+");
        assert!(app.zoom() > z0);
        press(&mut app, "z0");
        assert!((app.zoom() - 1.0).abs() < 1e-6);
        // fit width: page width (595) at zoom 1.0 fills viewport width (595).
        press(&mut app, "zw");
        assert!((app.zoom() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn open_file_key_requests_dialog() {
        let mut app = App::new(Config::default(), None);
        // `press` parses with `parse_sequence`, which has no leader, so the
        // default leader `<Space>` is spelled out rather than `<leader>o`.
        let effects = press(&mut app, "<Space>o");
        assert!(effects.open_file_dialog);
        // A bare `o` is free for modes to claim -- visual mode swaps ends with
        // it -- so it must not still open the dialog.
        let effects = press(&mut app, "o");
        assert!(!effects.open_file_dialog);
    }

    #[test]
    fn keys_without_document_do_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "j");
        press(&mut app, "G");
        assert_eq!(app.visible_pages(), vec![]);
        assert!(app.status_text().contains("no document"));
    }

    #[test]
    fn status_text_shows_page_zoom_and_pending() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        assert!(app.status_text().contains("[1/3]"));
        assert!(app.status_text().contains("100%"));
        press(&mut app, "2");
        assert!(app.status_text().contains('2'));
    }

    #[test]
    fn position_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("syodep.sqlite3");
        let pdf = write_test_pdf(dir.path(), 5);

        {
            let mut app = App::new(Config::default(), Some(Storage::open(&db).unwrap()));
            app.set_viewport_size(595.0, 600.0);
            app.open_document(&pdf).unwrap();
            press(&mut app, "3G");
            press(&mut app, "z+");
            assert_eq!(app.current_page(), 2);
        }

        let mut app = App::new(Config::default(), Some(Storage::open(&db).unwrap()));
        app.set_viewport_size(595.0, 600.0);
        app.open_document(&pdf).unwrap();
        assert_eq!(app.current_page(), 2);
        assert!(app.zoom() > 1.0);
    }

    #[test]
    fn position_survives_file_rename() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("syodep.sqlite3");
        let pdf = write_test_pdf(dir.path(), 5);

        {
            let mut app = App::new(Config::default(), Some(Storage::open(&db).unwrap()));
            app.set_viewport_size(595.0, 600.0);
            app.open_document(&pdf).unwrap();
            press(&mut app, "G");
        }

        let renamed = dir.path().join("renamed.pdf");
        std::fs::rename(&pdf, &renamed).unwrap();
        let mut app = App::new(Config::default(), Some(Storage::open(&db).unwrap()));
        app.set_viewport_size(595.0, 600.0);
        app.open_document(&renamed).unwrap();
        assert_eq!(app.current_page(), 4);
    }

    #[test]
    fn render_page_returns_bitmap_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 2);
        let (w, h) = {
            let bitmap = app.render_page(0).unwrap();
            (bitmap.width, bitmap.height)
        };
        assert_eq!((w, h), (595, 842));
        assert_eq!(app.session.as_ref().unwrap().cache.len(), 1);
        app.render_page(0).unwrap();
        assert_eq!(app.session.as_ref().unwrap().cache.len(), 1);
    }

    #[test]
    fn page_text_is_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_with_doc(dir.path(), 2);
        assert!(app.page_text(1).unwrap().contains("Page 2 text"));
    }

    #[test]
    fn visible_pages_follow_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        let visible: Vec<usize> = app.visible_pages().iter().map(|p| p.page).collect();
        assert_eq!(visible, vec![0]);
        app.scroll_by_px(0.0, 700.0);
        let visible: Vec<usize> = app.visible_pages().iter().map(|p| p.page).collect();
        assert_eq!(visible, vec![0, 1]);
    }

    #[test]
    fn open_failure_keeps_previous_document() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 2);
        let err = app.open_document(Path::new("/nonexistent.pdf"));
        assert!(err.is_err());
        assert!(app.has_document());
        assert_eq!(app.visible_pages().len(), 1);
    }

    #[test]
    fn caret_enter_places_caret_and_shows_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        assert_eq!(app.mode(), Mode::Normal);
        // A single `c` is only the first half of the `cc` sequence: still
        // pending, so the mode does not change yet.
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Normal);
        // The second `c` completes `cc` and enters caret focus mode.
        press(&mut app, "c");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Char));
        let caret = app.caret().expect("caret placed");
        assert_eq!((caret.page, caret.line, caret.cell), (0, 0, 0));
        assert!(app.focus_screen_rect().is_some());
        assert!(app.status_text().contains("-- FOCUS (char) --"));
        assert!(app.status_text().contains("Ln 1, Col 1"));
    }

    #[test]
    fn caret_right_advances_chars_then_wraps_to_next_page() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "cc");
        // A few steps stay on the first line (the page's only line of text).
        press(&mut app, "3l");
        let caret = app.caret().unwrap();
        assert_eq!((caret.page, caret.line, caret.cell), (0, 0, 3));
        // Walking right past the end of page 0 wraps onto page 1 (the exact
        // glyph count is layout-dependent, so step until the page changes).
        for _ in 0..200 {
            if app.caret().unwrap().page != 0 {
                break;
            }
            press(&mut app, "l");
        }
        assert_eq!(app.caret().unwrap().page, 1);
    }

    #[test]
    fn caret_left_at_document_start_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 2);
        press(&mut app, "cc");
        press(&mut app, "h");
        let caret = app.caret().unwrap();
        assert_eq!((caret.page, caret.line, caret.cell), (0, 0, 0));
    }

    #[test]
    fn caret_vertical_crosses_pages_keeping_column() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "cc");
        press(&mut app, "3l"); // column 4 (cell index 3)
                               // Each page has a single line, so `j` crosses to the next page.
        press(&mut app, "j");
        let caret = app.caret().unwrap();
        assert_eq!(caret.page, 1);
        // Goal column is preserved across the page boundary (identical layout).
        assert!(
            (caret.cell as i32 - 3).abs() <= 1,
            "col drifted: {}",
            caret.cell
        );
        // `k` comes back up to the previous page.
        press(&mut app, "k");
        assert_eq!(app.caret().unwrap().page, 0);
    }

    #[test]
    fn caret_exit_restores_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "cc");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Char));
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.focus_screen_rect().is_none());
        // In normal mode `j` scrolls again rather than moving the caret.
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        let after = app.session.as_ref().unwrap().view.scroll().1;
        assert!(after > before);
    }

    #[test]
    fn caret_focus_keeps_non_hjkl_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);
        press(&mut app, "cc");
        // `G` still navigates pages while in caret focus mode, and the caret
        // follows to the newly visible page.
        press(&mut app, "G");
        assert_eq!(app.current_page(), 4);
        assert_eq!(app.caret().unwrap().page, 4);
    }

    #[test]
    fn caret_focus_page_jumps_carry_the_caret() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);
        press(&mut app, "cc");
        assert_eq!(app.caret().unwrap().page, 0);
        // Next/prev page move the caret onto the destination page.
        press(&mut app, "J");
        assert_eq!(app.caret().unwrap().page, 1);
        press(&mut app, "K");
        assert_eq!(app.caret().unwrap().page, 0);
        // Goto last / first page (G / gg) likewise.
        press(&mut app, "G");
        assert_eq!(app.caret().unwrap().page, 4);
        press(&mut app, "gg");
        assert_eq!(app.caret().unwrap().page, 0);
    }

    #[test]
    fn caret_focus_page_scroll_advances_the_caret() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);
        press(&mut app, "cc");
        let start = app.caret().unwrap().page;
        // A full page-down scroll (<C-f>) carries the caret to later content.
        press(&mut app, "<C-f>");
        assert!(
            app.caret().unwrap().page > start,
            "caret should advance past page {start}"
        );
    }

    #[test]
    fn caret_focus_zoom_leaves_the_caret_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "cc");
        press(&mut app, "3l"); // move within the line
        let before = app.caret().unwrap();
        press(&mut app, "z+"); // zoom_in does not move the caret
        assert_eq!(app.caret().unwrap(), before);
        press(&mut app, "zw"); // fit_width does not move the caret
        assert_eq!(app.caret().unwrap(), before);
    }

    #[test]
    fn center_view_centers_the_focus_span() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_stacked_lines(dir.path(), 80.0);
        press(&mut app, "ce");
        press(&mut app, "20j");
        // Scroll away without a focus-repositioning command so the highlight
        // is left off-center (and possibly off-screen).
        app.session.as_mut().unwrap().view.scroll_by_px(0.0, 350.0);
        let (start, end) = app.focus_span().unwrap();
        let page_rect = app.span_bbox(start, end).unwrap();
        let before = app
            .session
            .as_ref()
            .unwrap()
            .view
            .page_rect_to_screen(
                start.page,
                page_rect.x0,
                page_rect.y0,
                page_rect.x1,
                page_rect.y1,
            )
            .unwrap();
        let before_cy = before.y + before.height / 2.0;
        assert!(
            (before_cy - 300.0).abs() > 50.0,
            "precondition: focus should not already be centered, got cy={before_cy}"
        );
        press(&mut app, "zc");
        let (_, rect) = app.focus_screen_rect().unwrap();
        let cy = rect.y + rect.height / 2.0;
        assert!(
            (cy - 300.0).abs() < 2.0,
            "focus span center y should be near viewport center, got {cy} (rect={rect:?})"
        );
        // Horizontal centering is requested too, but when the page already
        // fills the viewport width the scroll clamp refuses to move — same
        // as every other scroll. The fixture is fit-width, so only y moves.
    }

    /// A single page of 48 stacked lines, tall enough that the highlight can
    /// walk off both edges of the 600px viewport. One page on purpose: the
    /// fixture repeats its text, and furniture detection would read identical
    /// lines on a second page as running headers.
    fn app_with_stacked_lines(dir: &Path, scroll_off: f32) -> App {
        let mut config = Config::default();
        config.view.scroll_off = scroll_off;
        let mut app = App::new(config, Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        let path = write_pdf_bytes(dir, "lines.pdf", pdf_with_line_numbers(1, 48));
        app.open_document(&path).unwrap();
        app
    }

    #[test]
    fn scroll_off_keeps_the_focus_clear_of_the_bottom_edge() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_stacked_lines(dir.path(), 80.0);
        press(&mut app, "ce");
        press(&mut app, "40j");
        let (_, rect) = app.focus_screen_rect().unwrap();
        assert!(
            (rect.y + rect.height - (600.0 - 80.0)).abs() < 1.0,
            "the view should stop scrolling 80px short of the edge, got {rect:?}"
        );
    }

    #[test]
    fn scroll_off_zero_lets_the_focus_reach_the_edge() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_stacked_lines(dir.path(), 0.0);
        press(&mut app, "ce");
        press(&mut app, "40j");
        let (_, rect) = app.focus_screen_rect().unwrap();
        assert!(
            (rect.y + rect.height - 600.0).abs() < 1.0,
            "opting out should put the highlight flush with the edge, got {rect:?}"
        );
    }

    #[test]
    fn scroll_off_keeps_the_visual_head_clear_of_the_bottom_edge() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_stacked_lines(dir.path(), 80.0);
        press(&mut app, "ve");
        press(&mut app, "40j");
        let rects = app.visual_screen_rects().unwrap();
        let head = rects.last().expect("the selection covers visible lines");
        assert!(
            head.y + head.height <= 600.0 - 80.0 + 1.0,
            "the moving end should stay clear of the edge, got {head:?}"
        );
    }

    #[test]
    fn scroll_off_is_conceded_at_the_document_ends() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_stacked_lines(dir.path(), 80.0);
        // The first line has nothing above it to scroll to, so it must still be
        // reachable flush against the top.
        press(&mut app, "ce");
        assert_caret(&app, 0, 0, 0);
        assert_eq!(app.session.as_ref().unwrap().view.scroll().1, 0.0);
        // Likewise the last line against the end of the document.
        press(&mut app, "47j");
        let view = &app.session.as_ref().unwrap().view;
        let max_y = view.layout().total_height() - 600.0 / view.zoom();
        assert!((view.scroll().1 - max_y).abs() < 0.01);
    }

    #[test]
    fn scroll_off_moves_the_landing_line_after_a_page_scroll() {
        let dir = tempfile::tempdir().unwrap();
        // <C-d> carries the highlight to the newly visible content. With a
        // buffer it must land past it, not flush against the top edge, or the
        // next `k` would immediately scroll the view back.
        let mut buffered = app_with_stacked_lines(dir.path(), 80.0);
        press(&mut buffered, "ce");
        press(&mut buffered, "<C-d>");
        let (_, rect) = buffered.focus_screen_rect().unwrap();
        assert!(
            rect.y + rect.height >= 80.0,
            "the highlight should clear the top buffer, got {rect:?}"
        );

        let mut flush = app_with_stacked_lines(dir.path(), 0.0);
        press(&mut flush, "ce");
        press(&mut flush, "<C-d>");
        let (_, flush_rect) = flush.focus_screen_rect().unwrap();
        assert!(flush_rect.y + flush_rect.height < 80.0);
        assert!(
            buffered.caret().unwrap().line > flush.caret().unwrap().line,
            "the buffer should push the landing line further down the page"
        );
    }

    #[test]
    fn caret_next_word_skips_current_run_and_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta-gamma"]);
        press(&mut app, "cc");
        press(&mut app, "w");
        assert_caret(&app, 0, 0, 6);
    }

    #[test]
    fn caret_next_line_moves_to_the_start_of_the_next_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_line_pdf(dir.path(), "alpha beta", "gamma delta");
        press(&mut app, "cc");
        press(&mut app, "3l"); // partway into the first line
        press(&mut app, "e");
        // `e` always lands at column 0 of the next line, whatever the active
        // scope is -- here char, unaffected by where on the line it started.
        assert_caret(&app, 0, 1, 0);
        assert_eq!(
            app.focus_scope(),
            Scope::Char,
            "e does not change the scope"
        );
    }

    #[test]
    fn caret_next_line_clamps_at_the_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["only line"]);
        press(&mut app, "cc");
        press(&mut app, "e");
        assert_caret(&app, 0, 0, 0);
    }

    #[test]
    fn caret_prev_word_uses_current_then_previous_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        press(&mut app, "cc");
        press(&mut app, "8l");
        assert_caret(&app, 0, 0, 8);
        press(&mut app, "b");
        assert_caret(&app, 0, 0, 6);
        press(&mut app, "b");
        assert_caret(&app, 0, 0, 0);
    }

    #[test]
    fn caret_word_counts_cross_lines_and_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_line_pdf(dir.path(), "one two", "three four");
        press(&mut app, "cc");
        press(&mut app, "2w");
        assert_caret(&app, 0, 1, 0);

        let page_dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(page_dir.path(), &["one two", "three four"]);
        press(&mut app, "cc");
        press(&mut app, "2w");
        assert_caret(&app, 1, 0, 0);
    }

    #[test]
    fn caret_word_motions_clamp_at_document_edges() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["one"]);
        press(&mut app, "cc");
        press(&mut app, "b");
        assert_caret(&app, 0, 0, 0);
        // A single word has no next run either, so `w` clamps at the same
        // start it began on rather than moving anywhere.
        press(&mut app, "w");
        assert_caret(&app, 0, 0, 0);
    }

    // ---- Lists ------------------------------------------------------------

    /// A lead-in line, three bulleted items (one of them wrapping) and a
    /// closing sentence. No item ends in a full stop, which is the whole
    /// problem: without list boundaries this is all one sentence.
    fn list_page_content() -> PageContent {
        let texts = [
            "The files created by the tool are:",
            "\u{2022} the globs file",
            "\u{2022} the magic file, which maps content",
            "to types by inspection",
            "\u{2022} the aliases file",
            "Each of them is regenerated in turn.",
        ];
        let lines: Vec<ContentLine> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| text_line(100.0 + i as f32 * 12.0, t))
            .collect();
        let item = |start: usize, end: usize| ContentObject {
            kind: syodep_pdf::ObjectKind::ListItem,
            bbox: lines[start].bbox.union(lines[end].bbox),
            start_line: start,
            end_line: end,
        };
        PageContent {
            objects: vec![item(1, 1), item(2, 3), item(4, 4)],
            lines,
            ..Default::default()
        }
    }

    fn app_with_list_page(dir: &Path) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder page"]);
        app.set_page_content(0, list_page_content());
        app
    }

    #[test]
    fn the_lead_in_prose_does_not_run_into_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_list_page(dir.path());
        press(&mut app, "cs");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(
            (start.line, end.line),
            (0, 0),
            "the colon line swallowed the list"
        );
    }

    #[test]
    fn each_list_item_is_its_own_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_list_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "\u{2022} the globs file");
        press(&mut app, "s");
        // The second item wraps onto the line below, and carries it along.
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (2, 3));
        press(&mut app, "s");
        let (start, _) = app.focus_span().unwrap();
        assert_eq!(
            start.line, 4,
            "the third item should start its own sentence"
        );
    }

    #[test]
    fn the_last_item_does_not_run_on_into_the_prose_after_the_list() {
        // The item has an end of its own, so the closing sentence stays out of
        // it even though the item carries no full stop.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_list_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "sss"); // onto the third item
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (4, 4));
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "Each of them is regenerated in turn.");
    }

    #[test]
    fn a_list_is_still_one_paragraph() {
        // Items bound sentences, not paragraphs: after the colon lead-in (its
        // own paragraph by the line-final-colon rule), `p` skips the whole
        // list plus the closing prose in one step.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_list_page(dir.path());
        press(&mut app, "cp");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(
            (start.line, end.line),
            (0, 0),
            "colon lead-in is its own paragraph"
        );
        press(&mut app, "j");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(
            (start.line, end.line),
            (1, 5),
            "list items must not split the paragraph that follows the lead-in"
        );
    }

    #[test]
    fn a_numbered_item_is_one_sentence_including_its_marker() {
        // `1.` would otherwise end a sentence all by itself.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["placeholder page"]);
        let lines = vec![
            text_line(100.0, "1. First point of the list"),
            text_line(112.0, "2. Second point of the list"),
        ];
        let item = |i: usize| ContentObject {
            kind: syodep_pdf::ObjectKind::ListItem,
            bbox: lines[i].bbox,
            start_line: i,
            end_line: i,
        };
        app.set_page_content(
            0,
            PageContent {
                objects: vec![item(0), item(1)],
                lines,
                ..Default::default()
            },
        );
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "1. First point of the list");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "2. Second point of the list");
    }

    #[test]
    fn a_sentence_never_reaches_back_out_of_an_item() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_list_page(dir.path());
        // Land inside the second item's wrapped line, then take its sentence.
        press(&mut app, "cc");
        press(&mut app, "jjj");
        assert_eq!(app.caret().unwrap().line, 3);
        press(&mut app, "cs");
        let (start, _) = app.focus_span().unwrap();
        assert_eq!(start.line, 2, "the sentence reached back past the marker");
    }

    #[test]
    fn word_motion_is_unaffected_by_a_list() {
        // Lists bound sentences only; `w` still walks the marker and words.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_list_page(dir.path());
        press(&mut app, "cw");
        for _ in 0..12 {
            if app.caret().unwrap().line == 1 {
                break;
            }
            press(&mut app, "w");
        }
        assert_eq!(app.caret().unwrap().line, 1, "never reached the first item");
        assert_eq!(span_text(&mut app), "\u{2022}");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "the");
    }

    // ---- Numbers ----------------------------------------------------------

    /// A page whose one line is `text`, for exercising word and sentence runs.
    fn app_with_line(dir: &Path, text: &str) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder page"]);
        app.set_page_content(
            0,
            PageContent {
                lines: vec![text_line(100.0, text)],
                ..Default::default()
            },
        );
        app
    }

    /// Two tightly-spaced lines on one page (no paragraph-sized vertical gap).
    fn app_with_two_lines(dir: &Path, first: &str, second: &str) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder page"]);
        app.set_page_content(
            0,
            PageContent {
                lines: vec![text_line(100.0, first), text_line(112.0, second)],
                ..Default::default()
            },
        );
        app
    }

    /// Like [`app_with_line`], but the whitespace cells at the byte indices
    /// in `synthetic_at` carry MuPDF's `SYNTHETIC` flag -- what a URL/DOI
    /// drawn as separate positioning runs looks like once MuPDF has guessed
    /// a space between two of its segments.
    fn app_with_synthetic_line(dir: &Path, text: &str, synthetic_at: &[usize]) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder page"]);
        app.set_page_content(
            0,
            PageContent {
                lines: vec![text_line_with_synthetic(100.0, text, synthetic_at)],
                ..Default::default()
            },
        );
        app
    }

    /// The text the focus span currently covers, across however many lines.
    ///
    /// Goes through the production `App::span_text`, so these tests also pin down
    /// what a stored highlight records as its text.
    fn span_text(app: &mut App) -> String {
        let (start, end) = app.focus_span().unwrap();
        app.span_text(start, end)
    }

    // ---- Abbreviations ----------------------------------------------------

    #[test]
    fn dotted_initials_do_not_break_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Use a glob, e.g. the star form. Then stop.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "Use a glob, e.g. the star form.");
    }

    #[test]
    fn dotted_initials_are_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Use a glob, e.g. the star form.");
        press(&mut app, "cw");
        press(&mut app, "www"); // Use, a, glob, then the comma
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "e.g.");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "the");
    }

    #[test]
    fn a_known_abbreviation_does_not_break_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "As shown in Fig. 3 the glob wins. Then stop.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "As shown in Fig. 3 the glob wins.");
    }

    #[test]
    fn an_abbreviation_still_ends_a_sentence_before_a_capital() {
        // `etc.` genuinely closes this one, and the capital says so.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Globs, magic, etc. The next sentence here.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "Globs, magic, etc.");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "The next sentence here.");
    }

    #[test]
    fn an_abbreviation_before_a_lower_case_word_keeps_the_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Globs, magic, etc. and then more. Next.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "Globs, magic, etc. and then more.");
    }

    #[test]
    fn a_bracketed_abbreviation_is_one_word_between_its_brackets() {
        // `(e.g.,` is three stops: the bracket, the abbreviation, the comma.
        // The punctuation around it is not part of the construct, and the
        // construct does not dissolve into it either.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Many globs (e.g., the star form) work.");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "globs");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "(");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "e.g.");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), ",");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "the");
    }

    #[test]
    fn a_bracketed_abbreviation_does_not_break_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Many globs (e.g., the star) work. Then stop.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "Many globs (e.g., the star) work.");
    }

    #[test]
    fn an_abbreviation_before_a_comma_keeps_the_sentence_even_before_a_capital() {
        // The citation form of the same construct: a comma cannot open a
        // sentence, so the capitalised author name is not a new one.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Recent work (e.g., Smith 2020) shows it. Next.");
        press(&mut app, "cs");
        assert_eq!(
            span_text(&mut app),
            "Recent work (e.g., Smith 2020) shows it."
        );
    }

    #[test]
    fn a_bracket_after_an_abbreviation_can_still_close_a_sentence() {
        // The `)` is beside the construct, not inside it, so it behaves as it
        // would anywhere else: a closing bracket after a stop ends the group.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Globs and magic (etc.) Then more.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "Globs and magic (etc.)");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "Then more.");
    }

    #[test]
    fn a_quoted_abbreviation_is_still_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Globs, magic, [etc.] and more.");
        press(&mut app, "cw");
        press(&mut app, "wwww"); // comma, magic, comma, then the bracket
        assert_eq!(span_text(&mut app), "[");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "etc.");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "]");
    }

    #[test]
    fn an_ordinary_word_before_a_lower_case_word_still_ends_the_sentence() {
        // The capital test is consulted only where an abbreviation is already
        // suspected. Applied to prose at large it would merge these two, which
        // is exactly what a technical document does constantly.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "It scans all the data. glob rules follow.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "It scans all the data.");
    }

    #[test]
    fn a_decimal_number_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "pi is 3.14 exactly");
        press(&mut app, "cw");
        press(&mut app, "ww"); // "pi", "is", then the number
        assert_eq!(span_text(&mut app), "3.14");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "exactly");
    }

    #[test]
    fn a_grouped_number_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "about 1,234.56 units");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "1,234.56");
    }

    #[test]
    fn a_full_stop_after_a_number_is_still_its_own_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "it costs 3. Next");
        press(&mut app, "cw");
        press(&mut app, "ww");
        assert_eq!(span_text(&mut app), "3");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), ".");
    }

    #[test]
    fn a_decimal_point_does_not_end_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "pi is 3.14 exactly. Then more.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "pi is 3.14 exactly.");
    }

    #[test]
    fn a_dotted_version_token_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "see B-VII.0 next");
        press(&mut app, "cw");
        press(&mut app, "w"); // "see", then the version token
        assert_eq!(span_text(&mut app), "B-VII.0");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "next");
    }

    #[test]
    fn a_dotted_version_token_does_not_end_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(
            dir.path(),
            "released in 2006 (ENDF/B-VII.0), 2011 (ENDF/B-VII.1) and 2018.",
        );
        press(&mut app, "cs");
        assert_eq!(
            span_text(&mut app),
            "released in 2006 (ENDF/B-VII.0), 2011 (ENDF/B-VII.1) and 2018."
        );
    }

    #[test]
    fn a_dotted_filename_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "open file.txt now");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "file.txt");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "now");
    }

    #[test]
    fn a_dotted_token_survives_a_synthetic_space_before_its_separator() {
        // MuPDF drew "9p4kxc2cvd" and ".1" as separate positioning runs (as
        // it does for a DOI split across `Tj` operations) and guessed a
        // space between them. It is not an authored word boundary, so the
        // token still steps as one word for `w`.
        let dir = tempfile::tempdir().unwrap();
        let text = "id 9p4kxc2cvd .1 done";
        let synthetic_space = text.find(" .1").unwrap();
        let mut app = app_with_synthetic_line(dir.path(), text, &[synthetic_space]);
        press(&mut app, "cw");
        press(&mut app, "w"); // "id", then the dotted token
        assert_eq!(span_text(&mut app), "9p4kxc2cvd .1");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "done");
    }

    #[test]
    fn a_synthetic_space_does_not_merge_two_unrelated_words() {
        // The false-positive this fix must not introduce: MuPDF's SYNTHETIC
        // flag is a general per-character signal, not URL-specific, so a
        // document where MuPDF happens to flag an ordinary inter-word gap as
        // synthetic must still see two separate words, not one welded
        // together. Neither side of this gap is a number, hyphen or dotted
        // token, so nothing should bridge it.
        let dir = tempfile::tempdir().unwrap();
        let text = "cache Foo next";
        let synthetic_space = text.find(' ').unwrap();
        let mut app = app_with_synthetic_line(dir.path(), text, &[synthetic_space]);
        press(&mut app, "cw");
        assert_eq!(span_text(&mut app), "cache");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "Foo");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "next");
    }

    #[test]
    fn a_chain_of_dotted_identifiers_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "see a.b.c next");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "a.b.c");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "next");
    }

    #[test]
    fn a_dotted_token_does_not_join_across_a_space() {
        // "word. Next" — the stop ends the sentence; the capital opens another.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "word. Next");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "word.");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "Next");
    }

    #[test]
    fn a_sentence_ends_across_a_synthetic_space_after_the_stop() {
        // IOP-style layout: MuPDF draws `reactor.` and `Efficacy` as separate
        // runs and flags the gap synthetic. Word motion may still peek through
        // for DOI fragments, but sentence boundaries must not — otherwise the
        // stop is mistaken for a dotted token (`r` + `.` + `E`).
        let dir = tempfile::tempdir().unwrap();
        let text = "reactor. Efficacy next.";
        let synthetic_space = text.find(". ").unwrap() + 1;
        let mut app = app_with_synthetic_line(dir.path(), text, &[synthetic_space]);
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "reactor.");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "Efficacy next.");
    }

    #[test]
    fn a_grouped_number_does_not_end_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "we saw 1,234.56 of them. Then more.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "we saw 1,234.56 of them.");
    }

    #[test]
    fn a_full_stop_after_a_number_still_ends_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "it costs 3. Then more.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "it costs 3.");
    }

    #[test]
    fn a_sentence_motion_steps_over_a_number_rather_than_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "pi is 3.14 exactly. Then more.");
        press(&mut app, "cs");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "Then more.");
    }

    #[test]
    fn a_percentage_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "about 45.5% of them");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "45.5%");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "of");
    }

    #[test]
    fn a_percent_sign_without_a_figure_is_still_its_own_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "the % sign here");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "%");
    }

    #[test]
    fn a_full_stop_after_a_percentage_still_ends_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "It grew 45.5%. Then more.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "It grew 45.5%.");
    }

    #[test]
    fn scientific_notation_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "about 1.5e-10 metres");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "1.5e-10");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "metres");
    }

    #[test]
    fn scientific_notation_with_a_signed_exponent_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "about 2.3E+5 units");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "2.3E+5");
    }

    #[test]
    fn scientific_notation_does_not_end_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "It is 1.5e-10 exactly. Then more.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "It is 1.5e-10 exactly.");
    }

    #[test]
    fn a_plus_after_a_word_is_still_its_own_word() {
        // The exponent rule needs a digit before the `e`, so an identifier that
        // merely ends in one does not absorb the sign after it.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "the cache+1 case");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "cache");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "+");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "1");
    }

    // ---- Links ------------------------------------------------------------

    #[test]
    fn a_url_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "See https://example.com/a?x=1 for more");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "https://example.com/a?x=1");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "for");
    }

    #[test]
    fn a_url_does_not_break_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "See https://example.com/a.html for it. Then.");
        press(&mut app, "cs");
        assert_eq!(
            span_text(&mut app),
            "See https://example.com/a.html for it."
        );
    }

    #[test]
    fn a_stop_after_a_url_is_its_own_word_and_ends_the_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "See https://example.com. Then more.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "See https://example.com.");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "https://example.com");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), ".");
    }

    #[test]
    fn a_bracketed_url_is_one_word_between_its_brackets() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "see (https://doi.org/10.1000/182) there");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "(");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "https://doi.org/10.1000/182");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), ")");
    }

    #[test]
    fn a_url_keeps_a_bracket_that_is_part_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "at https://x.org/wiki/Glob_(pattern) today");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "https://x.org/wiki/Glob_(pattern)");
    }

    #[test]
    fn a_url_survives_a_synthetic_space_inside_it() {
        // The abstract-breaking case: a GitHub link drawn as separate
        // positioning runs, with MuPDF guessing a space between "github"
        // and ".com". `token_span` no longer splits on it, so `link_at`
        // sees the whole address and `w` steps over it as one word -- the
        // gap stays in the copied text (it is still real space on the
        // page), but the recognizer itself never saw it.
        let dir = tempfile::tempdir().unwrap();
        let text = "see https://github .com/njoy/ENDFtk today";
        let synthetic_space = text.find(" .com").unwrap();
        let mut app = app_with_synthetic_line(dir.path(), text, &[synthetic_space]);
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "https://github .com/njoy/ENDFtk");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "today");
    }

    #[test]
    fn a_synthetic_space_does_not_end_a_sentence_inside_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let text = "See https://github .com/njoy/ENDFtk today. Then more.";
        let synthetic_space = text.find(" .com").unwrap();
        let mut app = app_with_synthetic_line(dir.path(), text, &[synthetic_space]);
        press(&mut app, "cs");
        assert_eq!(
            span_text(&mut app),
            "See https://github .com/njoy/ENDFtk today."
        );
    }

    #[test]
    fn a_url_survives_a_real_space_inside_it() {
        // Some PDFs draw a URL's path segments with a genuine, authored
        // space character between them -- not MuPDF's synthetic guess, and
        // geometrically identical to an ordinary word space beside it, so
        // neither `synthetic` nor width can tell them apart (a real DOI
        // rendered exactly this way: `https://doi .org /10 .17632
        // /9p4kxc2cvd .1`). `link_at` stitches across it anyway, gated on
        // shape (the next fragment opens with `.` or `/`) and re-validated
        // by `link_span` at each step, not on why there happens to be a gap.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(
            dir.path(),
            "see https://doi .org /10 .17632 /9p4kxc2cvd .1 today",
        );
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(
            span_text(&mut app),
            "https://doi .org /10 .17632 /9p4kxc2cvd .1"
        );
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "today");
    }

    #[test]
    fn a_link_resolves_the_same_span_from_any_fragment_of_a_real_space_chain() {
        // The word-run walk asks about the boundary between `.org` and the
        // space before `/10` just as often as it asks about the boundary
        // right after `https://doi` -- `link_at` must resolve to the same
        // full span from a caret landed on *any* fragment, not just the
        // first, or the chain only stitches together when queried from one
        // specific spot.
        let dir = tempfile::tempdir().unwrap();
        let text = "see https://doi .org /10 .17632 /9p4kxc2cvd .1 today";
        let mut app = app_with_line(dir.path(), text);
        let org_cell = text.find(".org").unwrap() + 1; // inside "org", not the dot
        app.focus = Some(Caret {
            page: 0,
            line: 0,
            cell: org_cell,
        });
        app.focus_scope = Scope::Word;
        app.refresh_focus_span();
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(start.cell, text.find("https").unwrap());
        assert_eq!(end.cell, text.find(" today").unwrap() - 1);
    }

    #[test]
    fn a_real_space_does_not_merge_ordinary_prose_around_a_slash() {
        // The false-positive this mechanism must not introduce: a `/`
        // sitting alone between two ordinary words, spaced on both sides as
        // English prose does ("one and / or two"), must not weld "and" onto
        // it. `link_span("and/")` does not recognise a link, so the
        // extension never gets past its first attempt.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "one and / or two");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "and");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "/");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "or");
    }

    #[test]
    fn a_bare_host_with_a_path_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "see doi.org/10.1000/182 and www.example.com/x");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "doi.org/10.1000/182");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "and");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "www.example.com/x");
    }

    #[test]
    fn an_email_address_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "write to jane.doe@example.com today");
        press(&mut app, "cw");
        press(&mut app, "ww");
        assert_eq!(span_text(&mut app), "jane.doe@example.com");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "today");
    }

    #[test]
    fn a_word_pair_joined_by_a_slash_is_not_a_link() {
        // "and/or" has no host in front of the slash, so it keeps the three
        // stops it has always had.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "one and/or two");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "and");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "/");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "or");
    }

    #[test]
    fn a_hyphenated_compound_is_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "a well-known result");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "well-known");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "result");
    }

    #[test]
    fn a_multiply_hyphenated_compound_is_one_word() {
        // The rule composes: every hyphen with word characters on both sides
        // joins, so the whole chain is a single stop.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "the state-of-the-art method");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "state-of-the-art");
    }

    #[test]
    fn a_hyphen_joins_letters_to_digits() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "the COVID-19 data");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "COVID-19");
    }

    #[test]
    fn a_hyphenated_compound_walks_backwards_as_one_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "a well-known result");
        press(&mut app, "cw");
        press(&mut app, "www"); // a, well-known, result
        press(&mut app, "b");
        assert_eq!(span_text(&mut app), "well-known");
    }

    #[test]
    fn a_spaced_hyphen_is_still_its_own_word() {
        // Nothing to join: a dash used as punctuation keeps its own stop.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "one - two");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "-");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "two");
    }

    #[test]
    fn a_trailing_hyphen_is_still_its_own_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "the well- known result");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "well");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "-");
    }

    #[test]
    fn an_em_dash_between_words_does_not_join_them() {
        // Only hyphens join. A dash separates clauses, so it stays a stop of
        // its own however tightly it is set.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "one—two three");
        press(&mut app, "cw");
        assert_eq!(span_text(&mut app), "one");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "—");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "two");
    }

    #[test]
    fn a_double_hyphen_between_words_does_not_join_them() {
        // `--` is an ASCII dash, not a hyphen: neither one has a word
        // character on both sides, so the pair is punctuation as before.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "one--two three");
        press(&mut app, "cw");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "--");
        press(&mut app, "w");
        assert_eq!(span_text(&mut app), "two");
    }

    #[test]
    fn a_hyphenated_compound_does_not_disturb_sentences() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "It is a well-known result. Then more.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "It is a well-known result.");
    }

    // ---- Page furniture ---------------------------------------------------

    fn app_with_running_header(dir: &Path, pages: usize) -> App {
        let mut app = App::new(Config::default(), Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        let bytes = syodep_pdf::test_support::pdf_with_running_header(pages, true);
        let path = write_pdf_bytes(dir, "header.pdf", bytes);
        app.open_document(&path).unwrap();
        app
    }

    fn page_text_of(app: &App, page: usize) -> String {
        app.content(page)
            .iter()
            .flat_map(|l| l.cells.iter())
            .filter_map(|c| match c.kind {
                CellKind::Char(ch) => Some(ch),
                CellKind::Image => None,
            })
            .collect()
    }

    #[test]
    fn the_caret_never_reaches_a_running_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_running_header(dir.path(), 6);
        press(&mut app, "cw");
        // Walk a whole page; the header and folio must never come up.
        for _ in 0..80 {
            press(&mut app, "w");
        }
        for page in 0..3 {
            let text = page_text_of(&app, page);
            if text.is_empty() {
                continue;
            }
            assert!(
                !text.contains("Shared MIME-info Database"),
                "page {page} still carries the header"
            );
            assert!(
                text.contains("The database is a set"),
                "page {page} lost its body"
            );
        }
    }

    #[test]
    fn focus_enters_on_the_first_body_line_not_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_running_header(dir.path(), 6);
        press(&mut app, "ce");
        let (start, _) = app.focus_span().unwrap();
        let line: String = app.content(start.page)[start.line]
            .cells
            .iter()
            .filter_map(|c| match c.kind {
                CellKind::Char(ch) => Some(ch),
                CellKind::Image => None,
            })
            .collect();
        assert!(
            line.starts_with("The database"),
            "focus entered on {line:?}"
        );
    }

    #[test]
    fn the_furniture_profile_is_learned_once_per_session() {
        // Pages extracted in any order must see the same evidence, or
        // navigation would depend on which page was visited first.
        let dir = tempfile::tempdir().unwrap();
        let mut forwards = app_with_running_header(dir.path(), 6);
        for page in 0..4 {
            forwards.ensure_content(page);
        }
        let mut backwards = app_with_running_header(dir.path(), 6);
        for page in (0..4).rev() {
            backwards.ensure_content(page);
        }
        for page in 0..4 {
            assert_eq!(
                page_text_of(&forwards, page),
                page_text_of(&backwards, page),
                "page {page} differs by visit order"
            );
        }
    }

    #[test]
    fn turning_the_option_off_restores_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.view.skip_page_furniture = false;
        let mut app = App::new(config, Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        let bytes = syodep_pdf::test_support::pdf_with_running_header(6, true);
        let path = write_pdf_bytes(dir.path(), "header.pdf", bytes);
        app.open_document(&path).unwrap();
        app.ensure_content(2);
        assert!(page_text_of(&app, 2).contains("Shared MIME-info Database"));
    }

    // ---- Atomic objects (tables and images) ------------------------------

    /// One content line of `text` at `y`, with a 6pt-wide cell per character.
    fn text_line(y: f32, text: &str) -> ContentLine {
        let cells: Vec<syodep_pdf::Cell> = text
            .chars()
            .enumerate()
            .map(|(i, ch)| {
                let x = 100.0 + i as f32 * 6.0;
                syodep_pdf::Cell {
                    kind: CellKind::Char(ch),
                    bbox: Rect {
                        x0: x,
                        y0: y,
                        x1: x + 6.0,
                        y1: y + 10.0,
                    },
                    synthetic: false,
                }
            })
            .collect();
        ContentLine {
            bbox: Rect {
                x0: 100.0,
                y0: y,
                x1: 100.0 + text.chars().count() as f32 * 6.0,
                y1: y + 10.0,
            },
            cells,
        }
    }

    /// Like [`text_line`], but the cells at the byte indices in
    /// `synthetic_at` (which must be whitespace in `text`) carry MuPDF's
    /// `SYNTHETIC` flag — a guessed inter-glyph gap, not an authored space.
    /// This is what a URL/DOI drawn as several positioning runs looks like
    /// once MuPDF has guessed a space between two of its segments.
    fn text_line_with_synthetic(y: f32, text: &str, synthetic_at: &[usize]) -> ContentLine {
        let mut line = text_line(y, text);
        for &i in synthetic_at {
            line.cells[i].synthetic = true;
        }
        line
    }

    /// A page of prose, then a four-line table, then more prose. Lines 2..=5
    /// are the table; the gaps around it are small enough that the paragraph
    /// heuristic alone would happily merge it into the prose.
    fn table_page_content() -> PageContent {
        let lines = vec![
            text_line(100.0, "Alpha beta."),
            text_line(112.0, "Gamma delta."),
            text_line(126.0, "R1C1 R1C2"),
            text_line(138.0, "R2C1 R2C2"),
            text_line(150.0, "R3C1 R3C2"),
            text_line(162.0, "R4C1 R4C2"),
            text_line(186.0, "Omega final."),
            text_line(198.0, "Last one here."),
        ];
        let objects = vec![ContentObject {
            kind: syodep_pdf::ObjectKind::Table,
            bbox: Rect {
                x0: 96.0,
                y0: 122.0,
                x1: 260.0,
                y1: 176.0,
            },
            start_line: 2,
            end_line: 5,
        }];
        PageContent {
            lines,
            objects,
            ..Default::default()
        }
    }

    fn app_with_table_page(dir: &Path) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder page"]);
        app.set_page_content(0, table_page_content());
        app
    }

    /// The table occupies lines 2..=5 of [`table_page_content`].
    fn in_table(caret: Caret) -> bool {
        (2..=5).contains(&caret.line)
    }

    /// Repeat `key` until the caret is inside the table, and report how many
    /// presses it took. Panics rather than looping forever.
    fn press_into_table(app: &mut App, key: &str) -> usize {
        for presses in 0..10 {
            if in_table(app.caret().unwrap()) {
                return presses;
            }
            press(app, key);
        }
        panic!("{key} never reached the table");
    }

    /// A page whose lines are evenly spaced, so the vertical-gap heuristic
    /// alone would merge the whole page into one paragraph: line 2 is a
    /// numbered heading and lines 5-6 are a heading that wraps.
    fn heading_page_content() -> PageContent {
        let texts = [
            "Alpha beta.",
            "Gamma delta.",
            "2.10. Storing the type",
            "Body text here. More text.",
            "Second body line.",
            "A very long heading that",
            "wraps onto two lines",
            "Final prose line.",
        ];
        let lines: Vec<ContentLine> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| text_line(100.0 + i as f32 * 12.0, t))
            .collect();
        let heading = |start: usize, end: usize| ContentObject {
            kind: ObjectKind::Heading,
            bbox: lines[start].bbox.union(lines[end].bbox),
            start_line: start,
            end_line: end,
        };
        let objects = vec![heading(2, 2), heading(5, 6)];
        PageContent {
            lines,
            objects,
            ..Default::default()
        }
    }

    fn app_with_heading_page(dir: &Path) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder page"]);
        app.set_page_content(0, heading_page_content());
        app
    }

    /// Repeat `key` until the caret reaches `line`, panicking if it never does.
    fn press_until_line(app: &mut App, key: &str, line: usize) {
        for _ in 0..12 {
            if app.caret().unwrap().line == line {
                return;
            }
            press(app, key);
        }
        panic!("{key} never reached line {line}");
    }

    #[test]
    fn sentence_motion_treats_a_heading_as_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "ss"); // past both prose sentences
        assert_caret(&app, 0, 2, 0);
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(
            (start.line, end.line),
            (2, 2),
            "the heading is one sentence"
        );
        press(&mut app, "s");
        assert_eq!(
            app.caret().unwrap().line,
            3,
            "next s must leave the heading"
        );
    }

    #[test]
    fn a_numbered_heading_is_still_one_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "ss");
        // "2.10. Storing the type" would otherwise be three sentences.
        let (start, end) = app.focus_span().unwrap();
        let last = heading_page_content().lines[2].cells.len() - 1;
        assert_eq!((start.line, start.cell), (2, 0));
        assert_eq!((end.line, end.cell), (2, last));
    }

    fn subsection_heading_page_content() -> PageContent {
        // The shape detector's output, as the app sees it: a `1.1.` heading at
        // body size with prose on either side.
        let texts = [
            "Opening prose sentence.",
            "1.1. The ENDF format and nuclear data libraries",
            "Evaluated nuclear data encapsulate the known physics.",
        ];
        let lines: Vec<ContentLine> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| text_line(100.0 + i as f32 * 12.0, t))
            .collect();
        let heading = ContentObject {
            kind: ObjectKind::Heading,
            bbox: lines[1].bbox,
            start_line: 1,
            end_line: 1,
        };
        PageContent {
            lines,
            objects: vec![heading],
            ..Default::default()
        }
    }

    fn app_with_subsection_heading_page(dir: &Path) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder"]);
        app.set_page_content(0, subsection_heading_page_content());
        app
    }

    #[test]
    fn a_body_size_subsection_heading_is_one_sentence_step() {
        // Without the heading region, `s` would stop after `1.1.` and glue the
        // title into the following prose.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_subsection_heading_page(dir.path());

        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "Opening prose sentence.");
        press(&mut app, "s");
        assert_eq!(
            span_text(&mut app),
            "1.1. The ENDF format and nuclear data libraries"
        );
        press(&mut app, "s");
        assert_eq!(
            span_text(&mut app),
            "Evaluated nuclear data encapsulate the known physics."
        );
    }

    #[test]
    fn a_body_size_subsection_heading_is_one_paragraph_step() {
        // Without the heading region, `p` would glue the title into the body
        // below it.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_subsection_heading_page(dir.path());

        press(&mut app, "cp");
        assert_eq!(span_text(&mut app), "Opening prose sentence.");
        press(&mut app, "p");
        assert_eq!(
            span_text(&mut app),
            "1.1. The ENDF format and nuclear data libraries"
        );
        press(&mut app, "p");
        assert_eq!(
            span_text(&mut app),
            "Evaluated nuclear data encapsulate the known physics."
        );
    }

    #[test]
    fn a_sentence_above_a_heading_does_not_run_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "s");
        let (_, end) = app.focus_span().unwrap();
        assert_eq!(end.line, 1, "sentence leaked into the heading");
    }

    #[test]
    fn a_wrapped_heading_is_one_sentence_across_both_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "cs");
        press_until_line(&mut app, "s", 5);
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (5, 6));
    }

    #[test]
    fn paragraph_motion_treats_a_heading_as_one_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "cp");
        // The lines are evenly spaced, so the gap heuristic alone would make
        // the whole page one paragraph: the heading edges are doing the work.
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (0, 1));
        press(&mut app, "p");
        assert_caret(&app, 0, 2, 0);
        assert_eq!(
            {
                let (s, e) = app.focus_span().unwrap();
                (s.line, e.line)
            },
            (2, 2)
        );
        press(&mut app, "p");
        assert_caret(&app, 0, 3, 0);
    }

    #[test]
    fn the_paragraph_below_a_heading_excludes_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "cp");
        press(&mut app, "pp"); // heading, then the prose under it
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (3, 4));
    }

    #[test]
    fn word_motion_still_walks_through_a_heading() {
        // The guard on the whole design: a heading is not atomic, so its
        // words stay individually reachable.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "cw");
        press_until_line(&mut app, "w", 2);
        let mut stops = vec![app.caret().unwrap().cell];
        for _ in 0..3 {
            press(&mut app, "w");
            let caret = app.caret().unwrap();
            if caret.line != 2 {
                break;
            }
            stops.push(caret.cell);
        }
        assert!(
            stops.len() > 1,
            "the heading was one stop, not several words: {stops:?}"
        );
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(start.line, 2);
        assert!(
            end.cell < heading_page_content().lines[2].cells.len() - 1,
            "word scope highlighted the whole heading"
        );
    }

    #[test]
    fn line_motion_still_steps_through_a_wrapped_heading_line_by_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "ce");
        press_until_line(&mut app, "j", 4);
        press(&mut app, "j");
        assert_caret(&app, 0, 5, 0);
        press(&mut app, "j");
        assert_caret(&app, 0, 6, 0);
    }

    #[test]
    fn a_wrapped_heading_draws_one_rectangle_per_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_heading_page(dir.path());
        press(&mut app, "cs");
        press_until_line(&mut app, "s", 5);
        let rects = app.focus_screen_rects().unwrap();
        assert_eq!(
            rects.len(),
            2,
            "a heading is not drawn as one block: {rects:?}"
        );
    }

    // ---- Equations --------------------------------------------------------

    /// A page whose lines are evenly spaced, with a two-line display equation
    /// at lines 2-3. Its first line ends in a full stop, as an equation in a
    /// paper routinely does — a stop in the middle of the construct, and the one
    /// thing `is_one_sentence` has to make inert. Hand-built, like the heading
    /// page, so the behaviour here cannot be broken by the detector's
    /// heuristics.
    fn equation_page_content() -> PageContent {
        let texts = [
            "Alpha beta.",
            "We obtain the bound",
            "f(x) = 0.",
            "g(y) = 1.",
            "Final prose line.",
        ];
        let lines: Vec<ContentLine> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| text_line(100.0 + i as f32 * 12.0, t))
            .collect();
        let objects = vec![ContentObject {
            kind: ObjectKind::Equation,
            bbox: lines[2].bbox.union(lines[3].bbox),
            start_line: 2,
            end_line: 3,
        }];
        PageContent {
            lines,
            objects,
            ..Default::default()
        }
    }

    fn app_with_equation_page(dir: &Path) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder page"]);
        app.set_page_content(0, equation_page_content());
        app
    }

    #[test]
    fn sentence_motion_treats_an_equation_as_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "ss"); // past both prose sentences
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(
            (start.line, end.line),
            (2, 3),
            "the whole equation is one sentence"
        );
        press(&mut app, "s");
        assert_eq!(
            app.caret().unwrap().line,
            4,
            "next s must leave the equation"
        );
    }

    #[test]
    fn a_stop_inside_an_equation_does_not_split_it() {
        // The stop closing the first line would otherwise end a sentence there,
        // leaving the second line of the same equation as another.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "ss");
        let (start, end) = app.focus_span().unwrap();
        let last = equation_page_content().lines[3].cells.len() - 1;
        assert_eq!((start.line, start.cell), (2, 0));
        assert_eq!((end.line, end.cell), (3, last));
    }

    #[test]
    fn a_sentence_above_an_equation_does_not_run_into_it() {
        // The prose line has no full stop, so only the region edge stops it.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "s");
        let (_, end) = app.focus_span().unwrap();
        assert_eq!(end.line, 1, "sentence leaked into the equation");
    }

    #[test]
    fn paragraph_motion_treats_an_equation_as_one_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "cp");
        press(&mut app, "p");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (2, 3));
        press(&mut app, "p");
        assert_caret(&app, 0, 4, 0);
    }

    #[test]
    fn word_motion_still_walks_through_an_equation() {
        // The point of the kind: an equation is a region, not atomic, so its
        // parts stay reachable one word at a time.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "cw");
        press_until_line(&mut app, "w", 2);
        let mut stops = vec![app.caret().unwrap().cell];
        for _ in 0..4 {
            press(&mut app, "w");
            let caret = app.caret().unwrap();
            if caret.line != 2 {
                break;
            }
            stops.push(caret.cell);
        }
        assert!(
            stops.len() > 1,
            "the equation was one stop, not several words: {stops:?}"
        );
    }

    #[test]
    fn line_motion_steps_over_a_whole_equation_with_one_press() {
        // The rows of an aligned system are not reading lines, so line scope
        // treats the formula as one stop the way it does a table.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "ce");
        press(&mut app, "jj");
        assert_caret(&app, 0, 2, 0); // the equation (lines 2..=3), as one stop
        press(&mut app, "j");
        assert_caret(&app, 0, 4, 0); // straight out the far side
        press(&mut app, "k");
        assert_caret(&app, 0, 2, 0);
    }

    #[test]
    fn line_scope_spans_the_whole_equation() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "ce");
        press(&mut app, "jj");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, start.cell), (2, 0));
        assert_eq!(end.line, 3, "span stopped short of the last row");
    }

    #[test]
    fn a_whole_equation_draws_as_one_rectangle() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "ce");
        press(&mut app, "jj");
        let rects = app.focus_screen_rects().unwrap();
        assert_eq!(
            rects.len(),
            1,
            "expected one rect for the equation: {rects:?}"
        );

        // Word scope reaches inside, and its highlight shrinks to the word
        // rather than staying the whole box.
        let whole = rects[0];
        press(&mut app, "cw");
        let rects = app.focus_screen_rects().unwrap();
        assert_eq!(rects.len(), 1);
        assert!(
            rects[0].width < whole.width,
            "word highlight should be smaller than the equation box, got {rects:?}"
        );
    }

    #[test]
    fn char_motion_still_walks_through_an_equation() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_equation_page(dir.path());
        press(&mut app, "cc");
        press_until_line(&mut app, "j", 2);
        press(&mut app, "l");
        assert_caret(&app, 0, 2, 1);
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(
            (start.cell, end.cell),
            (1, 1),
            "char scope highlighted more than one character"
        );
    }

    #[test]
    fn word_motion_walks_into_a_table() {
        // A table is a block, not atomic: word scope reaches the words in its
        // cells rather than stepping over the whole thing in one press.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cw");
        press_into_table(&mut app, "w");
        let first = app.caret().unwrap();
        let mut stops = vec![(first.line, first.cell)];
        for _ in 0..4 {
            press(&mut app, "w");
            let caret = app.caret().unwrap();
            if !in_table(caret) {
                break;
            }
            stops.push((caret.line, caret.cell));
        }
        assert!(
            stops.len() > 1,
            "the table was one stop, not several words: {stops:?}"
        );
    }

    #[test]
    fn word_motion_backwards_walks_back_through_a_table() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cw");
        press_into_table(&mut app, "w");
        let first = app.caret().unwrap();
        press(&mut app, "w");
        let second = app.caret().unwrap();
        assert!(
            in_table(second) && second > first,
            "expected a second word stop inside the table, got {second:?}"
        );
        press(&mut app, "b");
        assert_eq!(
            app.caret().unwrap(),
            first,
            "`b` should step back one word, not out to the table start"
        );
    }

    #[test]
    fn line_motion_steps_over_a_whole_table_with_one_press() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "ce");
        press(&mut app, "jj");
        assert_caret(&app, 0, 2, 0);
        press(&mut app, "j");
        assert_caret(&app, 0, 6, 0);
        press(&mut app, "k");
        assert_caret(&app, 0, 2, 0);
    }

    #[test]
    fn paragraph_motion_treats_a_table_as_one_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cp");
        assert_caret(&app, 0, 0, 0);
        press(&mut app, "p");
        assert_caret(&app, 0, 2, 0);
        press(&mut app, "p");
        assert_caret(&app, 0, 6, 0);
    }

    #[test]
    fn paragraph_span_stops_at_the_table_edge() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cp");
        // The prose paragraph above must not reach into the table.
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (0, 1));
    }

    #[test]
    fn sentence_span_does_not_run_into_a_table() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "s"); // second sentence: "Gamma delta."
        let (_, end) = app.focus_span().unwrap();
        assert_eq!(end.line, 1, "sentence leaked into the table");
        press(&mut app, "s");
        assert_caret(&app, 0, 2, 0);
    }

    #[test]
    fn every_block_scope_spans_the_whole_table() {
        // Line scope and coarser: the table is one unit, so the span — and
        // therefore the highlight — covers all of it. Word scope is excluded
        // on purpose; it walks inside instead.
        let dir = tempfile::tempdir().unwrap();
        for scope in ["ce", "cs", "cp"] {
            let mut app = app_with_table_page(dir.path());
            press(&mut app, scope);
            press_into_table(&mut app, "j");
            let (start, end) = app.focus_span().unwrap();
            assert_eq!(
                (start.line, start.cell, end.line),
                (2, 0, 5),
                "{scope}: span does not cover the whole table"
            );
        }
    }

    #[test]
    fn word_scope_spans_only_a_word_inside_a_table() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cw");
        press_into_table(&mut app, "w");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(start.line, end.line, "a word span crossed table rows");
        assert!(
            end.line < 5 || end.cell + 1 < app.line_cell_count(0, 5),
            "word scope covered the whole table"
        );
    }

    #[test]
    fn char_scope_still_walks_into_table_characters() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "ce");
        press(&mut app, "jj");
        assert_caret(&app, 0, 2, 0);
        // Switching to char scope is the escape hatch into a table's contents.
        press(&mut app, "cc");
        press(&mut app, "l");
        assert_caret(&app, 0, 2, 1);
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, start.cell), (2, 1));
        assert_eq!((end.line, end.cell), (2, 1));
    }

    #[test]
    fn a_count_treats_a_table_as_a_single_unit_at_line_scope() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "ce");
        // Two line stops of prose, the third lands on the table, and the
        // fourth is the prose past it — the table costs one repetition.
        press(&mut app, "3j");
        assert_caret(&app, 0, 6, 0);
    }

    #[test]
    fn next_line_treats_the_table_as_a_single_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cw");
        press_into_table(&mut app, "e");
        let caret = app.caret().unwrap();
        // Every scope stepper lands on a unit's *start*, and a table is one
        // unit: `e` steps onto its first line rather than walking through it
        // line by line.
        assert_eq!(caret.line, 2, "e should land on the table's first line");
        press(&mut app, "e");
        assert_eq!(app.caret().unwrap().line, 6, "next e must leave the table");
    }

    #[test]
    fn motion_never_rests_part_way_through_a_table() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "ce");
        for _ in 0..12 {
            press(&mut app, "j");
            let caret = app.caret().unwrap();
            assert!(
                !(3..=5).contains(&caret.line),
                "caret stranded inside the table at {caret:?}"
            );
        }
    }

    #[test]
    fn visual_selection_covers_a_whole_table_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "ce");
        press(&mut app, "j"); // line 1, the prose above
        press(&mut app, "v");
        press(&mut app, "j"); // extend over the table
        let selection = app.visual_span().unwrap();
        assert_eq!(selection.0.line, 1);
        assert_eq!(
            selection.1.line, 5,
            "selection stops short of the table end"
        );
    }

    #[test]
    fn a_char_scope_visual_end_inside_a_table_is_not_expanded() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cc");
        press(&mut app, "jj"); // char scope walks into the table
        assert_caret(&app, 0, 2, 0);
        press(&mut app, "v");
        press(&mut app, "l");
        let selection = app.visual_span().unwrap();
        assert_eq!(
            (
                selection.0.line,
                selection.0.cell,
                selection.1.line,
                selection.1.cell
            ),
            (2, 0, 2, 1),
            "char scope must stay inside the table"
        );
    }

    #[test]
    fn a_fully_selected_table_draws_as_one_rectangle() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "ce");
        press(&mut app, "jj");
        let rects = app.focus_screen_rects().unwrap();
        assert_eq!(rects.len(), 1, "expected one rect for the table: {rects:?}");

        // A partial (char-scope) selection inside it still draws per line.
        press(&mut app, "cc");
        let rects = app.focus_screen_rects().unwrap();
        assert_eq!(rects.len(), 1);
        assert!(
            rects[0].width < 20.0,
            "char highlight should cover one cell, got {rects:?}"
        );
    }

    #[test]
    fn a_paragraph_no_longer_swallows_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(Config::default(), Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        let path = write_pdf_bytes(dir.path(), "image.pdf", pdf_with_image());
        app.open_document(&path).unwrap();

        press(&mut app, "cp");
        let (start, end) = app.focus_span().unwrap();
        let image_line = app.session.as_ref().unwrap().content[&0]
            .objects
            .iter()
            .find(|o| o.kind == syodep_pdf::ObjectKind::Image)
            .unwrap()
            .start_line;
        assert!(
            !(start.line..=end.line).contains(&image_line),
            "the caption paragraph swallowed the image"
        );
    }

    #[test]
    fn caret_word_motion_treats_images_as_single_stops() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(Config::default(), Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        let path = write_pdf_bytes(dir.path(), "image.pdf", pdf_with_image());
        app.open_document(&path).unwrap();

        press(&mut app, "cc");
        press(&mut app, "w");
        let image = app.caret().unwrap();
        let cell =
            &app.session.as_ref().unwrap().content[&image.page].lines[image.line].cells[image.cell];
        assert_eq!(cell.kind, CellKind::Image);
        press(&mut app, "b");
        assert_caret(&app, 0, 0, 0);
    }

    #[test]
    fn caret_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "cc");
        assert!(app.focus_screen_rect().is_none());
        press(&mut app, "l");
        assert!(app.caret().is_none());
    }

    fn app_with_two_column_page(dir: &Path) -> App {
        let mut app = App::new(Config::default(), Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        let path = dir.join("cols.pdf");
        std::fs::write(&path, syodep_pdf::test_support::pdf_two_column_page(3)).unwrap();
        app.open_document(&path).unwrap();
        app
    }

    #[test]
    fn line_enter_marks_line_and_shows_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        assert_eq!(app.mode(), Mode::Normal);
        // A single `c` is only the first half of `ce`: still pending.
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Normal);
        press(&mut app, "e");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Line));
        let mark = app.line_mark().expect("line marked");
        assert_eq!((mark.page, mark.line), (0, 0));
        assert!(app.focus_screen_rect().is_some());
        assert!(app.status_text().contains("-- FOCUS (line) --"));
        assert!(app.status_text().contains("Ln 1"));
    }

    #[test]
    fn line_vertical_crosses_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "ce");
        // Each page has a single line, so `j` crosses to the next page.
        press(&mut app, "j");
        assert_eq!(app.line_mark().unwrap().page, 1);
        press(&mut app, "k");
        assert_eq!(app.line_mark().unwrap().page, 0);
        // `k` at the document start is clamped.
        press(&mut app, "k");
        assert_eq!(app.line_mark().unwrap().page, 0);
    }

    #[test]
    fn line_exit_restores_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "ce");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Line));
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.focus_screen_rect().is_none());
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        let after = app.session.as_ref().unwrap().view.scroll().1;
        assert!(after > before);
    }

    #[test]
    fn line_focus_keeps_non_hjkl_bindings_and_carries_mark() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);
        press(&mut app, "ce");
        press(&mut app, "G");
        assert_eq!(app.current_page(), 4);
        assert_eq!(app.line_mark().unwrap().page, 4);
        press(&mut app, "gg");
        assert_eq!(app.line_mark().unwrap().page, 0);
        // A full page-down scroll carries the mark to later content.
        press(&mut app, "<C-f>");
        assert!(app.line_mark().unwrap().page > 0);
    }

    #[test]
    fn line_horizontal_is_noop_on_single_column() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 2);
        press(&mut app, "ce");
        let before = app.line_mark().unwrap();
        press(&mut app, "l");
        assert_eq!(app.line_mark().unwrap(), before);
        press(&mut app, "h");
        assert_eq!(app.line_mark().unwrap(), before);
    }

    #[test]
    fn line_horizontal_jumps_columns() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_column_page(dir.path());
        press(&mut app, "ce");
        // Start in the left column on its first line.
        let start = app.line_mark().unwrap();
        // `l` jumps to the right column, keeping the goal row (same first line).
        press(&mut app, "l");
        let right = app.line_mark().unwrap();
        assert_ne!(right.line, start.line, "should move to a right-column line");
        // `l` again is a no-op at the right edge column.
        press(&mut app, "l");
        assert_eq!(app.line_mark().unwrap(), right);
        // `h` returns to the left column.
        press(&mut app, "h");
        assert_eq!(app.line_mark().unwrap(), start);
    }

    #[test]
    fn sentence_horizontal_jumps_columns() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_column_page(dir.path());
        press(&mut app, "cs");
        let start = app.sentence_mark().unwrap();
        press(&mut app, "l");
        let right = app.sentence_mark().unwrap();
        assert_ne!(
            (right.start_line, right.start_cell),
            (start.start_line, start.start_cell),
            "should move to a right-column sentence"
        );
        press(&mut app, "l");
        assert_eq!(app.sentence_mark().unwrap(), right);
        press(&mut app, "h");
        assert_eq!(app.sentence_mark().unwrap(), start);
    }

    #[test]
    fn paragraph_horizontal_jumps_columns() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_column_page(dir.path());
        press(&mut app, "cp");
        let start = app.paragraph_mark().unwrap();
        press(&mut app, "l");
        let right = app.paragraph_mark().unwrap();
        assert_ne!(
            (right.start_line, right.end_line),
            (start.start_line, start.end_line),
            "should move to a right-column paragraph"
        );
        press(&mut app, "l");
        assert_eq!(app.paragraph_mark().unwrap(), right);
        press(&mut app, "h");
        assert_eq!(app.paragraph_mark().unwrap(), start);
    }

    #[test]
    fn sentence_column_jump_tracks_goal_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_column_page(dir.path());
        press(&mut app, "cs");
        // Drop one sentence in the left column, then jump across: land on the
        // right-column sentence nearest that new row, not the top one.
        press(&mut app, "j");
        let left = app.sentence_mark().unwrap();
        press(&mut app, "l");
        let right = app.sentence_mark().unwrap();
        assert_ne!(right.start_line, left.start_line);
        // The left column's second sentence and the right column's second
        // sentence sit on matching rows in the fixture.
        assert_ne!(
            right.start_line, 0,
            "must not reset to the top of the column"
        );
    }

    #[test]
    fn line_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "ce");
        assert!(app.focus_screen_rect().is_none());
        press(&mut app, "j");
        assert!(app.line_mark().is_none());
    }

    #[test]
    fn word_enter_marks_word_and_shows_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        assert_eq!(app.mode(), Mode::Normal);
        // A single `c` is only the first half of `cw`: still pending.
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Normal);
        press(&mut app, "w");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Word));
        let mark = app.word_mark().expect("word marked");
        // The first word "alpha" is the run cells 0..=4.
        assert_eq!(
            (mark.page, mark.line, mark.start_cell, mark.end_cell),
            (0, 0, 0, 4)
        );
        assert!(app.focus_screen_rect().is_some());
        assert!(app.status_text().contains("-- FOCUS (word) --"));
        assert!(app.status_text().contains("Ln 1, Col 1"));
    }

    #[test]
    fn word_enter_starts_at_topmost_visible_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_line_pdf(dir.path(), "alpha beta", "gamma delta");
        let first_line_bottom = app.line_bbox(0, 0).unwrap().y1;
        app.scroll_by_px(0.0, first_line_bottom + 1.0);
        press(&mut app, "cw");
        let mark = app.word_mark().expect("word marked");
        assert_eq!((mark.page, mark.line, mark.start_cell), (0, 1, 0));
    }

    #[test]
    fn word_right_and_left_step_between_words() {
        let dir = tempfile::tempdir().unwrap();
        // A colon, not a hyphen: a hyphen between two words joins them into a
        // single stop -- see `a_hyphenated_compound_is_one_word`.
        let mut app = app_with_text_pages(dir.path(), &["alpha beta:gamma"]);
        press(&mut app, "cw");
        // `l`/`w` advance to the next word run (the "beta" before the colon).
        press(&mut app, "l");
        let mark = app.word_mark().unwrap();
        assert_eq!((mark.start_cell, mark.end_cell), (6, 9));
        press(&mut app, "w");
        let mark = app.word_mark().unwrap();
        // The ":" punctuation run is its own word-like stop.
        assert_eq!(mark.start_cell, 10);
        // `h`/`b` move back.
        press(&mut app, "b");
        let mark = app.word_mark().unwrap();
        assert_eq!((mark.start_cell, mark.end_cell), (6, 9));
        press(&mut app, "h");
        let mark = app.word_mark().unwrap();
        assert_eq!((mark.start_cell, mark.end_cell), (0, 4));
    }

    #[test]
    fn word_right_crosses_lines_and_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_line_pdf(dir.path(), "one two", "three four");
        press(&mut app, "cw");
        press(&mut app, "2l");
        let mark = app.word_mark().unwrap();
        assert_eq!((mark.line, mark.start_cell), (1, 0)); // "three" on line 2

        let page_dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(page_dir.path(), &["one two", "three four"]);
        press(&mut app, "cw");
        press(&mut app, "2l");
        let mark = app.word_mark().unwrap();
        assert_eq!((mark.page, mark.start_cell), (1, 0));
    }

    #[test]
    fn word_vertical_crosses_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta", "gamma delta"]);
        press(&mut app, "cw");
        // Each page has a single line, so `j` crosses to the next page.
        press(&mut app, "j");
        assert_eq!(app.word_mark().unwrap().page, 1);
        press(&mut app, "k");
        assert_eq!(app.word_mark().unwrap().page, 0);
        // `k` at the document start is clamped.
        press(&mut app, "k");
        assert_eq!(app.word_mark().unwrap().page, 0);
    }

    #[test]
    fn word_exit_restores_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "cw");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Word));
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.focus_screen_rect().is_none());
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        let after = app.session.as_ref().unwrap().view.scroll().1;
        assert!(after > before);
    }

    #[test]
    fn word_focus_keeps_non_hjkl_bindings_and_carries_mark() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);
        press(&mut app, "cw");
        press(&mut app, "G");
        assert_eq!(app.current_page(), 4);
        assert_eq!(app.word_mark().unwrap().page, 4);
        press(&mut app, "gg");
        assert_eq!(app.word_mark().unwrap().page, 0);
    }

    /// The bug five separate focus modes made possible: each kept its own mark,
    /// so changing granularity teleported you to wherever you last were at
    /// *that* granularity. One position plus a scope field makes it
    /// unrepresentable — this test would have failed before the collapse.
    #[test]
    fn changing_scope_keeps_the_position() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);
        // Establish a position at char scope on page 0, then leave it behind.
        press(&mut app, "cc");
        assert_eq!(app.focus_caret().unwrap().page, 0);
        // Work at word scope, several pages away.
        press(&mut app, "cw");
        press(&mut app, "G");
        assert_eq!(app.focus_caret().unwrap().page, 4);
        // Every other scope re-reads the position we are actually at.
        for (keys, scope) in [
            ("ce", Scope::Line),
            ("cs", Scope::Sentence),
            ("cp", Scope::Paragraph),
            ("cc", Scope::Char),
        ] {
            press(&mut app, keys);
            assert_eq!(app.focus_scope(), scope);
            assert_eq!(
                app.focus_caret().unwrap().page,
                4,
                "switching to {scope:?} left the stale position behind"
            );
        }
    }

    /// Leaving visual mode by naming a scope keeps your place, exactly as
    /// `<Esc>` does.
    ///
    /// The moving end of a selection *is* the focus position, so there is no
    /// second copy to go stale. When they were separate fields, the `c` chords
    /// read the pre-selection position and silently discarded the head.
    #[test]
    fn leaving_visual_by_scope_chord_keeps_the_position() {
        let dir = tempfile::tempdir().unwrap();
        // Exit at word scope, so the landing cell is the word run containing
        // the head rather than the start of a line — otherwise snapping would
        // hide the bug on a single-line fixture.
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma delta"]);
        press(&mut app, "cw");
        press(&mut app, "vk");
        press(&mut app, "l");
        let head = app.visual_selection().expect("selection").head;
        assert!(head.cell > 0, "the head must have actually moved");
        press(&mut app, "cw");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Word));
        assert_eq!(
            app.focus_caret(),
            Some(head),
            "the scope chord dropped back to the pre-selection position"
        );

        // And across lines, where a line-scope exit is meaningful: the head's
        // line is kept, snapping only the column.
        let mut app = app_with_two_line_pdf(dir.path(), "alpha beta", "gamma delta");
        press(&mut app, "cw");
        assert_eq!(app.focus_caret().unwrap().line, 0);
        press(&mut app, "vk");
        press(&mut app, "j"); // head down to the second line
        let head = app.visual_selection().expect("selection").head;
        assert_eq!(head.line, 1, "the head must have changed line");
        press(&mut app, "ce");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Line));
        assert_eq!(
            app.focus_caret(),
            Some(Caret { cell: 0, ..head }),
            "the scope chord dropped back to the pre-selection line"
        );
    }

    /// The scope follows you out of visual mode, the same way the position
    /// does. `ve` inside a selection means "I am working line-wise now", and
    /// that survives `<Esc>`.
    #[test]
    fn scope_carries_out_of_visual() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma delta"]);
        press(&mut app, "cw");
        press(&mut app, "vk");
        press(&mut app, "ve");
        let head = app.visual_selection().expect("selection").head;
        press(&mut app, "<Esc>");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Line));
        assert_eq!(app.focus_caret(), Some(head));
    }

    /// `oo` exchanges the two ends wholesale — positions *and* scopes — so the
    /// invariant stays "the focus position is the end that moves".
    #[test]
    fn swapping_ends_exchanges_positions_and_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma delta"]);
        press(&mut app, "cc");
        press(&mut app, "vk"); // visual, char scope at both ends
        press(&mut app, "l"); // head moves off the anchor
        press(&mut app, "vw"); // head becomes word-granular
        let before = app.visual_selection().expect("selection");
        let span_before = app.visual_span();
        assert_ne!(before.head, before.anchor);
        assert_eq!(
            (before.head_scope, before.anchor_scope),
            (Scope::Word, Scope::Char)
        );

        press(&mut app, "oo");
        let after = app.visual_selection().expect("selection");
        assert_eq!(after.head, before.anchor);
        assert_eq!(after.anchor, before.head);
        assert_eq!(after.head_scope, before.anchor_scope);
        assert_eq!(after.anchor_scope, before.head_scope);
        // The focus position tracks whichever end is now moving.
        assert_eq!(app.focus_caret(), Some(after.head));
        assert_eq!(app.focus_scope(), after.head_scope);
        // Swapping never changes what is drawn.
        assert_eq!(app.visual_span(), span_before);
    }

    /// Entering visual mode inherits the focus scope, and leaving it hands the
    /// head back — for every scope, not just the ones with their own mode.
    #[test]
    fn visual_inherits_and_returns_every_focus_scope() {
        let dir = tempfile::tempdir().unwrap();
        for (keys, scope) in [
            ("cc", Scope::Char),
            ("cw", Scope::Word),
            ("ce", Scope::Line),
            ("cs", Scope::Sentence),
            ("cp", Scope::Paragraph),
        ] {
            let mut app = app_with_text_pages(dir.path(), &["alpha beta. gamma delta."]);
            press(&mut app, keys);
            // `v` is a binding *and* a prefix, so it fires via longest-prefix
            // fallback on the next key. `k` is that key and cannot move on a
            // one-line document, leaving the head exactly where it entered.
            press(&mut app, "vk");
            assert_eq!(app.mode(), Mode::Visual);
            let sel = app.visual_selection().expect("selection");
            assert_eq!(sel.head_scope, scope, "bare `v` from {keys}");
            assert_eq!(sel.return_mode, Mode::Focus);
            // `<Esc>` returns to focus at the same scope, carrying the head.
            press(&mut app, "<Esc>");
            assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, scope));
            assert_eq!(app.focus_caret(), Some(sel.head));
        }
    }

    /// `w`/`e`/`b` are word-named motions in every scope, mirroring visual
    /// mode: the highlight still snaps to the active scope afterwards.
    #[test]
    fn word_motions_work_at_every_scope() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cs");
        // The whole sentence is highlighted, starting at cell 0.
        let (start, end) = app.focus_span().unwrap();
        assert_eq!(start.cell, 0);
        assert!(end.cell > start.cell);
        // `w` moves a word; the sentence span is unchanged, since the caret is
        // still inside the same sentence.
        press(&mut app, "w");
        assert_eq!(app.focus_caret().unwrap().cell, 6);
        assert_eq!(app.focus_span().unwrap(), (start, end));
    }

    #[test]
    fn word_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "cw");
        assert!(app.focus_screen_rect().is_none());
        press(&mut app, "l");
        assert!(app.word_mark().is_none());
    }

    // ---- Footnotes ----------------------------------------------------

    /// A page of prose, a one-line footnote, then more prose. `s`/`p`
    /// reading through the body must skip the footnote entirely; word and
    /// char scope can still step into it deliberately.
    fn footnote_page_content() -> PageContent {
        let lines = vec![
            text_line(100.0, "Alpha beta gamma."),
            text_line(112.0, "Delta epsilon zeta."),
            text_line(900.0, "1 A footnote sentence here."),
            text_line(940.0, "Eta theta iota."),
        ];
        let objects = vec![ContentObject {
            kind: syodep_pdf::ObjectKind::Footnote,
            bbox: Rect {
                x0: 96.0,
                y0: 892.0,
                x1: 260.0,
                y1: 950.0,
            },
            start_line: 2,
            end_line: 2,
        }];
        PageContent {
            lines,
            objects,
            ..Default::default()
        }
    }

    fn app_with_footnote_page(dir: &Path) -> App {
        let mut app = app_with_text_pages(dir, &["placeholder page"]);
        app.set_page_content(0, footnote_page_content());
        app
    }

    #[test]
    fn sentence_next_skips_a_footnote_block_entirely() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_footnote_page(dir.path());
        press(&mut app, "cs");
        press(&mut app, "s"); // "Delta epsilon zeta."
        assert_caret(&app, 0, 1, 0);
        // Unlike a table, which costs `s` one stop of its own (see
        // `sentence_span_does_not_run_into_a_table`), the footnote must never
        // be landed on at all: this press lands straight past it.
        press(&mut app, "s");
        assert_caret(&app, 0, 3, 0);
    }

    #[test]
    fn paragraph_next_skips_a_footnote_block_entirely() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_footnote_page(dir.path());
        press(&mut app, "cp");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (0, 1));
        // The footnote is its own paragraph (it splits paragraphs like every
        // other kind), but `p` must skip straight past that paragraph too.
        press(&mut app, "p");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (3, 3));
    }

    #[test]
    fn word_motion_can_still_step_into_a_footnote() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_footnote_page(dir.path());
        press(&mut app, "cw");
        for _ in 0..20 {
            if app.caret().unwrap().line == 2 {
                return;
            }
            press(&mut app, "w");
        }
        panic!("word motion never reached the footnote (line 2)");
    }

    #[test]
    fn a_sentence_started_inside_a_footnote_still_expands_normally() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_footnote_page(dir.path());
        press(&mut app, "ce");
        press(&mut app, "jj");
        assert_caret(&app, 0, 2, 0);
        // Switching to sentence scope, still on the footnote's own line, must
        // expand it as an ordinary sentence: deliberate entry is unaffected
        // by the auto-search skip that keeps `s`/`p` from landing here.
        press(&mut app, "cs");
        let (start, end) = app.focus_span().unwrap();
        assert_eq!((start.line, end.line), (2, 2));
        assert_eq!(start.cell, 0);
    }

    // ---- Sentence focus ------------------------------------------------

    #[test]
    fn sentence_enter_marks_sentence_and_shows_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["Alpha beta. Gamma delta."]);
        assert_eq!(app.mode(), Mode::Normal);
        // A single `c` is only the first half of `cs`: still pending.
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Normal);
        press(&mut app, "s");
        assert_eq!(
            (app.mode(), app.focus_scope()),
            (Mode::Focus, Scope::Sentence)
        );
        let mark = app.sentence_mark().expect("sentence marked");
        // The first sentence "Alpha beta." is cells 0..=10 on line 0.
        assert_eq!(
            (mark.page, mark.start_line, mark.start_cell, mark.end_line),
            (0, 0, 0, 0)
        );
        assert!(app.focus_screen_rects().is_some());
        assert!(app.status_text().contains("-- FOCUS (sentence) --"));
    }

    /// `s` and `p` are *motions*, not scope changes: they move by their own
    /// unit whatever the active scope is, and the highlight stays the size the
    /// active scope makes it. That distinction is the whole point of the
    /// feature — `cs` would change both.
    #[test]
    fn sentence_and_paragraph_motions_work_at_every_scope() {
        let dir = tempfile::tempdir().unwrap();
        for (enter, scope) in [
            ("cc", Scope::Char),
            ("cw", Scope::Word),
            ("ce", Scope::Line),
        ] {
            let mut app = app_with_text_pages(dir.path(), &["Alpha beta. Gamma delta."]);
            press(&mut app, enter);
            let (before_start, before_end) = app.focus_span().unwrap();
            let before_width = before_end.cell - before_start.cell;

            press(&mut app, "s");
            assert_eq!(
                (app.mode(), app.focus_scope()),
                (Mode::Focus, scope),
                "`s` must not change the scope"
            );
            // "Gamma delta." starts at cell 12.
            assert_eq!(app.focus_caret().unwrap().cell, 12, "from {enter}");
            // The highlight is still the active scope's size, not a sentence.
            let (start, end) = app.focus_span().unwrap();
            assert_eq!(
                end.cell - start.cell,
                before_width,
                "`s` from {enter} resized the highlight"
            );
        }
    }

    #[test]
    fn paragraph_motion_moves_without_changing_scope() {
        let dir = tempfile::tempdir().unwrap();
        // A two-column page has more than one paragraph (split at the gap).
        let mut app = app_with_two_column_page(dir.path());
        press(&mut app, "cw");
        let before = app.focus_caret().unwrap();
        press(&mut app, "p");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Word));
        assert_ne!(app.focus_caret().unwrap(), before, "`p` did not move");
    }

    #[test]
    fn scope_motions_take_counts() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["Alpha beta. Gamma delta. Epsilon zeta."]);
        press(&mut app, "cw");
        press(&mut app, "2s");
        // Third sentence: "Epsilon zeta." begins at cell 25.
        assert_eq!(app.focus_caret().unwrap().cell, 25);
    }

    #[test]
    fn scope_motions_clamp_at_the_document_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["Alpha beta. Gamma delta."]);
        press(&mut app, "cw");
        press(&mut app, "s");
        let last = app.focus_caret().unwrap();
        // Already on the final sentence: further motion is a no-op, not a wrap
        // and not a crash.
        press(&mut app, "9s");
        assert_eq!(app.focus_caret().unwrap(), last);
    }

    #[test]
    fn sentence_motion_grows_a_selection_in_visual_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["Alpha beta. Gamma delta."]);
        press(&mut app, "cw");
        press(&mut app, "vk");
        let anchor = app.visual_selection().unwrap().anchor;
        press(&mut app, "s");
        let sel = app.visual_selection().unwrap();
        assert_eq!(sel.anchor, anchor, "the anchor must stay put");
        assert_eq!(sel.head.cell, 12, "the head moved a sentence");
        assert_eq!(sel.head_scope, Scope::Word, "the scope is unchanged");
        let (start, end) = app.visual_span().unwrap();
        assert!(end.cell > start.cell, "the selection grew");
    }

    /// A bare `s` and the `cs` chord live on different trie paths, so adding
    /// the motion must not shadow the scope chord.
    #[test]
    fn motion_keys_do_not_shadow_the_scope_chords() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["Alpha beta. Gamma delta."]);
        press(&mut app, "cw");
        let at = app.focus_caret().unwrap();
        // `cs` switches scope in place...
        press(&mut app, "cs");
        assert_eq!(app.focus_scope(), Scope::Sentence);
        assert_eq!(app.focus_caret().unwrap(), at, "`cs` must not move");
        // ...while `s` moves without touching the scope.
        press(&mut app, "s");
        assert_eq!(app.focus_scope(), Scope::Sentence);
        assert_eq!(app.focus_caret().unwrap().cell, 12);
    }

    #[test]
    fn sentence_next_and_prev_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["Alpha beta. Gamma delta."]);
        press(&mut app, "cs");
        assert_eq!(app.sentence_mark().unwrap().start_cell, 0);
        // `j` advances to the next sentence ("Gamma delta." starting at G);
        // `h`/`l` are reserved for column jumps on multi-column pages.
        press(&mut app, "j");
        assert_eq!(app.sentence_mark().unwrap().start_cell, 12);
        // `k` moves back to the first sentence.
        press(&mut app, "k");
        assert_eq!(app.sentence_mark().unwrap().start_cell, 0);
    }

    #[test]
    fn sentence_spans_lines() {
        let dir = tempfile::tempdir().unwrap();
        // The first sentence runs from line 0 (no terminator) into line 1's period.
        let mut app = app_with_two_line_pdf(dir.path(), "Alpha beta", "gamma. Delta");
        press(&mut app, "cs");
        let mark = app.sentence_mark().unwrap();
        assert_eq!(mark.start_line, 0);
        assert_eq!(mark.end_line, 1);
        // A multi-line sentence yields one rect per spanned line.
        let rects = app.focus_screen_rects().unwrap();
        assert!(rects.len() >= 2);
    }

    #[test]
    fn a_line_final_colon_ends_the_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_lines(dir.path(), "A lead-in:", "Continued text here.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "A lead-in:");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "Continued text here.");
    }

    #[test]
    fn a_line_final_colon_with_trailing_spaces_ends_the_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_two_lines(dir.path(), "A lead-in:  ", "Continued text here.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "A lead-in:");
        press(&mut app, "s");
        assert_eq!(span_text(&mut app), "Continued text here.");
    }

    #[test]
    fn a_mid_line_colon_does_not_end_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_line(dir.path(), "Note: more words here.");
        press(&mut app, "cs");
        assert_eq!(span_text(&mut app), "Note: more words here.");
    }

    #[test]
    fn a_line_final_colon_starts_a_new_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        // Tight spacing: without the colon rule these would be one paragraph.
        let mut app = app_with_two_lines(dir.path(), "A lead-in:", "Continued text here.");
        press(&mut app, "cp");
        let mark = app.paragraph_mark().unwrap();
        assert_eq!(
            (mark.start_line, mark.end_line),
            (0, 0),
            "colon line should be its own paragraph"
        );
        press(&mut app, "j");
        let next = app.paragraph_mark().unwrap();
        assert_eq!((next.start_line, next.end_line), (1, 1));
    }

    #[test]
    fn sentence_next_crosses_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["One sentence.", "Second sentence."]);
        press(&mut app, "cs");
        assert_eq!(app.sentence_mark().unwrap().page, 0);
        // The page has a single sentence, so `j` crosses to the next page.
        press(&mut app, "j");
        assert_eq!(app.sentence_mark().unwrap().page, 1);
        press(&mut app, "k");
        assert_eq!(app.sentence_mark().unwrap().page, 0);
        // `k` at the document start is clamped.
        press(&mut app, "k");
        assert_eq!(app.sentence_mark().unwrap().page, 0);
    }

    #[test]
    fn sentence_exit_restores_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "cs");
        assert_eq!(
            (app.mode(), app.focus_scope()),
            (Mode::Focus, Scope::Sentence)
        );
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.focus_screen_rects().is_none());
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        let after = app.session.as_ref().unwrap().view.scroll().1;
        assert!(after > before);
    }

    #[test]
    fn sentence_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "cs");
        assert!(app.focus_screen_rects().is_none());
        press(&mut app, "l");
        assert!(app.sentence_mark().is_none());
    }

    // ---- Paragraph focus -----------------------------------------------

    #[test]
    fn paragraph_enter_marks_paragraph_and_shows_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["Hello world"]);
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Normal);
        press(&mut app, "p");
        assert_eq!(
            (app.mode(), app.focus_scope()),
            (Mode::Focus, Scope::Paragraph)
        );
        let mark = app.paragraph_mark().expect("paragraph marked");
        assert_eq!((mark.page, mark.start_line), (0, 0));
        assert!(app.focus_screen_rect().is_some());
        assert!(app.status_text().contains("-- FOCUS (paragraph) --"));
    }

    #[test]
    fn paragraph_next_steps_within_page() {
        let dir = tempfile::tempdir().unwrap();
        // A two-column page has more than one paragraph (split at the column gap).
        let mut app = app_with_two_column_page(dir.path());
        press(&mut app, "cp");
        let first = app.paragraph_mark().unwrap();
        press(&mut app, "j");
        let next = app.paragraph_mark().unwrap();
        assert_eq!(next.page, 0, "still on the same page");
        assert_ne!(
            (next.start_line, next.end_line),
            (first.start_line, first.end_line),
            "moved to a different paragraph"
        );
    }

    #[test]
    fn paragraph_next_crosses_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["First page", "Second page"]);
        press(&mut app, "cp");
        assert_eq!(app.paragraph_mark().unwrap().page, 0);
        // Each page is a single paragraph, so `j` crosses to the next page.
        press(&mut app, "j");
        assert_eq!(app.paragraph_mark().unwrap().page, 1);
        press(&mut app, "k");
        assert_eq!(app.paragraph_mark().unwrap().page, 0);
        // `k` at the document start is clamped.
        press(&mut app, "k");
        assert_eq!(app.paragraph_mark().unwrap().page, 0);
    }

    #[test]
    fn paragraph_exit_restores_scrolling() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "cp");
        assert_eq!(
            (app.mode(), app.focus_scope()),
            (Mode::Focus, Scope::Paragraph)
        );
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.focus_screen_rect().is_none());
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        let after = app.session.as_ref().unwrap().view.scroll().1;
        assert!(after > before);
    }

    #[test]
    fn paragraph_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "cp");
        assert!(app.focus_screen_rect().is_none());
        press(&mut app, "j");
        assert!(app.paragraph_mark().is_none());
    }

    // ---- Visual mode ---------------------------------------------------

    #[test]
    fn visual_enter_from_normal_selects_one_character() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        // `v` is a binding and a prefix of `vc`/`vw`/..., so it waits for the
        // next key rather than firing eagerly.
        press(&mut app, "v");
        assert_eq!(app.mode(), Mode::Normal, "still pending");
        assert!(app.status_text().contains('v'), "pending input is shown");
        // `vc` is explicit; either way the scope is char.
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Visual);
        let sel = app.visual_selection().expect("selection");
        assert_eq!(sel.head_scope, Scope::Char);
        assert_eq!(sel.return_mode, Mode::Normal);
        // Anchor and head start together, so the span is a single cell.
        let (start, end) = app.visual_span().unwrap();
        assert_eq!(start, end);
        assert!(app.visual_screen_rects().is_some());
        assert!(app.status_text().contains("-- VISUAL (char) --"));
    }

    /// A pause commits a half-typed sequence, so `c` and `v` can each enter
    /// their mode on their own — keeping whatever scope is live.
    #[test]
    fn pausing_after_c_or_v_enters_the_mode_keeping_the_scope() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta. gamma delta."]);

        // `c` alone from normal mode: focus at char, since normal resets it.
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Normal, "still waiting for a second key");
        assert!(app.handle_timeout().redraw);
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Char));

        // Change the scope, leave, and come back with a bare `c`: the scope is
        // whatever focus mode last had.
        press(&mut app, "cs");
        assert_eq!(app.focus_scope(), Scope::Sentence);
        press(&mut app, "v");
        app.handle_timeout();
        assert_eq!(app.mode(), Mode::Visual);
        assert_eq!(
            app.visual_selection().expect("selection").head_scope,
            Scope::Sentence,
            "a bare `v` inherits the live scope"
        );

        // `c` from visual mode keeps the selection's scope and drops the anchor.
        let head = app.visual_selection().unwrap().head;
        press(&mut app, "c");
        app.handle_timeout();
        assert_eq!(
            (app.mode(), app.focus_scope()),
            (Mode::Focus, Scope::Sentence)
        );
        assert_eq!(app.focus_caret(), Some(head));
        assert!(app.visual_selection().is_none());
    }

    /// Typing a sequence at speed must never reach the pause, and the shell is
    /// told exactly when a pause could do something.
    #[test]
    fn pending_input_effect_tracks_partial_sequences() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        // Half a chord: the shell should arm its timer.
        assert!(press(&mut app, "c").pending_input);
        // Completing it disarms, and `cw` means word focus -- not "focus, then
        // move a word", which is what a pause between the keys would give.
        let effects = press(&mut app, "w");
        assert!(!effects.pending_input);
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Word));
        assert_eq!(
            app.focus_caret().unwrap().cell,
            0,
            "`cw` must not also move"
        );
        // A count on its own is not a partial sequence: nothing for a pause to
        // resolve, so no timer.
        assert!(!press(&mut app, "5").pending_input);
    }

    /// Normal mode has no granularity, so it does not remember one: a scope
    /// used before an `<Esc>` must not leak into the next thing you do.
    #[test]
    fn scope_resets_when_returning_to_normal() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta. gamma delta."]);

        // From focus mode.
        press(&mut app, "cs");
        assert_eq!(app.focus_scope(), Scope::Sentence);
        press(&mut app, "<Esc>");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Normal, Scope::Char));
        // So a bare `v` now selects by character, not by sentence.
        press(&mut app, "vk");
        assert_eq!(
            app.visual_selection().expect("selection").head_scope,
            Scope::Char
        );

        // And from visual mode entered straight from normal.
        press(&mut app, "vs");
        assert_eq!(app.focus_scope(), Scope::Sentence);
        press(&mut app, "<Esc>");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Normal, Scope::Char));

        // Leaving visual back into *focus* still carries the scope: focus does
        // have a granularity, so there is something to remember.
        press(&mut app, "cw");
        press(&mut app, "vk");
        press(&mut app, "ve");
        press(&mut app, "<Esc>");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Line));
    }

    #[test]
    fn visual_enter_inherits_the_focus_modes_scope() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        press(&mut app, "cw");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Word));
        // A bare `v` inherits word granularity. `vk` is unbound, so `v` fires
        // via the longest-prefix fallback and `k` replays as a motion.
        press(&mut app, "vk");
        assert_eq!(app.mode(), Mode::Visual);
        let sel = app.visual_selection().unwrap();
        assert_eq!(sel.head_scope, Scope::Word);
        assert_eq!(sel.return_mode, Mode::Focus);
        // `k` had nowhere to go on a one-line document, so the span is still
        // exactly the entered word.
        let (start, end) = app.visual_span().unwrap();
        assert_eq!((start.cell, end.cell), (0, 4), "\"alpha\" is cells 0..=4");
        assert!(app.status_text().contains("-- VISUAL (word) --"));
    }

    #[test]
    fn visual_explicit_scope_overrides_the_inherited_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        press(&mut app, "ve");
        let sel = app.visual_selection().unwrap();
        assert_eq!(sel.head_scope, Scope::Line);
        // A whole line is selected from the very first entry.
        let (start, end) = app.visual_span().unwrap();
        assert_eq!(start.cell, 0);
        assert!(end.cell > 0, "line scope reaches the last cell");
    }

    #[test]
    fn visual_motion_extends_by_the_active_scope() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "vw");
        let (_, end_before) = app.visual_span().unwrap();
        assert_eq!(end_before.cell, 4, "\"alpha\"");
        press(&mut app, "l");
        let (start, end) = app.visual_span().unwrap();
        assert_eq!(start.cell, 0, "the anchor stays put");
        assert_eq!(end.cell, 9, "now through \"beta\"");
    }

    #[test]
    fn visual_counts_repeat_the_motion() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "vw");
        press(&mut app, "2l");
        let (_, end) = app.visual_span().unwrap();
        assert_eq!(end.cell, 15, "through \"gamma\"");
    }

    #[test]
    fn visual_swap_ends_does_not_change_the_span() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "vw");
        press(&mut app, "l");
        let before = app.visual_span().unwrap();
        // `oo` swaps without waiting for a motion.
        press(&mut app, "oo");
        assert_eq!(app.visual_span().unwrap(), before, "`o` is a render no-op");
        // The ends really did swap: the head is now the earlier one.
        let sel = app.visual_selection().unwrap();
        assert!(sel.head < sel.anchor);
    }

    #[test]
    fn visual_swap_then_motion_moves_the_other_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "vw");
        press(&mut app, "l");
        let (start_before, end_before) = app.visual_span().unwrap();
        // After swapping, `l` shrinks the selection from the front instead of
        // growing it at the back.
        press(&mut app, "oo");
        press(&mut app, "l");
        let (start, end) = app.visual_span().unwrap();
        assert!(start > start_before, "the front end moved");
        assert_eq!(end, end_before, "the back end stayed put");
    }

    #[test]
    fn visual_other_scope_changes_only_that_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "ve");
        let (_, end_before) = app.visual_span().unwrap();
        // `ow` switches to the other end and makes it word-granular; the end
        // that is not moving keeps its line scope.
        press(&mut app, "ow");
        let sel = app.visual_selection().unwrap();
        assert_eq!(sel.head_scope, Scope::Word);
        assert_eq!(sel.anchor_scope, Scope::Line);
        assert_eq!(app.visual_span().unwrap().1, end_before);
        assert!(app.status_text().contains("-- VISUAL (word/line) --"));
    }

    #[test]
    fn visual_scope_changes_the_active_end_without_swapping() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "vc");
        let sel_before = app.visual_selection().unwrap();
        press(&mut app, "vw");
        let sel = app.visual_selection().unwrap();
        assert_eq!(sel.head_scope, Scope::Word);
        assert_eq!(sel.anchor_scope, Scope::Char);
        assert_eq!(sel.head, sel_before.head, "no swap");
    }

    #[test]
    fn crossing_and_returning_restores_the_span() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma delta"]);
        // Anchor on the third word, so the head has room to cross it.
        press(&mut app, "cw");
        press(&mut app, "ll");
        press(&mut app, "vw");
        let before = app.visual_span().unwrap();
        // Drag the head back past the anchor and forward again. Because scope
        // belongs to the endpoint rather than to "first"/"last", this is the
        // identity -- the granularities do not silently trade places.
        press(&mut app, "hh");
        assert_ne!(app.visual_span().unwrap(), before, "the head crossed over");
        press(&mut app, "ll");
        assert_eq!(app.visual_span().unwrap(), before);
    }

    #[test]
    fn visual_line_scope_jumps_columns() {
        let dir = tempfile::tempdir().unwrap();
        // Two columns, so there is somewhere for a column jump to land.
        let mut app = app_with_two_column_page(dir.path());
        press(&mut app, "ve");
        let (_, end_before) = app.visual_span().unwrap();
        // `l` in line scope jumps to the next column rather than moving down a
        // line -- line scope now means the same thing in visual as in focus.
        press(&mut app, "l");
        let (_, end) = app.visual_span().unwrap();
        assert_ne!(end, end_before, "the head moved to the other column");
        press(&mut app, "h");
        assert_eq!(
            app.visual_span().unwrap().1,
            end_before,
            "`h` comes back to the first column"
        );
    }

    #[test]
    fn visual_selection_spans_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta", "gamma delta"]);
        press(&mut app, "vw");
        assert_eq!(app.visual_span().unwrap().1.page, 0);
        // Walk off the end of page 1 into page 2.
        press(&mut app, "3l");
        let (start, end) = app.visual_span().unwrap();
        assert_eq!(start.page, 0);
        assert_eq!(end.page, 1, "the selection crosses the page boundary");
    }

    #[test]
    fn visual_rects_cover_only_visible_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(
            dir.path(),
            &["one two", "three four", "five six", "seven eight"],
        );
        press(&mut app, "vw");
        // Select through every page, then check we only draw what is on screen.
        press(&mut app, "20l");
        assert_eq!(app.visual_span().unwrap().1.page, 3);
        let rects = app.visual_screen_rects().expect("rects");
        let visible = app.session.as_ref().unwrap().view.visible_pages().len();
        assert!(
            rects.len() <= visible,
            "one rect per visible line, got {} for {visible} visible pages",
            rects.len()
        );
    }

    #[test]
    fn visual_exit_restores_the_prior_mode_and_carries_the_mark() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        press(&mut app, "vw");
        press(&mut app, "l");
        let head = app.visual_selection().unwrap().head;
        press(&mut app, "<Esc>");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Word));
        assert!(app.visual_screen_rects().is_none());
        // The word mark follows the head rather than snapping back.
        let mark = app.word_mark().expect("word mark carried over");
        assert_eq!(mark.start_cell, head.cell);
    }

    #[test]
    fn visual_exit_from_normal_returns_to_normal() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "vc");
        assert_eq!(app.mode(), Mode::Visual);
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        // Scrolling works again.
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        assert!(app.session.as_ref().unwrap().view.scroll().1 > before);
    }

    #[test]
    fn focus_enter_from_visual_drops_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        press(&mut app, "vw");
        assert!(app.visual_selection().is_some());
        // The focus-mode entry chords stay bound inside visual mode.
        press(&mut app, "ce");
        assert_eq!((app.mode(), app.focus_scope()), (Mode::Focus, Scope::Line));
        assert!(app.visual_selection().is_none());
        assert!(app.visual_screen_rects().is_none());
    }

    #[test]
    fn visual_keeps_non_hjkl_bindings_and_carries_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha", "beta", "gamma"]);
        press(&mut app, "vw");
        // `G` is inherited from the normal keymap and still jumps pages.
        press(&mut app, "G");
        assert_eq!(app.mode(), Mode::Visual, "still selecting");
        assert!(app.current_page() > 0);
    }

    #[test]
    fn replayed_chords_use_the_new_modes_keymap() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        let before = app.word_mark().unwrap();
        // `vj` is unbound, so `v` fires and `j` replays. The replay must
        // resolve against the *visual* keymap: if it used the word-focus one
        // it would move the word mark instead of the selection.
        press(&mut app, "vj");
        assert_eq!(app.mode(), Mode::Visual);
        assert_eq!(
            app.word_mark().unwrap(),
            before,
            "the word mark must not have moved"
        );
        assert!(app.visual_selection().is_some());
    }

    #[test]
    fn visual_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "vc");
        assert!(app.visual_screen_rects().is_none());
        press(&mut app, "l");
        press(&mut app, "oo");
        assert!(app.visual_selection().is_none());
    }

    #[test]
    fn invalid_keybindings_become_startup_warnings() {
        let config = Config::from_toml(
            r#"
            [keys]
            "<Bogus>" = "scroll_down"
            "x" = "no_such_command"
            "#,
        )
        .unwrap();
        let app = App::new(config, None);
        assert_eq!(app.startup_warnings().len(), 2);
    }

    // ---- Highlight mode -------------------------------------------------
    //
    // `press` parses with `parse_sequence`, which has no leader, so the save
    // binding is written out as `<Space>w` here. That is the expansion these
    // tests are checking anyway.

    /// The default highlight colour, as the config defines it.
    fn highlight_color() -> String {
        syodep_config::ViewConfig::default().highlight_color
    }

    #[test]
    fn a_from_focus_mode_starts_a_highlight_over_the_focused_word() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        let span = app.focus_span().expect("word focused");

        press(&mut app, "a");
        assert_eq!(app.mode(), Mode::Highlight);
        assert!(app.has_pending_highlight());
        // Entering synthesises the second end, so the extent is unchanged but
        // every visual motion now applies to it.
        assert_eq!(app.visual_span(), Some(span));
        assert!(app.highlight_screen_rects().is_some());
        // Nothing is stored until it is committed.
        assert!(app.highlights().is_empty());
        assert!(app.status_text().contains("-- HIGHLIGHT (word) --"));
    }

    #[test]
    fn a_from_visual_mode_keeps_the_selection_it_had() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "vw");
        press(&mut app, "w");
        let span = app.visual_span().expect("two words selected");
        let anchor = app.visual_selection().unwrap();

        press(&mut app, "a");
        assert_eq!(app.mode(), Mode::Highlight);
        assert_eq!(app.visual_span(), Some(span));
        assert_eq!(app.visual_selection().unwrap(), anchor);
    }

    #[test]
    fn highlight_mode_reshapes_exactly_as_visual_mode_does() {
        // The invariant behind binding `[highlight_keys]` to the `visual_*`
        // commands: there is one implementation of reshaping a selection, so the
        // same keys must produce the same extent in both modes.
        let dir = tempfile::tempdir().unwrap();
        for keys in ["l", "w", "e", "b", "j", "oo", "ol", "ow", "3l"] {
            let mut visual = app_with_text_pages(dir.path(), &["alpha beta gamma delta"]);
            press(&mut visual, "vw");
            press(&mut visual, keys);

            let mut highlight = app_with_text_pages(dir.path(), &["alpha beta gamma delta"]);
            press(&mut highlight, "vw");
            press(&mut highlight, "a");
            press(&mut highlight, keys);

            assert_eq!(highlight.mode(), Mode::Highlight, "for {keys:?}");
            assert_eq!(
                highlight.visual_span(),
                visual.visual_span(),
                "reshaping with {keys:?} must match visual mode"
            );
        }
    }

    #[test]
    fn a_second_a_stores_the_highlight_and_returns_to_focus_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        press(&mut app, "a");
        press(&mut app, "w");
        let head = app.focus_caret().expect("head after growing");

        press(&mut app, "a");
        assert_eq!(app.mode(), Mode::Focus, "back to focus on the moving end");
        assert!(!app.has_pending_highlight());
        assert!(app.visual_selection().is_none());
        assert!(app.visual_span().is_none());
        assert_eq!(app.focus_caret(), Some(head));

        assert_eq!(app.highlights().len(), 1);
        let stored = &app.highlights()[0];
        assert_eq!(stored.color, highlight_color());
        assert_eq!(stored.text, "alpha beta");
        assert!(!stored.rects.is_empty());
        assert!(stored.rects.iter().all(|r| r.page == 0));
        assert!(stored.id.is_some(), "persisted to the database");
        // The stored highlight keeps being drawn now that the pending one is gone.
        assert!(app.highlight_screen_rects().is_some());
    }

    #[test]
    fn escape_and_backspace_restore_what_a_was_pressed_on() {
        for undo in ["<Esc>", "<BS>"] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma delta"]);
            // From visual mode, so there is an anchor that must survive intact.
            press(&mut app, "vw");
            press(&mut app, "w");
            let before = (
                app.mode(),
                app.focus_caret(),
                app.focus_scope(),
                app.visual_selection(),
                app.visual_span(),
            );

            press(&mut app, "a");
            // Reshape first: the restore must undo the motions too, not just the
            // mode change.
            press(&mut app, "3l");
            assert_ne!(app.visual_span(), before.4);

            press(&mut app, undo);
            assert_eq!(
                (
                    app.mode(),
                    app.focus_caret(),
                    app.focus_scope(),
                    app.visual_selection(),
                    app.visual_span(),
                ),
                before,
                "{undo} must restore the mode and the selection"
            );
            assert!(app.highlights().is_empty(), "{undo} must store nothing");
            assert!(!app.has_pending_highlight());
        }
    }

    #[test]
    fn discarding_from_focus_mode_takes_the_synthesised_anchor_away_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        let span = app.focus_span().unwrap();
        press(&mut app, "a");
        press(&mut app, "l");
        press(&mut app, "<Esc>");

        assert_eq!(app.mode(), Mode::Focus);
        assert_eq!(app.focus_span(), Some(span));
        // Entering from focus mode invented the second end, so discarding must
        // uninvent it — otherwise focus mode would be left holding a selection.
        assert!(app.visual_selection().is_none());
        assert!(app.visual_span().is_none());
        assert!(app.visual_screen_rects().is_none());
    }

    #[test]
    fn v_and_c_store_the_highlight_and_switch_mode() {
        let dir = tempfile::tempdir().unwrap();

        // `vw`: keep it, carry on selecting, head now word-granular.
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cc");
        press(&mut app, "a");
        press(&mut app, "l");
        let span = app.visual_span().unwrap();
        press(&mut app, "vw");
        assert_eq!(app.mode(), Mode::Visual);
        assert_eq!(app.focus_scope(), Scope::Word);
        assert_eq!(app.highlights().len(), 1, "the highlight was kept");
        // The selection survives: the head's scope grew it, it was not collapsed.
        let after = app.visual_span().unwrap();
        assert!(
            after.0 <= span.0 && after.1 >= span.1,
            "{after:?} vs {span:?}"
        );

        // `cw`: keep it, and go to focus mode, which has no second end.
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "vc");
        press(&mut app, "a");
        press(&mut app, "cw");
        assert_eq!(app.mode(), Mode::Focus);
        assert_eq!(app.focus_scope(), Scope::Word);
        assert_eq!(app.highlights().len(), 1);
        assert!(app.visual_selection().is_none());
    }

    #[test]
    fn bare_v_out_of_highlight_mode_keeps_the_selection() {
        // The regression the early branch in `enter_visual` exists for: its
        // ordinary path collapses the selection onto the head, which would throw
        // away the extent the user just built.
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "vw");
        press(&mut app, "w");
        press(&mut app, "a");
        let span = app.visual_span().unwrap();
        press(&mut app, "v");
        // `v` is also a prefix, so the pause is what resolves it.
        app.handle_timeout();
        assert_eq!(app.mode(), Mode::Visual);
        assert_eq!(app.visual_span(), Some(span));
        assert_eq!(app.highlights().len(), 1);
    }

    #[test]
    fn a_is_unbound_in_normal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        // Leave a focus position behind, then return to normal: `a` must not
        // resurrect it as a highlight.
        press(&mut app, "cw");
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        press(&mut app, "a");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(!app.has_pending_highlight());
        assert!(app.highlights().is_empty());
    }

    #[test]
    fn highlight_commands_without_a_document_do_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "a");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.highlight_screen_rects().is_none());
        press(&mut app, "<Space>w");
        assert!(app.highlights().is_empty());
    }

    #[test]
    fn a_highlight_can_span_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta", "gamma delta"]);
        press(&mut app, "cw");
        press(&mut app, "a");
        // Four words forward crosses onto the second page.
        press(&mut app, "4w");
        press(&mut app, "a");

        let stored = &app.highlights()[0];
        let pages: Vec<usize> = stored.rects.iter().map(|r| r.page).collect();
        assert!(pages.contains(&0) && pages.contains(&1), "{pages:?}");
        assert!(stored.text.contains("alpha"));
        assert!(stored.text.contains("gamma"));
    }

    #[test]
    fn stored_highlights_come_back_on_reopen_without_extracting_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_pdf_bytes(
            dir.path(),
            "text.pdf",
            pdf_with_pages(&["alpha beta gamma"]),
        );
        let db = dir.path().join("syodep.sqlite3");

        let stored = {
            let mut app = App::new(Config::default(), Some(Storage::open(&db).unwrap()));
            app.set_viewport_size(595.0, 600.0);
            app.open_document(&path).unwrap();
            press(&mut app, "cw");
            press(&mut app, "aa");
            let stored = app.highlights().to_vec();
            assert_eq!(stored.len(), 1);
            stored
        };

        // A second app on the same database is the "closed and reopened" case:
        // the highlight can only come from storage.
        let mut app = App::new(Config::default(), Some(Storage::open(&db).unwrap()));
        app.set_viewport_size(595.0, 600.0);
        app.open_document(&path).unwrap();
        assert_eq!(app.mode(), Mode::Normal, "a fresh open resets the mode");
        assert_eq!(app.highlights(), stored.as_slice());
        assert!(
            app.highlight_screen_rects().is_some(),
            "stored geometry is enough to draw them, with no content extracted"
        );
    }

    #[test]
    fn saving_embeds_the_highlights_and_reopens_where_you_were() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        let path = app.document_path().unwrap().to_owned();
        let before = std::fs::read(&path).unwrap();

        press(&mut app, "cw");
        press(&mut app, "aa");
        assert_eq!(app.mode(), Mode::Focus);
        let caret = app.focus_caret().unwrap();
        let zoom = app.zoom();

        let effects = press(&mut app, "<Space>w");
        assert!(effects.reload, "the shell must drop its page bitmaps");
        assert!(
            app.status_text().contains("saved 1 highlight"),
            "{}",
            app.status_text()
        );
        assert!(app.last_error().is_none(), "{:?}", app.last_error());

        // The file was rewritten, in place, with a real PDF highlight in it.
        assert_ne!(std::fs::read(&path).unwrap(), before);
        assert_eq!(syodep_pdf::page_highlights(&path, 0).unwrap().len(), 1);
        assert!(
            !path.with_extension("pdf.syodep-tmp").exists(),
            "the temporary file must be gone"
        );

        // Where you were survived the reload.
        assert_eq!(app.mode(), Mode::Focus);
        assert_eq!(app.focus_caret(), Some(caret));
        assert_eq!(app.zoom(), zoom);
        // The highlight lives in the PDF now, so the overlay stops drawing it —
        // otherwise it would be painted twice.
        assert!(app.highlights().is_empty());
        assert!(app.highlight_screen_rects().is_none());
    }

    #[test]
    fn saving_carries_the_reading_position_to_the_new_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_pdf_bytes(
            dir.path(),
            "text.pdf",
            pdf_with_pages(&["alpha beta", "second page"]),
        );
        let db = dir.path().join("syodep.sqlite3");

        let scrolled = {
            let mut app = App::new(Config::default(), Some(Storage::open(&db).unwrap()));
            app.set_viewport_size(595.0, 600.0);
            app.open_document(&path).unwrap();
            press(&mut app, "J"); // a distinctive reading position
            let scrolled = app.current_page();
            assert!(scrolled > 0);
            press(&mut app, "cw");
            press(&mut app, "aa");
            press(&mut app, "<Space>w");
            assert!(app.last_error().is_none(), "{:?}", app.last_error());
            scrolled
        };

        // Rewriting changed the content hash, and documents are keyed by hash.
        // Opening the saved file afresh must still find the position, which only
        // works because the row was re-keyed rather than orphaned.
        let mut app = App::new(Config::default(), Some(Storage::open(&db).unwrap()));
        app.set_viewport_size(595.0, 600.0);
        app.open_document(&path).unwrap();
        assert_eq!(app.current_page(), scrolled);
    }

    #[test]
    fn saving_with_nothing_to_save_leaves_the_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        let path = app.document_path().unwrap().to_owned();
        let before = std::fs::read(&path).unwrap();
        press(&mut app, "<Space>w");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(app.last_error().is_none());
    }

    #[test]
    fn saving_from_highlight_mode_keeps_the_pending_highlight() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        let path = app.document_path().unwrap().to_owned();
        press(&mut app, "cw");
        press(&mut app, "a");
        press(&mut app, "<Space>w");
        assert!(app.last_error().is_none(), "{:?}", app.last_error());
        assert_eq!(app.mode(), Mode::Focus, "the highlight was committed");
        assert!(!app.has_pending_highlight());
        assert!(app.visual_selection().is_none());
        assert_eq!(syodep_pdf::page_highlights(&path, 0).unwrap().len(), 1);
    }

    /// A save that cannot write must leave everything exactly as it was.
    ///
    /// Unix only, because the failure is forced by taking write permission off
    /// the containing directory — the portable-enough way to stop the temporary
    /// file being created. Putting a directory in its place does *not* work:
    /// MuPDF removes whatever is at the path it is told to save to.
    #[cfg(unix)]
    #[test]
    fn a_failed_save_leaves_the_document_open_and_intact() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        let path = app.document_path().unwrap().to_owned();
        press(&mut app, "cw");
        press(&mut app, "aa");
        let before = std::fs::read(&path).unwrap();

        let original = std::fs::metadata(dir.path()).unwrap().permissions();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let effects = press(&mut app, "<Space>w");
        // Restored before asserting, so a failing assertion cannot leave an
        // undeletable temporary directory behind.
        std::fs::set_permissions(dir.path(), original).unwrap();

        assert!(!effects.reload);
        assert!(app.last_error().is_some(), "the failure must be reported");
        assert_eq!(std::fs::read(&path).unwrap(), before, "original untouched");
        assert!(app.has_document(), "still open");
        assert_eq!(app.highlights().len(), 1, "the highlight is still there");
        assert!(!path.with_extension("pdf.syodep-tmp").exists());
    }

    #[test]
    fn the_leader_is_configurable_and_degrades_when_invalid() {
        let dir = tempfile::tempdir().unwrap();
        // A comma leader means `,w` saves and `<Space>w` no longer does.
        let config = Config::from_toml("[input]\nleader = \",\"\n").unwrap();
        let mut app = App::new(config, Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        app.open_document(&write_pdf_bytes(
            dir.path(),
            "text.pdf",
            pdf_with_pages(&["alpha beta"]),
        ))
        .unwrap();
        let path = app.document_path().unwrap().to_owned();
        press(&mut app, "cw");
        press(&mut app, "aa");
        press(&mut app, ",w");
        assert!(app.last_error().is_none(), "{:?}", app.last_error());
        assert_eq!(syodep_pdf::page_highlights(&path, 0).unwrap().len(), 1);

        // An unparseable leader warns and falls back rather than costing the user
        // every `<leader>` binding they have.
        let config = Config::from_toml("[input]\nleader = \"<Nope>\"\n").unwrap();
        let app = App::new(config, None);
        let warnings = app.startup_warnings();
        assert!(
            warnings.iter().any(|w| w.contains("leader")),
            "{warnings:?}"
        );
    }

    // ---- Quit -------------------------------------------------------------

    #[test]
    fn quit_with_no_highlights_quits_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 1);
        let effects = press(&mut app, "<Space>q");
        assert!(effects.quit);
        assert!(!effects.confirm_quit);
    }

    #[test]
    fn quit_with_unsaved_highlights_asks_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        press(&mut app, "aa");
        assert_eq!(app.highlights().len(), 1);

        let effects = press(&mut app, "<Space>q");
        assert!(!effects.quit);
        assert!(effects.confirm_quit);
        assert!(app.has_document(), "still open");
        assert_eq!(app.highlights().len(), 1, "nothing discarded");
    }

    #[test]
    fn quit_commits_a_pending_highlight_before_deciding() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        press(&mut app, "a"); // enter highlight mode, do not commit yet
        assert!(app.has_pending_highlight());

        let effects = press(&mut app, "<Space>q");
        assert!(effects.confirm_quit);
        assert_eq!(app.mode(), Mode::Focus, "the mode fixup ran");
        assert!(!app.has_pending_highlight(), "the highlight was committed");
        assert!(app.visual_selection().is_none());
        assert_eq!(app.highlights().len(), 1);
    }

    #[test]
    fn quit_discarding_highlights_always_quits_and_leaves_highlights_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        press(&mut app, "aa");
        assert_eq!(app.highlights().len(), 1);

        let effects = app.quit_discarding_highlights();
        assert!(effects.quit);
        assert_eq!(
            app.highlights().len(),
            1,
            "discarding does not delete the highlight, only skips embedding it"
        );
    }

    #[test]
    fn save_and_quit_quits_when_the_save_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        let path = app.document_path().unwrap().to_owned();
        press(&mut app, "cw");
        press(&mut app, "aa");

        let effects = app.save_and_quit();
        assert!(effects.quit);
        assert!(effects.reload);
        assert!(app.highlights().is_empty());
        assert_eq!(syodep_pdf::page_highlights(&path, 0).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn save_and_quit_does_not_quit_when_the_save_fails() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta gamma"]);
        press(&mut app, "cw");
        press(&mut app, "aa");

        let original = std::fs::metadata(dir.path()).unwrap().permissions();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let effects = app.save_and_quit();
        std::fs::set_permissions(dir.path(), original).unwrap();

        assert!(!effects.quit);
        assert!(app.last_error().is_some());
        assert_eq!(app.highlights().len(), 1, "the highlight is still there");
        assert!(app.has_document(), "still open");
    }
}
