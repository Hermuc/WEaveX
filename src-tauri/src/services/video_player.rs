//! External player discovery and launching.
//!
//! The streaming feature deliberately does not embed a player: video wallpapers
//! are handed to a real media player over a local HTTP range URL. This module
//! finds a player that supports range-based streaming (mpv preferred, VLC as a
//! fallback) and launches it detached from WEave.

use std::path::PathBuf;
use std::process::Command;

/// A player we know how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerKind {
    Mpv,
    Vlc,
}

impl PlayerKind {
    pub fn name(self) -> &'static str {
        match self {
            PlayerKind::Mpv => "mpv",
            PlayerKind::Vlc => "vlc",
        }
    }
}

/// A discovered player executable.
#[derive(Debug, Clone)]
pub struct Player {
    pub kind: PlayerKind,
    pub path: PathBuf,
}

/// Candidate executables to look for, in preference order.
fn candidates() -> Vec<(PlayerKind, &'static str)> {
    vec![(PlayerKind::Mpv, "mpv.exe"), (PlayerKind::Vlc, "vlc.exe")]
}

/// Directories to scan besides PATH.
fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // Scoop shims and common portable layouts are where these usually live.
    for base in [
        std::env::var_os("USERPROFILE").map(PathBuf::from),
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    {
        dirs.push(base.join("scoop").join("shims"));
        dirs.push(base.join("Scoop").join("shims"));
        dirs.push(base.join("Programs").join("mpv"));
        dirs.push(base.join("mpv"));
        dirs.push(base.join("VideoLAN").join("VLC"));
    }
    dirs
}

/// Find the best available player.
///
/// Order: PATH first (so a user-installed shim wins), then the extra dirs.
pub fn detect() -> Option<Player> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let path_dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();

    for (kind, exe) in candidates() {
        // PATH lookup.
        for dir in &path_dirs {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(Player {
                    kind,
                    path: candidate,
                });
            }
        }
        // Extra directories.
        for dir in extra_dirs() {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(Player {
                    kind,
                    path: candidate,
                });
            }
        }
    }
    None
}

/// Launch the player against `url`, detached from WEave.
///
/// The player is spawned with no console window and not waited on: it keeps
/// playing after WEave closes (the range server must therefore outlive this call
/// only as long as the player needs it).
pub fn launch(player: &Player, url: &str) -> std::io::Result<()> {
    let mut cmd = Command::new(&player.path);
    match player.kind {
        PlayerKind::Mpv => {
            // --force-window: show a window even though this is a stream.
            // --keep-open: do not exit at EOF (the file may still be growing).
            // --no-terminal: no console output.
            cmd.arg("--force-window=yes")
                .arg("--keep-open=yes")
                .arg("--no-terminal")
                .arg(url);
        }
        PlayerKind::Vlc => {
            cmd.arg("--no-video-title-show").arg(url);
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_finds_something_or_none() {
        // Can only assert it does not panic; presence depends on the machine.
        let p = detect();
        if let Some(p) = p {
            assert!(p.path.is_file(), "detected player must exist on disk");
        }
    }

    #[test]
    fn candidates_prefer_mpv() {
        let c = candidates();
        assert_eq!(c[0].0, PlayerKind::Mpv);
    }
}
