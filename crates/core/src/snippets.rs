//! Code snippets: one markdown file per snippet in a plain folder (so it can
//! be git-synced or edited by hand).
//!
//! ~~~markdown
//! ---
//! title: "Debounce hook"
//! language: "typescript"
//! tags: ["react", "hooks"]
//! description: "Delays a value until it stops changing."
//! created: "2026-09-24T12:00:00Z"
//! updated: "2026-09-24T12:00:00Z"
//! ---
//!
//! ```typescript
//! export function useDebounce…
//! ```
//!
//! Notes in markdown.
//! ~~~

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{RwLock, RwLockReadGuard};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    /// File stem, e.g. `debounce-hook` for `debounce-hook.md`.
    pub id: String,
    pub title: String,
    pub language: String,
    pub tags: Vec<String>,
    pub description: String,
    /// The first fenced code block.
    pub code: String,
    /// Everything else in the body.
    pub notes: String,
    /// RFC 3339, UTC.
    pub created: String,
    pub updated: String,
}

/// What callers provide when creating or editing a snippet.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnippetInput {
    pub title: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub code: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Frontmatter {
    title: String,
    language: String,
    tags: Vec<String>,
    description: String,
    created: String,
    updated: String,
}

/// Parses a snippet file. Files without (valid) frontmatter still load, with
/// the id as the title, so hand-written notes aren't lost.
pub fn parse(id: &str, text: &str) -> Snippet {
    let text = text.replace("\r\n", "\n");
    let (front, body) = split_frontmatter(&text);
    let fm: Frontmatter = front
        .and_then(|f| serde_saphyr::from_str(f).ok())
        .unwrap_or_default();
    let (fence_lang, code, notes) = split_code(body);
    Snippet {
        id: id.to_string(),
        title: if fm.title.is_empty() {
            id.to_string()
        } else {
            fm.title
        },
        language: if fm.language.is_empty() {
            fence_lang
        } else {
            fm.language
        },
        tags: fm.tags,
        description: fm.description,
        code,
        notes,
        created: fm.created,
        updated: fm.updated,
    }
}

pub fn render(s: &Snippet) -> String {
    let q = |v: &str| serde_json::to_string(v).expect("string serializes");
    let longest_run = s.code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_run.max(2) + 1);
    let mut out = format!(
        "---\ntitle: {}\nlanguage: {}\ntags: {}\ndescription: {}\ncreated: {}\nupdated: {}\n---\n\n{fence}{}\n{}\n{fence}\n",
        q(&s.title),
        q(&s.language),
        serde_json::to_string(&s.tags).expect("tags serialize"),
        q(&s.description),
        q(&s.created),
        q(&s.updated),
        s.language,
        s.code.trim_end_matches('\n'),
    );
    if !s.notes.trim().is_empty() {
        out.push('\n');
        out.push_str(s.notes.trim());
        out.push('\n');
    }
    out
}

fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    match rest.find("\n---") {
        Some(end) => {
            let after = &rest[end + 4..];
            (
                Some(&rest[..end]),
                after.strip_prefix('\n').unwrap_or(after),
            )
        }
        None => (None, text),
    }
}

/// Splits out the first fenced code block: `(info-string language, code, notes)`.
fn split_code(body: &str) -> (String, String, String) {
    let lines: Vec<&str> = body.lines().collect();
    let Some((start, fence_char, fence_len, lang)) = lines.iter().enumerate().find_map(|(i, l)| {
        let t = l.trim_start();
        let ch = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let n = t.chars().take_while(|c| *c == ch).count();
        (n >= 3).then(|| (i, ch, n, t[n..].trim().to_string()))
    }) else {
        return (String::new(), String::new(), body.trim().to_string());
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| {
            let t = l.trim();
            t.len() >= fence_len && t.chars().all(|c| c == fence_char)
        })
        .map_or(lines.len(), |p| start + 1 + p);
    let code = lines[start + 1..end].join("\n");
    let notes = [&lines[..start], lines.get(end + 1..).unwrap_or(&[])]
        .concat()
        .join("\n")
        .trim()
        .to_string();
    (lang, code, notes)
}

/// Lowercase, dash-separated file stem from a title.
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(60).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "snippet".into()
    } else {
        slug
    }
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// The snippets folder, mirrored in memory.
pub struct SnippetStore {
    dir: PathBuf,
    snippets: RwLock<BTreeMap<String, Snippet>>,
}

