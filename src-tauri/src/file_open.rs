//! Opening PDFs handed to the app by the operating system (double-click, "Open with",
//! dragging onto the app icon, `pdfcraft file.pdf`).
//!
//! Only one PDFCraft window runs at a time: when the app is launched again, the
//! single-instance plugin forwards the new launch's arguments to the running instance,
//! which queues the PDFs, brings its window to the front and tells the frontend, which
//! opens each file as a new tab in Studio. The second process then exits.
//!
//! Files are queued (rather than only sent as an event) so nothing is lost when they
//! arrive before the page has finished loading — the page asks for the queue on start.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Event emitted (no payload) whenever new files are queued.
pub const FILES_OPENED_EVENT: &str = "pdfcraft://files-opened";

#[derive(Default)]
pub struct OpenedFiles(Mutex<Vec<String>>);

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Turns a `file://` URL (Linux desktops, macOS) into a path.
fn from_file_url(arg: &str) -> Option<PathBuf> {
    let rest = arg.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let mut path = String::from_utf8_lossy(&out).into_owned();
    // file:///C:/x.pdf -> C:/x.pdf on Windows
    if cfg!(windows) && path.len() > 2 && path.as_bytes()[2] == b':' {
        path.remove(0);
    }
    Some(PathBuf::from(path))
}

/// Picks the PDF files out of a command line. `args` includes the program path first,
/// which is skipped; flags are ignored; relative paths are resolved against `cwd`; only
/// files that exist are returned, without duplicates.
pub fn pdf_paths_from_args<I>(args: I, cwd: &Path) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut out: Vec<String> = Vec::new();
    for arg in args.into_iter().skip(1) {
        if arg.starts_with('-') || arg.is_empty() {
            continue;
        }
        let path = from_file_url(&arg).unwrap_or_else(|| PathBuf::from(&arg));
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        if !is_pdf(&path) || !path.is_file() {
            continue;
        }
        let path = path.canonicalize().unwrap_or(path);
        let mut s = path.display().to_string();
        // canonicalize() on Windows yields \\?\C:\..., which file names don't need.
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            if !stripped.starts_with("UNC\\") {
                s = stripped.to_string();
            }
        }
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Queues files for the frontend and notifies it.
pub fn queue<R: Runtime>(app: &AppHandle<R>, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<OpenedFiles>() {
        if let Ok(mut pending) = state.0.lock() {
            pending.extend(paths);
        }
    }
    let _ = app.emit(FILES_OPENED_EVENT, ());
}

/// Brings the main window to the front (restoring it if minimised).
pub fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Returns and clears the queued file paths.
#[tauri::command]
pub fn take_opened_files(state: tauri::State<'_, OpenedFiles>) -> Vec<String> {
    state
        .0
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}

/// True when files are waiting to be opened (without taking them).
#[tauri::command]
pub fn has_opened_files(state: tauri::State<'_, OpenedFiles>) -> bool {
    state.0.lock().map(|p| !p.is_empty()).unwrap_or(false)
}

/// Reads a file as a raw binary IPC response (fast for large PDFs, unlike a JSON array).
#[tauri::command]
pub async fn read_file_binary(path: String) -> Result<tauri::ipc::Response, String> {
    let read_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || std::fs::read(read_path))
        .await
        .map_err(|e| e.to_string())?
        .map(tauri::ipc::Response::new)
        .map_err(|e| format!("Could not open \"{path}\": {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_existing_pdfs_from_command_line() {
        let dir = std::env::temp_dir().join(format!("pdfcraft_args_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub dir")).unwrap();
        std::fs::write(dir.join("a.pdf"), b"%PDF").unwrap();
        std::fs::write(dir.join("sub dir").join("B.PDF"), b"%PDF").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let abs_b = dir.join("sub dir").join("B.PDF").display().to_string();

        let args = vec![
            "C:/Program Files/PDFCraft/pdfcraft.exe".to_string(),
            "--flag".into(),
            "a.pdf".into(),       // relative to cwd
            abs_b.clone(),        // absolute, upper-case extension
            "a.pdf".into(),       // duplicate
            "notes.txt".into(),   // not a PDF
            "missing.pdf".into(), // does not exist
        ];
        let found = pdf_paths_from_args(args, &dir);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].ends_with("a.pdf"));
        assert!(found[1].ends_with("B.PDF"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn program_path_alone_opens_nothing() {
        let args = vec!["pdfcraft".to_string()];
        assert!(pdf_paths_from_args(args, Path::new("/")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn decodes_file_urls() {
        assert_eq!(
            from_file_url("file:///home/me/My%20Report.pdf"),
            Some(PathBuf::from("/home/me/My Report.pdf"))
        );
        assert_eq!(from_file_url("/plain/path.pdf"), None);
    }
}
