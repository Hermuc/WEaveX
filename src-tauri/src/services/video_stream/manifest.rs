//! Manifest and chunk-id helpers for video streaming.
//!
//! Pure functions extracted from `video_stream.rs`: hex chunk-id parsing,
//! `is_video`, and unzipping a manifest body. None touch network or global
//! state, so each is directly testable.

use steamroom::depot::ChunkId;

pub(super) fn parse_chunk_id(hex: &str) -> anyhow::Result<ChunkId> {
    let bytes = hex.as_bytes();
    if bytes.len() != 40 {
        return Err(anyhow::anyhow!("bad chunk id length {}", bytes.len()));
    }
    let mut raw = [0u8; 20];
    for i in 0..20 {
        let hi = (bytes[i * 2] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("bad hex in chunk id"))?;
        let lo = (bytes[i * 2 + 1] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("bad hex in chunk id"))?;
        raw[i] = ((hi << 4) | lo) as u8;
    }
    Ok(ChunkId(raw))
}

const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mkv", "avi", "mov"];

pub(super) fn is_video(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIDEO_EXTS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// Manifest bytes from the CDN are zip-compressed; unwrap when needed.
pub(super) fn decompress_manifest(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    if data.len() > 2 && data[0] == 0x50 && data[1] == 0x4B {
        let cursor = std::io::Cursor::new(data);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| anyhow::anyhow!("manifest zip: {e}"))?;
        let mut file = archive
            .by_index(0)
            .map_err(|e| anyhow::anyhow!("manifest entry: {e}"))?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buf)?;
        Ok(buf)
    } else {
        Ok(data.to_vec())
    }
}
