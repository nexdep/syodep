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
}

/// A saved reading position for a document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub zoom: f32,
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
}
