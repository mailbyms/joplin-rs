//! Export the current webview to PDF through WebView2.
//!
//! `window.print()` opens the interactive Edge print preview and Tauri's own
//! `print()` is macOS-only, so the export goes one level down to the COM API:
//! `ICoreWebView2_7::PrintToPdf` renders the page with the `@media print`
//! stylesheet and writes the file itself — no dialog, no extra process, and the
//! text stays vector (unlike a canvas/screenshot based generator).
//!
//! Everything here has to run on the UI thread, so the actual call is posted via
//! `with_webview` and the result is awaited on the caller's (worker) thread.
#![cfg(windows)]

use std::{sync::mpsc, time::Duration};

use tauri::{webview::PlatformWebview, WebviewWindow};
use webview2_com::{
    Microsoft::Web::WebView2::Win32::{ICoreWebView2Environment6, ICoreWebView2_7},
    PrintToPdfCompletedHandler,
};
use windows::core::{Interface, HSTRING};

use crate::error::{Error, Result};

/// A4 in CSS inches. The page margins are applied here, not in the stylesheet:
/// an element's padding is only honoured once, so using it for margins would
/// leave every page after the first flush against the paper edge.
const A4_WIDTH_IN: f64 = 8.27;
const A4_HEIGHT_IN: f64 = 11.69;
/// 20mm top/bottom, 18mm left/right — the same numbers the on-screen preview
/// shows via `.print-paper`'s padding.
const MARGIN_Y_IN: f64 = 0.79;
const MARGIN_X_IN: f64 = 0.71;

/// Long enough for a note with images, short enough to fail visibly if the
/// callback never fires.
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

/// Render `window` into the PDF at `path`.
///
/// Must not be called on the UI thread: it blocks until WebView2 reports that
/// the file has been written.
pub fn export(window: &WebviewWindow, path: &str) -> Result<()> {
    let (tx, rx) = mpsc::channel::<std::result::Result<(), String>>();
    let target = path.to_string();
    let post = tx.clone();

    window
        .with_webview(move |platform| {
            if let Err(msg) = start(&platform, &target, post.clone()) {
                let _ = post.send(Err(msg));
            }
        })
        .map_err(|e| Error::msg(format!("无法访问窗口的 WebView: {e}")))?;

    match rx.recv_timeout(WRITE_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(msg)) => Err(Error::msg(msg)),
        Err(_) => Err(Error::msg("导出超时：WebView2 没有在 120 秒内完成 PDF 渲染")),
    }
}

/// Kick off the render. Returns as soon as the request has been accepted, the
/// outcome arrives through `done` (on the UI thread, later).
fn start(
    platform: &PlatformWebview,
    path: &str,
    done: mpsc::Sender<std::result::Result<(), String>>,
) -> std::result::Result<(), String> {
    let controller = platform.controller();
    let core = unsafe { controller.CoreWebView2() }
        .map_err(|e| format!("拿不到 CoreWebView2: {e}"))?;
    let print: ICoreWebView2_7 = core
        .cast()
        .map_err(|e| format!("当前 WebView2 运行时版本不支持 PrintToPdf: {e}"))?;
    let environment: ICoreWebView2Environment6 = platform
        .environment()
        .cast()
        .map_err(|e| format!("当前 WebView2 运行时没有打印设置接口: {e}"))?;
    let settings = unsafe { environment.CreatePrintSettings() }
        .map_err(|e| format!("创建打印设置失败: {e}"))?;

    unsafe {
        // Failures here only cost a default margin; the page is still printable.
        let _ = settings.SetPageWidth(A4_WIDTH_IN);
        let _ = settings.SetPageHeight(A4_HEIGHT_IN);
        let _ = settings.SetScaleFactor(1.0);
        let _ = settings.SetMarginTop(MARGIN_Y_IN);
        let _ = settings.SetMarginBottom(MARGIN_Y_IN);
        let _ = settings.SetMarginLeft(MARGIN_X_IN);
        let _ = settings.SetMarginRight(MARGIN_X_IN);
        // Code blocks, blockquotes and tables have light backgrounds; without
        // this the PDF comes out with white boxes where the fills should be.
        let _ = settings.SetShouldPrintBackgrounds(true);
        let _ = settings.SetShouldPrintHeaderAndFooter(false);
        let _ = settings.SetShouldPrintSelectionOnly(false);
    }

    let handler = PrintToPdfCompletedHandler::create(Box::new(move |code, ok| {
        let outcome = match (code, ok) {
            (Err(e), _) => Err(format!("PDF 渲染失败: {e}")),
            (Ok(()), true) => Ok(()),
            (Ok(()), false) => Err("PDF 渲染失败：WebView2 报告未成功".to_string()),
        };
        let _ = done.send(outcome);
        Ok(())
    }));

    let target = HSTRING::from(path);
    unsafe { print.PrintToPdf(&target, &settings, &handler) }
        .map_err(|e| format!("启动 PDF 渲染失败: {e}"))
}
