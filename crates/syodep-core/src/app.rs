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
use syodep_pdf::{Bitmap, CellKind, ContentLine, Rect};
use syodep_storage::{Position, Storage};

use crate::caret::{
    column_index_of, column_ranges, continues_word_run, is_sentence_terminator,
    is_sentence_trailer, is_word_target, nearest_cell_in_line, nearest_line_in_column,
    paragraph_segments, word_class, Caret, Dir, LineMark, Mode, ParagraphMark, SentenceMark,
    WordClass, WordMark,
};
use crate::command::Command;
use crate::input::{InputState, KeyOutcome, Keymap, KeymapError};
use crate::layout::{DocumentLayout, PageSize, ScreenRect, View};
use crate::render_cache::RenderCache;

/// Side effects the UI shell must perform after an input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effects {
    pub redraw: bool,
    pub quit: bool,
    /// The shell should show a native file-open dialog and call
    /// [`App::open_document`] with the result.
    pub open_file_dialog: bool,
}

impl Effects {
    fn redraw() -> Self {
        Self {
            redraw: true,
            ..Self::default()
        }
    }
}

/// A page to draw, in canvas pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisiblePage {
    pub page: usize,
    pub rect: ScreenRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordMotion {
    NextStart,
    End,
    PrevStart,
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
    content: HashMap<usize, Vec<ContentLine>>,
}

