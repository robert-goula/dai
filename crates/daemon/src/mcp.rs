//! MCP tools for agents. Output is compact text/markdown rather than JSON to
//! keep token use down.

use std::fmt::Write;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

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
        let hits = match self
            .backend
            .search(args.query.clone(), args.docsets, limit)
            .await
        {
            Ok(h) => h,
            Err(e) => return Ok(tool_error(e)),
        };
        if hits.is_empty() {
            return Ok(text(format!("No results for `{}`.", args.query)));
        }
        let mut out = String::new();
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
        let mut out = format!(
            "<!-- {} {} | source: {} | chars {}-{} of {}",
            page.docset, page.path, page.url, page.offset, end, page.total_chars
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
list_docsets to see what's installed; for anything not installed, use other sources."
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
