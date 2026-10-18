//! Desktop-managed copy of the bundled `agentgate-serve` sidecar.
//!
//! MCP clients must keep working after Desktop closes or moves (including App
//! Translocation when launched from a DMG). The app therefore copies its
//! signed sidecar to a stable user directory and records ownership explicitly.
//! A user-installed binary at the same path is never overwritten.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

pub const MANAGED_MARKER: &str = ".agentgate-serve.managed.json";
pub const FORCE_ENV: &str = "MUXLAYER_FORCE_BUNDLED_CLI";

#[derive(Debug, Serialize, Deserialize)]
struct ManagedMarker {
    version: String,
    size: u64,
}

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "agentgate-serve.exe"
    } else {
        "agentgate-serve"
    }
}

pub fn managed_cli_path_for(home: &Path) -> PathBuf {
    home.join(".muxlayer").join("bin").join(executable_name())
}

pub fn managed_cli_path() -> Option<PathBuf> {
    crate::fsutil::home_dir()
        .ok()
        .map(|home| managed_cli_path_for(&home))
}

pub fn bundled_source() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(executable_name()))
}

fn read_version(path: &Path) -> Result<String, String> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|error| format!("cannot execute {}: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} --version failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !value.contains("agentgate") {
        return Err(format!(
            "{} did not report an agentgate version",
            path.display()
        ));
    }
    Ok(value)
}

fn marker_path(dest: &Path) -> PathBuf {
    dest.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(MANAGED_MARKER)
}

fn read_marker(dest: &Path) -> Option<ManagedMarker> {
    let bytes = std::fs::read(marker_path(dest)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_marker(dest: &Path, version: &str, size: u64) -> Result<(), String> {
    let marker = serde_json::to_vec(&ManagedMarker {
        version: version.to_string(),
        size,
    })
    .map_err(|error| format!("cannot serialize managed CLI marker: {error}"))?;
    crate::fsutil::atomic_write(&marker_path(dest), &marker, Some(0o600))
        .map_err(|error| format!("cannot write managed CLI marker: {error}"))
}

fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

fn temp_path(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(
        ".{}.{}.tmp",
        dest.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("agentgate-serve"))
            .to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ))
}

/// Copy the sidecar to the stable per-user path.
///
/// Ownership rule:
/// - no destination: install and write a marker
/// - destination + marker matching our size/version: replace only when source differs
/// - destination without a matching marker: treat as user-managed, never overwrite
pub fn ensure_managed_cli(home: &Path, source: &Path) -> Result<Option<PathBuf>, String> {
    if !source.is_file() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(source)
            .map_err(|error| format!("cannot stat {}: {error}", source.display()))?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(format!(
                "bundled CLI is not executable: {}",
                source.display()
            ));
        }
    }

    let source_version = read_version(source)?;
    let source_size = std::fs::metadata(source)
        .map_err(|error| format!("cannot stat {}: {error}", source.display()))?
        .len();
    let dest = managed_cli_path_for(home);
    let parent = dest
        .parent()
        .ok_or_else(|| "managed CLI path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755));
    }

    if dest.exists() {
        let Some(marker) = read_marker(&dest) else {
            return Ok(Some(dest));
        };
        let dest_size = std::fs::metadata(&dest)
            .map_err(|error| format!("cannot stat {}: {error}", dest.display()))?
            .len();
        // A marker that no longer matches the file means the user replaced it.
        if dest_size != marker.size
            || read_version(&dest).ok().as_deref() != Some(marker.version.as_str())
        {
            return Ok(Some(dest));
        }
        if marker.version == source_version && dest_size == source_size {
            return Ok(Some(dest));
        }
    }

    let tmp = temp_path(&dest);
    if let Err(error) = std::fs::copy(source, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot stage bundled CLI: {error}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("cannot set executable mode: {error}"));
        }
    }
    if let Err(error) = read_version(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("staged CLI failed verification: {error}"));
    }
    if let Err(error) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot install managed CLI: {error}"));
    }
    sync_parent(&dest);
    write_marker(&dest, &source_version, source_size)?;
    Ok(Some(dest))
}

/// Startup hook. Debug builds do not copy into the real home directory unless
/// explicitly requested, so `tauri dev` cannot clobber a released managed CLI.
pub fn ensure_for_startup() -> Result<Option<PathBuf>, String> {
    if cfg!(debug_assertions) && std::env::var_os(FORCE_ENV).is_none() {
        return Ok(None);
    }
    let Some(source) = bundled_source() else {
        return Ok(None);
    };
    let home = crate::fsutil::home_dir().map_err(|error| error.message)?;
    ensure_managed_cli(&home, &source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn fake_cli(dir: &Path, version: &str) -> PathBuf {
        let path = dir.join(format!("agentgate-serve-{version}"));
        std::fs::write(&path, format!("#!/bin/sh\necho 'agentgate {version}'\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn installs_sidecar_to_stable_path_with_marker() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let source = fake_cli(dir.path(), "2.0.8");
        let dest = ensure_managed_cli(&home, &source).unwrap().unwrap();

        assert_eq!(dest, managed_cli_path_for(&home));
        assert!(dest.is_file());
        assert!(marker_path(&dest).is_file());
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        assert!(read_marker(&dest).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_overwrite_a_user_managed_binary() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let dest = managed_cli_path_for(&home);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, "#!/bin/sh\necho user\n").unwrap();

        let result = ensure_managed_cli(&home, &fake_cli(dir.path(), "2.0.8")).unwrap();

        assert_eq!(result, Some(dest.clone()));
        assert!(std::fs::read_to_string(&dest).unwrap().contains("user"));
        assert!(!marker_path(&dest).exists());
    }

    #[cfg(unix)]
    #[test]
    fn updates_a_marked_copy_when_the_bundled_version_changes() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        ensure_managed_cli(&home, &fake_cli(dir.path(), "2.0.7")).unwrap();
        let dest = ensure_managed_cli(&home, &fake_cli(dir.path(), "2.0.8"))
            .unwrap()
            .unwrap();

        assert_eq!(read_version(&dest).unwrap(), "agentgate 2.0.8");
        assert_eq!(read_marker(&dest).unwrap().version, "agentgate 2.0.8");
    }

    #[cfg(unix)]
    #[test]
    fn invalid_sidecar_leaves_the_stable_copy_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let dest = ensure_managed_cli(&home, &fake_cli(dir.path(), "2.0.7"))
            .unwrap()
            .unwrap();
        let before = std::fs::read(&dest).unwrap();
        let source = dir.path().join("bad");
        std::fs::write(&source, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(ensure_managed_cli(&home, &source).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), before);
    }
}
