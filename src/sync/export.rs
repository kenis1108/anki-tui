use anyhow::Result;
use rand::RngExt;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::{ExportNote, Store};
use crate::models::CardState;

/// Build a schema-11 Anki collection sqlite file from local data.
pub fn export_collection_bytes(store: &Store) -> Result<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("anki-tui-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("collection.anki2");
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    let conn = Connection::open(&path)?;
    init_schema11(&conn)?;

    let decks = store.list_decks_for_export()?;
    let notes = store.list_notes_for_export()?;

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let now_ms = now_secs * 1000;

    let model_id = now_ms - 1;
    let models = basic_model(model_id, now_secs);
    let mut deck_json = json!({});
    let mut deck_id_map: HashMap<i64, i64> = HashMap::new();

    // Default deck id 1
    let default = json!({
        "id": 1,
        "mod": now_secs,
        "name": "Default",
        "usn": 0,
        "lrnToday": [0, 0],
        "revToday": [0, 0],
        "newToday": [0, 0],
        "timeToday": [0, 0],
        "collapsed": false,
        "browserCollapsed": false,
        "desc": "",
        "dyn": 0,
        "conf": 1,
        "extendNew": 0,
        "extendRev": 0
    });
    deck_json["1"] = default;
    deck_id_map.insert(-1, 1);

    let mut next_deck_id = 2i64;
    for d in &decks {
        let anki_id = if d.name == "Default" {
            1
        } else {
            let id = next_deck_id;
            next_deck_id += 1;
            id
        };
        deck_id_map.insert(d.id, anki_id);
        deck_json[anki_id.to_string()] = json!({
            "id": anki_id,
            "mod": now_secs,
            "name": d.name,
            "usn": 0,
            "lrnToday": [0, 0],
            "revToday": [0, 0],
            "newToday": [0, 0],
            "timeToday": [0, 0],
            "collapsed": false,
            "browserCollapsed": false,
            "desc": "",
            "dyn": 0,
            "conf": 1,
            "extendNew": 0,
            "extendRev": 0
        });
    }

    let dconf = default_dconf(now_secs);
    conn.execute(
        "UPDATE col SET crt=?1, mod=?2, scm=?2, ver=11, dty=0, usn=0, ls=?2, conf=?3, models=?4, decks=?5, dconf=?6, tags=?7",
        params![
            now_secs,
            now_ms,
            conf_json(),
            models.to_string(),
            deck_json.to_string(),
            dconf.to_string(),
            "{}"
        ],
    )?;

    let mut note_pos = 0i64;
    for n in &notes {
        note_pos += 1;
        let did = deck_id_map.get(&n.deck_id).copied().unwrap_or(1);
        let guid = gen_guid();
        let flds = format!("{}\u{1f}{}", n.front, n.back);
        let sfld = n.front.clone();
        let csum = field_checksum(&n.front);
        let tags = if n.tags.is_empty() {
            String::new()
        } else if n.tags.starts_with(' ') {
            n.tags.clone()
        } else {
            format!(" {} ", n.tags.trim())
        };

        conn.execute(
            "INSERT INTO notes (id, guid, mid, mod, usn, tags, flds, sfld, csum, flags, data)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, 0, '')",
            params![note_pos, guid, model_id, now_secs, tags, flds, sfld, csum],
        )?;

        let (ctype, queue, due, ivl, factor, reps, lapses) = map_schedule(n);
        conn.execute(
            "INSERT INTO cards (id, nid, did, ord, mod, usn, type, queue, due, ivl, factor, reps, lapses, left, odue, odid, flags, data)
             VALUES (?1, ?2, ?3, 0, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, 0, 0, 0, '')",
            params![
                note_pos,
                note_pos,
                did,
                now_secs,
                ctype,
                queue,
                due,
                ivl,
                factor,
                reps,
                lapses
            ],
        )?;
    }

    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    drop(conn);

    let mut file = std::fs::File::open(&path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(bytes)
}

fn map_schedule(n: &ExportNote) -> (i32, i32, i32, i32, i32, i32, i32) {
    if n.suspended {
        return (n.state as i32, -1, 0, 0, 0, n.reps, n.lapses);
    }
    match n.state {
        CardState::New => (0, 0, n.id as i32, 0, 0, 0, 0),
        CardState::Learning | CardState::Relearning => {
            let due = n.due.timestamp() as i32;
            (1, 1, due, 0, 0, n.reps, n.lapses)
        }
        CardState::Review => {
            let days = n.scheduled_days.max(1) as i32;
            (2, 2, days, days, 2500, n.reps, n.lapses)
        }
    }
}

fn init_schema11(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE col (
            id integer PRIMARY KEY,
            crt integer NOT NULL,
            mod integer NOT NULL,
            scm integer NOT NULL,
            ver integer NOT NULL,
            dty integer NOT NULL,
            usn integer NOT NULL,
            ls integer NOT NULL,
            conf text NOT NULL,
            models text NOT NULL,
            decks text NOT NULL,
            dconf text NOT NULL,
            tags text NOT NULL
        );
        CREATE TABLE notes (
            id integer PRIMARY KEY,
            guid text NOT NULL,
            mid integer NOT NULL,
            mod integer NOT NULL,
            usn integer NOT NULL,
            tags text NOT NULL,
            flds text NOT NULL,
            sfld integer NOT NULL,
            csum integer NOT NULL,
            flags integer NOT NULL,
            data text NOT NULL
        );
        CREATE TABLE cards (
            id integer PRIMARY KEY,
            nid integer NOT NULL,
            did integer NOT NULL,
            ord integer NOT NULL,
            mod integer NOT NULL,
            usn integer NOT NULL,
            type integer NOT NULL,
            queue integer NOT NULL,
            due integer NOT NULL,
            ivl integer NOT NULL,
            factor integer NOT NULL,
            reps integer NOT NULL,
            lapses integer NOT NULL,
            left integer NOT NULL,
            odue integer NOT NULL,
            odid integer NOT NULL,
            flags integer NOT NULL,
            data text NOT NULL
        );
        CREATE TABLE revlog (
            id integer PRIMARY KEY,
            cid integer NOT NULL,
            usn integer NOT NULL,
            ease integer NOT NULL,
            ivl integer NOT NULL,
            lastIvl integer NOT NULL,
            factor integer NOT NULL,
            time integer NOT NULL,
            type integer NOT NULL
        );
        CREATE TABLE graves (
            usn integer NOT NULL,
            oid integer NOT NULL,
            type integer NOT NULL
        );
        CREATE INDEX ix_notes_usn ON notes (usn);
        CREATE INDEX ix_cards_usn ON cards (usn);
        CREATE INDEX ix_revlog_usn ON revlog (usn);
        CREATE INDEX ix_cards_nid ON cards (nid);
        CREATE INDEX ix_cards_sched ON cards (did, queue, due);
        CREATE INDEX ix_revlog_cid ON revlog (cid);
        CREATE INDEX ix_notes_csum ON notes (csum);
        INSERT INTO col VALUES (1,0,0,0,11,0,0,0,'{}','{}','{}','{}','{}');
        ",
    )?;
    Ok(())
}

