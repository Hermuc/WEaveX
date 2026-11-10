//! Renders the Steam QR login challenge into a data URL.
//!
//! The login dialog used to point an `<img>` at
//! `https://api.qrserver.com/v1/create-qr-code/?data=...`. That failed for two
//! independent reasons:
//!   1. the app's CSP `img-src` whitelist does not include that host, so
//!      WebView2 blocked the request and the element rendered as a broken image;
//!   2. even with CSP fixed it would depend on a third-party service that is
//!      frequently unreachable from mainland China.
//!
//! Rendering locally with the `qrcode` crate removes both problems and keeps the
//! app green/portable (no network, no external tool).

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// Encode `data` as a QR code and return a `data:image/png;base64,...` URL.
///
/// The payload is rendered as a small module grid and expanded into an RGBA
/// buffer by hand, so no image-encoding dependency is required beyond the PNG
/// writer that Tauri already pulls in.
pub fn render_qr_data_url(data: &str) -> Result<String, String> {
    use qrcode::QrCode;

    let code = QrCode::new(data.as_bytes()).map_err(|e| e.to_string())?;
    let width = code.width();
    let colors = code.to_colors();

    // Draw at SCALE device pixels per module plus a quiet zone, which is what
    // scanners need to lock on reliably.
    const SCALE: usize = 4;
    const QUIET: usize = 4;
    let side = (width + QUIET * 2) * SCALE;

    let mut rgba = vec![255u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let mx = x / SCALE;
            let my = y / SCALE;
            if mx < QUIET || my < QUIET || mx >= QUIET + width || my >= QUIET + width {
                continue; // quiet zone stays white
            }
            let module = colors[(my - QUIET) * width + (mx - QUIET)];
            if module == qrcode::Color::Dark {
                let idx = (y * side + x) * 4;
                rgba[idx] = 0;
                rgba[idx + 1] = 0;
                rgba[idx + 2] = 0;
                rgba[idx + 3] = 255;
            }
        }
    }

    let png = encode_png(side as u32, side as u32, &rgba)?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

/// Minimal RGBA -> PNG encoder (no filtering, single IDAT chunk).
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    // Raw scanlines: each row prefixed with filter type 0.
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0u8);
        let start = y * width as usize * 4;
        raw.extend_from_slice(&rgba[start..start + width as usize * 4]);
    }

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    // IHDR
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, filter, no interlace
    push_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT (stored/deflate via miniz-like path is overkill: use zlib "stored" blocks)
    let compressed = zlib_stored(&raw);
    push_chunk(&mut out, b"IDAT", &compressed);

    // IEND
    push_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// zlib stream using uncompressed ("stored") deflate blocks. Valid PNG, and
/// avoids pulling in a compression crate for a few KB of QR bitmap.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // CMF/FLG: deflate, 32K window, no dict
    let mut offset = 0;
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    }
    while offset < data.len() {
        let chunk = (data.len() - offset).min(65_535);
        let last = offset + chunk >= data.len();
        out.push(if last { 0x01 } else { 0x00 });
        out.extend_from_slice(&(chunk as u16).to_le_bytes());
        out.extend_from_slice(&(!(chunk as u16)).to_le_bytes());
        out.extend_from_slice(&data[offset..offset + chunk]);
        offset += chunk;
    }
    // Adler-32 of the uncompressed data
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_valid_png_data_url() {
        let url = render_qr_data_url("https://example.com/test-challenge").unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        let raw = STANDARD
            .decode(url.trim_start_matches("data:image/png;base64,"))
            .unwrap();
        assert_eq!(&raw[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert!(raw.len() > 500, "png looks too small: {}", raw.len());
    }
}
