//! Project awareness: read a project's manifests, then match its dependencies
//! (and language/runtime) to installed docsets by name and version.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::library::CatalogEntry;
use crate::store::Docset;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dependency {
    /// `npm`, `cargo`, `go`, `pypi`, or `language`.
    pub ecosystem: String,
    pub name: String,
    /// As written in the manifest, e.g. `^18.2.0`. May be empty.
    pub spec: String,
    /// Exact version from a lockfile or `node_modules`, when available.
    pub resolved: Option<String>,
}

impl Dependency {
    /// The most precise version known: resolved, else the spec's numbers.
    pub fn version(&self) -> Option<Vec<u64>> {
        self.resolved
            .as_deref()
            .and_then(parse_version)
            .or_else(|| parse_version(&self.spec))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocsetMatch {
    pub id: String,
    pub version: String,
    /// `Some(true)` if the docset covers the project's version, `Some(false)` if
    /// it's a different version, `None` if unknown (e.g. generated docsets).
    pub matches: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepReport {
    pub dependency: Dependency,
    /// Installed docsets for this dependency, version matches first.
    pub installed: Vec<DocsetMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectReport {
    /// Folder the manifests were read from.
    pub root: PathBuf,
    /// Dependencies with at least one installed docset.
    pub covered: Vec<DepReport>,
    /// Dependencies with no installed docset.
    pub uncovered: Vec<Dependency>,
    /// Catalog docsets that would cover what's missing (see `suggest`).
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Dependency name.
    pub dependency: String,
    /// Docset id to install, e.g. `react~18` or `dash:React@18.3.1`.
    pub id: String,
    pub version: String,
}

impl ProjectReport {
    /// Installed docsets to leave out of searches: other versions of a
    /// dependency when a matching version is installed.
    pub fn excluded_docsets(&self) -> Vec<String> {
        let mut out = Vec::new();
        for d in &self.covered {
            if d.installed.iter().any(|m| m.matches == Some(true)) {
                out.extend(
                    d.installed
                        .iter()
                        .filter(|m| m.matches == Some(false))
                        .map(|m| m.id.clone()),
                );
            }
        }
        out
    }
}

/// Reads manifests from `path` or its nearest ancestor that has any, and
/// matches them against `installed`.
pub fn analyze(path: &Path, installed: &[Docset]) -> Result<ProjectReport> {
    let (root, deps) = find_dependencies(path)?;
    let mut covered = Vec::new();
    let mut uncovered = Vec::new();
    for dep in deps {
        let keys = dependency_keys(&dep);
        let version = dep.version();
        let mut matches: Vec<DocsetMatch> = installed
            .iter()
            .filter(|ds| docset_key(&ds.id).is_some_and(|k| keys.contains(&k)))
            .map(|ds| DocsetMatch {
                id: ds.id.clone(),
                version: ds.version.clone(),
                matches: version.as_deref().and_then(|v| version_matches(v, ds)),
            })
            .collect();
        if matches.is_empty() {
            uncovered.push(dep);
        } else {
            matches.sort_by_key(|m| match m.matches {
                Some(true) => 0,
                None => 1,
                Some(false) => 2,
            });
            covered.push(DepReport {
                dependency: dep,
                installed: matches,
            });
        }
    }
    Ok(ProjectReport {
        root,
        covered,
        uncovered,
        suggestions: vec![],
    })
}

/// Catalog docsets for dependencies that have no installed docs, or only
/// other versions: a version match when the catalog has one (DevDocs pins
/// first, then Dash versions), else the latest.
pub fn suggest(report: &ProjectReport, catalog: &[CatalogEntry]) -> Vec<Suggestion> {
    let needs_docs = report
        .covered
        .iter()
        .filter(|d| !d.installed.iter().any(|m| m.matches != Some(false)))
        .map(|d| &d.dependency)
        .chain(&report.uncovered);
    let mut out = Vec::new();
    for dep in needs_docs {
        let keys = dependency_keys(dep);
        let candidates: Vec<&CatalogEntry> = catalog
            .iter()
            .filter(|c| docset_key(&c.id).is_some_and(|k| keys.contains(&k)))
            .collect();
        let Some(latest) = candidates.first() else {
            continue;
        };
        let pick = |id: String, version: &str| Suggestion {
            dependency: dep.name.clone(),
            id,
            version: version.to_string(),
        };
        let Some(version) = dep.version() else {
            out.push(pick(latest.id.clone(), &latest.version));
            continue;
        };
        let fake = |id: &str, v: &str| Docset {
            id: id.into(),
            name: String::new(),
            source: String::new(),
            version: v.into(),
            release: String::new(),
            mtime: 0,
            installed_at: 0,
        };
        let exact = candidates
            .iter()
            .find(|c| version_matches(&version, &fake(&c.id, &c.version)) == Some(true));
        let older_dash = || {
            candidates.iter().find_map(|c| {
                c.versions.iter().find_map(|v| {
                    let id = format!("{}@{v}", c.id);
                    (version_matches(&version, &fake(&id, v)) == Some(true)).then(|| pick(id, v))
                })
            })
        };
        match exact {
            Some(c) => out.push(pick(c.id.clone(), &c.version)),
            None => {
                out.push(older_dash().unwrap_or_else(|| pick(latest.id.clone(), &latest.version)))
            }
        }
    }
    out
}

// --- manifests ------------------------------------------------------------------

const MANIFESTS: [&str; 5] = [
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
];

pub fn find_dependencies(path: &Path) -> Result<(PathBuf, Vec<Dependency>)> {
    let start = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let Some(root) = start
        .ancestors()
        .find(|d| MANIFESTS.iter().any(|m| d.join(m).is_file()))
    else {
        anyhow::bail!(
            "no package.json, Cargo.toml, go.mod, pyproject.toml, or requirements.txt in {} or above",
            path.display()
        );
    };
    let mut deps = Vec::new();
    // Manifests are parsed best-effort: one broken file shouldn't hide the rest.
    if let Ok(d) = npm(root) {
        deps.extend(d);
    }
    if let Ok(d) = cargo(root) {
        deps.extend(d);
    }
    if let Ok(d) = go(root) {
        deps.extend(d);
    }
    if let Ok(d) = python(root) {
        deps.extend(d);
    }
    let mut seen = HashSet::new();
    deps.retain(|d| seen.insert((d.ecosystem.clone(), d.name.clone())));
    Ok((root.to_path_buf(), deps))
}

fn dep(ecosystem: &str, name: &str, spec: &str, resolved: Option<String>) -> Dependency {
    Dependency {
        ecosystem: ecosystem.into(),
        name: name.into(),
        spec: spec.trim().into(),
        resolved,
    }
}

fn npm(root: &Path) -> Result<Vec<Dependency>> {
    let text = std::fs::read_to_string(root.join("package.json"))?;
    let pkg: serde_json::Value = serde_json::from_str(&text)?;
    let mut out = Vec::new();
    if let Some(node) = pkg.pointer("/engines/node").and_then(|v| v.as_str()) {
        out.push(dep("language", "node", node, None));
    } else {
        out.push(dep("language", "node", "", None));
    }
    for section in ["dependencies", "devDependencies"] {
        let Some(map) = pkg.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        for (name, spec) in map {
            let resolved =
                std::fs::read_to_string(root.join("node_modules").join(name).join("package.json"))
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .and_then(|v| v.get("version")?.as_str().map(String::from));
            out.push(dep("npm", name, spec.as_str().unwrap_or(""), resolved));
        }
    }
    Ok(out)
}

fn cargo(root: &Path) -> Result<Vec<Dependency>> {
    let manifest: toml::Table = toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml"))?)?;
    let lock = cargo_lock(root);
    let rust_version = manifest
        .get("package")
        .and_then(|p| p.get("rust-version"))
        .or_else(|| {
            manifest
                .get("workspace")?
                .get("package")?
                .get("rust-version")
        })
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| rust_toolchain(root));
    let mut out = vec![dep(
        "language",
        "rust",
        rust_version.as_deref().unwrap_or(""),
        None,
    )];

    let mut tables = vec![manifest.clone()];
    // Workspace members hold most dependencies.
    if let Some(members) = manifest
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        for pattern in members.iter().filter_map(|m| m.as_str()) {
            for dir in expand_member(root, pattern) {
                if let Ok(t) = std::fs::read_to_string(dir.join("Cargo.toml"))
                    .map(|s| toml::from_str::<toml::Table>(&s))
                {
                    tables.extend(t);
                }
            }
        }
    }
    for table in &tables {
        let sections = ["dependencies", "dev-dependencies", "build-dependencies"]
            .into_iter()
            .filter_map(|s| table.get(s))
            .chain(table.get("workspace").and_then(|w| w.get("dependencies")));
        for section in sections.filter_map(|s| s.as_table()) {
            for (key, value) in section {
                // Local path dependencies (workspace crates) aren't documented anywhere.
                if value.get("path").is_some() {
                    continue;
                }
                let name = value.get("package").and_then(|p| p.as_str()).unwrap_or(key);
                let spec = value
                    .as_str()
                    .or_else(|| value.get("version")?.as_str())
                    .unwrap_or("");
                let resolved = lock.get(name).and_then(|vs| pick_locked(vs, spec));
                out.push(dep("cargo", name, spec, resolved));
            }
        }
    }
    Ok(out)
}

fn expand_member(root: &Path, pattern: &str) -> Vec<PathBuf> {
    match pattern.strip_suffix("/*") {
        Some(parent) => std::fs::read_dir(root.join(parent))
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default(),
        None => vec![root.join(pattern)],
    }
}

/// Every locked version of each crate (lockfiles often hold several, e.g. a
/// direct `toml 1.x` next to a transitive `toml 0.8`).
fn cargo_lock(root: &Path) -> BTreeMap<String, Vec<String>> {
    #[derive(Deserialize)]
    struct Lock {
        #[serde(default)]
        package: Vec<LockPackage>,
    }
    #[derive(Deserialize)]
    struct LockPackage {
        name: String,
        version: String,
    }
    let lock = root
        .ancestors()
        .map(|d| d.join("Cargo.lock"))
        .find(|p| p.is_file());
    let parsed = lock
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str::<Lock>(&t).ok());
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in parsed.map(|l| l.package).unwrap_or_default() {
        out.entry(p.name).or_default().push(p.version);
    }
    out
}

/// The highest locked version compatible with `spec` (same major, or same
/// minor for 0.x), else the highest locked version.
fn pick_locked(versions: &[String], spec: &str) -> Option<String> {
    let wanted = parse_version(spec);
    let compatible = |v: &Vec<u64>| match &wanted {
        Some(w) if w.first() == Some(&0) => v.first() == Some(&0) && v.get(1) == w.get(1),
        Some(w) => v.first() == w.first(),
        None => true,
    };
    let mut parsed: Vec<(Vec<u64>, &String)> = versions
        .iter()
        .filter_map(|v| Some((parse_version(v)?, v)))
        .collect();
    parsed.sort();
    parsed
        .iter()
        .rev()
        .find(|(v, _)| compatible(v))
        .or(parsed.last())
        .map(|(_, v)| (*v).clone())
}

fn rust_toolchain(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("rust-toolchain.toml"))
        .or_else(|_| std::fs::read_to_string(root.join("rust-toolchain")))
        .ok()?;
    let channel = toml::from_str::<toml::Table>(&text)
        .ok()
        .and_then(|t| {
            t.get("toolchain")?
                .get("channel")?
                .as_str()
                .map(String::from)
        })
        .unwrap_or_else(|| text.trim().to_string());
    parse_version(&channel).is_some().then_some(channel)
}

