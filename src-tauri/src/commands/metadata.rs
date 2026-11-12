use serde_json::Value;
use tauri::command;

use crate::commands::AppStateHandle;

/// Return every stored metadata entry.
///
/// Runs on a blocking worker: Tauri executes synchronous commands on the main
/// thread, and the UI thread must never be blocked — a blocked main thread
/// stops the window from repainting, which is what produced the translucent
/// rectangles during view switches (the compositor had nothing to show).
#[command]
pub async fn metadata_get_all(state: AppStateHandle<'_>) -> Result<Value, String> {
    let store = state.metadata.clone();
    tauri::async_runtime::spawn_blocking(move || store.read().get_all())
        .await
        .map_err(|e| e.to_string())
}

#[command]
pub fn metadata_get(state: AppStateHandle<'_>, pubfileid: String) -> Option<Value> {
    state.metadata.read().get_item(&pubfileid)
}

#[command]
pub fn metadata_save(state: AppStateHandle<'_>, pubfileid: String, data: Value) {
    state.metadata.read().set_item(&pubfileid, data);
}

#[command]
pub fn metadata_remove(state: AppStateHandle<'_>, pubfileid: String) {
    state.metadata.read().remove_item(&pubfileid);
}
