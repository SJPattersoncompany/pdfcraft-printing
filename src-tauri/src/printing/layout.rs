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
}
