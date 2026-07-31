//! SQLite persistence for syodep.
//!
//! Design decisions (see `docs/architecture.md`):
//!
//! - All dynamic user state (positions, and later marks/bookmarks/highlights/
//!   notes) lives in SQLite, never in TOML.
//! - Documents are identified by a SHA-256 content fingerprint, not by path,
//!   so state survives moves/renames of the file.
//! - Migrations are versioned through `PRAGMA user_version` and run
//!   unconditionally at open. Schema changes append a new entry to
//!   [`MIGRATIONS`]; existing entries are immutable.

mod migrations;

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub use migrations::MIGRATIONS;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cannot fingerprint {path}: {source}")]
    Fingerprint {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "database schema version {found} is newer than this build supports ({supported}); \
         refusing to open"
    )]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("invalid highlight pdf_state value {0}")]
    InvalidHighlightState(i64),
    #[error("highlight {0} was not pending for document {1}")]
    HighlightNotPending(i64, i64),
}

/// A saved reading position for a document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub zoom: f32,
}

/// Stable identity of a persisted highlight row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HighlightId(pub i64);

/// Whether a highlight has been written into the PDF yet.
///
/// Integer values are stored in `highlights.pdf_state` and must stay stable:
/// `0` Pending, `1` Embedded, `2` External.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HighlightPdfState {
    /// Stored by syodep but not yet successfully embedded. Drawn as a syodep
    /// overlay and included in the next PDF save.
    Pending = 0,
    /// Successfully embedded into the PDF. Kept in SQLite for the sidebar and
    /// export; MuPDF draws it, so the overlay must not.
    Embedded = 1,
    /// Reserved for a future annotation discovered in the PDF that syodep did
    /// not create or cannot confidently match to a local record.
    External = 2,
}

impl HighlightPdfState {
    pub fn from_i64(value: i64) -> Result<Self, StorageError> {
        match value {
            0 => Ok(Self::Pending),
            1 => Ok(Self::Embedded),
            2 => Ok(Self::External),
            other => Err(StorageError::InvalidHighlightState(other)),
        }
    }

    pub fn as_i64(self) -> i64 {
        self as i64
    }
}

/// One rectangle of a highlight, in page points with the origin at the top left
/// (the space the content layer reports).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighlightRect {
    pub page: usize,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// A stored highlight: its geometry, covered text, and PDF lifecycle state.
///
/// Geometry rather than a document position, because rectangles are what all
/// three consumers need — the overlay renderer, the PDF writer's per-page
/// `/QuadPoints`, and later the exporters — and because they stay valid without
/// re-extracting the page's content layer on reload.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredHighlight {
    pub id: HighlightId,
    /// `#rrggbb`.
    pub color: String,
    pub text: String,
    /// In document order, one per covered line.
    pub rects: Vec<HighlightRect>,
    pub pdf_state: HighlightPdfState,
}

/// Optional user-authored Markdown comment for one highlight.
///
/// Distinct from [`StoredHighlight::text`], which is the immutable PDF source
/// quote. Absence of a row means no comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredHighlightNote {
    pub highlight_id: HighlightId,
    pub body_markdown: String,
}

