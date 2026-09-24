//! Generated markdown docsets, for libraries without a DevDocs/Dash docset or
//! with stale ones. Every source produces the same on-disk format:
//!
//! ```text
//! <data dir>/docsets/md/<slug>/
//!   docset.toml     # Manifest: name, version, and how to regenerate
//!   pages/**.md     # plus .mdx and images from repos/folders
//! ```

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::normalize::{html_to_markdown, main_content};
use crate::snippets::slugify;

/// Docset ids for generated docsets are `md:<slug>`, e.g. `md:svelte`.
pub const ID_PREFIX: &str = "md:";

/// Linked pages fetched from one `llms.txt` index.
const MAX_LINKED_PAGES: usize = 2000;
const FETCH_THREADS: usize = 8;
const CONTEXT7_BASE: &str = "https://context7.com/api/v2";
const DEFAULT_CONTEXT7_TOPICS: [&str; 4] = [
    "getting started and installation",
    "core API reference",
    "configuration options",
    "common patterns and examples",
];
const ASSET_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "svg", "webp"];

/// Where a generated docset comes from; stored in its manifest so an update
/// re-runs the same generator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// A site's `llms-full.txt` / `llms.txt` (the site root or the file's URL).
    Llms { url: String },
    /// A git repository's README and docs folders (shallow clone).
    Repo {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
    },
    /// Markdown files in a local folder.
    Dir { path: String },
    /// Context7 results for a library, one page per topic.
    Context7 {
        library_id: String,
        #[serde(default)]
        topics: Vec<String>,
    },
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Llms { .. } => "llms",
            Self::Repo { .. } => "repo",
            Self::Dir { .. } => "dir",
            Self::Context7 { .. } => "context7",
        }
    }

    pub fn origin(&self) -> &str {
        match self {
            Self::Llms { url } | Self::Repo { url, .. } => url,
            Self::Dir { path } => path,
            Self::Context7 { library_id, .. } => library_id,
        }
    }

    /// A name derived from the source alone (so the docset id is known before
    /// anything is fetched): `https://tanstack.com/query/latest/llms.txt` →
    /// `tanstack-query`, a repo → its name, a folder → its name, a Context7
    /// library → `<name>-context7`.
    pub fn default_name(&self) -> String {
        match self {
            Self::Llms { url } => {
                let Ok(u) = reqwest::Url::parse(url) else {
                    return slugify(url);
                };
                let host = u.host_str().unwrap_or("docs");
                let mut labels: Vec<&str> = host.split('.').collect();
                if labels.len() > 1 {
                    labels.pop(); // TLD
                }
                labels.retain(|l| !matches!(*l, "www" | "docs"));
                let skip = [
                    "latest",
                    "docs",
                    "llms.txt",
                    "llms-full.txt",
                    "en",
                    "v1",
                    "v2",
                    "stable",
                ];
                let path = u
                    .path_segments()
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty() && !skip.contains(s));
                slugify(&labels.into_iter().chain(path).collect::<Vec<_>>().join(" "))
            }
            Self::Repo { url, .. } => {
                let last = url
                    .trim_end_matches('/')
                    .rsplit(['/', ':'])
                    .next()
                    .unwrap_or(url);
                slugify(last.trim_end_matches(".git"))
            }
            Self::Dir { path } => slugify(
                Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path),
            ),
            Self::Context7 { library_id, .. } => {
                let last = library_id
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(library_id);
                format!("{}-context7", slugify(last))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// Unix seconds.
    pub generated_at: i64,
    pub source: Source,
}

pub struct GeneratedPage {
    /// Relative path under `pages/`, ending in `.md` or `.mdx`.
    pub path: String,
    pub markdown: String,
}

pub struct Generated {
    pub version: String,
    pub pages: Vec<GeneratedPage>,
    /// Images to copy: (relative path under `pages/`, source file).
    pub assets: Vec<(String, PathBuf)>,
    /// Keeps a clone alive until its assets are copied.
    _tmp: Option<tempfile::TempDir>,
}

