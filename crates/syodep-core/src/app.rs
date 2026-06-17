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
use syodep_pdf::{Bitmap, ContentLine, Rect};
use syodep_storage::{Position, Storage};

use crate::caret::{nearest_cell_in_line, Caret, Dir, Mode};
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
    /// Keymap used while in caret mode: the normal keymap plus the
    /// `[caret_keys]` overrides (so `hjkl`/`<Esc>` change meaning there).
    caret_keymap: Keymap,
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
        // The caret keymap is the normal keymap with the caret-mode overrides
        // applied, so every normal binding still works in caret mode and only
        // the overridden keys (hjkl/<Esc>) change meaning. Cloning then
        // overlaying avoids re-validating (and double-reporting) normal keys.
        let mut caret_keymap = keymap.clone();
        let caret_entries = config
            .caret_keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()));
        keymap_errors.extend(caret_keymap.overlay(caret_entries));
        let startup_warnings = keymap_errors.iter().map(KeymapError::to_string).collect();
        Self {
            config,
            keymap,
            caret_keymap,
            input: InputState::new(),
            storage,
            session: None,
            viewport: (800.0, 600.0),
            mode: Mode::Normal,
            caret: None,
            caret_goal_x: 0.0,
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
            Mode::Caret => &self.caret_keymap,
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
            Command::CaretEnter => return self.enter_caret_mode(),
            Command::CaretExit => {
                self.mode = Mode::Normal;
                return Effects::redraw();
            }
            Command::CaretLeft => return self.caret_move(Dir::Left, count),
            Command::CaretRight => return self.caret_move(Dir::Right, count),
            Command::CaretUp => return self.caret_move(Dir::Up, count),
            Command::CaretDown => return self.caret_move(Dir::Down, count),
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
            | Command::CaretEnter
            | Command::CaretExit
            | Command::CaretLeft
            | Command::CaretRight
            | Command::CaretUp
            | Command::CaretDown => unreachable!("handled above"),
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

    fn enter_caret_mode(&mut self) -> Effects {
        if self.session.is_none() {
            return Effects::default();
        }
        self.mode = Mode::Caret;
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
            return self.enter_caret_mode();
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
    /// caret mode or when no caret is placed. The shell paints this overlay.
    pub fn caret_screen_rect(&self) -> Option<(usize, ScreenRect)> {
        if self.mode != Mode::Caret {
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
        if self.mode == Mode::Caret {
            out.push_str("  -- CARET --");
            if let Some(caret) = self.caret {
                out.push_str(&format!("  Ln {}, Col {}", caret.line + 1, caret.cell + 1));
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
    use syodep_pdf::test_support::pdf_with_pages;

    fn write_test_pdf(dir: &Path, pages: usize) -> PathBuf {
        let texts: Vec<String> = (1..=pages).map(|i| format!("Page {i} text")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let path = dir.join("doc.pdf");
        std::fs::write(&path, pdf_with_pages(&refs)).unwrap();
        path
    }

    fn app_with_doc(dir: &Path, pages: usize) -> App {
        let mut app = App::new(Config::default(), Some(Storage::in_memory().unwrap()));
        app.set_viewport_size(595.0, 600.0);
        app.open_document(&write_test_pdf(dir, pages)).unwrap();
        app
    }

    fn press(app: &mut App, sequence: &str) -> Effects {
        let mut effects = Effects::default();
        for chord in parse_sequence(sequence).unwrap() {
            effects = app.handle_key(chord);
        }
        effects
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
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Caret);
        let caret = app.caret().expect("caret placed");
        assert_eq!((caret.page, caret.line, caret.cell), (0, 0, 0));
        assert!(app.caret_screen_rect().is_some());
        assert!(app.status_text().contains("-- CARET --"));
        assert!(app.status_text().contains("Ln 1, Col 1"));
    }

    #[test]
    fn caret_right_advances_chars_then_wraps_to_next_page() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "c");
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
        press(&mut app, "c");
        press(&mut app, "h");
        let caret = app.caret().unwrap();
        assert_eq!((caret.page, caret.line, caret.cell), (0, 0, 0));
    }

    #[test]
    fn caret_vertical_crosses_pages_keeping_column() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 3);
        press(&mut app, "c");
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
        press(&mut app, "c");
        assert_eq!(app.mode(), Mode::Caret);
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
    fn caret_mode_keeps_non_hjkl_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_doc(dir.path(), 5);
        press(&mut app, "c");
        // `G` still navigates pages while in caret mode.
        press(&mut app, "G");
        assert_eq!(app.current_page(), 4);
    }

    #[test]
    fn caret_without_document_does_not_crash() {
        let mut app = App::new(Config::default(), None);
        press(&mut app, "c");
        assert!(app.caret_screen_rect().is_none());
        press(&mut app, "l");
        assert!(app.caret().is_none());
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
