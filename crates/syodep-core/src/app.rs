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

use std::path::{Path, PathBuf};

use syodep_config::keys::Chord;
use syodep_config::Config;
use syodep_pdf::Bitmap;
use syodep_storage::{Position, Storage};

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
}

/// Top-level application state. One instance per window.
pub struct App {
    config: Config,
    keymap: Keymap,
    input: InputState,
    storage: Option<Storage>,
    session: Option<Session>,
    viewport: (f32, f32),
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
        let (keymap, keymap_errors) = Keymap::from_entries(entries);
        let startup_warnings = keymap_errors.iter().map(KeymapError::to_string).collect();
        Self {
            config,
            keymap,
            input: InputState::new(),
            storage,
            session: None,
            viewport: (800.0, 600.0),
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
        });
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
        match self.input.handle(&self.keymap, chord) {
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
            Command::Quit | Command::OpenFile | Command::Cancel => unreachable!("handled above"),
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