/// Runs a generator. `progress` gets `(done, total)` page counts where known.
pub fn fetch(source: &Source, progress: &(dyn Fn(u64, Option<u64>) + Sync)) -> Result<Generated> {
    match source {
        Source::Llms { url } => llms(url, progress),
        Source::Repo { url, git_ref } => repo(url, git_ref.as_deref()),
        Source::Dir { path } => {
            let root = Path::new(path);
            if !root.is_dir() {
                bail!("{path} is not a folder");
            }
            Ok(collect_dir(root, root, today())?)
        }
        Source::Context7 { library_id, topics } => context7(library_id, topics, progress),
    }
}

/// Writes a docset folder (manifest, pages, assets) at `dir`.
pub fn write(dir: &Path, manifest: &Manifest, generated: &Generated) -> Result<()> {
    let pages = dir.join("pages");
    std::fs::create_dir_all(&pages)?;
    std::fs::write(dir.join("docset.toml"), toml::to_string_pretty(manifest)?)?;
    for p in &generated.pages {
        let path = pages.join(safe_rel(&p.path)?);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, &p.markdown)?;
    }
    for (rel, src) in &generated.assets {
        let path = pages.join(safe_rel(rel)?);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::copy(src, path)?;
    }
    Ok(())
}

pub fn read_manifest(dir: &Path) -> Result<Manifest> {
    let path = dir.join("docset.toml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(toml::from_str(&text)?)
}

/// Markdown files under a docset's `pages/`, as (relative path, contents).
pub fn read_pages(dir: &Path) -> Result<Vec<(String, String)>> {
    let root = dir.join("pages");
    let mut out = Vec::new();
    for file in walk(&root)? {
        if is_markdown(&file) {
            let rel = rel_path(&root, &file);
            out.push((rel, std::fs::read_to_string(&file)?));
        }
    }
    Ok(out)
}

fn safe_rel(rel: &str) -> Result<&Path> {
    let p = Path::new(rel);
    if rel.is_empty() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        bail!("unsafe page path `{rel}`");
    }
    Ok(p)
}

fn today() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

// --- llms.txt -------------------------------------------------------------

fn llms(url: &str, progress: &(dyn Fn(u64, Option<u64>) + Sync)) -> Result<Generated> {
    let http = crate::net::client(Some(Duration::from_secs(60)))?;
    let base = url.trim_end_matches('/');
    let candidates: Vec<String> = if base.ends_with(".txt") || base.ends_with(".md") {
        vec![base.to_string()]
    } else {
        vec![format!("{base}/llms-full.txt"), format!("{base}/llms.txt")]
    };
    for candidate in candidates {
        let Ok(res) = http.get(&candidate).send() else {
            continue;
        };
        let is_html = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("html"));
        if !res.status().is_success() || is_html {
            continue;
        }
        let text = res.text()?;
        if text.trim().is_empty() {
            continue;
        }
        let pages = if candidate.ends_with("llms-full.txt") {
            split_by_h1(&text)
        } else {
            linked_pages(&http, &candidate, &text, progress)?
        };
        return Ok(Generated {
            version: today(),
            pages,
            assets: vec![],
            _tmp: None,
        });
    }
    bail!("no llms-full.txt or llms.txt found at {url}")
}

/// Splits an `llms-full.txt` concatenation into one page per `# ` section.
fn split_by_h1(text: &str) -> Vec<GeneratedPage> {
    let mut sections: Vec<(String, String)> = vec![("index".into(), String::new())];
    let mut in_fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence && let Some(title) = line.strip_prefix("# ") {
            sections.push((title.trim().to_string(), String::new()));
        }
        let body = &mut sections.last_mut().expect("non-empty").1;
        body.push_str(line);
        body.push('\n');
    }
    let mut used = HashSet::new();
    sections
        .into_iter()
        .filter(|(_, body)| !body.trim().is_empty())
        .map(|(title, markdown)| GeneratedPage {
            path: unique(&mut used, &slugify(&title), "md"),
            markdown,
        })
        .collect()
}

