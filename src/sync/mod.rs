mod auth;
mod client;
mod delta;
mod export;
mod import;
pub(crate) mod media;
pub(crate) mod paths;

pub use auth::{load_auth, save_auth, Auth};
pub use client::AnkiWebClient;
pub use export::export_collection_bytes;
pub use import::{import_anki_collection_bytes, ImportStats, ImportedNote};
pub use media::MediaSyncStats;

use anyhow::{bail, Context, Result};

use crate::db::Store;
use crate::sync::delta::{run_delta_sync, DeltaOutcome};

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub message: String,
    pub imported: Option<ImportStats>,
    pub media: Option<MediaSyncStats>,
}

pub fn login(user: &str, pass: &str, endpoint: Option<&str>) -> Result<Auth> {
    let mut client = AnkiWebClient::new(endpoint.unwrap_or(AnkiWebClient::DEFAULT_ENDPOINT), "");
    client.login(user, pass)?;
    let auth = Auth {
        hkey: client.hkey().to_string(),
        endpoint: client.endpoint().to_string(),
    };
    save_auth(&auth)?;
    Ok(auth)
}

pub fn logout() -> Result<()> {
    auth::clear_auth()
}

fn client_from_auth() -> Result<AnkiWebClient> {
    let auth = load_auth()?.context("not logged in — open Sync and sign in first")?;
    Ok(AnkiWebClient::new(&auth.endpoint, &auth.hkey))
}

fn persist_client(client: &AnkiWebClient) -> Result<()> {
    save_auth(&Auth {
        hkey: client.hkey().to_string(),
        endpoint: client.endpoint().to_string(),
    })
}

/// Full download from AnkiWeb, save shadow collection, import, then media sync.
pub fn full_download(store: &Store) -> Result<SyncResult> {
    let mut client = client_from_auth()?;
    let meta = client.meta()?;
    if !meta.cont {
        bail!("server refused sync: {}", meta.msg);
    }
    let bytes = client.download().context("download collection")?;
    paths::save_anki2_bytes(&bytes)?;
    persist_client(&client)?;
    let stats = import_anki_collection_bytes(store, &bytes)?;
    let media = media::sync_media(&mut client).ok();
    persist_client(&client)?;
    let media_msg = media
        .as_ref()
        .map(|m| format!(" · media ↓{} ↑{}", m.downloaded, m.uploaded))
        .unwrap_or_default();
    Ok(SyncResult {
        message: format!(
            "Full download · {} decks, {} notes{media_msg}",
            stats.decks, stats.notes
        ),
        imported: Some(stats),
        media,
    })
}

/// Full upload: export → replace AnkiWeb, save shadow, media sync.
pub fn full_upload(store: &Store) -> Result<SyncResult> {
    let mut client = client_from_auth()?;
    let meta = client.meta()?;
    if !meta.cont {
        bail!("server refused sync: {}", meta.msg);
    }
    let bytes = export_collection_bytes(store)?;
    client.upload(&bytes).context("upload collection")?;
    paths::save_anki2_bytes(&bytes)?;
    persist_client(&client)?;
    let media = media::sync_media(&mut client).ok();
    persist_client(&client)?;
    let media_msg = media
        .as_ref()
        .map(|m| format!(" · media ↓{} ↑{}", m.downloaded, m.uploaded))
        .unwrap_or_default();
    Ok(SyncResult {
        message: format!("Full upload to AnkiWeb{media_msg}"),
        imported: None,
        media,
    })
}

/// Incremental collection sync against shadow `collection.anki2`, then media sync.
/// Falls back to advising full sync when schema diverges.
pub fn normal_sync(store: &Store) -> Result<SyncResult> {
    let mut client = client_from_auth()?;
    if !paths::anki2_exists() {
        return full_download(store);
    }

    let anki2 = paths::anki2_path()?;
    // Push local TUI content into shadow before delta (best-effort content merge via re-export overlay)
    push_tui_into_shadow(store)?;

    let outcome = run_delta_sync(&anki2, &mut client)?;
    persist_client(&client)?;

    let (col_msg, imported) = match outcome {
        DeltaOutcome::NoChanges => ("Collection already in sync".into(), None),
        DeltaOutcome::Success { .. } => {
            let bytes = paths::load_anki2_bytes()?.context("shadow missing after delta")?;
            let stats = import_anki_collection_bytes(store, &bytes)?;
            (
                format!(
                    "Incremental sync · {} decks, {} notes",
                    stats.decks, stats.notes
                ),
                Some(stats),
            )
        }
        DeltaOutcome::FullSyncRequired { reason } => {
            return Ok(SyncResult {
                message: format!(
                    "Full sync required ({reason}). Press F2 to download or F3 to upload."
                ),
                imported: None,
                media: None,
            });
        }
    };

    let media = media::sync_media(&mut client).ok();
    persist_client(&client)?;
    let media_msg = media
        .as_ref()
        .map(|m| {
            format!(
                " · media ↓{} ↑{} ✕{}",
                m.downloaded, m.uploaded, m.deleted
            )
        })
        .unwrap_or_else(|| " · media skipped".into());

    Ok(SyncResult {
        message: format!("{col_msg}{media_msg}"),
        imported,
        media,
    })
}

/// Media-only sync.
pub fn media_only_sync() -> Result<SyncResult> {
    let mut client = client_from_auth()?;
    let media = media::sync_media(&mut client)?;
    persist_client(&client)?;
    Ok(SyncResult {
        message: format!(
            "Media sync · ↓{} ↑{} ✕{} (checked {})",
            media.downloaded, media.uploaded, media.deleted, media.checked
        ),
        imported: None,
        media: Some(media),
    })
}

/// Best-effort: mark dirty notes in shadow by rewriting from TUI export for notes
/// that exist, then leave USNs for delta. For simplicity we merge exported notes
/// into the shadow DB with usn=-1 when flds differ.
fn push_tui_into_shadow(store: &Store) -> Result<()> {
    let Some(_) = paths::load_anki2_bytes()? else {
        return Ok(());
    };
    let anki2 = paths::anki2_path()?;
    let conn = rusqlite::Connection::open(&anki2)?;
    let notes = store.list_notes_for_export()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    for n in notes {
        // Find a note with matching front field (first field)
        let mut stmt = conn.prepare("SELECT id, flds, tags FROM notes")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut matched = None;
        for row in rows.flatten() {
            let front = row.1.split('\u{1f}').next().unwrap_or("");
            if front == n.front {
                matched = Some(row);
                break;
            }
        }
        if let Some((id, old_flds, old_tags)) = matched {
            let new_flds = format!("{}\u{1f}{}", n.front, n.back);
            let tags = if n.tags.is_empty() {
                String::new()
            } else {
                format!(" {} ", n.tags.trim())
            };
            if new_flds != old_flds || tags != old_tags {
                conn.execute(
                    "UPDATE notes SET flds = ?1, tags = ?2, mod = ?3, usn = -1 WHERE id = ?4",
                    rusqlite::params![new_flds, tags, now, id],
                )?;
            }
        }
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

pub fn auth_status() -> String {
    let sync_hint = if paths::anki2_exists() {
        "shadow collection ready"
    } else {
        "no shadow yet (full download first)"
    };
    match load_auth() {
        Ok(Some(a)) => format!("Signed in · {} · {sync_hint}", a.endpoint),
        Ok(None) => "Not signed in".into(),
        Err(e) => format!("Auth error: {e}"),
    }
}
