//! Small filesystem predicates shared by the download and extract managers.
//!
//! Both used to carry their own copy of `dir_has_files`; keeping one copy
//! means "did the plugin actually produce output?" is answered the same way
//! in both places.

use std::path::Path;

/// True when `dir` exists and contains at least one regular file, at any
/// depth. Used to detect "the plugin actually produced output" even when the
/// process exit code is misleading.
pub fn dir_has_files(dir: &Path) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in read.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            return true;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) && dir_has_files(&entry.path()) {
            return true;
        }
    }
    false
}

/// True when the folder holds a real project (`project.json` present).
///
/// Used to decide whether a failed download left anything worth keeping: a
/// folder with only a partial DepotDownloader cache is unplayable and would
/// pollute the Installed view.
pub fn dir_has_project(dir: &Path) -> bool {
    dir.join("project.json").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn dir_has_files_sees_nested_files() {
        let dir = tempdir().unwrap();
        assert!(!dir_has_files(dir.path()));

        let subdir = dir.path().join("sub");
        fs::create_dir(&subdir).unwrap();
        assert!(!dir_has_files(dir.path()));

        fs::write(subdir.join("file.txt"), "test").unwrap();
        assert!(dir_has_files(dir.path()));
    }

    #[test]
    fn dir_has_project_requires_project_json() {
        let dir = tempdir().unwrap();
        assert!(!dir_has_project(dir.path()));
        fs::write(dir.path().join("project.json"), "{}").unwrap();
        assert!(dir_has_project(dir.path()));
    }
}
