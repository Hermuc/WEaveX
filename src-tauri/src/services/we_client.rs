//! Wallpaper Engine client. Full port of the Python
//! `wallpaper_engine_client.py`: detection, project enumeration, current
//! wallpaper lookup and the `-control openWallpaper / applyProperties`
//! command surface.

mod control;
mod scan;
mod steam_dirs;

use steam_dirs::{steam_install_dirs, steam_library_folders};

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sysinfo::System;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledWallpaper {
    pub pubfileid: String,
    pub folder: String,
    pub project_json_path: String,
    pub has_pkg: bool,
    pub title: String,
    pub preview: String,
    pub description: String,
    pub file_type: String,
    pub tags: Vec<String>,
    /// Steam content rating from project.json: "Everyone" / "Questionable" /
    /// "Mature". Empty when the project does not declare one.
    pub content_rating: String,
    pub size_bytes: u64,
    pub installed_ts: i64,
    /// Where this wallpaper was found. See the SOURCE_ constants below.
    pub source: String,
}

/// A project WEave downloaded itself into projects/myprojects.
pub const SOURCE_MY_PROJECTS: &str = "myprojects";
/// One of Wallpaper Engine's bundled sample projects.
pub const SOURCE_DEFAULT_PROJECTS: &str = "defaultprojects";
/// A Steam Workshop subscription, owned and updated by the Steam client.
pub const SOURCE_WORKSHOP: &str = "workshop";

