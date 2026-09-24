//! `dai serve`: HTTP API under `/api`, MCP (streamable HTTP) at `/mcp`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dai_core::Library;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::backend::{Backend, blocking};
use crate::mcp::DaiMcp;
use crate::{DaemonInfo, daemon_info_path};

#[derive(Serialize, Deserialize)]
pub struct Health {
    pub app: String,
    pub version: String,
}

#[derive(Clone)]
struct AppState {
    lib: Arc<Library>,
    token: Arc<str>,
    shutdown: CancellationToken,
}

pub async fn serve(home: &Path, port: u16) -> Result<()> {
    let lib = Arc::new(Library::open(home)?);
    let shutdown = CancellationToken::new();
    let state = AppState {
        lib: lib.clone(),
        token: crate::token(home)?.into(),
        shutdown: shutdown.clone(),
    };

    let mut mcp_config = StreamableHttpServerConfig::default();
    mcp_config.cancellation_token = shutdown.child_token();
    let mcp = StreamableHttpService::new(
        move || Ok(DaiMcp::new(Backend::Local(lib.clone()))),
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
        .route("/api/shutdown", post(shutdown_handler))
        .nest_service("/mcp", mcp)
        .layer(middleware::from_fn_with_state(state.clone(), auth));
    let app = Router::new()
        .route("/api/health", get(health))
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
    Ok(Json(blocking(&s.lib, move |l| l.catalog(p.refresh)).await?))
}

async fn docsets(State(s): State<AppState>) -> ApiResult<Json<impl Serialize>> {
    Ok(Json(blocking(&s.lib, |l| l.installed()).await?))
}

async fn outdated(
    State(s): State<AppState>,
    Query(p): Query<RefreshParams>,
) -> ApiResult<Json<impl Serialize>> {
    Ok(Json(
        blocking(&s.lib, move |l| l.outdated(p.refresh)).await?,
    ))
}

async fn install(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<impl Serialize>> {
    Ok(Json(blocking(&s.lib, move |l| l.install(&id)).await?))
}

async fn remove(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<impl Serialize>> {
    Ok(Json(blocking(&s.lib, move |l| l.remove(&id)).await?))
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    /// Comma-separated docset ids.
    #[serde(default)]
    docsets: String,
    #[serde(default = "default_limit")]
    limit: usize,
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
        blocking(&s.lib, move |l| l.search(&p.q, &docsets, p.limit)).await?,
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
    let page = blocking(&s.lib, move |l| {
        l.get_doc(&p.docset, &p.path, p.offset, p.max_chars)
    })
    .await?;
    Ok(match page {
        Some(page) => Json(page).into_response(),
        None => (StatusCode::NOT_FOUND, "no such page").into_response(),
    })
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
