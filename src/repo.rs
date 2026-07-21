//! Shared repository traversal used by `hint` (context grep) and `scan`
//! (env-var discovery). It applies one consistent set of skips so neither ever
//! reads secrets, build output, lockfiles, or huge/binary files.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Skip files larger than this (avoids loading huge generated files).
pub const MAX_FILE_BYTES: u64 = 256 * 1024;

/// Walks `repo_root`, yielding `(path, contents)` for every readable text file
/// that passes the skip filters: excluded dirs, secret dotenv files, lockfiles,
/// oversized files, and binary/unreadable files.
pub(crate) fn text_files(repo_root: &Path) -> impl Iterator<Item = (PathBuf, String)> {
    WalkDir::new(repo_root)
        .into_iter()
        .filter_entry(|e| !is_excluded(e.path()))
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().is_file() {
                return None;
            }
            let path = entry.path();
            if is_lockfile(path) {
                return None;
            }
            if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                return None;
            }
            // read_to_string fails on binary/non-UTF8 files — those are skipped.
            let content = std::fs::read_to_string(path).ok()?;
            Some((path.to_path_buf(), content))
        })
}

/// Directories/files to prune from the walk.
///
/// Beyond the usual build/VCS dirs, this deliberately skips real dotenv files
/// (`.env`, `.env.local`, `.env.production`, …) so their secret *values* can
/// never be read into a prompt or reported. Template files (`.env.example`,
/// `.env.sample`, …) are safe and kept.
pub(crate) fn is_excluded(path: &Path) -> bool {
    path.file_name()
        .map(|n| {
            let n = n.to_string_lossy();
            n == ".git"
                || n == "target"
                || n == "node_modules"
                || n == ".venv"
                || n == "vendor"
                || is_secret_env_file(&n)
        })
        .unwrap_or(false)
}

/// True for dotenv files that may contain real secrets — everything named
/// `.env` or `.env.*` except the safe template variants.
pub(crate) fn is_secret_env_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower != ".env" && !lower.starts_with(".env.") {
        return false;
    }
    !(lower.ends_with(".example")
        || lower.ends_with(".sample")
        || lower.ends_with(".template")
        || lower.ends_with(".dist"))
}

/// Recognizes dependency lockfiles, which are large and add no useful context.
pub(crate) fn is_lockfile(path: &Path) -> bool {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".lock")
                || lower == "package-lock.json"
                || lower == "yarn.lock"
                || lower == "pnpm-lock.yaml"
                || lower == "poetry.lock"
                || lower == "composer.lock"
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_secret_vs_template_env_files() {
        assert!(is_secret_env_file(".env"));
        assert!(is_secret_env_file(".env.local"));
        assert!(is_secret_env_file(".env.production"));
        assert!(is_secret_env_file(".ENV")); // case-insensitive
        assert!(!is_secret_env_file(".env.example"));
        assert!(!is_secret_env_file(".env.sample"));
        assert!(!is_secret_env_file(".env.template"));
        assert!(!is_secret_env_file("settings.py"));
    }

    #[test]
    fn text_files_skips_dotenv_lockfiles_and_oversized() {
        let dir = std::env::temp_dir().join("env-wizard-repo-walk-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".env"), "SECRET=leak\n").unwrap();
        std::fs::write(dir.join("Cargo.lock"), "noise\n").unwrap();
        std::fs::write(
            dir.join("big.txt"),
            "x".repeat((MAX_FILE_BYTES + 1) as usize),
        )
        .unwrap();
        std::fs::write(dir.join("src.rs"), "fn main() {}\n").unwrap();

        let names: Vec<String> = text_files(&dir)
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(names.contains(&"src.rs".to_string()), "{names:?}");
        assert!(!names.contains(&".env".to_string()), "{names:?}");
        assert!(!names.contains(&"Cargo.lock".to_string()), "{names:?}");
        assert!(!names.contains(&"big.txt".to_string()), "{names:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
