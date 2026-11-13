//! Wallpaper Engine client. Full port of the Python
//! `wallpaper_engine_client.py`: detection, project enumeration, current
//! wallpaper lookup and the `-control openWallpaper / applyProperties`
//! command surface.

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

    pub fn is_installed(&self, pubfileid: &str) -> bool {
        self.projects_path()
            .map(|p| p.join(pubfileid).exists())
            .unwrap_or(false)
    }

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
        let scanned = self.scan_installed_wallpapers();
        if let Ok(mut guard) = self.installed_cache.lock() {
            *guard = Some(scanned.clone());
        }
        scanned
    }

    /// The uncached disk walk.
    fn scan_installed_wallpapers(&self) -> Vec<InstalledWallpaper> {
        let mut out: Vec<InstalledWallpaper> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (dir, source) in self.scan_directories() {
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

    pub fn get_config_key(&self, project_path: &Path) -> Option<String> {
        let is_workshop = project_path
            .to_string_lossy()
            .contains("workshop\\content\\431960")
            || project_path
                .to_string_lossy()
                .contains("workshop/content/431960");

        if is_workshop {
            return project_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string());
        }

        // For local projects, the key is the absolute path to the main file with forward slashes
        let json_path = project_path.join("project.json");
        if let Ok(raw) = std::fs::read_to_string(&json_path) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&raw) {
                let file = config
                    .get("file")
                    .and_then(|v| v.as_str())
                    .unwrap_or("scene.pkg");
                let main_file = project_path.join(file);
                let mut path_str = main_file.to_string_lossy().replace("\\", "/");
                // Remove UNC prefix if present
                if path_str.starts_with("//?/") {
                    path_str = path_str[4..].to_string();
                }
                return Some(path_str);
            }
        }

        None
    }

    pub fn apply(
        &self,
        project_path: &Path,
        monitor: Option<u32>,
        force: bool,
    ) -> anyhow::Result<()> {
        let executable = self
            .executable()
            .ok_or_else(|| anyhow::anyhow!("Wallpaper Engine executable not found"))?;

        let (folder, json_path) = if project_path.is_dir() {
            (
                project_path.to_path_buf(),
                project_path.join("project.json"),
            )
        } else {
            (
                project_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default(),
                project_path.to_path_buf(),
            )
        };

        if !force {
            if let Some(name) = folder.file_name().and_then(|n| n.to_str()) {
                if self
                    .current_wallpaper_pubfileid(monitor.unwrap_or(0))
                    .as_deref()
                    == Some(name)
                {
                    return Ok(());
                }
            }
        }

        let mut cmd = Command::new(executable.clone());
        if let Some(parent) = executable.parent() {
            cmd.current_dir(parent);
        }
        cmd.arg("-control")
            .arg("openWallpaper")
            .arg("-file")
            .arg(&json_path);
        if let Some(m) = monitor {
            cmd.arg("-monitor").arg(m.to_string());
        }
        spawn_detached(cmd)?;
        Ok(())
    }

    pub fn apply_properties(
        &self,
        properties: &serde_json::Map<String, serde_json::Value>,
        monitor: Option<u32>,
    ) -> anyhow::Result<()> {
        let executable = self
            .executable()
            .ok_or_else(|| anyhow::anyhow!("Wallpaper Engine executable not found"))?;

        let properties_json = serde_json::to_string(properties)?;

        let mut cmd = Command::new(executable.clone());
        if let Some(parent) = executable.parent() {
            cmd.current_dir(parent);
        }
        cmd.arg("-control").arg("applyProperties");
        if let Some(m) = monitor {
            cmd.arg("-monitor").arg(m.to_string());
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let raw_properties = format!("-properties RAW~({})~END", properties_json);
            cmd.raw_arg(raw_properties);
        }

        #[cfg(not(windows))]
        {
            cmd.arg("-properties")
                .arg(format!("RAW~({})~END", properties_json));
        }

        spawn_detached(cmd)?;
        Ok(())
    }

    pub fn save_properties(
        &self,
        json_path: &Path,
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> anyhow::Result<()> {
        let executable = self
            .executable()
            .ok_or_else(|| anyhow::anyhow!("Wallpaper Engine executable not found"))?;

        let properties_json = serde_json::to_string(properties)?;

        let mut cmd = Command::new(executable.clone());
        if let Some(parent) = executable.parent() {
            cmd.current_dir(parent);
        }
        cmd.arg("-control")
            .arg("openWallpaper")
            .arg("-file")
            .arg(json_path);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let raw_properties = format!("-properties RAW~({})~END", properties_json);
            cmd.raw_arg(raw_properties);
        }

        #[cfg(not(windows))]
        {
            cmd.arg("-properties")
                .arg(format!("RAW~({})~END", properties_json));
        }

        spawn_detached(cmd)?;
        Ok(())
    }

    pub fn send_control(&self, control: &str, monitor: Option<u32>) -> anyhow::Result<()> {
        let executable = self
            .executable()
            .ok_or_else(|| anyhow::anyhow!("Wallpaper Engine executable not found"))?;
        let mut cmd = Command::new(executable.clone());
        if let Some(parent) = executable.parent() {
            cmd.current_dir(parent);
        }
        cmd.arg("-control").arg(control);
        if let Some(m) = monitor {
            cmd.arg("-monitor").arg(m.to_string());
        }
        spawn_detached(cmd)?;
        Ok(())
    }

    pub fn play(&self, monitor: Option<u32>) -> anyhow::Result<()> {
        self.send_control("play", monitor)
    }
    pub fn pause(&self, monitor: Option<u32>) -> anyhow::Result<()> {
        self.send_control("pause", monitor)
    }
    pub fn stop(&self, monitor: Option<u32>) -> anyhow::Result<()> {
        self.send_control("stop", monitor)
    }
    pub fn mute(&self) -> anyhow::Result<()> {
        self.send_control("mute", None)
    }
    pub fn unmute(&self) -> anyhow::Result<()> {
        self.send_control("unmute", None)
    }
    pub fn next_wallpaper(&self, monitor: Option<u32>) -> anyhow::Result<()> {
        self.send_control("nextWallpaper", monitor)
    }
    pub fn previous_wallpaper(&self, monitor: Option<u32>) -> anyhow::Result<()> {
        self.send_control("previousWallpaper", monitor)
    }
    pub fn show_icons(&self) -> anyhow::Result<()> {
        self.send_control("showIcons", None)
    }
    pub fn hide_icons(&self) -> anyhow::Result<()> {
        self.send_control("hideIcons", None)
    }

    pub fn set_volume(&self, volume: u32) -> anyhow::Result<()> {
        let executable = self
            .executable()
            .ok_or_else(|| anyhow::anyhow!("Wallpaper Engine executable not found"))?;
        let mut cmd = Command::new(executable.clone());
        if let Some(parent) = executable.parent() {
            cmd.current_dir(parent);
        }
        cmd.arg("-control")
            .arg("setVolume")
            .arg("-volume")
            .arg(volume.to_string());
        spawn_detached(cmd)?;
        Ok(())
    }

    pub fn open_playlist(&self, playlist: &str, monitor: Option<u32>) -> anyhow::Result<()> {
        let executable = self
            .executable()
            .ok_or_else(|| anyhow::anyhow!("Wallpaper Engine executable not found"))?;
        let mut cmd = Command::new(executable.clone());
        if let Some(parent) = executable.parent() {
            cmd.current_dir(parent);
        }
        cmd.arg("-control")
            .arg("openPlaylist")
            .arg("-playlist")
            .arg(playlist);
        if let Some(m) = monitor {
            cmd.arg("-monitor").arg(m.to_string());
        }
        spawn_detached(cmd)?;
        Ok(())
    }

    pub fn open_profile(&self, profile: &str) -> anyhow::Result<()> {
        let executable = self
            .executable()
            .ok_or_else(|| anyhow::anyhow!("Wallpaper Engine executable not found"))?;
        let mut cmd = Command::new(executable.clone());
        if let Some(parent) = executable.parent() {
            cmd.current_dir(parent);
        }
        cmd.arg("-control")
            .arg("openProfile")
            .arg("-profile")
            .arg(profile);
        spawn_detached(cmd)?;
        Ok(())
    }

    pub fn open_wallpaper_engine(&self, show_window: bool) -> anyhow::Result<()> {
        let executable = self
            .executable()
            .ok_or_else(|| anyhow::anyhow!("Wallpaper Engine executable not found"))?;

        let name = executable
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        let mut system = System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        let is_running = system
            .processes()
            .iter()
            .any(|(_, p)| p.name().to_string_lossy().to_ascii_lowercase() == name);

        let mut cmd = Command::new(executable.clone());
        if let Some(parent) = executable.parent() {
            cmd.current_dir(parent);
        }
        if is_running && show_window {
            cmd.arg("-showwindow");
        }

        if !is_running || show_window {
            spawn_detached(cmd)?;
        }
        Ok(())
    }
}