fn go(root: &Path) -> Result<Vec<Dependency>> {
    let text = std::fs::read_to_string(root.join("go.mod"))?;
    let mut out = Vec::new();
    let mut in_require = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("go ") {
            out.push(dep("language", "go", v, Some(v.trim().to_string())));
        } else if line == "require (" {
            in_require = true;
        } else if in_require && line == ")" {
            in_require = false;
        } else if let Some(rest) = line.strip_prefix("require ").or(in_require.then_some(line)) {
            if rest.contains("// indirect") {
                continue;
            }
            let mut parts = rest.split_whitespace();
            if let (Some(module), Some(version)) = (parts.next(), parts.next()) {
                out.push(dep("go", module, version, Some(version.to_string())));
            }
        }
    }
    Ok(out)
}

fn python(root: &Path) -> Result<Vec<Dependency>> {
    let mut out = Vec::new();
    if let Ok(text) = std::fs::read_to_string(root.join("pyproject.toml")) {
        let t: toml::Table = toml::from_str(&text)?;
        let project = t.get("project");
        let requires = project
            .and_then(|p| p.get("requires-python"))
            .and_then(|v| v.as_str());
        let poetry = t
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_table());
        let poetry_python = poetry
            .and_then(|d| d.get("python"))
            .and_then(|v| v.as_str());
        out.push(dep(
            "language",
            "python",
            requires.or(poetry_python).unwrap_or(""),
            None,
        ));
        for req in project
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_array())
            .into_iter()
            .flatten()
        {
            if let Some((name, spec)) = req.as_str().and_then(pep508) {
                out.push(dep("pypi", &name, &spec, None));
            }
        }
        for (name, spec) in poetry.into_iter().flatten().filter(|(n, _)| *n != "python") {
            let spec = spec
                .as_str()
                .or_else(|| spec.get("version")?.as_str())
                .unwrap_or("");
            out.push(dep("pypi", name, spec, None));
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("requirements.txt")) {
        if !out.iter().any(|d| d.name == "python") {
            out.push(dep("language", "python", "", None));
        }
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with(['#', '-']))
        {
            if let Some((name, spec)) = pep508(line) {
                out.push(dep("pypi", &name, &spec, None));
            }
        }
    }
    Ok(out)
}