/// Fetches the pages an `llms.txt` index links to (same host, one level deep).
fn linked_pages(
    http: &reqwest::blocking::Client,
    index_url: &str,
    index: &str,
    progress: &(dyn Fn(u64, Option<u64>) + Sync),
) -> Result<Vec<GeneratedPage>> {
    static LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(([^)\s]+)\)").unwrap());
    let base = reqwest::Url::parse(index_url)?;
    let mut seen = HashSet::from([base.as_str().to_string()]);
    let links: Vec<(String, reqwest::Url)> = LINK
        .captures_iter(index)
        .filter_map(|c| {
            let mut url = base.join(&c[2]).ok()?;
            url.set_fragment(None);
            let same_site = url.host_str() == base.host_str() && url.scheme().starts_with("http");
            (same_site && seen.insert(url.to_string())).then(|| (c[1].to_string(), url))
        })
        .take(MAX_LINKED_PAGES)
        .collect();

    let total = links.len() as u64;
    let done = AtomicU64::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(FETCH_THREADS)
        .build()?;
    let fetched: Vec<(String, String, String)> = pool.install(|| {
        links
            .par_iter()
            .filter_map(|(title, url)| {
                let md = fetch_page(http, url).ok();
                progress(done.fetch_add(1, Ordering::Relaxed) + 1, Some(total));
                Some((title.clone(), url.path().to_string(), md?))
            })
            .collect()
    });

    let mut used = HashSet::new();
    let mut pages = vec![GeneratedPage {
        path: unique(&mut used, "index", "md"),
        markdown: index.to_string(),
    }];
    for (title, url_path, mut markdown) in fetched {
        if crate::markdown::title(&markdown).is_none() {
            markdown = format!("# {title}\n\n{markdown}");
        }
        pages.push(GeneratedPage {
            path: unique(&mut used, &page_stem(&url_path), "md"),
            markdown,
        });
    }
    Ok(pages)
}

fn fetch_page(http: &reqwest::blocking::Client, url: &reqwest::Url) -> Result<String> {
    let res = http.get(url.clone()).send()?.error_for_status()?;
    let is_html = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("html"));
    let body = res.text()?;
    if is_html {
        html_to_markdown(&main_content(&body))
    } else {
        Ok(body)
    }
}

/// A URL path as a relative page path without extension: `/query/latest/docs/overview.md` →
/// `query/latest/docs/overview`.
fn page_stem(url_path: &str) -> String {
    let trimmed = url_path.trim_matches('/');
    let without_ext = trimmed
        .strip_suffix(".md")
        .or_else(|| trimmed.strip_suffix(".txt"))
        .or_else(|| trimmed.strip_suffix(".html"))
        .unwrap_or(trimmed);
    let parts: Vec<String> = without_ext
        .split('/')
        .filter(|s| !s.is_empty())
        .map(slugify)
        .collect();
    if parts.is_empty() {
        "index".into()
    } else {
        parts.join("/")
    }
}

/// `stem.ext`, or `stem-2.ext`, … if already used.
fn unique(used: &mut HashSet<String>, stem: &str, ext: &str) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem}-{n}.{ext}")
            }
        })
        .find(|p| used.insert(p.clone()))
        .expect("unbounded range")
}

// --- git repos and folders --------------------------------------------------