impl SnippetStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let store = Self {
            dir: dir.to_path_buf(),
            snippets: RwLock::default(),
        };
        store.reload()?;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Re-reads every `*.md` file in the folder (not recursive).
    pub fn reload(&self) -> Result<()> {
        let mut map = BTreeMap::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "md") || !path.is_file() {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            map.insert(id.to_string(), parse(id, &text));
        }
        *self.snippets.write().unwrap_or_else(|e| e.into_inner()) = map;
        Ok(())
    }

    pub fn all(&self) -> RwLockReadGuard<'_, BTreeMap<String, Snippet>> {
        self.snippets.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, id: &str) -> Option<Snippet> {
        self.all().get(id).cloned()
    }

    /// Writes a new file, never replacing an existing one (`-2`, `-3`, … suffixes).
    pub fn create(&self, input: SnippetInput) -> Result<Snippet> {
        validate(&input)?;
        let base = slugify(&input.title);
        let id = (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|id| !self.path(id).exists())
            .expect("unbounded range");
        let ts = now();
        let snippet = from_input(id, input, ts.clone(), ts);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path(&snippet.id));
        std::io::Write::write_all(&mut file?, render(&snippet).as_bytes())?;
        self.reload()?;
        Ok(snippet)
    }

    pub fn update(&self, id: &str, input: SnippetInput) -> Result<Snippet> {
        validate(&input)?;
        let existing = self.get(id).with_context(|| format!("no snippet `{id}`"))?;
        let created = if existing.created.is_empty() {
            now()
        } else {
            existing.created
        };
        let snippet = from_input(id.to_string(), input, created, now());
        std::fs::write(self.path(id), render(&snippet))?;
        self.reload()?;
        Ok(snippet)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        if self.get(id).is_none() {
            return Ok(false);
        }
        std::fs::remove_file(self.path(id))?;
        self.reload()?;
        Ok(true)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }
}

fn validate(input: &SnippetInput) -> Result<()> {
    if input.title.trim().is_empty() {
        bail!("a snippet needs a title");
    }
    if input.code.trim().is_empty() {
        bail!("a snippet needs code");
    }
    Ok(())
}

fn from_input(id: String, i: SnippetInput, created: String, updated: String) -> Snippet {
    Snippet {
        id,
        title: i.title.trim().to_string(),
        language: i.language.trim().to_lowercase(),
        tags: i
            .tags
            .iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect(),
        description: i.description.trim().to_string(),
        code: i.code,
        notes: i.notes,
        created,
        updated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(title: &str, code: &str) -> SnippetInput {
        SnippetInput {
            title: title.into(),
            language: "TypeScript".into(),
            tags: vec!["React".into(), " hooks ".into()],
            description: "Delays a value.".into(),
            code: code.into(),
            notes: "Use with *care*.".into(),
        }
    }

    #[test]
    fn render_parse_round_trip() {
        let s = from_input(
            "x".into(),
            input("Debounce: \"hook\"", "const a = 1;\n"),
            "c".into(),
            "u".into(),
        );
        let parsed = parse("x", &render(&s));
        assert_eq!(parsed.title, "Debounce: \"hook\"");
        assert_eq!(parsed.language, "typescript");
        assert_eq!(parsed.tags, ["react", "hooks"]);
        assert_eq!(parsed.code, "const a = 1;");
        assert_eq!(parsed.notes, "Use with *care*.");
        assert_eq!(
            (parsed.created.as_str(), parsed.updated.as_str()),
            ("c", "u")
        );
    }

    #[test]
    fn code_containing_fences_gets_a_longer_fence() {
        let code = "```js\nx\n```";
        let s = from_input("x".into(), input("t", code), String::new(), String::new());
        assert_eq!(parse("x", &render(&s)).code, code);
    }

    #[test]
    fn hand_written_files_still_load() {
        let s = parse("quick-note", "Some notes\n\n```sh\nls -la\n```\nafter");
        assert_eq!(s.title, "quick-note");
        assert_eq!(s.language, "sh");
        assert_eq!(s.code, "ls -la");
        assert_eq!(s.notes, "Some notes\n\nafter");

        let bad_yaml = parse("b", "---\ntitle: [unclosed\n---\n```\ncode\n```");
        assert_eq!(
            (bad_yaml.title.as_str(), bad_yaml.code.as_str()),
            ("b", "code")
        );
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Debounce Hook (React)!"), "debounce-hook-react");
        assert_eq!(slugify("???"), "snippet");
    }

    #[test]
    fn create_never_overwrites_and_update_keeps_created() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnippetStore::open(dir.path()).unwrap();
        let a = store.create(input("Same title", "a")).unwrap();
        let b = store.create(input("Same title", "b")).unwrap();
        assert_eq!(
            (a.id.as_str(), b.id.as_str()),
            ("same-title", "same-title-2")
        );
        assert_eq!(store.all().len(), 2);

        let updated = store.update("same-title", input("Renamed", "a2")).unwrap();
        assert_eq!(updated.id, "same-title", "ids are stable across renames");
        assert_eq!(updated.created, a.created);
        assert_eq!(store.get("same-title").unwrap().code, "a2");

        assert!(store.create(input("  ", "x")).is_err());
        assert!(store.delete("same-title-2").unwrap());
        assert!(!store.delete("same-title-2").unwrap());
    }
}
