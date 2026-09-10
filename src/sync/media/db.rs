use anyhow::Result;
use rusqlite::{params, Connection};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct MediaEntry {
    pub fname: String,
    pub sha1: Option<String>,
    pub mtime: i64,
    pub dirty: bool,
}

pub struct MediaDb {
    conn: Connection,
}

impl MediaDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS media (
              fname text NOT NULL PRIMARY KEY,
              csum text,
              mtime int NOT NULL,
              dirty int NOT NULL
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS idx_media_dirty ON media (dirty) WHERE dirty = 1;
            CREATE TABLE IF NOT EXISTS meta (dirMod int, lastUsn int);
            ",
        )?;
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM meta", [], |row| row.get(0))?;
        if count == 0 {
            self.conn.execute("INSERT INTO meta VALUES (0, 0)", [])?;
        }
        Ok(())
    }

    pub fn set_entry(&self, entry: &MediaEntry) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO media (fname, csum, mtime, dirty) VALUES (?1, ?2, ?3, ?4)",
            params![entry.fname, entry.sha1, entry.mtime, entry.dirty as i32],
        )?;
        Ok(())
    }
}

pub fn sha1_hex(data: &[u8]) -> String {
    let digest = Sha1::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn mtime_secs(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