fn repo(url: &str, git_ref: Option<&str>) -> Result<Generated> {
    let tmp = tempfile::tempdir()?;
    let mut cmd = Command::new("git");
    cmd.args(["clone", "--depth", "1", "--quiet"]);
    if let Some(r) = git_ref {
        cmd.args(["--branch", r]);
    }
    let out = cmd
        .arg(url)
        .arg(tmp.path())
        .output()
        .context("running git (is it installed and on PATH?)")?;
    if !out.status.success() {
        bail!(
            "git clone failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let version = match git_ref {
        Some(r) => r.to_string(),
        None => {
            let head = Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(["rev-parse", "--short", "HEAD"])
                .output()?;
            String::from_utf8_lossy(&head.stdout).trim().to_string()
        }
    };

    // Prefer the README plus conventional docs folders; fall back to every
    // markdown file in the repo.
    let docs_dirs = [
        "docs",
        "doc",
        "documentation",
        "website/docs",
        "content/docs",
    ];
    let found: Vec<PathBuf> = docs_dirs
        .iter()
        .map(|d| tmp.path().join(d))
        .filter(|d| d.is_dir())
        .collect();
    let mut generated = if found.is_empty() {
        collect_dir(tmp.path(), tmp.path(), version)?
    } else {
        let mut g = Generated {
            version,
            pages: vec![],
            assets: vec![],
            _tmp: None,
        };
        for readme in ["README.md", "readme.md", "Readme.md"] {
            let p = tmp.path().join(readme);
            if p.is_file() {
                g.pages.push(GeneratedPage {
                    path: "README.md".into(),
                    markdown: std::fs::read_to_string(p)?,
                });
                break;
            }
        }
        for d in found {
            let sub = collect_dir(&d, tmp.path(), String::new())?;
            g.pages.extend(sub.pages);
            g.assets.extend(sub.assets);
        }
        g
    };
    if generated.pages.is_empty() {
        bail!("no markdown files found in {url}");
    }
    generated._tmp = Some(tmp);
    Ok(generated)
}

/// Markdown files and images under `dir`, with paths relative to `root`.
fn collect_dir(dir: &Path, root: &Path, version: String) -> Result<Generated> {
    let mut g = Generated {
        version,
        pages: vec![],
        assets: vec![],
        _tmp: None,
    };
    for file in walk(dir)? {
        let rel = rel_path(root, &file);
        if is_markdown(&file) {
            g.pages.push(GeneratedPage {
                path: rel,
                markdown: std::fs::read_to_string(&file)?,
            });
        } else if file
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ASSET_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        {
            g.assets.push((rel, file));
        }
    }
    Ok(g)
}

/// Files under `dir`, skipping hidden entries and dependency/build folders.
fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    const SKIP: [&str; 5] = ["node_modules", "target", "vendor", "dist", "build"];
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP.contains(&name.as_ref()) {
                continue;
            }
            let ty = entry.file_type()?;
            if ty.is_dir() {
                stack.push(entry.path());
            } else if ty.is_file() {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    Ok(out)
}

fn is_markdown(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("mdx"))
}

fn rel_path(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

// --- Context7 -----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context7Library {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub total_snippets: u64,
    #[serde(default)]
    pub versions: Vec<String>,
}

fn context7_get(path: &str, params: &[(&str, &str)]) -> Result<reqwest::blocking::Response> {
    let mut req = crate::net::client(Some(Duration::from_secs(60)))?
        .get(format!("{CONTEXT7_BASE}{path}"))
        .query(params);
    if let Some(key) = crate::paths::context7_api_key() {
        req = req.bearer_auth(key);
    }
    let res = req.send()?;
    if !res.status().is_success() {
        let status = res.status();
        bail!(
            "Context7 returned {status}: {}",
            res.text().unwrap_or_default().trim()
        );
    }
    Ok(res)
}

/// Libraries matching `name`, ranked for `query`.
pub fn context7_search(name: &str, query: &str) -> Result<Vec<Context7Library>> {
    #[derive(Deserialize)]
    struct Results {
        results: Vec<Context7Library>,
    }
    let query = if query.trim().is_empty() { name } else { query };
    let res = context7_get(
        "/libs/search",
        &[("libraryName", name), ("query", query), ("fast", "true")],
    )?;
    Ok(res.json::<Results>()?.results)
}

