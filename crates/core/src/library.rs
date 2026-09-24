//! The public facade: install, update, remove, search, and read docsets.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::devdocs::{self, CatalogDoc, DocDb, DocIndex};
use crate::index::{Hit, Index, IndexDoc};
use crate::normalize::{chunk_markdown, html_to_markdown};
use crate::store::{self, Docset, Page, Store};

const CATALOG_MAX_AGE_SECS: i64 = 24 * 60 * 60;
const DEFAULT_MAX_CHARS: usize = 20_000;

#[derive(Debug, Serialize, Deserialize)]
pub struct DocPage {
    pub docset: String,
    pub path: String,
    pub url: String,
    /// One window of the page's markdown, starting at `offset` (in chars).
    pub markdown: String,
    pub offset: usize,
    pub total_chars: usize,
    pub next_offset: Option<usize>,
}

pub struct Library {
    home: PathBuf,
    store: Store,
    index: Index,
    /// Serializes writers: tantivy allows one index writer at a time.
    write_lock: Mutex<()>,
}

impl Library {
    pub fn open(home: &Path) -> Result<Self> {
        std::fs::create_dir_all(home)?;
        Ok(Self {
            home: home.to_path_buf(),
            store: Store::open(&home.join("meta.db"))?,
            index: Index::open(&home.join("index"))?,
            write_lock: Mutex::new(()),
        })
    }

