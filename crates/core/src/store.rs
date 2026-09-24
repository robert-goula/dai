//! SQLite metadata and page content (`meta.db`).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS docsets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    version TEXT NOT NULL,
    release TEXT NOT NULL,
    mtime INTEGER NOT NULL,
    installed_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS entries (
    docset TEXT NOT NULL REFERENCES docsets(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    path TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS entries_docset ON entries(docset);
CREATE TABLE IF NOT EXISTS pages (
    docset TEXT NOT NULL REFERENCES docsets(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    html TEXT NOT NULL,
    markdown TEXT NOT NULL,
    PRIMARY KEY (docset, path)
);
";

#[derive(Debug, Clone, Serialize)]
pub struct Docset {
    /// Stable id used everywhere else (the DevDocs slug, e.g. `react`, `python~3.12`).
    pub id: String,
    pub name: String,
    pub source: String,
    pub version: String,
    pub release: String,
    /// Upstream modification time; compared against the catalog to detect updates.
    pub mtime: i64,
    pub installed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    /// Page path, optionally with `#anchor`.
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
}

pub struct Page {
    pub path: String,
    pub html: String,
    pub markdown: String,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn docsets(&self) -> Result<Vec<Docset>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, source, version, release, mtime, installed_at FROM docsets ORDER BY id",
        )?;
        let rows = stmt.query_map([], row_to_docset)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn docset(&self, id: &str) -> Result<Option<Docset>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, source, version, release, mtime, installed_at FROM docsets WHERE id = ?1",
                [id],
                row_to_docset,
            )
            .optional()?)
    }

    /// Replaces a docset and all its entries and pages in one transaction.
    pub fn replace_docset(&mut self, ds: &Docset, entries: &[Entry], pages: &[Page]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM docsets WHERE id = ?1", [&ds.id])?;
        tx.execute(
            "INSERT INTO docsets (id, name, source, version, release, mtime, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                ds.id,
                ds.name,
                ds.source,
                ds.version,
                ds.release,
                ds.mtime,
                ds.installed_at
            ],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO entries (docset, name, type, path) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for e in entries {
                stmt.execute(params![ds.id, e.name, e.kind, e.path])?;
            }
            let mut stmt = tx.prepare(
                "INSERT INTO pages (docset, path, html, markdown) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for p in pages {
                stmt.execute(params![ds.id, p.path, p.html, p.markdown])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_docset(&mut self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM docsets WHERE id = ?1", [id])?
            > 0)
    }

    pub fn page(&self, docset: &str, path: &str) -> Result<Option<Page>> {
        Ok(self
            .conn
            .query_row(
                "SELECT path, html, markdown FROM pages WHERE docset = ?1 AND path = ?2",
                [docset, path],
                |r| {
                    Ok(Page {
                        path: r.get(0)?,
                        html: r.get(1)?,
                        markdown: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }
}

fn row_to_docset(r: &rusqlite::Row) -> rusqlite::Result<Docset> {
    Ok(Docset {
        id: r.get(0)?,
        name: r.get(1)?,
        source: r.get(2)?,
        version: r.get(3)?,
        release: r.get(4)?,
        mtime: r.get(5)?,
        installed_at: r.get(6)?,
    })
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
