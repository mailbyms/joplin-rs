use crate::{
    db::{base64_decode, base64_encode, make_excerpt, now_ms, ProfileConfig},
    error::{Error, Result},
    markdown,
    model::*,
    state::AppState,
};
use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::State;

fn render_with(state: &AppState, body: &str) -> RenderedNote {
    let res = state.resource_ext.read();
    let notes = state.note_titles.read();
    markdown::render(body, &res, &notes)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub ok: bool,
    pub error: Option<String>,
    /// Always the application's own data directory: the library is created
    /// there and only ever filled from WebDAV.
    pub data_dir: String,
    /// Shown in the status bar, e.g. "0.1.0".
    pub version: String,
    pub stats: Option<Stats>,
}

static FILTER_CHECK: std::sync::Once = std::sync::Once::new();

/// Prove at runtime that the SQLite folder filter returns a real subset.
/// Logged once per process so a broken list is immediately diagnosable.
fn folder_filter_self_check(state: &AppState) {
    FILTER_CHECK.call_once(|| {
        let folders = match state.with_db(|db| db.list_folders()) {
            Ok(f) => f,
            Err(e) => {
                crate::startup_log(&format!("filter self-check skipped: {e}"));
                return;
            }
        };
        let total = state
            .with_db(|db| db.stats(&state.cfg.read()))
            .map(|s| s.notes)
            .unwrap_or(0);
        let Some(target) = folders
            .iter()
            .find(|f| f.note_count > 0 && f.note_count < total)
        else {
            crate::startup_log("filter self-check skipped: no folder with a partial note count");
            return;
        };
        let got = state
            .with_db(|db| db.list_notes(Some(&target.id), None, 10_000, 0))
            .map(|v| v.len() as i64)
            .unwrap_or(-1);
        crate::startup_log(&format!(
            "filter self-check: folder '{}' -> {} notes (sidebar {}, all {}): {}",
            target.title,
            got,
            target.note_count,
            total,
            if got == target.note_count {
                "OK"
            } else {
                "MISMATCH"
            }
        ));
    });
}

