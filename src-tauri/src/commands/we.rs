use std::path::PathBuf;

use tauri::command;

use crate::commands::{map_err, AppStateHandle};
use crate::we_client::{InstalledWallpaper, WallpaperEngineClient};

#[command]
pub fn we_detect() -> Option<String> {
    WallpaperEngineClient::detect_installation().map(|p| p.to_string_lossy().to_string())
}

#[command]
pub fn we_get_directory(state: AppStateHandle<'_>) -> Option<String> {
    state
        .we_client
        .read()
        .directory
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
}

#[command]
pub fn we_set_directory(state: AppStateHandle<'_>, path: String) -> Result<(), String> {
    let dir = PathBuf::from(&path);
    if !dir.join("wallpaper64.exe").exists() && !dir.join("wallpaper32.exe").exists() {
        return Err("Not a Wallpaper Engine directory".into());
    }
    state.we_client.write().set_directory(Some(dir));
    state.settings.read().set_directory(&path);
    Ok(())
}

/// List installed wallpapers.
///
/// Scanning walks three directories and parses every project.json, which is
/// slow enough to freeze the UI when run on the main thread (Tauri executes
/// synchronous commands there, so the window cannot even repaint). The scan
/// therefore runs on a blocking worker and the result is returned asynchronously.
#[command]
pub async fn we_list_installed(
    state: AppStateHandle<'_>,
) -> Result<Vec<InstalledWallpaper>, String> {
    // Clone the Arc so the worker owns it; the RwLock guard must not cross an
    // await point.
    let client = state.we_client.clone();
    tauri::async_runtime::spawn_blocking(move || client.read().installed_wallpapers())
        .await
        .map_err(|e| e.to_string())
}

#[command]
pub fn we_apply(
    state: AppStateHandle<'_>,
    project_path: String,
    monitor: Option<u32>,
    force: Option<bool>,
) -> Result<(), String> {
    state
        .we_client
        .read()
        .apply(
            &PathBuf::from(project_path),
            monitor,
            force.unwrap_or(false),
        )
        .map_err(map_err)
}

#[command]
pub fn we_apply_properties(
    state: AppStateHandle<'_>,
    pubfileid: String,
    properties: serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let we = state.we_client.read();

    // Find all monitors where this wallpaper is active
    let mut applied_any = false;
    for i in 0..8 {
        if let Some(active_id) = we.current_wallpaper_pubfileid(i) {
            if active_id == pubfileid {
                let _ = we.apply_properties(&properties, Some(i));
                applied_any = true;
            }
        }
    }

    // Fallback if not currently active on any known monitor (though it won't be visible)
    if !applied_any {
        let _ = we.apply_properties(&properties, None);
    }

    Ok(())
}

#[command]
pub fn we_save_project_properties(
    project_json_path: String,
    properties: serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let json_path = std::path::PathBuf::from(&project_json_path);
    let backup_path = json_path.with_file_name("project_backup.json");

    // Create a backup if it doesn't exist
    if !backup_path.exists() && json_path.exists() {
        let _ = std::fs::copy(&json_path, &backup_path);
    }

    if let Ok(raw) = std::fs::read_to_string(&json_path) {
        if let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(general) = config.get_mut("general").and_then(|v| v.as_object_mut()) {
                if !general.contains_key("properties") {
                    general.insert("properties".to_string(), serde_json::json!({}));
                }

                if let Some(props) = general
                    .get_mut("properties")
                    .and_then(|v| v.as_object_mut())
                {
                    for (key, value) in properties {
                        if !props.contains_key(&key) {
                            props.insert(key, serde_json::json!({ "value": value }));
                        } else {
                            if let Some(prop_obj) =
                                props.get_mut(&key).and_then(|v| v.as_object_mut())
                            {
                                prop_obj.insert("value".to_string(), value);
                            }
                        }
                    }
                }
            }

            // Write it back
            let _ = std::fs::write(
                &json_path,
                serde_json::to_string_pretty(&config).unwrap_or_default(),
            );
            return Ok(());
        }
    }

    Err("Failed to save to project.json".to_string())
}

#[command]
pub fn we_restore_project_properties(project_json_path: String) -> Result<(), String> {
    let json_path = std::path::PathBuf::from(&project_json_path);
    let backup_path = json_path.with_file_name("project_backup.json");

    if backup_path.exists() {
        if let Err(e) = std::fs::copy(&backup_path, &json_path) {
            return Err(format!("Failed to restore backup: {}", e));
        }
        Ok(())
    } else {
        Err("No backup found".to_string())
    }
}

#[command]
pub fn we_open(state: AppStateHandle<'_>, show_window: Option<bool>) -> Result<(), String> {
    state
        .we_client
        .read()
        .open_wallpaper_engine(show_window.unwrap_or(true))
        .map_err(map_err)
}

