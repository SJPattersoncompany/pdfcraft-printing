//! Windows printing.
//!
//! Pages are rendered with the PDF engine that ships with Windows 10/11
//! (`Windows.Data.Pdf`) and drawn onto a GDI printer device context, so printing works
//! no matter which application is registered as the default PDF viewer.

use super::layout::{
    apply_orientation, gdi_compatible_dib, place_page, printer_enum_count, stretch_dibits_failed,
    Caps,
};
use super::{PrintOptions, ShellReport, SystemPrinter};
use std::path::Path;

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Graphics::Imaging::BitmapEncoder;
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, GetDeviceCaps, ResetDCW, SetBrushOrgEx, SetStretchBltMode, StretchDIBits,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DEVMODEW, DIB_RGB_COLORS, DMDUP_SIMPLEX, DMDUP_VERTICAL,
    DMORIENT_LANDSCAPE, DMORIENT_PORTRAIT, DM_DUPLEX, DM_ORIENTATION, DM_OUT_BUFFER, HALFTONE, HDC,
    HORZRES, LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY,
    PHYSICALWIDTH, SRCCOPY, VERTRES,
};
use windows::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW,
    PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE, PRINTER_INFO_4W,
};
use windows::Win32::Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

/// Render resolution cap. 300 dpi is plenty for text, sketches and photos and keeps a
/// Letter page at ~25 MB of pixels.
const MAX_RENDER_DPI: f64 = 300.0;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win_err(context: &str, e: windows::core::Error) -> String {
    format!("{context}: {}", e.message())
}

// ---------------------------------------------------------------------------------------
// Printer list
// ---------------------------------------------------------------------------------------

pub fn default_printer_name() -> Option<String> {
    unsafe {
        let mut len: u32 = 0;
        let _ = GetDefaultPrinterW(None, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        if GetDefaultPrinterW(Some(PWSTR(buf.as_mut_ptr())), &mut len).as_bool() {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        } else {
            None
        }
    }
}

/// Lists local printers and printer connections with `EnumPrintersW` level 4, which reads
/// the spooler's registry data and does not contact network printers. (The previous
/// implementation ran PowerShell + `Win32_Printer`, which can take minutes when a network
/// printer is offline.)
pub fn list_printers() -> Result<Vec<SystemPrinter>, String> {
    let default = default_printer_name();
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let mut printers = Vec::new();

    unsafe {
        let mut needed: u32 = 0;
        let mut returned: u32 = 0;
        let _ = EnumPrintersW(flags, PCWSTR::null(), 4, None, &mut needed, &mut returned);
        if needed > 0 {
            let record_size = std::mem::size_of::<PRINTER_INFO_4W>();
            // Refuse a huge or inconsistent count before any pointer is formed from it.
            printer_enum_count(needed, 0, record_size)?;
            let buffer_bytes = needed;
            // u64 storage keeps the PRINTER_INFO_4W array pointer-aligned.
            let mut storage = vec![0u64; (buffer_bytes as usize).div_ceil(8)];
            let bytes = std::slice::from_raw_parts_mut(
                storage.as_mut_ptr() as *mut u8,
                buffer_bytes as usize,
            );
            EnumPrintersW(
                flags,
                PCWSTR::null(),
                4,
                Some(bytes),
                &mut needed,
                &mut returned,
            )
            .map_err(|e| win_err("Could not list printers", e))?;
            let count = printer_enum_count(buffer_bytes, returned, record_size)?;
            let infos =
                std::slice::from_raw_parts(storage.as_ptr() as *const PRINTER_INFO_4W, count);
            for info in infos {
                if info.pPrinterName.is_null() {
                    continue;
                }
                let name = info.pPrinterName.to_string().unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                printers.push(SystemPrinter {
                    is_default: default.as_deref() == Some(name.as_str()),
                    name,
                    port_name: None,
                });
            }
        }
    }

    // A default printer that enumeration missed (rare, e.g. some redirected RDP printers).
    if let Some(def) = &default {
        if !printers.iter().any(|p| &p.name == def) {
            printers.push(SystemPrinter {
                name: def.clone(),
                is_default: true,
                port_name: None,
            });
        }
    }
    printers.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
    Ok(printers)
}

// ---------------------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------------------

struct PrinterHandle(PRINTER_HANDLE);
impl Drop for PrinterHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = ClosePrinter(self.0);
        }
    }
}

