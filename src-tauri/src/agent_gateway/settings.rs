//! Persistent Agent Gateway CLI path overrides.
//!
//! The Desktop app and the headless Agent Service must observe the same paths.
//! The Desktop app's own SQLite file is the Model Gateway database, so these
//! settings live in the **CLI data directory** database (`~/.muxlayer` by
//! default). `agentgate-serve agent-service` reads them before discovering
//! Codex / Claude Code, which makes a Desktop-saved path effective for
//! MCP-auto-started services too.

use std::path::{Path, PathBuf};

use super::protocol::{AgentError, ErrorCode};

pub const CODEX_BIN_KEY: &str = "agent_gateway.codex_bin";
pub const CLAUDE_BIN_KEY: &str = "agent_gateway.claude_bin";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPaths {
    pub codex_bin: Option<PathBuf>,
    pub claude_bin: Option<PathBuf>,
}

fn storage_error(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::Storage, message)
}

fn open_pool(db_dir: &Path) -> Result<crate::storage::db::DbPool, AgentError> {
    crate::storage::db::init_database(&db_dir.to_path_buf())
        .map_err(|error| storage_error(format!("cannot open Agent Gateway settings: {error}")))
}

fn read_path(conn: &rusqlite::Connection, key: &str) -> Result<Option<PathBuf>, AgentError> {
    let value = crate::storage::app_settings::get(conn, key)
        .map_err(|error| storage_error(format!("cannot read {key}: {error}")))?;
    Ok(value
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from))
}

/// Load configured CLI overrides. Missing keys mean automatic discovery.
pub fn load(db_dir: &Path) -> Result<AgentPaths, AgentError> {
    // Reading status must not create a CLI database just because the Desktop
    // page was opened before agentgate-serve was installed.
    if !db_dir.join("agentgate.db").is_file() {
        return Ok(AgentPaths::default());
    }
    let pool = open_pool(db_dir)?;
    let conn = pool
        .get()
        .map_err(|error| storage_error(format!("cannot acquire settings connection: {error}")))?;
    Ok(AgentPaths {
        codex_bin: read_path(&conn, CODEX_BIN_KEY)?,
        claude_bin: read_path(&conn, CLAUDE_BIN_KEY)?,
    })
}

/// Save (or clear with `None`) one CLI override. The value must be absolute;
/// validation against the real executable happens in the Desktop command
/// before this function is called.
pub fn set_bin(db_dir: &Path, key: &str, value: Option<&Path>) -> Result<(), AgentError> {
    if key != CODEX_BIN_KEY && key != CLAUDE_BIN_KEY {
        return Err(AgentError::new(
            ErrorCode::InvalidRequest,
            "unsupported Agent Gateway path setting",
        ));
    }
    let pool = open_pool(db_dir)?;
    let conn = pool
        .get()
        .map_err(|error| storage_error(format!("cannot acquire settings connection: {error}")))?;
    match value {
        Some(path) => {
            if !path.is_absolute() {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "Agent CLI path must be absolute",
                ));
            }
            crate::storage::app_settings::set(&conn, key, &path.to_string_lossy())
                .map_err(|error| storage_error(format!("cannot save {key}: {error}")))
        }
        None => crate::storage::app_settings::set(&conn, key, "")
            .map_err(|error| storage_error(format!("cannot clear {key}: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentgate_settings_{name}_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn paths_round_trip_and_clear() {
        let dir = temp_dir("round_trip");
        assert_eq!(load(&dir).unwrap(), AgentPaths::default());
        let codex = PathBuf::from("/opt/homebrew/bin/codex");
        let claude = PathBuf::from("/Users/me/.local/bin/claude");
        set_bin(&dir, CODEX_BIN_KEY, Some(&codex)).unwrap();
        set_bin(&dir, CLAUDE_BIN_KEY, Some(&claude)).unwrap();
        assert_eq!(
            load(&dir).unwrap(),
            AgentPaths {
                codex_bin: Some(codex),
                claude_bin: Some(claude)
            }
        );
        set_bin(&dir, CODEX_BIN_KEY, None).unwrap();
        assert_eq!(load(&dir).unwrap().codex_bin, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn relative_paths_are_rejected() {
        let dir = temp_dir("relative");
        let error = set_bin(&dir, CODEX_BIN_KEY, Some(Path::new("codex"))).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let dir = temp_dir("key");
        let error = set_bin(&dir, "some.other.key", None).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        let _ = std::fs::remove_dir_all(dir);
    }
}
