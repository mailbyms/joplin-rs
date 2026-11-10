//! Turn a note's headings into the PDF's sidebar bookmarks.
//!
//! WebView2's `PrintToPdf` never writes an outline, but it does something just
//! as useful: every in-document link (`<a href="#id">`) becomes a named
//! destination under the catalog's `/Dests`, and that entry carries the exact
//! page object plus the `/XYZ` coordinates of the link. So the print sheet tags
//! each heading with a self-referencing anchor, the render hands us those
//! destinations, and this module folds them into a `/Outlines` tree.
//!
//! Everything is best-effort: a PDF without bookmarks is still a valid export,
//! so callers should log and carry on when this fails.

use std::collections::HashMap;

use lopdf::{Document, Dictionary, Object, ObjectId, StringFormat};
use serde::Deserialize;

use crate::error::{Error, Result};

/// One heading lifted from the note, in document order.
#[derive(Debug, Clone, Deserialize)]
pub struct Heading {
    /// 1..=6 — the markdown level, used to nest the bookmarks.
    pub level: u8,
    pub title: String,
    /// The `id` the heading was given in the print sheet. Chromium turns the
    /// matching `<a href="#…">` into a named destination with the same name.
    pub anchor: String,
}

/// Longest bookmark caption we keep; the sidebar clips the rest anyway and a
/// runaway heading would bloat the file.
const MAX_TITLE_CHARS: usize = 120;

/// A heading that survived the jump from HTML to PDF.
struct Node {
    level: u8,
    title: String,
    /// The `/Dest` array copied straight out of the PDF's `/Dests`.
    dest: Object,
    children: Vec<Node>,
}

/// Add bookmarks for `headings` to the PDF at `path`, in place.
///
/// Returns how many bookmarks were written — zero when the note has no
/// headings, or when the renderer produced no destination for any of them.
pub fn apply(path: &str, headings: &[Heading]) -> Result<usize> {
    if headings.is_empty() {
        return Ok(0);
    }

    let mut doc = Document::load(path).map_err(|e| Error::msg(format!("读取 PDF 失败: {e}")))?;
    let dests = collect_dests(&doc);
    if dests.is_empty() {
        return Ok(0);
    }

    let tree = build_tree(headings, &dests);
    if tree.is_empty() {
        return Ok(0);
    }

    let total = count(&tree);
    let root = next_id(&mut doc);
    let (first, last, _) = emit(&mut doc, &tree, root);

    let mut outlines = Dictionary::new();
    outlines.set("Type", Object::Name(b"Outlines".to_vec()));
    outlines.set("First", Object::Reference(first));
    outlines.set("Last", Object::Reference(last));
    // Positive /Count means "expanded", which is what a reader expects when it
    // opens a freshly exported note.
    outlines.set("Count", total);
    doc.objects.insert(root, Object::Dictionary(outlines));

    let catalog_id = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|e| Error::msg(format!("PDF 缺少文档目录: {e}")))?;
    let catalog = doc
        .get_dictionary_mut(catalog_id)
        .map_err(|e| Error::msg(format!("PDF 文档目录不可读: {e}")))?;
    catalog.set("Outlines", Object::Reference(root));
    // Ask viewers to open the bookmark pane — the whole point of writing it.
    catalog.set("PageMode", Object::Name(b"UseOutlines".to_vec()));

    doc.save(path)
        .map_err(|e| Error::msg(format!("写入 PDF 书签失败: {e}")))?;

    Ok(total as usize)
}

/// Read the catalog's `/Dests` dictionary into `name -> dest array`.
///
/// Chromium emits the pre-1.2 form (a plain dictionary on the catalog) rather
/// than a name tree, so this is a flat loop; anything that is not an array is
/// skipped instead of failing the export.
fn collect_dests(doc: &Document) -> HashMap<Vec<u8>, Object> {
    let mut out = HashMap::new();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    let Some(value) = catalog.get(b"Dests").ok() else {
        return out;
    };
    let Ok((_, resolved)) = doc.dereference(value) else {
        return out;
    };
    let Ok(dict) = resolved.as_dict() else {
        return out;
    };
    for (name, dest) in dict.iter() {
        if matches!(dest, Object::Array(_)) {
            out.insert(name.clone(), dest.clone());
        }
    }
    out
}

