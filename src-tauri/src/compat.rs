use std::path::{Path, PathBuf};

pub const CURRENT_CLI_DATA_DIR: &str = ".muxlayer";
pub const LEGACY_CLI_DATA_DIR: &str = ".agentgate";

/// Resolve the headless server data directory without splitting an existing
/// installation into two databases.
///
/// New installations use `~/.muxlayer`. If the old directory contains the
/// database or token, it remains the active directory until a future release
/// can perform an explicit, atomic copy. This is intentionally a lazy
/// migration: the old user's state is adopted automatically and nothing is
/// deleted or moved while the server may still be running.
pub fn default_cli_data_dir(home: &Path) -> PathBuf {
    let current = home.join(CURRENT_CLI_DATA_DIR);
    let legacy = home.join(LEGACY_CLI_DATA_DIR);

    if has_state(&current) || !has_state(&legacy) {
        current
    } else {
        legacy
    }
}

fn has_state(dir: &Path) -> bool {
    dir.join("agentgate.db").is_file() || dir.join("token").is_file()
}

pub fn prefer_current_env(current: Option<&str>, legacy: Option<&str>) -> Option<String> {
    current
        .filter(|value| !value.trim().is_empty())
        .or_else(|| legacy.filter(|value| !value.trim().is_empty()))
        .map(str::to_string)
}

pub fn env_value(current: &str, legacy: &str) -> Option<String> {
    let current_value = std::env::var(current).ok();
    let legacy_value = std::env::var(legacy).ok();
    prefer_current_env(current_value.as_deref(), legacy_value.as_deref())
}

/// 布尔开关环境变量(新名优先,兼容旧 AGENTGATE_* 名):`1` / `true` / `yes` / `on`
/// (不分大小写)为开,其余(含未设置)为关。
pub fn env_flag(current: &str, legacy: &str) -> bool {
    flag_enabled(env_value(current, legacy).as_deref())
}

fn flag_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_home(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("muxlayer_compat_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn new_install_uses_muxlayer_directory() {
        let home = test_home("new");

        assert_eq!(default_cli_data_dir(&home), home.join(CURRENT_CLI_DATA_DIR));

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn existing_agentgate_data_is_used_without_copying_or_losing_state() {
        let home = test_home("legacy");
        let legacy = home.join(LEGACY_CLI_DATA_DIR);
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("agentgate.db"), b"legacy database").unwrap();

        assert_eq!(default_cli_data_dir(&home), legacy);
        assert!(!home.join(CURRENT_CLI_DATA_DIR).exists());

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn current_data_wins_when_both_directories_have_state() {
        let home = test_home("both");
        let current = home.join(CURRENT_CLI_DATA_DIR);
        let legacy = home.join(LEGACY_CLI_DATA_DIR);
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(current.join("agentgate.db"), b"current database").unwrap();
        fs::write(legacy.join("agentgate.db"), b"legacy database").unwrap();

        assert_eq!(default_cli_data_dir(&home), current);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn flag_values_are_opt_in() {
        for on in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(flag_enabled(Some(on)), "{on}");
        }
        for off in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("2"),
        ] {
            assert!(!flag_enabled(off), "{off:?}");
        }
    }

    #[test]
    fn current_environment_value_takes_precedence() {
        assert_eq!(
            prefer_current_env(Some("/new"), Some("/old")),
            Some("/new".to_string())
        );
        assert_eq!(
            prefer_current_env(Some("  "), Some("/old")),
            Some("/old".to_string())
        );
        assert_eq!(
            prefer_current_env(None, Some("/old")),
            Some("/old".to_string())
        );
    }
}
