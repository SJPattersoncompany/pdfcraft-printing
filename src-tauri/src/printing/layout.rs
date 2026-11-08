//! Platform-neutral helpers for the Windows print path, kept here so they can be unit
//! tested on every platform.
#![cfg_attr(not(windows), allow(dead_code))]

/// The parts of a BMP file needed to hand its pixels to GDI.
#[derive(Debug, PartialEq, Eq)]
pub struct BmpInfo {
    pub width: i32,
    /// Positive = bottom-up rows, negative = top-down (as in BITMAPINFOHEADER).
    pub height: i32,
    pub bpp: u16,
    pub pixels_offset: usize,
}

/// Parses a BMP produced by the WIC BMP encoder (24 or 32 bpp, uncompressed or with the
/// standard BGRA bit masks).
pub fn parse_bmp(data: &[u8]) -> Result<BmpInfo, String> {
    if data.len() < 54 || &data[0..2] != b"BM" {
        return Err("Page rendering produced an invalid bitmap".into());
    }
    let rd_u16 = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
    let rd_u32 = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    let pixels_offset = rd_u32(10) as usize;
    let width = rd_u32(18) as i32;
    let height = rd_u32(22) as i32;
    let bpp = rd_u16(28);
    let compression = rd_u32(30);
    if width <= 0
        || height == 0
        || !(bpp == 24 || bpp == 32)
        || !(compression == 0 || compression == 3)
    {
        return Err(format!(
            "Unsupported page bitmap ({width}x{height}, {bpp} bpp, compression {compression})"
        ));
    }
    let stride = (width as usize * bpp as usize).div_ceil(32) * 4;
    let needed = stride
        .checked_mul(height.unsigned_abs() as usize)
        .and_then(|n| n.checked_add(pixels_offset))
        .ok_or("Page bitmap is too large")?;
    if needed > data.len() {
        return Err("Page bitmap is truncated".into());
    }
    Ok(BmpInfo {
        width,
        height,
        bpp,
        pixels_offset,
    })
}

/// Printer device capabilities, in device pixels.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub dpi_x: i32,
    pub dpi_y: i32,
    pub printable_w: i32,
    pub printable_h: i32,
    pub physical_w: i32,
    pub physical_h: i32,
    pub offset_x: i32,
    pub offset_y: i32,
}

/// Where a page lands on paper, as (x, y, w, h) in printable-area coordinates: actual
/// size, centred on the sheet, shrunk only when it does not fit inside the printable area
/// (like "Shrink oversized pages" in Acrobat).
pub fn place_page(page_w_in: f64, page_h_in: f64, c: &Caps) -> (i32, i32, i32, i32) {
    let natural_w = (page_w_in * c.dpi_x as f64).max(1.0);
    let natural_h = (page_h_in * c.dpi_y as f64).max(1.0);
    let scale = (c.printable_w as f64 / natural_w)
        .min(c.printable_h as f64 / natural_h)
        .min(1.0);
    let w = (natural_w * scale).round() as i32;
    let h = (natural_h * scale).round() as i32;
    let x = ((c.physical_w - w) / 2 - c.offset_x).clamp(0, (c.printable_w - w).max(0));
    let y = ((c.physical_h - h) / 2 - c.offset_y).clamp(0, (c.printable_h - h).max(0));
    (x, y, w, h)
}

/// `StretchDIBits` returns the number of scan lines copied, or `GDI_ERROR` (`-1`).
/// Zero and every negative value are failures. A check of `== 0` misses `GDI_ERROR`.
pub fn stretch_dibits_failed(scan_lines: i32) -> bool {
    scan_lines <= 0
}

/// Cap for one `EnumPrintersW` buffer. A spooler that asks for more is refused
/// instead of allocating an unbounded block.
pub const MAX_PRINTER_ENUM_BYTES: u32 = 16 * 1024 * 1024;