struct PrinterDc(HDC);
impl Drop for PrinterDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

/// The printer's current default DEVMODE (a variable-size buffer: the public struct is
/// followed by driver-private data, which must be kept).
struct DevMode {
    storage: Vec<u64>,
}
impl DevMode {
    fn ptr(&self) -> *const DEVMODEW {
        self.storage.as_ptr() as *const DEVMODEW
    }
    fn get_mut(&mut self) -> &mut DEVMODEW {
        unsafe { &mut *(self.storage.as_mut_ptr() as *mut DEVMODEW) }
    }
    fn set_orientation(&mut self, landscape: bool) {
        let dm = self.get_mut();
        dm.dmFields |= DM_ORIENTATION;
        // Writing a Copy field of a union is safe.
        dm.Anonymous1.Anonymous1.dmOrientation = if landscape {
            DMORIENT_LANDSCAPE as i16
        } else {
            DMORIENT_PORTRAIT as i16
        };
    }
    fn set_duplex(&mut self, duplex: bool) {
        let dm = self.get_mut();
        dm.dmFields |= DM_DUPLEX;
        dm.dmDuplex = if duplex {
            DMDUP_VERTICAL
        } else {
            DMDUP_SIMPLEX
        };
    }
}

fn load_devmode(printer: &[u16]) -> Option<DevMode> {
    unsafe {
        let mut handle = PRINTER_HANDLE::default();
        OpenPrinterW(PCWSTR(printer.as_ptr()), &mut handle, None).ok()?;
        let handle = PrinterHandle(handle);
        let size = DocumentPropertiesW(None, handle.0, PCWSTR(printer.as_ptr()), None, None, 0);
        if size <= 0 {
            return None;
        }
        let mut storage = vec![0u64; (size as usize).div_ceil(8)];
        let out = storage.as_mut_ptr() as *mut DEVMODEW;
        let r = DocumentPropertiesW(
            None,
            handle.0,
            PCWSTR(printer.as_ptr()),
            Some(out),
            None,
            DM_OUT_BUFFER.0,
        );
        if r < 0 {
            return None;
        }
        Some(DevMode { storage })
    }
}

/// One rendered page, ready for `StretchDIBits` as 24 bpp BI_RGB (no alpha byte).
struct PageBitmap {
    header: BITMAPINFOHEADER,
    pixels: Vec<u8>,
}
impl PageBitmap {
    fn from_bmp(data: Vec<u8>) -> Result<Self, String> {
        let dib = gdi_compatible_dib(&data)?;
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: dib.width,
            biHeight: dib.height,
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        Ok(Self {
            header,
            pixels: dib.pixels,
        })
    }
    fn width(&self) -> i32 {
        self.header.biWidth
    }
    fn height(&self) -> i32 {
        self.header.biHeight.abs()
    }
}

