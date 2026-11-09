use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub title: String,
    pub parent_id: String,
    pub note_count: i64,
    pub updated_time: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub parent_id: String,
    pub created_time: i64,
    pub updated_time: i64,
    pub is_todo: i64,
    pub todo_completed: i64,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub parent_id: String,
    pub created_time: i64,
    pub updated_time: i64,
    pub is_todo: i64,
    pub todo_completed: i64,
    pub markup_language: i64,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: String,
    pub title: String,
    pub note_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedNote {
    pub html: String,
    pub word_count: usize,
    pub char_count: usize,
    pub outline: Vec<OutlineItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineItem {
    pub level: u8,
    pub text: String,
    pub anchor: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub parent_id: String,
    pub excerpt: String,
    pub score: i64,
    pub updated_time: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub notes: i64,
    pub folders: i64,
    pub resources: i64,
    pub tags: i64,
    pub data_dir: String,
    pub work_db: String,
    pub work_db_bytes: u64,
    pub synced_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceData {
    pub base64: String,
    pub mime: String,
}
