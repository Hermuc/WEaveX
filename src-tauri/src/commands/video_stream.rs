//! Commands backing the "watch while downloading" feature.
//!
//! The expensive part (Steam login + manifest resolve) is done in the background
//! as soon as the user opens a wallpaper's details (`video_stream_prewarm`), and
//! the resolved video is cached. `video_stream_start` then only starts the local
//! range server and launches the player, so clicking "watch" is near-instant.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::State;

use crate::commands::AppStateHandle;
use crate::services::video_player;
use crate::services::video_server::{self, VideoServer};
use crate::services::video_stream::{self, ResolvedVideo};

type ReadyMap = std::collections::HashMap<u64, Arc<ResolvedVideo>>;

/// Live streams plus pre-resolved videos, keyed by workshop item id.
#[derive(Default)]
pub struct StreamRegistry {
    servers: Mutex<std::collections::HashMap<u64, VideoServer>>,
    ready: Mutex<ReadyMap>,
    /// Items currently being resolved in the background (dedupe prewarm).
    pending: Mutex<std::collections::HashSet<u64>>,
}

impl StreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Serialize)]
pub struct PlayerInfo {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct StreamStarted {
    pub item_id: u64,
    pub url: String,
    pub filename: String,
    pub total_size: u64,
    pub player: String,
}

/// Report the detected external player, if any.
#[tauri::command]
pub fn video_stream_player() -> Option<PlayerInfo> {
    video_player::detect().map(|p| PlayerInfo {
        name: p.kind.name().to_string(),
        path: p.path.to_string_lossy().to_string(),
    })
}

/// Resolve a video in the background and cache it, so a later "watch" click is
/// instant. Safe to call repeatedly; concurrent calls for the same item dedupe.
#[tauri::command]
pub async fn video_stream_prewarm(
    state: AppStateHandle<'_>,
    registry: State<'_, Arc<StreamRegistry>>,
    item_id: u64,
) -> Result<(), String> {
    if registry.ready.lock().contains_key(&item_id) {
        return Ok(());
    }
    {
        let mut pending = registry.pending.lock();
        if !pending.insert(item_id) {
            return Ok(()); // already resolving
        }
    }
    let reg = registry.inner().clone();
    let creds = state.accounts.preferred_parser_credentials();
    tauri::async_runtime::spawn(async move {
        let result = resolve_with_retry(&creds, item_id).await;
        reg.pending.lock().remove(&item_id);
        match result {
            Ok(video) => {
                // Browsing many wallpapers must not pin a resolved video (and
                // its manifest) per item forever. Keep only the most recent
                // few; the rest re-resolve from the on-disk layout cache.
                const MAX_READY: usize = 4;
                let mut ready = reg.ready.lock();
                if ready.len() >= MAX_READY {
                    if let Some(oldest) = ready.keys().next().copied() {
                        ready.remove(&oldest);
                    }
                }
                ready.insert(item_id, Arc::new(video));
                log::info!("[video-stream] prewarm ready for {item_id}");
            }
            Err(e) => log::warn!("[video-stream] prewarm failed for {item_id}: {e}"),
        }
    });
    Ok(())
}

async fn resolve_with_retry(
    creds: &crate::services::accounts::AccountCredentials,
    item_id: u64,
) -> anyhow::Result<ResolvedVideo> {
    if creds.username.is_empty() {
        return Err(anyhow::anyhow!("NO_ACCOUNT"));
    }
    let client = video_stream::shared_login(&creds.username, &creds.password).await?;
    match video_stream::resolve_video(&client, item_id).await {
        Ok(v) => Ok(v),
        Err(e) => {
            log::warn!("[video-stream] resolve on cached session failed ({e}); fresh login");
            video_stream::forget_session();
            let fresh = video_stream::login(&creds.username, &creds.password).await?;
            video_stream::resolve_video(&fresh, item_id).await
        }
    }
}

/// Start the range server and launch the player. Uses the prewarmed video when
/// available; otherwise resolves inline (first-ever click).
#[tauri::command]
pub async fn video_stream_start(
    state: AppStateHandle<'_>,
    registry: State<'_, Arc<StreamRegistry>>,
    item_id: u64,
) -> Result<StreamStarted, String> {
    let player = video_player::detect().ok_or_else(|| "NO_PLAYER".to_string())?;
    let creds = state.accounts.preferred_parser_credentials();
    if creds.username.is_empty() {
        return Err("NO_ACCOUNT".into());
    }

    // Stop an existing stream for this item first (idempotent re-click).
    {
        let mut servers = registry.servers.lock();
        if let Some(mut old) = servers.remove(&item_id) {
            old.stop();
        }
    }

    // Take the prewarmed video under a short-lived lock so the guard is dropped
    // before any await (parking_lot guards are not Send).
    let prewarmed = registry.ready.lock().remove(&item_id);
    let video = match prewarmed {
        Some(v) => v,
        None => Arc::new(
            resolve_with_retry(&creds, item_id)
                .await
                .map_err(|e| e.to_string())?,
        ),
    };

    let info = video.info.clone();
    let server = video_server::serve_arc(video)
        .await
        .map_err(|e| e.to_string())?;
    let url = server.url();
    video_player::launch(&player, &url).map_err(|e| format!("launch failed: {e}"))?;
    let player_name = player.kind.name().to_string();
    registry.servers.lock().insert(item_id, server);

    Ok(StreamStarted {
        item_id,
        url,
        filename: info.filename,
        total_size: info.total_size,
        player: player_name,
    })
}

/// Stop the stream for one item.
#[tauri::command]
pub fn video_stream_stop(registry: State<'_, Arc<StreamRegistry>>, item_id: u64) -> bool {
    let mut servers = registry.servers.lock();
    match servers.remove(&item_id) {
        Some(mut s) => {
            s.stop();
            true
        }
        None => false,
    }
}

/// Stop every running stream (used on shutdown).
#[tauri::command]
pub fn video_stream_stop_all(registry: State<'_, Arc<StreamRegistry>>) -> usize {
    let mut servers = registry.servers.lock();
    let n = servers.len();
    for (_, mut s) in servers.drain() {
        s.stop();
    }
    n
}
