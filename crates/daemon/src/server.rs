//! `dai serve`: HTTP API under `/api`, MCP (streamable HTTP) at `/mcp`.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dai_core::project::ProjectReport;
use dai_core::snippets::{Snippet, SnippetInput, slugify};
use dai_core::store::Docset;
use dai_core::{Library, Progress};
use dai_core::{generate, markdown};
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::backend::{Backend, Local, OpenOutcome, blocking};
use crate::mcp::DaiMcp;
use crate::{DaemonInfo, DaiEvent, daemon_info_path, viewer};

#[derive(Serialize, Deserialize)]
pub struct Health {
    pub app: String,
    pub version: String,
}

#[derive(Clone)]
struct AppState {
    local: Arc<Local>,
    token: Arc<str>,
    shutdown: CancellationToken,
}

pub async fn serve(home: &Path, port: u16) -> Result<()> {
    let local = Arc::new(Local::new(Library::open(
        home,
        &dai_core::paths::snippets_dir()?,
    )?));
    let _watcher = watch_snippets(local.clone())?;
    let shutdown = CancellationToken::new();
    let state = AppState {
        local: local.clone(),
        token: crate::token(home)?.into(),
        shutdown: shutdown.clone(),
    };

    let mut mcp_config = StreamableHttpServerConfig::default();
    mcp_config.cancellation_token = shutdown.child_token();
    let mcp = StreamableHttpService::new(
        move || Ok(DaiMcp::new(Backend::Local(local.clone()))),
        Arc::new(LocalSessionManager::default()),
        mcp_config,
    );

    let protected = Router::new()
        .route("/api/catalog", get(catalog))
        .route("/api/docsets", get(docsets))
        .route("/api/docsets/{id}", post(install).delete(remove))
        .route("/api/outdated", get(outdated))
        .route("/api/search", get(search))
        .route("/api/doc", get(get_doc))
        .route("/api/project", get(project))
        .route("/api/generate", post(generate_docset))
        .route("/api/context7/libraries", get(context7_libraries))
        .route("/api/context7/docs", get(context7_docs))
        .route("/api/snippets", get(list_snippets).post(create_snippet))
        .route(
            "/api/snippets/{id}",
            get(get_snippet).put(update_snippet).delete(delete_snippet),
        )
        .route("/api/events", get(events))
        .route("/api/open", post(open))
        .route("/api/shutdown", post(shutdown_handler))
        .nest_service("/mcp", mcp)
        .layer(middleware::from_fn_with_state(state.clone(), auth));
    let app = Router::new()
        .route("/api/health", get(health))
        // Unauthenticated so the viewer iframe can load it; serves public docs only.
        .route("/content/{docset}/{*path}", get(content))
        .merge(protected)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("binding 127.0.0.1:{port} (is another daemon running?)"))?;
    let _info = InfoFile::write(home, port)?;
    eprintln!("dai daemon listening on http://127.0.0.1:{port} (MCP at /mcp)");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = shutdown.cancelled() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        })
        .await?;
    Ok(())
}

/// Removes `daemon.json` when the server stops.
struct InfoFile(PathBuf);

impl InfoFile {
    fn write(home: &Path, port: u16) -> Result<Self> {
        let path = daemon_info_path(home);
        let info = DaemonInfo {
            port,
            pid: std::process::id(),
        };
        std::fs::write(&path, serde_json::to_vec(&info)?)?;
        Ok(Self(path))
    }
}

impl Drop for InfoFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == &*state.token);
    if ok {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "missing or bad token").into_response()
    }
}

