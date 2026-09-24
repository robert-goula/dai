//! SQLite metadata and page content (`meta.db`).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rayon::prelude::*;
use rusqlite::types::{Value, ValueRef};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Incremental auto-vacuum lets replaced/removed docsets give their
        // space back. It must be set before any table exists, or be followed
        // by a one-time VACUUM for databases created without it.
        let mode: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        if mode != 2 {
            conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")?;
        }
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn docsets(&self) -> Result<Vec<Docset>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, source, version, release, mtime, installed_at FROM docsets ORDER BY id",
        )?;
        let rows = stmt.query_map([], row_to_docset)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn docset(&self, id: &str) -> Result<Option<Docset>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, name, source, version, release, mtime, installed_at FROM docsets WHERE id = ?1",
                [id],
                row_to_docset,
            )
            .optional()?)
    }

    /// Replaces a docset and all its entries and pages in one transaction.
    /// Page contents are stored zstd-compressed.
    pub fn replace_docset(&self, ds: &Docset, entries: &[Entry], pages: &[Page]) -> Result<()> {
        // Compress up front (in parallel) so the connection isn't held meanwhile.
        let compressed: Vec<(Value, Value)> = pages
            .par_iter()
            .map(|p| Ok((pack(&p.html)?, pack(&p.markdown)?)))
            .collect::<Result<_>>()?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
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
            for (p, (html, markdown)) in pages.iter().zip(compressed) {
                stmt.execute(params![ds.id, p.path, html, markdown])?;
            }
        }
        tx.commit()?;
        reclaim(&conn)?;
        Ok(())
    }

    pub fn remove_docset(&self, id: &str) -> Result<bool> {
        let conn = self.conn();
        let removed = conn.execute("DELETE FROM docsets WHERE id = ?1", [id])? > 0;
        reclaim(&conn)?;
        Ok(removed)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic mid-transaction rolls back, so the connection is still usable.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn page(&self, docset: &str, path: &str) -> Result<Option<Page>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT path, html, markdown FROM pages WHERE docset = ?1 AND path = ?2",
                [docset, path],
                |r| {
                    Ok(Page {
                        path: r.get(0)?,
                        html: unpack(r.get_ref(1)?)?,
                        markdown: unpack(r.get_ref(2)?)?,
                    })
                },
            )
            .optional()?)
    }
}

/// Returns free pages to the filesystem. `incremental_vacuum` frees one batch
/// per step, so it has to be stepped to completion.
fn reclaim(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA incremental_vacuum")?;
    let mut rows = stmt.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

const ZSTD_LEVEL: i32 = 3;

/// Page text as a zstd blob (empty text stays empty).
fn pack(text: &str) -> Result<Value> {
    if text.is_empty() {
        return Ok(Value::Text(String::new()));
    }
    Ok(Value::Blob(zstd::encode_all(text.as_bytes(), ZSTD_LEVEL)?))
}

/// Reads page text stored either compressed (blob) or, in databases written
/// before compression, as plain text.
fn unpack(v: ValueRef) -> rusqlite::Result<String> {
    let err = |e: Box<dyn std::error::Error + Send + Sync>| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, e)
    };
    match v {
        ValueRef::Blob(b) => {
            let bytes = zstd::decode_all(b).map_err(|e| err(e.into()))?;
            String::from_utf8(bytes).map_err(|e| err(e.into()))
        }
        other => Ok(other.as_str()?.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_round_trip_compressed_and_legacy_text() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("meta.db")).unwrap();
        let ds = Docset {
            id: "x".into(),
            name: "X".into(),
            source: "devdocs".into(),
            version: "1".into(),
            release: "1".into(),
            mtime: 0,
            installed_at: 0,
        };
        let body = "# Title\n\n".to_string() + &"repetitive text ".repeat(500);
        let page = Page {
            path: "p".into(),
            html: String::new(),
            markdown: body.clone(),
        };
        store.replace_docset(&ds, &[], &[page]).unwrap();

        let stored: Vec<u8> = store
            .conn()
            .query_row("SELECT markdown FROM pages WHERE path = 'p'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(stored.len() < body.len() / 10, "stored compressed");
        let got = store.page("x", "p").unwrap().unwrap();
        assert_eq!((got.html.as_str(), got.markdown), ("", body));

        // Rows written before compression are plain text and still readable.
        store
            .conn()
            .execute("INSERT INTO pages (docset, path, html, markdown) VALUES ('x', 'old', '<p>', 'old md')", [])
            .unwrap();
        let old = store.page("x", "old").unwrap().unwrap();
        assert_eq!(
            (old.html.as_str(), old.markdown.as_str()),
            ("<p>", "old md")
        );

        // Removing a docset gives its pages back.
        store.remove_docset("x").unwrap();
        let free: i64 = store
            .conn()
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))
            .unwrap();
        assert_eq!(free, 0);
    }
}
