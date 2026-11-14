//! Wallpaper Engine control commands (play/pause/apply/...).
//!
//! Split out of `we_client.rs`: these are all thin `-control` wrappers
//! around `spawn_detached`, kept separate from the scanning and path-discovery
//! logic in the parent module.

use std::path::Path;
use std::process::Command;

use sysinfo::System;

use super::{spawn_detached, WallpaperEngineClient};

impl WallpaperEngineClient {
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