fn parse_project(folder: &Path, pubfileid: &str) -> InstalledWallpaper {
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

/// Locate every Steam install on the machine. We look at default
/// directories first, then on Windows we read the `Software\Valve\Steam`
/// registry key.
fn steam_install_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let push_if_exists = |out: &mut Vec<PathBuf>, p: PathBuf| {
        if p.join("steamapps").exists() || p.join("Steam.exe").exists() {
            out.push(p);
        }
    };

    push_if_exists(&mut out, PathBuf::from(r"C:\Program Files (x86)\Steam"));
    push_if_exists(&mut out, PathBuf::from(r"C:\Program Files\Steam"));
    if let Ok(p) = std::env::var("ProgramFiles(x86)") {
        push_if_exists(&mut out, PathBuf::from(p).join("Steam"));
    }
    if let Ok(p) = std::env::var("ProgramFiles") {
        push_if_exists(&mut out, PathBuf::from(p).join("Steam"));
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(p) = read_steam_install_path_from_registry() {
            push_if_exists(&mut out, p);
        }
    }

    out
}

#[cfg(target_os = "windows")]
fn read_steam_install_path_from_registry() -> Option<PathBuf> {
    // Avoid an extra `winreg` dep — shell out to `reg query`. Output is
    // a few lines like:
    //     HKEY_CURRENT_USER\Software\Valve\Steam
    //         SteamPath    REG_SZ    C:\Program Files (x86)\Steam
    let try_query = |hive: &str| -> Option<PathBuf> {
        let out = std::process::Command::new("reg")
            .args(["query", hive, "/v", "SteamPath"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("SteamPath") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix("REG_SZ") {
                    let p = rest.trim();
                    if !p.is_empty() {
                        return Some(PathBuf::from(p.replace('/', "\\")));
                    }
                }
            }
        }
        None
    };
    try_query(r"HKCU\Software\Valve\Steam")
        .or_else(|| try_query(r"HKLM\SOFTWARE\WOW6432Node\Valve\Steam"))
        .or_else(|| try_query(r"HKLM\SOFTWARE\Valve\Steam"))
}