#[tauri::command]
pub fn bootstrap(state: State<'_, AppState>) -> Bootstrap {
    folder_filter_self_check(&state);
    let data_dir = crate::paths::data_dir().display().to_string();
    let error = state.init_error.read().clone();
    let stats = state
        .with_db(|db| db.stats(&state.cfg.read()))
        .ok();
    let result = Bootstrap {
        ok: error.is_none() && stats.is_some(),
        error,
        data_dir,
        version: env!("CARGO_PKG_VERSION").to_string(),
        stats,
    };
    crate::startup_log(&format!(
        "frontend IPC bootstrap: ok={} notes={} data_dir={}",
        result.ok,
        result.stats.as_ref().map_or(-1, |s| s.notes),
        result.data_dir
    ));
    result
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> ProfileConfig {
    state.cfg.read().clone()
}

#[tauri::command]
pub fn get_stats(state: State<'_, AppState>) -> Result<Stats> {
    state.with_db(|db| db.stats(&state.cfg.read()))
}

#[tauri::command]
pub fn list_folders(state: State<'_, AppState>) -> Result<Vec<Folder>> {
    state.with_db(|db| db.list_folders())
}

#[tauri::command]
pub fn list_notes(
    state: State<'_, AppState>,
    folder_id: Option<String>,
    tag_id: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<NoteSummary>> {
    let rows = state.with_db(|db| {
        db.list_notes(
            folder_id.as_deref(),
            tag_id.as_deref(),
            // No caller-supplied limit means "the whole list": the note list
            // header prints the row count, so a silent 300/500 cap made it lie.
            limit.unwrap_or(0),
            offset.unwrap_or(0),
        )
    })?;
    crate::startup_log(&format!(
        "list_notes IPC: folder={:?} tag={:?} rows={}",
        folder_id.as_deref(),
        tag_id.as_deref(),
        rows.len()
    ));
    Ok(rows)
}

#[tauri::command]
pub fn get_note(state: State<'_, AppState>, id: String) -> Result<Note> {
    state.with_db(|db| db.get_note(&id))
}

#[tauri::command]
pub fn render_note(state: State<'_, AppState>, id: String) -> Result<RenderedNote> {
    let note = state.with_db(|db| db.get_note(&id))?;
    Ok(render_with(&state, &note.body))
}

#[tauri::command]
pub fn render_markdown(state: State<'_, AppState>, body: String) -> Result<RenderedNote> {
    Ok(render_with(&state, &body))
}

#[tauri::command]
pub fn save_note(
    state: State<'_, AppState>,
    id: String,
    title: String,
    body: String,
) -> Result<NoteSummary> {
    let parent_id = state.with_db(|db| {
        db.save_note(&id, &title, &body)?;
        db.get_note(&id).map(|n| n.parent_id)
    })?;
    let now = now_ms();
    state.index.write().upsert(&id, &parent_id, &title, &body, now);
    state.note_titles.write().insert(id.clone(), title.clone());
    Ok(NoteSummary {
        id,
        title,
        parent_id,
        created_time: 0,
        updated_time: now,
        is_todo: 0,
        todo_completed: 0,
        excerpt: make_excerpt(&body, 110),
    })
}

#[tauri::command]
pub fn move_note(
    state: State<'_, AppState>,
    id: String,
    folder_id: String,
) -> Result<NoteSummary> {
    let note = state.with_db(|db| {
        db.move_note(&id, &folder_id)?;
        db.get_note(&id)
    })?;
    state.index.write().upsert(
        &note.id,
        &note.parent_id,
        &note.title,
        &note.body,
        note.updated_time,
    );
    Ok(NoteSummary {
        id: note.id,
        title: note.title,
        parent_id: note.parent_id,
        created_time: note.created_time,
        updated_time: note.updated_time,
        is_todo: note.is_todo,
        todo_completed: note.todo_completed,
        excerpt: make_excerpt(&note.body, 110),
    })
}

#[tauri::command]
pub fn create_note(
    state: State<'_, AppState>,
    folder_id: String,
    title: String,
) -> Result<Note> {
    let note = state.with_db(|db| db.create_note(&folder_id, &title))?;
    state.index.write().upsert(
        &note.id,
        &note.parent_id,
        &note.title,
        &note.body,
        note.updated_time,
    );
    state
        .note_titles
        .write()
        .insert(note.id.clone(), note.title.clone());
    Ok(note)
}

#[tauri::command]
pub fn delete_note(state: State<'_, AppState>, id: String) -> Result<()> {
    state.with_db(|db| db.delete_note(&id))?;
    state.index.write().remove(&id);
    state.note_titles.write().remove(&id);
    Ok(())
}

/// Full scrollable size of the print sheet, measured by the frontend once the
/// note is laid out. WKWebView's PDF capture defaults to the *visible viewport*
/// (WKPDFConfiguration's null rect), which silently truncates a long note to
/// one screen; `print_macos.rs` turns this into an explicit capture rect.
/// WebView2 paginates the print media itself and ignores it.
#[derive(serde::Deserialize)]
pub struct PdfBounds {
    pub width: f64,
    pub height: f64,
}

/// Write whatever the window currently shows to a PDF file.
///
/// The frontend first swaps the whole UI for a print-only sheet holding the
/// note, so no note id is needed here: the DOM *is* the document. Rendering and
/// file output both happen inside the platform webview (WebView2 on Windows,
/// WKWebView on macOS — see `print.rs` / `print_macos.rs`), which keeps the
/// text selectable instead of rasterising a screenshot.
///
/// `headings` carries the note's headings in document order; once the file
/// exists it is reopened and given a bookmark tree (see `outline.rs`).
#[tauri::command]
pub async fn export_note_pdf(
    window: tauri::WebviewWindow,
    path: String,
    headings: Vec<crate::outline::Heading>,
    bounds: Option<PdfBounds>,
) -> Result<()> {
    let size = bounds
        .filter(|b| {
            b.width.is_finite()
                && b.height.is_finite()
                && b.width > 0.0
                && b.height > 0.0
        })
        .map(|b| (b.width, b.height));
    render_pdf(window, path.clone(), size).await?;

    // Bookmarks are a bonus: a note without headings, or a renderer that
    // dropped an anchor, must not fail an export that already succeeded.
    match crate::outline::apply(&path, &headings) {
        Ok(n) if n > 0 => crate::startup_log(&format!("pdf outline: {n} bookmark(s)")),
        Ok(_) => {}
        Err(e) => crate::startup_log(&format!("pdf outline skipped: {e}")),
    }
    Ok(())
}

/// Platform-specific render + file write, kept separate so the bookmark
/// post-processing above stays shared between Windows and macOS.
///
/// Both platform exporters block until the webview reports completion, so the
/// work goes to a blocking thread rather than the async runtime's workers.
#[cfg(windows)]
async fn render_pdf(
    window: tauri::WebviewWindow,
    target: String,
    _size: Option<(f64, f64)>,
) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || crate::print::export(&window, &target))
        .await
        .map_err(|e| Error::msg(format!("导出任务异常: {e}")))?
}

