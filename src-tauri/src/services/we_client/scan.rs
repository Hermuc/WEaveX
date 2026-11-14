//! Library scanning for Wallpaper Engine: enumerate installed wallpapers
//! from myprojects, defaultprojects and the Steam workshop folders.
//!
//! Split out of `we_client.rs`; these methods read `self.directory` and the
//! scan cache only.

use std::path::{Path, PathBuf};

use super::steam_dirs::workshop_content_dirs;
use super::{
    parse_project, InstalledWallpaper, WallpaperEngineClient, SOURCE_DEFAULT_PROJECTS,
    SOURCE_MY_PROJECTS, SOURCE_WORKSHOP,
};

impl WallpaperEngineClient {

    /// Every wallpaper Wallpaper Engine can actually play. That means three
    /// separate directories:
    ///
    ///   1. projects/myprojects   — what WEave downloads itself
    ///   2. projects/defaultprojects — the sample projects WE ships with
    ///   3. steamapps/workshop/content/431960 — Steam Workshop subscriptions
    ///
    /// The third one is the reason this exists: most users subscribe in the
    /// Steam client, so their wallpapers never touch myprojects and a
    /// myprojects-only scan shows an empty library. Each entry carries a
    /// `source` so the UI can tell them apart (deleting a workshop item is
    /// pointless — Steam re-downloads it).
    pub fn installed_wallpapers(&self) -> Vec<InstalledWallpaper> {
        if let Ok(guard) = self.installed_cache.lock() {
            if let Some(hit) = guard.as_ref() {
                return hit.clone();
            }
        }
        let scanned = Self::scan_installed_from(self.scan_directories());
        self.store_installed(scanned.clone());
        scanned
    }

    /// The directory list to scan, in priority order.
    ///
    /// This is the ONLY part of an installed scan that needs the client lock:
    /// it reads `self.directory`. Everything the actual scan does is disk I/O
    /// and must happen with no lock held — see `scan_installed_from`.
    pub fn scan_targets(&self) -> Vec<(PathBuf, &'static str)> {
        self.scan_directories()
    }

    /// Replace the cached scan result.
    ///
    /// Holds the cache mutex for a single assignment, never across I/O. The
    /// cache lives behind its own `std::sync::Mutex`, which is independent of
    /// the `RwLock` the rest of `AppState` uses, so this cannot contend with
    /// the synchronous `we_*` commands that run on the main thread.
    pub fn store_installed(&self, items: Vec<InstalledWallpaper>) {
        if let Ok(mut guard) = self.installed_cache.lock() {
            *guard = Some(items);
        }
    }

    /// Walk the given directories and build the installed list.
    ///
    /// Deliberately an associated function taking no `&self`: callers cannot
    /// accidentally hold the client's RwLock across a disk scan. The old shape
    /// (`let guard = client.read(); guard.installed_wallpapers()`) kept a read
    /// lock for the whole walk, which blocked the 14 synchronous `we_*`
    /// commands that run on the main thread — the window then could not repaint
    /// and DWM composited the transparent frame instead.
    pub fn scan_installed_from(targets: Vec<(PathBuf, &'static str)>) -> Vec<InstalledWallpaper> {
        let mut out: Vec<InstalledWallpaper> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (dir, source) in targets {
            if !dir.is_dir() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let folder = entry.path();
                if !folder.is_dir() {
                    continue;
                }
                let pubfileid = folder
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                if pubfileid.is_empty() {
                    continue;
                }
                // A project WEave downloaded can also be subscribed in Steam;
                // keep the first hit so myprojects wins over workshop.
                if !seen.insert(pubfileid.clone()) {
                    continue;
                }
                let mut wallpaper = parse_project(&folder, &pubfileid);
                wallpaper.source = source.to_string();
                // Wallpaper Engine ships its sample projects without a
                // contentrating field, but they are first-party samples and
                // always family-friendly. Without this they would only ever
                // appear under the "Any" age filter.
                if wallpaper.content_rating.is_empty() && source == SOURCE_DEFAULT_PROJECTS {
                    wallpaper.content_rating = "Everyone".to_string();
                }
                out.push(wallpaper);
            }
        }

        out
    }

    /// The directories to scan, in priority order, paired with the source tag
    /// recorded on every entry found there.
    fn scan_directories(&self) -> Vec<(PathBuf, &'static str)> {
        let mut dirs: Vec<(PathBuf, &'static str)> = Vec::new();

        if let Some(root) = &self.directory {
            dirs.push((root.join("projects").join("myprojects"), SOURCE_MY_PROJECTS));
            dirs.push((
                root.join("projects").join("defaultprojects"),
                SOURCE_DEFAULT_PROJECTS,
            ));
        }

        for workshop in workshop_content_dirs() {
            dirs.push((workshop, SOURCE_WORKSHOP));
        }

        dirs
    }

    /// Locate a wallpaper folder by pubfileid across every scanned directory,
    /// returning the folder and the source tag it was found under.
    ///
    /// The Installed view merges three directories, so a delete has to look in
    /// all of them too: looking only in myprojects silently did nothing for
    /// workshop and defaultprojects entries, which then reappeared on refresh.
    pub fn find_wallpaper_dir(&self, pubfileid: &str) -> Option<(PathBuf, &'static str)> {
        if pubfileid.is_empty() {
            return None;
        }
        for (dir, source) in self.scan_directories() {
            let candidate = dir.join(pubfileid);
            if candidate.is_dir() {
                return Some((candidate, source));
            }
        }
        None
    }

    pub fn current_wallpaper_pubfileid(&self, monitor: u32) -> Option<String> {
        let config_path = self.config_path()?;
        let raw = std::fs::read_to_string(&config_path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let selected = Self::selected_wallpapers(&value)?
            .get(&format!("Monitor{monitor}"))?
            .get("file")?
            .as_str()?
            .to_string();
        Path::new(&selected)
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
    }

    /// Enumerate every pubfileid currently attached to a monitor (Monitor0,
    /// Monitor1, …). Used to refuse deleting a wallpaper that Wallpaper
    /// Engine is still displaying right now.
    pub fn active_pubfileids(&self) -> Vec<String> {
        let Some(config_path) = self.config_path() else {
            return Vec::new();
        };
        let Ok(raw) = std::fs::read_to_string(&config_path) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Vec::new();
        };
        let Some(selected) = Self::selected_wallpapers(&value) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (_monitor, entry) in selected {
            let Some(file) = entry.get("file").and_then(|f| f.as_str()) else {
                continue;
            };
            if let Some(id) = Path::new(file)
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
            {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

}
