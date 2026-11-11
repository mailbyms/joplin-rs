//! Two-way synchronisation between the working-copy SQLite database and a
//! Joplin WebDAV sync target.
//!
//! Strategy (deliberately simple and conservative):
//! * `info.json` must be sync version 3 with E2EE off.
//! * Remote items are `<id>.md`; resources also have `.resource/<id>` blobs.
//! * For every id present on either side, the newer `updated_time` wins.
//! * Exception — notes edited on *both* sides since the last sync: the newer
//!   revision keeps the original id and the other one is preserved as a
//!   separate "conflict copy" note (`is_conflict = 1`), so neither edit is
//!   silently overwritten. See [`conflict_copy`].
//! * A remote item that disappeared only deletes the local copy when our sync
//!   state proves the local copy has not been edited since the last sync.
//! * Local deletions recorded in `deleted_items` are propagated to the server
//!   only when they happened after the previous successful sync.

use crate::{
    db::now_ms,
    error::{Error, Result},
    sync::{
        protocol::{
            check_info_json, is_item_filename, is_resource_id, parse_item, peek_type,
            serialize_item, table_for_type, type_for_table, SyncItem, TYPE_NOTE, TYPE_RESOURCE,
            INFO_JSON,
        },
        webdav::{RemoteFile, WebDav},
        SyncConfig,
    },
};
use rusqlite::{params, params_from_iter, types::ValueRef, Connection, OpenFlags};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Emitter};

fn err(msg: impl Into<String>) -> Error {
    Error::msg(msg.into())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub dry_run: bool,
    pub downloaded: usize,
    pub uploaded: usize,
    pub deleted_remote: usize,
    pub deleted_local: usize,
    /// Notes that were edited on both sides since the last sync. The losing
    /// revision is kept as a separate conflict copy, so nothing is lost.
    pub conflicts: usize,
    /// Human-readable one-liner per conflict, shown in the sync dialog.
    pub conflict_notes: Vec<String>,
    pub skipped: usize,
    pub resources_down: usize,
    pub resources_up: usize,
    /// Remote item types this client does not implement (e.g. Joplin note
    /// revisions, `type_=13`). They are left untouched on the server.
    pub unsupported: usize,
    pub remote_items: usize,
    pub local_items: usize,
    pub errors: Vec<String>,
    pub finished_at: i64,
    pub message: String,
}

#[derive(Debug, Clone)]
struct StateRow {
    remote_mtime: i64,
    remote_updated_time: i64,
    synced_at: i64,
    local_updated_time: i64,
}

fn unsupported_note(count: usize, types: &BTreeMap<i64, usize>) -> String {
    if count == 0 {
        return String::new();
    }
    let detail = types
        .iter()
        .map(|(t, n)| format!("type_={t}×{n}"))
        .collect::<Vec<_>>()
        .join("、");
    format!("；忽略 {count} 个不支持的条目（{detail}，远端文件保持不变）")
}

fn emit(app: &AppHandle, phase: &str, current: usize, total: usize, message: impl Into<String>) {
    let _ = app.emit(
        "sync://progress",
        SyncProgress {
            phase: phase.to_string(),
            current,
            total,
            message: message.into(),
        },
    );
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| err(format!("读取 {table} 表结构失败: {e}")))?;
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| err(format!("读取 {table} 表结构失败: {e}")))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| err(format!("读取 {table} 表结构失败: {e}")))?;
    Ok(cols)
}

/// Columns that SQLite will refuse to leave out of an INSERT.
fn required_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| err(format!("读取 {table} 表结构失败: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| err(format!("读取 {table} 表结构失败: {e}")))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| err(format!("读取 {table} 表结构失败: {e}")))?;
    Ok(rows
        .into_iter()
        .filter(|(_, notnull, default, pk)| *notnull == 1 && default.is_none() && *pk == 0)
        .map(|(name, _, _, _)| name)
        .collect())
}

fn ensure_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS joplin_rs_sync_state (
            item_id TEXT PRIMARY KEY,
            remote_mtime INTEGER NOT NULL DEFAULT 0,
            remote_updated_time INTEGER NOT NULL DEFAULT 0,
            synced_at INTEGER NOT NULL DEFAULT 0,
            local_updated_time INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS joplin_rs_sync_meta (
            key TEXT PRIMARY KEY, value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS joplin_rs_pending_deletes (
            item_id TEXT PRIMARY KEY, deleted_time INTEGER NOT NULL
         );",
    )
    .map_err(|e| err(format!("创建同步状态表失败: {e}")))
}

fn load_state(conn: &Connection) -> Result<HashMap<String, StateRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT item_id, remote_mtime, remote_updated_time, synced_at, local_updated_time
             FROM joplin_rs_sync_state",
        )
        .map_err(|e| err(format!("读取同步状态失败: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                StateRow {
                    remote_mtime: r.get(1)?,
                    remote_updated_time: r.get(2)?,
                    synced_at: r.get(3)?,
                    local_updated_time: r.get(4)?,
                },
            ))
        })
        .map_err(|e| err(format!("读取同步状态失败: {e}")))?
        .collect::<std::result::Result<HashMap<_, _>, _>>()
        .map_err(|e| err(format!("读取同步状态失败: {e}")))?;
    Ok(rows)
}