/// How many `PRINTER_INFO_*` records may be read from an enumeration buffer.
/// `returned` larger than the buffer is an error: slicing that far would read
/// off the end of the allocation.
pub fn printer_enum_count(
    buffer_bytes: u32,
    returned: u32,
    record_size: usize,
) -> Result<usize, String> {
    if record_size == 0 {
        return Err("Could not list printers: invalid record size".into());
    }
    if buffer_bytes > MAX_PRINTER_ENUM_BYTES {
        return Err("Could not list printers: the list from Windows was unexpectedly large".into());
    }
    let max = (buffer_bytes as usize) / record_size;
    let count = returned as usize;
    if count > max {
        return Err("Could not list printers: the list from Windows did not fit its buffer".into());
    }
    Ok(count)
}

/// Paper orientation after `ResetDCW`. A failed reset must not be recorded as
/// the new orientation, or later pages are placed for a sheet that did not turn.
pub fn apply_orientation(
    current_landscape: bool,
    requested_landscape: bool,
    reset_succeeded: bool,
) -> Result<bool, String> {
    if requested_landscape == current_landscape {
        return Ok(current_landscape);
    }
    if !reset_succeeded {
        return Err("The printer could not turn the page for this sheet.".into());
    }
    Ok(requested_landscape)
}

/// 24 bpp `BI_RGB` pixels for GDI. Windows.Data.Pdf's BMP encoder writes 32 bpp
/// with an alpha byte. Some printer drivers treat that byte as alpha and draw a
/// blank page when it is zero. The alpha byte is dropped; the color bytes are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdiDib {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

pub fn gdi_compatible_dib(data: &[u8]) -> Result<GdiDib, String> {
    let info = parse_bmp(data)?;
    let header_size = read_u32(data, 14)?;
    if header_size < 40 {
        return Err("Page rendering produced an invalid bitmap".into());
    }
    let compression = read_u32(data, 30)?;
    let rows = info.height.unsigned_abs() as usize;
    let src_stride = (info.width as usize * info.bpp as usize).div_ceil(32) * 4;
    let dst_stride = (info.width as usize)
        .checked_mul(3)
        .map(|n| n.div_ceil(4) * 4)
        .ok_or("Page bitmap is too large")?;
    let dst_len = dst_stride
        .checked_mul(rows)
        .ok_or("Page bitmap is too large")?;
    let mut pixels = vec![0u8; dst_len];

    if info.bpp == 24 {
        if compression != 0 {
            return Err("Unsupported 24 bpp page bitmap".into());
        }
        let row_bytes = (info.width as usize)
            .checked_mul(3)
            .ok_or("Page bitmap is too large")?;
        for row in 0..rows {
            let src = info
                .pixels_offset
                .checked_add(row * src_stride)
                .ok_or("Page bitmap is truncated")?;
            let src_end = src
                .checked_add(row_bytes)
                .ok_or("Page bitmap is truncated")?;
            let row_px = data.get(src..src_end).ok_or("Page bitmap is truncated")?;
            let dst = row * dst_stride;
            pixels
                .get_mut(dst..dst + row_bytes)
                .ok_or("Page bitmap is too large")?
                .copy_from_slice(row_px);
        }
    } else {
        let (b_mask, g_mask, r_mask) =
            rgb_masks(data, header_size, compression, info.pixels_offset)?;
        let b_shift = mask_shift(b_mask)?;
        let g_shift = mask_shift(g_mask)?;
        let r_shift = mask_shift(r_mask)?;
        for row in 0..rows {
            let src = info
                .pixels_offset
                .checked_add(row * src_stride)
                .ok_or("Page bitmap is truncated")?;
            for x in 0..info.width as usize {
                let i = src.checked_add(x * 4).ok_or("Page bitmap is truncated")?;
                let px_bytes = data.get(i..i + 4).ok_or("Page bitmap is truncated")?;
                let mut raw = [0u8; 4];
                raw.copy_from_slice(px_bytes);
                let px = u32::from_le_bytes(raw);
                let o = row * dst_stride + x * 3;
                let dest = pixels.get_mut(o..o + 3).ok_or("Page bitmap is too large")?;
                dest[0] = ((px & b_mask) >> b_shift) as u8;
                dest[1] = ((px & g_mask) >> g_shift) as u8;
                dest[2] = ((px & r_mask) >> r_shift) as u8;
            }
        }
    }

    Ok(GdiDib {
        width: info.width,
        height: info.height,
        pixels,
    })
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or("Page bitmap is truncated")?;
    let mut raw = [0u8; 4];
    raw.copy_from_slice(bytes);
    Ok(u32::from_le_bytes(raw))
}

