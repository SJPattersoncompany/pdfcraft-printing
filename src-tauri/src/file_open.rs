//! Opening PDFs handed to the app by the operating system (double-click, "Open with",
//! dragging onto the app icon, `pdfcraft file.pdf`).
//!
//! Only one window of this bundle runs at a time: when the app is launched again, the
//! single-instance plugin forwards the new launch's arguments to the running instance,
//! which queues the PDFs, brings its window to the front and tells the frontend, which
//! opens each file as a new tab in Studio. The second process then exits.
//! A different bundle id (the unofficial print-test build uses `com.pdfcraft.printtest`)
//! is a different instance and does not attach to `com.pdfcraft.app`.
//!
//! Files are queued (rather than only sent as an event) so nothing is lost when they
//! arrive before the page has finished loading — the page asks for the queue on start.
//! `read_file_binary` can read only paths this process queued. It does not fetch URLs.

use serde::Serialize;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Event emitted (no payload) whenever new files are queued.
pub const FILES_OPENED_EVENT: &str = "pdfcraft://files-opened";

#[derive(Default)]
pub struct OpenedFiles {
    pending_paths: Mutex<Vec<String>>,
    pending_errors: Mutex<Vec<String>>,
    /// Canonical paths the OS asked this process to open. IPC reads are limited to these.
    allowed: Mutex<HashSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    pub paths: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenedFileBatch {
    pub paths: Vec<String>,
    pub errors: Vec<String>,
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file:///C:/x.pdf` -> `C:/x.pdf`. A leading slash before a drive letter is URL syntax,
/// not a directory, so this is applied on every OS.
pub fn file_url_to_path(arg: &str) -> Option<PathBuf> {
    let rest = arg.strip_prefix("file://")?;
    let mut path = percent_decode(rest);
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[2] == b':' {
        path.remove(0);
    }
    Some(PathBuf::from(path))
}

fn display_canonical(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut s = path.display().to_string();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        if !stripped.starts_with("UNC\\") {
            s = stripped.to_string();
        }
    }
    s
}

fn classify_local_pdf(path: &Path) -> Result<String, String> {
    let shown = path.display().to_string();
    if !is_pdf(path) {
        return Err(format!("Not a PDF: {shown}"));
    }
    if !path.is_file() {
        return Err(format!("Could not find \"{shown}\"."));
    }
    Ok(display_canonical(path))
}

/// Picks PDF files out of a command line and reports the rest as errors.
/// `args` includes the program path first, which is skipped. Flags are ignored.
/// Relative paths are resolved against `cwd`. Duplicate paths in one launch are kept once.
/// A later launch of the same path is not dropped: that is a separate call.
pub fn parse_launch_args<I, S>(args: I, cwd: &Path) -> ParsedArgs
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut paths = Vec::new();
    let mut errors = Vec::new();
    for (index, arg) in args.into_iter().enumerate() {
        if index == 0 {
            continue;
        }
        let arg = arg.as_ref();
        if arg.is_empty() {
            continue;
        }
        let Some(text) = arg.to_str() else {
            errors.push("A file name could not be read as text.".to_string());
            continue;
        };
        if text.starts_with('-') {
            continue;
        }
        if text.contains("://") && !text.starts_with("file://") {
            errors.push(format!("Remote addresses are not opened: {text}"));
            continue;
        }
        let path = file_url_to_path(text).unwrap_or_else(|| PathBuf::from(text));
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        match classify_local_pdf(&path) {
            Ok(canonical) => {
                if !paths.contains(&canonical) {
                    paths.push(canonical);
                }
            }
            Err(error) => errors.push(error),
        }
    }
    ParsedArgs { paths, errors }
}

/// Paths only. Prefer [`parse_launch_args`] when the caller can show errors.
#[cfg_attr(not(test), allow(dead_code))]
pub fn pdf_paths_from_args<I>(args: I, cwd: &Path) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    parse_launch_args(args, cwd).paths
}