fn meta_get(conn: &Connection, key: &str) -> i64 {
    conn.query_row(
        "SELECT value FROM joplin_rs_sync_meta WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(0)
}

fn meta_set(conn: &Connection, key: &str, value: i64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO joplin_rs_sync_meta (key, value) VALUES (?1, ?2)",
        params![key, value.to_string()],
    )
    .map_err(|e| err(format!("写入同步元数据失败: {e}")))?;
    Ok(())
}

/// Read every syncable, non-deleted item from the working copy.
fn load_local_items(conn: &Connection) -> Result<HashMap<String, SyncItem>> {
    let mut out = HashMap::new();
    for table in ["notes", "folders", "resources", "tags", "note_tags"] {
        let type_ = type_for_table(table).unwrap();
        let cols = table_columns(conn, table)?;
        let where_clause = if cols.iter().any(|c| c == "deleted_time") {
            " WHERE deleted_time = 0"
        } else {
            ""
        };
        let select = cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT {select} FROM {table}{where_clause}");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| err(format!("读取本地 {table} 失败: {e}")))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| err(format!("读取本地 {table} 失败: {e}")))?;
        while let Some(row) = rows
            .next()
            .map_err(|e| err(format!("遍历本地 {table} 失败: {e}")))?
        {
            let mut fields = BTreeMap::new();
            for (i, col) in cols.iter().enumerate() {
                let value = match row
                    .get_ref(i)
                    .map_err(|e| err(format!("读取本地 {table} 字段失败: {e}")))?
                {
                    ValueRef::Null => String::new(),
                    ValueRef::Integer(n) => n.to_string(),
                    ValueRef::Real(f) => f.to_string(),
                    ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                    ValueRef::Blob(_) => String::new(),
                };
                fields.insert(col.clone(), value);
            }
            let id = fields.get("id").cloned().unwrap_or_default();
            if !is_resource_id(&id) {
                continue;
            }
            let title = fields.remove("title").unwrap_or_default();
            let body = fields.remove("body").unwrap_or_default();
            out.insert(
                id.clone(),
                SyncItem {
                    id,
                    type_,
                    title,
                    body,
                    fields,
                },
            );
        }
    }
    Ok(out)
}

/// Merge a remote item into the working copy. Only columns shipped by the
/// remote item are touched: an `INSERT OR REPLACE` would silently zero out
/// local-only fields (and blow up on NOT NULL columns a phone client omits).
fn upsert_item(conn: &Connection, item: &SyncItem) -> Result<()> {
    let table = item
        .table()
        .ok_or_else(|| err(format!("未知条目类型 {}", item.type_)))?;
    let cols = table_columns(conn, table)?;
    let mut values = item.fields.clone();
    values.insert("id".into(), item.id.clone());
    values.insert("title".into(), item.title.clone());
    if item.type_ == 1 {
        values.insert("body".into(), item.body.clone());
    }
    // For an existing row a plain UPDATE is enough and preserves every field
    // the remote file did not send. For a new row we also have to supply any
    // NOT NULL columns that have no DEFAULT (a phone client may omit them).
    let exists: bool = conn
        .query_row(
            &format!("SELECT 1 FROM {table} WHERE id = ?1"),
            params![item.id],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !exists {
        let now = now_ms().to_string();
        for key in required_columns(conn, table)? {
            if values.contains_key(&key) {
                continue;
            }
            values.insert(key.clone(), if key.contains("time") { now.clone() } else { "0".into() });
        }
    }
    let used: Vec<String> = cols
        .into_iter()
        .filter(|c| values.contains_key(c))
        .collect();
    let editable: Vec<&String> = used.iter().filter(|c| c.as_str() != "id").collect();
    let (sql, args): (String, Vec<String>) = if exists {
        if editable.is_empty() {
            return Ok(());
        }
        (
            format!(
                "UPDATE {table} SET {} WHERE id = ?",
                editable
                    .iter()
                    .map(|c| format!("\"{c}\" = ?"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            editable
                .iter()
                .map(|c| values[*c].clone())
                .chain(std::iter::once(item.id.clone()))
                .collect(),
        )
    } else {
        (
            format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                used.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "),
                used.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
            ),
            used.iter().map(|c| values[c].clone()).collect(),
        )
    };
    conn.execute(&sql, params_from_iter(args))
        .map_err(|e| err(format!("写入 {table} 条目 {} 失败: {e}", item.id)))?;
    Ok(())
}

fn soft_delete_local(conn: &Connection, item: &SyncItem) -> Result<()> {
    let table = item.table().unwrap();
    let cols = table_columns(conn, table)?;
    if cols.iter().any(|c| c == "deleted_time") {
        conn.execute(
            &format!("UPDATE {table} SET deleted_time = ?1 WHERE id = ?2"),
            params![now_ms(), item.id],
        )
    } else {
        conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), params![item.id])
    }
    .map_err(|e| err(format!("本地删除 {} 失败: {e}", item.id)))?;
    Ok(())
}

/// `2026-11-11 19:12` — local time, so the title reads naturally in the list.
fn conflict_stamp(at: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(at)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| at.to_string())
}

fn conflict_title(original: &str, at: i64) -> String {
    let base = original.trim();
    let base = if base.is_empty() { "无标题笔记" } else { base };
    format!("{base}（冲突副本 {}）", conflict_stamp(at))
}

