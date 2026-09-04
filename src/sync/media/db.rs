use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct MediaEntry {
    pub fname: String,
    pub sha1: Option<String>,
    pub mtime: i64,
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub struct MediaMeta {
    pub folder_mtime: i64,
    pub last_sync_usn: i32,
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
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM meta", [], |r| r.get(0))?;
        if n == 0 {
            self.conn.execute("INSERT INTO meta VALUES (0, 0)", [])?;
        }
        Ok(())
    }

    pub fn get_meta(&self) -> Result<MediaMeta> {
        self.conn
            .query_row("SELECT dirMod, lastUsn FROM meta", [], |r| {
                Ok(MediaMeta {
                    folder_mtime: r.get(0)?,
                    last_sync_usn: r.get(1)?,
                })
            })
            .map_err(Into::into)
    }

    pub fn set_meta(&self, meta: &MediaMeta) -> Result<()> {
        self.conn.execute(
            "UPDATE meta SET dirMod = ?1, lastUsn = ?2",
            params![meta.folder_mtime, meta.last_sync_usn],
        )?;
        Ok(())
    }

    pub fn get_entry(&self, fname: &str) -> Result<Option<MediaEntry>> {
        self.conn
            .query_row(
                "SELECT fname, csum, mtime, dirty FROM media WHERE fname = ?1",
                params![fname],
                |r| {
                    Ok(MediaEntry {
                        fname: r.get(0)?,
                        sha1: r.get(1)?,
                        mtime: r.get(2)?,
                        dirty: r.get::<_, i32>(3)? != 0,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn set_entry(&self, e: &MediaEntry) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO media (fname, csum, mtime, dirty) VALUES (?1, ?2, ?3, ?4)",
            params![e.fname, e.sha1, e.mtime, e.dirty as i32],
        )?;
        Ok(())
    }

    pub fn remove_entry(&self, fname: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM media WHERE fname = ?1", params![fname])?;
        Ok(())
    }

    pub fn count(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM media WHERE csum IS NOT NULL",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn pending_uploads(&self, limit: u32) -> Result<Vec<MediaEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT fname FROM media WHERE dirty = 1 LIMIT ?1")?;
        let names: Vec<String> = stmt
            .query_map(params![limit], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for n in names {
            if let Some(e) = self.get_entry(&n)? {
                out.push(e);
            }
        }
        Ok(out)
    }

    pub fn mark_clean(&self, names: &[String]) -> Result<()> {
        for n in names {
            if let Some(mut e) = self.get_entry(n)? {
                if e.dirty {
                    e.dirty = false;
                    self.set_entry(&e)?;
                }
            }
        }
        Ok(())
    }

    pub fn force_resync(&self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM media; UPDATE meta SET lastUsn = 0, dirMod = 0")?;
        Ok(())
    }

    pub fn register_folder(&self, folder: &Path) -> Result<usize> {
        let mut checked = 0usize;
        let mut known = std::collections::HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT fname, mtime, csum FROM media WHERE csum IS NOT NULL")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (f, m, c) = row?;
                known.insert(f, (m, c));
            }
        }

        let mut seen = std::collections::HashSet::new();
        if folder.exists() {
            for entry in fs::read_dir(folder)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.starts_with('.') {
                    continue;
                }
                seen.insert(fname.clone());
                checked += 1;
                let meta = entry.metadata()?;
                let mtime = mtime_secs(&meta);
                if let Some((old_m, _)) = known.get(&fname) {
                    if *old_m == mtime {
                        continue;
                    }
                }
                let data = fs::read(entry.path())?;
                let csum = sha1_hex(&data);
                self.set_entry(&MediaEntry {
                    fname,
                    sha1: Some(csum),
                    mtime,
                    dirty: true,
                })?;
            }
        }

        // deletions
        for (fname, _) in known {
            if !seen.contains(&fname) {
                self.set_entry(&MediaEntry {
                    fname,
                    sha1: None,
                    mtime: 0,
                    dirty: true,
                })?;
                checked += 1;
            }
        }

        let mut meta = self.get_meta()?;
        meta.folder_mtime = folder_mtime(folder)?;
        self.set_meta(&meta)?;
        Ok(checked)
    }
}

pub fn sha1_hex(data: &[u8]) -> String {
    let d = Sha1::digest(data);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn mtime_secs(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn folder_mtime(folder: &Path) -> Result<i64> {
    if !folder.exists() {
        return Ok(0);
    }
    Ok(mtime_secs(&fs::metadata(folder)?))
}

pub fn media_file_path(folder: &Path, fname: &str) -> PathBuf {
    folder.join(fname)
}
