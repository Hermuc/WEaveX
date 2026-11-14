//! Locating Steam and its Wallpaper Engine workshop content on disk.
//!
//! Everything here is filesystem / registry probing with no notion of what a
//! wallpaper is, which is why it is separate from the client that parses
//! projects. The client only asks this module for the directories to scan.

use std::path::{Path, PathBuf};

/// Wallpaper Engine's Steam app id. Workshop content lives under
/// `<library>/steamapps/workshop/content/431960/`.
const WE_STEAM_APP_ID: &str = "431960";

/// Locate every Steam install on the machine. We look at default
/// directories first, then on Windows we read the `Software\\Valve\\Steam`
/// registry key.
pub(super) fn steam_install_dirs() -> Vec<PathBuf> {
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
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    // Same lookup order the old `reg query` version used: the per-user install
    // first (Steam's own default), then the two machine-wide views.
    //
    // Reading the registry through the API instead of spawning `reg.exe`
    // matters: creating a process makes Windows take the loader lock, which
    // briefly suspends *every* thread in the parent — including the one
    // painting the window. That is the documented cause of the translucent
    // rectangle that flashed when the Installed view did its first disk scan.
    // `HKEY` is a raw handle (`*mut c_void`), not an integer. Spelling the
    // type out avoids depending on winreg's internal alias.
    const CANDIDATES: [(winreg::HKEY, &str); 3] = [
        (HKEY_CURRENT_USER, r"Software\Valve\Steam"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Valve\Steam"),
    ];

    for (hive, subkey) in CANDIDATES {
        let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(subkey, KEY_READ) else {
            continue;
        };
        let Ok(path) = key.get_value::<String, _>("SteamPath") else {
            continue;
        };
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed.replace('/', "\\")));
        }
    }
    None
}

/// Every `steamapps/workshop/content/431960` directory on this machine.
///
/// Steam owns this tree: subscriptions, updates and removals all happen here,
/// which is exactly why WEave never writes to it. We only read it so the
/// Installed view reflects what the user actually has.
pub(super) fn workshop_content_dirs() -> Vec<PathBuf> {
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

/// Parse `<steam>/steamapps/libraryfolders.vdf`. Returns every library
/// path Steam knows about (the file is a Valve KeyValues format —
/// quoted-string pairs we can extract with a regex).
pub(super) fn steam_library_folders(steam_root: &Path) -> Vec<PathBuf> {
    let vdf = steam_root.join("steamapps").join("libraryfolders.vdf");
    let Ok(text) = std::fs::read_to_string(&vdf) else {
        return Vec::new();
    };
    let re = regex::Regex::new(r#""path"\s*"([^"]+)""#).unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(&text) {
        // Steam writes `\\\\` for backslashes — collapse them.
        let path = cap[1].replace(r"\\", r"\");
        out.push(PathBuf::from(path));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The VDF is a hand-rolled regex parse; make sure a realistic file yields
    /// both library paths and that escaped backslashes are collapsed.
    #[test]
    fn parses_library_folders_from_a_vdf() {
        let dir = tempfile::tempdir().unwrap();
        let steamapps = dir.path().join("steamapps");
        std::fs::create_dir(&steamapps).unwrap();
        std::fs::write(
            steamapps.join("libraryfolders.vdf"),
            r#"
            "libraryfolders"
            {
                "0"
                {
                    "path"        "C:\\Program Files (x86)\\Steam"
                }
                "1"
                {
                    "path"        "D:\\SteamLibrary"
                }
            }
            "#,
        )
        .unwrap();

        let folders = steam_library_folders(dir.path());
        assert_eq!(folders.len(), 2);
        assert_eq!(
            folders[0].to_str().unwrap(),
            r"C:\Program Files (x86)\Steam"
        );
        assert_eq!(folders[1].to_str().unwrap(), r"D:\SteamLibrary");
    }
}
