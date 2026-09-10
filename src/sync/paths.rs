use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use crate::db::collection_dir;

const OFFICIAL_MARKER: &str = ".official-backend-v1";

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

pub fn anki2_exists() -> bool {
    anki2_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn official_ready() -> bool {
    anki_dir()
        .map(|dir| dir.join(OFFICIAL_MARKER).exists())
        .unwrap_or(false)
        && anki2_exists()
}

pub fn migration_required() -> bool {
    anki2_exists() && !official_ready()
}

pub fn mark_official_ready(anki_version: &str) -> Result<()> {
    let marker = anki_dir()?.join(OFFICIAL_MARKER);
    fs::write(&marker, format!("anki={anki_version}\n"))
        .with_context(|| format!("write {}", marker.display()))
}