/// True when two revisions carry the same visible content. Equal
/// `updated_time`s are not required: a client may rewrite the file (or bump
/// the timestamp) without changing a single character, and duplicating such a
/// note would only add noise.
fn same_note_content(a: &SyncItem, b: &SyncItem) -> bool {
    a.title == b.title && a.body == b.body
}

/// Build the "loser" of a conflict as a brand-new note.
///
/// Two clients editing the same note is a real possibility (desktop + phone,
/// or two machines). Plain last-writer-wins destroys the other edit, so the
/// revision that loses the `updated_time` comparison is re-materialised under
/// a fresh 32-hex id, flagged `is_conflict = 1` (the column the official
/// client also uses) and titled after the original with a timestamp. Both
/// sides then converge on two notes instead of one silently-clobbered one.
fn conflict_copy(loser: &SyncItem, now: i64) -> SyncItem {
    let mut fields = loser.fields.clone();
    // `type_` is a private property written by `serialize_item` itself;
    // keeping the parsed copy would emit a duplicate line on upload.
    fields.remove("type_");
    fields.remove("id");
    fields.insert("created_time".into(), now.to_string());
    fields.insert("updated_time".into(), now.to_string());
    fields.insert("user_created_time".into(), now.to_string());
    fields.insert("user_updated_time".into(), now.to_string());
    fields.insert("is_conflict".into(), "1".into());
    fields.insert("conflict_original_id".into(), loser.id.clone());
    fields.insert("deleted_time".into(), "0".into());
    fields.insert("encryption_applied".into(), "0".into());
    SyncItem {
        id: uuid::Uuid::new_v4().simple().to_string(),
        type_: TYPE_NOTE,
        title: conflict_title(&loser.title, now),
        body: loser.body.clone(),
        fields,
    }
}

/// Which side of a two-sided item wins when neither has a conflict to preserve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Merge {
    TakeRemote,
    TakeLocal,
    InSync,
}

/// Newer `updated_time` wins; equal timestamps mean nothing to do.
fn merge_decision(local: &SyncItem, remote: &SyncItem) -> Merge {
    let lu = local.updated_time();
    let ru = remote.updated_time();
    if ru > lu {
        Merge::TakeRemote
    } else if lu > ru {
        Merge::TakeLocal
    } else {
        Merge::InSync
    }
}

/// A genuine conflict: a note that this client modified *and* that somebody
/// else modified on the sync target since the last time we reconciled it.
///
/// `prev` is the sync-state row written by the previous run, holding the
/// `updated_time` each side had back then. Without that row (first sync of the
/// id, or a database that predates state tracking) two-sided edits are
/// indistinguishable from a plain one-sided one, and we fall back to
/// last-writer-wins instead of spraying conflict copies.
fn is_conflict(local: &SyncItem, remote: &SyncItem, prev: Option<&StateRow>) -> bool {
    // Duplicating a folder forks the tree and duplicating an attachment costs
    // storage for identical bytes, so only notes get a conflict copy.
    if local.type_ != TYPE_NOTE {
        return false;
    }
    let lu = local.updated_time();
    let ru = remote.updated_time();
    if lu == ru {
        return false;
    }
    let Some(prev) = prev else {
        return false;
    };
    let local_changed = prev.local_updated_time != lu;
    let remote_changed = prev.remote_updated_time != ru;
    local_changed && remote_changed && !same_note_content(local, remote)
}

/// Older Joplin profiles leave `resources.file_extension` empty even though the
/// blob on disk is named `<id>.png`. In that case fall back to a prefix scan.
/// 45 of the 143 attachments in the reference profile are affected.
fn prefix_resource_path(id: &str, dir: &Path) -> Option<PathBuf> {
    let mut bare: Option<PathBuf> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(id) else {
            continue;
        };
        if rest.starts_with('.') {
            return Some(path);
        }
        if rest.is_empty() && bare.is_none() {
            bare = Some(path);
        }
    }
    bare
}

fn local_resource_path(item: &SyncItem, resources_dir: &Path) -> Option<PathBuf> {
    let ext = item.fields.get("file_extension").cloned().unwrap_or_default();
    if !ext.is_empty() {
        let with_ext = resources_dir.join(format!("{}.{}", item.id, ext));
        if with_ext.is_file() {
            return Some(with_ext);
        }
    }
    let bare = resources_dir.join(&item.id);
    if bare.is_file() {
        return Some(bare);
    }
    prefix_resource_path(&item.id, resources_dir)
}

fn app_resource_dir() -> PathBuf {
    crate::paths::resources_dir()
}

fn find_local_resource(item: &SyncItem, resources_dir: &Path) -> Option<PathBuf> {
    local_resource_path(item, resources_dir)
        .or_else(|| local_resource_path(item, &app_resource_dir()))
}

