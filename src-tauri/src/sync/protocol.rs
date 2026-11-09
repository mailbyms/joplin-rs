//! Joplin sync-target item format.
//!
//! A remote item is `<32-hex-id>.md` containing up to three blank-line
//! separated segments: `title`, `body` (notes only), then `key: value` lines.
//! This mirrors `BaseItem.serialize` / `BaseItem.unserialize` in Joplin.

use crate::error::{Error, Result};
use chrono::DateTime;
use std::collections::BTreeMap;

pub const TYPE_NOTE: i64 = 1;
pub const TYPE_FOLDER: i64 = 2;
pub const TYPE_RESOURCE: i64 = 4;
pub const TYPE_TAG: i64 = 5;
pub const TYPE_NOTE_TAG: i64 = 6;

const TIME_KEYS_WRITE: &[&str] = &[
    "created_time",
    "updated_time",
    "sync_time",
    "user_created_time",
    "user_updated_time",
];
const TIME_KEYS_READ: &[&str] = &[
    "created_time",
    "updated_time",
    "user_created_time",
    "user_updated_time",
];

#[derive(Debug, Clone, Default)]
pub struct SyncItem {
    pub id: String,
    pub type_: i64,
    pub title: String,
    pub body: String,
    /// All other Joplin model fields, already converted to database values
    /// (timestamps are Unix milliseconds as decimal strings).
    pub fields: BTreeMap<String, String>,
}

impl SyncItem {
    pub fn updated_time(&self) -> i64 {
        self.fields
            .get("updated_time")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    }

    pub fn table(&self) -> Option<&'static str> {
        table_for_type(self.type_)
    }

    /// Joplin keeps deleted items on the sync target as tombstones
    /// (`deleted_time > 0`) instead of removing the file.
    pub fn is_tombstone(&self) -> bool {
        self.fields
            .get("deleted_time")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
            > 0
    }
}

pub fn table_for_type(t: i64) -> Option<&'static str> {
    match t {
        TYPE_NOTE => Some("notes"),
        TYPE_FOLDER => Some("folders"),
        TYPE_RESOURCE => Some("resources"),
        TYPE_TAG => Some("tags"),
        TYPE_NOTE_TAG => Some("note_tags"),
        _ => None,
    }
}

pub fn type_for_table(t: &str) -> Option<i64> {
    match t {
        "notes" => Some(TYPE_NOTE),
        "folders" => Some(TYPE_FOLDER),
        "resources" => Some(TYPE_RESOURCE),
        "tags" => Some(TYPE_TAG),
        "note_tags" => Some(TYPE_NOTE_TAG),
        _ => None,
    }
}

