//! HTTP client for the daemon, including starting it when it isn't running.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use dai_core::CatalogEntry;
use dai_core::DocPage;
use dai_core::index::Hit;
use dai_core::store::Docset;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use tokio_stream::StreamExt;

use crate::DaiEvent;
use crate::backend::OpenOutcome;
use crate::server::Health;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Client {
    /// Connects to the running daemon, starting one in the background if needed.
    pub async fn connect(home: &Path) -> Result<Self> {
        let client = Self::new(home)?;
        if client.healthy().await {
            return Ok(client);
        }
        spawn_daemon(home)?;
        let started = Instant::now();
        while started.elapsed() < STARTUP_TIMEOUT {
            tokio::time::sleep(Duration::from_millis(100)).await;
            // The daemon may have picked a different port via $DAI_PORT; re-read.
            let client = Self::new(home)?;
            if client.healthy().await {
                return Ok(client);
            }
        }
        bail!(
            "daemon did not start within {STARTUP_TIMEOUT:?}; see {}",
            home.join("daemon.log").display()
        )
    }

    /// A client for the daemon without checking that it's up.
    pub fn new(home: &Path) -> Result<Self> {
        Ok(Self {
            base: format!("http://127.0.0.1:{}", crate::port(home)),
            token: crate::token(home)?,
            http: reqwest::Client::new(),
        })
    }

    /// e.g. `http://127.0.0.1:4747`
    pub fn base(&self) -> &str {
        &self.base
    }

    async fn healthy(&self) -> bool {
        let res = self
            .http
            .get(format!("{}/api/health", self.base))
            .timeout(Duration::from_secs(1))
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {
                r.json::<Health>().await.is_ok_and(|h| h.app == "dai")
            }
            _ => false,
        }
    }

    pub async fn catalog(&self, refresh: bool) -> Result<Vec<CatalogEntry>> {
        send(
            self.req(Method::GET, "/api/catalog")
                .query(&[("refresh", refresh)]),
        )
        .await
    }

    pub async fn docsets(&self) -> Result<Vec<Docset>> {
        send(self.req(Method::GET, "/api/docsets")).await
    }

    pub async fn outdated(&self, refresh: bool) -> Result<Vec<Docset>> {
        send(
            self.req(Method::GET, "/api/outdated")
                .query(&[("refresh", refresh)]),
        )
        .await
    }

    pub async fn install(&self, slug: &str) -> Result<Docset> {
        send(self.req(Method::POST, &format!("/api/docsets/{slug}"))).await
    }

    pub async fn remove(&self, id: &str) -> Result<bool> {
        send(self.req(Method::DELETE, &format!("/api/docsets/{id}"))).await
    }

    pub async fn search(&self, query: &str, docsets: &[String], limit: usize) -> Result<Vec<Hit>> {
        let params = [
            ("q", query.to_string()),
            ("docsets", docsets.join(",")),
            ("limit", limit.to_string()),
        ];
        send(self.req(Method::GET, "/api/search").query(&params)).await
    }

    pub async fn get_doc(
        &self,
        docset: &str,
        path: &str,
        offset: usize,
        max_chars: Option<usize>,
    ) -> Result<Option<DocPage>> {
        let mut params = vec![
            ("docset", docset.to_string()),
            ("path", path.to_string()),
            ("offset", offset.to_string()),
        ];
        if let Some(m) = max_chars {
            params.push(("max_chars", m.to_string()));
        }
        let res = self
            .req(Method::GET, "/api/doc")
            .query(&params)
            .send()
            .await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(decode(res).await?))
    }

    pub async fn open(&self, docset: &str, path: &str) -> Result<OpenOutcome> {
        let body = serde_json::json!({ "docset": docset, "path": path });
        send(self.req(Method::POST, "/api/open").json(&body)).await
    }

    /// Calls `on_event` for each daemon event until the stream ends. `app`
    /// marks this subscriber as a desktop app window (see `open`).
    pub async fn subscribe(&self, app: bool, mut on_event: impl FnMut(DaiEvent)) -> Result<()> {
        let res = self
            .req(Method::GET, "/api/events")
            .query(&[("app", app)])
            .send()
            .await?
            .error_for_status()?;
        let mut stream = res.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk?));
            // SSE events end with a blank line; `data:` lines carry the JSON.
            while let Some(end) = buf.find("\n\n") {
                let block: String = buf.drain(..end + 2).collect();
                let data: String = block
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect();
                if let Ok(event) = serde_json::from_str(&data) {
                    on_event(event);
                }
            }
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.req(Method::POST, "/api/shutdown")
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
    }
}

async fn send<T: DeserializeOwned>(req: RequestBuilder) -> Result<T> {
    decode(req.send().await.context("daemon request failed")?).await
}

async fn decode<T: DeserializeOwned>(res: reqwest::Response) -> Result<T> {
    let status = res.status();
    if !status.is_success() {
        bail!(
            "{}",
            res.text().await.unwrap_or_else(|_| status.to_string())
        );
    }
    Ok(res.json().await?)
}

/// Starts `dai serve` detached from this process, logging to `<home>/daemon.log`.
fn spawn_daemon(home: &Path) -> Result<()> {
    std::fs::create_dir_all(home)?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("daemon.log"))?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("serve")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // Own process group, so the daemon outlives whatever started it
    // (e.g. an agent's MCP stdio process being killed).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().context("starting daemon")?;
    Ok(())
}
