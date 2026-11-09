use crate::db::make_excerpt;
use crate::model::SearchHit;
use std::collections::HashMap;

struct Entry {
    id: String,
    parent_id: String,
    title: String,
    title_lower: String,
    body_lower: String,
    body: String,
    updated_time: i64,
}

/// A tiny in-memory full-text index. For a personal note corpus this is both
/// faster and far lighter than spinning up a JS search engine in the WebView.
/// Substring matching also works correctly for Chinese, which FTS5's default
/// unicode61 tokenizer does not.
pub struct SearchIndex {
    entries: HashMap<String, Entry>,
}

impl SearchIndex {
    pub fn build(rows: Vec<(String, String, String, String, i64)>) -> Self {
        let mut entries = HashMap::with_capacity(rows.len());
        for (id, parent_id, title, body, updated_time) in rows {
            entries.insert(
                id.clone(),
                Entry {
                    id,
                    parent_id,
                    title_lower: title.to_lowercase(),
                    title: title.clone(),
                    body_lower: body.to_lowercase(),
                    body: body.clone(),
                    updated_time,
                },
            );
        }
        Self { entries }
    }

    pub fn upsert(
        &mut self,
        id: &str,
        parent_id: &str,
        title: &str,
        body: &str,
        updated_time: i64,
    ) {
        self.entries.insert(
            id.to_string(),
            Entry {
                id: id.to_string(),
                parent_id: parent_id.to_string(),
                title: title.to_string(),
                title_lower: title.to_lowercase(),
                body_lower: body.to_lowercase(),
                body: body.to_string(),
                updated_time,
            },
        );
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.remove(id);
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|s| s.to_lowercase())
            .filter(|s| !s.is_empty())
            .take(8)
            .collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<SearchHit> = self
            .entries
            .values()
            .filter_map(|e| {
                let mut score = 0i64;
                for term in &terms {
                    let in_title = e.title_lower.contains(term);
                    let in_body = e.body_lower.contains(term);
                    if !in_title && !in_body {
                        return None;
                    }
                    if in_title {
                        score += 12;
                        if e.title_lower.starts_with(term) {
                            score += 8;
                        }
                    }
                    if in_body {
                        score += 2;
                    }
                }
                if e.title_lower == terms[0] {
                    score += 20;
                }
                Some(SearchHit {
                    id: e.id.clone(),
                    title: e.title.clone(),
                    parent_id: e.parent_id.clone(),
                    excerpt: make_excerpt(&e.body, 130),
                    score,
                    updated_time: e.updated_time,
                })
            })
            .collect();
        hits.sort_by(|a, b| b.score.cmp(&a.score).then(b.updated_time.cmp(&a.updated_time)));
        hits.truncate(limit);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> SearchIndex {
        SearchIndex::build(vec![
            (
                "1".into(),
                "f1".into(),
                "QT 环境配置".into(),
                "在 Windows 上编译 Qt 5.14".into(),
                100,
            ),
            (
                "2".into(),
                "f1".into(),
                "其它笔记".into(),
                "完全无关的内容".into(),
                200,
            ),
        ])
    }

    #[test]
    fn finds_chinese_in_title_and_body() {
        let idx = index();
        let by_title = idx.search("环境", 10);
        assert_eq!(by_title.len(), 1);
        assert_eq!(by_title[0].id, "1");
        let by_body = idx.search("编译", 10);
        assert_eq!(by_body[0].id, "1");
    }

    #[test]
    fn multiple_terms_are_anded() {
        let idx = index();
        assert_eq!(idx.search("qt 编译", 10).len(), 1);
        assert_eq!(idx.search("qt 无关", 10).len(), 0);
        assert!(idx.search("", 10).is_empty());
    }

    #[test]
    fn upsert_and_remove_are_reflected() {
        let mut idx = index();
        idx.upsert("3", "f1", "新增", "独一无二的内容", 300);
        assert_eq!(idx.search("独一无二", 10)[0].id, "3");
        idx.remove("3");
        assert!(idx.search("独一无二", 10).is_empty());
    }
}