pub fn is_item_filename(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".md") else {
        return false;
    };
    stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn is_resource_id(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Cheaply read the trailing `type_` property without parsing the whole file.
///
/// Joplin sync targets contain item types this client does not implement (most
/// notably `type_=13` note revisions). Those files may use a completely
/// different serialization (quoted JSON diffs, no title/body segments), so we
/// must recognise and skip them *before* running the full parser. Returns
/// `None` for an unrecognisable file, in which case the caller reports a real
/// parse error.
pub fn peek_type(content: &str) -> Option<i64> {
    for raw in content.lines().rev() {
        let line = raw.trim();
        if line.is_empty() {
            continue; // tolerate trailing newlines
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.trim() == "type_" {
                return value.trim().parse::<i64>().ok();
            }
        }
    }
    None
}

fn err(msg: impl Into<String>) -> Error {
    Error::msg(msg.into())
}

/// Joplin `serialize_format`: escape newlines in property values.
fn escape_value(s: &str) -> String {
    s.replace("\\n", "\\\\n")
        .replace("\\r", "\\\\r")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Joplin `unserialize_format`: reverse of `escape_value`.
fn unescape_value(s: &str) -> String {
    s.replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\\n", "\\n")
        .replace("\\\r", "\\r")
}

fn ms_to_iso(ms: &str) -> String {
    let ms = ms.parse::<i64>().unwrap_or(0);
    if ms == 0 {
        return String::new();
    }
    DateTime::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
}

fn iso_to_ms(s: &str) -> String {
    if s.is_empty() {
        return "0".to_string();
    }
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// Parse a remote `<id>.md` file into a database-ready item.
pub fn parse_item(content: &str) -> Result<SyncItem> {
    let mut lines: Vec<&str> = content.split('\n').collect();
    // Some clients leave a trailing newline. Joplin's own writer does not, but
    // tolerating it costs nothing and avoids "missing type_" on hand-edited
    // or third-party items.
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut body_start = 0usize;
    let mut found_separator = false;

    for i in (0..lines.len()).rev() {
        let line = lines[i].trim();
        if line.is_empty() {
            body_start = i;
            found_separator = true;
            break;
        }
        let Some(p) = line.find(':') else {
            return Err(err(format!("同步文件属性行格式错误: {line}")));
        };
        fields.insert(line[..p].trim().to_string(), line[p + 1..].trim().to_string());
    }

    let type_ = fields
        .get("type_")
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| err("同步文件缺少 type_ 属性"))?;
    if table_for_type(type_).is_none() {
        return Err(err(format!("不支持的同步条目类型 type_={type_}")));
    }

    let id = fields.get("id").cloned().unwrap_or_default();
    if !is_resource_id(&id) {
        return Err(err(format!("同步文件 id 非法: {id}")));
    }

    let mut title = String::new();
    let mut body = String::new();
    if found_separator && body_start > 0 {
        // Joplin: bodyLines = lines[0..i]; title = splice(0,2)[0]
        let head = &lines[..body_start];
        title = head.first().unwrap_or(&"").to_string();
        if type_ == TYPE_NOTE && head.len() > 2 {
            body = head[2..].join("\n");
        }
    }

    // Convert values back to database representation.
    for (key, value) in fields.iter_mut() {
        if key.ends_with('_') {
            continue; // private properties (type_) are left untouched
        }
        if TIME_KEYS_READ.contains(&key.as_str()) {
            *value = iso_to_ms(value);
        } else {
            *value = unescape_value(value);
        }
    }

    Ok(SyncItem {
        id,
        type_,
        title,
        body,
        fields,
    })
}

/// Serialize a database item into the exact text Joplin writes.
pub fn serialize_item(item: &SyncItem) -> String {
    let mut props: Vec<String> = Vec::with_capacity(item.fields.len() + 1);
    for (key, value) in &item.fields {
        let v = if TIME_KEYS_WRITE.contains(&key.as_str()) {
            ms_to_iso(value)
        } else {
            escape_value(value)
        };
        props.push(format!("{key}: {v}"));
    }
    props.push(format!("type_: {}", item.type_));

    let mut segments = vec![item.title.clone()];
    if !item.body.is_empty() {
        segments.push(item.body.clone());
    }
    segments.push(props.join("\n"));
    segments.join("\n\n")
}

/// `info.json` template matching Joplin sync version 3, E2EE disabled.
pub const INFO_JSON: &str = r#"{
	"version": 3,
	"e2ee": {
		"value": false,
		"updatedTime": 0
	},
	"activeMasterKeyId": {
		"value": "",
		"updatedTime": 0
	},
	"masterKeys": [],
	"ppk": {
		"value": null,
		"updatedTime": 0
	},
	"appMinVersion": "3.0.0"
}"#;

pub fn check_info_json(text: &str) -> Result<()> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| err(format!("info.json 不是合法 JSON: {e}")))?;
    let version = v.get("version").and_then(|x| x.as_i64()).unwrap_or(0);
    if version != 3 {
        return Err(err(format!(
            "远端同步格式版本为 {version}，本实现只支持 Joplin sync version 3"
        )));
    }
    let e2ee = v
        .get("e2ee")
        .and_then(|x| x.get("value"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    if e2ee {
        return Err(err("远端启用了端到端加密 (E2EE)，本实现不支持加密同步"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_note() {
        let mut fields = BTreeMap::new();
        fields.insert("id".into(), "a1b2c3d4e5f60718293a4b5c6d7e8f90".into());
        fields.insert("parent_id".into(), "0f1e2d3c4b5a69788796a5b4c3d2e1f0".into());
        fields.insert("created_time".into(), "1709251200000".into());
        fields.insert("updated_time".into(), "1709337600000".into());
        fields.insert("is_todo".into(), "0".into());
        fields.insert("markup_language".into(), "1".into());
        fields.insert("deleted_time".into(), "0".into());
        let item = SyncItem {
            id: "a1b2c3d4e5f60718293a4b5c6d7e8f90".into(),
            type_: TYPE_NOTE,
            title: "购物清单".into(),
            body: "- [ ] 牛奶\n\n第二段: 不是属性".into(),
            fields,
        };
        let text = serialize_item(&item);
        assert!(text.starts_with("购物清单\n\n- [ ] 牛奶"));
        assert!(text.contains("updated_time: 2024-03-02T00:00:00.000Z"));
        assert!(text.trim_end().ends_with("type_: 1"));

        let parsed = parse_item(&text).unwrap();
        assert_eq!(parsed.title, "购物清单");
        assert_eq!(parsed.body, "- [ ] 牛奶\n\n第二段: 不是属性");
        assert_eq!(parsed.updated_time(), 1709337600000);
        assert_eq!(parsed.type_, TYPE_NOTE);
    }

    #[test]
    fn parses_folder_without_body() {
        let mut fields = BTreeMap::new();
        fields.insert("id".into(), "0f1e2d3c4b5a69788796a5b4c3d2e1f0".into());
        fields.insert("updated_time".into(), "1709251200000".into());
        let text = serialize_item(&SyncItem {
            id: "0f1e2d3c4b5a69788796a5b4c3d2e1f0".into(),
            type_: TYPE_FOLDER,
            title: "工作".into(),
            body: String::new(),
            fields,
        });
        let parsed = parse_item(&text).unwrap();
        assert_eq!(parsed.title, "工作");
        assert!(parsed.body.is_empty());
        assert_eq!(parsed.updated_time(), 1709251200000);
    }

    #[test]
    fn parses_item_with_trailing_newline() {
        // Third-party clients (and the mobile app) often leave one or more
        // trailing blank lines. Previously the property scan stopped at the
        // first one and reported "missing type_".
        let mut fields = BTreeMap::new();
        fields.insert("id".into(), "1234567890abcdef1234567890abcdef".into());
        fields.insert("parent_id".into(), "0f1e2d3c4b5a69788796a5b4c3d2e1f0".into());
        fields.insert("updated_time".into(), "1709337600000".into());
        let text = format!(
            "{}\n\n",
            serialize_item(&SyncItem {
                id: "1234567890abcdef1234567890abcdef".into(),
                type_: TYPE_NOTE,
                title: "手机端笔记".into(),
                body: "正文".into(),
                fields,
            })
        );
        let parsed = parse_item(&text).unwrap();
        assert_eq!(parsed.title, "手机端笔记");
        assert_eq!(parsed.body, "正文");
        assert_eq!(parsed.type_, TYPE_NOTE);
        assert_eq!(parsed.updated_time(), 1709337600000);
    }

    #[test]
    fn peeks_item_type_before_parsing() {
        // A normal note: title/body segments, then the property block.
        let note = "标题\n\n正文\n\nid: a1b2c3d4e5f60718293a4b5c6d7e8f90\nupdated_time: 2024-03-02T00:00:00.000Z\ntype_: 1\n";
        assert_eq!(peek_type(note), Some(TYPE_NOTE));

        // A Joplin revision: no title/body at all, quoted JSON diff values.
        let revision = r#"id: 568fc3bd5be54a7097c7f85ace494a88
item_type: 1
title_diff: "[{\"diffs\":[[1,\"x\"]]}]"
updated_time: 2026-09-09T09:54:32.656Z
type_: 13
"#;
        assert_eq!(peek_type(revision), Some(13));
        assert!(table_for_type(13).is_none());
        // parse_item would reject it; peek_type lets the engine skip it first.
        assert!(parse_item(revision).is_err());

        assert_eq!(peek_type("not a sync file"), None);
    }

    #[test]
    fn rejects_encrypted_target() {
        assert!(check_info_json(r#"{"version":3,"e2ee":{"value":true}}"#).is_err());
        assert!(check_info_json(INFO_JSON).is_ok());
    }

    #[test]
    fn filename_rules() {
        assert!(is_item_filename("a1b2c3d4e5f60718293a4b5c6d7e8f90.md"));
        assert!(!is_item_filename("info.json"));
        assert!(!is_item_filename("abc.md"));
    }
}
