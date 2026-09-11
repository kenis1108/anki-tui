use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::models::{BrowseRow, Deck, DeckCounts, DeckOptions, NoteField, StatsSummary, StudyCard};

const HELPER: &str = include_str!("../scripts/anki_backend.py");
const PROGRESS_PREFIX: &str = "ANKI_TUI_PROGRESS ";

#[derive(Debug, Deserialize)]
struct Response {
    ok: bool,
    #[serde(default)]
    result: Value,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Health {
    pub anki_version: String,
    pub cards: i64,
}

#[derive(Debug, Deserialize)]
pub struct LoginOutput {
    pub hkey: String,
    pub endpoint: String,
}

#[derive(Debug, Deserialize)]
pub struct MediaProgress {
    pub added: String,
    pub removed: String,
    pub checked: String,
}

#[derive(Debug, Deserialize)]
pub struct OfficialSyncOutput {
    pub required: String,
    pub server_message: String,
    pub new_endpoint: String,
    pub media: Option<MediaProgress>,
}

#[derive(Debug, Deserialize)]
pub struct FullSyncOutput {
    pub cards: i64,
    pub media: MediaProgress,
    pub new_endpoint: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendProgress {
    pub stage: String,
    #[serde(default)]
    pub current: u64,
    pub total: Option<u64>,
    #[serde(default)]
    pub detail: String,
}

/// Anki may print diagnostics to stdout (e.g. "blocked main thread" stack
/// traces). Prefer the last JSON object that looks like our protocol reply.
fn extract_response_json(output: &[u8]) -> &[u8] {
    const MARKER: &[u8] = b"{\"ok\":";
    let start = output
        .windows(MARKER.len())
        .rposition(|window| window == MARKER)
        .unwrap_or(0);
    let mut end = output.len();
    while end > start && matches!(output[end - 1], b' ' | b'\n' | b'\r' | b'\t') {
        end -= 1;
    }
    &output[start..end]
}

fn decode_response<T: DeserializeOwned>(output: &[u8]) -> Result<T> {
    let payload = extract_response_json(output);
    let response: Response = serde_json::from_slice(payload).with_context(|| {
        format!(
            "parse official Anki backend response: {}",
            String::from_utf8_lossy(output).trim()
        )
    })?;
    if !response.ok {
        bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "official Anki backend failed".into())
        );
    }
    serde_json::from_value(response.result).context("decode official Anki backend result")
}

fn call<T: DeserializeOwned>(action: &str, args: Value) -> Result<T> {
    let collection = crate::sync::paths::anki2_path()?;
    call_at(&collection, action, args)
}

fn call_at<T: DeserializeOwned>(collection: &Path, action: &str, args: Value) -> Result<T> {
    let input = serde_json::to_vec(&json!({
        "action": action,
        "collection": collection,
        "args": args,
    }))?;

    let mut child = Command::new("python3")
        .arg("-c")
        .arg(HELPER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start official Anki backend (python3)")?;
    child
        .stdin
        .take()
        .context("open Anki backend stdin")?
        .write_all(&input)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "official Anki backend exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    decode_response(&output.stdout)
}

fn call_with_progress<T: DeserializeOwned>(
    action: &str,
    args: Value,
    on_progress: impl FnMut(BackendProgress),
) -> Result<T> {
    let collection = crate::sync::paths::anki2_path()?;
    call_at_with_progress(&collection, action, args, on_progress)
}

fn call_at_with_progress<T: DeserializeOwned>(
    collection: &Path,
    action: &str,
    args: Value,
    mut on_progress: impl FnMut(BackendProgress),
) -> Result<T> {
    let input = serde_json::to_vec(&json!({
        "action": action,
        "collection": collection,
        "args": args,
    }))?;

    let mut child = Command::new("python3")
        .arg("-c")
        .arg(HELPER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start official Anki backend (python3)")?;
    child
        .stdin
        .take()
        .context("open Anki backend stdin")?
        .write_all(&input)?;

    let mut stderr_output = String::new();
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines() {
            let line = line.context("read official Anki backend progress")?;
            if let Some(payload) = line.strip_prefix(PROGRESS_PREFIX) {
                match serde_json::from_str(payload) {
                    Ok(progress) => on_progress(progress),
                    Err(error) => {
                        stderr_output
                            .push_str(&format!("invalid backend progress ({error}): {payload}\n"));
                    }
                }
            } else {
                stderr_output.push_str(&line);
                stderr_output.push('\n');
            }
        }
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "official Anki backend exited with {}: {}",
            output.status,
            stderr_output.trim()
        );
    }
    decode_response(&output.stdout)
}

