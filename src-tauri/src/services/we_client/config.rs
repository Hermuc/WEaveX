//! Client construction, directory config and installation detection for
//! Wallpaper Engine.
//!
//! Split out of `we_client.rs`; these methods manage `self.directory` and
//! the executable path.

use std::path::PathBuf;

use sysinfo::System;

use super::steam_dirs::{steam_install_dirs, steam_library_folders};

use super::WallpaperEngineClient;


impl WallpaperEngineClient {
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            installed_cache: std::sync::Mutex::new(None),
        }
    }

    /// Drop the cached installed list so the next read rescans the disk.
    pub fn invalidate_installed(&self) {
        if let Ok(mut guard) = self.installed_cache.lock() {
            *guard = None;
        }
    }

    pub fn set_directory(&mut self, directory: Option<PathBuf>) {
        self.directory = directory;
        self.invalidate_installed();
    }

    pub fn config_path(&self) -> Option<PathBuf> {
        self.directory.as_ref().map(|d| d.join("config.json"))
    }

    /// Read `selectedwallpapers` out of Wallpaper Engine's config.json.
    ///
    /// The object is nested under a per-user key whose name is the Windows
    /// account that last saved the config (e.g. `"An1"`), not a fixed
    /// `"Main"`. Hard-coding `Main` made every lookup return `None` on any
    /// machine whose user is not literally called `Main`, which silently
    /// disabled the preset-settings button and the delete guard.
    ///
    /// Some builds also keep the block under a top-level `"Main"` key, so
    /// that spelling is still accepted.
    pub(super) fn selected_wallpapers(
        value: &serde_json::Value,
    ) -> Option<&serde_json::Map<String, serde_json::Value>> {
        let root = value.as_object()?;
        let mut candidates: Vec<&serde_json::Value> = Vec::new();
        if let Some(main) = root.get("Main") {
            candidates.push(main);
        }
        for (key, entry) in root {
            if key != "Main" && entry.get("general").is_some() {
                candidates.push(entry);
            }
        }
        candidates.into_iter().find_map(|entry| {
            entry
                .get("general")?
                .get("wallpaperconfig")?
                .get("selectedwallpapers")?
                .as_object()
        })
    }

    pub fn projects_path(&self) -> Option<PathBuf> {
        self.directory
            .as_ref()
            .map(|d| d.join("projects").join("myprojects"))
    }

    /// Best-effort detection of the Wallpaper Engine install. We look in,
    /// in order:
    ///   1. The legacy hard-coded `Program Files / Program Files (x86) /
    ///      Wallpaper Engine` paths (matches the original Python).
    ///   2. The default Steam install (`Steam/steamapps/common/wallpaper_engine`).
    ///   3. Every Steam library listed in `Steam/steamapps/libraryfolders.vdf` —
    ///      this is the common case because most users install Wallpaper
    ///      Engine on a separate drive (`D:\SteamLibrary`, etc.).
    ///   4. Currently running `wallpaper64.exe / wallpaper32.exe` processes.
    pub fn detect_installation() -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        candidates.push(PathBuf::from(r"C:\Program Files (x86)\Wallpaper Engine"));
        candidates.push(PathBuf::from(r"C:\Program Files\Wallpaper Engine"));
        if let Ok(p) = std::env::var("ProgramFiles") {
            candidates.push(PathBuf::from(p).join("Wallpaper Engine"));
        }
        if let Ok(p) = std::env::var("ProgramFiles(x86)") {
            candidates.push(PathBuf::from(p).join("Wallpaper Engine"));
        }

        // Resolve every Steam library and add the `wallpaper_engine` install
        // folder under it.
        for steam_root in steam_install_dirs() {
            candidates.push(
                steam_root
                    .join("steamapps")
                    .join("common")
                    .join("wallpaper_engine"),
            );
            for lib in steam_library_folders(&steam_root) {
                candidates.push(
                    lib.join("steamapps")
                        .join("common")
                        .join("wallpaper_engine"),
                );
            }
        }

        let mut seen = std::collections::HashSet::new();
        for candidate in &candidates {
            let key = candidate.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            if candidate.exists()
                && (candidate.join("wallpaper64.exe").exists()
                    || candidate.join("wallpaper32.exe").exists())
            {
                return Some(candidate.clone());
            }
        }

        let mut system = System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        for process in system.processes().values() {
            let name = process.name().to_string_lossy().to_lowercase();
            if name == "wallpaper64.exe" || name == "wallpaper32.exe" {
                if let Some(exe) = process.exe() {
                    if let Some(parent) = exe.parent() {
                        if parent.join("projects").exists() {
                            return Some(parent.to_path_buf());
                        }
                    }
                }
            }
        }

        None
    }

    pub fn executable(&self) -> Option<PathBuf> {
        let dir = self.directory.as_ref()?;
        let win64 = dir.join("wallpaper64.exe");
        if win64.exists() {
            return Some(win64);
        }
        let win32 = dir.join("wallpaper32.exe");
        if win32.exists() {
            return Some(win32);
        }
        None
    }

}
