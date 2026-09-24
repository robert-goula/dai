//! Where MCP tools get their data: the library in-process (inside the daemon)
//! or the daemon over HTTP (the `dai mcp` stdio server).

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use dai_core::index::Hit;
use dai_core::store::Docset;
use dai_core::{DocPage, Library};
use tokio::sync::broadcast;

use crate::DaiEvent;
use crate::client::Client;

#[derive(Clone)]
pub enum Backend {
    Local(Arc<Local>),
    Remote(Client),
}

/// The daemon's in-process state: the library plus the event hub.
pub struct Local {
    pub lib: Arc<Library>,
    pub events: broadcast::Sender<DaiEvent>,
    /// Desktop app windows currently subscribed to events.
    pub app_clients: AtomicUsize,
}

impl Local {
    pub fn new(lib: Library) -> Self {
        Self {
            lib: Arc::new(lib),
            events: broadcast::channel(64).0,
            app_clients: AtomicUsize::new(0),
        }
    }

    pub fn emit(&self, event: DaiEvent) {
        // No subscribers is fine.
        let _ = self.events.send(event);
    }

    /// Shows a page in the desktop app: via the event stream if the app is
    /// running, otherwise by launching it with a `dai://open` deep link.
    pub fn open(&self, docset: String, path: String) -> Result<OpenOutcome> {
        if self.app_clients.load(Ordering::SeqCst) > 0 {
            self.emit(DaiEvent::Open { docset, path });
            return Ok(OpenOutcome::Shown);
        }
        let url =
            reqwest::Url::parse_with_params("dai://open", [("docset", &docset), ("path", &path)])?;
        launch_url(url.as_str())?;
        Ok(OpenOutcome::Launched)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenOutcome {
    /// The running app navigated to the page.
    Shown,
    /// The app wasn't running; the OS was asked to launch it.
    Launched,
}

fn launch_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut cmd = Command::new("xdg-open");
    #[cfg(windows)]
    let mut cmd = {
        // Avoids cmd.exe's `start` and its quoting rules around `&`.
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    cmd.arg(url).spawn().context("launching the DAI app")?;
    Ok(())
}

impl Backend {
    pub async fn docsets(&self) -> Result<Vec<Docset>> {
        match self {
            Self::Local(l) => blocking(&l.lib, |l| l.installed()).await,
            Self::Remote(c) => c.docsets().await,
        }
    }

    pub async fn search(
        &self,
        query: String,
        docsets: Vec<String>,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        match self {
            Self::Local(l) => blocking(&l.lib, move |l| l.search(&query, &docsets, limit)).await,
            Self::Remote(c) => c.search(&query, &docsets, limit).await,
        }
    }

    pub async fn get_doc(
        &self,
        docset: String,
        path: String,
        offset: usize,
        max_chars: Option<usize>,
    ) -> Result<Option<DocPage>> {
        match self {
            Self::Local(l) => {
                blocking(&l.lib, move |l| {
                    l.get_doc(&docset, &path, offset, max_chars)
                })
                .await
            }
            Self::Remote(c) => c.get_doc(&docset, &path, offset, max_chars).await,
        }
    }

    pub async fn open(&self, docset: String, path: String) -> Result<OpenOutcome> {
        match self {
            Self::Local(l) => l.open(docset, path),
            Self::Remote(c) => c.open(&docset, &path).await,
        }
    }
}

/// Runs a library call on the blocking pool (SQLite, tantivy, and the DevDocs
/// client are all synchronous).
pub async fn blocking<T, F>(lib: &Arc<Library>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Library) -> Result<T> + Send + 'static,
{
    let lib = lib.clone();
    tokio::task::spawn_blocking(move || f(&lib)).await?
}
