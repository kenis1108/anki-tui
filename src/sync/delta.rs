use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sync::client::AnkiWebClient;

const CHUNK_SIZE: usize = 250;
const NOTE_COLS: &str =
    "id, guid, mid, mod, usn, tags, flds, sfld, csum, flags, data";
const CARD_COLS: &str =
    "id, nid, did, ord, mod, usn, type, queue, due, ivl, factor, reps, lapses, left, odue, odid, flags, data";
const REVLOG_COLS: &str = "id, cid, usn, ease, ivl, lastIvl, factor, time, type";

#[derive(Debug, Clone)]
pub enum DeltaOutcome {
    NoChanges,
    Success { new_mod: i64 },
    FullSyncRequired { reason: String },
}

#[derive(Debug, Deserialize)]
struct MetaWire {
    #[serde(default, rename = "mod")]
    modification: i64,
    #[serde(default)]
    scm: i64,
    #[serde(default)]
    usn: i32,
    #[serde(default)]
    msg: String,
    #[serde(default = "default_true")]
    cont: bool,
}

fn default_true() -> bool {
    true
}

pub fn run_delta_sync(anki2: &Path, client: &mut AnkiWebClient) -> Result<DeltaOutcome> {
    let conn = Connection::open(anki2).context("open shadow collection.anki2")?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    let (local_mod, local_scm, local_usn) = conn.query_row(
        "SELECT mod, scm, usn FROM col WHERE id = 1",
        [],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i32>(2)?)),
    )?;

    let remote_raw = client.meta_raw()?;
    let remote: MetaWire = serde_json::from_slice(&remote_raw)?;
    if !remote.cont {
        bail!("server refused sync: {}", remote.msg);
    }
    if !remote.msg.is_empty() {
        // informational
    }

    if remote.modification == local_mod {
        return Ok(DeltaOutcome::NoChanges);
    }
    if remote.scm != local_scm {
        return Ok(DeltaOutcome::FullSyncRequired {
            reason: format!(
                "schema differs (local scm {local_scm}, remote {})",
                remote.scm
            ),
        });
    }

    let local_newer = local_mod > remote.modification;
    let server_usn = remote.usn;
    let schema18 = has_table(&conn, "notetypes")?;

    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<DeltaOutcome> {
        let graves = client.json_method(
            "start",
            json!({ "minUsn": local_usn, "lnewer": local_newer }),
        )?;
        apply_graves(&conn, &graves, server_usn, schema18)?;

        // send local graves in chunks
        let pending = take_pending_graves(&conn, server_usn)?;
        for chunk in chunk_graves(pending) {
            client.json_method("applyGraves", json!({ "chunk": chunk }))?;
        }

        let local_changes = build_local_changes(&conn, server_usn, local_newer, schema18)?;
        let remote_changes =
            client.json_method("applyChanges", json!({ "changes": local_changes }))?;
        apply_remote_changes(&conn, &remote_changes, server_usn, schema18)?;

        // download chunks
        loop {
            let chunk = client.json_method("chunk", json!({}))?;
            apply_chunk(&conn, &chunk)?;
            if chunk.get("done").and_then(|v| v.as_bool()).unwrap_or(false) {
                break;
            }
        }

        // upload chunks
        let mut ids = pending_ids(&conn)?;
        loop {
            let chunk = take_chunk(&conn, &mut ids, server_usn)?;
            let done = chunk
                .get("done")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            client.json_method("applyChunk", json!({ "chunk": chunk }))?;
            if done {
                break;
            }
        }

        let client_counts = sanity_counts(&conn, schema18)?;
        let sanity = client.json_method("sanityCheck2", json!({ "client": client_counts }))?;
        let status = sanity
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if status != "ok" {
            let now_ms = now_ms();
            conn.execute("UPDATE col SET scm = ?1", params![now_ms])?;
            bail!("sanity check failed: {sanity}");
        }

        let finish_raw = client.request_json_or_int("finish", json!({}))?;
        let new_mod = match finish_raw {
            FinishParse::Int(n) => n,
            FinishParse::Json(v) => v.as_i64().unwrap_or(now_ms()),
        };
        conn.execute(
            "UPDATE col SET mod = ?1, ls = ?1, usn = ?2",
            params![new_mod, server_usn + 1],
        )?;
        Ok(DeltaOutcome::Success { new_mod })
    })();

    match &result {
        Ok(_) => {
            conn.execute_batch("COMMIT;")?;
        }
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK;");
            let _ = client.json_method("abort", json!({}));
        }
    }

    match result {
        Ok(o) => Ok(o),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("sanity check failed") {
                Ok(DeltaOutcome::FullSyncRequired { reason: msg })
            } else {
                Err(e)
            }
        }
    }
}

