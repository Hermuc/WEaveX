//! Thin helpers around the wallpaper metadata section of the config file.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::config::MetadataService;
use crate::we_client::WallpaperEngineClient;
use crate::workshop::WorkshopClient;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WallpaperMetadata {
    pub pubfileid: String,
    pub title: String,
    pub description: String,
    pub preview_url: String,
    pub author: String,
    pub author_url: String,
    pub file_size: String,
    pub posted_date: String,
    pub updated_date: String,
    pub rating_star_file: String,
    pub num_ratings: String,
    pub tags: serde_json::Value,
    pub collections: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetadataInitProgress {
    pub current: u32,
    pub total: u32,
}

/// Iterate over installed wallpapers and, for any that don't yet have
/// non-trivial metadata cached in the config file, fetch the workshop
/// page and persist the parsed metadata. Mirrors the original Python
/// `MetadataBatchInitializer`.
pub async fn batch_initialize_metadata(
    we_client: &Arc<RwLock<WallpaperEngineClient>>,
    metadata: &Arc<RwLock<MetadataService>>,
    workshop: &Arc<WorkshopClient>,
    app_handle: &AppHandle,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<u32> {
    let installed = we_client.read().installed_wallpapers();
    let cached = metadata.read().get_all();

    let pending: Vec<String> = installed
        .iter()
        .map(|w| w.pubfileid.clone())
        .filter(|id| !is_metadata_complete(&cached, id))
        .collect();

    let total = pending.len() as u32;
    let mut count: u32 = 0;

    cancel.store(false, std::sync::atomic::Ordering::SeqCst);

    // Accumulate results and flush once at the end.
    //
    // `set_item` rewrites the entire metadata.json per call, so feeding it
    // one wallpaper at a time during a 25-item batch meant 25 full
    // serializations plus 25 blocking fs writes — right while the UI is
    // trying to animate the progress dialog.
    let mut pending_writes: Vec<(String, Value)> = Vec::new();

    for (index, pubfileid) in pending.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            log::info!("Metadata initialization cancelled by user");
            break;
        }

        match workshop.item_details(pubfileid).await {
            Ok(item) => {
                let value = serde_json::to_value(WallpaperMetadata {
                    pubfileid: item.pubfileid.clone(),
                    title: item.title,
                    description: item.description,
                    preview_url: item.preview_url,
                    author: item.author,
                    author_url: item.author_url,
                    file_size: item.file_size,
                    posted_date: item.posted_date,
                    updated_date: item.updated_date,
                    rating_star_file: item.rating_star_file,
                    num_ratings: item.num_ratings,
                    tags: item.tags,
                    collections: serde_json::to_value(&item.collections)
                        .unwrap_or(Value::Array(Vec::new())),
                })
                .unwrap_or(Value::Null);
                if !value.is_null() {
                    pending_writes.push((item.pubfileid, value));
                    count += 1;
                }
            }
            Err(err) => {
                log::warn!("metadata batch: {pubfileid} fetch failed: {err:#}");
                // If this is a rate limit error, propagate it to stop the batch
                if err.to_string().contains("429") {
                    return Err(err);
                }
            }
        }

        // Emit progress event
        let current = (index + 1) as u32;
        let _ = app_handle.emit(
            "metadata-init-progress",
            MetadataInitProgress { current, total },
        );

        // Increased rate-limit delay to avoid 429 errors from Steam
        tokio::time::sleep(METADATA_RATE_LIMIT_DELAY).await;
    }

    // One rewrite for the whole batch instead of one per wallpaper.
    metadata.read().set_many(pending_writes);
    Ok(count)
}

fn is_metadata_complete(cached: &Value, pubfileid: &str) -> bool {
    let Some(item) = cached.get(pubfileid).and_then(|v| v.as_object()) else {
        return false;
    };
    let title_ok = item
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let tags_ok = item
        .get("tags")
        .map(|v| {
            v.as_array().map(|a| !a.is_empty()).unwrap_or(false)
                || v.as_object().map(|o| !o.is_empty()).unwrap_or(false)
        })
        .unwrap_or(false);
    title_ok && tags_ok
}
// ── Timing constants ──────────────────────────────────────────
/// Delay between metadata requests to avoid Steam 429s.
const METADATA_RATE_LIMIT_DELAY: Duration = Duration::from_millis(800);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_is_metadata_complete() {
        let cached = json!({
            "123": {
                "title": "A Title",
                "tags": ["tag1"]
            },
            "456": {
                "title": "",
                "tags": []
            }
        });

        assert!(is_metadata_complete(&cached, "123"));
        assert!(!is_metadata_complete(&cached, "456"));
        assert!(!is_metadata_complete(&cached, "999"));
    }
}