#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub fn accept_local_pdf(path: &Path) -> Result<String, String> {
    classify_local_pdf(path)
}

/// Queues files for the frontend and notifies it. Empty input does not emit.
pub fn queue<R: Runtime>(app: &AppHandle<R>, parsed: ParsedArgs) {
    if parsed.paths.is_empty() && parsed.errors.is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<OpenedFiles>() {
        if let Ok(mut allowed) = state.allowed.lock() {
            allowed.extend(parsed.paths.iter().cloned());
        }
        if let Ok(mut pending) = state.pending_paths.lock() {
            pending.extend(parsed.paths);
        }
        if let Ok(mut pending) = state.pending_errors.lock() {
            pending.extend(parsed.errors);
        }
    }
    let _ = app.emit(FILES_OPENED_EVENT, ());
}

/// Brings the main window to the front (restoring it if minimised).
pub fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        if window.set_focus().is_err() {
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }
}

/// Returns and clears the queued file paths and parse errors.
#[tauri::command]
pub fn take_opened_files(state: tauri::State<'_, OpenedFiles>) -> OpenedFileBatch {
    let paths = state
        .pending_paths
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default();
    let errors = state
        .pending_errors
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default();
    OpenedFileBatch { paths, errors }
}

/// True when files or errors are waiting (without taking them).
#[tauri::command]
pub fn has_opened_files(state: tauri::State<'_, OpenedFiles>) -> bool {
    let paths = state
        .pending_paths
        .lock()
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    let errors = state
        .pending_errors
        .lock()
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    paths || errors
}

/// Reads a queued local PDF as a raw binary IPC response.
/// Paths the OS did not hand to this process, URLs, and non-PDFs are refused.
#[tauri::command]
pub async fn read_file_binary(
    state: tauri::State<'_, OpenedFiles>,
    path: String,
) -> Result<tauri::ipc::Response, String> {
    let read_path = {
        let allowed = state
            .allowed
            .lock()
            .map_err(|_| "Could not read the opened-file list.".to_string())?;
        authorize_read(&path, &allowed)?
    };
    tauri::async_runtime::spawn_blocking(move || std::fs::read(&read_path))
        .await
        .map_err(|e| e.to_string())?
        .map(tauri::ipc::Response::new)
        .map_err(|e| format!("Could not open \"{path}\": {e}"))
}