/// `django[bcrypt]>=5.0; python_version>"3.10"` → (`django`, `>=5.0`).
fn pep508(req: &str) -> Option<(String, String)> {
    static REQ: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)\s*(\[[^\]]*\])?\s*([^;#]*)").unwrap()
    });
    let c = REQ.captures(req)?;
    Some((c[1].to_string(), c[3].trim().to_string()))
}

// --- matching ---------------------------------------------------------------------

/// First `1.2.3`-style number in `s` (`^18.2.0` → [18, 2, 0], `v1.21` → [1, 21]).
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    static NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+(\.\d+)*").unwrap());
    let m = NUM.find(s)?;
    m.as_str().split('.').map(|p| p.parse().ok()).collect()
}

/// Lowercase alphanumerics, with a trailing `js` dropped (`NodeJS`, `next.js`)
/// except where it's part of a different product's name.
fn normalize(s: &str) -> String {
    let n: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match n.strip_suffix("js") {
        Some(base) if base.len() >= 3 && n != "angularjs" => base.to_string(),
        _ => n,
    }
}

/// Names a dependency's docs might go by.
fn dependency_keys(dep: &Dependency) -> HashSet<String> {
    let mut keys = HashSet::new();
    let name = dep.name.as_str();
    match dep.ecosystem.as_str() {
        "npm" => {
            if let Some((scope, pkg)) = name.strip_prefix('@').and_then(|n| n.split_once('/')) {
                keys.insert(normalize(&format!("{scope}{pkg}")));
                keys.insert(normalize(pkg));
                // `@tanstack/react-query` is documented as TanStack Query.
                for fw in ["react-", "vue-", "solid-", "svelte-", "angular-", "preact-"] {
                    if let Some(rest) = pkg.strip_prefix(fw) {
                        keys.insert(normalize(&format!("{scope}{rest}")));
                    }
                }
                // `@angular/core`, `@types/node`.
                if pkg == "core" {
                    keys.insert(normalize(scope));
                }
                if scope == "types" {
                    keys.insert(normalize(pkg));
                }
            } else {
                keys.insert(normalize(name));
            }
            if name == "react-dom" {
                keys.insert("react".into());
            }
        }
        "go" => {
            // `github.com/labstack/echo/v4` → `echo`.
            let segments: Vec<&str> = name.split('/').collect();
            let last = segments
                .iter()
                .rev()
                .find(|s| !(s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit())))
                .unwrap_or(&name);
            keys.insert(normalize(last));
        }
        _ => {
            keys.insert(normalize(name));
        }
    }
    keys
}