fn conf_json() -> String {
    json!({
        "activeDecks": [1],
        "addToCur": true,
        "collapseTime": 1200,
        "curDeck": 1,
        "curModel": null,
        "dueCollapses": true,
        "estTimes": true,
        "newBury": true,
        "newSpread": 0,
        "nextDayAt": 4,
        "sortBackwards": false,
        "sortType": "noteFld",
        "timeLim": 0
    })
    .to_string()
}

fn basic_model(id: i64, now: i64) -> Value {
    json!({
        id.to_string(): {
            "id": id,
            "name": "Basic",
            "type": 0,
            "mod": now,
            "usn": 0,
            "sortf": 0,
            "did": 1,
            "tmpls": [{
                "name": "Card 1",
                "ord": 0,
                "qfmt": "{{Front}}",
                "afmt": "{{FrontSide}}\n\n<hr id=answer>\n\n{{Back}}",
                "bqfmt": "",
                "bafmt": "",
                "did": null,
                "bfont": "",
                "bsize": 0
            }],
            "flds": [
                {
                    "name": "Front",
                    "ord": 0,
                    "sticky": false,
                    "rtl": false,
                    "font": "Arial",
                    "size": 20,
                    "description": "",
                    "plainText": false,
                    "collapsed": false,
                    "excludeFromSearch": false
                },
                {
                    "name": "Back",
                    "ord": 1,
                    "sticky": false,
                    "rtl": false,
                    "font": "Arial",
                    "size": 20,
                    "description": "",
                    "plainText": false,
                    "collapsed": false,
                    "excludeFromSearch": false
                }
            ],
            "css": ".card {\n font-family: arial;\n font-size: 20px;\n text-align: center;\n color: black;\n background-color: white;\n}\n",
            "latexPre": "\\documentclass[12pt]{article}\n\\special{papersize=3in,5in}\n\\usepackage[utf8]{inputenc}\n\\usepackage{amssymb,amsmath}\n\\pagestyle{empty}\n\\setlength{\\parindent}{0in}\n\\begin{document}\n",
            "latexPost": "\\end{document}",
            "latexsvg": false,
            "req": [[0, "any", [0]]]
        }
    })
}

fn default_dconf(now: i64) -> Value {
    json!({
        "1": {
            "id": 1,
            "mod": now,
            "name": "Default",
            "usn": 0,
            "maxTaken": 60,
            "autoplay": true,
            "timer": 0,
            "replayq": true,
            "new": {
                "bury": false,
                "delays": [1.0, 10.0],
                "initialFactor": 2500,
                "ints": [1, 4],
                "order": 1,
                "perDay": 20
            },
            "rev": {
                "bury": false,
                "ease4": 1.3,
                "ivlFct": 1.0,
                "maxIvl": 36500,
                "perDay": 200,
                "hardFactor": 1.2
            },
            "lapse": {
                "delays": [10.0],
                "leechAction": 1,
                "leechFails": 8,
                "minInt": 1,
                "mult": 0.0
            },
            "dyn": false,
            "newMix": 0,
            "newPerDayMinimum": 0,
            "interdayLearningMix": 0,
            "reviewOrder": 0,
            "newSortOrder": 0,
            "newGatherPriority": 0,
            "buryInterdayLearning": false
        }
    })
}

fn field_checksum(data: &str) -> i64 {
    let digest = Sha1::digest(data.as_bytes());
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    i64::from_str_radix(&hex[..8], 16).unwrap_or(0)
}

fn gen_guid() -> String {
    const TABLE: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&()*+,-./:;<=>?@[]^_`{|}~";
    let mut rng = rand::rng();
    (0..10)
        .map(|_| TABLE[rng.random_range(0..TABLE.len())] as char)
        .collect()
}
