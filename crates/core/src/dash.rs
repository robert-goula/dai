//! Dash/Zeal docsets: the Zeal catalog (Kapeli official, user-contributed, and
//! cheatsheets), tarball download and extraction, and `docSet.dsidx` reading.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::store::Entry;

/// Docset ids for Dash docsets are `dash:<Zeal name>`, e.g. `dash:React`.
pub const ID_PREFIX: &str = "dash:";

const CATALOG_URL: &str = "https://api.zealdocs.org/v1/docsets";
const DOWNLOAD_BASE: &str = "https://go.zealdocs.org/d/com.kapeli";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZealDoc {
    /// Feed name, e.g. `React`, `ABAP_Contrib`, `Ack_Cheatsheet`.
    pub name: String,
    pub title: String,
    /// Newest first. The Zeal API has `null`s in some lists; they're dropped.
    #[serde(default, deserialize_with = "non_null")]
    pub versions: Vec<String>,
    #[serde(default)]
    pub size: u64,
}

impl ZealDoc {
    pub fn id(&self) -> String {
        format!("{ID_PREFIX}{}", self.name)
    }

    pub fn latest_version(&self) -> &str {
        self.versions.first().map_or("", String::as_str)
    }
}

fn non_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let items: Option<Vec<Option<String>>> = Deserialize::deserialize(d)?;
    Ok(items.into_iter().flatten().flatten().collect())
}

pub fn catalog() -> Result<Vec<ZealDoc>> {
    let res = crate::net::client(Some(std::time::Duration::from_secs(60)))?
        .get(CATALOG_URL)
        .send()?
        .error_for_status()?;
    // Icons are dropped here (unknown fields), which keeps the cache small.
    Ok(serde_json::from_reader(res)?)
}

/// Streams the docset tarball (the latest, or a specific `version`) to `dest`,
/// reporting `(bytes, total)` as it goes.
pub fn download(
    name: &str,
    version: Option<&str>,
    dest: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<()> {
    let url = format!("{DOWNLOAD_BASE}/{name}/{}", version.unwrap_or("latest"));
    let mut res = crate::net::client(None)?
        .get(&url)
        .send()?
        .error_for_status()?;
    let total = res.content_length();
    let mut out = File::create(dest)?;
    let mut buf = vec![0; 256 * 1024];
    let mut bytes = 0;
    loop {
        let n = res.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        bytes += n as u64;
        progress(bytes, total);
    }
    out.sync_all()?;
    Ok(())
}

/// Extracts a docset tarball into `dest` and returns the `*.docset` directory.
///
/// Paths are made portable (see `portable_path`) so archives built on macOS
/// (e.g. `127.0.0.1:3000/…`) extract on Windows too. Entries that would escape
/// `dest`, and symlinks/hardlinks, are skipped.
pub fn extract(tgz: &Path, dest: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest)?;
    let gz = flate2::read::GzDecoder::new(BufReader::new(File::open(tgz)?));
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            continue;
        }
        let Some(rel) = portable_path(&entry.path()?) else {
            continue;
        };
        let target = dest.join(rel);
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(&target)?;
        }
    }
    find_docset_dir(dest)
}

