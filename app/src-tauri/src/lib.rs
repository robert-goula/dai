//! DAI desktop app. All daemon traffic goes through this Rust side (so the
//! API token never reaches the webview); the UI calls these commands and
//! listens for `dai-event`.

use std::time::Duration;

use dai_core::CatalogEntry;
use dai_core::index::Hit;
use dai_core::paths;
use dai_core::store::Docset;
use dai_daemon::DaiEvent;
use dai_daemon::client::Client;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tokio::sync::OnceCell;

const EVENT: &str = "dai-event";

#[derive(Default)]
struct Daemon(OnceCell<Client>);

impl Daemon {
    /// Connects on first use, starting the daemon (this binary with `serve`) if needed.
    async fn client(&self) -> Result<&Client, String> {
        self.0
            .get_or_try_init(|| async { Client::connect(&paths::home()?).await })
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
async fn daemon_url(daemon: State<'_, Daemon>) -> CmdResult<String> {
    Ok(daemon.client().await?.base().to_string())
}

#[tauri::command]
async fn docsets(daemon: State<'_, Daemon>) -> CmdResult<Vec<Docset>> {
    daemon.client().await?.docsets().await.map_err(err)
}

#[tauri::command]
async fn catalog(daemon: State<'_, Daemon>, refresh: bool) -> CmdResult<Vec<CatalogEntry>> {
    daemon.client().await?.catalog(refresh).await.map_err(err)
}

#[tauri::command]
async fn outdated(daemon: State<'_, Daemon>, refresh: bool) -> CmdResult<Vec<Docset>> {
    daemon.client().await?.outdated(refresh).await.map_err(err)
}

#[tauri::command]
async fn install(daemon: State<'_, Daemon>, id: String) -> CmdResult<Docset> {
    daemon.client().await?.install(&id).await.map_err(err)
}

#[tauri::command]
async fn remove(daemon: State<'_, Daemon>, id: String) -> CmdResult<bool> {
    daemon.client().await?.remove(&id).await.map_err(err)
}

#[tauri::command]
async fn search(
    daemon: State<'_, Daemon>,
    query: String,
    docsets: Vec<String>,
    limit: usize,
) -> CmdResult<Vec<Hit>> {
    daemon
        .client()
        .await?
        .search(&query, &docsets, limit)
        .await
        .map_err(err)
}

#[derive(Clone, Serialize)]
struct OpenTarget {
    docset: String,
    path: String,
}

/// The page to show if the app was launched by a `dai://open` link.
#[tauri::command]
fn initial_open(app: AppHandle) -> Option<OpenTarget> {
    app.deep_link()
        .get_current()
        .ok()
        .flatten()?
        .iter()
        .find_map(parse_open_url)
}

/// `dai://open?docset=react&path=reference/react/useeffect`
fn parse_open_url(url: &tauri::Url) -> Option<OpenTarget> {
    if url.scheme() != "dai" || url.host_str() != Some("open") {
        return None;
    }
    let get = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    Some(OpenTarget {
        docset: get("docset")?,
        path: get("path")?,
    })
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Relays daemon events to the UI, reconnecting (and restarting the daemon)
/// if the stream drops.
async fn relay_events(app: AppHandle) {
    let mut first = true;
    loop {
        // The first connect shares the commands' client so startup spawns one
        // daemon; later ones restart it if it died.
        let client = if first {
            app.state::<Daemon>().client().await.ok().cloned()
        } else {
            match paths::home() {
                Ok(home) => Client::connect(&home).await.ok(),
                Err(_) => None,
            }
        };
        first = false;
        if let Some(client) = client {
            let _ = client
                .subscribe(true, |event| {
                    if matches!(event, DaiEvent::Open { .. }) {
                        show_main_window(&app);
                    }
                    let _ = app.emit(EVENT, event);
                })
                .await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Runs the daemon instead of the UI (`<app> serve`), so the app can start a
/// daemon without a separately installed `dai` binary.
pub fn serve_daemon() -> anyhow::Result<()> {
    let home = paths::home()?;
    let port = dai_daemon::port_from_env().unwrap_or(dai_daemon::DEFAULT_PORT);
    tokio::runtime::Runtime::new()?.block_on(dai_daemon::server::serve(&home, port))
}

pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch (e.g. from a deep link) hands its
        // URL to this instance and exits.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Daemon::default())
        .setup(|app| {
            #[cfg(any(windows, target_os = "linux"))]
            app.deep_link().register_all()?;

            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                if let Some(target) = event.urls().iter().find_map(parse_open_url) {
                    show_main_window(&handle);
                    let _ = handle.emit(
                        EVENT,
                        DaiEvent::Open {
                            docset: target.docset,
                            path: target.path,
                        },
                    );
                }
            });
            tauri::async_runtime::spawn(relay_events(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            daemon_url,
            docsets,
            catalog,
            outdated,
            install,
            remove,
            search,
            initial_open,
        ])
        .run(tauri::generate_context!())
        .expect("error while running DAI");
}
