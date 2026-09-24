//! The public facade: install, update, remove, search, and read docsets from
//! DevDocs and Dash/Zeal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::dash::{self, ZealDoc};
use crate::devdocs::{self, CatalogDoc, DocDb, DocIndex};
use crate::index::{Hit, Index, IndexDoc};
use crate::normalize::{chunk_markdown, html_to_markdown, main_content};
use crate::store::{self, Docset, Entry, Page, Store};

const CATALOG_MAX_AGE_SECS: u64 = 24 * 60 * 60;
const DEFAULT_MAX_CHARS: usize = 20_000;

/// One installable docset from any source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Docset id once installed: the DevDocs slug, or `dash:<name>`.
    pub id: String,
    pub name: String,
    /// `devdocs` or `dash`.
    pub source: String,
    pub version: String,
    /// Download size in bytes.
    pub size: u64,
    /// DevDocs modification time (0 for Dash, which is versioned instead).
    pub mtime: i64,
}

/// Install progress, for the app's progress display.
#[derive(Debug, Clone, Copy)]
pub enum Progress {
    Download {
        bytes: u64,
        total: Option<u64>,
    },
    /// Converting and indexing pages.
    Index,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DocPage {
    pub docset: String,
    pub path: String,
    /// Upstream URL, when the source has one.
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

    /// Every installable docset (DevDocs, then Dash), cached for a day unless `refresh`.
    pub fn catalog(&self, refresh: bool) -> Result<Vec<CatalogEntry>> {
        let devdocs = self
            .devdocs_catalog(refresh)?
            .into_iter()
            .map(|d| CatalogEntry {
                version: if d.release.is_empty() {
                    d.version
                } else {
                    d.release
                },
                id: d.slug,
                name: d.name,
                source: "devdocs".into(),
                size: d.db_size,
                mtime: d.mtime,
            });
        let dash = self
            .dash_catalog(refresh)?
            .into_iter()
            .map(|d| CatalogEntry {
                id: d.id(),
                version: d.latest_version().to_string(),
                name: d.title,
                source: "dash".into(),
                size: d.size,
                mtime: 0,
            });
        Ok(devdocs.chain(dash).collect())
    }

    fn devdocs_catalog(&self, refresh: bool) -> Result<Vec<CatalogDoc>> {
        self.cached("devdocs-catalog.json", refresh, || {
            devdocs::Client::new()?.catalog()
        })
    }

    fn dash_catalog(&self, refresh: bool) -> Result<Vec<ZealDoc>> {
        self.cached("zeal-catalog.json", refresh, dash::catalog)
    }

    fn cached<T: Serialize + DeserializeOwned>(
        &self,
        file: &str,
        refresh: bool,
        fetch: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let path = self.home.join("cache").join(file);
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() < CATALOG_MAX_AGE_SECS);
        if fresh && !refresh {
            return Ok(serde_json::from_slice(&std::fs::read(&path)?)?);
        }
        let value = fetch()?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, serde_json::to_vec(&value)?)?;
        Ok(value)
    }

    pub fn installed(&self) -> Result<Vec<Docset>> {
        self.store.docsets()
    }

    /// Installed docsets with a newer upstream copy.
    pub fn outdated(&self, refresh: bool) -> Result<Vec<Docset>> {
        let latest: HashMap<String, CatalogEntry> = self
            .catalog(refresh)?
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect();
        Ok(self
            .installed()?
            .into_iter()
            .filter(|ds| {
                latest
                    .get(&ds.id)
                    .is_some_and(|c| match ds.source.as_str() {
                        "dash" => !c.version.is_empty() && c.version != ds.version,
                        _ => c.mtime > ds.mtime,
                    })
            })
            .collect())
    }

    /// Downloads and indexes a docset by id (also used to update one).
    pub fn install(&self, id: &str, progress: &(dyn Fn(Progress) + Sync)) -> Result<Docset> {
        match id.strip_prefix(dash::ID_PREFIX) {
            Some(name) => self.install_dash(name, progress),
            None => self.install_devdocs(id, progress),
        }
    }

    fn install_devdocs(&self, slug: &str, progress: &(dyn Fn(Progress) + Sync)) -> Result<Docset> {
        let doc = self
            .devdocs_catalog(false)?
            .into_iter()
            .find(|d| d.slug == slug)
            .with_context(|| format!("no docset `{slug}` in the catalog (see `dai catalog`)"))?;
        progress(Progress::Download {
            bytes: 0,
            total: Some(doc.db_size),
        });
        let client = devdocs::Client::new()?;
        let index = client.index(&doc)?;
        let db = client.db(&doc)?;
        progress(Progress::Index);
        self.ingest_devdocs(&doc, index, db)
    }

    /// Normalizes, stores, and indexes an already-downloaded DevDocs docset.
    pub fn ingest_devdocs(&self, doc: &CatalogDoc, index: DocIndex, db: DocDb) -> Result<Docset> {
        let pages = db
            .into_par_iter()
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
        let _w = self.write_lock();
        self.commit(&ds, &index.entries, &pages)?;
        Ok(ds)
    }

    fn install_dash(&self, name: &str, progress: &(dyn Fn(Progress) + Sync)) -> Result<Docset> {
        let doc = self
            .dash_catalog(false)?
            .into_iter()
            .find(|d| d.name == name)
            .with_context(|| {
                format!("no docset `dash:{name}` in the catalog (see `dai catalog`)")
            })?;
        let root = self.dash_root();
        std::fs::create_dir_all(&root)?;
        let dir = dash::dir_name(&doc.name);
        let tgz = root.join(format!("{dir}.tgz.part"));
        dash::download(&doc.name, &tgz, |bytes, total| {
            progress(Progress::Download { bytes, total })
        })?;
        progress(Progress::Index);

        let staging = root.join(format!("{dir}.new"));
        let _ = std::fs::remove_dir_all(&staging);
        let extracted = dash::extract(&tgz, &staging);
        let _ = std::fs::remove_file(&tgz);
        extracted?;

        let ds = Docset {
            id: doc.id(),
            version: doc.latest_version().to_string(),
            name: doc.title,
            source: "dash".into(),
            release: String::new(),
            mtime: 0,
            installed_at: store::now(),
        };
        let res = self.ingest_dash(&ds, &staging, &root.join(&dir));
        let _ = std::fs::remove_dir_all(&staging);
        res.map(|()| ds)
    }

    /// Indexes an extracted Dash docset in `staging`, then moves it to `dest`
    /// (replacing any previous copy) and commits.
    pub fn ingest_dash(&self, ds: &Docset, staging: &Path, dest: &Path) -> Result<()> {
        let docset_dir = dash::find_docset_dir(staging)?;
        let entries = dash::read_index(&docset_dir)?;
        let docs = dash::documents_dir(&docset_dir);

        let mut page_paths: Vec<&str> = entries.iter().map(|e| page_path(&e.path)).collect();
        page_paths.sort_unstable();
        page_paths.dedup();
        let pages: Vec<Page> = page_paths
            .into_par_iter()
            .filter(|p| is_html(p))
            .filter_map(|p| {
                let bytes = std::fs::read(dash::resolve(&docs, p)?).ok()?;
                let html = String::from_utf8_lossy(&bytes);
                let markdown = html_to_markdown(&main_content(&html)).ok()?;
                // The viewer serves Dash pages from disk, so no HTML copy is kept.
                Some(Page {
                    path: p.to_string(),
                    html: String::new(),
                    markdown,
                })
            })
            .collect();

        let _w = self.write_lock();
        if dest.exists() {
            std::fs::remove_dir_all(dest)?;
        }
        std::fs::rename(staging, dest)?;
        self.commit(ds, &entries, &pages)
    }

    /// Replaces a docset in the index and store. Callers hold the write lock.
    fn commit(&self, ds: &Docset, entries: &[Entry], pages: &[Page]) -> Result<()> {
        // Page title for chunks = the entry that points at the page itself.
        let titles: HashMap<&str, &str> = entries
            .iter()
            .filter(|e| !e.path.contains('#'))
            .map(|e| (e.path.as_str(), e.name.as_str()))
            .collect();
        let chunks: Vec<(&str, &str, _)> = pages
            .par_iter()
            .map(|p| {
                let title = titles.get(p.path.as_str()).copied().unwrap_or(&p.path);
                (p.path.as_str(), title, chunk_markdown(&p.markdown))
            })
            .collect();
        let entry_docs = entries.iter().map(|e| IndexDoc::Entry {
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
        self.index
            .replace_docset(&ds.id, entry_docs.chain(chunk_docs))?;
        self.store.replace_docset(ds, entries, pages)
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let _w = self.write_lock();
        self.index.remove_docset(id)?;
        let removed = self.store.remove_docset(id)?;
        if let Some(name) = id.strip_prefix(dash::ID_PREFIX) {
            let dir = self.dash_root().join(dash::dir_name(name));
            if dir.exists() {
                std::fs::remove_dir_all(dir)?;
            }
        }
        Ok(removed)
    }

    fn write_lock(&self) -> MutexGuard<'_, ()> {
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn dash_root(&self) -> PathBuf {
        self.home.join("docsets").join("dash")
    }

    pub fn search(&self, query: &str, docsets: &[String], limit: usize) -> Result<Vec<Hit>> {
        self.index.search(query, docsets, limit)
    }

    /// A DevDocs page's original HTML, for the app's viewer.
    pub fn page_html(&self, docset: &str, path: &str) -> Result<Option<String>> {
        Ok(self.store.page(docset, page_path(path))?.map(|p| p.html))
    }

    /// A file (page or asset) of an installed Dash docset, for the app's viewer.
    /// `None` for other sources or files that don't exist.
    pub fn content_file(&self, docset: &str, path: &str) -> Result<Option<PathBuf>> {
        let Some(name) = docset.strip_prefix(dash::ID_PREFIX) else {
            return Ok(None);
        };
        let dir = self.dash_root().join(dash::dir_name(name));
        if !dir.exists() {
            return Ok(None);
        }
        let docs = dash::documents_dir(&dash::find_docset_dir(&dir)?);
        Ok(dash::resolve(&docs, page_path(path)))
    }

    /// A window of a page's markdown. `path` may carry a `#anchor`, which is ignored.
    pub fn get_doc(
        &self,
        docset: &str,
        path: &str,
        offset: usize,
        max_chars: Option<usize>,
    ) -> Result<Option<DocPage>> {
        let Some(page) = self.store.page(docset, page_path(path))? else {
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
        let url = if docset.starts_with(dash::ID_PREFIX) {
            String::new()
        } else {
            devdocs::page_url(docset, path)
        };
        Ok(Some(DocPage {
            docset: docset.to_string(),
            url,
            path: page.path,
            markdown,
            offset,
            total_chars,
            next_offset,
        }))
    }
}

/// A path without its `#fragment` or `?query`.
fn page_path(path: &str) -> &str {
    path.split(['#', '?']).next().unwrap_or(path)
}

fn is_html(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("html" | "htm" | "xhtml"))
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

    /// A minimal Dash docset: one full-site page with nav chrome and a
    /// `dash_ref` anchor entry.
    fn dash_fixture(lib: &Library, home: &Path) -> PathBuf {
        let staging = home.join("staging");
        let res = staging.join("Widgets.docset/Contents/Resources");
        std::fs::create_dir_all(res.join("Documents/api")).unwrap();
        std::fs::write(
            res.join("Documents/api/widget.html"),
            "<html><body><nav>Site menu</nav><article><h1>Widget</h1>\
             <p>A widget.</p><h2><a name=\"//dash_ref/Method/frob/0\"></a>frob</h2>\
             <p>Frobnicates the widget.</p></article><footer>legal</footer></body></html>",
        )
        .unwrap();
        let conn = rusqlite::Connection::open(res.join("docSet.dsidx")).unwrap();
        conn.execute_batch(
            "CREATE TABLE searchIndex(id INTEGER PRIMARY KEY, name TEXT, type TEXT, path TEXT);
             INSERT INTO searchIndex(name, type, path) VALUES
               ('Widget', 'Class', 'api/widget.html'),
               ('Widget.frob', 'Method', 'api/widget.html#//dash_ref/Method/frob/0');",
        )
        .unwrap();
        let ds = Docset {
            id: "dash:Widgets".into(),
            name: "Widgets".into(),
            source: "dash".into(),
            version: "1.0".into(),
            release: String::new(),
            mtime: 0,
            installed_at: 0,
        };
        let dest = lib.dash_root().join("Widgets");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        lib.ingest_dash(&ds, &staging, &dest).unwrap();
        dest
    }

    #[test]
    fn dash_docsets_index_serve_and_remove() {
        let (dir, lib) = fixture();
        let dest = dash_fixture(&lib, dir.path());

        let hits = lib.search("frob", &[], 5).unwrap();
        assert_eq!(hits[0].docset, "dash:Widgets");
        assert_eq!(hits[0].path, "api/widget.html#//dash_ref/Method/frob/0");

        let page = lib
            .get_doc("dash:Widgets", &hits[0].path, 0, None)
            .unwrap()
            .unwrap();
        assert!(page.markdown.starts_with("# Widget"), "{}", page.markdown);
        assert!(!page.markdown.contains("Site menu") && !page.markdown.contains("legal"));
        assert!(page.url.is_empty());

        let file = lib
            .content_file("dash:Widgets", &hits[0].path)
            .unwrap()
            .unwrap();
        assert!(file.starts_with(&dest));
        assert!(
            lib.content_file("dash:Widgets", "../../meta.db")
                .unwrap()
                .is_none()
        );
        assert!(
            lib.content_file("react", "reference/react/useeffect")
                .unwrap()
                .is_none()
        );

        // DevDocs results are still there alongside.
        assert_eq!(lib.search("useEffect", &[], 1).unwrap()[0].docset, "react");

        assert!(lib.remove("dash:Widgets").unwrap());
        assert!(!dest.exists());
        assert!(lib.search("frob", &[], 5).unwrap().is_empty());
    }
}