pub fn run(
    app: &AppHandle,
    work_db: &Path,
    resources_dir: &Path,
    cfg: &SyncConfig,
    dry: bool,
) -> Result<SyncResult> {
    let mut result = SyncResult {
        dry_run: dry,
        finished_at: now_ms(),
        ..Default::default()
    };
    let dav = WebDav::new(cfg)?;

    emit(app, "connect", 0, 1, "连接 WebDAV 服务器…");
    let root = dav.propfind("", "1")?;
    emit(app, "connect", 1, 1, "连接成功，检查同步目录…");

    // info.json: version 3, E2EE off. Create it only for a genuinely new target.
    let info_present = root.iter().any(|f| f.name == "info.json");
    match dav.get_text("info.json")? {
        Some(text) => {
            check_info_json(&text)?;
            if !dry {
                // Keep whatever the official client wrote; never rewrite it.
            }
        }
        None => {
            if !dry {
                dav.put("info.json", INFO_JSON.as_bytes())?;
                result.uploaded += 1;
            }
        }
    }
    if !info_present && dry {
        result.message.push_str("（新同步目标：将创建 info.json）");
    }

    // Remote item files and resource blobs.
    let remote_files: Vec<&RemoteFile> = root
        .iter()
        .filter(|f| !f.is_dir && is_item_filename(&f.name))
        .collect();
    let resource_listing = dav.propfind(".resource", "1").unwrap_or_default();
    let remote_blobs: HashSet<String> = resource_listing
        .iter()
        .filter(|f| !f.is_dir && is_resource_id(&f.name))
        .map(|f| f.name.clone())
        .collect();
    if resource_listing.is_empty() && !dry {
        let _ = dav.mkcol(".resource");
    }

    let mut conn = Connection::open_with_flags(
        work_db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|e| err(format!("打开工作副本失败: {e}")))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| err(format!("设置数据库超时失败: {e}")))?;
    if !dry {
        ensure_tables(&conn)?;
    }
    let last_sync = meta_get(&conn, "last_sync");
    let state = load_state(&conn).unwrap_or_default();
    let local = load_local_items(&conn)?;
    result.local_items = local.len();
    result.remote_items = remote_files.len();

    // Deletions made from this app. Joplin's `deleted_items` holds years of
    // history, so we keep a small dedicated queue rather than replaying it on
    // the live sync target.
    let mut pending_deletes: HashMap<String, i64> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT item_id, deleted_time FROM joplin_rs_pending_deletes
         WHERE deleted_time > ?1",
    ) {
        if let Ok(rows) = stmt.query_map(params![last_sync], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        }) {
            for (id, at) in rows.flatten() {
                pending_deletes.insert(id, at);
            }
        }
    }

    // Fetch only files whose mtime changed (or that we have never seen).
    let mut remote: HashMap<String, (SyncItem, i64)> = HashMap::new();
    let mut unsupported_types: BTreeMap<i64, usize> = BTreeMap::new();
    let total = remote_files.len();
    for (i, f) in remote_files.iter().enumerate() {
        // Report at the top of the iteration: files whose mtime matches the
        // sync state are `continue`d below, so a trailing emit would stop the
        // counter at the last *uncached* file and never reach `total`.
        if i % 50 == 0 || i + 1 == total {
            emit(
                app,
                "download",
                i + 1,
                total,
                format!("已获取条目：{}/{}", i + 1, total),
            );
        }
        let id = f.name.trim_end_matches(".md").to_string();
        if let Some(s) = state.get(&id) {
            if s.remote_mtime > 0 && s.remote_mtime == f.mtime_ms {
                // Reuse the cached item metadata without a GET.
                if let Some(item) = local.get(&id) {
                    let mut cached = item.clone();
                    cached
                        .fields
                        .insert("updated_time".into(), s.remote_updated_time.to_string());
                    remote.insert(id.clone(), (cached, f.mtime_ms));
                }
            }
        }
        if remote.contains_key(&id) {
            continue;
        }
        // A Joplin sync target also holds item types we do not model, above
        // all note revisions (`type_=13`). Their files use a different layout
        // (no title/body segments, quoted JSON diffs), so recognise them by
        // their trailing `type_` property and leave them completely alone
        // instead of letting the parser report a bogus error.
        let outcome: std::result::Result<Option<(SyncItem, i64)>, String> =
            match dav.get_text(&f.name) {
                Ok(Some(text)) => {
                    match peek_type(&text).filter(|t| table_for_type(*t).is_none()) {
                        Some(t) => {
                            result.unsupported += 1;
                            *unsupported_types.entry(t).or_insert(0) += 1;
                            Ok(None)
                        }
                        None => parse_item(&text)
                            .map(|item| Some((item, f.mtime_ms)))
                            .map_err(|e| e.to_string()),
                    }
                }
                Ok(None) => Err("文件在列举后消失".to_string()),
                Err(e) => Err(e.to_string()),
            };
        match outcome {
            Ok(Some((item, mtime))) => {
                // A tombstone (`deleted_time > 0`) is Joplin's deletion marker,
                // not a note to restore: treat it as absent so the local copy
                // follows the ordinary remote-deletion path. Skipping this made
                // deleted notes re-apply on every sync — their rows are hidden
                // from `local` by `deleted_time = 0`, so the `(None, Some)` arm
                // kept counting them as fresh downloads.
                if !item.is_tombstone() {
                    remote.insert(id.clone(), (item, mtime));
                }
            }
            Ok(None) => {}
            Err(e) => result.errors.push(format!("{}: {e}", f.name)),
        }
    }

    // Decide what to do for every id.
    let mut ids: HashSet<String> = local.keys().cloned().collect();
    ids.extend(remote.keys().cloned());
    let mut to_apply: Vec<SyncItem> = Vec::new();
    let mut to_upload: Vec<SyncItem> = Vec::new();
    let mut to_delete_local: Vec<SyncItem> = Vec::new();
    let mut to_delete_remote: Vec<String> = Vec::new();
    let mut new_state: HashMap<String, StateRow> = HashMap::new();
    let mut resources_to_get: Vec<(String, SyncItem)> = Vec::new();
    let mut resources_to_put: Vec<(String, SyncItem)> = Vec::new();

    for id in &ids {
        let l = local.get(id);
        let r = remote.get(id).map(|(item, _)| item);
        match (l, r) {
            (None, Some(remote_item)) => {
                // A note deleted here since the last sync must not be
                // resurrected by a stale copy that is still on the server.
                if let Some(&deleted_at) = pending_deletes.get(id) {
                    if deleted_at >= remote_item.updated_time()
                        && !to_delete_remote.contains(id)
                    {
                        to_delete_remote.push(id.clone());
                        continue;
                    }
                }
                to_apply.push(remote_item.clone());
                result.downloaded += 1;
                if remote_item.type_ == TYPE_RESOURCE {
                    resources_to_get.push((id.clone(), remote_item.clone()));
                }
                new_state.insert(
                    id.clone(),
                    StateRow {
                        remote_mtime: remote.get(id).map(|(_, m)| *m).unwrap_or(0),
                        remote_updated_time: remote_item.updated_time(),
                        synced_at: now_ms(),
                        local_updated_time: remote_item.updated_time(),
                    },
                );
            }
            (Some(local_item), None) => {
                // If we synced this id before and the local copy has not changed
                // since then, its disappearance means a remote deletion.
                let remotely_deleted = state
                    .get(id)
                    .map(|s| s.synced_at > 0 && local_item.updated_time() <= s.synced_at)
                    .unwrap_or(false);
                if remotely_deleted {
                    to_delete_local.push(local_item.clone());
                    result.deleted_local += 1;
                } else {
                    to_upload.push(local_item.clone());
                    result.uploaded += 1;
                    if local_item.type_ == TYPE_RESOURCE {
                        resources_to_put.push((id.clone(), local_item.clone()));
                    }
                    new_state.insert(
                        id.clone(),
                        StateRow {
                            remote_mtime: 0, // force a re-read next run
                            remote_updated_time: local_item.updated_time(),
                            synced_at: now_ms(),
                            local_updated_time: local_item.updated_time(),
                        },
                    );
                }
            }
            (Some(local_item), Some(remote_item)) => {
                let lu = local_item.updated_time();
                let ru = remote_item.updated_time();
                // The state row records the `updated_time` both sides had when
                // the item was last reconciled; that is what separates a real
                // two-sided edit from an ordinary upload/download.
                let prev = state.get(id);
                let conflict = is_conflict(local_item, remote_item, prev);
                // Newer wins (also the fallback when there is no state row,
                // i.e. the very first sync of an id).
                let merge = merge_decision(local_item, remote_item);
                let download = merge == Merge::TakeRemote;
                let upload = merge == Merge::TakeLocal;
                if conflict {
                    let now = now_ms();
                    let loser = if download { local_item } else { remote_item };
                    let copy = conflict_copy(loser, now);
                    // Write the preserved revision into the working copy and
                    // push it to the server in the same run. Deliberately no
                    // sync-state row: if the upload fails, the next run must
                    // still see it as a new local note instead of mistaking it
                    // for a remotely deleted one.
                    to_apply.push(copy.clone());
                    to_upload.push(copy);
                    result.conflicts += 1;
                    result.conflict_notes.push(format!(
                        "「{}」两端都有修改，已保留较新的一份，另一份另存为「{}」",
                        if local_item.title.trim().is_empty() {
                            local_item.id.as_str()
                        } else {
                            local_item.title.trim()
                        },
                        conflict_title(&loser.title, now)
                    ));
                }
                if download {
                    to_apply.push(remote_item.clone());
                    result.downloaded += 1;
                    if remote_item.type_ == TYPE_RESOURCE {
                        resources_to_get.push((id.clone(), remote_item.clone()));
                    }
                    new_state.insert(
                        id.clone(),
                        StateRow {
                            remote_mtime: remote.get(id).map(|(_, m)| *m).unwrap_or(0),
                            remote_updated_time: ru,
                            synced_at: now_ms(),
                            local_updated_time: ru,
                        },
                    );
                } else if upload {
                    to_upload.push(local_item.clone());
                    result.uploaded += 1;
                    if local_item.type_ == TYPE_RESOURCE {
                        resources_to_put.push((id.clone(), local_item.clone()));
                    }
                    new_state.insert(
                        id.clone(),
                        StateRow {
                            remote_mtime: 0,
                            remote_updated_time: lu,
                            synced_at: now_ms(),
                            local_updated_time: lu,
                        },
                    );
                } else {
                    result.skipped += 1;
                    new_state.insert(
                        id.clone(),
                        StateRow {
                            remote_mtime: remote.get(id).map(|(_, m)| *m).unwrap_or(0),
                            remote_updated_time: ru,
                            synced_at: now_ms(),
                            local_updated_time: lu,
                        },
                    );
                }
            }
            (None, None) => {}
        }
    }

    // Resource blobs live independently of their `.md` metadata. A previous
    // run can have uploaded the metadata while the binary failed, and the
    // updated_time comparison would then never schedule the blob again.
    for (id, item) in &local {
        if item.type_ != TYPE_RESOURCE
            || remote_blobs.contains(id)
            || to_delete_remote.contains(id)
            || resources_to_put.iter().any(|(x, _)| x == id)
        {
            continue;
        }
        resources_to_put.push((id.clone(), item.clone()));
    }
    for (id, (item, _)) in &remote {
        if item.type_ != TYPE_RESOURCE
            || !remote_blobs.contains(id)
            || resources_to_get.iter().any(|(x, _)| x == id)
            || find_local_resource(item, resources_dir).is_some()
        {
            continue;
        }
        resources_to_get.push((id.clone(), item.clone()));
    }

    // Pending deletions whose remote file is already gone (or that never
    // existed) only need to be dropped from the queue. Those still present
    // remotely were added to `to_delete_remote` by the loop above.
    let mut already_gone: Vec<String> = Vec::new();
    for id in pending_deletes.keys() {
        if !to_delete_remote.contains(id) {
            already_gone.push(id.clone());
        }
    }

    emit(
        app,
        "apply",
        0,
        1,
        format!(
            "计划：下载 {} · 上传 {} · 本地删除 {} · 远端删除 {}{}{}",
            result.downloaded,
            result.uploaded,
            result.deleted_local,
            to_delete_remote.len(),
            if result.conflicts > 0 {
                format!(" · 冲突 {}", result.conflicts)
            } else {
                String::new()
            },
            unsupported_note(result.unsupported, &unsupported_types)
        ),
    );

    if dry {
        result.deleted_remote = to_delete_remote.len();
        result.resources_down = resources_to_get
            .iter()
            .filter(|(id, _)| remote_blobs.contains(id))
            .count();
        result.resources_up = resources_to_put
            .iter()
            .filter(|(_, item)| find_local_resource(item, resources_dir).is_some())
            .count();
        result.message = format!(
            "预演完成：下载 {}、上传 {}、跳过 {}、本地删除 {}、远端删除 {}{}{}",
            result.downloaded,
            result.uploaded,
            result.skipped,
            result.deleted_local,
            result.deleted_remote,
            if result.conflicts > 0 {
                format!("、冲突 {}", result.conflicts)
            } else {
                String::new()
            },
            unsupported_note(result.unsupported, &unsupported_types)
        );
        return Ok(result);
    }

    // --- Apply remote changes locally (single DB transaction). ---
    // Milestone for the sidebar ticker: the plan is final here, so the number of
    // items about to be written into the local library is already known.
    if result.downloaded > 0 {
        emit(
            app,
            "create",
            0,
            1,
            format!("已新建本地条目：{}", result.downloaded),
        );
    }
    let tx = conn
        .transaction()
        .map_err(|e| err(format!("开启数据库事务失败: {e}")))?;
    for item in &to_apply {
        if let Err(e) = upsert_item(&tx, item) {
            result.errors.push(e.to_string());
        }
    }
    for item in &to_delete_local {
        if let Err(e) = soft_delete_local(&tx, item) {
            result.errors.push(e.to_string());
        }
    }
    for (id, row) in &new_state {
        if let Err(e) = tx.execute(
            "INSERT OR REPLACE INTO joplin_rs_sync_state
             (item_id, remote_mtime, remote_updated_time, synced_at, local_updated_time)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                row.remote_mtime,
                row.remote_updated_time,
                row.synced_at,
                row.local_updated_time
            ],
        ) {
            result.errors.push(format!("写入同步状态 {id} 失败: {e}"));
        }
    }
    for id in &to_delete_remote {
        let _ = tx.execute(
            "DELETE FROM joplin_rs_sync_state WHERE item_id = ?1",
            params![id],
        );
    }
    if let Err(e) = meta_set(&tx, "last_sync", now_ms()) {
        result.errors.push(e.to_string());
    }
    tx.commit()
        .map_err(|e| err(format!("提交数据库事务失败: {e}")))?;

    // --- Upload local changes and deletions. ---
    // Joplin puts a resource's binary before its `<id>.md` metadata: once
    // another client sees the metadata it must already be able to fetch the
    // blob. Upload every blob first, then every metadata file.
    for (id, item) in &resources_to_put {
        let Some(path) = find_local_resource(item, resources_dir) else {
            // A metadata row without a local blob used to be skipped silently,
            // showing "附件 ↑0" with no hint about what went wrong.
            result
                .errors
                .push(format!("附件 {id} 的本地文件不存在，无法上传"));
            continue;
        };
        match fs::read(&path) {
            Ok(bytes) => match dav.put(&format!(".resource/{id}"), &bytes) {
                Ok(()) => result.resources_up += 1,
                Err(e) => result.errors.push(e.to_string()),
            },
            Err(e) => result.errors.push(format!("读取本地资源 {id} 失败: {e}")),
        }
    }
    for (i, item) in to_upload.iter().enumerate() {
        let text = serialize_item(item);
        if let Err(e) = dav.put(&format!("{}.md", item.id), text.as_bytes()) {
            result.errors.push(e.to_string());
        }
        if i % 50 == 0 || i + 1 == to_upload.len() {
            emit(
                app,
                "upload",
                i + 1,
                to_upload.len(),
                format!("已上传条目：{}/{}", i + 1, to_upload.len()),
            );
        }
    }
    let mut cleared_deletes: Vec<String> = already_gone;
    for id in &to_delete_remote {
        match dav.delete(&format!("{id}.md")) {
            Ok(()) => {
                let _ = dav.delete(&format!(".resource/{id}"));
                result.deleted_remote += 1;
                cleared_deletes.push(id.clone());
            }
            Err(e) => result.errors.push(e.to_string()),
        }
    }
    // Only dequeue deletions that were actually applied, so a transient
    // WebDAV failure is retried on the next run.
    for id in &cleared_deletes {
        let _ = conn.execute(
            "DELETE FROM joplin_rs_pending_deletes WHERE item_id = ?1",
            params![id],
        );
    }

    // --- Download resource blobs into our own app-data resources directory. ---
    let target_dir = app_resource_dir();
    if !resources_to_get.is_empty() {
        fs::create_dir_all(&target_dir)
            .map_err(|e| err(format!("创建资源目录失败: {e}")))?;
    }
    for (id, item) in &resources_to_get {
        if !remote_blobs.contains(id) {
            continue;
        }
        match dav.get(&format!(".resource/{id}")) {
            Ok(Some(bytes)) => {
                let ext = item.fields.get("file_extension").cloned().unwrap_or_default();
                let name = if ext.is_empty() {
                    id.clone()
                } else {
                    format!("{id}.{ext}")
                };
                match fs::write(target_dir.join(name), bytes) {
                    Ok(()) => result.resources_down += 1,
                    Err(e) => result.errors.push(format!("保存资源 {id} 失败: {e}")),
                }
            }
            Ok(None) => {}
            Err(e) => result.errors.push(e.to_string()),
        }
    }

    result.finished_at = now_ms();
    result.message = format!(
        "同步完成：下载 {}、上传 {}、跳过 {}、本地删除 {}、远端删除 {}、附件 ↓{} ↑{}{}{}",
        result.downloaded,
        result.uploaded,
        result.skipped,
        result.deleted_local,
        result.deleted_remote,
        result.resources_down,
        result.resources_up,
        if result.conflicts > 0 {
            format!("、冲突 {}", result.conflicts)
        } else {
            String::new()
        },
        unsupported_note(result.unsupported, &unsupported_types)
    );
    emit(app, "done", 1, 1, result.message.clone());
    let _ = app.emit("sync://done", result.clone());
    Ok(result)
}