async fn health() -> Json<Health> {
    Json(Health {
        app: "dai".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

#[derive(Deserialize)]
struct RefreshParams {
    #[serde(default)]
    refresh: bool,
}

async fn catalog(
    State(s): State<AppState>,
    Query(p): Query<RefreshParams>,
) -> ApiResult<Json<impl Serialize>> {
    Ok(Json(
        blocking(&s.local.lib, move |l| l.catalog(p.refresh)).await?,
    ))
}

async fn docsets(State(s): State<AppState>) -> ApiResult<Json<impl Serialize>> {
    Ok(Json(blocking(&s.local.lib, |l| l.installed()).await?))
}

async fn outdated(
    State(s): State<AppState>,
    Query(p): Query<RefreshParams>,
) -> ApiResult<Json<impl Serialize>> {
    Ok(Json(
        blocking(&s.local.lib, move |l| l.outdated(p.refresh)).await?,
    ))
}

async fn install(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Docset>> {
    let job_id = id.clone();
    Ok(Json(
        with_install_events(s.local, job_id, move |l, progress| l.install(&id, progress)).await?,
    ))
}

#[derive(Deserialize)]
struct GenerateRequest {
    name: Option<String>,
    source: generate::Source,
}

/// Builds a markdown docset (llms.txt, git repo, folder, or Context7).
async fn generate_docset(
    State(s): State<AppState>,
    Json(req): Json<GenerateRequest>,
) -> ApiResult<Json<Docset>> {
    let name = req.name.clone().filter(|n| !n.trim().is_empty());
    let slug = slugify(&name.clone().unwrap_or_else(|| req.source.default_name()));
    let id = format!("{}{slug}", generate::ID_PREFIX);
    Ok(Json(
        with_install_events(s.local, id, move |l, progress| {
            l.generate(name.as_deref(), req.source, progress)
        })
        .await?,
    ))
}

/// Runs an install-like job on the blocking pool, bracketed by
/// `install_started`/`install_finished` events with progress in between.
async fn with_install_events<F>(local: Arc<Local>, id: String, job: F) -> Result<Docset>
where
    F: FnOnce(&Library, &(dyn Fn(Progress) + Sync)) -> Result<Docset> + Send + 'static,
{
    local.emit(DaiEvent::InstallStarted { id: id.clone() });
    let res = blocking(&local.lib, {
        let (id, local) = (id.clone(), local.clone());
        move |l| job(l, &progress_reporter(&id, &local))
    })
    .await;
    local.emit(DaiEvent::InstallFinished {
        id,
        error: res.as_ref().err().map(|e| format!("{e:#}")),
    });
    res
}

#[derive(Deserialize)]
struct Context7SearchParams {
    name: String,
    #[serde(default)]
    query: String,
}

async fn context7_libraries(
    Query(p): Query<Context7SearchParams>,
) -> ApiResult<Json<Vec<generate::Context7Library>>> {
    let libs = tokio::task::spawn_blocking(move || generate::context7_search(&p.name, &p.query))
        .await
        .map_err(anyhow::Error::from)??;
    Ok(Json(libs))
}

#[derive(Deserialize)]
struct Context7DocsParams {
    library_id: String,
    query: String,
}

/// Context7 results as markdown (for the app's "not found locally" panel).
async fn context7_docs(Query(p): Query<Context7DocsParams>) -> ApiResult<String> {
    Ok(
        tokio::task::spawn_blocking(move || generate::context7_docs(&p.library_id, &p.query))
            .await
            .map_err(anyhow::Error::from)??,
    )
}

/// Turns library progress into `install_progress` events, throttled to whole
/// percent steps (or every 4 MB when the size is unknown).
fn progress_reporter<'a>(id: &'a str, local: &'a Local) -> impl Fn(Progress) + Sync + 'a {
    let last_step = AtomicU64::new(u64::MAX);
    move |p| {
        let (stage, bytes, total) = match p {
            Progress::Download { bytes, total } => ("download", bytes, total),
            Progress::Index => ("index", 0, None),
        };
        let step = match (stage, total) {
            ("index", _) => u64::MAX - 1,
            (_, Some(t)) if t > 0 => bytes * 100 / t,
            _ => bytes / (4 << 20),
        };
        if last_step.swap(step, Ordering::Relaxed) != step {
            local.emit(DaiEvent::InstallProgress {
                id: id.to_string(),
                stage: stage.into(),
                bytes,
                total,
            });
        }
    }
}

async fn remove(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<impl Serialize>> {
    let removed = blocking(&s.local.lib, {
        let id = id.clone();
        move |l| l.remove(&id)
    })
    .await?;
    if removed {
        s.local.emit(DaiEvent::Removed { id });
    }
    Ok(Json(removed))
}

#[derive(Deserialize)]
struct EventParams {
    /// Set by the desktop app so `open` knows whether a window is listening.
    #[serde(default)]
    app: bool,
}

async fn events(
    State(s): State<AppState>,
    Query(p): Query<EventParams>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let guard = p.app.then(|| AppClientGuard::new(s.local.clone()));
    let stream = BroadcastStream::new(s.local.events.subscribe()).filter_map(move |e| {
        let _alive = &guard;
        // Lagged receivers just skip what they missed.
        let e = e.ok()?;
        Some(Ok(Event::default().json_data(e).expect("event serializes")))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Counts an app subscriber for as long as its event stream is open.
struct AppClientGuard(Arc<Local>);

impl AppClientGuard {
    fn new(local: Arc<Local>) -> Self {
        local.app_clients.fetch_add(1, Ordering::SeqCst);
        Self(local)
    }
}

impl Drop for AppClientGuard {
    fn drop(&mut self) {
        self.0.app_clients.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Deserialize)]
struct OpenParams {
    docset: String,
    path: String,
}

async fn open(
    State(s): State<AppState>,
    Json(p): Json<OpenParams>,
) -> ApiResult<Json<OpenOutcome>> {
    Ok(Json(s.local.open(p.docset, p.path)?))
}

/// A page's original HTML wrapped in the viewer shell.
async fn content(
    State(s): State<AppState>,
    UrlPath((docset, path)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    // Dash docsets are served from disk, assets included.
    let file = blocking(&s.local.lib, {
        let (docset, path) = (docset.clone(), path.clone());
        move |l| l.content_file(&docset, &path)
    })
    .await?;
    if let Some(file) = file {
        let bytes = tokio::fs::read(&file).await.map_err(anyhow::Error::from)?;
        let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
        if docset.starts_with(generate::ID_PREFIX) && matches!(ext, "md" | "mdx") {
            let text = String::from_utf8_lossy(&bytes);
            let text = if ext == "mdx" {
                markdown::clean_mdx(&text)
            } else {
                text.into_owned()
            };
            return Ok(
                Html(viewer::wrap(&docset, &path, &markdown::to_html(&text))).into_response(),
            );
        }
        let mime = mime_guess::from_path(&file).first_or_octet_stream();
        if mime.subtype() == mime_guess::mime::HTML {
            let html = String::from_utf8_lossy(&bytes);
            return Ok(Html(viewer::inject(&docset, &path, &html)).into_response());
        }
        return Ok((
            [(header::CONTENT_TYPE, mime.essence_str().to_string())],
            bytes,
        )
            .into_response());
    }

    let page = blocking(&s.local.lib, {
        let (docset, path) = (docset.clone(), path.clone());
        move |l| l.page_html(&docset, &path)
    })
    .await?;
    let Some(html) = page else {
        return Ok((StatusCode::NOT_FOUND, "no such page").into_response());
    };
    Ok(Html(viewer::wrap(&docset, &path, &html)).into_response())
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    /// Comma-separated docset ids.
    #[serde(default)]
    docsets: String,
    #[serde(default = "default_limit")]
    limit: usize,
    /// Project folder: prefer docs matching its dependency versions.
    project: Option<PathBuf>,
}

fn default_limit() -> usize {
    10
}

async fn search(
    State(s): State<AppState>,
    Query(p): Query<SearchParams>,
) -> ApiResult<Json<impl Serialize>> {
    let docsets: Vec<String> = p
        .docsets
        .split(',')
        .filter(|d| !d.is_empty())
        .map(String::from)
        .collect();
    Ok(Json(
        blocking(&s.local.lib, move |l| {
            l.search_in(&p.q, &docsets, p.project.as_deref(), p.limit)
        })
        .await?,
    ))
}

#[derive(Deserialize)]
struct ProjectParams {
    path: PathBuf,
}

/// A project's dependencies matched to installed docsets, with install suggestions.
async fn project(
    State(s): State<AppState>,
    Query(p): Query<ProjectParams>,
) -> ApiResult<Json<ProjectReport>> {
    Ok(Json(
        blocking(&s.local.lib, move |l| l.project_with_suggestions(&p.path)).await?,
    ))
}

#[derive(Deserialize)]
struct DocParams {
    docset: String,
    path: String,
    #[serde(default)]
    offset: usize,
    max_chars: Option<usize>,
}

async fn get_doc(State(s): State<AppState>, Query(p): Query<DocParams>) -> ApiResult<Response> {
    let page = blocking(&s.local.lib, move |l| {
        l.get_doc(&p.docset, &p.path, p.offset, p.max_chars)
    })
    .await?;
    Ok(match page {
        Some(page) => Json(page).into_response(),
        None => (StatusCode::NOT_FOUND, "no such page").into_response(),
    })
}

/// Reloads snippets when files in the folder change (hand edits, git pulls).
/// Dropping the returned debouncer stops watching.
fn watch_snippets(local: Arc<Local>) -> Result<Debouncer<RecommendedWatcher>> {
    let dir = local.lib.snippets_dir().to_path_buf();
    let rt = tokio::runtime::Handle::current();
    let mut debouncer = new_debouncer(
        Duration::from_millis(300),
        move |res: DebounceEventResult| {
            if res.is_err() {
                return;
            }
            let local = local.clone();
            rt.spawn(async move {
                if blocking(&local.lib, |l| l.reload_snippets()).await.is_ok() {
                    local.emit(DaiEvent::SnippetsChanged);
                }
            });
        },
    )?;
    debouncer
        .watcher()
        .watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(debouncer)
}

#[derive(Deserialize)]
struct SnippetParams {
    #[serde(default)]
    q: String,
    language: Option<String>,
    tag: Option<String>,
    #[serde(default = "default_snippet_limit")]
    limit: usize,
}

fn default_snippet_limit() -> usize {
    200
}

async fn list_snippets(
    State(s): State<AppState>,
    Query(p): Query<SnippetParams>,
) -> ApiResult<Json<Vec<Snippet>>> {
    Ok(Json(
        blocking(&s.local.lib, move |l| {
            l.snippets(&p.q, p.language.as_deref(), p.tag.as_deref(), p.limit)
        })
        .await?,
    ))
}

async fn get_snippet(State(s): State<AppState>, UrlPath(id): UrlPath<String>) -> Response {
    match s.local.lib.snippet(&id) {
        Some(snippet) => Json(snippet).into_response(),
        None => (StatusCode::NOT_FOUND, "no such snippet").into_response(),
    }
}

async fn create_snippet(
    State(s): State<AppState>,
    Json(input): Json<SnippetInput>,
) -> ApiResult<Json<Snippet>> {
    let snippet = blocking(&s.local.lib, move |l| l.create_snippet(input)).await?;
    s.local.emit(DaiEvent::SnippetsChanged);
    Ok(Json(snippet))
}

async fn update_snippet(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(input): Json<SnippetInput>,
) -> ApiResult<Json<Snippet>> {
    let snippet = blocking(&s.local.lib, move |l| l.update_snippet(&id, input)).await?;
    s.local.emit(DaiEvent::SnippetsChanged);
    Ok(Json(snippet))
}

async fn delete_snippet(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<bool>> {
    let deleted = blocking(&s.local.lib, move |l| l.delete_snippet(&id)).await?;
    if deleted {
        s.local.emit(DaiEvent::SnippetsChanged);
    }
    Ok(Json(deleted))
}

async fn shutdown_handler(State(s): State<AppState>) -> StatusCode {
    s.shutdown.cancel();
    StatusCode::NO_CONTENT
}

type ApiResult<T> = Result<T, ApiError>;

struct ApiError(anyhow::Error);

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", self.0)).into_response()
    }
}