pub fn health() -> Result<Health> {
    call("health", json!({}))
}

pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("python3")
            .args(["-c", "import anki"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

pub fn login(username: &str, password: &str, endpoint: Option<&str>) -> Result<LoginOutput> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let root = std::env::temp_dir().join(format!("anki-tui-login-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&root)?;
    let collection = root.join("collection.anki2");
    let result = call_at(
        &collection,
        "login",
        json!({ "username": username, "password": password, "endpoint": endpoint }),
    );
    let _ = std::fs::remove_dir_all(root);
    result
}

pub fn list_deck_counts() -> Result<Vec<DeckCounts>> {
    call("deck_tree", json!({}))
}

pub fn get_deck(deck_id: i64) -> Result<Option<Deck>> {
    call("get_deck", json!({ "deck_id": deck_id }))
}

pub fn create_deck(name: &str) -> Result<i64> {
    call("create_deck", json!({ "name": name }))
}

pub fn rename_deck(deck_id: i64, name: &str) -> Result<()> {
    call("rename_deck", json!({ "deck_id": deck_id, "name": name }))
}

pub fn delete_deck(deck_id: i64) -> Result<()> {
    call("delete_deck", json!({ "deck_id": deck_id }))
}

pub fn update_deck_options(deck_id: i64, options: &DeckOptions) -> Result<()> {
    call(
        "update_deck_options",
        json!({
            "deck_id": deck_id,
            "new_per_day": options.new_per_day,
            "rev_per_day": options.rev_per_day,
        }),
    )
}

pub fn add_note(deck_id: i64, front: &str, back: &str, tags: &str) -> Result<i64> {
    call(
        "add_note",
        json!({ "deck_id": deck_id, "front": front, "back": back, "tags": tags }),
    )
}

pub fn update_note(note_id: i64, fields: &[NoteField], tags: &str) -> Result<()> {
    call(
        "update_note",
        json!({ "note_id": note_id, "fields": fields, "tags": tags }),
    )
}

pub fn add_media_file(filename: &str, data: &[u8]) -> Result<String> {
    call(
        "add_media",
        json!({ "filename": filename, "data_hex": hex::encode(data) }),
    )
}

pub fn delete_card(card_id: i64) -> Result<()> {
    call("delete_card", json!({ "card_id": card_id }))
}

pub fn set_card_suspended(card_id: i64, suspended: bool) -> Result<()> {
    call(
        "set_suspended",
        json!({ "card_id": card_id, "suspended": suspended }),
    )
}

pub fn forget_card(card_id: i64) -> Result<()> {
    call("forget_card", json!({ "card_id": card_id }))
}

pub fn forget_deck(deck_id: i64) -> Result<usize> {
    call("forget_deck", json!({ "deck_id": deck_id }))
}

pub fn next_study_queue(deck_id: i64, limit: usize) -> Result<Vec<StudyCard>> {
    call("study_queue", json!({ "deck_id": deck_id, "limit": limit }))
}

pub fn answer_card(
    card_id: i64,
    rating: i32,
    milliseconds_taken: u128,
    scheduling_states_hex: &str,
    deck_id: i64,
    limit: usize,
) -> Result<Vec<StudyCard>> {
    call(
        "answer_card",
        json!({
            "card_id": card_id,
            "rating": rating,
            "milliseconds_taken": milliseconds_taken.min(i64::MAX as u128) as i64,
            "scheduling_states_hex": scheduling_states_hex,
            "deck_id": deck_id,
            "limit": limit,
        }),
    )
}

pub fn browse(query: &str, limit: i64) -> Result<Vec<BrowseRow>> {
    call("browse", json!({ "query": query, "limit": limit }))
}

pub fn stats() -> Result<StatsSummary> {
    call("stats", json!({}))
}

fn auth_args(auth: &crate::sync::Auth) -> Value {
    json!({ "hkey": auth.hkey, "endpoint": auth.endpoint })
}

pub fn normal_sync(
    auth: &crate::sync::Auth,
    on_progress: impl FnMut(BackendProgress),
) -> Result<OfficialSyncOutput> {
    call_with_progress("normal_sync", auth_args(auth), on_progress)
}

pub fn full_download(
    auth: &crate::sync::Auth,
    on_progress: impl FnMut(BackendProgress),
) -> Result<FullSyncOutput> {
    call_with_progress("full_download", auth_args(auth), on_progress)
}

pub fn full_upload(
    auth: &crate::sync::Auth,
    on_progress: impl FnMut(BackendProgress),
) -> Result<FullSyncOutput> {
    call_with_progress("full_upload", auth_args(auth), on_progress)
}

pub fn media_sync(
    auth: &crate::sync::Auth,
    on_progress: impl FnMut(BackendProgress),
) -> Result<MediaProgress> {
    call_with_progress("media_sync", auth_args(auth), on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn backend_error_does_not_require_result_field() {
        let error =
            decode_response::<Value>(br#"{"ok":false,"error":"SyncError: missing original size"}"#)
                .unwrap_err();

        assert_eq!(error.to_string(), "SyncError: missing original size");
    }

    #[test]
    fn decode_response_ignores_anki_stdout_diagnostics() {
        let polluted = b"blocked main thread for 1460ms:\n  File \"<string>\", line 1\n{\"ok\":true,\"result\":{\"hkey\":\"secret\",\"endpoint\":\"\"}}\n";
        let login: LoginOutput = decode_response(polluted).unwrap();
        assert_eq!(login.hkey, "secret");
        assert_eq!(login.endpoint, "");
    }

    #[test]
    fn backend_progress_message_decodes() {
        let progress: BackendProgress = serde_json::from_str(
            r#"{"stage":"Downloading collection","current":512,"total":1024,"detail":""}"#,
        )
        .unwrap();

        assert_eq!(progress.stage, "Downloading collection");
        assert_eq!(progress.current, 512);
        assert_eq!(progress.total, Some(1024));
    }

    #[test]
    fn official_backend_roundtrip_when_available() {
        if !available() {
            return;
        }

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "anki-tui-official-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let collection = root.join("collection.anki2");

        let health: Health = call_at(&collection, "health", json!({})).unwrap();
        assert_eq!(health.cards, 0);
        let deck_id: i64 =
            call_at(&collection, "create_deck", json!({ "name": "Bridge Test" })).unwrap();
        let _: i64 = call_at(
            &collection,
            "add_note",
            json!({
                "deck_id": deck_id,
                "front": "Question",
                "back": "Answer",
                "tags": "bridge",
            }),
        )
        .unwrap();

        let queue: Vec<StudyCard> = call_at(
            &collection,
            "study_queue",
            json!({ "deck_id": deck_id, "limit": 10 }),
        )
        .unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].answer_intervals.len(), 4);
        assert_eq!(queue[0].fields.len(), 2);

        let mut fields = queue[0].fields.clone();
        fields[1].value = "Updated answer".into();
        let _: () = call_at(
            &collection,
            "update_note",
            json!({
                "note_id": queue[0].card.note_id,
                "fields": fields,
                "tags": "bridge safe",
            }),
        )
        .unwrap();
        let rows: Vec<BrowseRow> =
            call_at(&collection, "browse", json!({ "query": "", "limit": 10 })).unwrap();
        assert_eq!(rows[0].fields[1].value, "Updated answer");
        assert_eq!(rows[0].tags, "bridge safe");

        let _after_answer: Vec<StudyCard> = call_at(
            &collection,
            "answer_card",
            json!({
                "card_id": queue[0].card.id,
                "rating": 3,
                "milliseconds_taken": 1234,
                "scheduling_states_hex": queue[0].scheduling_states_hex,
                "deck_id": deck_id,
                "limit": 10,
            }),
        )
        .unwrap();
        let rows: Vec<BrowseRow> =
            call_at(&collection, "browse", json!({ "query": "", "limit": 10 })).unwrap();
        assert_eq!(rows[0].reps, 1);

        std::fs::remove_dir_all(root).unwrap();
    }
}
