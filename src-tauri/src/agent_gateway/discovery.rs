//! Shared CLI candidate discovery for the Agent Gateway.
//!
//! GUI-launched apps do not inherit the user's shell PATH (macOS gives them a
//! minimal launchd PATH). We therefore keep the adapter's documented priority
//! (explicit path -> environment -> PATH) and append common install locations.
//! This module never executes a login shell; candidates are only path strings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn common_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ];
    if let Some(home) = home_dir() {
        dirs.extend([
            home.join(".local/bin"),
            home.join("bin"),
            home.join(".cargo/bin"),
            home.join(".npm-global/bin"),
            home.join("Library/pnpm"),
            home.join(".volta/bin"),
            home.join(".asdf/shims"),
        ]);
        // Node version managers keep CLI wrappers in versioned bin directories.
        let nvm = home.join(".nvm/versions/node");
        if let Ok(entries) = std::fs::read_dir(&nvm) {
            let mut versions: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path().join("bin"))
                .filter(|path| path.is_dir())
                .collect();
            versions.sort();
            versions.reverse();
            dirs.extend(versions);
        }
    }
    dirs
}

/// Candidate paths in deterministic priority order. `explicit` wins outright:
/// a manually configured path must not silently fall back to an auto-detected
/// binary. Environment overrides and PATH come before common install locations.
pub(crate) fn candidate_paths(
    explicit: Option<&Path>,
    env_key: Option<&str>,
    binary: &str,
) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return vec![path.to_path_buf()];
    }
    let mut candidates = Vec::new();
    if let Some(key) = env_key {
        if let Some(value) = std::env::var_os(key).filter(|value| !value.is_empty()) {
            candidates.push(PathBuf::from(value));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join(binary));
        }
    }
    for dir in common_dirs() {
        candidates.push(dir.join(binary));
    }
    let mut seen = HashSet::new();
    candidates.retain(|path| seen.insert(path.clone()));
    candidates
}

/// True for paths that point inside a macOS app bundle. The adapters reject
/// these because the bundled helper is not the public CLI.
pub(crate) fn is_app_bundle(path: &Path) -> bool {
    path.to_string_lossy().contains(".app/Contents/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_path_is_the_only_candidate() {
        let explicit = PathBuf::from("/custom/codex");
        assert_eq!(
            candidate_paths(Some(&explicit), Some("IGNORED_ENV_FOR_TEST"), "codex"),
            vec![explicit]
        );
    }

    #[test]
    fn candidates_are_deduplicated_and_include_common_locations() {
        let candidates = candidate_paths(None, None, "codex");
        let unique: HashSet<_> = candidates.iter().collect();
        assert_eq!(unique.len(), candidates.len());
        assert!(candidates
            .iter()
            .any(|path| path == Path::new("/opt/homebrew/bin/codex")));
    }

    #[test]
    fn app_bundle_paths_are_rejected() {
        assert!(is_app_bundle(Path::new(
            "/Applications/Codex.app/Contents/MacOS/codex"
        )));
        assert!(!is_app_bundle(Path::new("/opt/homebrew/bin/codex")));
    }
}
