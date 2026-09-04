//! Pick a local image via `yazi --chooser-file` and import into Anki media.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::sync::media::db::{mtime_secs, sha1_hex, MediaDb, MediaEntry};
use crate::sync::paths;

/// Run yazi as a file chooser. Returns `None` if cancelled / no selection.
pub fn pick_with_yazi() -> Result<Option<PathBuf>> {
    if !yazi_available() {
        bail!("yazi not found — install yazi to pick images (https://yazi-rs.github.io/)");
    }

    let chooser = std::env::temp_dir().join(format!(
        "anki-tui-yazi-chooser-{}.txt",
        std::process::id()
    ));
    let _ = fs::remove_file(&chooser);

    let status = Command::new("yazi")
        .arg("--chooser-file")
        .arg(&chooser)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("spawn yazi")?;

    if !status.success() && !chooser.exists() {
        return Ok(None);
    }
    if !chooser.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&chooser).unwrap_or_default();
    let _ = fs::remove_file(&chooser);
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(line);
    if !path.is_file() {
        bail!("yazi selection is not a file: {}", path.display());
    }
    Ok(Some(path))
}

pub fn yazi_available() -> bool {
    Command::new("yazi")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Copy `src` into `collection.media/`, mark dirty for sync, return Anki media filename.
pub fn import_media_file(src: &Path) -> Result<String> {
    let folder = paths::media_folder()?;
    let db_path = paths::media_db_path()?;
    let data = fs::read(src).with_context(|| format!("read {}", src.display()))?;
    let csum = sha1_hex(&data);

    let fname = unique_media_name(src, &folder, &csum)?;
    let dest = folder.join(&fname);
    if !dest.exists() {
        fs::write(&dest, &data).with_context(|| format!("write {}", dest.display()))?;
    }

    let meta = fs::metadata(&dest)?;
    let db = MediaDb::open(&db_path)?;
    db.set_entry(&MediaEntry {
        fname: fname.clone(),
        sha1: Some(csum),
        mtime: mtime_secs(&meta),
        dirty: true,
    })?;

    Ok(fname)
}

fn unique_media_name(src: &Path, folder: &Path, csum: &str) -> Result<String> {
    let raw = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{csum}.bin"));
    let safe = sanitize_fname(&raw);
    let dest = folder.join(&safe);
    if !dest.exists() {
        return Ok(safe);
    }
    // Same content → reuse existing name
    if let Ok(existing) = fs::read(&dest) {
        if sha1_hex(&existing) == csum {
            return Ok(safe);
        }
    }
    let (stem, ext) = split_stem_ext(&safe);
    for i in 1..10_000 {
        let candidate = format!("{stem}-{i}{ext}");
        let p = folder.join(&candidate);
        if !p.exists() {
            return Ok(candidate);
        }
        if let Ok(existing) = fs::read(&p) {
            if sha1_hex(&existing) == csum {
                return Ok(candidate);
            }
        }
    }
    Ok(format!("{csum}{ext}"))
}

fn sanitize_fname(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => out.push(ch),
            ' ' => out.push('_'),
            _ => {}
        }
    }
    if out.is_empty() || out == "." || out.starts_with('.') {
        format!("image-{out}")
    } else {
        out
    }
}

fn split_stem_ext(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

pub fn img_tag(fname: &str) -> String {
    format!(r#"<img src="{fname}">"#)
}
