use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::path::PathBuf;

use crate::models::{
    BrowseRow, Card, CardState, Deck, DeckCounts, DeckOptions, NoteField, StatsSummary, StudyCard,
};

pub struct Store {
    conn: Connection,
    allow_official: bool,
}

impl Store {
    pub fn open_default() -> Result<Self> {
        let path = default_db_path()?;
        let mut store = Self::open(&path)?;
        store.allow_official = true;
        Ok(store)
    }

    pub fn open(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open database at {}", path.display()))?;
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            ",
        )?;
        let store = Self {
            conn,
            allow_official: false,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS decks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                new_per_day INTEGER NOT NULL DEFAULT 20,
                rev_per_day INTEGER NOT NULL DEFAULT 200,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                deck_id INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
                front TEXT NOT NULL,
                back TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                modified_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cards (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                deck_id INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
                due TEXT NOT NULL,
                stability REAL NOT NULL DEFAULT 0,
                difficulty REAL NOT NULL DEFAULT 0,
                elapsed_days INTEGER NOT NULL DEFAULT 0,
                scheduled_days INTEGER NOT NULL DEFAULT 0,
                reps INTEGER NOT NULL DEFAULT 0,
                lapses INTEGER NOT NULL DEFAULT 0,
                state INTEGER NOT NULL DEFAULT 0,
                last_review TEXT NOT NULL,
                suspended INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS revlog (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
                rating INTEGER NOT NULL,
                state INTEGER NOT NULL,
                scheduled_days INTEGER NOT NULL,
                elapsed_days INTEGER NOT NULL,
                stability REAL NOT NULL,
                difficulty REAL NOT NULL,
                reviewed_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_cards_deck_due ON cards(deck_id, due);
            CREATE INDEX IF NOT EXISTS idx_cards_state ON cards(state);
            CREATE INDEX IF NOT EXISTS idx_notes_deck ON notes(deck_id);
            CREATE INDEX IF NOT EXISTS idx_revlog_card ON revlog(card_id);
            CREATE INDEX IF NOT EXISTS idx_revlog_time ON revlog(reviewed_at);
            ",
        )?;

        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM decks", [], |r| r.get(0))?;
        if count == 0 {
            let now = Utc::now();
            self.conn.execute(
                "INSERT INTO decks (name, created_at) VALUES (?1, ?2)",
                params!["Default", now.to_rfc3339()],
            )?;
        }
        Ok(())
    }

    pub fn list_deck_counts(&self) -> Result<Vec<DeckCounts>> {
        if self.uses_official_backend() {
            return crate::anki_backend::list_deck_counts();
        }
        let now = Utc::now().to_rfc3339();
        let mut stmt = self.conn.prepare(
            "
            SELECT
                d.id,
                d.name,
                COALESCE(SUM(CASE WHEN c.id IS NOT NULL AND c.suspended = 0 AND c.state = 0 THEN 1 ELSE 0 END), 0) AS new_count,
                COALESCE(SUM(CASE WHEN c.id IS NOT NULL AND c.suspended = 0 AND c.state IN (1, 3) AND c.due <= ?1 THEN 1 ELSE 0 END), 0) AS learning_count,
                COALESCE(SUM(CASE WHEN c.id IS NOT NULL AND c.suspended = 0 AND c.state = 2 AND c.due <= ?1 THEN 1 ELSE 0 END), 0) AS review_count,
                COALESCE(SUM(CASE WHEN c.id IS NOT NULL THEN 1 ELSE 0 END), 0) AS total_count
            FROM decks d
            LEFT JOIN cards c ON c.deck_id = d.id
            GROUP BY d.id
            ORDER BY d.name COLLATE NOCASE
            ",
        )?;

        let rows = stmt.query_map(params![now], |row| {
            Ok(DeckCounts {
                deck_id: row.get(0)?,
                name: row.get(1)?,
                new: row.get(2)?,
                learning: row.get(3)?,
                review: row.get(4)?,
                total: row.get(5)?,
                level: 0,
            })
        })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_deck(&self, id: i64) -> Result<Option<Deck>> {
        if self.uses_official_backend() {
            return crate::anki_backend::get_deck(id);
        }
        self.conn
            .query_row(
                "SELECT id, name, new_per_day, rev_per_day, created_at FROM decks WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Deck {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        new_per_day: row.get(2)?,
                        rev_per_day: row.get(3)?,
                        created_at: parse_dt(&row.get::<_, String>(4)?)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn create_deck(&self, name: &str) -> Result<i64> {
        if self.uses_official_backend() {
            return crate::anki_backend::create_deck(name);
        }
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO decks (name, created_at) VALUES (?1, ?2)",
            params![name.trim(), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_deck(&self, id: i64, name: &str) -> Result<()> {
        if self.uses_official_backend() {
            return crate::anki_backend::rename_deck(id, name);
        }
        self.conn.execute(
            "UPDATE decks SET name = ?1 WHERE id = ?2",
            params![name.trim(), id],
        )?;
        Ok(())
    }

    pub fn delete_deck(&self, id: i64) -> Result<()> {
        if self.uses_official_backend() {
            return crate::anki_backend::delete_deck(id);
        }
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM decks", [], |r| r.get(0))?;
        anyhow::ensure!(count > 1, "cannot delete the last deck");
        self.conn
            .execute("DELETE FROM decks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn update_deck_options(&self, id: i64, opts: &DeckOptions) -> Result<()> {
        if self.uses_official_backend() {
            return crate::anki_backend::update_deck_options(id, opts);
        }
        self.conn.execute(
            "UPDATE decks SET new_per_day = ?1, rev_per_day = ?2 WHERE id = ?3",
            params![opts.new_per_day, opts.rev_per_day, id],
        )?;
        Ok(())
    }

    pub fn add_note(&self, deck_id: i64, front: &str, back: &str, tags: &str) -> Result<i64> {
        if self.uses_official_backend() {
            return crate::anki_backend::add_note(deck_id, front, back, tags);
        }
        let now = Utc::now();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO notes (deck_id, front, back, tags, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                deck_id,
                front.trim(),
                back.trim(),
                tags.trim(),
                now.to_rfc3339(),
                now.to_rfc3339()
            ],
        )?;
        let note_id = tx.last_insert_rowid();
        let fsrs = rs_fsrs::Card::new();
        tx.execute(
            "INSERT INTO cards (
                note_id, deck_id, due, stability, difficulty, elapsed_days, scheduled_days,
                reps, lapses, state, last_review, suspended, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
            params![
                note_id,
                deck_id,
                fsrs.due.to_rfc3339(),
                fsrs.stability,
                fsrs.difficulty,
                fsrs.elapsed_days,
                fsrs.scheduled_days,
                fsrs.reps,
                fsrs.lapses,
                CardState::from(fsrs.state) as i32,
                fsrs.last_review.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        Ok(note_id)
    }

    pub fn update_note(&self, note_id: i64, front: &str, back: &str, tags: &str) -> Result<()> {
        if self.uses_official_backend() {
            anyhow::bail!("official Anki notes must be updated with their complete field list");
        }
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE notes SET front = ?1, back = ?2, tags = ?3, modified_at = ?4 WHERE id = ?5",
            params![front.trim(), back.trim(), tags.trim(), now, note_id],
        )?;
        Ok(())
    }

    pub fn update_note_fields(&self, note_id: i64, fields: &[NoteField], tags: &str) -> Result<()> {
        if self.uses_official_backend() {
            return crate::anki_backend::update_note(note_id, fields, tags);
        }
        let front = fields
            .first()
            .map(|field| field.value.as_str())
            .unwrap_or("");
        let back = fields
            .get(1)
            .map(|field| field.value.as_str())
            .unwrap_or("");
        self.update_note(note_id, front, back, tags)
    }

    pub fn preview_card(
        &self,
        card_id: i64,
        fields: &[NoteField],
        tags: &str,
    ) -> Result<crate::models::CardPreviewRender> {
        if self.uses_official_backend() {
            return crate::anki_backend::preview_card(card_id, fields, tags);
        }
        anyhow::bail!("card preview requires the official Anki backend")
    }

    pub fn delete_note(&self, note_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM notes WHERE id = ?1", params![note_id])?;
        Ok(())
    }

    pub fn set_card_suspended(&self, card_id: i64, suspended: bool) -> Result<()> {
        if self.uses_official_backend() {
            return crate::anki_backend::set_card_suspended(card_id, suspended);
        }
        self.conn.execute(
            "UPDATE cards SET suspended = ?1 WHERE id = ?2",
            params![suspended as i32, card_id],
        )?;
        Ok(())
    }

    /// Anki Cards → Reset (Forget): turn card back into New; review log kept but ignored.
    pub fn forget_card(&self, card_id: i64) -> Result<()> {
        if self.uses_official_backend() {
            return crate::anki_backend::forget_card(card_id);
        }
        let fsrs = rs_fsrs::Card::new();
        self.conn.execute(
            "UPDATE cards SET
                due = ?1, stability = ?2, difficulty = ?3, elapsed_days = ?4,
                scheduled_days = ?5, reps = ?6, lapses = ?7, state = ?8, last_review = ?9
             WHERE id = ?10",
            params![
                fsrs.due.to_rfc3339(),
                fsrs.stability,
                fsrs.difficulty,
                fsrs.elapsed_days,
                fsrs.scheduled_days,
                fsrs.reps,
                fsrs.lapses,
                CardState::from(fsrs.state) as i32,
                fsrs.last_review.to_rfc3339(),
                card_id,
            ],
        )?;
        Ok(())
    }

    /// Reset all cards in a deck to New (same as selecting all in browser → Reset).
    pub fn forget_deck(&self, deck_id: i64) -> Result<usize> {
        if self.uses_official_backend() {
            return crate::anki_backend::forget_deck(deck_id);
        }
        let fsrs = rs_fsrs::Card::new();
        let n = self.conn.execute(
            "UPDATE cards SET
                due = ?1, stability = ?2, difficulty = ?3, elapsed_days = ?4,
                scheduled_days = ?5, reps = ?6, lapses = ?7, state = ?8, last_review = ?9
             WHERE deck_id = ?10",
            params![
                fsrs.due.to_rfc3339(),
                fsrs.stability,
                fsrs.difficulty,
                fsrs.elapsed_days,
                fsrs.scheduled_days,
                fsrs.reps,
                fsrs.lapses,
                CardState::from(fsrs.state) as i32,
                fsrs.last_review.to_rfc3339(),
                deck_id,
            ],
        )?;
        Ok(n)
    }

    pub fn delete_card(&self, card_id: i64) -> Result<()> {
        if self.uses_official_backend() {
            return crate::anki_backend::delete_card(card_id);
        }
        let note_id: i64 = self.conn.query_row(
            "SELECT note_id FROM cards WHERE id = ?1",
            params![card_id],
            |r| r.get(0),
        )?;
        let remaining: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM cards WHERE note_id = ?1 AND id != ?2",
            params![note_id, card_id],
            |r| r.get(0),
        )?;
        if remaining == 0 {
            self.delete_note(note_id)?;
        } else {
            self.conn
                .execute("DELETE FROM cards WHERE id = ?1", params![card_id])?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn get_card(&self, card_id: i64) -> Result<Option<Card>> {
        self.conn
            .query_row(
                "SELECT id, note_id, deck_id, due, stability, difficulty, elapsed_days,
                        scheduled_days, reps, lapses, state, last_review, suspended, created_at
                 FROM cards WHERE id = ?1",
                params![card_id],
                map_card,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn update_card_schedule(&self, card: &Card, rating: i32) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE cards SET
                due = ?1, stability = ?2, difficulty = ?3, elapsed_days = ?4,
                scheduled_days = ?5, reps = ?6, lapses = ?7, state = ?8, last_review = ?9
             WHERE id = ?10",
            params![
                card.due.to_rfc3339(),
                card.stability,
                card.difficulty,
                card.elapsed_days,
                card.scheduled_days,
                card.reps,
                card.lapses,
                card.state as i32,
                card.last_review.to_rfc3339(),
                card.id,
            ],
        )?;
        tx.execute(
            "INSERT INTO revlog (
                card_id, rating, state, scheduled_days, elapsed_days, stability, difficulty, reviewed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                card.id,
                rating,
                card.state as i32,
                card.scheduled_days,
                card.elapsed_days,
                card.stability,
                card.difficulty,
                Utc::now().to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Fetch next due cards for a deck, respecting daily new/review limits.
    pub fn next_study_queue(&self, deck_id: i64, limit: usize) -> Result<Vec<StudyCard>> {
        if self.uses_official_backend() {
            return crate::anki_backend::next_study_queue(deck_id, limit);
        }
        let deck = self
            .get_deck(deck_id)?
            .with_context(|| format!("deck {deck_id} not found"))?;
        let now = Utc::now();
        let today = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
        let today_start = DateTime::<Utc>::from_naive_utc_and_offset(today, Utc).to_rfc3339();

        let new_studied: i64 = self.conn.query_row(
            "
            SELECT COUNT(DISTINCT c.id)
            FROM revlog r
            JOIN cards c ON c.id = r.card_id
            WHERE c.deck_id = ?1
              AND r.reviewed_at >= ?2
              AND r.state = 0
            ",
            params![deck_id, today_start],
            |r| r.get(0),
        )?;
        let rev_studied: i64 = self.conn.query_row(
            "
            SELECT COUNT(*)
            FROM revlog r
            JOIN cards c ON c.id = r.card_id
            WHERE c.deck_id = ?1
              AND r.reviewed_at >= ?2
              AND r.state = 2
            ",
            params![deck_id, today_start],
            |r| r.get(0),
        )?;

        let new_left = (deck.new_per_day - new_studied).max(0);
        let rev_left = (deck.rev_per_day - rev_studied).max(0);
        let now_s = now.to_rfc3339();

        let mut out = Vec::new();

        // Learning / relearning first
        self.append_study_cards(
            &mut out,
            "
            SELECT c.id, c.note_id, c.deck_id, c.due, c.stability, c.difficulty, c.elapsed_days,
                   c.scheduled_days, c.reps, c.lapses, c.state, c.last_review, c.suspended, c.created_at,
                   n.front, n.back, n.tags, d.name
            FROM cards c
            JOIN notes n ON n.id = c.note_id
            JOIN decks d ON d.id = c.deck_id
            WHERE c.deck_id = ?1 AND c.suspended = 0 AND c.state IN (1, 3) AND c.due <= ?2
            ORDER BY c.due ASC
            LIMIT ?3
            ",
            params![deck_id, now_s.clone(), limit as i64],
        )?;

        if out.len() < limit && rev_left > 0 {
            let take = (limit - out.len()).min(rev_left as usize);
            self.append_study_cards(
                &mut out,
                "
                SELECT c.id, c.note_id, c.deck_id, c.due, c.stability, c.difficulty, c.elapsed_days,
                       c.scheduled_days, c.reps, c.lapses, c.state, c.last_review, c.suspended, c.created_at,
                       n.front, n.back, n.tags, d.name
                FROM cards c
                JOIN notes n ON n.id = c.note_id
                JOIN decks d ON d.id = c.deck_id
                WHERE c.deck_id = ?1 AND c.suspended = 0 AND c.state = 2 AND c.due <= ?2
                ORDER BY c.due ASC
                LIMIT ?3
                ",
                params![deck_id, now_s.clone(), take as i64],
            )?;
        }

        if out.len() < limit && new_left > 0 {
            let take = (limit - out.len()).min(new_left as usize);
            self.append_study_cards(
                &mut out,
                "
                SELECT c.id, c.note_id, c.deck_id, c.due, c.stability, c.difficulty, c.elapsed_days,
                       c.scheduled_days, c.reps, c.lapses, c.state, c.last_review, c.suspended, c.created_at,
                       n.front, n.back, n.tags, d.name
                FROM cards c
                JOIN notes n ON n.id = c.note_id
                JOIN decks d ON d.id = c.deck_id
                WHERE c.deck_id = ?1 AND c.suspended = 0 AND c.state = 0
                ORDER BY c.id ASC
                LIMIT ?3
                ",
                params![deck_id, now_s, take as i64],
            )?;
        }

        Ok(out)
    }

    fn append_study_cards(
        &self,
        out: &mut Vec<StudyCard>,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |row| {
            let card = map_card(row)?;
            Ok(StudyCard {
                card,
                front: row.get(14)?,
                back: row.get(15)?,
                tags: row.get(16)?,
                deck_name: row.get(17)?,
                answer_intervals: Vec::new(),
                answer_includes_question: false,
                front_document: None,
                back_document: None,
                scheduling_states_hex: String::new(),
                fields: vec![
                    NoteField {
                        name: "Front".into(),
                        value: row.get(14)?,
                    },
                    NoteField {
                        name: "Back".into(),
                        value: row.get(15)?,
                    },
                ],
            })
        })?;
        for row in rows {
            out.push(row?);
        }
        Ok(())
    }

    pub fn browse(&self, query: &str, limit: i64) -> Result<Vec<BrowseRow>> {
        if self.uses_official_backend() {
            return crate::anki_backend::browse(query, limit);
        }
        let q = query.trim();
        let like = format!("%{}%", q);
        let sql = if q.is_empty() {
            "
            SELECT c.id, n.id, d.name, n.front, n.back, n.tags, c.state, c.due, c.reps, c.lapses, c.suspended
            FROM cards c
            JOIN notes n ON n.id = c.note_id
            JOIN decks d ON d.id = c.deck_id
            ORDER BY n.modified_at DESC
            LIMIT ?1
            "
        } else {
            "
            SELECT c.id, n.id, d.name, n.front, n.back, n.tags, c.state, c.due, c.reps, c.lapses, c.suspended
            FROM cards c
            JOIN notes n ON n.id = c.note_id
            JOIN decks d ON d.id = c.deck_id
            WHERE n.front LIKE ?2 ESCAPE '\\'
               OR n.back LIKE ?2 ESCAPE '\\'
               OR n.tags LIKE ?2 ESCAPE '\\'
               OR d.name LIKE ?2 ESCAPE '\\'
            ORDER BY n.modified_at DESC
            LIMIT ?1
            "
        };

        let mut stmt = self.conn.prepare(sql)?;
        let map = |row: &rusqlite::Row<'_>| {
            Ok(BrowseRow {
                card_id: row.get(0)?,
                note_id: row.get(1)?,
                deck_name: row.get(2)?,
                front: row.get(3)?,
                back: row.get(4)?,
                tags: row.get(5)?,
                state: CardState::from_i32(row.get(6)?),
                due: parse_dt(&row.get::<_, String>(7)?)?,
                reps: row.get(8)?,
                lapses: row.get(9)?,
                suspended: row.get::<_, i32>(10)? != 0,
                due_label: String::new(),
                fields: vec![
                    NoteField {
                        name: "Front".into(),
                        value: row.get(3)?,
                    },
                    NoteField {
                        name: "Back".into(),
                        value: row.get(4)?,
                    },
                ],
            })
        };

        let rows = if q.is_empty() {
            stmt.query_map(params![limit], map)?
        } else {
            stmt.query_map(params![limit, like], map)?
        };
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn stats(&self) -> Result<StatsSummary> {
        if self.uses_official_backend() {
            return crate::anki_backend::stats();
        }
        let now = Utc::now();
        let day0 = day_start(now).to_rfc3339();
        let day7 = day_start(now - chrono::Duration::days(7)).to_rfc3339();
        let day30 = day_start(now - chrono::Duration::days(30)).to_rfc3339();
        Ok(StatsSummary {
            total_decks: self
                .conn
                .query_row("SELECT COUNT(*) FROM decks", [], |row| row.get(0))?,
            total_notes: self
                .conn
                .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))?,
            total_cards: self
                .conn
                .query_row("SELECT COUNT(*) FROM cards", [], |row| row.get(0))?,
            new_cards: self.conn.query_row(
                "SELECT COUNT(*) FROM cards WHERE suspended = 0 AND state = 0",
                [],
                |row| row.get(0),
            )?,
            learning_cards: self.conn.query_row(
                "SELECT COUNT(*) FROM cards WHERE suspended = 0 AND state = 1",
                [],
                |row| row.get(0),
            )?,
            review_cards: self.conn.query_row(
                "SELECT COUNT(*) FROM cards WHERE suspended = 0 AND state = 2",
                [],
                |row| row.get(0),
            )?,
            relearning_cards: self.conn.query_row(
                "SELECT COUNT(*) FROM cards WHERE suspended = 0 AND state = 3",
                [],
                |row| row.get(0),
            )?,
            suspended_cards: self.conn.query_row(
                "SELECT COUNT(*) FROM cards WHERE suspended = 1",
                [],
                |row| row.get(0),
            )?,
            mature_cards: self.conn.query_row(
                "SELECT COUNT(*) FROM cards WHERE suspended = 0 AND state = 2 AND scheduled_days >= 21",
                [],
                |row| row.get(0),
            )?,
            reviews_today: self.conn.query_row(
                "SELECT COUNT(*) FROM revlog WHERE reviewed_at >= ?1",
                params![day0],
                |row| row.get(0),
            )?,
            reviews_7d: self.conn.query_row(
                "SELECT COUNT(*) FROM revlog WHERE reviewed_at >= ?1",
                params![day7],
                |row| row.get(0),
            )?,
            reviews_30d: self.conn.query_row(
                "SELECT COUNT(*) FROM revlog WHERE reviewed_at >= ?1",
                params![day30],
                |row| row.get(0),
            )?,
        })
    }

    pub fn uses_official_backend(&self) -> bool {
        self.allow_official
            && crate::sync::paths::official_ready()
            && crate::anki_backend::available()
    }
}

fn day_start(dt: DateTime<Utc>) -> DateTime<Utc> {
    let naive = dt.date_naive().and_hms_opt(0, 0, 0).unwrap();
    DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
}

/// App data dir: `$XDG_DATA_HOME/anki-tui` or `~/.local/share/anki-tui` on all platforms.
pub fn collection_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("XDG_DATA_HOME") {
        Some(xdg) if !xdg.is_empty() => PathBuf::from(xdg).join("anki-tui"),
        _ => {
            let home = dirs::home_dir().context("cannot resolve home directory")?;
            home.join(".local/share/anki-tui")
        }
    };
    fs::create_dir_all(&dir).with_context(|| format!("create data dir {}", dir.display()))?;
    Ok(dir)
}

fn default_db_path() -> Result<PathBuf> {
    Ok(collection_dir()?.join("collection.db"))
}

fn parse_dt(s: &str) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
                .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
                .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc))
        })
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

fn map_card(row: &rusqlite::Row<'_>) -> Result<Card, rusqlite::Error> {
    Ok(Card {
        id: row.get(0)?,
        note_id: row.get(1)?,
        deck_id: row.get(2)?,
        due: parse_dt(&row.get::<_, String>(3)?)?,
        stability: row.get(4)?,
        difficulty: row.get(5)?,
        elapsed_days: row.get(6)?,
        scheduled_days: row.get(7)?,
        reps: row.get(8)?,
        lapses: row.get(9)?,
        state: CardState::from_i32(row.get(10)?),
        last_review: parse_dt(&row.get::<_, String>(11)?)?,
        suspended: row.get::<_, i32>(12)? != 0,
        created_at: parse_dt(&row.get::<_, String>(13)?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::Scheduler;

    #[test]
    fn add_and_review_card() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let decks = store.list_deck_counts().unwrap();
        assert_eq!(decks.len(), 1);
        let deck_id = decks[0].deck_id;
        store.add_note(deck_id, "Q?", "A!", "tag1").unwrap();
        let queue = store.next_study_queue(deck_id, 10).unwrap();
        assert_eq!(queue.len(), 1);
        let mut card = queue[0].card.clone();
        let sched = Scheduler::new();
        let (updated, rating) = sched.review(&card, rs_fsrs::Rating::Good);
        card = updated;
        store.update_card_schedule(&card, rating as i32).unwrap();
        let again = store.get_card(card.id).unwrap().unwrap();
        assert!(again.reps >= 1);
    }

    #[test]
    fn update_complete_note_fields() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let decks = store.list_deck_counts().unwrap();
        let deck_id = decks[0].deck_id;
        let note_id = store
            .add_note(deck_id, "Hello?", "World!", "greetings")
            .unwrap();
        store
            .update_note_fields(
                note_id,
                &[
                    NoteField {
                        name: "Front".into(),
                        value: "Updated?".into(),
                    },
                    NoteField {
                        name: "Back".into(),
                        value: "Still here!".into(),
                    },
                ],
                "safe",
            )
            .unwrap();
        let rows = store.browse("", 10).unwrap();
        assert_eq!(rows[0].front, "Updated?");
        assert_eq!(rows[0].back, "Still here!");
        assert_eq!(rows[0].tags, "safe");
    }
}
