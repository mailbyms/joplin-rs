//! Export the current webview to PDF on macOS through WKWebView.
//!
//! Windows gets `ICoreWebView2_7::PrintToPdf`, which renders the page with
//! `@media print` straight into a file. WKWebView has no file-writing print
//! API, but macOS 11+ exposes `createPDFWithConfiguration:completionHandler:`:
//! it returns the rendered document as `NSData`, still vector and with
//! selectable text, so we just write those bytes to the path the user picked.
//!
//! The important part is the capture rect. `WKPDFConfiguration` starts with a
//! null rect, which means "the bounds of the currently displayed web page" —
//! the visible viewport, *not* the whole document. Since the print sheet is a
//! fixed, internally scrolling overlay, that silently truncates a long note to
//! one screen. The frontend therefore measures the sheet's full scroll size
//! after pinning it to the paper width and passes it here; we set an explicit
//! rect from (0, 0) covering all of it. This is the same approach Textus uses
//! for its offscreen WKWebView, minus its A4 pagination.
//!
//! Two macOS-specific differences remain:
//!
//! * `createPDF` does **not** paginate. WebKit emits one tall page and ignores
//!   `@page { size: …; margin: … }`. Real A4 sheets need either
//!   `printOperationWithPrintInfo:` + `NSPrintSaveJob`, or cutting the tall
//!   page with PDFKit (as Textus does). A single tall page is still a valid,
//!   searchable PDF with all of the content — just not ideal for printing.
//! * WebKit cannot make a PDF page taller than 14400pt; beyond that it splits
//!   the capture at its own seam. Content is still complete, only the seam is
//!   arbitrary. We deliberately do not paginate, so we let WebKit decide.
//! * Unlike Chromium, WebKit does not turn `<a href="#id">` into named `/Dests`.
//!   `outline::apply` is therefore best-effort here: it usually finds no
//!   destinations and leaves the file untouched.
//!
//! The call must happen on the main thread, so it is posted with `with_webview`
//! and the result is awaited on the caller's (worker) thread.
#![cfg(target_os = "macos")]

use std::{fs, sync::mpsc, time::Duration};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSData, NSError};
use objc2_web_kit::{WKPDFConfiguration, WKWebView};
use tauri::{webview::PlatformWebview, WebviewWindow};

use crate::error::{Error, Result};

/// Long enough for a note with images, short enough to fail visibly if the
/// callback never fires.
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

/// Sanity guard against a broken measurement producing an absurd allocation
/// inside WebKit. A4 is 595pt wide; 100k pt is ~35 m of paper.
const MAX_CAPTURE: f64 = 100_000.0;

type Outcome = std::result::Result<Vec<u8>, String>;

/// Render `window` into the PDF at `path`.
///
/// `size` is the print sheet's `(scrollWidth, scrollHeight)` in CSS px, which
/// `createPDF` treats as 1 px = 1 pt. Must not be called on the UI thread: it
/// blocks until WebKit reports that the PDF data is ready.
pub fn export(window: &WebviewWindow, path: &str, size: Option<(f64, f64)>) -> Result<()> {
    let (tx, rx) = mpsc::channel::<Outcome>();
    let post = tx.clone();

    window
        .with_webview(move |platform| {
            if let Err(msg) = start(&platform, size, post.clone()) {
                let _ = post.send(Err(msg));
            }
        })
        .map_err(|e| Error::msg(format!("无法访问窗口的 WebView: {e}")))?;

    let bytes = match rx.recv_timeout(WRITE_TIMEOUT) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(msg)) => return Err(Error::msg(msg)),
        Err(_) => {
            return Err(Error::msg(
                "导出超时：WKWebView 没有在 120 秒内完成 PDF 渲染",
            ));
        }
    };

    fs::write(path, ensure_pdf(bytes)?)?;
    Ok(())
}

