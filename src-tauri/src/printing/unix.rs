//! macOS / Linux printing through CUPS (`lpstat`, `lpr`).

use super::{PrintOptions, SystemPrinter};
use std::path::Path;
use std::process::Command;

/// `lpstat` output is localised ("printer", "Drucker", "imprimante" …); force the C locale
/// so it can be parsed.
fn cups(cmd: &str) -> Command {
    let mut command = Command::new(cmd);
    command.env("LC_ALL", "C").env("LANG", "C");
    command
}

/// Parses `lpstat -d`: "system default destination: NAME" (or "no system default destination").
pub(crate) fn parse_default(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("system default destination:")
            .map(|rest| rest.trim().to_string())
            .filter(|name| !name.is_empty())
    })
}

/// Parses `lpstat -e` (one destination per line, including `printer/instance` entries).
pub(crate) fn parse_destinations(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parses `lpstat -p` ("printer NAME is idle. ...").
pub(crate) fn parse_printer_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("printer "))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

pub fn list_printers() -> Result<Vec<SystemPrinter>, String> {
    let mut names = cups("lpstat")
        .arg("-e")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_destinations(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();

    if names.is_empty() {
        let output = cups("lpstat").arg("-p").output().map_err(|e| {
            format!("Could not query printers (is CUPS installed and running?): {e}")
        })?;
        names = parse_printer_lines(&String::from_utf8_lossy(&output.stdout));
    }

    let default = cups("lpstat")
        .arg("-d")
        .output()
        .ok()
        .and_then(|o| parse_default(&String::from_utf8_lossy(&o.stdout)));

    names.dedup();
    let mut printers: Vec<SystemPrinter> = names
        .into_iter()
        .map(|name| SystemPrinter {
            is_default: default.as_deref() == Some(name.as_str()),
            name,
            port_name: None,
        })
        .collect();
    // Default printer first.
    printers.sort_by_key(|p| !p.is_default);
    Ok(printers)
}

pub fn print_pdf(path: &Path, options: &PrintOptions) -> Result<(), String> {
    let mut cmd = cups("lpr");
    if let Some(printer) = &options.printer_name {
        cmd.arg("-P").arg(printer);
    }
    cmd.arg("-#").arg(options.copies.to_string());
    cmd.arg("-T").arg(&options.job_name);
    cmd.arg("-o").arg(if options.duplex {
        "sides=two-sided-long-edge"
    } else {
        "sides=one-sided"
    });
    if options.copies > 1 {
        cmd.arg("-o").arg("collate=true");
    }
    cmd.arg(path);

    let result = cmd.output();
    // lpr copies the file into the spool before returning, so the temp file can go.
    let _ = std::fs::remove_file(path);

    let output = result.map_err(|e| format!("Could not start lpr: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("The printer rejected the job: {err}"));
    }
    Ok(())
}

fn try_spawn(program: &str, args: &[&str]) -> bool {
    Command::new(program).args(args).spawn().is_ok()
}

pub fn open_printer_settings(add: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = add;
        // macOS 13+ (System Settings) first, then the classic preference pane.
        if Command::new("open")
            .arg("x-apple.systempreferences:com.apple.Printers-Scanners-Settings.extension")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        if try_spawn(
            "open",
            &["/System/Library/PreferencePanes/PrintAndFax.prefPane"],
        ) {
            return Ok(());
        }
        return Err("Could not open Printers & Scanners settings".to_string());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let candidates: &[(&str, &[&str])] = &[
            ("system-config-printer", &[]),
            ("gnome-control-center", &["printers"]),
            ("kcmshell6", &["kcm_printer_manager"]),
            ("kcmshell5", &["kcm_printer_manager"]),
        ];
        for (program, args) in candidates {
            if try_spawn(program, args) {
                return Ok(());
            }
        }
        // Fall back to the CUPS web interface.
        let url = if add {
            "http://localhost:631/admin/?OP=add-printer"
        } else {
            "http://localhost:631/printers/"
        };
        if try_spawn("xdg-open", &[url]) {
            return Ok(());
        }
        Err("Could not find a printer settings tool. Open http://localhost:631 in a browser to add a printer.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_destination() {
        assert_eq!(
            parse_default("system default destination: Office_HP\n"),
            Some("Office_HP".into())
        );
        assert_eq!(parse_default("no system default destination\n"), None);
    }

    #[test]
    fn parses_destinations_and_printer_lines() {
        assert_eq!(parse_destinations("A\nB/duplex\n\n"), vec!["A", "B/duplex"]);
        assert_eq!(
            parse_printer_lines("printer HP is idle.  enabled since x\nprinter Brother disabled\n"),
            vec!["HP", "Brother"]
        );
    }
}
