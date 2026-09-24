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
use crate::generate;
use crate::index::SNIPPETS_DOCSET;
use crate::index::{Hit, Index, IndexDoc};
use crate::markdown;
use crate::normalize::{chunk_markdown, html_to_markdown, main_content};
use crate::project::{self, ProjectReport};
use crate::snippets::{Snippet, SnippetInput, SnippetStore, slugify};
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
    /// Older versions installable side by side as `<id>@<version>` (Dash only;
    /// DevDocs lists versions as separate ids like `react~18`).
    #[serde(default)]
    pub versions: Vec<String>,
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
    snippets: SnippetStore,
    /// Serializes writers: tantivy allows one index writer at a time.
    write_lock: Mutex<()>,
}

impl Library {
    pub fn open(home: &Path, snippets_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(home)?;
        let lib = Self {
            home: home.to_path_buf(),
            store: Store::open(&home.join("meta.db"))?,
            index: Index::open(&home.join("index"))?,
            snippets: SnippetStore::open(snippets_dir)?,
            write_lock: Mutex::new(()),
        };
        lib.index_snippets()?;
        Ok(lib)
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
                versions: vec![],
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
                versions: d.versions.iter().skip(1).cloned().collect(),
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
                        "dash" => {
                            !ds.id.contains('@') && !c.version.is_empty() && c.version != ds.version
                        }
                        "devdocs" => c.mtime > ds.mtime,
                        // Generated docsets are only updated on request.
                        _ => false,
                    })
            })
            .collect())
    }

    /// Downloads and indexes a docset by id (also used to update one).
    /// For generated (`md:`) docsets this re-runs the generator.
    pub fn install(&self, id: &str, progress: &(dyn Fn(Progress) + Sync)) -> Result<Docset> {
        if let Some(name) = id.strip_prefix(dash::ID_PREFIX) {
            return self.install_dash(name, progress);
        }
        if let Some(slug) = id.strip_prefix(generate::ID_PREFIX) {
            let manifest = generate::read_manifest(&self.md_root().join(slug))
                .with_context(|| format!("`{id}` is not installed; generate it first"))?;
            return self.generate(Some(&manifest.name), manifest.source, progress);
        }
        self.install_devdocs(id, progress)
    }

    /// Builds a markdown docset from `source` and indexes it as `md:<slug>`
    /// (slug from `name`, else derived from the source). Replaces an existing
    /// docset with the same id.
    pub fn generate(
        &self,
        name: Option<&str>,
        source: generate::Source,
        progress: &(dyn Fn(Progress) + Sync),
    ) -> Result<Docset> {
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map_or_else(|| source.default_name(), String::from);
        let slug = slugify(&name);
        progress(Progress::Download {
            bytes: 0,
            total: None,
        });
        let generated = generate::fetch(&source, &|done, total| {
            progress(Progress::Download { bytes: done, total });
        })?;
        progress(Progress::Index);

        let manifest = generate::Manifest {
            name: name.clone(),
            version: generated.version.clone(),
            generated_at: store::now(),
            source,
        };
        let root = self.md_root();
        std::fs::create_dir_all(&root)?;
        let staging = root.join(format!("{slug}.new"));
        let _ = std::fs::remove_dir_all(&staging);
        let written = generate::write(&staging, &manifest, &generated);
        drop(generated);
        let res = written.and_then(|()| self.ingest_markdown(&staging, &root.join(&slug)));
        let _ = std::fs::remove_dir_all(&staging);
        res
    }

    /// Indexes a markdown docset folder in `staging`, then moves it to `dest`
    /// (replacing any previous copy) and commits.
    pub fn ingest_markdown(&self, staging: &Path, dest: &Path) -> Result<Docset> {
        let manifest = generate::read_manifest(staging)?;
        let slug = dest
            .file_name()
            .and_then(|n| n.to_str())
            .context("bad docset folder")?;
        let ds = Docset {
            id: format!("{}{slug}", generate::ID_PREFIX),
            name: manifest.name.clone(),
            source: manifest.source.kind().into(),
            version: manifest.version.clone(),
            release: manifest.source.origin().to_string(),
            mtime: manifest.generated_at,
            installed_at: store::now(),
        };
        let files = generate::read_pages(staging)?;
        let (pages, entries): (Vec<Page>, Vec<Vec<Entry>>) = files
            .into_par_iter()
            .map(|(path, text)| {
                let markdown = if path.ends_with(".mdx") {
                    markdown::clean_mdx(&text)
                } else {
                    text
                };
                let stem = path.rsplit('/').next().unwrap_or(&path);
                let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
                let entries = markdown::entries(&path, &markdown, stem);
                (
                    Page {
                        path,
                        html: String::new(),
                        markdown,
                    },
                    entries,
                )
            })
            .unzip();
        if pages.is_empty() {
            bail!("no markdown pages were generated");
        }
        let entries: Vec<Entry> = entries.into_iter().flatten().collect();

        let _w = self.write_lock();
        if dest.exists() {
            std::fs::remove_dir_all(dest)?;
        }
        std::fs::rename(staging, dest)?;
        self.commit(&ds, &entries, &pages)?;
        Ok(ds)
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

    /// `name` is a Zeal name, optionally pinned: `React` or `React@18.3.1`.
    /// Pinned versions install side by side with the latest.
    fn install_dash(&self, name: &str, progress: &(dyn Fn(Progress) + Sync)) -> Result<Docset> {
        let (name, pinned) = match name.split_once('@') {
            Some((n, v)) => (n, Some(v)),
            None => (name, None),
        };
        let doc = self
            .dash_catalog(false)?
            .into_iter()
            .find(|d| d.name == name)
            .with_context(|| {
                format!("no docset `dash:{name}` in the catalog (see `dai catalog`)")
            })?;
        if let Some(v) = pinned
            && !doc.versions.iter().any(|x| x == v)
        {
            bail!(
                "`dash:{name}` has no version {v}; available: {}",
                doc.versions.join(", ")
            );
        }
        let root = self.dash_root();
        std::fs::create_dir_all(&root)?;
        let dir = dash::dir_name(&match pinned {
            Some(v) => format!("{}@{v}", doc.name),
            None => doc.name.clone(),
        });
        let tgz = root.join(format!("{dir}.tgz.part"));
        dash::download(&doc.name, pinned, &tgz, |bytes, total| {
            progress(Progress::Download { bytes, total })
        })?;
        progress(Progress::Index);

        let staging = root.join(format!("{dir}.new"));
        let _ = std::fs::remove_dir_all(&staging);
        let extracted = dash::extract(&tgz, &staging);
        let _ = std::fs::remove_file(&tgz);
        extracted?;

        let ds = Docset {
            id: match pinned {
                Some(v) => format!("{}@{v}", doc.id()),
                None => doc.id(),
            },
            version: pinned.unwrap_or(doc.latest_version()).to_string(),
            name: match pinned {
                Some(v) => format!("{} {v}", doc.title),
                None => doc.title,
            },
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
        let dir = if let Some(name) = id.strip_prefix(dash::ID_PREFIX) {
            Some(self.dash_root().join(dash::dir_name(name)))
        } else {
            id.strip_prefix(generate::ID_PREFIX)
                .map(|slug| self.md_root().join(slugify(slug)))
        };
        if let Some(dir) = dir.filter(|d| d.exists()) {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(removed)
    }

    fn write_lock(&self) -> MutexGuard<'_, ()> {
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn dash_root(&self) -> PathBuf {
        self.home.join("docsets").join("dash")
    }

    fn md_root(&self) -> PathBuf {
        self.home.join("docsets").join("md")
    }

    pub fn search(&self, query: &str, docsets: &[String], limit: usize) -> Result<Vec<Hit>> {
        self.index.search(query, docsets, &[], limit)
    }

    /// Search that prefers the project's versions: installed docsets for a
    /// different version of one of its dependencies are skipped when a
    /// matching version is installed.
    pub fn search_for_project(
        &self,
        query: &str,
        docsets: &[String],
        project: &ProjectReport,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        self.index
            .search(query, docsets, &project.excluded_docsets(), limit)
    }

    /// `search`, made project-aware when `project` is a path with manifests
    /// (a path without any falls back to a plain search).
    pub fn search_in(
        &self,
        query: &str,
        docsets: &[String],
        project: Option<&Path>,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        match project.map(|p| self.project(p)) {
            Some(Ok(report)) => self.search_for_project(query, docsets, &report, limit),
            _ => self.search(query, docsets, limit),
        }
    }

    /// The project at `path` (or its nearest ancestor with a manifest), matched
    /// against installed docsets.
    pub fn project(&self, path: &Path) -> Result<ProjectReport> {
        project::analyze(path, &self.installed()?)
    }

    /// `project` plus install suggestions from the catalog (which may be
    /// fetched if it isn't cached yet).
    pub fn project_with_suggestions(&self, path: &Path) -> Result<ProjectReport> {
        let mut report = self.project(path)?;
        if let Ok(catalog) = self.catalog(false) {
            report.suggestions = project::suggest(&report, &catalog);
        }
        Ok(report)
    }

    /// A DevDocs page's original HTML, for the app's viewer.
    pub fn page_html(&self, docset: &str, path: &str) -> Result<Option<String>> {
        Ok(self.store.page(docset, page_path(path))?.map(|p| p.html))
    }

    /// A file (page or asset) of an installed Dash or generated docset, for
    /// the app's viewer. `None` for DevDocs or files that don't exist.
    pub fn content_file(&self, docset: &str, path: &str) -> Result<Option<PathBuf>> {
        if let Some(slug) = docset.strip_prefix(generate::ID_PREFIX) {
            let pages = self.md_root().join(slugify(slug)).join("pages");
            return Ok(dash::resolve(&pages, page_path(path)));
        }
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
        let url = if docset.starts_with(dash::ID_PREFIX) || docset.starts_with(generate::ID_PREFIX)
        {
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

impl Library {
    pub fn snippets_dir(&self) -> &Path {
        self.snippets.dir()
    }

    /// Whether a filesystem change at `path` affects snippets.
    pub fn is_snippet_file(&self, path: &Path) -> bool {
        self.snippets.is_snippet_file(path)
    }

    /// Snippets matching `query` (ranked), or all of them newest first when
    /// `query` is empty. `language` and `tag` filter case-insensitively.
    pub fn snippets(
        &self,
        query: &str,
        language: Option<&str>,
        tag: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Snippet>> {
        let keep = |s: &Snippet| {
            language.is_none_or(|l| s.language.eq_ignore_ascii_case(l))
                && tag.is_none_or(|t| s.tags.iter().any(|x| x.eq_ignore_ascii_case(t)))
        };
        if query.trim().is_empty() {
            let mut all: Vec<Snippet> = self
                .snippets
                .all()
                .values()
                .filter(|s| keep(s))
                .cloned()
                .collect();
            all.sort_by(|a, b| b.updated.cmp(&a.updated));
            all.truncate(limit);
            return Ok(all);
        }
        // Over-fetch so filtering still leaves `limit` results.
        let hits = self
            .index
            .search(query, &[SNIPPETS_DOCSET.to_string()], &[], limit * 4 + 10)?;
        let all = self.snippets.all();
        Ok(hits
            .iter()
            .filter_map(|h| all.get(&h.path))
            .filter(|s| keep(s))
            .take(limit)
            .cloned()
            .collect())
    }

    pub fn snippet(&self, id: &str) -> Option<Snippet> {
        self.snippets.get(id)
    }

    pub fn create_snippet(&self, input: SnippetInput) -> Result<Snippet> {
        let s = self.snippets.create(input)?;
        self.index_snippets()?;
        Ok(s)
    }

    pub fn update_snippet(&self, id: &str, input: SnippetInput) -> Result<Snippet> {
        let s = self.snippets.update(id, input)?;
        self.index_snippets()?;
        Ok(s)
    }

    pub fn delete_snippet(&self, id: &str) -> Result<bool> {
        let deleted = self.snippets.delete(id)?;
        self.index_snippets()?;
        Ok(deleted)
    }

    /// Re-reads the snippets folder (after outside edits) and reindexes it.
    pub fn reload_snippets(&self) -> Result<()> {
        self.snippets.reload()?;
        self.index_snippets()
    }

    fn index_snippets(&self) -> Result<()> {
        let all = self.snippets.all();
        let rows: Vec<(&Snippet, String, String)> = all
            .values()
            .map(|s| {
                (
                    s,
                    s.tags.join(" "),
                    format!("{}\n{}\n{}", s.description, s.code, s.notes),
                )
            })
            .collect();
        let docs = rows.iter().map(|(s, tags, text)| IndexDoc::Snippet {
            id: &s.id,
            title: &s.title,
            language: &s.language,
            tags,
            text,
        });
        let _w = self.write_lock();
        self.index.replace_docset(SNIPPETS_DOCSET, docs)
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
        let lib = Library::open(dir.path(), &dir.path().join("snippets")).unwrap();
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

    #[test]
    fn snippets_search_filter_and_stay_out_of_doc_search() {
        let (_d, lib) = fixture();
        let input = |title: &str, lang: &str, tags: &[&str], code: &str| SnippetInput {
            title: title.into(),
            language: lang.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            code: code.into(),
            ..Default::default()
        };
        lib.create_snippet(input(
            "useEffect cleanup pattern",
            "tsx",
            &["react"],
            "useEffect(() => () => ws.close(), [])",
        ))
        .unwrap();
        lib.create_snippet(input(
            "Retry with backoff",
            "rust",
            &["async"],
            "for attempt in 0..5 {}",
        ))
        .unwrap();

        let found = lib.snippets("cleanup", None, None, 10).unwrap();
        assert_eq!(found[0].id, "useeffect-cleanup-pattern");
        assert!(
            lib.snippets("cleanup", Some("rust"), None, 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            lib.snippets("", None, Some("ASYNC"), 10).unwrap()[0].id,
            "retry-with-backoff"
        );
        assert_eq!(lib.snippets("", None, None, 10).unwrap().len(), 2);

        // Doc search without a docset filter doesn't return snippets.
        assert!(
            lib.search("useEffect", &[], 20)
                .unwrap()
                .iter()
                .all(|h| h.kind != "snippet")
        );

        assert!(lib.delete_snippet("retry-with-backoff").unwrap());
        assert!(lib.snippets("backoff", None, None, 10).unwrap().is_empty());
    }

    #[test]
    fn generated_docsets_from_a_folder() {
        let (dir, lib) = fixture();
        let src = dir.path().join("my-docs");
        std::fs::create_dir_all(src.join("guide")).unwrap();
        std::fs::write(
            src.join("guide/routing.md"),
            "# Routing\n\n## Nested layouts\n\nLayouts wrap child routes.\n",
        )
        .unwrap();
        std::fs::write(
            src.join("guide/forms.mdx"),
            "import { Callout } from 'x';\n\n# Forms\n\n<Callout>Validate on blur.</Callout>\n",
        )
        .unwrap();
        let source = generate::Source::Dir {
            path: src.to_string_lossy().into(),
        };

        let ds = lib.generate(None, source, &|_| {}).unwrap();
        assert_eq!((ds.id.as_str(), ds.source.as_str()), ("md:my-docs", "dir"));

        let hits = lib
            .search("Nested layouts", &["md:my-docs".into()], 5)
            .unwrap();
        assert_eq!(hits[0].path, "guide/routing.md#nested-layouts");
        let forms = lib
            .get_doc("md:my-docs", "guide/forms.mdx", 0, None)
            .unwrap()
            .unwrap();
        assert!(
            forms.markdown.contains("Validate on blur.") && !forms.markdown.contains("Callout")
        );
        assert!(
            lib.content_file("md:my-docs", "guide/routing.md#x")
                .unwrap()
                .is_some()
        );
        assert!(
            lib.content_file("md:my-docs", "../docset.toml")
                .unwrap()
                .is_none()
        );

        // Updating re-runs the generator from the manifest.
        std::fs::write(
            src.join("guide/routing.md"),
            "# Routing\n\n## Loaders\n\nFetch data first.\n",
        )
        .unwrap();
        lib.install("md:my-docs", &|_| {}).unwrap();
        assert!(
            lib.search("Loaders", &["md:my-docs".into()], 5)
                .unwrap()
                .iter()
                .any(|h| h.kind == "entry")
        );
        assert!(
            lib.search("Nested layouts", &["md:my-docs".into()], 5)
                .unwrap()
                .is_empty()
        );

        assert!(lib.remove("md:my-docs").unwrap());
        assert!(!lib.md_root().join("my-docs").exists());
    }
}