#[cfg(target_os = "macos")]
async fn render_pdf(
    window: tauri::WebviewWindow,
    target: String,
    size: Option<(f64, f64)>,
) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::print_macos::export(&window, &target, size)
    })
    .await
    .map_err(|e| Error::msg(format!("导出任务异常: {e}")))?
}

#[cfg(not(any(windows, target_os = "macos")))]
async fn render_pdf(
    _window: tauri::WebviewWindow,
    _target: String,
    _size: Option<(f64, f64)>,
) -> Result<()> {
    Err(Error::msg("当前平台暂不支持导出 PDF"))
}

#[tauri::command]
pub fn create_folder(
    state: State<'_, AppState>,
    title: String,
    parent_id: String,
) -> Result<Folder> {
    state.with_db(|db| db.create_folder(&title, &parent_id))
}

#[tauri::command]
pub fn rename_folder(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<()> {
    state.with_db(|db| db.rename_folder(&id, &title))
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<Tag>> {
    state.with_db(|db| db.list_tags())
}

#[tauri::command]
pub fn search_notes(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<SearchHit>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    // No caller limit = every hit. The list header prints the hit count, so a
    // hidden 60/120 cap would make the number disagree with the rows shown.
    Ok(state
        .index
        .read()
        .search(&query, limit.unwrap_or(usize::MAX)))
}

#[tauri::command]
pub fn app_quit(app: tauri::AppHandle) {
    app.exit(0);
}

/// Attachments are read **only** from our own data directory, where the sync
/// engine writes downloaded blobs and `create_resource` writes locally
/// inserted images. Returns base64 so a single IPC round-trip delivers the
/// bytes to the WebView.
#[tauri::command]
pub fn read_resource(state: State<'_, AppState>, id: String) -> Result<ResourceData> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::msg("非法的资源 ID"));
    }
    let mime = state.with_db(|db| db.resource_mime(&id))?;
    let ext = state.resource_ext.read().get(&id).cloned().unwrap_or_default();
    let names: Vec<String> = if ext.is_empty() {
        vec![id.clone(), format!("{id}.")]
    } else {
        vec![format!("{id}.{ext}"), id.clone()]
    };
    let dir = crate::paths::resources_dir();
    let mut found: Option<PathBuf> = None;
    for name in &names {
        let candidate = dir.join(name);
        if candidate.is_file() {
            found = Some(candidate);
            break;
        }
    }
    // Some Joplin blobs carry an extension the database doesn't record.
    if found.is_none() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&id) {
                    found = Some(entry.path());
                    break;
                }
            }
        }
    }
    let path = found.ok_or_else(|| {
        Error::msg(format!(
            "附件 {id} 还没有下载到本地，先执行一次 WebDAV 同步"
        ))
    })?;
    let bytes = fs::read(&path)?;
    Ok(ResourceData {
        base64: base64_encode(&bytes),
        mime,
    })
}

