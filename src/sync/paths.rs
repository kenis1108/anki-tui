use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::collection_dir;

pub fn anki_dir() -> Result<PathBuf> {
    let dir = collection_dir()?.join("anki");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn anki2_path() -> Result<PathBuf> {
    Ok(anki_dir()?.join("collection.anki2"))
}

pub fn media_folder() -> Result<PathBuf> {
    let dir = anki_dir()?.join("collection.media");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn media_db_path() -> Result<PathBuf> {
    Ok(anki_dir()?.join("collection.media.db2"))
}

pub fn save_anki2_bytes(bytes: &[u8]) -> Result<PathBuf> {
    let path = anki2_path()?;
    // remove wal/shm sidecars if any
    for ext in ["-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{ext}", path.display()));
        let _ = fs::remove_file(p);
    }
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn load_anki2_bytes() -> Result<Option<Vec<u8>>> {
    let path = anki2_path()?;
    if !path.exists() {
        return Ok(None);
    }
    // checkpoint if openable
    if let Ok(conn) = rusqlite::Connection::open(&path) {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
    Ok(Some(fs::read(&path)?))
}

pub fn anki2_exists() -> bool {
    anki2_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn ensure_media_layout() -> Result<(PathBuf, PathBuf)> {
    Ok((media_folder()?, media_db_path()?))
}

#[allow(dead_code)]
pub fn remove_anki2() -> Result<()> {
    let path = anki2_path()?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    for ext in ["-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{ext}", path.display()));
        let _ = fs::remove_file(p);
    }
    Ok(())
}

pub fn path_str(p: &Path) -> String {
    p.display().to_string()
}
