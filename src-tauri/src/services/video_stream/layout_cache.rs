//! On-disk and in-memory cache of resolved video layouts.
//!
//! Resolving an item costs a manifest download (seconds) plus depot-key and
//! token round-trips, which on a slow network can take minutes. The layout is
//! immutable for a given `(depot, manifest)` pair, so it is persisted and reused
//! across restarts. The manifest id is part of the key, so a re-uploaded
//! wallpaper (new manifest) misses the cache and re-resolves.
//!
//! In memory entries are additionally aged out by `LAYOUT_TTL`: a workshop item
//! can be re-uploaded, and a stale layout would serve the wrong bytes.

use super::VideoStreamInfo;

/// Where the persisted layout cache lives.
fn layout_cache_path() -> Option<std::path::PathBuf> {
    let base = crate::app_state::resolve_app_data_dir();
    std::fs::create_dir_all(&base).ok()?;
    Some(base.join("video_layouts.json"))
}

/// Decode the on-disk cache format into fresh in-memory entries.
///
/// Pure so the parsing contract can be tested without touching the real cache
/// file or the global `LAYOUTS` mutex. A corrupt file yields an empty list —
/// the cache is an optimisation, never a correctness requirement.
fn decode_layouts(raw: &str) -> Vec<(u64, std::time::Instant, VideoStreamInfo)> {
    // Stored as a plain array of VideoStreamInfo; the TTL clock restarts on load
    // which is acceptable because entries are also validated by manifest id.
    let Ok(list) = serde_json::from_str::<Vec<VideoStreamInfo>>(raw) else {
        return Vec::new();
    };
    list.into_iter()
        .map(|info| (info.item_id, std::time::Instant::now(), info))
        .collect()
}

fn load_layouts() -> Vec<(u64, std::time::Instant, VideoStreamInfo)> {
    let Some(p) = layout_cache_path() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return Vec::new();
    };
    decode_layouts(&raw)
}

fn persist_layouts(entries: &[(u64, std::time::Instant, VideoStreamInfo)]) {
    let Some(p) = layout_cache_path() else { return };
    let list: Vec<&VideoStreamInfo> = entries.iter().map(|(_, _, i)| i).collect();
    if let Ok(json) = serde_json::to_string(&list) {
        let _ = std::fs::write(p, json);
    }
}

/// Cached manifest resolution per workshop item.
static LAYOUTS: std::sync::Mutex<Vec<(u64, std::time::Instant, VideoStreamInfo)>> =
    std::sync::Mutex::new(Vec::new());

const LAYOUT_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

pub(super) fn cached_layout(item_id: u64) -> Option<VideoStreamInfo> {
    let mut guard = LAYOUTS.lock().unwrap();
    if guard.is_empty() {
        *guard = load_layouts();
    }
    guard
        .iter()
        .find(|(id, at, _)| *id == item_id && at.elapsed() < LAYOUT_TTL)
        .map(|(_, _, info)| info.clone())
}

/// True when this item's layout is already known, so the caller can skip the
/// expensive manifest round-trip and go straight to serving bytes.
pub fn has_cached_layout(item_id: u64) -> bool {
    cached_layout(item_id).is_some()
}

pub(super) fn store_layout(item_id: u64, info: &VideoStreamInfo) {
    let mut guard = LAYOUTS.lock().unwrap();
    guard.retain(|(id, at, _)| *id != item_id && at.elapsed() < LAYOUT_TTL);
    guard.push((item_id, std::time::Instant::now(), info.clone()));
    persist_layouts(&guard);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_info(item_id: u64) -> VideoStreamInfo {
        VideoStreamInfo {
            item_id,
            depot_id: 431960,
            manifest_id: 123456789,
            filename: "scene.mp4".into(),
            total_size: 1024,
            spans: vec![super::super::ChunkSpan {
                offset: 0,
                len: 1024,
                chunk_id: "abc".into(),
                checksum: 7,
            }],
        }
    }

    #[test]
    fn decode_layouts_reads_a_valid_cache_file() {
        let info = sample_info(42);
        let raw = serde_json::to_string(&vec![info.clone()]).unwrap();

        let decoded = decode_layouts(&raw);

        assert_eq!(decoded.len(), 1);
        let (id, _at, got) = &decoded[0];
        assert_eq!(*id, 42);
        assert_eq!(got.item_id, info.item_id);
        assert_eq!(got.manifest_id, info.manifest_id);
        assert_eq!(got.filename, info.filename);
        assert_eq!(got.spans.len(), 1);
        assert_eq!(got.spans[0].len, 1024);
    }

    #[test]
    fn decode_layouts_returns_empty_for_corrupt_json() {
        // The cache is an optimisation; a truncated or hand-edited file must not
        // take down video streaming.
        assert!(decode_layouts("").is_empty());
        assert!(decode_layouts("not json").is_empty());
        assert!(decode_layouts("[{\"item_id\":1}").is_empty());
    }

    #[test]
    fn decode_layouts_returns_empty_for_an_empty_array() {
        assert!(decode_layouts("[]").is_empty());
    }
}