/// Joplin's `file_extension` is derived from the MIME type; a well-formed
/// filename extension wins when a real file was picked from disk.
fn resource_extension(mime: &str, filename: &str) -> String {
    let from_name = PathBuf::from(filename)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !from_name.is_empty()
        && from_name.len() <= 8
        && from_name.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return from_name;
    }
    let from_mime = match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" | "image/x-ms-bmp" => "bmp",
        "image/avif" => "avif",
        "image/heic" => "heic",
        "image/tiff" => "tiff",
        "application/pdf" => "pdf",
        _ => "bin",
    };
    from_mime.to_string()
}

/// Clipboard images sometimes arrive with an empty MIME type. Recognise the
/// four common magic headers so Joplin still treats the resource as an image.
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Register a pasted / dropped / picked file as a Joplin resource.
///
/// This is the missing write side of the attachment pipeline: Milkdown's
/// default uploader inlines a `data:` URL into the note body, which is never a
/// resource and therefore never reaches WebDAV. Here we generate a 32-hex id,
/// write the blob to our own `resources/` directory and insert the metadata row,
/// exactly like Joplin's `shim.createResourceFromPath()`. The editor then
/// stores `:/<id>` in the body and the existing sync engine does the rest.
#[tauri::command]
pub fn create_resource(
    state: State<'_, AppState>,
    base64: String,
    mime: String,
    filename: String,
) -> Result<CreatedResource> {
    let bytes = base64_decode(&base64)?;
    if bytes.is_empty() {
        return Err(Error::msg("附件内容为空"));
    }
    let mut mime = mime.trim().to_ascii_lowercase();
    if mime.is_empty() || mime == "application/octet-stream" {
        mime = sniff_mime(&bytes)
            .unwrap_or("application/octet-stream")
            .to_string();
    } else if mime == "image/jpg" {
        mime = "image/jpeg".to_string();
    }
    let ext = resource_extension(&mime, &filename);
    let id = uuid::Uuid::new_v4().simple().to_string();

    let dir = crate::paths::resources_dir();
    fs::create_dir_all(&dir)?;
    let path = if ext.is_empty() {
        dir.join(&id)
    } else {
        dir.join(format!("{id}.{ext}"))
    };
    fs::write(&path, &bytes)?;

    let title = if filename.trim().is_empty() {
        format!("image.{ext}")
    } else {
        filename.clone()
    };
    let size = bytes.len() as i64;
    let inserted = state.with_db(|db| {
        db.create_resource(&id, &title, &mime, &filename, &ext, size, now_ms())
    });
    if let Err(e) = inserted {
        // A half-created attachment would be silently skipped by the sync
        // engine (metadata without a blob), so roll the file back as well.
        let _ = fs::remove_file(&path);
        return Err(e);
    }
    // Keep the in-memory extension map in sync so `render_with` resolves the
    // new `:/id` immediately, without waiting for a full state reload.
    state.resource_ext.write().insert(id.clone(), ext);
    Ok(CreatedResource { id, mime, size })
}

/// Absolute path of the application's own data directory, shown in the UI.
#[tauri::command]
pub fn app_data_dir() -> String {
    crate::paths::data_dir().to_string_lossy().into_owned()
}

/// Opens WebView2's inspector — the desktop equivalent of Electron's "Toggle
/// Developer Tools". The window is a separate top-level one owned by WebView2;
/// Tauri cannot close it on Windows, so the user closes it like any other
/// window. Requires the `devtools` feature on release builds.
#[tauri::command]
pub fn open_devtools(window: tauri::WebviewWindow) {
    window.open_devtools();
}
