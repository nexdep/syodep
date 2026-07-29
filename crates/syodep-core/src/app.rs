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
    Bitmap, CellKind, ContentLine, ContentObject, ContentOptions, ObjectKind, PageContent, Rect,
};
use syodep_storage::{Position, Storage};

use crate::caret::{
    column_index_of, column_ranges, continues_word_run, is_sentence_terminator,
    is_sentence_trailer, is_word_target, nearest_cell_in_line, nearest_line_in_column,
    paragraph_segments, split_segments_at_objects, word_class, Caret, Dir, Landing, LineMark, Mode,
    ObjectId, ParagraphMark, Scope, SentenceMark, VisualAnchor, VisualSelection, WordClass,
    WordMark,
};
use crate::command::Command;
use crate::input::{InputState, KeyOutcome, Keymap, KeymapError};
use crate::layout::{DocumentLayout, PageSize, ScreenRect, View};
use crate::render_cache::RenderCache;

/// Hard bound on how many raw steps one atomic step may take to leave an
/// object. Only a runaway detection could ever approach it; it exists so the
/// loop terminates no matter what the per-scope steppers do.
const MAX_ATOMIC_STEPS: usize = 4096;

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

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Pdf(#[from] syodep_pdf::PdfError),
    #[error(transparent)]
    Storage(#[from] syodep_storage::StorageError),
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
    /// Config/keymap problems collected at startup, for the UI to surface.
    startup_warnings: Vec<String>,
    last_error: Option<String>,
}

impl App {
    /// Create the core with an already-loaded config and an optional storage
    /// handle. `storage = None` disables persistence (used by some tests and
    /// as graceful degradation when the database cannot be opened).
    pub fn new(config: Config, storage: Option<Storage>) -> Self {
        let entries = config.keys.iter().map(|(k, v)| (k.as_str(), v.as_str()));
        let (keymap, mut keymap_errors) = Keymap::from_entries(entries);
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
        let startup_warnings = keymap_errors.iter().map(KeymapError::to_string).collect();
        Self {
            config,
            keymap,
            focus_keymap,
            visual_keymap,
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
            startup_warnings,
            last_error: None,
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
        self.last_error = None;
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
            if effects.quit {
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
        let n = count.unwrap_or(1).max(1);
        let step = self.config.view.scroll_step * n as f32;
        let hstep = self.config.view.horizontal_scroll_step * n as f32;
        let zoom_step = self.config.view.zoom_step;

        match command {
            Command::Quit => {
                self.save_position();
                return Effects {
                    quit: true,
                    ..Effects::default()
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
            Command::FocusEndWord => return self.focus_word_end(count),
            Command::FocusNextSentence => {
                return self.focus_scope_motion(Scope::Sentence, Dir::Right, count)
            }
            Command::FocusNextParagraph => {
                return self.focus_scope_motion(Scope::Paragraph, Dir::Right, count)
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
            Command::VisualEndWord => return self.visual_word_end(count),
            Command::VisualNextSentence => {
                return self.visual_scope_motion(Scope::Sentence, Dir::Right, count)
            }
            Command::VisualNextParagraph => {
                return self.visual_scope_motion(Scope::Paragraph, Dir::Right, count)
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
            | Command::FocusEndWord
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
            | Command::VisualEndWord
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
            | Command::VisualOtherParagraph => unreachable!("handled above"),
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
        let opts = ContentOptions {
            detect_tables: self.config.view.detect_tables,
            detect_headings: self.config.view.detect_headings,
        };
        let content = session.doc.page_content(page, opts).unwrap_or_default();
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

    /// Cached atomic objects (tables, images) for `page`.
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

    /// The region containing `line`, but only if motion should treat it as a
    /// single stop — a table or an image, never a heading.
    ///
    /// The distinction is the whole reason headings behave differently from
    /// tables: everything that moves or highlights by a *unit* goes through
    /// here, so a heading keeps its words individually reachable.
    fn atomic_object_at(&mut self, page: usize, line: usize) -> Option<ContentObject> {
        self.region_at(page, line).filter(|o| o.kind.is_atomic())
    }

    /// Identity of the region a caret sits in, for "are these two positions in
    /// the same run of content".
    fn region_id_at(&mut self, at: Caret) -> Option<ObjectId> {
        self.region_at(at.page, at.line).map(|o| ObjectId {
            page: at.page,
            start_line: o.start_line,
        })
    }

    /// Identity of the atomic object a caret sits in, for "did we leave it
    /// yet" while stepping.
    fn atomic_id_at(&mut self, at: Caret) -> Option<ObjectId> {
        self.atomic_object_at(at.page, at.line).map(|o| ObjectId {
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
        let Some(left_class) = self.word_class_at(left) else {
            return false;
        };
        let Some(right_class) = self.word_class_at(right) else {
            return false;
        };
        continues_word_run(
            left_class,
            right_class,
            left.page == right.page && left.line == right.line,
        )
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

    fn step_word_end(&mut self, caret: &mut Caret) -> bool {
        let Some(current_class) = self.word_class_at(*caret) else {
            return false;
        };
        let target = if is_word_target(current_class) {
            let end = self.word_run_end(*caret);
            if end != *caret {
                *caret = end;
                return true;
            }
            let Some(next) = self.next_cell(*caret) else {
                return false;
            };
            self.next_word_target_from(next)
        } else {
            let Some(next) = self.next_cell(*caret) else {
                return false;
            };
            self.next_word_target_from(next)
        };
        let Some(target) = target else {
            return false;
        };
        *caret = self.word_run_end(target);
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
        // A heading is exactly one sentence, whatever punctuation it contains:
        // `3.1. Methods` would otherwise be three. The run still stops at the
        // heading's edges, because those are region boundaries.
        if self.in_heading(c) {
            return false;
        }
        let here = match self.char_at(c) {
            Some(ch) if is_sentence_terminator(ch) || is_sentence_trailer(ch) => ch,
            _ => return false,
        };
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

    /// Whether `at` sits inside a heading.
    fn in_heading(&mut self, at: Caret) -> bool {
        self.region_at(at.page, at.line)
            .is_some_and(|o| o.kind == ObjectKind::Heading)
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
        while matches!(self.word_class_at(cur), Some(WordClass::Whitespace) | None) {
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

    /// A page's paragraphs, cut so that no paragraph straddles a table or an
    /// image and each object stands alone.
    fn page_paragraphs(&mut self, page: usize) -> Vec<(usize, usize)> {
        self.ensure_content(page);
        let segs = paragraph_segments(self.content(page));
        split_segments_at_objects(&segs, self.objects(page))
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
            if i + 1 < segs.len() {
                let (s, e) = segs[i + 1];
                *mark = ParagraphMark {
                    page: mark.page,
                    start_line: s,
                    end_line: e,
                };
                return true;
            }
        }
        if let Some(page) = self.next_content_page(mark.page) {
            self.ensure_content(page);
            if let Some(&(s, e)) = self.page_paragraphs(page).first() {
                *mark = ParagraphMark {
                    page,
                    start_line: s,
                    end_line: e,
                };
                return true;
            }
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
            if i > 0 {
                let (s, e) = segs[i - 1];
                *mark = ParagraphMark {
                    page: mark.page,
                    start_line: s,
                    end_line: e,
                };
                return true;
            }
        }
        if let Some(page) = self.prev_content_page(mark.page) {
            self.ensure_content(page);
            if let Some(&(s, e)) = self.page_paragraphs(page).last() {
                *mark = ParagraphMark {
                    page,
                    start_line: s,
                    end_line: e,
                };
                return true;
            }
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
    /// position: the top-most content line in the viewport, falling back to the
    /// first content line of the document.
    fn entry_caret(&mut self) -> Option<Caret> {
        let from_visible = if let Some(view_top) = self.session.as_ref().map(|s| s.view.scroll().1)
        {
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
        // A table or an image is one unit at every scope above char, so the
        // highlight covers all of it however coarse the scope is.
        if scope != Scope::Char {
            if let Some(object) = self.atomic_object_at(at.page, at.line) {
                return (
                    self.object_landing(at.page, object, Landing::Start),
                    self.object_landing(at.page, object, Landing::End),
                );
            }
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
        if scope != Scope::Char {
            if let Some(object) = self.atomic_object_at(at.page, at.line) {
                return self.object_landing(at.page, object, Landing::Start);
            }
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

    /// Move `caret` one unit of `scope` in `dir`, counting a whole table or
    /// image as a single unit.
    ///
    /// This wraps [`Self::step_scope`] rather than changing it: the per-scope
    /// table stays the pure description of what a word, line, sentence or
    /// paragraph is, and atomicity is one rule applied on top of all of them.
    /// Because it has the same signature, counts (`5w`) and every caller keep
    /// working unchanged, and a table costs exactly one repetition.
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
        let from = self.atomic_id_at(*caret);
        if !self.step_scope(caret, scope, dir, goal_x, goal_y) {
            self.land_on_object(caret, Landing::Start);
            return false;
        }
        // Still inside the object we started in: keep going until we leave it,
        // so the whole object costs one step rather than one step per line.
        if from.is_some() {
            let mut guard = 0;
            while self.atomic_id_at(*caret) == from {
                guard += 1;
                // `step_scope` returning true does not guarantee document-order
                // progress (line scope's column jumps move sideways), so the
                // loop needs a hard bound to be provably terminating.
                if guard > MAX_ATOMIC_STEPS || !self.step_scope(caret, scope, dir, goal_x, goal_y) {
                    self.land_on_object(caret, Landing::Start);
                    return false;
                }
            }
        }
        self.land_on_object(caret, Landing::Start);
        true
    }

    /// `e`, counting a whole table or image as one word.
    fn step_word_end_atomic(&mut self, caret: &mut Caret) -> bool {
        let from = self.atomic_id_at(*caret);
        if !self.step_word_end(caret) {
            self.land_on_object(caret, Landing::End);
            return false;
        }
        if from.is_some() {
            let mut guard = 0;
            while self.atomic_id_at(*caret) == from {
                guard += 1;
                if guard > MAX_ATOMIC_STEPS || !self.step_word_end(caret) {
                    self.land_on_object(caret, Landing::End);
                    return false;
                }
            }
        }
        // Landing on the object's *end* means the next `e` leaves it, instead
        // of walking back through the words inside.
        self.land_on_object(caret, Landing::End);
        true
    }

    /// If `caret` sits inside an object, move it to that object's canonical
    /// position, so a caret never rests part-way through one.
    fn land_on_object(&mut self, caret: &mut Caret, land: Landing) {
        if let Some(object) = self.atomic_object_at(caret.page, caret.line) {
            *caret = self.object_landing(caret.page, object, land);
        }
    }

    /// Move `caret` one unit of `scope` in `dir`. Returns false at a document
    /// edge, so callers can stop early on a repeated motion.
    ///
    /// This is the single per-scope motion table. Note two deliberate
    /// asymmetries, both pre-existing:
    ///   * line scope uses `h`/`l` for *column* jumps on multi-column pages
    ///     (keyed on the goal row) and `j`/`k` for lines,
    ///   * sentence and paragraph have no second axis, so all four directions
    ///     collapse to previous/next.
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
            Scope::Sentence => {
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
            Scope::Paragraph => {
                let Some(mut mark) = self.paragraph_mark_containing(caret.page, caret.line) else {
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
        }
    }

    /// A span's bounding box in page points, for scrolling it into view.
    /// `None` when the span's page has no content extracted.
    ///
    /// Multi-line spans give one box covering every line, which is what
    /// `scroll_doc_rect_into_view` wants: it scrolls the minimum amount, so a
    /// span taller than the viewport simply pins its top edge.
    fn span_bbox(&mut self, start: Caret, end: Caret) -> Option<Rect> {
        // A whole table scrolls into view by its own bounds, so its ruling
        // lines and empty cells come along with the text.
        if start.page == end.page {
            if let Some(object) = self.atomic_object_at(start.page, start.line) {
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

    /// Scroll the minimum amount needed to bring a page-space rectangle on
    /// `page` into view.
    fn scroll_page_rect_into_view(&mut self, page: usize, rect: Rect) {
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
        );
    }

    /// An inclusive cell range as one screen rectangle per spanned line,
    /// restricted to the pages currently on screen.
    ///
    /// Walking the viewport rather than the span keeps this O(visible lines)
    /// however long the span is, and avoids forcing content extraction for
    /// pages the reader cannot see.
    fn span_screen_rects(&self, start: Caret, end: Caret) -> Option<Vec<ScreenRect>> {
        let session = self.session.as_ref()?;
        let mut rects = Vec::new();
        for (page, _) in session.view.visible_pages() {
            if page < start.page || page > end.page {
                continue;
            }
            // Content is loaded lazily; a page we have not visited yet simply
            // has nothing to draw.
            let Some(content) = session.content.get(&page) else {
                continue;
            };
            let lines = &content.lines;
            let first_line = if page == start.page { start.line } else { 0 };
            let last_line = if page == end.page {
                end.line
            } else {
                lines.len().saturating_sub(1)
            };
            let mut line_idx = first_line;
            while line_idx <= last_line {
                // A fully covered table or image draws as one rectangle over
                // its own bounds: per-line boxes would leave its rules and
                // empty cells unpainted, which reads as a broken highlight.
                if let Some(object) = content.atomic_object_at(line_idx) {
                    let whole = object.start_line == line_idx
                        && object.end_line <= last_line
                        && !(page == start.page && start.line == line_idx && start.cell > 0)
                        && !(page == end.page
                            && end.line == object.end_line
                            && end.cell + 1
                                < lines.get(object.end_line).map_or(0, |l| l.cells.len()));
                    if whole {
                        if let Some(rect) = session.view.page_rect_to_screen(
                            page,
                            object.bbox.x0,
                            object.bbox.y0,
                            object.bbox.x1,
                            object.bbox.y1,
                        ) {
                            rects.push(rect);
                        }
                        line_idx = object.end_line + 1;
                        continue;
                    }
                }
                let Some(line) = lines.get(line_idx) else {
                    line_idx += 1;
                    continue;
                };
                if line.cells.is_empty() {
                    line_idx += 1;
                    continue;
                }
                let at_start = page == start.page && line_idx == start.line;
                let at_end = page == end.page && line_idx == end.line;
                let (x0, x1) = match (at_start, at_end) {
                    (true, true) => {
                        let s = line.cells.get(start.cell)?.bbox;
                        let e = line.cells.get(end.cell).map_or(s, |c| c.bbox);
                        (s.x0.min(e.x0), s.x1.max(e.x1))
                    }
                    (true, false) => {
                        let s = line.cells.get(start.cell)?.bbox;
                        (s.x0, line.bbox.x1)
                    }
                    (false, true) => {
                        let e = line.cells.get(end.cell)?.bbox;
                        (line.bbox.x0, e.x1)
                    }
                    (false, false) => (line.bbox.x0, line.bbox.x1),
                };
                if let Some(rect) =
                    session
                        .view
                        .page_rect_to_screen(page, x0, line.bbox.y0, x1, line.bbox.y1)
                {
                    rects.push(rect);
                }
                line_idx += 1;
            }
        }
        if rects.is_empty() {
            None
        } else {
            Some(rects)
        }
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

    /// Update the remembered goal row from the focused line, for line-scope
    /// column motion.
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
        // except in line scope, where the axes are swapped: there `h`/`l` jump
        // columns and `j`/`k` set the row those jumps aim at.
        if scope == Scope::Line {
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

    /// `w`/`b`/`e` move a word at a time in *every* scope: they are word-named
    /// motions, and the highlight still snaps to the active scope.
    /// Move by `count` units of `scope`, *whatever the active scope is*.
    ///
    /// This is what `w`/`b`/`s`/`p` are: a motion names its own unit, and the
    /// highlight still snaps back out to the active scope afterwards. It is
    /// the same [`Self::step_scope`] table `hjkl` use — the only difference is
    /// that the scope comes from the command rather than from the mode.
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
        // These are horizontal motions, so they redefine the column `j`/`k`
        // aim at.
        self.update_focus_goal_x(at);
        self.refresh_focus_span();
        self.ensure_focus_visible();
        self.save_position();
        Effects::redraw()
    }

    /// `e`: the end of the current word run, or the next run's end if already
    /// there. The one motion no scope expresses — every scope motion lands on a
    /// unit *start*.
    fn focus_word_end(&mut self, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        let Some(mut at) = self.focus else {
            return self.enter_focus(self.focus_scope);
        };
        let steps = count.unwrap_or(1).max(1);
        for _ in 0..steps {
            if !self.step_word_end_atomic(&mut at) {
                break;
            }
        }
        self.focus = Some(at);
        self.update_focus_goal_x(at);
        self.refresh_focus_span();
        self.ensure_focus_visible();
        self.save_position();
        Effects::redraw()
    }

    /// After a scroll or page jump in focus mode, move the highlight to the
    /// top-most content line now visible, keeping its goal column. Unlike focus
    /// motion this does *not* scroll the view back, so the highlight follows the
    /// scroll rather than fighting it.
    fn reposition_focus_to_viewport(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let view_top = session.view.scroll().1;
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
        // except in line scope, where the axes are swapped: there `h`/`l` jump
        // columns and `j`/`k` set the row those jumps aim at.
        if scope == Scope::Line {
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

    /// `w`/`b`/`e` move the head a word at a time in *every* scope: they are
    /// word-named motions, and the edge still snaps to the active scope.
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

    /// `e` in visual mode: see [`Self::focus_word_end`].
    fn visual_word_end(&mut self, count: Option<u32>) -> Effects {
        let (Some(_), Some(mut head)) = (self.visual, self.focus) else {
            return Effects::default();
        };
        let steps = count.unwrap_or(1).max(1);
        for _ in 0..steps {
            if !self.step_word_end_atomic(&mut head) {
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
            None => out.push_str("no document - press <C-o> to open a PDF"),
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
        if self.mode == Mode::Visual {
            if let Some(a) = self.visual {
                // Both scopes are shown, head first, when the ends differ.
                let head_scope = self.focus_scope;
                if head_scope == a.anchor_scope {
                    out.push_str(&format!("  -- VISUAL ({}) --", head_scope.name()));
                } else {
                    out.push_str(&format!(
                        "  -- VISUAL ({}/{}) --",
                        head_scope.name(),
                        a.anchor_scope.name()
                    ));
                }
            } else {
                out.push_str("  -- VISUAL --");
            }
            if let Some((start, end)) = self.visual_span {
                out.push_str(&format!("  Ln {}-{}", start.line + 1, end.line + 1));
            }
        }
        if self.input.has_pending() {
            out.push_str(&format!("  {}", self.input.pending_display()));
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
        test_support::{pdf_with_image, pdf_with_pages},
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
    fn scroll_commands_move_and_quit_reports_effect() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        let y0 = 0.0;
        press(&mut app, "j");
        let scrolled = app.session.as_ref().unwrap().view.scroll().1;
        assert!(scrolled > y0);
        press(&mut app, "5k");
        assert_eq!(app.session.as_ref().unwrap().view.scroll().1, 0.0);

        let effects = press(&mut app, "q");
        assert!(effects.quit);
    }

    #[test]
    fn zoom_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 2);
        let z0 = app.zoom();
        press(&mut app, "+");
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
        let effects = press(&mut app, "<C-o>");
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
            press(&mut app, "+");
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
        press(&mut app, "+"); // zoom_in does not move the caret
        assert_eq!(app.caret().unwrap(), before);
        press(&mut app, "zw"); // fit_width does not move the caret
        assert_eq!(app.caret().unwrap(), before);
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
    fn caret_end_word_uses_current_then_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["alpha beta"]);
        press(&mut app, "cc");
        press(&mut app, "e");
        assert_caret(&app, 0, 0, 4);
        press(&mut app, "e");
        assert_caret(&app, 0, 0, 9);
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
        press(&mut app, "e");
        assert_caret(&app, 0, 0, 2);
        press(&mut app, "e");
        assert_caret(&app, 0, 0, 2);
        press(&mut app, "w");
        assert_caret(&app, 0, 0, 2);
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
        PageContent { lines, objects }
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
        PageContent { lines, objects }
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

    #[test]
    fn word_motion_treats_a_table_as_one_stop() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cw");
        press_into_table(&mut app, "w");
        assert_caret(&app, 0, 2, 0); // the table, as one stop
        press(&mut app, "w");
        assert_caret(&app, 0, 6, 0); // straight out the far side
    }

    #[test]
    fn word_motion_backwards_lands_on_the_table_start() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cw");
        press_into_table(&mut app, "w");
        press(&mut app, "w");
        assert_caret(&app, 0, 6, 0);
        press(&mut app, "b");
        assert_caret(&app, 0, 2, 0); // its start, never a row in the middle
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
    fn every_coarse_scope_spans_the_whole_table() {
        let dir = tempfile::tempdir().unwrap();
        for scope in ["cw", "ce", "cs", "cp"] {
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
    fn a_count_treats_a_table_as_a_single_unit() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cw");
        // Five word stops follow the starting one ("beta", ".", "Gamma",
        // "delta", "."), the sixth step lands on the table, and the seventh
        // is the prose past it — the table costs exactly one repetition.
        press(&mut app, "7w");
        assert_caret(&app, 0, 6, 0);
    }

    #[test]
    fn word_end_lands_on_the_table_end_then_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_table_page(dir.path());
        press(&mut app, "cw");
        press_into_table(&mut app, "e");
        let caret = app.caret().unwrap();
        assert_eq!(caret.line, 5, "e should rest on the table's last line");
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
        let mut app = app_with_text_pages(dir.path(), &["alpha beta-gamma"]);
        press(&mut app, "cw");
        // `l`/`w` advance to the next word run (the "beta" before the hyphen).
        press(&mut app, "l");
        let mark = app.word_mark().unwrap();
        assert_eq!((mark.start_cell, mark.end_cell), (6, 9));
        press(&mut app, "w");
        let mark = app.word_mark().unwrap();
        // The "-" punctuation run is its own word-like stop.
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
        // `l`/`j` advance to the next sentence ("Gamma delta." starting at G).
        press(&mut app, "l");
        assert_eq!(app.sentence_mark().unwrap().start_cell, 12);
        // `h`/`k` move back to the first sentence.
        press(&mut app, "h");
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
    fn sentence_next_crosses_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_text_pages(dir.path(), &["One sentence.", "Second sentence."]);
        press(&mut app, "cs");
        assert_eq!(app.sentence_mark().unwrap().page, 0);
        // The page has a single sentence, so `l` crosses to the next page.
        press(&mut app, "l");
        assert_eq!(app.sentence_mark().unwrap().page, 1);
        press(&mut app, "h");
        assert_eq!(app.sentence_mark().unwrap().page, 0);
        // `h` at the document start is clamped.
        press(&mut app, "h");
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
}
