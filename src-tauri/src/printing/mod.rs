//! Native printing for the desktop app.
//!
//! * Listing printers, printing a PDF and opening the operating system's printer setup
//!   are exposed as async Tauri commands, so a slow print spooler or a slow network
//!   printer never freezes the window.
//! * On Windows the PDF is rendered with the PDF engine built into Windows
//!   (`Windows.Data.Pdf`) and sent straight to the chosen printer through GDI. Earlier
//!   builds asked the *default PDF application* to print the file
//!   (`Start-Process -Verb PrintTo`), which can exit successfully when that application
//!   is Microsoft Edge or any other viewer that does not register a print verb.
//!   That shell path is not used: its exit code is not evidence a job was spooled.
//! * On macOS and Linux the file is handed to CUPS with `lpr`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod layout;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SystemPrinter {
    pub name: String,
    pub is_default: bool,
    pub port_name: Option<String>,
}

/// Options for one print job.
#[derive(Debug, Clone)]
pub struct PrintOptions {
    /// Printer to use; `None` means the system default printer.
    pub printer_name: Option<String>,
    /// Number of (collated) copies, 1..=99.
    pub copies: u32,
    /// Print on both sides of the paper (long-edge binding).
    pub duplex: bool,
    /// Name shown in the print queue.
    pub job_name: String,
}

/// What a legacy shell print verb reported.
///
/// `ExitedZero` is not a spooled job. `Start-Process -Verb PrintTo` returns as soon
/// as the other program has been asked to start, including when that program does
/// not print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellReport {
    NotUsed,
    ExitedZero,
    Failed(String),
}

/// Native print result, plus an optional note when a shell verb was also tried.
/// A shell exit of 0 never turns a native failure into success.
pub fn combine_print_outcome(native: Result<(), String>, shell: ShellReport) -> Result<(), String> {
    match native {
        Ok(()) => Ok(()),
        Err(native_err) => {
            let note = match shell {
                ShellReport::NotUsed => String::new(),
                ShellReport::ExitedZero => " The default PDF app reported that it started, which does not mean the printer accepted the job.".to_string(),
                ShellReport::Failed(err) => {
                    if err.is_empty() {
                        " The default PDF app also failed to print.".to_string()
                    } else {
                        format!(" The default PDF app also failed to print: {err}")
                    }
                }
            };
            Err(format!("{native_err}{note}"))
        }
    }
}

/// Removes the temp PDF and returns [`combine_print_outcome`]. The file is removed
/// on success and on failure so a document is not left in the temp directory.
pub fn settle_print_file(
    path: &std::path::Path,
    native: Result<(), String>,
    shell: ShellReport,
) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    combine_print_outcome(native, shell)
}

fn temp_pdf_path() -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("pdfcraft_print_{timestamp}.pdf"))
}

/// Decodes a `encodeURIComponent` value sent in an IPC header (header values must be ASCII).
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
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

fn header(request: &tauri::ipc::Request<'_>, name: &str) -> Option<String> {
    request
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .filter(|v| !v.trim().is_empty())
}

/// Runs blocking work on its own OS thread (COM / WinRT need a thread they can initialise).
async fn run_blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("Printing task failed: {e}"))?
}

#[tauri::command]
pub async fn get_system_printers() -> Result<Vec<SystemPrinter>, String> {
    run_blocking(|| {
        #[cfg(windows)]
        {
            windows::list_printers()
        }
        #[cfg(unix)]
        {
            unix::list_printers()
        }
        #[cfg(not(any(windows, unix)))]
        {
            Ok(vec![])
        }
    })
    .await
}