#[command]
pub fn we_delete_wallpaper(state: AppStateHandle<'_>, pubfileid: String) -> Result<(), String> {
    // Refuse to delete a wallpaper that's currently being displayed on any
    // monitor — otherwise Wallpaper Engine keeps a file handle and the
    // delete fails with a confusing OS error halfway through. The caller
    // matches on the "ACTIVE:" prefix to show a friendly prompt.
    let active = state.we_client.read().active_pubfileids();
    if active.iter().any(|id| id == &pubfileid) {
        return Err(format!("ACTIVE:{pubfileid}"));
    }

    // The Installed view merges myprojects, defaultprojects and the Steam
    // Workshop directory, so the folder can live in any of them. Search all
    // three instead of assuming myprojects.
    let Some((folder, source)) = state.we_client.read().find_wallpaper_dir(&pubfileid) else {
        return Err(format!("NOTFOUND:{pubfileid}"));
    };

    // Workshop folders belong to the Steam client. Removing one just makes
    // Steam re-download it, and can leave the subscription bookkeeping
    // inconsistent, so refuse and let the caller explain.
    if source == crate::we_client::SOURCE_WORKSHOP {
        return Err(format!("WORKSHOP:{pubfileid}"));
    }

    state.metadata.read().remove_item(&pubfileid);
    remove_folder(&folder).map_err(map_err)
}

#[command]
pub fn we_current_pubfileid(state: AppStateHandle<'_>, monitor: u32) -> Option<String> {
    state.we_client.read().current_wallpaper_pubfileid(monitor)
}

#[command]
pub fn we_get_project_json(project_json_path: String) -> Result<serde_json::Value, String> {
    let raw = std::fs::read_to_string(&project_json_path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

#[command]
pub fn we_active_pubfileids(state: AppStateHandle<'_>) -> Vec<String> {
    state.we_client.read().active_pubfileids()
}

#[command]
pub fn we_play(state: AppStateHandle<'_>, monitor: Option<u32>) -> Result<(), String> {
    state.we_client.read().play(monitor).map_err(map_err)
}

#[command]
pub fn we_pause(state: AppStateHandle<'_>, monitor: Option<u32>) -> Result<(), String> {
    state.we_client.read().pause(monitor).map_err(map_err)
}

#[command]
pub fn we_stop(state: AppStateHandle<'_>, monitor: Option<u32>) -> Result<(), String> {
    state.we_client.read().stop(monitor).map_err(map_err)
}

#[command]
pub fn we_mute(state: AppStateHandle<'_>) -> Result<(), String> {
    state.we_client.read().mute().map_err(map_err)
}

#[command]
pub fn we_unmute(state: AppStateHandle<'_>) -> Result<(), String> {
    state.we_client.read().unmute().map_err(map_err)
}

#[command]
pub fn we_next_wallpaper(state: AppStateHandle<'_>, monitor: Option<u32>) -> Result<(), String> {
    state
        .we_client
        .read()
        .next_wallpaper(monitor)
        .map_err(map_err)
}

#[command]
pub fn we_previous_wallpaper(
    state: AppStateHandle<'_>,
    monitor: Option<u32>,
) -> Result<(), String> {
    state
        .we_client
        .read()
        .previous_wallpaper(monitor)
        .map_err(map_err)
}

#[command]
pub fn we_show_icons(state: AppStateHandle<'_>) -> Result<(), String> {
    state.we_client.read().show_icons().map_err(map_err)
}

#[command]
pub fn we_hide_icons(state: AppStateHandle<'_>) -> Result<(), String> {
    state.we_client.read().hide_icons().map_err(map_err)
}

#[command]
pub fn we_set_volume(state: AppStateHandle<'_>, volume: u32) -> Result<(), String> {
    state.we_client.read().set_volume(volume).map_err(map_err)
}

#[command]
pub fn we_open_playlist(
    state: AppStateHandle<'_>,
    playlist: String,
    monitor: Option<u32>,
) -> Result<(), String> {
    state
        .we_client
        .read()
        .open_playlist(&playlist, monitor)
        .map_err(map_err)
}

#[command]
pub fn we_open_profile(state: AppStateHandle<'_>, profile: String) -> Result<(), String> {
    state
        .we_client
        .read()
        .open_profile(&profile)
        .map_err(map_err)
}

/// Remove a wallpaper folder outright.
///
/// Separate from `delete_wallpaper_folder` because the caller already resolved
/// the absolute path: re-joining a pubfileid would only work for myprojects.
fn remove_folder(folder: &std::path::Path) -> anyhow::Result<()> {
    if folder.exists() {
        std::fs::remove_dir_all(folder)?;
    }
    Ok(())
}