    /// DevDocs catalog, cached on disk for a day unless `refresh` is set.
    pub fn catalog(&self, refresh: bool) -> Result<Vec<CatalogDoc>> {
        let path = self.home.join("cache").join("devdocs-catalog.json");
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| (age.as_secs() as i64) < CATALOG_MAX_AGE_SECS);
        if fresh && !refresh {
            return Ok(serde_json::from_slice(&std::fs::read(&path)?)?);
        }
        let catalog = devdocs::Client::new()?.catalog()?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, serde_json::to_vec(&catalog)?)?;
        Ok(catalog)
    }

    pub fn installed(&self) -> Result<Vec<Docset>> {
        self.store.docsets()
    }

    /// Downloads and indexes a DevDocs docset (also used to update one).
    pub fn install(&self, slug: &str) -> Result<Docset> {
        let doc = self
            .catalog(false)?
            .into_iter()
            .find(|d| d.slug == slug)
            .with_context(|| format!("no DevDocs docset named `{slug}` (see `dai catalog`)"))?;
        let client = devdocs::Client::new()?;
        let index = client.index(&doc)?;
        let db = client.db(&doc)?;
        self.ingest_devdocs(&doc, index, db)
    }

    /// Installed docsets whose upstream copy has changed.
    pub fn outdated(&self, refresh: bool) -> Result<Vec<Docset>> {
        let latest: HashMap<String, i64> = self
            .catalog(refresh)?
            .into_iter()
            .map(|d| (d.slug, d.mtime))
            .collect();
        Ok(self
            .installed()?
            .into_iter()
            .filter(|ds| latest.get(&ds.id).is_some_and(|m| *m > ds.mtime))
            .collect())
    }

    /// Normalizes, stores, and indexes an already-downloaded DevDocs docset.
    pub fn ingest_devdocs(&self, doc: &CatalogDoc, index: DocIndex, db: DocDb) -> Result<Docset> {
        let pages = db
            .into_iter()
            .map(|(path, html)| {
                let markdown =
                    html_to_markdown(&html).with_context(|| format!("converting {path}"))?;
                Ok(Page {
                    path,
                    html,
                    markdown,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let ds = Docset {
            id: doc.slug.clone(),
            name: doc.name.clone(),
            source: "devdocs".into(),
            version: if doc.version.is_empty() {
                doc.release.clone()
            } else {
                doc.version.clone()
            },
            release: doc.release.clone(),
            mtime: doc.mtime,
            installed_at: store::now(),
        };

        // Page title for chunks = the entry that points at the page itself.
        let titles: HashMap<&str, &str> = index
            .entries
            .iter()
            .filter(|e| !e.path.contains('#'))
            .map(|e| (e.path.as_str(), e.name.as_str()))
            .collect();
        let chunks: Vec<(&str, &str, _)> = pages
            .iter()
            .map(|p| {
                (
                    p.path.as_str(),
                    titles.get(p.path.as_str()).copied().unwrap_or(&p.path),
                    chunk_markdown(&p.markdown),
                )
            })
            .collect();
        let entries = index.entries.iter().map(|e| IndexDoc::Entry {
            name: &e.name,
            entry_type: &e.kind,
            path: &e.path,
        });
        let chunk_docs = chunks.iter().flat_map(|(path, title, cs)| {
            cs.iter().map(move |c| IndexDoc::Chunk {
                path,
                title,
                heading: &c.heading,
                body: &c.body,
            })
        });
        let _w = self.write_lock();
        self.index
            .replace_docset(&ds.id, entries.chain(chunk_docs))?;
        self.store.replace_docset(&ds, &index.entries, &pages)?;
        Ok(ds)
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let _w = self.write_lock();
        self.index.remove_docset(id)?;
        self.store.remove_docset(id)
    }

    fn write_lock(&self) -> MutexGuard<'_, ()> {
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn search(&self, query: &str, docsets: &[String], limit: usize) -> Result<Vec<Hit>> {
        self.index.search(query, docsets, limit)
    }

    /// A window of a page's markdown. `path` may carry a `#anchor`, which is ignored.
    pub fn get_doc(
        &self,
        docset: &str,
        path: &str,
        offset: usize,
        max_chars: Option<usize>,
    ) -> Result<Option<DocPage>> {
        let page_path = path.split('#').next().unwrap_or(path);
        let Some(page) = self.store.page(docset, page_path)? else {
            if self.store.docset(docset)?.is_none() {
                bail!("docset `{docset}` is not installed");
            }
            return Ok(None);
        };
        let (markdown, total_chars, next_offset) = window(
            &page.markdown,
            offset,
            max_chars.unwrap_or(DEFAULT_MAX_CHARS),
        );
        Ok(Some(DocPage {
            docset: docset.to_string(),
            url: devdocs::page_url(docset, path),
            path: page.path,
            markdown,
            offset,
            total_chars,
            next_offset,
        }))
    }
}

/// Slices `text` by chars, preferring to end on a line break.
fn window(text: &str, offset: usize, max_chars: usize) -> (String, usize, Option<usize>) {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let start = offset.min(total);
    let mut end = (start + max_chars).min(total);
    if end < total
        && let Some(nl) = chars[start..end].iter().rposition(|c| *c == '\n')
        && nl > 0
    {
        end = start + nl + 1;
    }
    let next = (end < total).then_some(end);
    (chars[start..end].iter().collect(), total, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Entry;

    fn fixture() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(dir.path()).unwrap();
        let doc = CatalogDoc {
            name: "React".into(),
            slug: "react".into(),
            version: String::new(),
            release: "19.1".into(),
            mtime: 100,
            db_size: 0,
        };
        let entry = |name: &str, path: &str| Entry {
            name: name.into(),
            path: path.into(),
            kind: "Hooks".into(),
        };
        let index = DocIndex {
            entries: vec![
                entry("useEffect", "reference/react/useeffect"),
                entry("useState", "reference/react/usestate"),
                entry("useEffect: cleanup", "reference/react/useeffect#cleanup"),
            ],
        };
        let db = DocDb::from([
            (
                "reference/react/useeffect".into(),
                "<h1>useEffect</h1><p>Synchronize a component with an external system.</p>\
                 <h2>Cleanup</h2><p>Return a cleanup function to disconnect.</p>"
                    .into(),
            ),
            (
                "reference/react/usestate".into(),
                "<h1>useState</h1><p>Call useState to add a state variable. useState returns a pair.</p>".into(),
            ),
        ]);
        lib.ingest_devdocs(&doc, index, db).unwrap();
        (dir, lib)
    }

    #[test]
    fn exact_name_ranks_first() {
        let (_d, lib) = fixture();
        let hits = lib.search("useEffect", &[], 5).unwrap();
        assert_eq!(hits[0].name, "useEffect");
        assert_eq!(hits[0].kind, "entry");
    }

    #[test]
    fn prefix_matches_for_type_ahead() {
        let (_d, lib) = fixture();
        let hits = lib.search("usest", &[], 5).unwrap();
        assert_eq!(
            (hits[0].name.as_str(), hits[0].kind.as_str()),
            ("useState", "entry")
        );
    }

    #[test]
    fn body_search_finds_chunks_with_stemming() {
        let (_d, lib) = fixture();
        let hits = lib.search("disconnecting", &[], 5).unwrap();
        let chunk = hits.iter().find(|h| h.kind == "chunk").expect("chunk hit");
        assert_eq!(chunk.heading, "useEffect > Cleanup");
        assert_eq!(chunk.name, "useEffect");
    }

    #[test]
    fn docset_filter_and_remove() {
        let (_d, lib) = fixture();
        assert!(
            lib.search("useEffect", &["vue".into()], 5)
                .unwrap()
                .is_empty()
        );
        assert!(lib.remove("react").unwrap());
        assert!(lib.search("useEffect", &[], 5).unwrap().is_empty());
        assert!(lib.installed().unwrap().is_empty());
    }

    #[test]
    fn get_doc_strips_anchor_and_pages() {
        let (_d, lib) = fixture();
        let page = lib
            .get_doc("react", "reference/react/useeffect#cleanup", 0, Some(20))
            .unwrap()
            .unwrap();
        assert!(page.markdown.starts_with("# useEffect"));
        let next = page.next_offset.expect("more pages");
        let rest = lib
            .get_doc("react", "reference/react/useeffect", next, None)
            .unwrap()
            .unwrap();
        assert_eq!(rest.next_offset, None);
        assert_eq!(
            page.markdown.chars().count() + rest.markdown.chars().count(),
            page.total_chars
        );
        assert!(lib.get_doc("nope", "x", 0, None).is_err());
    }
}
