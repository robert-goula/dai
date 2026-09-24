use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// DAI's data dir: `$DAI_HOME` if set, otherwise the platform data dir.
pub fn home() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("DAI_HOME") {
        return Ok(PathBuf::from(p));
    }
    let dirs = directories::ProjectDirs::from("", "", "dai").context("no home directory found")?;
    Ok(dirs.data_dir().to_path_buf())
}

/// `~/.config/dai` on every platform (holds `config.toml` and, by default, snippets).
pub fn config_dir() -> Result<PathBuf> {
    Ok(user_home()?.join(".config").join("dai"))
}

#[derive(Default, Deserialize)]
struct Config {
    snippets_dir: Option<String>,
}

/// Where snippet `.md` files live: `$DAI_SNIPPETS_DIR`, else `snippets_dir` in
/// `~/.config/dai/config.toml`, else `~/.config/dai/snippets`.
pub fn snippets_dir() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("DAI_SNIPPETS_DIR") {
        return Ok(PathBuf::from(p));
    }
    let config_path = config_dir()?.join("config.toml");
    let config: Config = match std::fs::read_to_string(&config_path) {
        Ok(s) => {
            toml::from_str(&s).with_context(|| format!("reading {}", config_path.display()))?
        }
        Err(_) => Config::default(),
    };
    match config.snippets_dir {
        Some(dir) => expand_tilde(&dir),
        None => Ok(config_dir()?.join("snippets")),
    }
}

fn expand_tilde(path: &str) -> Result<PathBuf> {
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => Ok(user_home()?.join(rest)),
        None if path == "~" => user_home(),
        None => Ok(Path::new(path).to_path_buf()),
    }
}

fn user_home() -> Result<PathBuf> {
    Ok(directories::BaseDirs::new()
        .context("no home directory found")?
        .home_dir()
        .to_path_buf())
}