/// Blue, green, and red masks. `BI_RGB` 32 bpp is B, G, R in the low three bytes.
/// `BI_BITFIELDS` stores red, green, blue masks after the info header (or inside a V4/V5 header).
fn rgb_masks(
    data: &[u8],
    header_size: u32,
    compression: u32,
    pixels_offset: usize,
) -> Result<(u32, u32, u32), String> {
    if compression == 0 {
        return Ok((0x0000_00FF, 0x0000_FF00, 0x00FF_0000));
    }
    if compression != 3 {
        return Err(format!("Unsupported page bitmap compression {compression}"));
    }
    let mask_at = if header_size >= 108 {
        14 + 40
    } else {
        14 + header_size as usize
    };
    if pixels_offset < mask_at + 12 {
        return Err("Page bitmap color masks overlap the pixels".into());
    }
    let red = read_u32(data, mask_at)?;
    let green = read_u32(data, mask_at + 4)?;
    let blue = read_u32(data, mask_at + 8)?;
    Ok((blue, green, red))
}

fn mask_shift(mask: u32) -> Result<u32, String> {
    let shift = mask.trailing_zeros();
    if mask == 0 || (mask >> shift) != 0xFF {
        return Err(format!("Unsupported page bitmap channel mask {mask:#x}"));
    }
    Ok(shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmp(width: i32, height: i32, bpp: u16, compression: u32) -> Vec<u8> {
        let stride = (width as usize * bpp as usize).div_ceil(32) * 4;
        let offset = 54usize;
        let mut d = vec![0u8; offset + stride * height.unsigned_abs() as usize];
        d[0] = b'B';
        d[1] = b'M';
        d[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
        d[14..18].copy_from_slice(&40u32.to_le_bytes());
        d[18..22].copy_from_slice(&width.to_le_bytes());
        d[22..26].copy_from_slice(&height.to_le_bytes());
        d[28..30].copy_from_slice(&bpp.to_le_bytes());
        d[30..34].copy_from_slice(&compression.to_le_bytes());
        d
    }

    #[test]
    fn parses_wic_style_bitmaps() {
        assert_eq!(
            parse_bmp(&bmp(2550, 3300, 32, 3)).unwrap(),
            BmpInfo {
                width: 2550,
                height: 3300,
                bpp: 32,
                pixels_offset: 54
            }
        );
        assert_eq!(parse_bmp(&bmp(3, -2, 24, 0)).unwrap().height, -2);
    }

    #[test]
    fn rejects_bad_bitmaps() {
        assert!(parse_bmp(b"nope").is_err());
        assert!(parse_bmp(&bmp(10, 10, 8, 0)).is_err());
        let mut short = bmp(100, 100, 32, 0);
        short.truncate(1000);
        assert!(parse_bmp(&short).is_err());
    }

    /// Letter paper at 600 dpi with 1/4" unprintable margins.
    fn letter_600() -> Caps {
        Caps {
            dpi_x: 600,
            dpi_y: 600,
            printable_w: 4800,
            printable_h: 6300,
            physical_w: 5100,
            physical_h: 6600,
            offset_x: 150,
            offset_y: 150,
        }
    }

    #[test]
    fn letter_page_is_shrunk_just_enough_and_centred() {
        let (x, y, w, h) = place_page(8.5, 11.0, &letter_600());
        assert!(w <= 4800 && h <= 6300);
        assert_eq!(w, 4800); // width is the limiting side (94%)
        assert!((w as f64 / h as f64 - 8.5 / 11.0).abs() < 0.001);
        assert_eq!(x, 0);
        // centred vertically on the physical sheet
        assert_eq!(y + 150, (6600 - h) / 2);
    }

    #[test]
    fn small_page_prints_at_actual_size() {
        let (x, y, w, h) = place_page(4.0, 6.0, &letter_600());
        assert_eq!((w, h), (2400, 3600));
        assert_eq!((x + 150, y + 150), ((5100 - 2400) / 2, (6600 - 3600) / 2));
    }

    #[test]
    fn stretch_failure_includes_gdi_error() {
        assert!(stretch_dibits_failed(0));
        assert!(stretch_dibits_failed(-1));
        assert!(!stretch_dibits_failed(1));
        assert!(!stretch_dibits_failed(3300));
    }

    #[test]
    fn printer_enum_count_rejects_a_count_past_the_buffer() {
        assert_eq!(printer_enum_count(32, 2, 16).unwrap(), 2);
        assert_eq!(printer_enum_count(16, 0, 16).unwrap(), 0);
        assert!(printer_enum_count(32, 3, 16).is_err());
        assert!(printer_enum_count(15, 1, 16).is_err());
        assert!(printer_enum_count(MAX_PRINTER_ENUM_BYTES + 1, 0, 16).is_err());
    }

    #[test]
    fn failed_orientation_reset_is_not_stored() {
        assert_eq!(apply_orientation(false, false, false).unwrap(), false);
        assert_eq!(apply_orientation(true, true, false).unwrap(), true);
        assert_eq!(apply_orientation(false, true, true).unwrap(), true);
        assert!(apply_orientation(false, true, false).is_err());
        assert!(apply_orientation(true, false, false).is_err());
    }

    #[test]
    fn thirty_two_bpp_keeps_color_when_alpha_is_zero() {
        // B, G, R, A. Alpha 0 must not blank the pixel.
        let mut file = bmp(2, 1, 32, 0);
        let px = file.len() - 8;
        file[px..px + 8].copy_from_slice(&[10, 20, 30, 0, 1, 2, 3, 0]);
        let dib = gdi_compatible_dib(&file).unwrap();
        assert_eq!(dib.height, 1);
        assert_eq!(&dib.pixels[..6], &[10, 20, 30, 1, 2, 3]);
        assert_eq!(dib.pixels.len(), 8); // 6 color bytes, padded to a DWORD
    }

    #[test]
    fn bitfields_masks_are_honored() {
        // Red in the low byte, unlike the usual BGRA layout.
        let width = 1i32;
        let stride = 4usize;
        let offset = 66usize;
        let mut file = vec![0u8; offset + stride];
        file[0] = b'B';
        file[1] = b'M';
        file[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
        file[14..18].copy_from_slice(&40u32.to_le_bytes());
        file[18..22].copy_from_slice(&width.to_le_bytes());
        file[22..26].copy_from_slice(&1i32.to_le_bytes());
        file[28..30].copy_from_slice(&32u16.to_le_bytes());
        file[30..34].copy_from_slice(&3u32.to_le_bytes());
        file[54..58].copy_from_slice(&0x0000_00FFu32.to_le_bytes()); // red
        file[58..62].copy_from_slice(&0x0000_FF00u32.to_le_bytes()); // green
        file[62..66].copy_from_slice(&0x00FF_0000u32.to_le_bytes()); // blue
        file[offset] = 255; // red channel, alpha in the high byte is 0
        let dib = gdi_compatible_dib(&file).unwrap();
        assert_eq!(&dib.pixels[..3], &[0, 0, 255]);
    }
}