/// Nest the flat heading list the same way the document nests them: a heading
/// belongs to the closest preceding heading of a higher level. A document that
/// starts at `##` simply gets top-level bookmarks.
fn build_tree(headings: &[Heading], dests: &HashMap<Vec<u8>, Object>) -> Vec<Node> {
    let mut roots: Vec<Node> = Vec::new();
    for heading in headings {
        let Some(dest) = dests.get(heading.anchor.as_bytes()) else {
            // The renderer skipped this anchor (a heading already containing a
            // link, for instance); leaving a gap in the tree is fine.
            continue;
        };
        let title: String = heading.title.trim().chars().take(MAX_TITLE_CHARS).collect();
        if title.is_empty() {
            continue;
        }
        let node = Node {
            level: heading.level.clamp(1, 6),
            title,
            dest: dest.clone(),
            children: Vec::new(),
        };
        insert(&mut roots, node);
    }
    roots
}

fn insert(nodes: &mut Vec<Node>, node: Node) {
    match nodes.last_mut() {
        Some(last) if last.level < node.level => insert(&mut last.children, node),
        _ => nodes.push(node),
    }
}

fn count(nodes: &[Node]) -> i64 {
    nodes
        .iter()
        .map(|n| 1 + count(&n.children))
        .sum()
}

/// Write `nodes` (and their subtrees) as outline items under `parent`, and
/// return the first/last item ids plus how many items the subtree holds.
fn emit(doc: &mut Document, nodes: &[Node], parent: ObjectId) -> (ObjectId, ObjectId, i64) {
    // Allocate every sibling id up front so Prev/Next can point at each other
    // without a second pass.
    let ids: Vec<ObjectId> = nodes.iter().map(|_| next_id(doc)).collect();
    let mut total = 0;

    for (i, node) in nodes.iter().enumerate() {
        let id = ids[i];
        let mut item = Dictionary::new();
        item.set("Title", pdf_text(&node.title));
        item.set("Parent", Object::Reference(parent));
        item.set("Dest", node.dest.clone());
        if i > 0 {
            item.set("Prev", Object::Reference(ids[i - 1]));
        }
        if let Some(next) = ids.get(i + 1) {
            item.set("Next", Object::Reference(*next));
        }

        if !node.children.is_empty() {
            let (first, last, inner) = emit(doc, &node.children, id);
            item.set("First", Object::Reference(first));
            item.set("Last", Object::Reference(last));
            item.set("Count", inner);
            total += inner;
        }

        doc.objects.insert(id, Object::Dictionary(item));
        total += 1;
    }

    (ids[0], ids[ids.len() - 1], total)
}

fn next_id(doc: &mut Document) -> ObjectId {
    doc.max_id += 1;
    (doc.max_id, 0)
}

/// PDF text strings are either PDFDocEncoding or UTF-16BE with a BOM. Chinese
/// headings rule out the former, so everything goes out as hex UTF-16BE — which
/// every reader understands, at the cost of two bytes per ASCII character.
fn pdf_text(text: &str) -> Object {
    let mut bytes = vec![0xFE, 0xFF];
    bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    Object::String(bytes, StringFormat::Hexadecimal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(level: u8, title: &str, anchor: &str) -> Heading {
        Heading {
            level,
            title: title.to_string(),
            anchor: anchor.to_string(),
        }
    }

    fn dests(names: &[&str]) -> HashMap<Vec<u8>, Object> {
        names
            .iter()
            .map(|n| {
                (
                    n.as_bytes().to_vec(),
                    Object::Array(vec![
                        Object::Reference((7, 0)),
                        Object::Name(b"XYZ".to_vec()),
                        Object::Integer(6),
                        Object::Integer(700),
                        Object::Integer(0),
                    ]),
                )
            })
            .collect()
    }

    #[test]
    fn headings_nest_by_level() {
        let tree = build_tree(
            &[
                heading(1, "A", "a"),
                heading(2, "A1", "a1"),
                heading(3, "A1x", "a1x"),
                heading(2, "A2", "a2"),
                heading(1, "B", "b"),
            ],
            &dests(&["a", "a1", "a1x", "a2", "b"]),
        );

        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].title, "A");
        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].children[0].title, "A1x");
        assert_eq!(tree[1].title, "B");
        assert_eq!(count(&tree), 5);
    }

    #[test]
    fn a_document_that_starts_deep_still_gets_top_level_items() {
        let tree = build_tree(
            &[heading(3, "Deep", "d"), heading(4, "Deeper", "d2")],
            &dests(&["d", "d2"]),
        );

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 1);
    }

    #[test]
    fn anchors_the_renderer_dropped_are_skipped() {
        let tree = build_tree(
            &[heading(1, "Kept", "k"), heading(1, "Lost", "missing")],
            &dests(&["k"]),
        );

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].title, "Kept");
    }

    #[test]
    fn titles_are_utf16be_with_a_bom() {
        let Object::String(bytes, StringFormat::Hexadecimal) = pdf_text("笔记") else {
            panic!("expected a hex string");
        };
        assert_eq!(bytes[0..2], [0xFE, 0xFF]);
        assert_eq!(bytes.len(), 2 + 2 * 2);
    }
}