/// Prints a PDF. The PDF bytes travel as the raw IPC body (much faster than a JSON number
/// array for large documents); options travel as headers:
/// `x-printer-name`, `x-copies`, `x-duplex` ("1"/"0") and `x-job-name`
/// (string values URI-encoded).
#[tauri::command]
pub async fn print_pdf_file(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let data: Vec<u8> = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        tauri::ipc::InvokeBody::Json(value) => {
            // Accept the old `{ data: number[] }` shape too.
            serde_json::from_value::<Vec<u8>>(value.get("data").cloned().unwrap_or_default())
                .map_err(|_| "No PDF data was sent to the printer".to_string())?
        }
    };
    if data.is_empty() {
        return Err("No PDF data was sent to the printer".to_string());
    }

    let options = PrintOptions {
        printer_name: header(&request, "x-printer-name"),
        copies: header(&request, "x-copies")
            .and_then(|c| c.parse::<u32>().ok())
            .unwrap_or(1)
            .clamp(1, 99),
        duplex: header(&request, "x-duplex").as_deref() == Some("1"),
        job_name: header(&request, "x-job-name").unwrap_or_else(|| "PDFCraft document".into()),
    };

    let path = temp_pdf_path();
    if let Err(error) = std::fs::write(&path, &data) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("Could not prepare the print file: {error}"));
    }
    let print_path = path.clone();

    let printed = run_blocking(move || {
        #[cfg(windows)]
        {
            windows::print_pdf(&print_path, &options)
        }
        #[cfg(unix)]
        {
            unix::print_pdf(&print_path, &options)
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (&print_path, &options);
            let _ = std::fs::remove_file(&print_path);
            Err("Printing is not supported on this operating system".to_string())
        }
    })
    .await;
    // The platform print function removes the file. This covers a task that
    // never reached that function (for example a cancelled runtime).
    if printed.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    printed
}

/// Opens the operating system's printer setup so the user can add or manage printers.
/// `mode` is `"add"` (Add Printer wizard where available) or `"manage"`.
#[tauri::command]
pub async fn open_printer_settings(mode: Option<String>) -> Result<(), String> {
    let add = mode.as_deref() != Some("manage");
    run_blocking(move || {
        #[cfg(windows)]
        {
            windows::open_printer_settings(add)
        }
        #[cfg(unix)]
        {
            unix::open_printer_settings(add)
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = add;
            Err("Printer settings are not available on this operating system".to_string())
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::{combine_print_outcome, percent_decode, settle_print_file, ShellReport};

    #[test]
    fn decodes_uri_components() {
        assert_eq!(
            percent_decode("HP%20LaserJet%20(Office)"),
            "HP LaserJet (Office)"
        );
        assert_eq!(percent_decode("%E5%8A%9E%E5%85%AC"), "办公");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("bad%zzend%"), "bad%zzend%");
    }

    #[test]
    fn shell_exit_zero_does_not_hide_a_native_print_error() {
        let err = combine_print_outcome(
            Err("Could not render page 1".into()),
            ShellReport::ExitedZero,
        )
        .expect_err("shell success is not a spooled job");
        assert!(err.contains("Could not render page 1"));
        assert!(err.contains("does not mean the printer accepted"));
        assert!(combine_print_outcome(Ok(()), ShellReport::NotUsed).is_ok());
    }

    #[test]
    fn native_error_is_preserved_and_temp_file_is_removed() {
        let path = std::env::temp_dir().join(format!(
            "pdfcraft_print_test_{}_{}",
            std::process::id(),
            "native-error"
        ));
        std::fs::write(&path, b"%PDF-test").expect("temp file");
        let err = settle_print_file(
            &path,
            Err("Windows could not read this PDF".into()),
            ShellReport::NotUsed,
        )
        .expect_err("native failure");
        assert_eq!(err, "Windows could not read this PDF");
        assert!(
            !path.exists(),
            "failed print must not leave the PDF in temp"
        );
    }

    #[test]
    fn native_success_removes_temp_file() {
        let path = std::env::temp_dir().join(format!(
            "pdfcraft_print_test_{}_{}",
            std::process::id(),
            "native-ok"
        ));
        std::fs::write(&path, b"%PDF-test").expect("temp file");
        settle_print_file(&path, Ok(()), ShellReport::NotUsed).expect("native success");
        assert!(
            !path.exists(),
            "successful print must not leave the PDF in temp"
        );
    }
}
