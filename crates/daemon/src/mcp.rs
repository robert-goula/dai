//! MCP tools for agents. Output is compact text/markdown rather than JSON to
//! keep token use down.

use std::fmt::Write;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use dai_core::snippets::SnippetInput;

use std::path::PathBuf;

use dai_core::index::Hit;
use dai_core::project::{Dependency, ProjectReport};

use crate::backend::{Backend, OpenOutcome};

const MAX_LIMIT: usize = 50;

#[derive(Clone)]
pub struct DaiMcp {
    backend: Backend,
}

impl DaiMcp {
    pub fn new(backend: Backend) -> Self {
        Self { backend }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// A symbol name (`useEffect`, `Vec::push`, `os.path.join`) or a few keywords.
    query: String,
    /// Restrict to these docset ids (see list_docsets). Empty searches all.
    #[serde(default)]
    docsets: Vec<String>,
    /// Max results (default 10, max 50).
    limit: Option<usize>,
    /// Absolute path of the project you're working in. Results then prefer
    /// docs for the versions in its package.json / Cargo.toml / go.mod /
    /// pyproject.toml, and version mismatches are flagged.
    project_path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProjectArgs {
    /// Absolute path of the project (or any folder inside it).
    project_path: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetDocArgs {
    /// Docset id from a search hit.
    docset: String,
    /// Page path from a search hit (a `#anchor` suffix is fine).
    path: String,
    /// Character offset to continue a long page from (use `next_offset`).
    offset: Option<usize>,
    /// Max characters to return (default 20000).
    max_chars: Option<usize>,
}

#[tool_router]
impl DaiMcp {
    #[tool(description = "List installed docsets (id, name, version).")]
    async fn list_docsets(&self) -> Result<CallToolResult, ErrorData> {
        let docsets = match self.backend.docsets().await {
            Ok(d) => d,
            Err(e) => return Ok(tool_error(e)),
        };
        if docsets.is_empty() {
            return Ok(text(
                "No docsets installed. The user can install some with `dai install <slug>`.",
            ));
        }
        let mut out = String::new();
        for d in docsets {
            let _ = writeln!(out, "{}: {} {}", d.id, d.name, d.version);
        }
        Ok(text(out))
    }

    #[tool(
        description = "Search installed documentation. Returns ranked hits with docset and path \
                          for get_doc. Symbol names rank named API entries first."
    )]
    async fn search_docs(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        if args.query.trim().is_empty() {
            return Err(ErrorData::invalid_params("query must not be empty", None));
        }
        let limit = args.limit.unwrap_or(10).clamp(1, MAX_LIMIT);
        let project = args.project_path.map(PathBuf::from);
        let search = self
            .backend
            .search(args.query.clone(), args.docsets, project.clone(), limit);
        let report = async {
            match &project {
                Some(p) => self.backend.project(p.clone()).await.ok(),
                None => None,
            }
        };
        let (hits, report) = tokio::join!(search, report);
        let hits = match hits {
            Ok(h) => h,
            Err(e) => return Ok(tool_error(e)),
        };
        let mut out = report.map(|r| version_notes(&r, &hits)).unwrap_or_default();
        if hits.is_empty() {
            out.push_str(&format!("No results for `{}`.", args.query));
            return Ok(text(out));
        }
        for (i, h) in hits.iter().enumerate() {
            let label = if h.kind == "entry" {
                format!("[{}]", h.entry_type)
            } else {
                h.heading.clone()
            };
            let _ = writeln!(
                out,
                "{}. {} — {}  (docset: {}, path: {})",
                i + 1,
                h.name,
                label,
                h.docset,
                h.path
            );
            if !h.snippet.is_empty() {
                let _ = writeln!(
                    out,
                    "   {}",
                    h.snippet.split_whitespace().collect::<Vec<_>>().join(" ")
                );
            }
        }
        Ok(text(out))
    }

    #[tool(
        description = "Match a project's dependencies (package.json, Cargo.toml, go.mod, \
                          pyproject.toml, requirements.txt) and language version to installed \
                          docsets, flag version mismatches, and suggest docsets to install."
    )]
    async fn resolve_project_versions(
        &self,
        Parameters(args): Parameters<ProjectArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = match self
            .backend
            .project(PathBuf::from(&args.project_path))
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        Ok(text(project_summary(&report)))
    }

    #[tool(
        description = "Read a documentation page as markdown. Long pages are paged: pass the \
                          returned next_offset as offset to continue."
    )]
    async fn get_doc(
        &self,
        Parameters(args): Parameters<GetDocArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let res = self
            .backend
            .get_doc(
                args.docset.clone(),
                args.path.clone(),
                args.offset.unwrap_or(0),
                args.max_chars,
            )
            .await;
        let page = match res {
            Ok(Some(p)) => p,
            Ok(None) => {
                return Ok(tool_error(format!(
                    "no page `{}` in `{}`",
                    args.path, args.docset
                )));
            }
            Err(e) => return Ok(tool_error(e)),
        };
        let end = page.next_offset.unwrap_or(page.total_chars);
        let mut out = format!("<!-- {} {}", page.docset, page.path);
        if !page.url.is_empty() {
            let _ = write!(out, " | source: {}", page.url);
        }
        let _ = write!(
            out,
            " | chars {}-{} of {}",
            page.offset, end, page.total_chars
        );
        if let Some(next) = page.next_offset {
            let _ = write!(out, " | next_offset: {next}");
        }
        out.push_str(" -->\n\n");
        out.push_str(&page.markdown);
        Ok(text(out))
    }

    #[tool(
        description = "Show a documentation page to the user in the DAI desktop app. Only use \
                          this when the user asks to see or open docs; it takes over their screen."
    )]
    async fn open_in_app(
        &self,
        Parameters(args): Parameters<OpenArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.backend.open(args.docset, args.path).await {
            Ok(OpenOutcome::Shown) => text("Opened in the DAI app."),
            Ok(OpenOutcome::Launched) => text(
                "The DAI app wasn't running; asked the OS to launch it at that page. \
                 If nothing appears, the app isn't installed.",
            ),
            Err(e) => tool_error(e),
        })
    }

    #[tool(
        description = "Search the user's saved code snippets (their preferred patterns and \
                          reference code). Returns matching snippets with their code. An empty \
                          query lists the most recently updated."
    )]
    async fn search_snippets(
        &self,
        Parameters(args): Parameters<SnippetSearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let limit = args.limit.unwrap_or(5).clamp(1, MAX_LIMIT);
        let query = args.query.unwrap_or_default();
        let found = match self
            .backend
            .snippets(query.clone(), args.language, args.tag, limit)
            .await
        {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(e)),
        };
        if found.is_empty() {
            return Ok(text(format!("No snippets match `{query}`.")));
        }
        let mut out = String::new();
        for s in found {
            let _ = writeln!(out, "## {} (id: {})", s.title, s.id);
            let mut meta = vec![];
            if !s.language.is_empty() {
                meta.push(format!("language: {}", s.language));
            }
            if !s.tags.is_empty() {
                meta.push(format!("tags: {}", s.tags.join(", ")));
            }
            if !meta.is_empty() {
                let _ = writeln!(out, "{}", meta.join(" | "));
            }
            if !s.description.is_empty() {
                let _ = writeln!(out, "{}", s.description);
            }
            let code: String = s.code.chars().take(SNIPPET_PREVIEW_CHARS).collect();
            let more = if code.len() < s.code.len() {
                "\n… (truncated; use get_snippet)"
            } else {
                ""
            };
            let _ = writeln!(out, "```{}\n{code}\n```{more}\n", s.language);
        }
        Ok(text(out))
    }

    #[tool(description = "Read one saved snippet in full (code and notes) by id.")]
    async fn get_snippet(
        &self,
        Parameters(args): Parameters<SnippetIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(match self.backend.snippet(args.id.clone()).await {
            Ok(Some(s)) => text(dai_core::snippets::render(&s)),
            Ok(None) => tool_error(format!("no snippet `{}`", args.id)),
            Err(e) => tool_error(e),
        })
    }

    #[tool(
        description = "Save a new code snippet to the user's snippet library. Only use this \
                          when the user asks to save or remember a snippet. Never overwrites an \
                          existing snippet."
    )]
    async fn save_snippet(
        &self,
        Parameters(args): Parameters<SaveSnippetArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let input = SnippetInput {
            title: args.title,
            language: args.language,
            tags: args.tags,
            description: args.description,
            code: args.code,
            notes: args.notes,
        };
        Ok(match self.backend.create_snippet(input).await {
            Ok(s) => text(format!("Saved snippet `{}` ({}.md).", s.title, s.id)),
            Err(e) => tool_error(e),
        })
    }
}