/// Kick off the render. Returns as soon as WebKit accepts the request; the
/// bytes arrive through `done` (on the main thread, later).
fn start(
    platform: &PlatformWebview,
    size: Option<(f64, f64)>,
    done: mpsc::Sender<Outcome>,
) -> std::result::Result<(), String> {
    let raw = platform.inner();
    if raw.is_null() {
        return Err("拿不到 WKWebView 句柄".to_string());
    }
    // SAFETY: Tauri hands us the live WKWebView for this window, and
    // `with_webview` runs this closure on the main thread. We only use the
    // reference for the duration of this call; the completion block captures
    // no pointer into the webview.
    let webview = unsafe { &*(raw as *const WKWebView) };

    let (width, height) = valid_size(size)?;
    let Some(mtm) = MainThreadMarker::new() else {
        return Err("PDF 导出未在主线程运行".to_string());
    };
    // SAFETY: `with_webview` runs on the main thread, which `MainThreadMarker`
    // just confirmed. WebKit copies the configuration before returning.
    let config = unsafe { WKPDFConfiguration::new(mtm) };
    unsafe {
        config.setRect(CGRect {
            origin: CGPoint::new(0.0, 0.0),
            size: CGSize::new(width, height),
        });
    }

    let handler = RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
        let outcome = if !error.is_null() {
            // SAFETY: WebKit passes a live NSError when the render fails.
            let description = unsafe { &*error }.localizedDescription().to_string();
            Err(format!("PDF 渲染失败: {description}"))
        } else if data.is_null() {
            Err("PDF 渲染失败：WKWebView 没有返回数据".to_string())
        } else {
            // SAFETY: WebKit passes a live NSData on success. Copy the bytes
            // out before the callback returns and the object is released.
            Ok(unsafe { &*data }.to_vec())
        };
        let _ = done.send(outcome);
    });

    // SAFETY: called on the main thread via `with_webview`. WebKit copies
    // completion blocks, so `handler` stays alive after this function returns
    // even though Rust drops its handle here.
    unsafe { webview.createPDFWithConfiguration_completionHandler(Some(&config), &handler) };
    Ok(())
}

fn valid_size(size: Option<(f64, f64)>) -> std::result::Result<(f64, f64), String> {
    match size {
        Some((w, h))
            if w.is_finite()
                && h.is_finite()
                && w > 0.0
                && h > 0.0
                && w <= MAX_CAPTURE
                && h <= MAX_CAPTURE =>
        {
            Ok((w, h))
        }
        Some((w, h)) => Err(format!("PDF 导出区域尺寸无效: {w} × {h}")),
        None => Err("无法确定 PDF 导出区域尺寸，请重试".to_string()),
    }
}

/// Guard against writing an error payload under a `.pdf` name.
fn ensure_pdf(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.starts_with(b"%PDF-") {
        Ok(bytes)
    } else {
        Err(Error::msg(
            "PDF 渲染失败：WKWebView 返回的数据不是 PDF",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_accepts_pdf_payloads() {
        assert!(ensure_pdf(b"%PDF-1.4\n1 0 obj".to_vec()).is_ok());
        assert!(ensure_pdf(b"<!doctype html>".to_vec()).is_err());
        assert!(ensure_pdf(Vec::new()).is_err());
    }

    #[test]
    fn rejects_missing_or_nonsensical_capture_sizes() {
        assert_eq!(valid_size(Some((794.0, 4888.0))), Ok((794.0, 4888.0)));
        assert!(valid_size(None).is_err());
        assert!(valid_size(Some((0.0, 100.0))).is_err());
        assert!(valid_size(Some((794.0, -1.0))).is_err());
        assert!(valid_size(Some((f64::NAN, 100.0))).is_err());
        assert!(valid_size(Some((f64::INFINITY, 100.0))).is_err());
        assert!(valid_size(Some((794.0, MAX_CAPTURE + 1.0))).is_err());
    }
}
