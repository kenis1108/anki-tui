use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::db::collection_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    pub hkey: String,
    pub endpoint: String,
}

pub fn auth_file_path() -> Result<PathBuf> {
    Ok(collection_dir()?.join("ankiweb-auth.json"))
}

pub fn load_auth() -> Result<Option<Auth>> {
    let path = auth_file_path()?;
    if !path.exists() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("read auth file {}", path.display()))?;
    let auth: Auth = serde_json::from_str(&text)?;
    if auth.hkey.is_empty() {
        return Ok(None);
    }
    Ok(Some(auth))
}

pub fn save_auth(auth: &Auth) -> Result<()> {
    let path = auth_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(auth)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("open auth file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(text.as_bytes())
        .with_context(|| format!("write auth file {}", path.display()))?;
    Ok(())
}

pub fn clear_auth() -> Result<()> {
    let path = auth_file_path()?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}