fn render_page(doc: &PdfDocument, index: u32, dpi: f64) -> Result<PageBitmap, String> {
    let page = doc
        .GetPage(index)
        .map_err(|e| win_err("Could not read page", e))?;
    let size = page
        .Size()
        .map_err(|e| win_err("Could not read page size", e))?; // 1/96 inch
    let width_px = ((size.Width as f64 / 96.0) * dpi)
        .round()
        .clamp(1.0, 12_000.0) as u32;

    let options = PdfPageRenderOptions::new().map_err(|e| win_err("Render options", e))?;
    // Only the width is set; the height follows the page's aspect ratio (after rotation).
    options
        .SetDestinationWidth(width_px)
        .map_err(|e| win_err("Render options", e))?;
    options
        .SetBitmapEncoderId(BitmapEncoder::BmpEncoderId().map_err(|e| win_err("BMP encoder", e))?)
        .map_err(|e| win_err("Render options", e))?;

    let stream = InMemoryRandomAccessStream::new().map_err(|e| win_err("Render stream", e))?;
    page.RenderWithOptionsToStreamAsync(&stream, &options)
        .and_then(|op| op.join())
        .map_err(|e| win_err(&format!("Could not render page {}", index + 1), e))?;

    let len = stream.Size().map_err(|e| win_err("Render stream", e))? as u32;
    let input = stream
        .GetInputStreamAt(0)
        .map_err(|e| win_err("Render stream", e))?;
    let reader = DataReader::CreateDataReader(&input).map_err(|e| win_err("Render stream", e))?;
    reader
        .LoadAsync(len)
        .and_then(|op| op.join())
        .map_err(|e| win_err("Render stream", e))?;
    let mut bytes = vec![0u8; len as usize];
    reader
        .ReadBytes(&mut bytes)
        .map_err(|e| win_err("Render stream", e))?;
    PageBitmap::from_bmp(bytes)
}

fn caps(hdc: HDC) -> Caps {
    unsafe {
        let g = |i| GetDeviceCaps(Some(hdc), i);
        Caps {
            dpi_x: g(LOGPIXELSX).max(1),
            dpi_y: g(LOGPIXELSY).max(1),
            printable_w: g(HORZRES).max(1),
            printable_h: g(VERTRES).max(1),
            physical_w: g(PHYSICALWIDTH).max(1),
            physical_h: g(PHYSICALHEIGHT).max(1),
            offset_x: g(PHYSICALOFFSETX),
            offset_y: g(PHYSICALOFFSETY),
        }
    }
}