/// Top-level application state. One instance per window.
pub struct App {
    config: Config,
    keymap: Keymap,
    /// Keymap used while in caret focus mode: the normal keymap plus the
    /// `[caret_focus_keys]` overrides (so `hjkl`/`<Esc>` change meaning there).
    caret_focus_keymap: Keymap,
    /// Keymap used while in line focus mode: the normal keymap plus the
    /// `[line_focus_keys]` overrides.
    line_focus_keymap: Keymap,
    /// Keymap used while in word focus mode: the normal keymap plus the
    /// `[word_focus_keys]` overrides.
    word_focus_keymap: Keymap,
    /// Keymap used while in sentence focus mode: the normal keymap plus the
    /// `[sentence_focus_keys]` overrides.
    sentence_focus_keymap: Keymap,
    /// Keymap used while in paragraph focus mode: the normal keymap plus the
    /// `[paragraph_focus_keys]` overrides.
    paragraph_focus_keymap: Keymap,
    input: InputState,
    storage: Option<Storage>,
    session: Option<Session>,
    viewport: (f32, f32),
    /// Whether `hjkl` scroll or move the caret.
    mode: Mode,
    /// Current caret position, remembered across mode toggles.
    caret: Option<Caret>,
    /// Remembered goal column (page-space x) for vertical caret motion.
    caret_goal_x: f32,
    /// Current line-focus position, remembered across mode toggles.
    line_mark: Option<LineMark>,
    /// Remembered goal row (page-space y center) for horizontal column motion.
    line_goal_y: f32,
    /// Current word-focus position, remembered across mode toggles.
    word_mark: Option<WordMark>,
    /// Remembered goal column (page-space x) for vertical word motion.
    word_goal_x: f32,
    /// Current sentence-focus position, remembered across mode toggles.
    sentence_mark: Option<SentenceMark>,
    /// Current paragraph-focus position, remembered across mode toggles.
    paragraph_mark: Option<ParagraphMark>,
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
        // The caret keymap is the normal keymap with the caret-focus overrides
        // applied, so every normal binding still works in caret focus mode and
        // only the overridden keys (hjkl/<Esc>) change meaning. Cloning then
        // overlaying avoids re-validating (and double-reporting) normal keys.
        let mut caret_focus_keymap = keymap.clone();
        let caret_entries = config
            .caret_focus_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(caret_focus_keymap.overlay(caret_entries));
        // The line-focus keymap is built the same way, from `[line_focus_keys]`.
        let mut line_focus_keymap = keymap.clone();
        let line_entries = config
            .line_focus_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(line_focus_keymap.overlay(line_entries));
        // The word-focus keymap is built the same way, from `[word_focus_keys]`.
        let mut word_focus_keymap = keymap.clone();
        let word_entries = config
            .word_focus_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(word_focus_keymap.overlay(word_entries));
        // The sentence- and paragraph-focus keymaps follow the same recipe.
        let mut sentence_focus_keymap = keymap.clone();
        let sentence_entries = config
            .sentence_focus_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(sentence_focus_keymap.overlay(sentence_entries));
        let mut paragraph_focus_keymap = keymap.clone();
        let paragraph_entries = config
            .paragraph_focus_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(paragraph_focus_keymap.overlay(paragraph_entries));
        let startup_warnings = keymap_errors.iter().map(KeymapError::to_string).collect();
        Self {
            config,
            keymap,
            caret_focus_keymap,
            line_focus_keymap,
            word_focus_keymap,
            sentence_focus_keymap,
            paragraph_focus_keymap,
            input: InputState::new(),
            storage,
            session: None,
            viewport: (800.0, 600.0),
            mode: Mode::Normal,
            caret: None,
            caret_goal_x: 0.0,
            line_mark: None,
            line_goal_y: 0.0,
            word_mark: None,
            word_goal_x: 0.0,
            sentence_mark: None,
            paragraph_mark: None,
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
        self.caret = None;
        self.caret_goal_x = 0.0;
        self.line_mark = None;
        self.line_goal_y = 0.0;
        self.word_mark = None;
        self.word_goal_x = 0.0;
        self.sentence_mark = None;
        self.paragraph_mark = None;
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
    pub fn handle_key(&mut self, chord: Chord) -> Effects {
        let keymap = match self.mode {
            Mode::Normal => &self.keymap,
            Mode::CaretFocus => &self.caret_focus_keymap,
            Mode::LineFocus => &self.line_focus_keymap,
            Mode::WordFocus => &self.word_focus_keymap,
            Mode::SentenceFocus => &self.sentence_focus_keymap,
            Mode::ParagraphFocus => &self.paragraph_focus_keymap,
        };
        match self.input.handle(keymap, chord) {
            // Redraw on pending input so the status line shows it.
            KeyOutcome::Pending => Effects::redraw(),
            KeyOutcome::Unmatched => Effects::redraw(),
            KeyOutcome::Command { command, count } => self.execute(command, count),
        }
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
            Command::CaretFocusEnter => return self.enter_caret_focus(),
            Command::CaretFocusExit => {
                self.mode = Mode::Normal;
                return Effects::redraw();
            }
            Command::CaretFocusLeft => return self.caret_move(Dir::Left, count),
            Command::CaretFocusRight => return self.caret_move(Dir::Right, count),
            Command::CaretFocusUp => return self.caret_move(Dir::Up, count),
            Command::CaretFocusDown => return self.caret_move(Dir::Down, count),
            Command::CaretFocusNextWord => {
                return self.caret_word_move(WordMotion::NextStart, count)
            }
            Command::CaretFocusEndWord => return self.caret_word_move(WordMotion::End, count),
            Command::CaretFocusPrevWord => {
                return self.caret_word_move(WordMotion::PrevStart, count)
            }
            Command::LineFocusEnter => return self.enter_line_focus(),
            Command::LineFocusExit => {
                self.mode = Mode::Normal;
                return Effects::redraw();
            }
            Command::LineFocusLeft => return self.line_move(Dir::Left, count),
            Command::LineFocusRight => return self.line_move(Dir::Right, count),
            Command::LineFocusUp => return self.line_move(Dir::Up, count),
            Command::LineFocusDown => return self.line_move(Dir::Down, count),
            Command::WordFocusEnter => return self.enter_word_focus(),
            Command::WordFocusExit => {
                self.mode = Mode::Normal;
                return Effects::redraw();
            }
            Command::WordFocusLeft => return self.word_move(Dir::Left, count),
            Command::WordFocusRight => return self.word_move(Dir::Right, count),
            Command::WordFocusUp => return self.word_move(Dir::Up, count),
            Command::WordFocusDown => return self.word_move(Dir::Down, count),
            Command::SentenceFocusEnter => return self.enter_sentence_focus(),
            Command::SentenceFocusExit => {
                self.mode = Mode::Normal;
                return Effects::redraw();
            }
            Command::SentenceFocusNext => return self.sentence_move(true, count),
            Command::SentenceFocusPrev => return self.sentence_move(false, count),
            Command::ParagraphFocusEnter => return self.enter_paragraph_focus(),
            Command::ParagraphFocusExit => {
                self.mode = Mode::Normal;
                return Effects::redraw();
            }
            Command::ParagraphFocusNext => return self.paragraph_move(true, count),
            Command::ParagraphFocusPrev => return self.paragraph_move(false, count),
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
            | Command::CaretFocusEnter
            | Command::CaretFocusExit
            | Command::CaretFocusLeft
            | Command::CaretFocusRight
            | Command::CaretFocusUp
            | Command::CaretFocusDown
            | Command::CaretFocusNextWord
            | Command::CaretFocusEndWord
            | Command::CaretFocusPrevWord
            | Command::LineFocusEnter
            | Command::LineFocusExit
            | Command::LineFocusLeft
            | Command::LineFocusRight
            | Command::LineFocusUp
            | Command::LineFocusDown
            | Command::WordFocusEnter
            | Command::WordFocusExit
            | Command::WordFocusLeft
            | Command::WordFocusRight
            | Command::WordFocusUp
            | Command::WordFocusDown
            | Command::SentenceFocusEnter
            | Command::SentenceFocusExit
            | Command::SentenceFocusNext
            | Command::SentenceFocusPrev
            | Command::ParagraphFocusEnter
            | Command::ParagraphFocusExit
            | Command::ParagraphFocusNext
            | Command::ParagraphFocusPrev => unreachable!("handled above"),
        }
        // In caret focus mode, scroll and page jumps carry the caret to the
        // newly visible content; zoom commands leave it where it is.
        let moves_caret = matches!(
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
        if self.mode == Mode::CaretFocus && moves_caret {
            self.reposition_caret_to_viewport();
        }
        if self.mode == Mode::LineFocus && moves_caret {
            self.reposition_line_to_viewport();
        }
        if self.mode == Mode::WordFocus && moves_caret {
            self.reposition_word_to_viewport();
        }
        if self.mode == Mode::SentenceFocus && moves_caret {
            self.reposition_sentence_to_viewport();
        }
        if self.mode == Mode::ParagraphFocus && moves_caret {
            self.reposition_paragraph_to_viewport();
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

    // ---- Caret navigation ----------------------------------------------

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn caret(&self) -> Option<Caret> {
        self.caret
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
        let lines = session.doc.page_content(page).unwrap_or_default();
        session.content.insert(page, lines);
    }

    /// Cached content for `page` (empty if absent/uncached).
    fn content(&self, page: usize) -> &[ContentLine] {
        self.session
            .as_ref()
            .and_then(|s| s.content.get(&page))
            .map(Vec::as_slice)
            .unwrap_or(&[])
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

    fn enter_caret_focus(&mut self) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        self.mode = Mode::CaretFocus;
        if self.caret.is_none() {
            let start = self.current_page();
            if let Some(page) = self.content_page_from(start) {
                let caret = Caret {
                    page,
                    line: 0,
                    cell: 0,
                };
                self.caret = Some(caret);
                self.update_goal_x(caret);
            }
        }
        self.ensure_caret_visible();
        self.save_position();
        Effects::redraw()
    }

    fn caret_move(&mut self, dir: Dir, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        // No caret yet (e.g. entered on an empty document): try to place one.
        let Some(mut caret) = self.caret else {
            return self.enter_caret_focus();
        };
        let steps = count.unwrap_or(1).max(1);
        let goal_x = self.caret_goal_x;
        for _ in 0..steps {
            let moved = match dir {
                Dir::Right => self.step_right(&mut caret),
                Dir::Left => self.step_left(&mut caret),
                Dir::Down => self.step_down(&mut caret, goal_x),
                Dir::Up => self.step_up(&mut caret, goal_x),
            };
            if !moved {
                break; // reached a document edge
            }
        }
        self.caret = Some(caret);
        // Horizontal motion sets a new goal column; vertical motion keeps it.
        if matches!(dir, Dir::Left | Dir::Right) {
            self.update_goal_x(caret);
        }
        self.ensure_caret_visible();
        self.save_position();
        Effects::redraw()
    }

    fn caret_word_move(&mut self, motion: WordMotion, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        let Some(mut caret) = self.caret else {
            return self.enter_caret_focus();
        };
        let steps = count.unwrap_or(1).max(1);
        for _ in 0..steps {
            let moved = match motion {
                WordMotion::NextStart => self.step_next_word_start(&mut caret),
                WordMotion::End => self.step_word_end(&mut caret),
                WordMotion::PrevStart => self.step_prev_word_start(&mut caret),
            };
            if !moved {
                break;
            }
        }
        self.caret = Some(caret);
        self.update_goal_x(caret);
        self.ensure_caret_visible();
        self.save_position();
        Effects::redraw()
    }

    /// After a scroll or page jump in caret focus mode, move the caret to the
    /// top-most content line now visible, keeping its goal column. Unlike caret
    /// motion this does *not* scroll the view back, so the caret follows the
    /// scroll rather than fighting it.
    fn reposition_caret_to_viewport(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let view_top = session.view.scroll().1;
        let goal_x = self.caret_goal_x;
        if let Some((page, line)) = self.topmost_visible_line(view_top) {
            let cell = self.nearest_cell(page, line, goal_x);
            self.caret = Some(Caret { page, line, cell });
        }
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

    fn update_goal_x(&mut self, caret: Caret) {
        if let Some(r) = self.cell_rect(caret.page, caret.line, caret.cell) {
            self.caret_goal_x = (r.x0 + r.x1) / 2.0;
        }
    }

    /// Scroll the minimum amount needed to keep the caret on screen.
    fn ensure_caret_visible(&mut self) {
        let Some(caret) = self.caret else {
            return;
        };
        let Some(rect) = self.cell_rect(caret.page, caret.line, caret.cell) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(page) = session.view.layout().page(caret.page) else {
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

    /// The caret's rectangle in canvas pixels, with its page. `None` outside
    /// caret focus mode or when no caret is placed. The shell paints this overlay.
    pub fn caret_screen_rect(&self) -> Option<(usize, ScreenRect)> {
        if self.mode != Mode::CaretFocus {
            return None;
        }
        let caret = self.caret?;
        let session = self.session.as_ref()?;
        let cell = session
            .content
            .get(&caret.page)?
            .get(caret.line)?
            .cells
            .get(caret.cell)?;
        let b = cell.bbox;
        session
            .view
            .page_rect_to_screen(caret.page, b.x0, b.y0, b.x1, b.y1)
            .map(|rect| (caret.page, rect))
    }

    // ---- Line focus navigation -----------------------------------------

    pub fn line_mark(&self) -> Option<LineMark> {
        self.line_mark
    }

    /// Bounding box of a content line in page points (`None` if absent).
    fn line_bbox(&mut self, page: usize, line: usize) -> Option<Rect> {
        self.ensure_content(page);
        self.content(page).get(line).map(|l| l.bbox)
    }

    /// Update the remembered goal row from the marked line's vertical center.
    fn update_goal_y(&mut self, mark: LineMark) {
        if let Some(b) = self.line_bbox(mark.page, mark.line) {
            self.line_goal_y = (b.y0 + b.y1) / 2.0;
        }
    }

    fn enter_line_focus(&mut self) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        self.mode = Mode::LineFocus;
        if self.line_mark.is_none() {
            let start = self.current_page();
            if let Some(page) = self.content_page_from(start) {
                let mark = LineMark { page, line: 0 };
                self.line_mark = Some(mark);
                self.update_goal_y(mark);
            }
        }
        self.ensure_line_visible();
        self.save_position();
        Effects::redraw()
    }

    fn line_move(&mut self, dir: Dir, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        let Some(mut mark) = self.line_mark else {
            return self.enter_line_focus();
        };
        let steps = count.unwrap_or(1).max(1);
        let goal_y = self.line_goal_y;
        for _ in 0..steps {
            let moved = match dir {
                Dir::Down => self.line_step_down(&mut mark),
                Dir::Up => self.line_step_up(&mut mark),
                Dir::Left => self.line_step_column(&mut mark, goal_y, false),
                Dir::Right => self.line_step_column(&mut mark, goal_y, true),
            };
            if !moved {
                break; // document edge, or single-column page for H/L
            }
        }
        self.line_mark = Some(mark);
        // Vertical motion sets a new goal row; horizontal (column) motion keeps it.
        if matches!(dir, Dir::Up | Dir::Down) {
            self.update_goal_y(mark);
        }
        self.ensure_line_visible();
        self.save_position();
        Effects::redraw()
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

    /// After a scroll or page jump in line focus mode, move the mark to the
    /// top-most content line now visible, keeping its goal row.
    fn reposition_line_to_viewport(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let view_top = session.view.scroll().1;
        if let Some((page, line)) = self.topmost_visible_line(view_top) {
            self.line_mark = Some(LineMark { page, line });
        }
    }

    /// Scroll the minimum amount needed to keep the marked line on screen.
    fn ensure_line_visible(&mut self) {
        let Some(mark) = self.line_mark else {
            return;
        };
        let Some(rect) = self.line_bbox(mark.page, mark.line) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(page) = session.view.layout().page(mark.page) else {
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

    /// The marked line's rectangle in canvas pixels, with its page. `None`
    /// outside line focus mode or when no line is marked. The shell paints this.
    pub fn line_screen_rect(&self) -> Option<(usize, ScreenRect)> {
        if self.mode != Mode::LineFocus {
            return None;
        }
        let mark = self.line_mark?;
        let session = self.session.as_ref()?;
        let line = session.content.get(&mark.page)?.get(mark.line)?;
        let b = line.bbox;
        session
            .view
            .page_rect_to_screen(mark.page, b.x0, b.y0, b.x1, b.y1)
            .map(|rect| (mark.page, rect))
    }

    // ---- Word focus navigation -----------------------------------------

    pub fn word_mark(&self) -> Option<WordMark> {
        self.word_mark
    }

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

    /// Update the remembered goal column from the marked word's first cell.
    fn update_word_goal_x(&mut self, mark: WordMark) {
        if let Some(r) = self.cell_rect(mark.page, mark.line, mark.start_cell) {
            self.word_goal_x = (r.x0 + r.x1) / 2.0;
        }
    }

    fn enter_word_focus(&mut self) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        self.mode = Mode::WordFocus;
        if self.word_mark.is_none() {
            let from_visible =
                if let Some(view_top) = self.session.as_ref().map(|s| s.view.scroll().1) {
                    self.topmost_visible_line(view_top)
                        .map(|(page, line)| Caret {
                            page,
                            line,
                            cell: 0,
                        })
                } else {
                    None
                };
            let from = from_visible.or_else(|| {
                let start = self.current_page();
                self.content_page_from(start).map(|page| Caret {
                    page,
                    line: 0,
                    cell: 0,
                })
            });
            if let Some(target) = from.and_then(|from| self.next_word_target_from(from)) {
                let mark = self.word_mark_from_caret(target);
                self.word_mark = Some(mark);
                self.update_word_goal_x(mark);
            }
        }
        self.ensure_word_visible();
        self.save_position();
        Effects::redraw()
    }

    fn word_move(&mut self, dir: Dir, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        let Some(mut mark) = self.word_mark else {
            return self.enter_word_focus();
        };
        let steps = count.unwrap_or(1).max(1);
        let goal_x = self.word_goal_x;
        for _ in 0..steps {
            let moved = match dir {
                Dir::Right => self.word_step_next(&mut mark),
                Dir::Left => self.word_step_prev(&mut mark),
                Dir::Down => self.word_step_vertical(&mut mark, goal_x, true),
                Dir::Up => self.word_step_vertical(&mut mark, goal_x, false),
            };
            if !moved {
                break; // reached a document edge
            }
        }
        self.word_mark = Some(mark);
        // Horizontal (word) motion sets a new goal column; vertical keeps it.
        if matches!(dir, Dir::Left | Dir::Right) {
            self.update_word_goal_x(mark);
        }
        self.ensure_word_visible();
        self.save_position();
        Effects::redraw()
    }

    fn word_step_next(&mut self, mark: &mut WordMark) -> bool {
        let mut caret = Self::word_mark_caret(*mark);
        if !self.step_next_word_start(&mut caret) {
            return false;
        }
        *mark = self.word_mark_from_caret(caret);
        true
    }

    fn word_step_prev(&mut self, mark: &mut WordMark) -> bool {
        let mut caret = Self::word_mark_caret(*mark);
        if !self.step_prev_word_start(&mut caret) {
            return false;
        }
        *mark = self.word_mark_from_caret(caret);
        true
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

    /// After a scroll or page jump in word focus mode, move the mark to the
    /// first word of the top-most content line now visible.
    fn reposition_word_to_viewport(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let view_top = session.view.scroll().1;
        let goal_x = self.word_goal_x;
        if let Some((page, line)) = self.topmost_visible_line(view_top) {
            let cell = self.nearest_cell(page, line, goal_x);
            let mark = self.word_mark_from_caret(Caret { page, line, cell });
            self.word_mark = Some(mark);
        }
    }

    /// Bounding box (page points) of the marked word's run, unioning its cells.
    fn word_bbox(&mut self, mark: WordMark) -> Option<Rect> {
        let start = self.cell_rect(mark.page, mark.line, mark.start_cell)?;
        let end = self
            .cell_rect(mark.page, mark.line, mark.end_cell)
            .unwrap_or(start);
        Some(Rect {
            x0: start.x0.min(end.x0),
            y0: start.y0.min(end.y0),
            x1: start.x1.max(end.x1),
            y1: start.y1.max(end.y1),
        })
    }

    /// Scroll the minimum amount needed to keep the marked word on screen.
    fn ensure_word_visible(&mut self) {
        let Some(mark) = self.word_mark else {
            return;
        };
        let Some(rect) = self.word_bbox(mark) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(page) = session.view.layout().page(mark.page) else {
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

    /// The marked word's rectangle in canvas pixels, with its page. `None`
    /// outside word focus mode or when no word is marked. The shell paints this.
    pub fn word_screen_rect(&self) -> Option<(usize, ScreenRect)> {
        if self.mode != Mode::WordFocus {
            return None;
        }
        let mark = self.word_mark?;
        let session = self.session.as_ref()?;
        let cells = &session.content.get(&mark.page)?.get(mark.line)?.cells;
        let start = cells.get(mark.start_cell)?.bbox;
        let end = cells.get(mark.end_cell).map_or(start, |c| c.bbox);
        session
            .view
            .page_rect_to_screen(
                mark.page,
                start.x0.min(end.x0),
                start.y0.min(end.y0),
                start.x1.max(end.x1),
                start.y1.max(end.y1),
            )
            .map(|rect| (mark.page, rect))
    }

    // ---- Shared focus-entry helper -------------------------------------

    /// The caret a focus mode should start from: the top-most content line in
    /// the viewport, falling back to the first content line of the document.
    /// Shared by sentence and paragraph entry (mirrors the logic in
    /// [`Self::enter_word_focus`]).
    fn focus_entry_caret(&mut self) -> Option<Caret> {
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

    // ---- Sentence focus navigation -------------------------------------

    pub fn sentence_mark(&self) -> Option<SentenceMark> {
        self.sentence_mark
    }

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
            match self.next_cell_same_page(cur) {
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
        while let Some(prev) = self.prev_cell_same_page(cur) {
            if self.sentence_boundary_after(prev) {
                break;
            }
            cur = prev;
        }
        self.skip_whitespace_forward(cur)
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

    fn enter_sentence_focus(&mut self) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        self.mode = Mode::SentenceFocus;
        if self.sentence_mark.is_none() {
            if let Some(from) = self.focus_entry_caret() {
                let mark = self.sentence_mark_from_caret(from);
                self.sentence_mark = Some(mark);
            }
        }
        self.ensure_sentence_visible();
        self.save_position();
        Effects::redraw()
    }

    fn sentence_move(&mut self, forward: bool, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        let Some(mut mark) = self.sentence_mark else {
            return self.enter_sentence_focus();
        };
        let steps = count.unwrap_or(1).max(1);
        for _ in 0..steps {
            let moved = if forward {
                self.sentence_step_next(&mut mark)
            } else {
                self.sentence_step_prev(&mut mark)
            };
            if !moved {
                break; // reached a document edge
            }
        }
        self.sentence_mark = Some(mark);
        self.ensure_sentence_visible();
        self.save_position();
        Effects::redraw()
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

    /// After a scroll or page jump in sentence focus mode, move the mark to the
    /// sentence containing the top-most content line now visible.
    fn reposition_sentence_to_viewport(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let view_top = session.view.scroll().1;
        if let Some((page, line)) = self.topmost_visible_line(view_top) {
            let mark = self.sentence_mark_from_caret(Caret {
                page,
                line,
                cell: 0,
            });
            self.sentence_mark = Some(mark);
        }
    }

    /// Bounding box (page points) spanning the marked sentence, for scrolling.
    fn sentence_bbox(&mut self, mark: SentenceMark) -> Option<Rect> {
        let start = self.cell_rect(mark.page, mark.start_line, mark.start_cell)?;
        let end = self
            .cell_rect(mark.page, mark.end_line, mark.end_cell)
            .unwrap_or(start);
        Some(Rect {
            x0: start.x0.min(end.x0),
            y0: start.y0.min(end.y0),
            x1: start.x1.max(end.x1),
            y1: start.y1.max(end.y1),
        })
    }

    fn ensure_sentence_visible(&mut self) {
        let Some(mark) = self.sentence_mark else {
            return;
        };
        let Some(rect) = self.sentence_bbox(mark) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(page) = session.view.layout().page(mark.page) else {
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

    /// The marked sentence as one screen rectangle per spanned line (a
    /// text-selection shape): the first line runs from the sentence start to the
    /// line end, middle lines are full, the last line runs from the line start to
    /// the sentence end. `None` outside sentence focus mode. The shell paints these.
    pub fn sentence_screen_rects(&self) -> Option<(usize, Vec<ScreenRect>)> {
        if self.mode != Mode::SentenceFocus {
            return None;
        }
        let mark = self.sentence_mark?;
        let session = self.session.as_ref()?;
        let lines = session.content.get(&mark.page)?;
        let mut rects = Vec::new();
        for line_idx in mark.start_line..=mark.end_line {
            let Some(line) = lines.get(line_idx) else {
                continue;
            };
            if line.cells.is_empty() {
                continue;
            }
            let (x0, x1) = if mark.start_line == mark.end_line {
                let s = line.cells.get(mark.start_cell)?.bbox;
                let e = line.cells.get(mark.end_cell).map_or(s, |c| c.bbox);
                (s.x0.min(e.x0), s.x1.max(e.x1))
            } else if line_idx == mark.start_line {
                let s = line.cells.get(mark.start_cell)?.bbox;
                (s.x0, line.bbox.x1)
            } else if line_idx == mark.end_line {
                let e = line.cells.get(mark.end_cell)?.bbox;
                (line.bbox.x0, e.x1)
            } else {
                (line.bbox.x0, line.bbox.x1)
            };
            if let Some(rect) =
                session
                    .view
                    .page_rect_to_screen(mark.page, x0, line.bbox.y0, x1, line.bbox.y1)
            {
                rects.push(rect);
            }
        }
        if rects.is_empty() {
            None
        } else {
            Some((mark.page, rects))
        }
    }

    // ---- Paragraph focus navigation ------------------------------------

    pub fn paragraph_mark(&self) -> Option<ParagraphMark> {
        self.paragraph_mark
    }

    /// The paragraph (segment of lines) that contains `line` on `page`.
    fn paragraph_mark_containing(&mut self, page: usize, line: usize) -> Option<ParagraphMark> {
        self.ensure_content(page);
        let segs = paragraph_segments(self.content(page));
        segs.iter()
            .find(|(s, e)| *s <= line && line <= *e)
            .or_else(|| segs.last())
            .map(|&(s, e)| ParagraphMark {
                page,
                start_line: s,
                end_line: e,
            })
    }

    fn enter_paragraph_focus(&mut self) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        self.mode = Mode::ParagraphFocus;
        if self.paragraph_mark.is_none() {
            if let Some(from) = self.focus_entry_caret() {
                self.paragraph_mark = self.paragraph_mark_containing(from.page, from.line);
            }
        }
        self.ensure_paragraph_visible();
        self.save_position();
        Effects::redraw()
    }

    fn paragraph_move(&mut self, forward: bool, count: Option<u32>) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        let Some(mut mark) = self.paragraph_mark else {
            return self.enter_paragraph_focus();
        };
        let steps = count.unwrap_or(1).max(1);
        for _ in 0..steps {
            let moved = if forward {
                self.paragraph_step_next(&mut mark)
            } else {
                self.paragraph_step_prev(&mut mark)
            };
            if !moved {
                break; // reached a document edge
            }
        }
        self.paragraph_mark = Some(mark);
        self.ensure_paragraph_visible();
        self.save_position();
        Effects::redraw()
    }

    fn paragraph_step_next(&mut self, mark: &mut ParagraphMark) -> bool {
        self.ensure_content(mark.page);
        let segs = paragraph_segments(self.content(mark.page));
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
            if let Some(&(s, e)) = paragraph_segments(self.content(page)).first() {
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
        let segs = paragraph_segments(self.content(mark.page));
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
            if let Some(&(s, e)) = paragraph_segments(self.content(page)).last() {
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

    /// After a scroll or page jump in paragraph focus mode, move the mark to the
    /// paragraph containing the top-most content line now visible.
    fn reposition_paragraph_to_viewport(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let view_top = session.view.scroll().1;
        if let Some((page, line)) = self.topmost_visible_line(view_top) {
            self.paragraph_mark = self.paragraph_mark_containing(page, line);
        }
    }

    /// Bounding box (page points) of the marked paragraph's lines.
    fn paragraph_bbox(&mut self, mark: ParagraphMark) -> Option<Rect> {
        let start = self.line_bbox(mark.page, mark.start_line)?;
        let end = self.line_bbox(mark.page, mark.end_line).unwrap_or(start);
        Some(Rect {
            x0: start.x0.min(end.x0),
            y0: start.y0.min(end.y0),
            x1: start.x1.max(end.x1),
            y1: start.y1.max(end.y1),
        })
    }

    fn ensure_paragraph_visible(&mut self) {
        let Some(mark) = self.paragraph_mark else {
            return;
        };
        let Some(rect) = self.paragraph_bbox(mark) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(page) = session.view.layout().page(mark.page) else {
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

    /// The marked paragraph's bounding rectangle in canvas pixels, with its page.
    /// `None` outside paragraph focus mode. The shell paints this overlay.
    pub fn paragraph_screen_rect(&self) -> Option<(usize, ScreenRect)> {
        if self.mode != Mode::ParagraphFocus {
            return None;
        }
        let mark = self.paragraph_mark?;
        let session = self.session.as_ref()?;
        let lines = session.content.get(&mark.page)?;
        let start = lines.get(mark.start_line)?.bbox;
        let end = lines.get(mark.end_line).map_or(start, |l| l.bbox);
        session
            .view
            .page_rect_to_screen(
                mark.page,
                start.x0.min(end.x0),
                start.y0.min(end.y0),
                start.x1.max(end.x1),
                start.y1.max(end.y1),
            )
            .map(|rect| (mark.page, rect))
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
            None => out.push_str("no document - press 'o' to open a PDF"),
        }
        if self.mode == Mode::CaretFocus {
            out.push_str("  -- CARET FOCUS --");
            if let Some(caret) = self.caret {
                out.push_str(&format!("  Ln {}, Col {}", caret.line + 1, caret.cell + 1));
            }
        }
        if self.mode == Mode::LineFocus {
            out.push_str("  -- LINE FOCUS --");
            if let Some(mark) = self.line_mark {
                out.push_str(&format!("  Ln {}", mark.line + 1));
            }
        }
        if self.mode == Mode::WordFocus {
            out.push_str("  -- WORD FOCUS --");
            if let Some(mark) = self.word_mark {
                out.push_str(&format!(
                    "  Ln {}, Col {}",
                    mark.line + 1,
                    mark.start_cell + 1
                ));
            }
        }
        if self.mode == Mode::SentenceFocus {
            out.push_str("  -- SENTENCE FOCUS --");
            if let Some(mark) = self.sentence_mark {
                out.push_str(&format!("  Ln {}", mark.start_line + 1));
            }
        }
        if self.mode == Mode::ParagraphFocus {
            out.push_str("  -- PARAGRAPH FOCUS --");
            if let Some(mark) = self.paragraph_mark {
                out.push_str(&format!(
                    "  ¶ {}-{}",
                    mark.start_line + 1,
                    mark.end_line + 1
                ));
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
        let effects = press(&mut app, "o");
        assert!(effects.open_file_dialog);
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
        assert_eq!(app.mode(), Mode::CaretFocus);
        let caret = app.caret().expect("caret placed");
        assert_eq!((caret.page, caret.line, caret.cell), (0, 0, 0));
        assert!(app.caret_screen_rect().is_some());
        assert!(app.status_text().contains("-- CARET FOCUS --"));
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
        assert_eq!(app.mode(), Mode::CaretFocus);
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.caret_screen_rect().is_none());
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
            &app.session.as_ref().unwrap().content[&image.page][image.line].cells[image.cell];
        assert_eq!(cell.kind, CellKind::Image);
        press(&mut app, "b");
        assert_caret(&app, 0, 0, 0);
    }

    #[test]
    fn caret_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "cc");
        assert!(app.caret_screen_rect().is_none());
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
        // A single `c` is only the first half of `cl`: still pending.
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Normal);
        press(&mut app, "l");
        assert_eq!(app.mode(), Mode::LineFocus);
        let mark = app.line_mark().expect("line marked");
        assert_eq!((mark.page, mark.line), (0, 0));
        assert!(app.line_screen_rect().is_some());
        assert!(app.status_text().contains("-- LINE FOCUS --"));
        assert!(app.status_text().contains("Ln 1"));
    }

    #[test]
    fn line_vertical_crosses_pages() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "cl");
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
        press(&mut app, "cl");
        assert_eq!(app.mode(), Mode::LineFocus);
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.line_screen_rect().is_none());
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        let after = app.session.as_ref().unwrap().view.scroll().1;
        assert!(after > before);
    }

    #[test]
    fn line_focus_keeps_non_hjkl_bindings_and_carries_mark() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);
        press(&mut app, "cl");
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
        press(&mut app, "cl");
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
        press(&mut app, "cl");
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
        press(&mut app, "cl");
        assert!(app.line_screen_rect().is_none());
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
        assert_eq!(app.mode(), Mode::WordFocus);
        let mark = app.word_mark().expect("word marked");
        // The first word "alpha" is the run cells 0..=4.
        assert_eq!(
            (mark.page, mark.line, mark.start_cell, mark.end_cell),
            (0, 0, 0, 4)
        );
        assert!(app.word_screen_rect().is_some());
        assert!(app.status_text().contains("-- WORD FOCUS --"));
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
        assert_eq!(app.mode(), Mode::WordFocus);
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.word_screen_rect().is_none());
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

    #[test]
    fn word_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "cw");
        assert!(app.word_screen_rect().is_none());
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
        assert_eq!(app.mode(), Mode::SentenceFocus);
        let mark = app.sentence_mark().expect("sentence marked");
        // The first sentence "Alpha beta." is cells 0..=10 on line 0.
        assert_eq!(
            (mark.page, mark.start_line, mark.start_cell, mark.end_line),
            (0, 0, 0, 0)
        );
        assert!(app.sentence_screen_rects().is_some());
        assert!(app.status_text().contains("-- SENTENCE FOCUS --"));
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
        let (_, rects) = app.sentence_screen_rects().unwrap();
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
        assert_eq!(app.mode(), Mode::SentenceFocus);
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.sentence_screen_rects().is_none());
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        let after = app.session.as_ref().unwrap().view.scroll().1;
        assert!(after > before);
    }

    #[test]
    fn sentence_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "cs");
        assert!(app.sentence_screen_rects().is_none());
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
        assert_eq!(app.mode(), Mode::ParagraphFocus);
        let mark = app.paragraph_mark().expect("paragraph marked");
        assert_eq!((mark.page, mark.start_line), (0, 0));
        assert!(app.paragraph_screen_rect().is_some());
        assert!(app.status_text().contains("-- PARAGRAPH FOCUS --"));
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
        assert_eq!(app.mode(), Mode::ParagraphFocus);
        press(&mut app, "<Esc>");
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.paragraph_screen_rect().is_none());
        let before = app.session.as_ref().unwrap().view.scroll().1;
        press(&mut app, "j");
        let after = app.session.as_ref().unwrap().view.scroll().1;
        assert!(after > before);
    }

    #[test]
    fn paragraph_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "cp");
        assert!(app.paragraph_screen_rect().is_none());
        press(&mut app, "j");
        assert!(app.paragraph_mark().is_none());
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
