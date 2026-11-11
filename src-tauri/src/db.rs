use crate::error::{Error, Result};
use crate::model::*;
use crate::paths;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileConfig {
    /// The application's own SQLite database. WebDAV is the only data source,
    /// so this file is created empty on first launch and then filled by a sync.
    pub work_db: PathBuf,
    /// Attachment blobs downloaded from WebDAV, inside our own data directory.
    pub resources: PathBuf,
    /// When the (empty) library was created.
    pub synced_at: i64,
}

impl ProfileConfig {
    fn config_path() -> PathBuf {
        paths::paths().profile_config
    }

    /// Load the app profile. A missing data directory is never a reason to look
    /// for data elsewhere: the library starts empty and is populated by a
    /// WebDAV sync. This single rule is what makes `%APPDATA%\joplin-rs` the
    /// only place this application reads notes from.
    pub fn load() -> Result<Self> {
        // One call creates the whole app-owned directory tree.
        let layout = paths::ensure()?;
        let path = layout.profile_config.clone();
        let (mut cfg, legacy_source) = if path.exists() {
            let raw = fs::read_to_string(&path)?;
            // Builds before the single-source change stored a `source` path to
            // the local Joplin profile. Parsing ignores unknown keys, but the
            // stale entry is dropped from disk so nobody believes data still
            // comes from there.
            let had_source = raw.contains("\"source\"");
            let parsed = serde_json::from_str::<Self>(&raw)
                .map_err(|e| Error::msg(format!("profile.json 损坏: {e}")))?;
            (parsed, had_source)
        } else {
            (
                Self {
                    work_db: layout.database.clone(),
                    resources: layout.resources.clone(),
                    synced_at: 0,
                },
                false,
            )
        };
        // A legacy config may still point at a database outside the data
        // directory. Fold it back in, without discarding any data.
        normalise_paths(&mut cfg, &layout)?;
        if !cfg.work_db.exists() {
            create_empty_database(&cfg.work_db)?;
            cfg.synced_at = now_ms();
            cfg.save()?;
            crate::startup_log(
                "empty library created (nothing imported; WebDAV is the only data source)",
            );
        } else {
            // Cheap and idempotent: guarantees the tables this build needs even
            // for a database created by an older version.
            let conn = Connection::open(&cfg.work_db)?;
            ensure_schema(&conn)?;
        }
        if legacy_source {
            cfg.save()?;
            crate::startup_log("profile.json: dropped legacy `source` (local import removed)");
        }
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(paths::data_dir())?;
        fs::write(Self::config_path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Keep every runtime path inside our own data directory. A legacy working copy
/// is copied (never moved or deleted) so no user data is lost on upgrade.
fn normalise_paths(cfg: &mut ProfileConfig, layout: &paths::DataLayout) -> Result<()> {
    let mut changed = false;
    if cfg.work_db != layout.database {
        if cfg.work_db.is_file() && !layout.database.exists() {
            fs::copy(&cfg.work_db, &layout.database)?;
            for suffix in ["-wal", "-shm"] {
                let from = PathBuf::from(format!("{}{}", cfg.work_db.display(), suffix));
                if from.exists() {
                    let to = PathBuf::from(format!("{}{}", layout.database.display(), suffix));
                    let _ = fs::copy(&from, &to);
                }
            }
        }
        cfg.work_db = layout.database.clone();
        changed = true;
    }
    if cfg.resources != layout.resources {
        cfg.resources = layout.resources.clone();
        changed = true;
    }
    if changed {
        cfg.save()?;
        crate::startup_log("profile.json migrated to the app-owned data directory");
    }
    Ok(())
}

/// Joplin-compatible schema, reduced to what this client reads and writes.
///
/// The columns mirror Joplin's own tables so a synced item keeps every field an
/// official client sends (latitude, todo_due, share ids, ...). Notes and
/// attachments arrive from WebDAV via `sync::engine`, or are created locally
/// when a note is edited / an image is inserted.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS notes (
  id TEXT PRIMARY KEY,
  parent_id TEXT NOT NULL DEFAULT '',
  title TEXT NOT NULL DEFAULT '',
  body TEXT NOT NULL DEFAULT '',
  created_time INT NOT NULL,
  updated_time INT NOT NULL,
  is_conflict INT NOT NULL DEFAULT 0,
  latitude NUMERIC NOT NULL DEFAULT 0,
  longitude NUMERIC NOT NULL DEFAULT 0,
  altitude NUMERIC NOT NULL DEFAULT 0,
  author TEXT NOT NULL DEFAULT '',
  source_url TEXT NOT NULL DEFAULT '',
  is_todo INT NOT NULL DEFAULT 0,
  todo_due INT NOT NULL DEFAULT 0,
  todo_completed INT NOT NULL DEFAULT 0,
  source TEXT NOT NULL DEFAULT '',
  source_application TEXT NOT NULL DEFAULT '',
  application_data TEXT NOT NULL DEFAULT '',
  "order" NUMERIC NOT NULL DEFAULT 0,
  user_created_time INT NOT NULL DEFAULT 0,
  user_updated_time INT NOT NULL DEFAULT 0,
  encryption_cipher_text TEXT NOT NULL DEFAULT '',
  encryption_applied INT NOT NULL DEFAULT 0,
  markup_language INT NOT NULL DEFAULT 1,
  is_shared INT NOT NULL DEFAULT 0,
  share_id TEXT NOT NULL DEFAULT '',
  conflict_original_id TEXT NOT NULL DEFAULT '',
  master_key_id TEXT NOT NULL DEFAULT '',
  user_data TEXT NOT NULL DEFAULT '',
  deleted_time INT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS folders (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL DEFAULT '',
  created_time INT NOT NULL,
  updated_time INT NOT NULL,
  user_created_time INT NOT NULL DEFAULT 0,
  user_updated_time INT NOT NULL DEFAULT 0,
  encryption_cipher_text TEXT NOT NULL DEFAULT '',
  encryption_applied INT NOT NULL DEFAULT 0,
  parent_id TEXT NOT NULL DEFAULT '',
  is_shared INT NOT NULL DEFAULT 0,
  share_id TEXT NOT NULL DEFAULT '',
  master_key_id TEXT NOT NULL DEFAULT '',
  icon TEXT NOT NULL DEFAULT '',
  user_data TEXT NOT NULL DEFAULT '',
  deleted_time INT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS resources (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL DEFAULT '',
  mime TEXT NOT NULL DEFAULT '',
  filename TEXT NOT NULL DEFAULT '',
  created_time INT NOT NULL,
  updated_time INT NOT NULL,
  user_created_time INT NOT NULL DEFAULT 0,
  user_updated_time INT NOT NULL DEFAULT 0,
  file_extension TEXT NOT NULL DEFAULT '',
  encryption_cipher_text TEXT NOT NULL DEFAULT '',
  encryption_applied INT NOT NULL DEFAULT 0,
  encryption_blob_encrypted INT NOT NULL DEFAULT 0,
  size INT NOT NULL DEFAULT -1,
  is_shared INT NOT NULL DEFAULT 0,
  share_id TEXT NOT NULL DEFAULT '',
  master_key_id TEXT NOT NULL DEFAULT '',
  user_data TEXT NOT NULL DEFAULT '',
  blob_updated_time INT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS tags (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL DEFAULT '',
  created_time INT NOT NULL,
  updated_time INT NOT NULL,
  user_created_time INT NOT NULL DEFAULT 0,
  user_updated_time INT NOT NULL DEFAULT 0,
  encryption_cipher_text TEXT NOT NULL DEFAULT '',
  encryption_applied INT NOT NULL DEFAULT 0,
  is_shared INT NOT NULL DEFAULT 0,
  parent_id TEXT NOT NULL DEFAULT '',
  user_data TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS note_tags (
  id TEXT PRIMARY KEY,
  note_id TEXT NOT NULL,
  tag_id TEXT NOT NULL,
  created_time INT NOT NULL,
  updated_time INT NOT NULL,
  user_created_time INT NOT NULL DEFAULT 0,
  user_updated_time INT NOT NULL DEFAULT 0,
  encryption_cipher_text TEXT NOT NULL DEFAULT '',
  encryption_applied INT NOT NULL DEFAULT 0,
  is_shared INT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS deleted_items (
  id INTEGER PRIMARY KEY,
  item_type INT NOT NULL,
  item_id TEXT NOT NULL,
  deleted_time INT NOT NULL,
  sync_target INT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notes_parent ON notes(parent_id);
CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_time);
CREATE INDEX IF NOT EXISTS idx_note_tags_note ON note_tags(note_id);
CREATE INDEX IF NOT EXISTS idx_note_tags_tag ON note_tags(tag_id);
CREATE INDEX IF NOT EXISTS idx_resources_updated ON resources(updated_time);
"#;

/// Add the tables this build needs to an existing database. Idempotent.
pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    Ok(())
}

/// Create the (empty) library. Used on first launch only: no notes, no
/// folders, no attachments, nothing copied from any other application.
pub fn create_empty_database(path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    Ok(())
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // The library lives here and nowhere else, so a missing table is
        // something this build can simply create instead of an error.
        ensure_schema(&conn)?;
        Ok(Self { conn })
    }

    pub fn list_folders(&self) -> Result<Vec<Folder>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.title, COALESCE(f.parent_id,''), f.updated_time,
                    (SELECT COUNT(*) FROM notes n WHERE n.parent_id = f.id AND n.deleted_time = 0)
             FROM folders f
             WHERE f.deleted_time = 0
             ORDER BY f.title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                title: r.get(1)?,
                parent_id: r.get(2)?,
                updated_time: r.get(3)?,
                note_count: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// List notes for a folder/tag/whole library.
    ///
    /// `limit <= 0` means "no limit" (SQLite treats `LIMIT -1` that way), which
    /// is what the UI uses: the list header shows a count, so it must never
    /// disagree with the rows actually returned. Only the first 600 bytes of
    /// the body are read — the list needs an excerpt, not the whole note.
    pub fn list_notes(
        &self,
        folder_id: Option<&str>,
        tag_id: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<NoteSummary>> {
        let limit = if limit <= 0 { -1 } else { limit };
        let sql = match (folder_id.is_some(), tag_id.is_some()) {
            (_, true) => {
                "SELECT n.id, n.title, COALESCE(n.parent_id,''), n.created_time, n.updated_time,
                        n.is_todo, n.todo_completed, substr(n.body, 1, 600)
                 FROM notes n JOIN note_tags nt ON nt.note_id = n.id
                 WHERE nt.tag_id = ?1 AND n.deleted_time = 0
                 ORDER BY n.updated_time DESC LIMIT ?2 OFFSET ?3"
            }
            (true, false) => {
                "SELECT n.id, n.title, COALESCE(n.parent_id,''), n.created_time, n.updated_time,
                        n.is_todo, n.todo_completed, substr(n.body, 1, 600)
                 FROM notes n
                 WHERE n.parent_id = ?1 AND n.deleted_time = 0
                 ORDER BY n.updated_time DESC LIMIT ?2 OFFSET ?3"
            }
            (false, false) => {
                "SELECT n.id, n.title, COALESCE(n.parent_id,''), n.created_time, n.updated_time,
                        n.is_todo, n.todo_completed, substr(n.body, 1, 600)
                 FROM notes n
                 WHERE n.deleted_time = 0
                 ORDER BY n.updated_time DESC LIMIT ?1 OFFSET ?2"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let map_row = |r: &rusqlite::Row| -> rusqlite::Result<NoteSummary> {
            let body: String = r.get(7)?;
            Ok(NoteSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                parent_id: r.get(2)?,
                created_time: r.get(3)?,
                updated_time: r.get(4)?,
                is_todo: r.get(5)?,
                todo_completed: r.get(6)?,
                excerpt: make_excerpt(&body, 110),
            })
        };
        let rows = if let Some(t) = tag_id {
            stmt.query_map(params![t, limit, offset], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else if let Some(f) = folder_id {
            stmt.query_map(params![f, limit, offset], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![limit, offset], map_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub fn get_note(&self, id: &str) -> Result<Note> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, body, COALESCE(parent_id,''), created_time, updated_time,
                    is_todo, todo_completed, markup_language
             FROM notes WHERE id = ?1 AND deleted_time = 0",
        )?;
        let note = stmt
            .query_row(params![id], |r| {
                Ok(Note {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    body: r.get(2)?,
                    parent_id: r.get(3)?,
                    created_time: r.get(4)?,
                    updated_time: r.get(5)?,
                    is_todo: r.get(6)?,
                    todo_completed: r.get(7)?,
                    markup_language: r.get(8)?,
                    tags: Vec::new(),
                })
            })
            .optional()?
            .ok_or_else(|| Error::msg("笔记不存在"))?;
        let mut n = note;
        n.tags = self.tags_for_note(id)?;
        Ok(n)
    }

    pub fn tags_for_note(&self, id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.title FROM tags t
             JOIN note_tags nt ON nt.tag_id = t.id
             WHERE nt.note_id = ?1 ORDER BY t.title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn save_note(&self, id: &str, title: &str, body: &str) -> Result<()> {
        let now = now_ms();
        let changed = self.conn.execute(
            "UPDATE notes SET title = ?2, body = ?3, updated_time = ?4, user_updated_time = ?4
             WHERE id = ?1",
            params![id, title, body, now],
        )?;
        if changed == 0 {
            return Err(Error::msg("笔记不存在，保存失败"));
        }
        Ok(())
    }

    /// Register a locally attached file as a Joplin resource. The blob itself is
    /// written by `commands::create_resource`; this row is what makes the sync
    /// engine push both `<id>.md` and `.resource/<id>` on the next run.
    pub fn create_resource(
        &self,
        id: &str,
        title: &str,
        mime: &str,
        filename: &str,
        ext: &str,
        size: i64,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO resources
               (id, title, mime, filename, created_time, updated_time,
                user_created_time, user_updated_time, file_extension, size,
                blob_updated_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5, ?5, ?6, ?7, ?5)",
            params![id, title, mime, filename, now, ext, size],
        )?;
        Ok(())
    }

    /// Move a note into another notebook. The update stamp is bumped so the
    /// WebDAV synchroniser pushes the new `parent_id` on the next run.
    pub fn move_note(&self, id: &str, folder_id: &str) -> Result<()> {
        let folders: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM folders WHERE id = ?1",
            params![folder_id],
            |row| row.get(0),
        )?;
        if folders == 0 {
            return Err(Error::msg("目标笔记本不存在"));
        }
        let now = now_ms();
        let changed = self.conn.execute(
            "UPDATE notes SET parent_id = ?2, updated_time = ?3, user_updated_time = ?3
             WHERE id = ?1 AND deleted_time = 0",
            params![id, folder_id, now],
        )?;
        if changed == 0 {
            return Err(Error::msg("笔记不存在，移动失败"));
        }
        Ok(())
    }

    pub fn create_note(&self, folder_id: &str, title: &str) -> Result<Note> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO notes
             (id, parent_id, title, body, created_time, updated_time,
              user_created_time, user_updated_time, markup_language, deleted_time)
             VALUES (?1, ?2, ?3, '', ?4, ?4, ?4, ?4, 1, 0)",
            params![id, folder_id, title, now],
        )?;
        self.get_note(&id)
    }

    /// Soft delete, matching Joplin's own semantics. Also queues the deletion
    /// so the WebDAV synchroniser can propagate it to other devices.
    pub fn delete_note(&self, id: &str) -> Result<()> {
        let now = now_ms();
        self.conn.execute(
            "UPDATE notes SET deleted_time = ?2, updated_time = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        self.conn.execute(
            "INSERT INTO deleted_items (item_type, item_id, deleted_time, sync_target)
             VALUES (1, ?1, ?2, 0)",
            params![id, now],
        )?;
        // `deleted_items` contains years of historical deletions from the
        // desktop client; the synchroniser must only replay our own deletions.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS joplin_rs_pending_deletes (
                item_id TEXT PRIMARY KEY, deleted_time INTEGER NOT NULL
             );",
        )?;
        self.conn.execute(
            "INSERT OR REPLACE INTO joplin_rs_pending_deletes (item_id, deleted_time)
             VALUES (?1, ?2)",
            params![id, now],
        )?;
        Ok(())
    }

    pub fn create_folder(&self, title: &str, parent_id: &str) -> Result<Folder> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO folders
             (id, title, parent_id, created_time, updated_time,
              user_created_time, user_updated_time, deleted_time)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?4, 0)",
            params![id, title, parent_id, now],
        )?;
        Ok(Folder {
            id,
            title: title.to_string(),
            parent_id: parent_id.to_string(),
            note_count: 0,
            updated_time: now,
        })
    }

    pub fn rename_folder(&self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE folders SET title = ?2, updated_time = ?3 WHERE id = ?1",
            params![id, title, now_ms()],
        )?;
        Ok(())
    }

