// Without `custom-protocol`, Tauri treats the binary as a dev build and points
// the WebView at `devUrl` (http://localhost:1420), which is not running when the
// exe is launched directly. Fail loudly at compile time instead of shipping a
// broken binary that only shows "localhost refused to connect".
#[cfg(all(not(debug_assertions), dev))]
compile_error!(
    "release builds must enable the `custom-protocol` feature so the frontend is embedded; \
     run `npm run build:app` (or `cargo build --release --features custom-protocol`)"
);

mod commands;
mod db;
mod error;
mod markdown;
mod model;
mod outline;
mod paths;
#[cfg(windows)]
mod print;
#[cfg(target_os = "macos")]
mod print_macos;
mod search;
mod state;
mod sync;

use std::{fs, fs::OpenOptions, io::Write};

use tauri::{Emitter, Manager};

use state::AppState;

/// Append a line to `startup.log` inside our own data directory and to stderr.
/// Useful to diagnose a blank window when the exe is launched by double click
/// (no console attached).
pub(crate) fn startup_log(msg: &str) {
    let line = format!("[{}] {}\n", chrono::Utc::now().to_rfc3339(), msg);
    eprint!("{line}");
    let path = paths::log_path();
    if let Some(dir) = path.parent() {
        if fs::create_dir_all(dir).is_ok() {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }
}

pub fn run() {
    startup_log(&format!(
        "app data dir: {} ({} override={})",
        paths::data_dir().display(),
        paths::DATA_DIR_ENV,
        std::env::var_os(paths::DATA_DIR_ENV).is_some()
    ));
    startup_log(&format!(
        "startup mode: {} (custom-protocol={})",
        if cfg!(dev) {
            "DEV / external Vite dev server"
        } else {
            "PRODUCTION / frontend embedded"
        },
        !cfg!(dev)
    ));
    let app_state = match AppState::init() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[joplin-rs] profile 初始化失败（可在界面中重新指定）: {e}");
            AppState::fallback(&e)
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                match window.url() {
                    Ok(url) => startup_log(&format!("initial webview url: {url}")),
                    Err(e) => startup_log(&format!("initial webview url unavailable: {e}")),
                }
                // Native menu bar (Windows/Linux) so the app has the same
                // discoverable entry points as Joplin's Tools > Synchronise.
                let menu_result = (|| -> tauri::Result<()> {
                    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
                    let m = app.app_handle();
                    let new_note = MenuItem::with_id(
                        m,
                        "file_new_note",
                        "新建笔记(&N)",
                        true,
                        None::<&str>,
                    )?;
                    let sep1 = PredefinedMenuItem::separator(m)?;
                    let quit = MenuItem::with_id(m, "file_quit", "退出(&Q)", true, None::<&str>)?;
                    let file_menu = Submenu::with_items(
                        m,
                        "文件(&F)",
                        true,
                        &[&new_note, &sep1, &quit],
                    )?;

                    // View > Mode: raw Markdown source and the WYSIWYG editor.
                    // The WYSIWYG entry is labelled "打字机" in the UI, but the
                    // action id stays `view_wysiwyg` so the behaviour is the
                    // same one the frontend already handles.
                    let view_source =
                        MenuItem::with_id(m, "view_source", "源码(&S)", true, None::<&str>)?;
                    let view_wysiwyg = MenuItem::with_id(
                        m,
                        "view_wysiwyg",
                        "打字机(&W)",
                        true,
                        None::<&str>,
                    )?;
                    let view_menu = Submenu::with_items(
                        m,
                        "视图(&V)",
                        true,
                        &[&view_source, &view_wysiwyg],
                    )?;

                    let sync_now = MenuItem::with_id(
                        m,
                        "tools_sync_now",
                        "立即同步(&S)",
                        true,
                        None::<&str>,
                    )?;
                    let sync_preview = MenuItem::with_id(
                        m,
                        "tools_sync_preview",
                        "同步预演（不写入）",
                        true,
                        None::<&str>,
                    )?;
                    let sync_settings = MenuItem::with_id(
                        m,
                        "tools_sync_settings",
                        "同步设置…",
                        true,
                        None::<&str>,
                    )?;
                    // No "import from the local Joplin profile" entry: WebDAV
                    // is the only data source, so a sync is the only way data
                    // enters this application.
                    let sep2 = PredefinedMenuItem::separator(m)?;
                    let tools_menu = Submenu::with_items(
                        m,
                        "工具(&T)",
                        true,
                        &[&sync_now, &sync_preview, &sep2, &sync_settings],
                    )?;

                    let about = MenuItem::with_id(m, "help_about", "关于 Joplin RS", true, None::<&str>)?;
                    let help_menu = Submenu::with_items(m, "帮助(&H)", true, &[&about])?;

                    let menu = Menu::with_items(
                        m,
                        &[&file_menu, &view_menu, &tools_menu, &help_menu],
                    )?;
                    window.set_menu(menu)?;
                    Ok(())
                })();
                if let Err(e) = menu_result {
                    startup_log(&format!("native menu unavailable: {e}"));
                } else {
                    startup_log("native menu installed (文件 / 视图 / 工具 / 帮助)");
                }
            } else {
                startup_log("main webview window not found in setup");
            }
            app.on_menu_event(|app, event| {
                let _ = app.emit("menu://action", event.id().as_ref());
            });
            Ok(())
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::get_config,
            commands::get_stats,
            commands::list_folders,
            commands::list_notes,
            commands::get_note,
            commands::render_note,
            commands::render_markdown,
            commands::save_note,
            commands::create_note,
            commands::move_note,
            commands::delete_note,
            commands::export_note_pdf,
            commands::create_folder,
            commands::rename_folder,
            commands::list_tags,
            commands::search_notes,
            commands::read_resource,
            commands::app_quit,
            commands::app_data_dir,
            commands::open_devtools,
            sync::sync_get_config,
            sync::sync_status,
            sync::sync_set_config,
            sync::sync_test,
            sync::sync_preview,
            sync::sync_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running joplin-rs");
}
