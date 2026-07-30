//! Versioned schema migrations.
//!
//! Rules:
//! - `MIGRATIONS[i]` upgrades the schema from version `i` to `i + 1`.
//! - Published entries are immutable; schema changes append new entries.
//! - `PRAGMA user_version` records the number of applied migrations.

use rusqlite::Connection;

use crate::StorageError;

pub const MIGRATIONS: &[&str] = &[
    // v1: initial schema. `documents` is keyed by content fingerprint;
    // `positions` stores the last reading position per document. Tables for
    // marks, bookmarks, highlights and notes arrive in later migrations
    // (phase 2), keyed by document id.
    "
    CREATE TABLE documents (
        id              INTEGER PRIMARY KEY,
        fingerprint     TEXT NOT NULL UNIQUE,
        path            TEXT NOT NULL,
        created_at      TEXT NOT NULL DEFAULT (datetime('now')),
        last_opened_at  TEXT
    );

    CREATE TABLE positions (
        document_id  INTEGER PRIMARY KEY
                     REFERENCES documents(id) ON DELETE CASCADE,
        scroll_x     REAL NOT NULL,
        scroll_y     REAL NOT NULL,
        zoom         REAL NOT NULL,
        updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
    );
    ",
    // v2: highlights. Geometry lives in a child table because one highlight
    // covers a rectangle per line and may run across several pages, and because
    // that is the form every consumer needs: the overlay renderer, the PDF's
    // per-page `/QuadPoints`, and later the exporters. `text` is the covered
    // characters, kept so notes and export do not have to re-resolve a span
    // against a document that may have been re-saved since.
    "
    CREATE TABLE highlights (
        id           INTEGER PRIMARY KEY,
        document_id  INTEGER NOT NULL
                     REFERENCES documents(id) ON DELETE CASCADE,
        color        TEXT NOT NULL,
        text         TEXT NOT NULL,
        created_at   TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE INDEX highlights_document ON highlights(document_id);

    CREATE TABLE highlight_rects (
        highlight_id INTEGER NOT NULL
                     REFERENCES highlights(id) ON DELETE CASCADE,
        ordinal      INTEGER NOT NULL,
        page         INTEGER NOT NULL,
        x0           REAL NOT NULL,
        y0           REAL NOT NULL,
        x1           REAL NOT NULL,
        y1           REAL NOT NULL,
        PRIMARY KEY (highlight_id, ordinal)
    );
    ",
];

/// Apply all pending migrations inside transactions.
pub fn run(conn: &Connection) -> Result<(), StorageError> {
    let supported = MIGRATIONS.len() as u32;
    let mut version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > supported {
        return Err(StorageError::SchemaTooNew {
            found: version,
            supported,
        });
    }
    while (version as usize) < MIGRATIONS.len() {
        let sql = MIGRATIONS[version as usize];
        conn.execute_batch(&format!(
            "BEGIN;\n{sql}\nPRAGMA user_version = {};\nCOMMIT;",
            version + 1
        ))?;
        version += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_from_scratch() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as u32);
        // Tables exist.
        for table in ["documents", "positions", "highlights", "highlight_rects"] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    (table,),
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing table {table}");
        }
    }

    #[test]
    fn run_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        run(&conn).unwrap();
    }

    #[test]
    fn a_v1_database_upgrades_in_place() {
        // The upgrade path real users take: apply only v1, then everything.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\nPRAGMA user_version = 1;\nCOMMIT;",
            MIGRATIONS[0]
        ))
        .unwrap();
        conn.execute(
            "INSERT INTO documents (fingerprint, path) VALUES ('abc', '/x.pdf')",
            [],
        )
        .unwrap();

        run(&conn).unwrap();

        assert_eq!(
            conn.query_row::<u32, _, _>("PRAGMA user_version", [], |row| row.get(0))
                .unwrap(),
            MIGRATIONS.len() as u32
        );
        // The pre-existing row survived the upgrade.
        let path: String = conn
            .query_row(
                "SELECT path FROM documents WHERE fingerprint = 'abc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(path, "/x.pdf");
    }
}