/// A relative path with every component valid on all platforms (characters
/// Windows forbids become `_`), or `None` if it isn't a plain relative path.
pub fn portable_path(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(part) => {
                let part = dir_name(&part.to_string_lossy());
                let part = part.trim_end_matches(['.', ' ']);
                if part.is_empty() {
                    return None;
                }
                out.push(part);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// The single `*.docset` directory inside `dir`.
pub fn find_docset_dir(dir: &Path) -> Result<PathBuf> {
    std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.is_dir() && p.extension().is_some_and(|e| e == "docset"))
        .with_context(|| format!("no .docset directory in {}", dir.display()))
}

pub fn documents_dir(docset_dir: &Path) -> PathBuf {
    docset_dir.join("Contents/Resources/Documents")
}

/// Entries from the docset's `searchIndex` table.
pub fn read_index(docset_dir: &Path) -> Result<Vec<Entry>> {
    let path = docset_dir.join("Contents/Resources/docSet.dsidx");
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening {}", path.display()))?;
    let has_search_index: bool = conn.query_row(
        "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'searchIndex'",
        [],
        |r| r.get(0),
    )?;
    if !has_search_index {
        // Apple-style (Core Data) indexes aren't supported yet.
        bail!("unsupported docset index format (no searchIndex table)");
    }
    let mut stmt = conn.prepare("SELECT name, type, path FROM searchIndex")?;
    let rows = stmt.query_map([], |r| {
        Ok(Entry {
            name: r.get(0)?,
            kind: r.get(1)?,
            path: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Resolves a page path (no fragment) under `root`, refusing anything that
/// would escape it. Dash paths are sometimes percent-encoded.
pub fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let candidates = [
        path.to_string(),
        percent_encoding::percent_decode_str(path)
            .decode_utf8_lossy()
            .into_owned(),
    ];
    candidates.into_iter().find_map(|p| {
        let rel = Path::new(&p);
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return None;
        }
        // Same mapping as `extract`.
        let full = root.join(portable_path(rel)?);
        full.is_file().then_some(full)
    })
}

/// A directory name safe on every platform for a Zeal feed name.
pub fn dir_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_refuses_escapes_and_decodes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a b")).unwrap();
        std::fs::write(dir.path().join("a b/p.html"), "x").unwrap();

        assert!(resolve(dir.path(), "a b/p.html").is_some());
        assert!(resolve(dir.path(), "a%20b/p.html").is_some());
        assert!(resolve(dir.path(), "../secret.txt").is_none());
        assert!(resolve(dir.path(), "a b/../../secret.txt").is_none());
        assert!(resolve(dir.path(), "/etc/passwd").is_none());
        assert!(
            resolve(dir.path(), "a b").is_none(),
            "directories aren't pages"
        );
    }

    #[test]
    fn catalog_versions_skip_nulls() {
        let doc: ZealDoc = serde_json::from_str(
            r#"{"name":"X","title":"X","versions":["3.0.0",null,"2.9.0"],"size":1,"icon":"…"}"#,
        )
        .unwrap();
        assert_eq!(doc.versions, ["3.0.0", "2.9.0"]);
        let doc: ZealDoc =
            serde_json::from_str(r#"{"name":"Y","title":"Y","versions":null}"#).unwrap();
        assert!(doc.versions.is_empty());
    }

    #[test]
    fn archive_paths_become_portable() {
        let p = |s: &str| portable_path(Path::new(s));
        assert_eq!(
            p("Docs/127.0.0.1:3000/index.html"),
            Some(PathBuf::from("Docs/127.0.0.1_3000/index.html"))
        );
        assert_eq!(p("./a/b"), Some(PathBuf::from("a/b")));
        assert_eq!(p("../escape"), None);
        assert_eq!(p("/abs"), None);

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("127.0.0.1_3000")).unwrap();
        std::fs::write(dir.path().join("127.0.0.1_3000/p.html"), "x").unwrap();
        assert!(resolve(dir.path(), "127.0.0.1:3000/p.html").is_some());
    }

    #[test]
    fn extract_skips_links_and_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let tgz = dir.path().join("t.tgz");
        {
            let gz = flate2::write::GzEncoder::new(
                File::create(&tgz).unwrap(),
                flate2::Compression::fast(),
            );
            let mut b = tar::Builder::new(gz);
            let mut add = |path: &str, kind: tar::EntryType, data: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(kind);
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                if kind.is_symlink() {
                    h.set_link_name("/etc").unwrap();
                }
                // `append_data` would reject `..`; write the raw name to simulate a hostile archive.
                h.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path.as_bytes());
                h.set_cksum();
                b.append(&h, data).unwrap();
            };
            add("X.docset/Contents/a:b.html", tar::EntryType::Regular, b"ok");
            add("X.docset/link", tar::EntryType::Symlink, b"");
            add("../evil.txt", tar::EntryType::Regular, b"no");
            b.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.path().join("out");
        let docset = extract(&tgz, &out).unwrap();
        assert!(docset.join("Contents/a_b.html").is_file());
        assert!(!docset.join("link").exists());
        assert!(!dir.path().join("evil.txt").exists());
    }

    #[test]
    fn dir_names_are_portable() {
        assert_eq!(dir_name("Android_(Kotlin)"), "Android_(Kotlin)");
        assert_eq!(dir_name("a:b/c"), "a_b_c");
    }
}
