mod auth;
pub(crate) mod media;
pub(crate) mod paths;

pub use auth::{load_auth, save_auth, Auth};

use anyhow::{bail, Context, Result};

use crate::anki_backend::{FullSyncOutput, MediaProgress};

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub message: String,
    pub requires_user_action: bool,
}

pub fn login(user: &str, pass: &str, endpoint: Option<&str>) -> Result<Auth> {
    let output = crate::anki_backend::login(user, pass, endpoint)?;
    let auth = Auth {
        hkey: output.hkey,
        endpoint: output.endpoint,
    };
    save_auth(&auth)?;
    Ok(auth)
}

pub fn logout() -> Result<()> {
    auth::clear_auth()
}

fn require_auth() -> Result<Auth> {
    load_auth()?.context("not logged in — open Sync and sign in first")
}

fn require_official_collection() -> Result<()> {
    if !paths::official_ready() {
        bail!(
            "official collection is not initialized; first upload the known-good Desktop collection to AnkiWeb, then press F2 to download it"
        );
    }
    require_official_backend()?;
    Ok(())
}

fn require_official_backend() -> Result<()> {
    if !crate::anki_backend::available() {
        bail!("official Anki Python package is unavailable");
    }
    Ok(())
}

fn save_new_endpoint(auth: &mut Auth, endpoint: &str) -> Result<()> {
    if !endpoint.is_empty() && endpoint != auth.endpoint {
        auth.endpoint = endpoint.to_string();
        save_auth(auth)?;
    }
    Ok(())
}

/// Replace the local shadow with the AnkiWeb collection through Anki's official backend.
pub fn full_download(
    on_progress: impl FnMut(crate::anki_backend::BackendProgress),
) -> Result<SyncResult> {
    require_official_backend()?;
    let mut auth = require_auth()?;
    let output = crate::anki_backend::full_download(&auth, on_progress)?;
    save_new_endpoint(&mut auth, &output.new_endpoint)?;
    let health = crate::anki_backend::health().context("validate downloaded collection")?;
    anyhow::ensure!(
        health.cards == output.cards,
        "downloaded collection validation count changed unexpectedly"
    );
    paths::mark_official_ready(&health.anki_version)?;
    Ok(SyncResult {
        message: format_full("Full download", &output),
        requires_user_action: false,
    })
}

/// Replace AnkiWeb with the untouched authoritative shadow collection.
pub fn full_upload(
    on_progress: impl FnMut(crate::anki_backend::BackendProgress),
) -> Result<SyncResult> {
    require_official_collection()?;
    let mut auth = require_auth()?;
    let output = crate::anki_backend::full_upload(&auth, on_progress)?;
    save_new_endpoint(&mut auth, &output.new_endpoint)?;
    Ok(SyncResult {
        message: format_full("Full upload", &output),
        requires_user_action: false,
    })
}

/// Bidirectional sync using the same backend and collection format as Anki Desktop.
pub fn normal_sync(
    on_progress: impl FnMut(crate::anki_backend::BackendProgress),
) -> Result<SyncResult> {
    require_official_collection()?;
    let mut auth = require_auth()?;
    let output = crate::anki_backend::normal_sync(&auth, on_progress)?;
    save_new_endpoint(&mut auth, &output.new_endpoint)?;

    let (message, requires_user_action) = match output.required.as_str() {
        "no_changes" => {
            let media = output.media.as_ref().map(format_media).unwrap_or_default();
            if output.server_message.is_empty() {
                (format!("Official sync complete{media}"), false)
            } else {
                (
                    format!("Official sync complete · {}{media}", output.server_message),
                    false,
                )
            }
        }
        "full_download" => (
            "Anki requires a full download. Verify AnkiWeb is correct, then press F2.".into(),
            true,
        ),
        "full_upload" => (
            "Anki requires a full upload. Press F3 only if this TUI collection is authoritative."
                .into(),
            true,
        ),
        "full_sync" => (
            "Anki found a conflict. Choose F2 (keep AnkiWeb) or F3 (keep this collection).".into(),
            true,
        ),
        "normal_sync" => (
            "Anki returned an unexpected intermediate sync state; no completion was assumed."
                .into(),
            true,
        ),
        other => (
            format!("Anki returned an unknown sync requirement: {other}"),
            true,
        ),
    };
    Ok(SyncResult {
        message,
        requires_user_action,
    })
}

pub fn media_only_sync(
    on_progress: impl FnMut(crate::anki_backend::BackendProgress),
) -> Result<SyncResult> {
    require_official_collection()?;
    let auth = require_auth()?;
    let media = crate::anki_backend::media_sync(&auth, on_progress)?;
    Ok(SyncResult {
        message: format!("Official media sync complete{}", format_media(&media)),
        requires_user_action: false,
    })
}

fn format_full(label: &str, output: &FullSyncOutput) -> String {
    format!(
        "{label} · {} cards{}",
        output.cards,
        format_media(&output.media)
    )
}

fn format_media(media: &MediaProgress) -> String {
    if media.added.is_empty() && media.removed.is_empty() && media.checked.is_empty() {
        String::new()
    } else {
        format!(
            " · media added {} removed {} checked {}",
            media.added, media.removed, media.checked
        )
    }
}

pub fn auth_status() -> String {
    let collection: String = if paths::official_ready() && !crate::anki_backend::available() {
        "official collection present, but the Anki Python package is unavailable".into()
    } else if paths::official_ready() {
        "official Anki collection ready".into()
    } else if paths::anki2_exists() {
        "legacy shadow blocked (fresh F2 download required)".into()
    } else {
        "official collection not initialized (F2 download required)".into()
    };
    match load_auth() {
        Ok(Some(auth)) => {
            let endpoint = if auth.endpoint.is_empty() {
                "AnkiWeb"
            } else {
                &auth.endpoint
            };
            format!("Signed in · {endpoint} · {collection}")
        }
        Ok(None) => format!("Not signed in · {collection}"),
        Err(error) => format!("Auth error: {error} · {collection}"),
    }
}
