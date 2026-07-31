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
    // v3: highlight PDF lifecycle. Rows used to be deleted after a successful
    // save; they are kept now so the sidebar, export, and later comments can
    // still address them by stable id. `pdf_state` is 0=Pending (syodep
    // overlay, included in the next save), 1=Embedded (MuPDF draws it from
    // the PDF; not re-written), 2=External (reserved for annotations found in
    // the PDF that syodep did not create). Existing rows become Pending: under
    // the previous lifecycle successfully embedded rows were already gone, so
    // anything still here has not been removed after a save.
    "
    ALTER TABLE highlights
    ADD COLUMN pdf_state INTEGER NOT NULL DEFAULT 0
    CHECK (pdf_state IN (0, 1, 2));

    ALTER TABLE highlights
    ADD COLUMN updated_at TEXT;

    CREATE INDEX highlights_document_state
    ON highlights(document_id, pdf_state);
    ",
    // v4: one optional Markdown comment per highlight. Separate from
    // `highlights` so the immutable PDF source anchor stays distinct from
    // user-authored notes, and so comment timestamps/metadata can evolve
    // without widening the core annotation row. Chat/threads are a different
    // feature and must not reuse this table.
    "
    CREATE TABLE highlight_notes (
        highlight_id INTEGER PRIMARY KEY
                     REFERENCES highlights(id) ON DELETE CASCADE,
        body_markdown TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
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
        for table in [
            "documents",
            "positions",
            "highlights",
            "highlight_rects",
            "highlight_notes",
        ] {
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

    #[test]
    fn a_v2_database_upgrades_without_losing_highlights() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\nPRAGMA user_version = 2;\nCOMMIT;",
            MIGRATIONS[0], MIGRATIONS[1]
        ))
        .unwrap();
        conn.execute(
            "INSERT INTO documents (fingerprint, path) VALUES ('abc', '/x.pdf')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO highlights (document_id, color, text) VALUES (1, '#ffe066', 'kept')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO highlight_rects (highlight_id, ordinal, page, x0, y0, x1, y1)
             VALUES (1, 0, 0, 10.0, 20.0, 30.0, 40.0)",
            [],
        )
        .unwrap();

        run(&conn).unwrap();

        assert_eq!(
            conn.query_row::<u32, _, _>("PRAGMA user_version", [], |row| row.get(0))
                .unwrap(),
            MIGRATIONS.len() as u32
        );
        let (text, state): (String, i64) = conn
            .query_row(
                "SELECT text, pdf_state FROM highlights WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(text, "kept");
        assert_eq!(state, 0, "existing rows become Pending");
        let rect_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM highlight_rects WHERE highlight_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rect_count, 1);
        // Idempotent: rerunning does not disturb the upgraded rows.
        run(&conn).unwrap();
        let state_again: i64 = conn
            .query_row("SELECT pdf_state FROM highlights WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(state_again, 0);
    }

    #[test]
    fn migrations_apply_from_scratch_include_pdf_state() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        let has_column: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('highlights')
                 WHERE name = 'pdf_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_column, 1);
    }

    #[test]
    fn a_v3_database_upgrades_with_notes_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\n{}\n{}\nPRAGMA user_version = 3;\nCOMMIT;",
            MIGRATIONS[0], MIGRATIONS[1], MIGRATIONS[2]
        ))
        .unwrap();
        conn.execute(
            "INSERT INTO documents (fingerprint, path) VALUES ('abc', '/x.pdf')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO highlights (document_id, color, text, pdf_state)
             VALUES (1, '#ffe066', 'kept', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO highlight_rects (highlight_id, ordinal, page, x0, y0, x1, y1)
             VALUES (1, 0, 0, 10.0, 20.0, 30.0, 40.0)",
            [],
        )
        .unwrap();

        run(&conn).unwrap();

        assert_eq!(
            conn.query_row::<u32, _, _>("PRAGMA user_version", [], |row| row.get(0))
                .unwrap(),
            MIGRATIONS.len() as u32
        );
        let notes_table: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'highlight_notes'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(notes_table, 1);
        let (text, state): (String, i64) = conn
            .query_row(
                "SELECT text, pdf_state FROM highlights WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(text, "kept");
        assert_eq!(state, 0);
        let rect_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM highlight_rects WHERE highlight_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rect_count, 1);
        run(&conn).unwrap();
    }
}