fn native_print(path: &Path, options: &PrintOptions) -> Result<(), String> {
    unsafe {
        let hr = RoInitialize(RO_INIT_MULTITHREADED);
        if let Err(e) = &hr {
            if e.code() != RPC_E_CHANGED_MODE {
                return Err(win_err("Could not initialise Windows printing", e.clone()));
            }
        }
    }

    let printer = match &options.printer_name {
        Some(name) => name.clone(),
        None => default_printer_name()
            .ok_or("No printer was selected and Windows has no default printer")?,
    };
    let printer_w = wide(&printer);

    // Load the PDF first so a damaged file never leaves an empty job in the queue.
    let path_h = HSTRING::from(path.as_os_str());
    let file = StorageFile::GetFileFromPathAsync(&path_h)
        .and_then(|op| op.join())
        .map_err(|e| win_err("Could not open the PDF for printing", e))?;
    let doc = PdfDocument::LoadFromFileAsync(&file)
        .and_then(|op| op.join())
        .map_err(|e| win_err("Windows could not read this PDF", e))?;
    let page_count = doc
        .PageCount()
        .map_err(|e| win_err("Could not read the PDF", e))?;
    if page_count == 0 {
        return Err("The PDF has no pages".into());
    }

    let mut devmode = load_devmode(&printer_w);
    if let Some(dm) = devmode.as_mut() {
        dm.set_duplex(options.duplex);
        dm.set_orientation(false);
    }

    let driver_w = wide("WINSPOOL");
    let hdc = unsafe {
        CreateDCW(
            PCWSTR(driver_w.as_ptr()),
            PCWSTR(printer_w.as_ptr()),
            PCWSTR::null(),
            devmode.as_ref().map(|d| d.ptr()),
        )
    };
    if hdc.is_invalid() {
        return Err(format!(
            "Could not connect to printer \"{printer}\". Check that it is turned on and installed in Windows."
        ));
    }
    let dc = PrinterDc(hdc);

    let doc_name = wide(&options.job_name);
    let info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: PCWSTR(doc_name.as_ptr()),
        ..Default::default()
    };
    if unsafe { StartDocW(dc.0, &info) } <= 0 {
        return Err(format!(
            "Printer \"{printer}\" did not accept the job. It may be offline or paused."
        ));
    }

    let result = (|| -> Result<(), String> {
        let start_caps = caps(dc.0);
        let render_dpi = (start_caps.dpi_x.max(start_caps.dpi_y) as f64).min(MAX_RENDER_DPI);
        let mut landscape_now = false;
        let mut sheets_side = 0u32; // pages emitted, to pad odd duplex copies

        for copy in 0..options.copies {
            for index in 0..page_count {
                let bmp = render_page(&doc, index, render_dpi)?;
                let page_w_in = bmp.width() as f64 / render_dpi;
                let page_h_in = bmp.height() as f64 / render_dpi;
                let landscape = page_w_in > page_h_in;

                // Turn the paper for landscape pages (maps, sketches, photo pages).
                // ResetDCW returns NULL on failure. Recording the turn anyway places
                // the next pages for a sheet the driver did not rotate.
                if landscape != landscape_now {
                    let reset_ok = if let Some(dm) = devmode.as_mut() {
                        dm.set_orientation(landscape);
                        let reset = unsafe { ResetDCW(dc.0, dm.ptr()) };
                        !reset.is_invalid()
                    } else {
                        false
                    };
                    landscape_now = apply_orientation(landscape_now, landscape, reset_ok)?;
                }

                if unsafe { StartPage(dc.0) } <= 0 {
                    return Err("The printer stopped accepting pages".into());
                }
                let c = caps(dc.0);
                let (x, y, w, h) = place_page(page_w_in, page_h_in, &c);
                let drawn = unsafe {
                    SetStretchBltMode(dc.0, HALFTONE);
                    let _ = SetBrushOrgEx(dc.0, 0, 0, None);
                    StretchDIBits(
                        dc.0,
                        x,
                        y,
                        w,
                        h,
                        0,
                        0,
                        bmp.width(),
                        bmp.height(),
                        Some(bmp.pixels.as_ptr() as *const _),
                        &bmp.header as *const BITMAPINFOHEADER as *const BITMAPINFO,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    )
                };
                // GDI_ERROR is -1. Treating only 0 as failure reports a blank job as sent.
                if stretch_dibits_failed(drawn) {
                    unsafe { EndPage(dc.0) };
                    return Err(format!("Could not send page {} to the printer", index + 1));
                }
                if unsafe { EndPage(dc.0) } <= 0 {
                    return Err("The printer stopped accepting pages".into());
                }
                sheets_side += 1;
            }
            // Two-sided with an odd page count: add a blank back so the next copy starts
            // on a fresh sheet.
            if options.duplex && sheets_side % 2 == 1 && copy + 1 < options.copies {
                unsafe {
                    StartPage(dc.0);
                    EndPage(dc.0);
                }
                sheets_side += 1;
            }
        }
        Ok(())
    })();

    unsafe {
        if result.is_ok() {
            if EndDoc(dc.0) <= 0 {
                return Err("The printer did not finish the job".into());
            }
        } else {
            AbortDoc(dc.0);
        }
    }
    result
}

pub fn print_pdf(path: &Path, options: &PrintOptions) -> Result<(), String> {
    let native = native_print(path, options);
    // Do not ask the default PDF app to print. `Start-Process -Verb PrintTo` can exit 0
    // when nothing was spooled (the failure in #140 and #235), and the temp PDF would
    // have to be left behind for that process to read. The native error is returned
    // and the temp file is always removed.
    super::settle_print_file(path, native, ShellReport::NotUsed)
}

// ---------------------------------------------------------------------------------------
// Printer setup
// ---------------------------------------------------------------------------------------

pub fn open_printer_settings(add: bool) -> Result<(), String> {
    let spawned = if add {
        // The classic "Add Printer" wizard: search the network, or add by IP address /
        // hostname / shared-printer path, and install the driver.
        std::process::Command::new("rundll32.exe")
            .args(["printui.dll,PrintUIEntry", "/il"])
            .spawn()
    } else {
        std::process::Command::new("explorer.exe")
            .arg("ms-settings:printers")
            .spawn()
    };
    spawned
        .map(|_| ())
        .map_err(|e| format!("Could not open Windows printer settings: {e}"))
}