/// Lightweight connection check used by the settings dialog.
pub fn test_connection(cfg: &SyncConfig) -> Result<String> {
    let dav = WebDav::new(cfg)?;
    let root = dav.propfind("", "1")?;
    let items = root
        .iter()
        .filter(|f| !f.is_dir && is_item_filename(&f.name))
        .count();
    let mut notes = Vec::new();
    match dav.get_text("info.json")? {
        Some(text) => {
            check_info_json(&text)?;
            notes.push("info.json: sync v3 / 未加密".to_string());
        }
        None => notes.push("info.json 不存在（首次同步时会自动创建）".to_string()),
    }
    Ok(format!(
        "连接成功：{} 个同步条目，{} 个文件/目录。{}",
        items,
        root.len(),
        notes.join("；")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, title: &str, body: &str, updated: i64) -> SyncItem {
        let mut fields = BTreeMap::new();
        fields.insert("updated_time".into(), updated.to_string());
        fields.insert("parent_id".into(), "0f1e2d3c4b5a69788796a5b4c3d2e1f0".into());
        SyncItem {
            id: id.into(),
            type_: TYPE_NOTE,
            title: title.into(),
            body: body.into(),
            fields,
        }
    }

    fn state_row(local: i64, remote: i64) -> StateRow {
        StateRow {
            remote_mtime: 0,
            remote_updated_time: remote,
            synced_at: 1_700_000_000_000,
            local_updated_time: local,
        }
    }

    #[test]
    fn flags_conflict_only_when_both_sides_changed() {
        let id = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
        let prev = state_row(100, 100); // both sides were at updated_time 100

        // Only we edited -> plain upload, no conflict.
        let local = note(id, "标题", "本地新内容", 200);
        let remote = note(id, "标题", "旧内容", 100);
        assert!(!is_conflict(&local, &remote, Some(&prev)));
        assert_eq!(merge_decision(&local, &remote), Merge::TakeLocal);

        // Only the other client edited -> plain download, no conflict.
        let local = note(id, "标题", "旧内容", 100);
        let remote = note(id, "标题", "远端新内容", 200);
        assert!(!is_conflict(&local, &remote, Some(&prev)));
        assert_eq!(merge_decision(&local, &remote), Merge::TakeRemote);

        // Both edited -> conflict, newer side still wins the original id.
        let local = note(id, "标题", "本地新内容", 300);
        let remote = note(id, "标题", "远端新内容", 200);
        assert!(is_conflict(&local, &remote, Some(&prev)));
        assert_eq!(merge_decision(&local, &remote), Merge::TakeLocal);

        // Both edited, remote is the newer one.
        let local = note(id, "标题", "本地新内容", 200);
        let remote = note(id, "标题", "远端新内容", 300);
        assert!(is_conflict(&local, &remote, Some(&prev)));
        assert_eq!(merge_decision(&local, &remote), Merge::TakeRemote);
    }

    #[test]
    fn no_conflict_without_state_or_for_identical_content() {
        let id = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
        let local = note(id, "标题", "本地新内容", 300);
        let remote = note(id, "标题", "远端新内容", 200);
        // First sync of this id: nothing proves both sides were edited.
        assert!(!is_conflict(&local, &remote, None));
        // Both timestamps moved but the content is byte-identical (a client
        // re-wrote the file without touching a character).
        let remote_same = note(id, "标题", "本地新内容", 200);
        assert!(!is_conflict(&local, &remote_same, Some(&state_row(100, 100))));
        // Folders never get a conflict copy.
        let mut folder_local = note(id, "工作", "a", 300);
        folder_local.type_ = 2;
        let mut folder_remote = note(id, "工作", "b", 200);
        folder_remote.type_ = 2;
        assert!(!is_conflict(
            &folder_local,
            &folder_remote,
            Some(&state_row(100, 100))
        ));
    }

    #[test]
    fn conflict_copy_gets_new_id_flag_and_keeps_loser_body() {
        let loser = note(
            "a1b2c3d4e5f60718293a4b5c6d7e8f90",
            "购物清单",
            "- 牛奶\n- 面包",
            1_700_000_000_000,
        );
        let copy = conflict_copy(&loser, 1_700_000_100_000);
        assert_eq!(copy.id.len(), 32);
        assert_ne!(copy.id, loser.id);
        assert!(copy.id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(copy.type_, TYPE_NOTE);
        assert_eq!(copy.body, "- 牛奶\n- 面包");
        assert!(copy.title.starts_with("购物清单（冲突副本 "));
        assert!(copy.title.ends_with("）"));
        assert_eq!(copy.fields.get("is_conflict").map(String::as_str), Some("1"));
        assert_eq!(
            copy.fields.get("conflict_original_id").map(String::as_str),
            Some("a1b2c3d4e5f60718293a4b5c6d7e8f90")
        );
        assert_eq!(
            copy.fields.get("updated_time").map(String::as_str),
            Some("1700000100000")
        );
        // The serialized upload must carry exactly one `type_` line.
        let text = serialize_item(&copy);
        assert_eq!(text.matches("type_: 1").count(), 1);
        assert!(text.starts_with("购物清单（冲突副本"));
    }

    #[test]
    fn unsupported_note_reports_types_and_is_absent_when_zero() {
        let mut types = BTreeMap::new();
        types.insert(13_i64, 56_usize);
        assert_eq!(
            unsupported_note(56, &types),
            "；忽略 56 个不支持的条目（type_=13×56，远端文件保持不变）"
        );
        assert_eq!(unsupported_note(0, &types), "");
    }

    #[test]
    fn finds_resource_when_file_extension_is_absent() {
        let dir = std::env::temp_dir().join(format!(
            "joplin-rs-prefix-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let id = "21340daeb9ff988443f466aee9538c16";
        fs::write(dir.join(format!("{id}.png")), b"fake png").unwrap();
        let item = SyncItem {
            id: id.into(),
            type_: TYPE_RESOURCE,
            title: String::new(),
            body: String::new(),
            fields: BTreeMap::new(), // no file_extension column value
        };
        let found = local_resource_path(&item, &dir).unwrap();
        assert_eq!(found.file_name().unwrap(), format!("{id}.png").as_str());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upsert_merges_remote_fields_without_wiping_local_ones() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                body TEXT NOT NULL DEFAULT '',
                parent_id TEXT NOT NULL DEFAULT '',
                created_time INT NOT NULL,
                updated_time INT NOT NULL,
                source TEXT NOT NULL DEFAULT '',
                deleted_time INT NOT NULL DEFAULT 0
             );
             INSERT INTO notes (id, title, body, parent_id, created_time, updated_time, source)
             VALUES ('a1b2c3d4e5f60718293a4b5c6d7e8f90', 'old', 'old body',
                     'folder-x', 1, 1, 'joplin-desktop');",
        )
        .unwrap();
        let mut fields = BTreeMap::new();
        fields.insert("id".into(), "a1b2c3d4e5f60718293a4b5c6d7e8f90".into());
        fields.insert("updated_time".into(), "999".into());
        let item = SyncItem {
            id: "a1b2c3d4e5f60718293a4b5c6d7e8f90".into(),
            type_: 1,
            title: "new".into(),
            body: "new body".into(),
            fields,
        };
        upsert_item(&conn, &item).unwrap();
        let (title, body, parent, source, created, updated): (
            String,
            String,
            String,
            String,
            i64,
            i64,
        ) = conn
            .query_row(
                "SELECT title, body, parent_id, source, created_time, updated_time FROM notes",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(title, "new");
        assert_eq!(body, "new body");
        assert_eq!(parent, "folder-x");
        assert_eq!(source, "joplin-desktop");
        assert_eq!(created, 1);
        assert_eq!(updated, 999);
    }
}
