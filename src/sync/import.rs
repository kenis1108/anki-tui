use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::io::Write;

use crate::db::Store;

#[derive(Debug, Clone, Default)]
pub struct ImportStats {
    pub decks: usize,
    pub notes: usize,
}

#[derive(Debug, Clone)]
pub struct ImportedNote {
    pub deck_name: String,
    pub front: String,
    pub back: String,
    pub tags: String,
}

pub fn import_anki_collection_bytes(store: &Store, bytes: &[u8]) -> Result<ImportStats> {
    let dir = tempfile_dir()?;
    let path = dir.join("anki-download.db");
    {
        let mut f = std::fs::File::create(&path)?;
        f.write_all(bytes)?;
    }

    let conn = Connection::open(&path).context("open downloaded Anki collection")?;
    let notes = extract_notes(&conn)?;
    let deck_count = notes
        .iter()
        .map(|n| n.deck_name.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let note_count = notes.len();
    store.replace_with_imported(&notes)?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(ImportStats {
        decks: deck_count,
        notes: note_count,
    })
}

fn tempfile_dir() -> Result<std::path::PathBuf> {
    let base = std::env::temp_dir().join(format!("anki-tui-sync-{}", std::process::id()));
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

fn extract_notes(conn: &Connection) -> Result<Vec<ImportedNote>> {
    let tables: Vec<String> = {
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let has_decks_table = tables.iter().any(|t| t == "decks");

    let deck_names = if has_decks_table {
        load_decks_table(conn)?
    } else {
        load_decks_from_col(conn)?
    };

    let mut stmt = conn.prepare(
        "
        SELECT n.tags, n.flds, c.did
        FROM notes n
        JOIN cards c ON c.nid = n.id
        WHERE c.ord = 0
        ORDER BY n.id
        ",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (tags, flds, did) = row?;
        let fields: Vec<&str> = flds.split('\u{1f}').collect();
        if fields.is_empty() || fields[0].trim().is_empty() {
            continue;
        }
        let front = fields[0].to_string();
        let back = fields.get(1).copied().unwrap_or("").to_string();
        let deck_name = deck_names
            .get(&did)
            .cloned()
            .unwrap_or_else(|| "Default".into());
        out.push(ImportedNote {
            deck_name: normalize_deck_name(&deck_name),
            front,
            back,
            tags: tags.trim().to_string(),
        })
    }
    Ok(out)
}

fn normalize_deck_name(name: &str) -> String {
    name.replace('\u{1f}', "::")
}

fn load_decks_table(conn: &Connection) -> Result<HashMap<i64, String>> {
    let mut map = HashMap::new();
    let mut stmt = conn.prepare("SELECT id, name FROM decks")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (id, name) = row?;
        map.insert(id, name);
    }
    Ok(map)
}

fn load_decks_from_col(conn: &Connection) -> Result<HashMap<i64, String>> {
    let decks_json: String =
        conn.query_row("SELECT decks FROM col WHERE id = 1", [], |r| r.get(0))?;
    let value: Value = serde_json::from_str(&decks_json).context("parse col.decks")?;
    let mut map = HashMap::new();
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            if let Ok(id) = k.parse::<i64>() {
                if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                    map.insert(id, name.to_string());
                }
            }
        }
    }
    Ok(map)
}
