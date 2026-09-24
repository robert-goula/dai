use std::path::PathBuf;

use anyhow::{Context, Result};

/// DAI's data dir: `$DAI_HOME` if set, otherwise the platform data dir.
pub fn home() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("DAI_HOME") {
        return Ok(PathBuf::from(p));
    }
    let dirs = directories::ProjectDirs::from("", "", "dai").context("no home directory found")?;
    Ok(dirs.data_dir().to_path_buf())
}