pub enum FinishParse {
    Int(i64),
    Json(Value),
}

fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn apply_graves(conn: &Connection, graves: &Value, new_usn: i32, schema18: bool) -> Result<()> {
    let cards = graves
        .get("cards")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let notes = graves
        .get("notes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let decks = graves
        .get("decks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for c in cards {
        let id = c.as_i64().unwrap_or(0);
        conn.execute("DELETE FROM cards WHERE id = ?1", params![id])?;
        conn.execute(
            "INSERT OR REPLACE INTO graves (usn, oid, type) VALUES (?1, ?2, 0)",
            params![new_usn, id],
        )?;
    }
    for n in notes {
        let id = n.as_i64().unwrap_or(0);
        conn.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        conn.execute(
            "INSERT OR REPLACE INTO graves (usn, oid, type) VALUES (?1, ?2, 1)",
            params![new_usn, id],
        )?;
    }
    for d in decks {
        let id = d.as_i64().unwrap_or(0);
        if schema18 {
            conn.execute("DELETE FROM decks WHERE id = ?1", params![id])?;
        } else {
            // schema11: remove from col.decks JSON
            let mut decks_json: Value = conn.query_row("SELECT decks FROM col WHERE id=1", [], |r| {
                let s: String = r.get(0)?;
                Ok(serde_json::from_str(&s).unwrap_or(json!({})))
            })?;
            if let Some(obj) = decks_json.as_object_mut() {
                obj.remove(&id.to_string());
            }
            conn.execute(
                "UPDATE col SET decks = ?1 WHERE id = 1",
                params![decks_json.to_string()],
            )?;
        }
        conn.execute(
            "INSERT OR REPLACE INTO graves (usn, oid, type) VALUES (?1, ?2, 2)",
            params![new_usn, id],
        )?;
    }
    Ok(())
}

fn take_pending_graves(conn: &Connection, new_usn: i32) -> Result<Value> {
    let mut cards = Vec::new();
    let mut notes = Vec::new();
    let mut decks = Vec::new();
    let mut stmt = conn.prepare("SELECT oid, type FROM graves WHERE usn = -1")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i32>(1)?)))?;
    for row in rows {
        let (oid, typ) = row?;
        match typ {
            0 => cards.push(oid),
            1 => notes.push(oid),
            2 => decks.push(oid),
            _ => {}
        }
    }
    conn.execute("UPDATE graves SET usn = ?1 WHERE usn = -1", params![new_usn])?;
    Ok(json!({ "cards": cards, "notes": notes, "decks": decks }))
}

