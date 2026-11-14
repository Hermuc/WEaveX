//! ZIP extraction shared by the .NET runtime and plugin installers.
//!
//! Both used to carry their own copy of this walk: `runtime` needed the plain
//! case, `plugin_manager` also had to strip a leading directory because the
//! plugin archives nest everything under a version folder. Keeping one
//! implementation means the Unix-permission handling and the directory-entry
//! detection cannot drift apart between the two installers.

use std::path::Path;

/// Extract `zip_path` into `target_dir`.
///
/// When `strip_prefix` is set, entries are written relative to that leading
/// path: `Release/net9.0/foo.dll` with prefix `Release/net9.0/` becomes
/// `foo.dll`. Entries that are parents of the prefix (`Release/` itself) are
/// skipped rather than created.
pub fn extract_zip(
    zip_path: &Path,
    target_dir: &Path,
    strip_prefix: Option<&str>,
) -> anyhow::Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let raw_path = file
            .enclosed_name()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        // Strip the leading directory when the archive nests everything.
        let rel_path = if let Some(prefix) = strip_prefix {
            match raw_path.strip_prefix(prefix) {
                Ok(stripped) => stripped.to_path_buf(),
                Err(_) => {
                    // A parent directory of the prefix (e.g. "Release/" for
                    // prefix "Release/net9.0/") has nothing to extract.
                    continue;
                }
            }
        } else {
            raw_path
        };

        // The prefix directory entry itself strips down to an empty path.
        if rel_path.as_os_str().is_empty() {
            continue;
        }

        let outpath = target_dir.join(&rel_path);

        if file.name().ends_with('/') || rel_path.to_string_lossy().ends_with('/') {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = file.unix_mode() {
                std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a zip in memory with the given (path, contents) entries.
    fn make_zip(dir: &Path, entries: &[(&str, &str)]) -> std::path::PathBuf {
        let path = dir.join("test.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn extracts_entries_into_the_target_directory() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let zip = make_zip(dir.path(), &[("a.txt", "alpha"), ("sub/b.txt", "beta")]);

        extract_zip(&zip, &out, None).unwrap();

        assert_eq!(std::fs::read_to_string(out.join("a.txt")).unwrap(), "alpha");
        assert_eq!(
            std::fs::read_to_string(out.join("sub").join("b.txt")).unwrap(),
            "beta"
        );
    }

    #[test]
    fn strip_prefix_flattens_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let zip = make_zip(
            dir.path(),
            &[
                ("Release/net9.0/app.dll", "dll"),
                ("Release/net9.0/sub/x.txt", "x"),
                ("Other/ignored.txt", "no"),
            ],
        );

        extract_zip(&zip, &out, Some("Release/net9.0/")).unwrap();

        assert_eq!(std::fs::read_to_string(out.join("app.dll")).unwrap(), "dll");
        assert_eq!(
            std::fs::read_to_string(out.join("sub").join("x.txt")).unwrap(),
            "x"
        );
        // Entries outside the prefix are skipped, not written elsewhere.
        assert!(!out.join("Other").exists());
    }
}