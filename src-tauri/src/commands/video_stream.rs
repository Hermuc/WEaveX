//! Commands backing the "watch while downloading" feature.
//!
//! Flow: `video_stream_probe` (is a player available?) → `video_stream_start`
//! (resolve the workshop item, start the local range server, launch the player)
//! → `video_stream_stop` (tear the server down).

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::State;

use crate::commands::AppStateHandle;
use crate::services::video_player;
use crate::services::video_server::{self, VideoServer};
use crate::services::video_stream;

/// Live streams, keyed by workshop item id, so a second click can stop one.
#[derive(Default)]
pub struct StreamRegistry {
    servers: Mutex<std::collections::HashMap<u64, VideoServer>>,
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

/// Resolve a workshop item, start a range server for its video, and hand the
/// URL to the external player.
#[tauri::command]
pub async fn video_stream_start(
    state: AppStateHandle<'_>,
    registry: State<'_, Arc<StreamRegistry>>,
    item_id: u64,
) -> Result<StreamStarted, String> {
    // A player is required; fail before doing any network work.
    let player = video_player::detect().ok_or_else(|| "NO_PLAYER".to_string())?;

    // Reuse the account the user already configured.
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

    let client = video_stream::login(&creds.username, &creds.password)
        .await
        .map_err(|e| e.to_string())?;

    let video = video_stream::resolve_video(&client, item_id)
        .await
        .map_err(|e| e.to_string())?;

    let info = video.info.clone();
    let server = video_server::serve(video)
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

/// Stop the stream for one item (the player keeps its buffer and exits on its own).
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
