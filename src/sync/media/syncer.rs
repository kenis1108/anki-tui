use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::json;
use serde_tuple::Deserialize_tuple;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use super::db::{folder_mtime, media_file_path, sha1_hex, MediaDb, MediaEntry};
use super::ziputil::{unzip_download, zip_for_upload};
use crate::sync::client::AnkiWebClient;
use crate::sync::paths;

const MAX_ZIP_FILES: u32 = 25;

#[derive(Debug, Default, Clone)]
pub struct MediaSyncStats {
    pub downloaded: usize,
    pub uploaded: usize,
    pub deleted: usize,
    pub checked: usize,
}

#[derive(Debug, Deserialize_tuple)]
struct MediaChange {
    fname: String,
    usn: i32,
    sha1: String,
}

#[derive(Debug, Deserialize)]
struct BeginData {
    usn: i32,
    #[serde(default)]
    sk: String,
}

#[derive(Debug, Deserialize_tuple)]
struct UploadReply {
    processed: usize,
    current_usn: i32,
}

pub fn sync_media(client: &mut AnkiWebClient) -> Result<MediaSyncStats> {
    let (folder, db_path) = paths::ensure_media_layout()?;
    let db = MediaDb::open(&db_path)?;
    let checked = db.register_folder(&folder)?;
    let mut stats = MediaSyncStats {
        checked,
        ..Default::default()
    };

    let begin: BeginData = client.msync_json("begin", json!({ "v": "anki,25.09 (anki-tui),rust" }))?;
    let server_usn = begin.usn;
    let mut meta = db.get_meta()?;

    if meta.last_sync_usn != server_usn {
        fetch_changes(client, &db, &folder, &mut meta, &mut stats)?;
    }

    if !db.pending_uploads(1)?.is_empty() {
        send_changes(client, &db, &folder, &mut meta, &mut stats)?;
    }

    if stats.downloaded > 0 || stats.uploaded > 0 || stats.deleted > 0 {
        finalize(client, &db)?;
    }

    let _ = begin.sk;
    Ok(stats)
}

fn fetch_changes(
    client: &mut AnkiWebClient,
    db: &MediaDb,
    folder: &Path,
    meta: &mut super::db::MediaMeta,
    stats: &mut MediaSyncStats,
) -> Result<()> {
    let mut last_usn = meta.last_sync_usn;
    loop {
        let batch: Vec<MediaChange> =
            client.msync_json("mediaChanges", json!({ "lastUsn": last_usn }))?;
        if batch.is_empty() {
            break;
        }
        last_usn = batch.last().map(|b| b.usn).unwrap_or(last_usn);
        stats.checked += batch.len();

        let mut to_download = Vec::new();
        let mut to_delete = Vec::new();
        let mut to_clean = Vec::new();

        for remote in &batch {
            let (local_sha, pending) = match db.get_entry(&remote.fname)? {
                Some(e) => (e.sha1.unwrap_or_default(), e.dirty),
                None => (String::new(), false),
            };
            match decide(&local_sha, &remote.sha1, pending) {
                Action::Download => to_download.push(remote.fname.clone()),
                Action::Delete => to_delete.push(remote.fname.clone()),
                Action::Clean => to_clean.push(remote.fname.clone()),
                Action::None => {}
            }
        }

        for fname in &to_delete {
            let path = media_file_path(folder, fname);
            let _ = fs::remove_file(path);
            db.remove_entry(fname)?;
            stats.deleted += 1;
        }
        db.mark_clean(&to_clean)?;

        let mut rest = to_download.as_slice();
        while !rest.is_empty() {
            let batch_names: Vec<String> = rest
                .iter()
                .take(MAX_ZIP_FILES as usize)
                .cloned()
                .collect();
            let zip_bytes = client.msync_bytes(
                "downloadFiles",
                json!({ "files": batch_names.clone() }),
            )?;
            // downloadFiles returns raw zip (possibly wrapped?)
            let files = match unzip_download(&zip_bytes) {
                Ok(f) => f,
                Err(_) => {
                    // try unwrap {data: base64?} — usually raw zip after zstd
                    bail!("failed to unzip media download");
                }
            };
            let n = files.len();
            for (fname, data) in files {
                let path = media_file_path(folder, &fname);
                fs::write(&path, &data)?;
                let meta_fs = fs::metadata(&path)?;
                db.set_entry(&MediaEntry {
                    fname,
                    sha1: Some(sha1_hex(&data)),
                    mtime: super::db::mtime_secs(&meta_fs),
                    dirty: false,
                })?;
                stats.downloaded += 1;
            }
            rest = &rest[n.min(rest.len())..];
            if n == 0 {
                // server returned nothing for remaining names — skip to avoid loop
                break;
            }
            let _ = batch_names;
        }

        meta.last_sync_usn = last_usn;
        meta.folder_mtime = folder_mtime(folder)?;
        db.set_meta(meta)?;
    }
    Ok(())
}

