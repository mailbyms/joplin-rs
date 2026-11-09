use crate::model::{OutlineItem, RenderedNote};
use pulldown_cmark::{html, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::collections::HashMap;

fn options() -> Options {
    let mut o = Options::empty();
    o.insert(Options::ENABLE_TABLES);
    o.insert(Options::ENABLE_FOOTNOTES);
    o.insert(Options::ENABLE_STRIKETHROUGH);
    o.insert(Options::ENABLE_TASKLISTS);
    o.insert(Options::ENABLE_SMART_PUNCTUATION);
    o.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    o
}

fn is_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Joplin uses `:/<32-hex-id>` for both internal note links and attachments.
/// We turn them into inert custom schemes that the frontend hydrates via IPC,
/// which keeps all file IO on the Rust side.
fn rewrite_url(
    url: &str,
    resources: &HashMap<String, String>,
    notes: &HashMap<String, String>,
) -> Option<String> {
    let raw = url
        .strip_prefix(":/")
        .or_else(|| url.strip_prefix("joplin://"))?;
    let id = raw.split(['#', '?', '/']).next().unwrap_or("");
    if !is_id(id) {
        return None;
    }
    if resources.contains_key(id) {
        Some(format!("joplin-resource://{id}"))
    } else if notes.contains_key(id) {
        Some(format!("joplin-note://{id}"))
    } else {
        None
    }
}

/// Strip a raw HTML event by turning it into plain text: `push_html` will
/// escape it exactly once for us.
fn as_text(s: CowStr<'_>) -> CowStr<'_> {
    CowStr::Boxed(s.to_string().into_boxed_str())
}

fn heading_level(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Count words the way CJK writers expect: one CJK character = one word,
/// one run of ASCII alphanumerics = one word.
pub fn count_words(s: &str) -> usize {
    let mut count = 0usize;
    let mut in_ascii_word = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if !in_ascii_word {
                count += 1;
                in_ascii_word = true;
            }
        } else {
            in_ascii_word = false;
            if !c.is_whitespace() && !c.is_ascii_punctuation() && c != '#' && c != '*' && c != '`'
            {
                count += 1;
            }
        }
    }
    count
}

pub fn render(
    body: &str,
    resources: &HashMap<String, String>,
    notes: &HashMap<String, String>,
) -> RenderedNote {
    let mut events: Vec<Event<'_>> = Vec::with_capacity(body.len() / 8);
    let mut outline = Vec::new();
    let mut heading: Option<(u8, String, String)> = None;
    let mut heading_idx = 0usize;

    for ev in Parser::new_ext(body, options()) {
        match ev {
            Event::Start(Tag::Heading {
                level,
                id: _,
                classes,
                attrs,
            }) => {
                let anchor = format!("h-{heading_idx}");
                heading_idx += 1;
                heading = Some((heading_level(level), String::new(), anchor.clone()));
                events.push(Event::Start(Tag::Heading {
                    level,
                    id: Some(CowStr::Boxed(anchor.into_boxed_str())),
                    classes,
                    attrs,
                }));
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let dest = rewrite_url(&dest_url, resources, notes)
                    .map(|s| CowStr::Boxed(s.into_boxed_str()))
                    .unwrap_or(dest_url);
                events.push(Event::Start(Tag::Image {
                    link_type,
                    dest_url: dest,
                    title,
                    id,
                }));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let dest = rewrite_url(&dest_url, resources, notes)
                    .map(|s| CowStr::Boxed(s.into_boxed_str()))
                    .unwrap_or(dest_url);
                events.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url: dest,
                    title,
                    id,
                }));
            }
            // Never emit raw HTML from note bodies into a WebView.
            Event::Html(s) | Event::InlineHtml(s) => events.push(Event::Text(as_text(s))),
            Event::Text(t) => {
                if let Some(h) = heading.as_mut() {
                    h.1.push_str(&t);
                }
                events.push(Event::Text(t));
            }
            Event::End(e @ TagEnd::Heading(_)) => {
                if let Some((level, text, anchor)) = heading.take() {
                    outline.push(OutlineItem {
                        level,
                        text: text.trim().to_string(),
                        anchor,
                    });
                }
                events.push(Event::End(e));
            }
            other => events.push(other),
        }
    }

    let mut html_output = String::with_capacity(body.len() * 2);
    html::push_html(&mut html_output, events.into_iter());
    html_output = html_output.replace("<img src=\"", "<img loading=\"lazy\" src=\"");

    RenderedNote {
        html: html_output,
        word_count: count_words(body),
        char_count: body.chars().count(),
        outline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maps() -> (HashMap<String, String>, HashMap<String, String>) {
        let mut resources = HashMap::new();
        resources.insert("a".repeat(32), "png".to_string());
        let mut notes = HashMap::new();
        notes.insert("b".repeat(32), "另一篇笔记".to_string());
        (resources, notes)
    }

    #[test]
    fn renders_gfm_and_builds_outline() {
        let (resources, notes) = maps();
        let out = render(
            "# 标题\n\n- [ ] 待办\n\n| A | B |\n|---|---|\n| 1 | 2 |",
            &resources,
            &notes,
        );
        assert!(out.html.contains("<h1"), "heading missing: {}", out.html);
        assert!(out.html.contains("checkbox"), "task list missing");
        assert!(out.html.contains("<table>"), "table missing");
        assert_eq!(out.outline.len(), 1);
        assert_eq!(out.outline[0].anchor, "h-0");
        assert!(out.html.contains("h-0"), "heading anchor not emitted");
    }

    #[test]
    fn rewrites_joplin_resource_and_note_links() {
        let (resources, notes) = maps();
        let body = format!(
            "![图片](:/{})\n\n[跳转](:/{})",
            "a".repeat(32),
            "b".repeat(32)
        );
        let out = render(&body, &resources, &notes);
        assert!(out.html.contains(&format!("joplin-resource://{}", "a".repeat(32))));
        assert!(out.html.contains(&format!("joplin-note://{}", "b".repeat(32))));
        assert!(out.html.contains("loading=\"lazy\""));
    }

    #[test]
    fn never_emits_raw_html_from_notes() {
        let (resources, notes) = maps();
        let out = render("<script>alert(1)</script>", &resources, &notes);
        assert!(!out.html.contains("<script>"));
        assert!(out.html.contains("&lt;script&gt;"));
    }

    #[test]
    fn counts_cjk_and_latin_words() {
        assert_eq!(count_words("hello world 中文测试"), 6);
        assert_eq!(count_words(""), 0);
    }
}