/// Parse `<steam>/steamapps/libraryfolders.vdf`. Returns every library
/// path Steam knows about (the file is a Valve KeyValues format —
/// quoted-string pairs we can extract with a regex).
/// Wallpaper Engine's Steam app id. Workshop content for it lives under
/// <library>/steamapps/workshop/content/431960/.
const WE_STEAM_APP_ID: &str = "431960";

/// Every `steamapps/workshop/content/431960` directory on this machine.
///
/// Steam owns this tree: subscriptions, updates and removals all happen here,
/// which is exactly why WEave never writes to it. We only read it so the
/// Installed view reflects what the user actually has.
fn workshop_content_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut libraries: Vec<PathBuf> = Vec::new();
    for steam_root in steam_install_dirs() {
        libraries.push(steam_root.clone());
        libraries.extend(steam_library_folders(&steam_root));
    }

    for lib in libraries {
        let dir = lib
            .join("steamapps")
            .join("workshop")
            .join("content")
            .join(WE_STEAM_APP_ID);
        if !dir.is_dir() {
            continue;
        }
        let key = dir.to_string_lossy().to_lowercase();
        if seen.insert(key) {
            out.push(dir);
        }
    }

    out
}

fn steam_library_folders(steam_root: &Path) -> Vec<PathBuf> {
    let vdf = steam_root.join("steamapps").join("libraryfolders.vdf");
    let Ok(text) = std::fs::read_to_string(&vdf) else {
        return Vec::new();
    };
    let re = regex::Regex::new(r#""path"\s*"([^"]+)""#).unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(&text) {
        // Steam writes `\\` for backslashes — collapse them.
        let path = cap[1].replace(r"\\", r"\");
        out.push(PathBuf::from(path));
    }
    out
}

fn walk_size(path: &Path, total: &mut u64) {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_size(&p, total);
            } else if let Ok(meta) = entry.metadata() {
                *total = total.saturating_add(meta.len());
            }
        }
    }
}

#[cfg(windows)]
fn spawn_detached(mut cmd: Command) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW).spawn()?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_detached(mut cmd: Command) -> anyhow::Result<()> {
    cmd.spawn()?;
    Ok(())
}

pub fn delete_wallpaper_folder(projects_path: &Path, pubfileid: &str) -> anyhow::Result<()> {
    let target = projects_path.join(pubfileid);
    if target.exists() {
        std::fs::remove_dir_all(target)?;
    }
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

    #[test]
    fn test_delete_wallpaper_folder() {
        let dir = tempdir().unwrap();
        let pubfileid = "12345";
        let target = dir.path().join(pubfileid);
        fs::create_dir(&target).unwrap();
        assert!(target.exists());

        delete_wallpaper_folder(dir.path(), pubfileid).unwrap();
        assert!(!target.exists());
    }

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
}