/// Handle to the syodep database.
#[derive(Debug)]
pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// Open (creating and migrating if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            // Best effort; SQLite will report a usable error if this failed.
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// In-memory database, for tests.
    pub fn in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, StorageError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::run(&conn)?;
        Ok(Self { conn })
    }

    /// Current schema version, mainly for tests and diagnostics.
    pub fn schema_version(&self) -> Result<u32, StorageError> {
        let v: u32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        Ok(v)
    }

    /// Content fingerprint used to identify documents independently of path.
    pub fn fingerprint_file(path: &Path) -> Result<String, StorageError> {
        let map_err = |source| StorageError::Fingerprint {
            path: path.display().to_string(),
            source,
        };
        let mut file = std::fs::File::open(path).map_err(map_err)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(map_err)?;
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Insert or refresh a document row; returns its id.
    ///
    /// The path is updated on every open so the most recent location wins,
    /// while all per-document state keys off the stable fingerprint.
    pub fn upsert_document(&self, fingerprint: &str, path: &str) -> Result<i64, StorageError> {
        self.conn.execute(
            "INSERT INTO documents (fingerprint, path, last_opened_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT (fingerprint)
             DO UPDATE SET path = excluded.path, last_opened_at = excluded.last_opened_at",
            (fingerprint, path),
        )?;
        let id = self.conn.query_row(
            "SELECT id FROM documents WHERE fingerprint = ?1",
            (fingerprint,),
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn save_position(&self, document_id: i64, position: Position) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO positions (document_id, scroll_x, scroll_y, zoom, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT (document_id)
             DO UPDATE SET scroll_x = excluded.scroll_x,
                           scroll_y = excluded.scroll_y,
                           zoom = excluded.zoom,
                           updated_at = excluded.updated_at",
            (
                document_id,
                position.scroll_x as f64,
                position.scroll_y as f64,
                position.zoom as f64,
            ),
        )?;
        Ok(())
    }

    pub fn load_position(&self, document_id: i64) -> Result<Option<Position>, StorageError> {
        let position = self
            .conn
            .query_row(
                "SELECT scroll_x, scroll_y, zoom FROM positions WHERE document_id = ?1",
                (document_id,),
                |row| {
                    Ok(Position {
                        scroll_x: row.get::<_, f64>(0)? as f32,
                        scroll_y: row.get::<_, f64>(1)? as f32,
                        zoom: row.get::<_, f64>(2)? as f32,
                    })
                },
            )
            .optional()?;
        Ok(position)
    }

    /// Point an existing document row at a new content fingerprint.
    ///
    /// Needed because saving rewrites the PDF, which changes the SHA-256 the
    /// document is keyed by. Moving the row rather than inserting a new one is
    /// what carries the reading position across the save — otherwise every save
    /// would silently orphan it.
    pub fn rekey_document(
        &self,
        document_id: i64,
        fingerprint: &str,
        path: &str,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "UPDATE documents SET fingerprint = ?2, path = ?3 WHERE id = ?1",
            (document_id, fingerprint, path),
        )?;
        Ok(())
    }

    /// Store a highlight and its rectangles as [`HighlightPdfState::Pending`],
    /// returning the new row id.
    pub fn insert_highlight(
        &self,
        document_id: i64,
        color: &str,
        text: &str,
        rects: &[HighlightRect],
    ) -> Result<HighlightId, StorageError> {
        // One transaction: a highlight with no rectangles would be invisible and
        // unreachable, so the two inserts must not be separable.
        self.conn.execute("BEGIN", [])?;
        let result = self.insert_highlight_inner(document_id, color, text, rects);
        match &result {
            Ok(_) => self.conn.execute("COMMIT", [])?,
            Err(_) => self.conn.execute("ROLLBACK", [])?,
        };
        result
    }

    fn insert_highlight_inner(
        &self,
        document_id: i64,
        color: &str,
        text: &str,
        rects: &[HighlightRect],
    ) -> Result<HighlightId, StorageError> {
        self.conn.execute(
            "INSERT INTO highlights (document_id, color, text, pdf_state)
             VALUES (?1, ?2, ?3, ?4)",
            (
                document_id,
                color,
                text,
                HighlightPdfState::Pending.as_i64(),
            ),
        )?;
        let id = HighlightId(self.conn.last_insert_rowid());
        let mut statement = self.conn.prepare(
            "INSERT INTO highlight_rects (highlight_id, ordinal, page, x0, y0, x1, y1)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (ordinal, rect) in rects.iter().enumerate() {
            statement.execute((
                id.0,
                ordinal as i64,
                rect.page as i64,
                rect.x0 as f64,
                rect.y0 as f64,
                rect.x1 as f64,
                rect.y1 as f64,
            ))?;
        }
        Ok(id)
    }

    /// Every highlight of a document, oldest first (stable id order), each with
    /// its rectangles in the order they were stored. Includes Pending, Embedded,
    /// and External rows.
    pub fn load_highlights(&self, document_id: i64) -> Result<Vec<StoredHighlight>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT id, color, text, pdf_state FROM highlights
             WHERE document_id = ?1 ORDER BY id",
        )?;
        let rows = statement
            .query_map((document_id,), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut highlights = Vec::with_capacity(rows.len());
        for (id, color, text, state) in rows {
            highlights.push(StoredHighlight {
                id: HighlightId(id),
                color,
                text,
                rects: Vec::new(),
                pdf_state: HighlightPdfState::from_i64(state)?,
            });
        }

        let mut statement = self.conn.prepare(
            "SELECT page, x0, y0, x1, y1 FROM highlight_rects
             WHERE highlight_id = ?1 ORDER BY ordinal",
        )?;
        for highlight in &mut highlights {
            highlight.rects = statement
                .query_map((highlight.id.0,), |row| {
                    Ok(HighlightRect {
                        page: row.get::<_, i64>(0)? as usize,
                        x0: row.get::<_, f64>(1)? as f32,
                        y0: row.get::<_, f64>(2)? as f32,
                        x1: row.get::<_, f64>(3)? as f32,
                        y1: row.get::<_, f64>(4)? as f32,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }
        Ok(highlights)
    }

    /// Mark the exact highlight ids as [`HighlightPdfState::Embedded`] in one
    /// transaction. Only those ids — never every pending row of the document —
    /// so a save that wrote a subset cannot silently claim the rest.
    ///
    /// Each id must currently be Pending and belong to `document_id`; otherwise
    /// the whole transaction rolls back.
    pub fn mark_highlights_embedded(
        &self,
        document_id: i64,
        highlight_ids: &[HighlightId],
    ) -> Result<(), StorageError> {
        if highlight_ids.is_empty() {
            return Ok(());
        }
        self.conn.execute("BEGIN", [])?;
        let result = self.mark_highlights_embedded_inner(document_id, highlight_ids);
        match &result {
            Ok(_) => self.conn.execute("COMMIT", [])?,
            Err(_) => self.conn.execute("ROLLBACK", [])?,
        };
        result
    }

    fn mark_highlights_embedded_inner(
        &self,
        document_id: i64,
        highlight_ids: &[HighlightId],
    ) -> Result<(), StorageError> {
        let mut statement = self.conn.prepare(
            "UPDATE highlights
             SET pdf_state = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND document_id = ?3 AND pdf_state = ?4",
        )?;
        for id in highlight_ids {
            let updated = statement.execute((
                HighlightPdfState::Embedded.as_i64(),
                id.0,
                document_id,
                HighlightPdfState::Pending.as_i64(),
            ))?;
            if updated != 1 {
                return Err(StorageError::HighlightNotPending(id.0, document_id));
            }
        }
        Ok(())
    }

    /// Forget every highlight of a document. Kept for cascade tests and a
    /// future delete command; the save path no longer uses this.
    pub fn delete_highlights(&self, document_id: i64) -> Result<(), StorageError> {
        // `highlight_rects` and `highlight_notes` go with them: the foreign
        // keys cascade, and `foreign_keys` is ON for every connection this
        // type hands out.
        self.conn.execute(
            "DELETE FROM highlights WHERE document_id = ?1",
            (document_id,),
        )?;
        Ok(())
    }

    /// Load every Markdown note belonging to highlights of `document_id`.
    ///
    /// Joined through `highlights` so a note cannot leak from another document.
    pub fn load_highlight_notes(
        &self,
        document_id: i64,
    ) -> Result<HashMap<HighlightId, StoredHighlightNote>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT n.highlight_id, n.body_markdown
             FROM highlight_notes n
             JOIN highlights h ON h.id = n.highlight_id
             WHERE h.document_id = ?1
             ORDER BY h.id",
        )?;
        let rows = statement.query_map((document_id,), |row| {
            Ok(StoredHighlightNote {
                highlight_id: HighlightId(row.get(0)?),
                body_markdown: row.get(1)?,
            })
        })?;
        let mut notes = HashMap::new();
        for note in rows {
            let note = note?;
            notes.insert(note.highlight_id, note);
        }
        Ok(notes)
    }

    /// Upsert or clear a note for a highlight that belongs to `document_id`.
    ///
    /// Whitespace-only bodies (`trim().is_empty()`) delete the row. Returns
    /// `Ok(true)` when the highlight belonged to the document (even if the
    /// body was a no-op), `Ok(false)` when the id is missing or belongs to
    /// another document. Equal non-empty bodies skip the write.
    pub fn set_highlight_note_for_document(
        &self,
        document_id: i64,
        highlight_id: HighlightId,
        body_markdown: &str,
    ) -> Result<bool, StorageError> {
        let owned: Option<i64> = self
            .conn
            .query_row(
                "SELECT document_id FROM highlights WHERE id = ?1",
                (highlight_id.0,),
                |row| row.get(0),
            )
            .optional()?;
        match owned {
            Some(id) if id == document_id => {}
            _ => return Ok(false),
        }

        if body_markdown.trim().is_empty() {
            self.conn.execute(
                "DELETE FROM highlight_notes WHERE highlight_id = ?1",
                (highlight_id.0,),
            )?;
            return Ok(true);
        }

        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT body_markdown FROM highlight_notes WHERE highlight_id = ?1",
                (highlight_id.0,),
                |row| row.get(0),
            )
            .optional()?;
        if existing.as_deref() == Some(body_markdown) {
            return Ok(true);
        }

        self.conn.execute(
            "INSERT INTO highlight_notes (
                highlight_id, body_markdown, created_at, updated_at
             )
             VALUES (?1, ?2, datetime('now'), datetime('now'))
             ON CONFLICT(highlight_id)
             DO UPDATE SET
                body_markdown = excluded.body_markdown,
                updated_at = datetime('now')",
            (highlight_id.0, body_markdown),
        )?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_migrates_in_memory() {
        let storage = Storage::in_memory().unwrap();
        assert_eq!(storage.schema_version().unwrap(), MIGRATIONS.len() as u32);
    }

    #[test]
    fn opens_and_migrates_on_disk_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("syodep.sqlite3");
        {
            let storage = Storage::open(&path).unwrap();
            storage.upsert_document("abc", "/tmp/a.pdf").unwrap();
        }
        // Re-opening runs migrations again without error or data loss.
        let storage = Storage::open(&path).unwrap();
        let id = storage.upsert_document("abc", "/tmp/a.pdf").unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn upsert_is_stable_and_updates_path() {
        let storage = Storage::in_memory().unwrap();
        let id1 = storage.upsert_document("fp1", "/old/path.pdf").unwrap();
        let id2 = storage.upsert_document("fp1", "/new/path.pdf").unwrap();
        assert_eq!(id1, id2);
        let path: String = storage
            .conn
            .query_row("SELECT path FROM documents WHERE id = ?1", (id1,), |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(path, "/new/path.pdf");
        let other = storage.upsert_document("fp2", "/other.pdf").unwrap();
        assert_ne!(other, id1);
    }

    #[test]
    fn position_round_trips() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        assert_eq!(storage.load_position(id).unwrap(), None);
        let position = Position {
            scroll_x: 1.5,
            scroll_y: 1234.25,
            zoom: 1.75,
        };
        storage.save_position(id, position).unwrap();
        assert_eq!(storage.load_position(id).unwrap(), Some(position));
        // Overwrite.
        let moved = Position {
            scroll_y: 99.0,
            ..position
        };
        storage.save_position(id, moved).unwrap();
        assert_eq!(storage.load_position(id).unwrap(), Some(moved));
    }

    #[test]
    fn deleting_document_cascades_to_position() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        storage
            .save_position(
                id,
                Position {
                    scroll_x: 0.0,
                    scroll_y: 1.0,
                    zoom: 1.0,
                },
            )
            .unwrap();
        storage
            .conn
            .execute("DELETE FROM documents WHERE id = ?1", (id,))
            .unwrap();
        assert_eq!(storage.load_position(id).unwrap(), None);
    }

    #[test]
    fn fingerprint_is_content_based() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        std::fs::write(&a, b"same content").unwrap();
        std::fs::write(&b, b"same content").unwrap();
        let fa = Storage::fingerprint_file(&a).unwrap();
        let fb = Storage::fingerprint_file(&b).unwrap();
        assert_eq!(fa, fb);
        std::fs::write(&b, b"different").unwrap();
        assert_ne!(fa, Storage::fingerprint_file(&b).unwrap());
        // 64 hex chars of SHA-256.
        assert_eq!(fa.len(), 64);
    }

    #[test]
    fn refuses_databases_from_the_future() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 9999).unwrap();
        let err = Storage::from_connection(conn).unwrap_err();
        assert!(matches!(err, StorageError::SchemaTooNew { .. }), "{err}");
    }

    fn rect(page: usize, x0: f32) -> HighlightRect {
        HighlightRect {
            page,
            x0,
            y0: 10.0,
            x1: x0 + 40.0,
            y1: 24.0,
        }
    }

    #[test]
    fn a_highlight_spanning_pages_round_trips_in_order() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        assert!(storage.load_highlights(id).unwrap().is_empty());

        let rects = vec![rect(0, 72.0), rect(0, 120.0), rect(1, 72.0)];
        let first = storage
            .insert_highlight(id, "#ffe066", "hello there", &rects)
            .unwrap();
        let second = storage
            .insert_highlight(id, "#88ccff", "second", &[rect(2, 90.0)])
            .unwrap();

        let loaded = storage.load_highlights(id).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded[0],
            StoredHighlight {
                id: first,
                color: "#ffe066".to_owned(),
                text: "hello there".to_owned(),
                rects: rects.clone(),
                pdf_state: HighlightPdfState::Pending,
            }
        );
        assert_eq!(loaded[1].id, second);
        assert_eq!(loaded[1].rects, vec![rect(2, 90.0)]);
        assert_eq!(loaded[1].pdf_state, HighlightPdfState::Pending);
    }

    #[test]
    fn new_highlights_start_pending() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        storage
            .insert_highlight(id, "#ffe066", "hello", &[rect(0, 72.0)])
            .unwrap();
        let loaded = storage.load_highlights(id).unwrap();
        assert_eq!(loaded[0].pdf_state, HighlightPdfState::Pending);
    }

    #[test]
    fn mark_embedded_updates_only_the_named_ids() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        let first = storage
            .insert_highlight(id, "#ffe066", "one", &[rect(0, 72.0)])
            .unwrap();
        let second = storage
            .insert_highlight(id, "#88ccff", "two", &[rect(0, 90.0)])
            .unwrap();
        let third = storage
            .insert_highlight(id, "#aaffaa", "three", &[rect(1, 10.0)])
            .unwrap();

        storage
            .mark_highlights_embedded(id, &[first, third])
            .unwrap();

        let loaded = storage.load_highlights(id).unwrap();
        assert_eq!(loaded[0].pdf_state, HighlightPdfState::Embedded);
        assert_eq!(loaded[1].id, second);
        assert_eq!(loaded[1].pdf_state, HighlightPdfState::Pending);
        assert_eq!(loaded[2].pdf_state, HighlightPdfState::Embedded);
    }

    #[test]
    fn embedded_records_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("syodep.sqlite3");
        let highlight_id = {
            let storage = Storage::open(&path).unwrap();
            let id = storage.upsert_document("fp", "/a.pdf").unwrap();
            let hid = storage
                .insert_highlight(id, "#ffe066", "kept", &[rect(0, 72.0)])
                .unwrap();
            storage.mark_highlights_embedded(id, &[hid]).unwrap();
            hid
        };
        let storage = Storage::open(&path).unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        let loaded = storage.load_highlights(id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, highlight_id);
        assert_eq!(loaded[0].pdf_state, HighlightPdfState::Embedded);
        assert_eq!(loaded[0].text, "kept");
        assert_eq!(loaded[0].rects, vec![rect(0, 72.0)]);
    }

    #[test]
    fn invalid_pdf_state_values_are_refused_on_load() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        storage
            .insert_highlight(id, "#ffe066", "bad", &[rect(0, 72.0)])
            .unwrap();
        assert!(matches!(
            HighlightPdfState::from_i64(99),
            Err(StorageError::InvalidHighlightState(99))
        ));
        // Known values round-trip through storage.
        storage
            .conn
            .execute(
                "UPDATE highlights SET pdf_state = 1 WHERE document_id = ?1",
                (id,),
            )
            .unwrap();
        assert_eq!(
            storage.load_highlights(id).unwrap()[0].pdf_state,
            HighlightPdfState::Embedded
        );
        storage
            .conn
            .execute(
                "UPDATE highlights SET pdf_state = 2 WHERE document_id = ?1",
                (id,),
            )
            .unwrap();
        assert_eq!(
            storage.load_highlights(id).unwrap()[0].pdf_state,
            HighlightPdfState::External
        );
    }

    #[test]
    fn mark_embedded_rolls_back_when_one_id_is_not_pending() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        let first = storage
            .insert_highlight(id, "#ffe066", "one", &[rect(0, 72.0)])
            .unwrap();
        let second = storage
            .insert_highlight(id, "#88ccff", "two", &[rect(0, 90.0)])
            .unwrap();

        let err = storage
            .mark_highlights_embedded(id, &[first, HighlightId(999_999)])
            .unwrap_err();
        assert!(matches!(err, StorageError::HighlightNotPending(999_999, _)));
        let loaded = storage.load_highlights(id).unwrap();
        assert_eq!(loaded[0].id, first);
        assert_eq!(loaded[0].pdf_state, HighlightPdfState::Pending);
        assert_eq!(loaded[1].id, second);
        assert_eq!(loaded[1].pdf_state, HighlightPdfState::Pending);
    }

    #[test]
    fn highlights_belong_to_one_document() {
        let storage = Storage::in_memory().unwrap();
        let a = storage.upsert_document("fp-a", "/a.pdf").unwrap();
        let b = storage.upsert_document("fp-b", "/b.pdf").unwrap();
        storage
            .insert_highlight(a, "#ffe066", "in a", &[rect(0, 72.0)])
            .unwrap();
        assert_eq!(storage.load_highlights(a).unwrap().len(), 1);
        assert!(storage.load_highlights(b).unwrap().is_empty());
    }

    #[test]
    fn deleting_highlights_takes_their_rectangles_with_them() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        storage
            .insert_highlight(id, "#ffe066", "gone", &[rect(0, 72.0), rect(0, 120.0)])
            .unwrap();
        storage.delete_highlights(id).unwrap();
        assert!(storage.load_highlights(id).unwrap().is_empty());
        let orphans: i64 = storage
            .conn
            .query_row("SELECT count(*) FROM highlight_rects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn deleting_a_document_cascades_to_its_highlights() {
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("fp", "/a.pdf").unwrap();
        storage
            .insert_highlight(id, "#ffe066", "gone", &[rect(0, 72.0)])
            .unwrap();
        storage
            .conn
            .execute("DELETE FROM documents WHERE id = ?1", (id,))
            .unwrap();
        let orphans: i64 = storage
            .conn
            .query_row("SELECT count(*) FROM highlight_rects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn rekeying_carries_the_position_and_highlights_across() {
        // What saving does: the file is rewritten, so its fingerprint changes.
        // The row must follow, or the reading position is orphaned every time.
        let storage = Storage::in_memory().unwrap();
        let id = storage.upsert_document("old-fp", "/a.pdf").unwrap();
        let position = Position {
            scroll_x: 0.0,
            scroll_y: 500.0,
            zoom: 1.25,
        };
        storage.save_position(id, position).unwrap();
        storage
            .insert_highlight(id, "#ffe066", "kept", &[rect(0, 72.0)])
            .unwrap();

        storage.rekey_document(id, "new-fp", "/a.pdf").unwrap();

        // Opening the rewritten file finds the same row, not a fresh one.
        assert_eq!(storage.upsert_document("new-fp", "/a.pdf").unwrap(), id);
        assert_eq!(storage.load_position(id).unwrap(), Some(position));
        assert_eq!(storage.load_highlights(id).unwrap().len(), 1);
        // The old fingerprint is gone, so it cannot resurrect a stale row.
        let stale: i64 = storage
            .conn
            .query_row(
                "SELECT count(*) FROM documents WHERE fingerprint = 'old-fp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0);
    }

    #[test]
    fn highlight_notes_upsert_clear_and_scope() {
        let storage = Storage::in_memory().unwrap();
        let doc_a = storage.upsert_document("fp-a", "/a.pdf").unwrap();
        let doc_b = storage.upsert_document("fp-b", "/b.pdf").unwrap();
        let hid_a = storage
            .insert_highlight(doc_a, "#ffe066", "alpha", &[rect(0, 72.0)])
            .unwrap();
        let hid_b = storage
            .insert_highlight(doc_b, "#88ccff", "beta", &[rect(0, 90.0)])
            .unwrap();

        assert!(storage
            .set_highlight_note_for_document(doc_a, hid_a, "first note")
            .unwrap());
        assert!(storage
            .set_highlight_note_for_document(doc_a, hid_a, "updated note")
            .unwrap());
        // Equal body is a no-op write.
        assert!(storage
            .set_highlight_note_for_document(doc_a, hid_a, "updated note")
            .unwrap());

        let notes_a = storage.load_highlight_notes(doc_a).unwrap();
        assert_eq!(notes_a.len(), 1);
        assert_eq!(notes_a[&hid_a].body_markdown, "updated note");
        assert!(storage.load_highlight_notes(doc_b).unwrap().is_empty());

        // Cross-document id is rejected.
        assert!(!storage
            .set_highlight_note_for_document(doc_a, hid_b, "nope")
            .unwrap());
        assert!(storage.load_highlight_notes(doc_b).unwrap().is_empty());

        // Whitespace-only clears.
        assert!(storage
            .set_highlight_note_for_document(doc_a, hid_a, "  \n\t  ")
            .unwrap());
        assert!(storage.load_highlight_notes(doc_a).unwrap().is_empty());

        // Unicode / multiline.
        assert!(storage
            .set_highlight_note_for_document(doc_a, hid_a, "# 标题\n\n- one\n- two")
            .unwrap());
        assert_eq!(
            storage.load_highlight_notes(doc_a).unwrap()[&hid_a].body_markdown,
            "# 标题\n\n- one\n- two"
        );
    }

    #[test]
    fn highlight_notes_cascade_on_highlight_delete() {
        let storage = Storage::in_memory().unwrap();
        let doc = storage.upsert_document("fp", "/a.pdf").unwrap();
        let hid = storage
            .insert_highlight(doc, "#ffe066", "gone", &[rect(0, 72.0)])
            .unwrap();
        storage
            .set_highlight_note_for_document(doc, hid, "orphan me")
            .unwrap();
        storage.delete_highlights(doc).unwrap();
        let notes: i64 = storage
            .conn
            .query_row("SELECT count(*) FROM highlight_notes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(notes, 0);
    }

    #[test]
    fn highlight_notes_survive_pending_to_embedded_and_rekey() {
        let storage = Storage::in_memory().unwrap();
        let doc = storage.upsert_document("old-fp", "/a.pdf").unwrap();
        let hid = storage
            .insert_highlight(doc, "#ffe066", "quoted", &[rect(0, 72.0)])
            .unwrap();
        storage
            .set_highlight_note_for_document(doc, hid, "keeps going")
            .unwrap();
        storage.mark_highlights_embedded(doc, &[hid]).unwrap();
        assert_eq!(
            storage.load_highlight_notes(doc).unwrap()[&hid].body_markdown,
            "keeps going"
        );
        storage.rekey_document(doc, "new-fp", "/a.pdf").unwrap();
        assert_eq!(
            storage.load_highlight_notes(doc).unwrap()[&hid].body_markdown,
            "keeps going"
        );
    }

    #[test]
    fn unknown_highlight_note_target_is_rejected() {
        let storage = Storage::in_memory().unwrap();
        let doc = storage.upsert_document("fp", "/a.pdf").unwrap();
        assert!(!storage
            .set_highlight_note_for_document(doc, HighlightId(999), "x")
            .unwrap());
    }
}