    pub fn list_tags(&self) -> Result<Vec<Tag>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.title,
                    (SELECT COUNT(*) FROM note_tags nt JOIN notes n ON n.id = nt.note_id
                     WHERE nt.tag_id = t.id AND n.deleted_time = 0)
             FROM tags t ORDER BY t.title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                title: r.get(1)?,
                note_count: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn note_title_map(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title FROM notes WHERE deleted_time = 0")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }

    pub fn resource_ext_map(&self) -> Result<HashMap<String, String>> {
        let mut map = HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id, COALESCE(file_extension,'') FROM resources")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, ext) = row?;
            map.insert(id, ext);
        }
        Ok(map)
    }

    pub fn resource_mime(&self, id: &str) -> Result<String> {
        Ok(self
            .conn
            .query_row(
                "SELECT COALESCE(mime,'application/octet-stream') FROM resources WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "application/octet-stream".to_string()))
    }

    pub fn all_for_index(&self) -> Result<Vec<(String, String, String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, COALESCE(parent_id,''), title, body, updated_time
             FROM notes WHERE deleted_time = 0",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
            ))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn stats(&self, cfg: &ProfileConfig) -> Result<Stats> {
        let count = |sql: &str| -> i64 {
            self.conn
                .query_row(sql, [], |r| r.get(0))
                .unwrap_or(0)
        };
        Ok(Stats {
            notes: count("SELECT COUNT(*) FROM notes WHERE deleted_time = 0"),
            folders: count("SELECT COUNT(*) FROM folders WHERE deleted_time = 0"),
            resources: count("SELECT COUNT(*) FROM resources"),
            tags: count("SELECT COUNT(*) FROM tags"),
            data_dir: paths::data_dir().display().to_string(),
            work_db: cfg.work_db.display().to_string(),
            work_db_bytes: fs::metadata(&cfg.work_db).map(|m| m.len()).unwrap_or(0),
            synced_at: cfg.synced_at,
        })
    }
}

/// Strip markdown noise for the list excerpt. Keeps link labels, drops the
/// `:/<id>` targets that would otherwise pollute every summary.
pub fn make_excerpt(body: &str, max: usize) -> String {
    use std::sync::OnceLock;
    static LINK: OnceLock<regex::Regex> = OnceLock::new();
    let re = LINK.get_or_init(|| {
        regex::Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").expect("valid link regex")
    });
    let cleaned = re.replace_all(body, "$1");
    let mut out = String::with_capacity(max + 16);
    for ch in cleaned.chars() {
        match ch {
            '#' | '*' | '`' | '>' | '|' | '~' | '[' | ']' | '(' | ')' | '!' => continue,
            '\n' | '\r' | '\t' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                continue;
            }
            _ => out.push(ch),
        }
        if out.chars().count() >= max {
            break;
        }
    }
    out.trim().to_string()
}

pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        s.push(T[((n >> 18) & 63) as usize] as char);
        s.push(T[((n >> 12) & 63) as usize] as char);
        s.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    s
}

/// Inverse of `base64_encode`, used when the WebView hands us an attached file.
/// Whitespace is ignored so a data URL body with line breaks still decodes.
pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    fn digit(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in text.bytes().filter(|b| !b.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        let d = digit(c).ok_or_else(|| Error::msg("附件数据不是合法的 base64"))?;
        acc = (acc << 6) | d;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1u32 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_library_is_empty_and_has_the_full_schema() {
        // The whole point of the data-source change: a new data directory
        // yields zero notes, not a copy of somebody's Joplin profile.
        let dir = std::env::temp_dir().join(format!("joplin-rs-schema-{}", now_ms()));
        let path = dir.join("notes.sqlite");
        create_empty_database(&path).expect("create empty database");
        {
            let db = Db::open(&path).expect("open empty database");
            let cfg = ProfileConfig {
                work_db: path.clone(),
                resources: dir.join("resources"),
                synced_at: 0,
            };
            let stats = db.stats(&cfg).expect("stats");
            assert_eq!(stats.notes, 0);
            assert_eq!(stats.folders, 0);
            assert_eq!(stats.resources, 0);
            assert_eq!(stats.tags, 0);
            assert!(db.list_folders().expect("folders").is_empty());
            assert!(db
                .list_notes(None, None, 50, 0)
                .expect("notes")
                .is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_creation_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("joplin-rs-schema2-{}", now_ms()));
        let path = dir.join("notes.sqlite");
        create_empty_database(&path).expect("first run");
        create_empty_database(&path).expect("second run on the same file");
        {
            let conn = Connection::open(&path).expect("open");
            ensure_schema(&conn).expect("ensure schema again");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn excerpt_keeps_link_labels_and_drops_targets() {
        let body = "# 标题\n\n![图片](:/abcdef) 后面还有文字 `code`";
        let excerpt = make_excerpt(body, 100);
        assert!(!excerpt.contains(":/abcdef"));
        assert!(excerpt.contains("图片"));
        assert!(excerpt.contains("后面还有文字"));
        assert!(!excerpt.contains('#'));
    }

    #[test]
    fn list_notes_without_a_limit_returns_every_row() {
        // The note list header prints the row count, so `limit <= 0` must mean
        // "everything" — a silent cap used to make it read "500 篇笔记" for a
        // library of 919.
        let dir = std::env::temp_dir().join(format!("joplin-rs-limit-{}", now_ms()));
        let path = dir.join("notes.sqlite");
        create_empty_database(&path).expect("create empty database");
        {
            let db = Db::open(&path).expect("open");
            for i in 0..7 {
                db.create_note("f1", &format!("note {i}")).expect("create");
            }
            assert_eq!(db.list_notes(None, None, 0, 0).expect("all").len(), 7);
            assert_eq!(db.list_notes(None, None, -1, 0).expect("all").len(), 7);
            assert_eq!(db.list_notes(None, None, 3, 0).expect("capped").len(), 3);
            assert_eq!(db.list_notes(Some("f1"), None, 0, 0).expect("by folder").len(), 7);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moving_a_note_changes_its_folder_and_bumps_the_stamp() {
        // Drag-and-drop in the note list relies on this: the note has to leave
        // the old notebook, land in the new one, and look modified so the next
        // WebDAV run uploads the new parent_id.
        let dir = std::env::temp_dir().join(format!("joplin-rs-move-{}", now_ms()));
        let path = dir.join("notes.sqlite");
        create_empty_database(&path).expect("create empty database");
        {
            let db = Db::open(&path).expect("open");
            let from = db.create_folder("from", "").expect("folder");
            let to = db.create_folder("to", "").expect("folder");
            let note = db.create_note(&from.id, "movable").expect("note");
            let before = db.get_note(&note.id).expect("note");

            db.move_note(&note.id, &to.id).expect("move");
            let after = db.get_note(&note.id).expect("note");
            assert_eq!(after.parent_id, to.id);
            assert!(after.updated_time >= before.updated_time);
            assert_eq!(db.list_notes(None, None, 0, 0).expect("all").len(), 1);
            assert_eq!(
                db.list_notes(Some(&from.id), None, 0, 0).expect("from").len(),
                0
            );
            assert_eq!(
                db.list_notes(Some(&to.id), None, 0, 0).expect("to").len(),
                1
            );

            // Unknown notebooks are rejected instead of silently orphaning.
            assert!(db.move_note(&note.id, "nope").is_err());
            assert_eq!(db.get_note(&note.id).expect("note").parent_id, to.id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_padding_is_correct() {
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    }
}