fn chunk_graves(g: Value) -> Vec<Value> {
    let cards = g
        .get("cards")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let notes = g
        .get("notes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let decks = g
        .get("decks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut items: Vec<(&str, i64)> = Vec::new();
    for c in cards {
        if let Some(id) = c.as_i64() {
            items.push(("cards", id));
        }
    }
    for n in notes {
        if let Some(id) = n.as_i64() {
            items.push(("notes", id));
        }
    }
    for d in decks {
        if let Some(id) = d.as_i64() {
            items.push(("decks", id));
        }
    }
    if items.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for chunk in items.chunks(CHUNK_SIZE) {
        let mut c = json!({ "cards": [], "notes": [], "decks": [] });
        for (k, id) in chunk {
            c[*k].as_array_mut().unwrap().push(json!(id));
        }
        out.push(c);
    }
    out
}

fn build_local_changes(
    conn: &Connection,
    new_usn: i32,
    local_newer: bool,
    schema18: bool,
) -> Result<Value> {
    // For schema18 we skip uploading notetype/deck protobuf conversions;
    // note/card/revlog dirty rows go via chunks. Still bump usn on pending tags if any.
    let mut changes = json!({
        "models": [],
        "decks": [[], []],
        "tags": []
    });

    if !schema18 {
        // schema11: export dirty models/decks from col JSON where usn==-1 is not tracked;
        // send empty — chunk path covers notes/cards.
    }

    if local_newer {
        if has_table(conn, "config")? {
            // schema18 config table
            let mut conf = json!({});
            let mut stmt = conn.prepare("SELECT key, val FROM config")?;
            let rows = stmt.query_map([], |r| {
                let k: String = r.get(0)?;
                let v: Vec<u8> = r.get(1)?;
                Ok((k, v))
            })?;
            for row in rows.flatten() {
                if let Ok(val) = serde_json::from_slice::<Value>(&row.1) {
                    conf[row.0] = val;
                }
            }
            changes["conf"] = conf;
        } else {
            let conf_s: String =
                conn.query_row("SELECT conf FROM col WHERE id=1", [], |r| r.get(0))?;
            changes["conf"] = serde_json::from_str(&conf_s).unwrap_or(json!({}));
        }
        let crt: i64 = conn.query_row("SELECT crt FROM col WHERE id=1", [], |r| r.get(0))?;
        changes["crt"] = json!(crt);
    }

    let _ = new_usn;
    Ok(changes)
}

fn apply_remote_changes(
    conn: &Connection,
    ch: &Value,
    new_usn: i32,
    schema18: bool,
) -> Result<()> {
    // Models: schema11 merge into col.models; schema18 skip structural (full sync if needed)
    if let Some(models) = ch.get("models").and_then(|v| v.as_array()) {
        if !models.is_empty() {
            if schema18 {
                // structural notetype change — mark for full sync next time by bumping scm lightly
                // but still continue note sync
            } else {
                let mut models_json: Value =
                    conn.query_row("SELECT models FROM col WHERE id=1", [], |r| {
                        let s: String = r.get(0)?;
                        Ok(serde_json::from_str(&s).unwrap_or(json!({})))
                    })?;
                for m in models {
                    if let Some(id) = m.get("id") {
                        let key = id.to_string();
                        if let Some(obj) = models_json.as_object_mut() {
                            obj.insert(key, m.clone());
                        }
                    }
                }
                conn.execute(
                    "UPDATE col SET models = ?1 WHERE id = 1",
                    params![models_json.to_string()],
                )?;
            }
        }
    }

    if let Some(decks_pair) = ch.get("decks").and_then(|v| v.as_array()) {
        if let Some(decks) = decks_pair.first().and_then(|v| v.as_array()) {
            if !schema18 {
                let mut decks_json: Value =
                    conn.query_row("SELECT decks FROM col WHERE id=1", [], |r| {
                        let s: String = r.get(0)?;
                        Ok(serde_json::from_str(&s).unwrap_or(json!({})))
                    })?;
                for d in decks {
                    if let Some(id) = d.get("id") {
                        let key = id.as_i64().unwrap_or(0).to_string();
                        if let Some(obj) = decks_json.as_object_mut() {
                            obj.insert(key, d.clone());
                        }
                    }
                }
                conn.execute(
                    "UPDATE col SET decks = ?1 WHERE id = 1",
                    params![decks_json.to_string()],
                )?;
            } else {
                // update existing deck names when possible
                for d in decks {
                    if let (Some(id), Some(name)) = (
                        d.get("id").and_then(|v| v.as_i64()),
                        d.get("name").and_then(|v| v.as_str()),
                    ) {
                        let name = name.replace("::", "\u{1f}");
                        let mod_ = d.get("mod").and_then(|v| v.as_i64()).unwrap_or(0);
                        conn.execute(
                            "UPDATE decks SET name = ?1, mtime_secs = ?2, usn = ?3 WHERE id = ?4",
                            params![name, mod_, new_usn, id],
                        )?;
                    }
                }
            }
        }
    }

    if let Some(tags) = ch.get("tags").and_then(|v| v.as_array()) {
        if has_table(conn, "tags")? {
            for t in tags {
                if let Some(tag) = t.as_str() {
                    conn.execute(
                        "INSERT INTO tags (tag, usn) VALUES (?1, ?2)
                         ON CONFLICT(tag) DO UPDATE SET usn=excluded.usn",
                        params![tag, new_usn],
                    )?;
                }
            }
        }
    }

    if let Some(conf) = ch.get("conf") {
        if has_table(conn, "config")? {
            conn.execute("DELETE FROM config", [])?;
            if let Some(obj) = conf.as_object() {
                for (k, v) in obj {
                    conn.execute(
                        "INSERT INTO config (key, usn, mtime_secs, val) VALUES (?1, 0, 0, ?2)",
                        params![k, serde_json::to_vec(v)?],
                    )?;
                }
            }
        } else {
            conn.execute(
                "UPDATE col SET conf = ?1 WHERE id = 1",
                params![conf.to_string()],
            )?;
        }
    }
    if let Some(crt) = ch.get("crt").and_then(|v| v.as_i64()) {
        conn.execute("UPDATE col SET crt = ?1 WHERE id = 1", params![crt])?;
    }
    Ok(())
}

fn apply_chunk(conn: &Connection, chunk: &Value) -> Result<()> {
    if let Some(rows) = chunk.get("revlog").and_then(|v| v.as_array()) {
        for r in rows {
            let a = row_array(r);
            if a.len() >= 9 {
                conn.execute(
                    "INSERT OR IGNORE INTO revlog (id, cid, usn, ease, ivl, lastIvl, factor, time, type)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        as_i64(&a[0]),
                        as_i64(&a[1]),
                        as_i64(&a[2]),
                        as_i64(&a[3]),
                        as_i64(&a[4]),
                        as_i64(&a[5]),
                        as_i64(&a[6]),
                        as_i64(&a[7]),
                        as_i64(&a[8])
                    ],
                )?;
            }
        }
    }
    if let Some(rows) = chunk.get("cards").and_then(|v| v.as_array()) {
        for r in rows {
            let a = row_array(r);
            if a.len() >= 18 {
                let id = as_i64(&a[0]);
                let cur: Option<(i32, i64)> = conn
                    .query_row(
                        "SELECT usn, mod FROM cards WHERE id = ?1",
                        params![id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .ok();
                if let Some((usn, mod_)) = cur {
                    if usn == -1 && mod_ >= as_i64(&a[4]) {
                        continue;
                    }
                }
                conn.execute(
                    "INSERT OR REPLACE INTO cards
                     (id,nid,did,ord,mod,usn,type,queue,due,ivl,factor,reps,lapses,left,odue,odid,flags,data)
                     VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    params![
                        as_i64(&a[0]),
                        as_i64(&a[1]),
                        as_i64(&a[2]),
                        as_i64(&a[3]),
                        as_i64(&a[4]),
                        as_i64(&a[5]),
                        as_i64(&a[6]),
                        as_i64(&a[7]),
                        as_i64(&a[8]),
                        as_i64(&a[9]),
                        as_i64(&a[10]),
                        as_i64(&a[11]),
                        as_i64(&a[12]),
                        as_i64(&a[13]),
                        as_i64(&a[14]),
                        as_i64(&a[15]),
                        as_i64(&a[16]),
                        as_str(&a[17])
                    ],
                )?;
            }
        }
    }
    if let Some(rows) = chunk.get("notes").and_then(|v| v.as_array()) {
        for r in rows {
            let a = row_array(r);
            if a.len() >= 11 {
                let id = as_i64(&a[0]);
                let cur: Option<(i32, i64)> = conn
                    .query_row(
                        "SELECT usn, mod FROM notes WHERE id = ?1",
                        params![id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .ok();
                if let Some((usn, mod_)) = cur {
                    if usn == -1 && mod_ >= as_i64(&a[3]) {
                        continue;
                    }
                }
                let flds = as_str(&a[6]);
                let first = flds.split('\u{1f}').next().unwrap_or("");
                conn.execute(
                    "INSERT OR REPLACE INTO notes
                     (id,guid,mid,mod,usn,tags,flds,sfld,csum,flags,data)
                     VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                    params![
                        as_i64(&a[0]),
                        as_str(&a[1]),
                        as_i64(&a[2]),
                        as_i64(&a[3]),
                        as_i64(&a[4]),
                        as_str(&a[5]),
                        flds,
                        first,
                        field_csum(first),
                        as_i64(&a[9]),
                        as_str(&a[10])
                    ],
                )?;
            }
        }
    }
    Ok(())
}

fn as_i64(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

fn as_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string().trim_matches('"').to_string(),
    }
}

fn row_array(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn field_csum(data: &str) -> i64 {
    use sha1::{Digest, Sha1};
    let digest = Sha1::digest(data.as_bytes());
    let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    i64::from_str_radix(&hex[..8], 16).unwrap_or(0)
}

fn pending_ids(conn: &Connection) -> Result<HashMap<&'static str, Vec<i64>>> {
    let mut map = HashMap::new();
    for (table, key) in [("revlog", "revlog"), ("cards", "cards"), ("notes", "notes")] {
        let mut stmt = conn.prepare(&format!("SELECT id FROM {table} WHERE usn = -1"))?;
        let ids: Vec<i64> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        map.insert(key, ids);
    }
    Ok(map)
}

fn take_chunk(
    conn: &Connection,
    ids: &mut HashMap<&'static str, Vec<i64>>,
    new_usn: i32,
) -> Result<Value> {
    let mut chunk = json!({});
    let mut n = 0usize;
    for (table, cols) in [
        ("revlog", REVLOG_COLS),
        ("cards", CARD_COLS),
        ("notes", NOTE_COLS),
    ] {
        let list = ids.get_mut(table).unwrap();
        let take_n = (CHUNK_SIZE - n).min(list.len());
        let take: Vec<i64> = list.drain(..take_n).collect();
        n += take.len();
        let mut rows = Vec::new();
        for oid in &take {
            let row: Vec<Value> = {
                let sql = format!("SELECT {cols} FROM {table} WHERE id = ?1");
                conn.query_row(&sql, params![oid], |r| {
                    let width = cols.split(',').count();
                    let mut vals = Vec::with_capacity(width);
                    for i in 0..width {
                        vals.push(value_from_sql(r, i)?);
                    }
                    Ok(vals)
                })?
            };
            // set usn column
            let mut row = row;
            let usn_idx = cols.split(", ").position(|c| c == "usn").unwrap_or(2);
            if usn_idx < row.len() {
                row[usn_idx] = json!(new_usn);
            }
            if table == "notes" && row.len() > 8 {
                row[7] = json!("");
                row[8] = json!("");
            }
            rows.push(Value::Array(row));
        }
        if !rows.is_empty() {
            chunk[table] = Value::Array(rows);
        }
        for oid in take {
            conn.execute(
                &format!("UPDATE {table} SET usn = ?1 WHERE id = ?2"),
                params![new_usn, oid],
            )?;
        }
    }
    let done = ids.values().all(|v| v.is_empty());
    chunk["done"] = json!(done);
    Ok(chunk)
}

fn value_from_sql(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Value> {
    let v = row.get_ref(idx)?;
    Ok(match v {
        rusqlite::types::ValueRef::Null => Value::Null,
        rusqlite::types::ValueRef::Integer(i) => json!(i),
        rusqlite::types::ValueRef::Real(f) => json!(f),
        rusqlite::types::ValueRef::Text(t) => {
            json!(String::from_utf8_lossy(t).to_string())
        }
        rusqlite::types::ValueRef::Blob(b) => {
            // store as empty string for wire (data fields)
            let _ = b;
            json!("")
        }
    })
}

fn sanity_counts(conn: &Connection, schema18: bool) -> Result<Value> {
    let count = |t: &str| -> Result<i64> {
        Ok(conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))?)
    };
    let mut arr = vec![
        json!([0, 0, 0]),
        json!(count("cards")?),
        json!(count("notes")?),
        json!(count("revlog")?),
        json!(count("graves")?),
    ];
    if schema18 {
        arr.push(json!(count("notetypes")?));
        arr.push(json!(count("decks")?));
        arr.push(json!(count("deck_config")?));
    } else {
        arr.push(json!(0));
        arr.push(json!(0));
        arr.push(json!(0));
    }
    Ok(Value::Array(arr))
}