fn send_changes(
    client: &mut AnkiWebClient,
    db: &MediaDb,
    folder: &Path,
    meta: &mut super::db::MediaMeta,
    stats: &mut MediaSyncStats,
) -> Result<()> {
    loop {
        let pending = db.pending_uploads(MAX_ZIP_FILES)?;
        if pending.is_empty() {
            break;
        }
        let mut entries = Vec::new();
        for e in &pending {
            if let Some(ref sha) = e.sha1 {
                let path = media_file_path(folder, &e.fname);
                let data = fs::read(&path).with_context(|| format!("read media {}", e.fname))?;
                if sha1_hex(&data) != *sha {
                    // stale; refresh
                }
                entries.push((e.fname.clone(), Some(data)));
            } else {
                entries.push((e.fname.clone(), None));
            }
        }
        let zip = zip_for_upload(entries)?;
        let reply_val = client.msync_upload_zip(&zip)?;
        let reply: UploadReply = serde_json::from_value(reply_val)?;

        let processed: Vec<String> = pending
            .into_iter()
            .take(reply.processed)
            .map(|e| {
                if e.sha1.is_some() {
                    stats.uploaded += 1;
                } else {
                    stats.deleted += 1;
                }
                e.fname
            })
            .collect();
        db.mark_clean(&processed)?;

        let expected = meta.last_sync_usn + processed.len() as i32;
        if expected == reply.current_usn {
            meta.last_sync_usn = reply.current_usn;
            meta.folder_mtime = folder_mtime(folder)?;
            db.set_meta(meta)?;
        }
    }
    Ok(())
}

fn finalize(client: &mut AnkiWebClient, db: &MediaDb) -> Result<()> {
    let local = db.count()?;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Sanity {
        Text(String),
        Obj { status: String },
    }
    let resp: Sanity = client.msync_json("mediaSanity", json!({ "local": local }))?;
    let ok = match resp {
        Sanity::Text(s) => s == "OK" || s.eq_ignore_ascii_case("ok"),
        Sanity::Obj { status } => status == "OK" || status.eq_ignore_ascii_case("ok"),
    };
    if !ok {
        db.force_resync()?;
        bail!("media sanity check failed; will resync next time");
    }
    Ok(())
}

enum Action {
    None,
    Download,
    Delete,
    Clean,
}

fn decide(local_sha: &str, remote_sha: &str, pending: bool) -> Action {
    match (local_sha, remote_sha, pending) {
        ("", "", false) => Action::None,
        ("", "", true) => Action::Delete,
        ("", _, _) => Action::Download,
        (_, "", true) => Action::None,
        (_, "", false) => Action::Delete,
        (l, r, true) if l == r => Action::Clean,
        (l, r, _) if l == r => Action::None,
        _ => Action::Download,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::media::ziputil::{unzip_download, zip_for_upload};

    #[test]
    fn media_zip_roundtrip() {
        let zip = zip_for_upload(vec![
            ("a.png".into(), Some(b"hello".to_vec())),
            ("gone.jpg".into(), None),
        ])
        .unwrap();
        // upload format differs from download; just ensure zip builds
        assert!(zip.len() > 20);

        // download-style zip
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("0", opts).unwrap();
        use std::io::Write;
        zw.write_all(b"world").unwrap();
        zw.start_file("_meta", opts).unwrap();
        zw.write_all(br#"{"0":"b.png"}"#).unwrap();
        let data = zw.finish().unwrap().into_inner();
        let files = unzip_download(&data).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "b.png");
        assert_eq!(files[0].1, b"world");
    }

    #[test]
    fn change_actions() {
        assert!(matches!(decide("", "abc", false), Action::Download));
        assert!(matches!(decide("abc", "", false), Action::Delete));
        assert!(matches!(decide("abc", "abc", true), Action::Clean));
    }
}