const SNIPPET_PREVIEW_CHARS: usize = 1500;

fn dep_label(d: &Dependency) -> String {
    match (&d.resolved, d.spec.is_empty()) {
        (Some(r), _) => format!("{} {r}", d.name),
        (None, false) => format!("{} {}", d.name, d.spec),
        (None, true) => d.name.clone(),
    }
}

/// Lines explaining version choices relevant to these hits: which pinned
/// docset was used, and which results are for a different version.
fn version_notes(report: &ProjectReport, hits: &[Hit]) -> String {
    let mut out = String::new();
    for d in &report.covered {
        let Some(best) = d.installed.first() else {
            continue;
        };
        let in_hits = |id: &str| hits.iter().any(|h| h.docset == id);
        match best.matches {
            // Only mention docsets these results came from, so unrelated
            // dependencies (e.g. the runtime) don't add noise to every search.
            Some(true)
                if in_hits(&best.id) && d.installed.iter().any(|m| m.matches == Some(false)) =>
            {
                let _ = writeln!(
                    out,
                    "Using {} for {} (other versions skipped).",
                    best.id,
                    dep_label(&d.dependency)
                );
            }
            Some(false) if d.installed.iter().any(|m| in_hits(&m.id)) => {
                let have: Vec<String> = d
                    .installed
                    .iter()
                    .map(|m| format!("{} ({})", m.id, m.version))
                    .collect();
                let fix = report
                    .suggestions
                    .iter()
                    .find(|s| s.dependency == d.dependency.name)
                    .map_or("check other sources".to_string(), |s| {
                        format!("the user can run `dai install {}`", s.id)
                    });
                let _ = writeln!(
                    out,
                    "Warning: project uses {}, but installed docs are {}; {fix}.",
                    dep_label(&d.dependency),
                    have.join(", ")
                );
            }
            _ => {}
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

pub fn project_summary(r: &ProjectReport) -> String {
    let mut out = format!(
        "Project: {}\n\nCovered by installed docs:\n",
        r.root.display()
    );
    if r.covered.is_empty() {
        out.push_str("- (none)\n");
    }
    for d in &r.covered {
        let docs: Vec<String> = d
            .installed
            .iter()
            .map(|m| {
                let mark = match m.matches {
                    Some(true) => "matches",
                    Some(false) => "different version",
                    None => "version unknown",
                };
                format!("{} ({}, {mark})", m.id, m.version)
            })
            .collect();
        let _ = writeln!(out, "- {}: {}", dep_label(&d.dependency), docs.join("; "));
    }
    if !r.suggestions.is_empty() {
        out.push_str("\nAvailable to install (the user can run `dai install <id>`):\n");
        for s in &r.suggestions {
            let _ = writeln!(out, "- {} → {} ({})", s.dependency, s.id, s.version);
        }
    }
    let suggested: Vec<&str> = r
        .suggestions
        .iter()
        .map(|s| s.dependency.as_str())
        .collect();
    let rest: Vec<String> = r
        .uncovered
        .iter()
        .filter(|d| d.ecosystem != "language" && !suggested.contains(&d.name.as_str()))
        .map(dep_label)
        .collect();
    if !rest.is_empty() {
        let _ = writeln!(
            out,
            "\nNo docset found ({}): {}",
            rest.len(),
            rest.join(", ")
        );
    }
    out
}

#[derive(Deserialize, JsonSchema)]
pub struct SnippetSearchArgs {
    /// Keywords matched against title, tags, description, and code. Omit to list recent ones.
    query: Option<String>,
    /// Only snippets in this language (e.g. `rust`, `tsx`).
    language: Option<String>,
    /// Only snippets with this tag.
    tag: Option<String>,
    /// Max results (default 5).
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SnippetIdArgs {
    /// Snippet id from search_snippets.
    id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SaveSnippetArgs {
    /// Short descriptive title; also becomes the file name.
    title: String,
    /// The code itself, without markdown fences.
    code: String,
    /// Language for highlighting, e.g. `rust`, `typescript`, `bash`.
    language: String,
    /// One-line summary of what it does and when to use it.
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    /// Optional markdown notes (caveats, links).
    #[serde(default)]
    notes: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct OpenArgs {
    /// Docset id from a search hit.
    docset: String,
    /// Page path from a search hit (a `#anchor` suffix scrolls to that section).
    path: String,
}

#[tool_handler(
    name = "dai",
    instructions = "Local, offline documentation for installed docsets (languages, \
frameworks, libraries). Use search_docs with a symbol (e.g. `useEffect`, `Vec::push`) or a few \
keywords, then get_doc with a hit's docset and path to read the page as markdown. Use \
list_docsets to see what's installed; for anything not installed, use other sources. \
Pass project_path (the project you're working in) to search_docs so results match the project's \
dependency versions; resolve_project_versions shows the full mapping. \
The user also keeps reference code snippets: check search_snippets for their preferred patterns \
before writing common code."
)]
impl ServerHandler for DaiMcp {}

fn text(s: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(s.into())])
}

fn tool_error(e: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(e.to_string())])
}

/// `dai mcp`: MCP over stdio, backed by the daemon (started if needed).
/// Nothing else may write to stdout in this mode.
pub async fn serve_stdio(home: &std::path::Path) -> anyhow::Result<()> {
    use rmcp::ServiceExt;
    let client = crate::client::Client::connect(home).await?;
    let service = DaiMcp::new(Backend::Remote(client))
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