pub struct WallpaperEngineClient {
    pub directory: Option<PathBuf>,
    /// Cached result of `installed_wallpapers()`.
    ///
    /// The scan walks three directories and parses a project.json per entry.
    /// Doing that on every view switch was measurable (and, combined with the
    /// lock contention it causes, made the window stutter). The result only
    /// changes when a wallpaper is added, deleted or re-applied, and every such
    /// path calls `invalidate_installed()` explicitly.
    installed_cache: std::sync::Mutex<Option<Vec<InstalledWallpaper>>>,
}

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
    fn selected_wallpapers(
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

pub(super) fn parse_project(folder: &Path, pubfileid: &str) -> InstalledWallpaper {
    let project_json_path = folder.join("project.json");
    let mut title = pubfileid.to_string();
    let mut preview = String::new();
    let mut description = String::new();
    let mut file_type = String::new();
    let mut content_rating = String::new();
    let mut tags: Vec<String> = Vec::new();

    if let Ok(raw) = std::fs::read_to_string(&project_json_path) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(s) = value.get("title").and_then(|v| v.as_str()) {
                title = s.to_string();
            }
            if let Some(s) = value.get("preview").and_then(|v| v.as_str()) {
                preview = folder.join(s).to_string_lossy().to_string();
            }
            if let Some(s) = value.get("description").and_then(|v| v.as_str()) {
                description = s.to_string();
            }
            if let Some(s) = value.get("contentrating").and_then(|v| v.as_str()) {
                content_rating = s.to_string();
            }
            if let Some(s) = value.get("type").and_then(|v| v.as_str()) {
                file_type = s.to_string();
            }
            if let Some(arr) = value.get("tags").and_then(|v| v.as_array()) {
                tags = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(ToString::to_string))
                    .collect();
            }
        }
    }

    let mut size_bytes: u64 = 0;
    walk_size(folder, &mut size_bytes);

    let installed_ts = std::fs::metadata(folder)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();

    let has_pkg = std::fs::read_dir(folder)
        .map(|iter| {
            iter.flatten().any(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("pkg"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    InstalledWallpaper {
        pubfileid: pubfileid.into(),
        folder: folder.to_string_lossy().to_string(),
        project_json_path: project_json_path.to_string_lossy().to_string(),
        has_pkg,
        title,
        preview,
        description,
        file_type,
        tags,
        content_rating,
        size_bytes,
        installed_ts,
        // Caller overwrites this with the directory it was found in.
        source: SOURCE_MY_PROJECTS.to_string(),
    }
}
/// against pathological trees (a project folder pointed at a huge nested
/// directory) turning a view switch into an unbounded walk. Depth is counted
/// from `path` itself, so 8 levels comfortably covers every real project.
fn walk_size(path: &Path, total: &mut u64) {
    walk_size_bounded(path, total, 0, 8);
}

fn walk_size_bounded(path: &Path, total: &mut u64, depth: usize, max_depth: usize) {
    if depth > max_depth {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_size_bounded(&p, total, depth + 1, max_depth);
            } else if let Ok(meta) = entry.metadata() {
                *total = total.saturating_add(meta.len());
            }
        }
    }
}
fn spawn_detached(mut cmd: Command) -> anyhow::Result<()> {
    crate::core::platform::hide_console_window(&mut cmd);
    cmd.spawn()?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_detached(mut cmd: Command) -> anyhow::Result<()> {
    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Regression: Wallpaper Engine nests the config under the Windows
    /// account name ("An1", "Main", …). The original code hard-coded
    /// `Main`, so on every other machine `selectedwallpapers` resolved to
    /// None, `isActive` stayed false, and the preset-settings button was
    /// permanently greyed out.

    #[test]
    fn selected_wallpapers_reads_the_per_user_key() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{
                "?installdirectory": "D:/Game/_Steam/steamapps/common/wallpaper_engine",
                "An1": {
                    "general": {
                        "wallpaperconfig": {
                            "selectedwallpapers": {
                                "Monitor0": { "file": "d:/game/x/2858284176/ingame.mp4" }
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let selected =
            WallpaperEngineClient::selected_wallpapers(&value).expect("per-user key must be found");
        assert_eq!(
            selected["Monitor0"]["file"].as_str(),
            Some("d:/game/x/2858284176/ingame.mp4")
        );
    }

    /// Some builds do spell the key `Main`; that must keep working.
    #[test]
    fn selected_wallpapers_still_accepts_main() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{
                "Main": {
                    "general": {
                        "wallpaperconfig": {
                            "selectedwallpapers": {
                                "Monitor0": { "file": "d:/game/y/111/scene.pkg" }
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let selected = WallpaperEngineClient::selected_wallpapers(&value)
            .expect("Main key must still be accepted");
        assert!(selected.contains_key("Monitor0"));
    }
    /// Regression: the Age Rating filter reads content_rating, so every scanned
    /// wallpaper must resolve to a known rating (project.json value, or the
    /// "Everyone" default applied to Wallpaper Engine's bundled samples).
    #[test]
    fn every_installed_wallpaper_has_a_content_rating() {
        let client = WallpaperEngineClient::new(WallpaperEngineClient::detect_installation());
        let unrated: Vec<String> = client
            .installed_wallpapers()
            .into_iter()
            .filter(|w| {
                !matches!(
                    w.content_rating.as_str(),
                    "Everyone" | "Questionable" | "Mature"
                )
            })
            .map(|w| format!("{} ({:?})", w.pubfileid, w.content_rating))
            .collect();
        assert!(
            unrated.is_empty(),
            "wallpapers without a usable rating: {unrated:?}"
        );
    }

    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Regression: the Installed view merges myprojects, defaultprojects and
    /// the Workshop directory, so delete must find the folder in whichever of
    /// them it actually lives. Looking only in myprojects made deletes silently
    /// no-op, and the wallpaper came back on refresh.
    #[test]
    fn find_wallpaper_dir_searches_every_scanned_directory() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // A wallpaper that only exists under defaultprojects.
        let default_projects = root.join("projects").join("defaultprojects");
        fs::create_dir_all(default_projects.join("98765")).unwrap();

        // And one under myprojects, which must still win over the others.
        let my_projects = root.join("projects").join("myprojects");
        fs::create_dir_all(my_projects.join("11111")).unwrap();

        let client = WallpaperEngineClient::new(Some(root.to_path_buf()));

        let found = client.find_wallpaper_dir("98765");
        assert!(found.is_some(), "defaultprojects entry must be found");
        let (folder, source) = found.unwrap();
        assert_eq!(source, SOURCE_DEFAULT_PROJECTS);
        assert!(folder.ends_with("98765"));

        let (_, source_my) = client.find_wallpaper_dir("11111").unwrap();
        assert_eq!(source_my, SOURCE_MY_PROJECTS);

        // Unknown ids must report "not found" instead of pretending success.
        assert!(client.find_wallpaper_dir("00000").is_none());
        assert!(client.find_wallpaper_dir("").is_none());
    }

    #[test]
    fn test_walk_size() {
        let dir = tempdir().unwrap();
        let file1 = dir.path().join("file1.txt");
        let file2 = dir.path().join("file2.txt");
        fs::write(&file1, "123").unwrap(); // 3 bytes
        fs::write(&file2, "12345").unwrap(); // 5 bytes

        let mut total = 0;
        walk_size(dir.path(), &mut total);
        assert_eq!(total, 8);
    }

    #[test]
    fn test_steam_library_folders() {
        let dir = tempdir().unwrap();
        let steamapps = dir.path().join("steamapps");
        fs::create_dir(&steamapps).unwrap();
        let vdf = steamapps.join("libraryfolders.vdf");
        fs::write(
            &vdf,
            r#"
        "libraryfolders"
        {
            "0"
            {
                "path" "C:\\Program Files (x86)\\Steam"
            }
            "1"
            {
                "path" "D:\\SteamLibrary"
            }
        }
        "#,
        )
        .unwrap();

        let folders = steam_library_folders(dir.path());
        assert_eq!(folders.len(), 2);
        assert_eq!(
            folders[0].to_str().unwrap(),
            "C:\\Program Files (x86)\\Steam"
        );
        assert_eq!(folders[1].to_str().unwrap(), "D:\\SteamLibrary");
    }

    #[test]
    fn test_parse_project() {
        let dir = tempdir().unwrap();
        let project_json = dir.path().join("project.json");
        fs::write(
            &project_json,
            r#"
        {
            "title": "Test Title",
            "preview": "preview.jpg",
            "description": "A test wallpaper",
            "type": "scene",
            "tags": ["Anime", "Cyberpunk"]
        }
        "#,
        )
        .unwrap();
        let pkg_file = dir.path().join("scene.pkg");
        fs::write(&pkg_file, "dummy").unwrap();

        let wallpaper = parse_project(dir.path(), "777");
        assert_eq!(wallpaper.pubfileid, "777");
        assert_eq!(wallpaper.title, "Test Title");
        assert_eq!(wallpaper.description, "A test wallpaper");
        assert_eq!(wallpaper.file_type, "scene");
        assert_eq!(
            wallpaper.tags,
            vec!["Anime".to_string(), "Cyberpunk".to_string()]
        );
        assert!(wallpaper.has_pkg);
        assert!(wallpaper.preview.contains("preview.jpg"));
    }

    /// `scan_installed_from` must be a plain associated function: if it took
    /// `&self`, a caller holding the client's RwLock would keep it held for the
    /// whole 30 GB walk, blocking the synchronous `we_*` commands on the main
    /// thread. This test simply calls it without ever building a client.
    #[test]
    fn scan_installed_from_needs_no_client() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let project = root.join("2858284176");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("project.json"),
            r#"{"title":"Scanned Without A Lock","type":"video"}"#,
        )
        .unwrap();

        let targets: Vec<(PathBuf, &'static str)> = vec![(root.to_path_buf(), SOURCE_MY_PROJECTS)];
        let scanned = WallpaperEngineClient::scan_installed_from(targets);

        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].pubfileid, "2858284176");
        assert_eq!(scanned[0].title, "Scanned Without A Lock");
        assert_eq!(scanned[0].source, SOURCE_MY_PROJECTS);
    }

    /// The depth cap must not change sizes for ordinary projects: a normal
    /// wallpaper is one or two levels deep and must still report its exact
    /// byte count, because the UI sorts and totals by `size_bytes`.
    #[test]
    fn walk_size_counts_a_normal_project_exactly() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.bin"), vec![0u8; 100]).unwrap();
        fs::write(root.join("sub").join("b.bin"), vec![0u8; 250]).unwrap();

        let mut total = 0u64;
        walk_size(root, &mut total);
        assert_eq!(total, 350);
    }

    /// Pathological nesting is cut off at the cap so a view switch cannot turn
    /// into an unbounded walk. Files past the cap are simply not counted.
    #[test]
    fn walk_size_stops_at_the_depth_cap() {
        let dir = tempdir().unwrap();
        let mut deep = dir.path().to_path_buf();
        for i in 0..12 {
            deep = deep.join(format!("d{i}"));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep.bin"), vec![0u8; 4096]).unwrap();
        fs::write(dir.path().join("shallow.bin"), vec![0u8; 7]).unwrap();

        let mut total = 0u64;
        walk_size(dir.path(), &mut total);
        // The shallow file is always counted; the 4 KiB file sitting 12 levels
        // down is past the 8-level cap and must be excluded.
        assert_eq!(total, 7);
    }
}