/// Documentation for `query` from one library, as markdown.
pub fn context7_docs(library_id: &str, query: &str) -> Result<String> {
    Ok(context7_get("/context", &[("libraryId", library_id), ("query", query)])?.text()?)
}

fn context7(
    library_id: &str,
    topics: &[String],
    progress: &(dyn Fn(u64, Option<u64>) + Sync),
) -> Result<Generated> {
    let topics: Vec<String> = if topics.is_empty() {
        DEFAULT_CONTEXT7_TOPICS
            .iter()
            .map(|t| t.to_string())
            .collect()
    } else {
        topics.to_vec()
    };
    let mut used = HashSet::new();
    let mut pages = Vec::new();
    for (i, topic) in topics.iter().enumerate() {
        let text = context7_docs(library_id, topic)?;
        pages.push(GeneratedPage {
            path: unique(&mut used, &slugify(topic), "md"),
            markdown: format!("# {topic}\n\n{text}"),
        });
        progress(i as u64 + 1, Some(topics.len() as u64));
    }
    Ok(Generated {
        version: today(),
        pages,
        assets: vec![],
        _tmp: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names() {
        let llms = |u: &str| Source::Llms { url: u.into() }.default_name();
        assert_eq!(llms("https://svelte.dev"), "svelte");
        assert_eq!(
            llms("https://orm.drizzle.team/llms-full.txt"),
            "orm-drizzle"
        );
        assert_eq!(
            llms("https://tanstack.com/query/latest/llms.txt"),
            "tanstack-query"
        );
        assert_eq!(llms("https://docs.astro.build/"), "astro");
        let repo = |u: &str| {
            Source::Repo {
                url: u.into(),
                git_ref: None,
            }
            .default_name()
        };
        assert_eq!(repo("https://github.com/withastro/docs.git"), "docs");
        assert_eq!(repo("git@github.com:honojs/website"), "website");
        let c7 = Source::Context7 {
            library_id: "/sveltejs/svelte".into(),
            topics: vec![],
        };
        assert_eq!(c7.default_name(), "svelte-context7");
    }

    #[test]
    fn llms_full_splits_on_h1_outside_fences() {
        let pages =
            split_by_h1("preamble\n# Intro\ntext\n```md\n# not a page\n```\n# Intro\nagain\n");
        let paths: Vec<_> = pages.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths, ["index.md", "intro.md", "intro-2.md"]);
        assert!(pages[1].markdown.contains("# not a page"));
    }

    #[test]
    fn page_stems_from_urls() {
        assert_eq!(
            page_stem("/query/latest/docs/overview.md"),
            "query/latest/docs/overview"
        );
        assert_eq!(page_stem("/"), "index");
        assert_eq!(
            page_stem("/Guide/Getting Started.html"),
            "guide/getting-started"
        );
    }

    #[test]
    fn folders_round_trip_through_write() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("guide/img")).unwrap();
        std::fs::create_dir_all(src.path().join("node_modules/x")).unwrap();
        std::fs::write(src.path().join("guide/intro.mdx"), "# Intro").unwrap();
        std::fs::write(src.path().join("guide/img/a.png"), [0u8; 4]).unwrap();
        std::fs::write(src.path().join("node_modules/x/README.md"), "skip").unwrap();

        let source = Source::Dir {
            path: src.path().to_string_lossy().into(),
        };
        let generated = fetch(&source, &|_, _| {}).unwrap();
        let manifest = Manifest {
            name: "x".into(),
            version: generated.version.clone(),
            generated_at: 0,
            source,
        };
        let out = tempfile::tempdir().unwrap();
        write(out.path(), &manifest, &generated).unwrap();

        assert_eq!(read_manifest(out.path()).unwrap().source.kind(), "dir");
        let pages = read_pages(out.path()).unwrap();
        assert_eq!(
            pages,
            [("guide/intro.mdx".to_string(), "# Intro".to_string())]
        );
        assert!(out.path().join("pages/guide/img/a.png").is_file());
    }
}