pub fn authorize_read(path: &str, allowed: &HashSet<String>) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("No file path was provided.".into());
    }
    if trimmed.contains("://") || trimmed.starts_with("file:") {
        return Err("Only a local PDF that this app was asked to open can be read.".into());
    }
    let raw = PathBuf::from(trimmed);
    if !is_pdf(&raw) {
        return Err(format!("Not a PDF: {trimmed}"));
    }
    if !raw.is_file() {
        return Err(format!("Could not find \"{trimmed}\"."));
    }
    let canonical = display_canonical(&raw);
    let raw_display = raw.display().to_string();
    if !allowed.contains(&canonical) && !allowed.contains(trimmed) && !allowed.contains(&raw_display)
    {
        return Err("That file was not opened from this app.".into());
    }
    Ok(PathBuf::from(canonical))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pdfcraft_args_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn picks_existing_pdfs_from_command_line() {
        let dir = scratch();
        std::fs::create_dir_all(dir.join("sub dir")).unwrap();
        std::fs::write(dir.join("a.pdf"), b"%PDF").unwrap();
        std::fs::write(dir.join("sub dir").join("B.PDF"), b"%PDF").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let abs_b = dir.join("sub dir").join("B.PDF").display().to_string();

        let args = vec![
            "C:/Program Files/PDFCraft/pdfcraft.exe".to_string(),
            "--flag".into(),
            "a.pdf".into(),
            abs_b.clone(),
            "a.pdf".into(),
            "notes.txt".into(),
            "missing.pdf".into(),
        ];
        let found = pdf_paths_from_args(args, &dir);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].ends_with("a.pdf"), "{found:?}");
        assert!(found[1].ends_with("B.PDF"), "{found:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn program_path_alone_opens_nothing() {
        let args = vec!["pdfcraft".to_string()];
        assert!(pdf_paths_from_args(args, Path::new("/")).is_empty());
    }

    #[test]
    fn decodes_file_urls_with_spaces_unicode_and_drive_letters() {
        assert_eq!(
            file_url_to_path("file:///home/me/My%20Report.pdf"),
            Some(PathBuf::from("/home/me/My Report.pdf"))
        );
        assert_eq!(
            file_url_to_path("file:///tmp/%E6%8A%A5%E5%91%8A.pdf"),
            Some(PathBuf::from("/tmp/报告.pdf"))
        );
        assert_eq!(
            file_url_to_path("file:///C:/Users/Me/My%20Report.pdf"),
            Some(PathBuf::from("C:/Users/Me/My Report.pdf"))
        );
        assert_eq!(file_url_to_path("/plain/path.pdf"), None);
        assert_eq!(file_url_to_path("https://example.com/a.pdf"), None);
    }

    #[test]
    fn reports_missing_non_pdf_and_remote_args() {
        let dir = scratch();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let parsed = parse_launch_args(
            [
                "pdfcraft",
                "notes.txt",
                "missing.pdf",
                "https://example.com/secret.pdf",
                "--ignored",
            ],
            &dir,
        );
        assert!(parsed.paths.is_empty(), "{parsed:?}");
        assert!(
            parsed.errors.iter().any(|e| e.contains("notes.txt") && e.contains("Not a PDF")),
            "{parsed:?}"
        );
        assert!(
            parsed.errors.iter().any(|e| e.contains("missing.pdf")),
            "{parsed:?}"
        );
        assert!(
            parsed.errors.iter().any(|e| e.contains("Remote addresses")),
            "{parsed:?}"
        );
        assert!(!parsed.errors.iter().any(|e| e.contains("ignored")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn spaces_unicode_and_a_later_open_of_the_same_file() {
        let dir = scratch();
        let spaced = dir.join("My Report.pdf");
        let unicode = dir.join("报告.pdf");
        std::fs::write(&spaced, b"%PDF").unwrap();
        std::fs::write(&unicode, b"%PDF").unwrap();
        let first = parse_launch_args(
            ["pdfcraft", "My Report.pdf", "报告.pdf", "My Report.pdf"],
            &dir,
        );
        assert_eq!(first.paths.len(), 2, "{first:?}");
        assert!(first.errors.is_empty(), "{first:?}");
        assert!(first.paths.iter().any(|p| p.contains("My Report.pdf")));
        assert!(first.paths.iter().any(|p| p.contains("报告.pdf")));
        let again = parse_launch_args(["pdfcraft", "报告.pdf"], &dir);
        assert_eq!(again.paths.len(), 1, "{again:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_is_limited_to_queued_local_pdfs() {
        let dir = scratch();
        let pdf = dir.join("queued.pdf");
        let other = dir.join("other.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        std::fs::write(&other, b"%PDF").unwrap();
        let queued = parse_launch_args(["pdfcraft", "queued.pdf"], &dir);
        let allowed: HashSet<String> = queued.paths.iter().cloned().collect();
        let ok = authorize_read(&queued.paths[0], &allowed).expect("queued pdf");
        assert!(ok.ends_with("queued.pdf"));
        assert!(authorize_read(&other.display().to_string(), &allowed).is_err());
        assert!(authorize_read("https://example.com/queued.pdf", &allowed).is_err());
        assert!(authorize_read("file:///tmp/queued.pdf", &allowed).is_err());
        assert!(authorize_read("", &allowed).is_err());
        assert!(authorize_read("notes.txt", &allowed).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
