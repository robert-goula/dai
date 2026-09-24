//! DAI daemon: HTTP API, MCP server, and the client that talks to them.

pub mod backend;
pub mod client;
pub mod mcp;
pub mod server;
mod viewer;

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 4747;

/// Written to `<home>/daemon.json` while the daemon runs, so clients can find it.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub port: u16,
    pub pid: u32,
}

/// Pushed to subscribers of `/api/events` (the desktop app).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaiEvent {
    InstallStarted {
        id: String,
    },
    InstallFinished {
        id: String,
        error: Option<String>,
    },
    Removed {
        id: String,
    },
    /// An agent asked to show a page to the user.
    Open {
        docset: String,
        path: String,
    },
}

/// `$DAI_PORT` if set, else the running daemon's port, else the default.
pub fn port(home: &Path) -> u16 {
    if let Some(p) = port_from_env() {
        return p;
    }
    std::fs::read(daemon_info_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice::<DaemonInfo>(&b).ok())
        .map_or(DEFAULT_PORT, |i| i.port)
}

pub fn port_from_env() -> Option<u16> {
    std::env::var("DAI_PORT").ok()?.parse().ok()
}

fn daemon_info_path(home: &Path) -> PathBuf {
    home.join("daemon.json")
}

/// Bearer token for the HTTP API and MCP endpoint. Created once and kept, so
/// agent configs that embed it keep working across restarts.
pub fn token(home: &Path) -> Result<String> {
    let path = home.join("token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        return Ok(t.trim().to_string());
    }
    std::fs::create_dir_all(home)?;
    let t = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(&path, &t)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(t)
}