/// The name part of a docset id, normalized: `react~18` → `react`,
/// `dash:NodeJS` → `node`, `dash:React@18.3.1` → `react`,
/// `md:tanstack-query` → `tanstackquery`. Cheatsheets never match.
fn docset_key(id: &str) -> Option<String> {
    if let Some(dash) = id.strip_prefix(crate::dash::ID_PREFIX) {
        let name = dash.split('@').next().unwrap_or(dash);
        if name.ends_with("_Cheatsheet") {
            return None;
        }
        let name = name.trim_end_matches("_Contrib");
        // Zeal splits Python by major version: `Python_3`.
        let name = name
            .strip_suffix("_3")
            .or_else(|| name.strip_suffix("_2"))
            .unwrap_or(name);
        return Some(normalize(name));
    }
    if let Some(md) = id.strip_prefix(crate::generate::ID_PREFIX) {
        return Some(normalize(md.trim_end_matches("-context7")));
    }
    Some(normalize(id.split('~').next().unwrap_or(id)))
}

/// Whether a docset covers `version`. Pinned docsets (`react~18`,
/// `python~3.12`, `dash:React@18.3.1`) compare as many components as the pin
/// gives (up to major.minor); "latest" docsets compare the major version (and
/// minor for 0.x). Generated docsets aren't compared.
fn version_matches(version: &[u64], ds: &Docset) -> Option<bool> {
    let pin = ds
        .id
        .split_once('~')
        .map(|(_, p)| p.trim_end_matches("_lts"))
        .or_else(|| ds.id.split_once('@').map(|(_, v)| v))
        .and_then(parse_version);
    if ds.id.starts_with(crate::generate::ID_PREFIX) {
        return None;
    }
    let (theirs, depth) = match pin {
        // A full `@18.3.1` pin still only needs the major (or major.minor for 0.x).
        Some(p) if ds.id.contains('@') => {
            let depth = if p.first() == Some(&0) { 2 } else { 1 };
            (p, depth)
        }
        Some(p) => {
            let depth = p.len().min(2);
            (p, depth)
        }
        None => {
            let p = parse_version(&ds.version)?;
            let depth = if p.first() == Some(&0) { 2 } else { 1 };
            (p, depth)
        }
    };
    let depth = depth.min(version.len()).min(theirs.len());
    Some(version[..depth] == theirs[..depth])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ds(id: &str, version: &str) -> Docset {
        Docset {
            id: id.into(),
            name: id.into(),
            source: String::new(),
            version: version.into(),
            release: String::new(),
            mtime: 0,
            installed_at: 0,
        }
    }

    #[test]
    fn versions_parse_from_specs() {
        assert_eq!(parse_version("^18.2.0"), Some(vec![18, 2, 0]));
        assert_eq!(parse_version(">=3.12,<4"), Some(vec![3, 12]));
        assert_eq!(parse_version("v1.21.0"), Some(vec![1, 21, 0]));
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn docset_names_normalize() {
        assert_eq!(docset_key("react~18").as_deref(), Some("react"));
        assert_eq!(docset_key("node~22_lts").as_deref(), Some("node"));
        assert_eq!(docset_key("dash:NodeJS").as_deref(), Some("node"));
        assert_eq!(docset_key("dash:next.js_Contrib").as_deref(), Some("next"));
        assert_eq!(docset_key("nextjs").as_deref(), Some("next"));
        assert_eq!(docset_key("dash:React@18.3.1").as_deref(), Some("react"));
        assert_eq!(docset_key("dash:Python_3").as_deref(), Some("python"));
        assert_eq!(docset_key("angularjs~1.8").as_deref(), Some("angularjs"));
        assert_eq!(
            docset_key("md:tanstack-query-context7").as_deref(),
            Some("tanstackquery")
        );
        assert_eq!(docset_key("dash:Ack_Cheatsheet"), None);
    }

    #[test]
    fn dependency_names_map_to_docset_names() {
        let keys = |eco: &str, name: &str| dependency_keys(&dep(eco, name, "", None));
        assert!(keys("npm", "@tanstack/react-query").contains("tanstackquery"));
        assert!(keys("npm", "@angular/core").contains("angular"));
        assert!(keys("npm", "@types/node").contains("node"));
        assert!(keys("npm", "react-dom").contains("react"));
        assert!(keys("npm", "vue-router").contains("vuerouter"));
        assert_eq!(docset_key("vue_router~4").as_deref(), Some("vuerouter"));
        assert!(keys("go", "github.com/labstack/echo/v4").contains("echo"));
        assert!(keys("pypi", "Django").contains("django"));
    }

    #[test]
    fn version_matching_by_pin_precision() {
        let m = |v: &[u64], id: &str, ver: &str| version_matches(v, &ds(id, ver));
        assert_eq!(m(&[18, 3, 1], "react~18", "18"), Some(true));
        assert_eq!(m(&[18, 3, 1], "react", "19.2"), Some(false));
        assert_eq!(
            m(&[19, 1, 0], "react", "19.2"),
            Some(true),
            "latest docsets compare majors"
        );
        assert_eq!(m(&[3, 12], "python~3.12", "3.12"), Some(true));
        assert_eq!(m(&[3, 11, 4], "python~3.12", "3.12"), Some(false));
        assert_eq!(m(&[22, 1], "node~22_lts", "22 LTS"), Some(true));
        assert_eq!(m(&[18, 2], "dash:React@18.3.1", "18.3.1"), Some(true));
        assert_eq!(
            m(&[0, 4, 1], "tokio", "0.3"),
            Some(false),
            "0.x compares minors"
        );
        assert_eq!(m(&[1, 0], "md:svelte", "2026-09-24"), None);
    }

    #[test]
    fn analyze_a_node_and_python_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"engines":{"node":">=22"},"dependencies":{"react":"^18.2.0","@tanstack/react-query":"^5"},"devDependencies":{"leftpad":"1"}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("node_modules/react")).unwrap();
        std::fs::write(
            root.join("node_modules/react/package.json"),
            r#"{"version":"18.3.1"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("requirements.txt"),
            "# deps\nDjango[argon2]>=5.0 ; python_version > '3.10'\n-r other.txt\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src/components")).unwrap();

        let installed = [
            ds("react", "19.2"),
            ds("react~18", "18"),
            ds("node~22_lts", "22 LTS"),
            ds("django~4.2", "4.2"),
        ];
        // Analyzing from a subfolder finds the manifests above it.
        let report = analyze(&root.join("src/components"), &installed).unwrap();
        assert_eq!(report.root, root);

        let react = report
            .covered
            .iter()
            .find(|d| d.dependency.name == "react")
            .unwrap();
        assert_eq!(react.dependency.resolved.as_deref(), Some("18.3.1"));
        let ids: Vec<_> = react
            .installed
            .iter()
            .map(|m| (m.id.as_str(), m.matches))
            .collect();
        assert_eq!(ids, [("react~18", Some(true)), ("react", Some(false))]);

        let django = report
            .covered
            .iter()
            .find(|d| d.dependency.name == "Django")
            .unwrap();
        assert_eq!(django.installed[0].matches, Some(false));
        assert!(
            report
                .covered
                .iter()
                .any(|d| d.dependency.name == "node" && d.installed[0].matches == Some(true))
        );
        assert!(
            report
                .uncovered
                .iter()
                .any(|d| d.name == "@tanstack/react-query")
        );

        // Only the mismatched React is excluded; Django has no matching version
        // installed, so its docs stay searchable.
        assert_eq!(report.excluded_docsets(), ["react"]);
    }

    #[test]
    fn analyze_a_cargo_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n[workspace.package]\nrust-version = \"1.85\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("crates/app")).unwrap();
        std::fs::write(
            root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\n[dependencies]\ntokio = { version = \"1\", features = [\"full\"] }\nlocal = { path = \"../local\" }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("Cargo.lock"),
            "[[package]]\nname = \"tokio\"\nversion = \"0.2.25\"\n\n[[package]]\nname = \"tokio\"\nversion = \"1.53.1\"\n",
        )
        .unwrap();

        let (_, deps) = find_dependencies(root).unwrap();
        assert!(deps.contains(&dep("language", "rust", "1.85", None)));
        assert!(deps.contains(&dep("cargo", "tokio", "1", Some("1.53.1".into()))));
        assert!(!deps.iter().any(|d| d.name == "local"));
    }

    #[test]
    fn suggestions_prefer_matching_versions() {
        let entry = |id: &str, version: &str, versions: &[&str]| CatalogEntry {
            id: id.into(),
            name: id.into(),
            source: String::new(),
            version: version.into(),
            size: 0,
            mtime: 0,
            versions: versions.iter().map(|v| v.to_string()).collect(),
        };
        let catalog = [
            entry("react", "19.2", &[]),
            entry("react~18", "18", &[]),
            entry("vue~3", "3.5", &[]),
            entry("dash:VueJS", "3.5.0", &["2.7.16", "2.6.0"]),
            entry("dash:Express", "5.1.0", &[]),
        ];
        let report = ProjectReport {
            root: PathBuf::new(),
            covered: vec![],
            uncovered: vec![
                dep("npm", "react", "^18.2.0", None),
                dep("npm", "vue", "^2.7.0", None),
                dep("npm", "express", "", None),
                dep("npm", "leftpad", "1", None),
            ],
            suggestions: vec![],
        };
        let got: Vec<_> = suggest(&report, &catalog)
            .into_iter()
            .map(|s| (s.dependency, s.id))
            .collect();
        assert_eq!(
            got,
            [
                ("react".to_string(), "react~18".to_string()),
                ("vue".to_string(), "dash:VueJS@2.7.16".to_string()),
                ("express".to_string(), "dash:Express".to_string()),
            ]
        );
    }

    #[test]
    fn go_mod_requires() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("go.mod"),
            "module x\n\ngo 1.22\n\nrequire (\n\tgithub.com/labstack/echo/v4 v4.11.4\n\tgolang.org/x/net v0.20.0 // indirect\n)\n",
        )
        .unwrap();
        let (_, deps) = find_dependencies(dir.path()).unwrap();
        let names: Vec<_> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["go", "github.com/labstack/echo/v4"]);
    }
}
