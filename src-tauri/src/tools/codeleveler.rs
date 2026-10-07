//! CodeLeveler (`leveler`) 一键接入 MuxLayer。
//!
//! 配置文件：`$LEVELER_HOME/config.toml`，未设置时 `~/.leveler/config.toml`。
//! 解析器 `deny_unknown_fields`，只写它认识的键。`default_model` 必须是
//! `provider/model`。网关是 OpenAI Chat Completions，协议名 `openai_chat`。

use std::path::PathBuf;

use serde::Serialize;

use crate::errors::codes;
use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;
use crate::tools::toml_merge;

const PROVIDER: &str = "muxlayer";
const LEGACY_PROVIDER: &str = "agentgate";
const MODEL_ALIAS: &str = "muxlayer";
const DEFAULT_MODEL: &str = "muxlayer/muxlayer";
const CONTEXT_WINDOW: u32 = 1_048_576;

pub fn data_dir() -> Result<PathBuf, AppError> {
    if let Some(dir) = std::env::var("LEVELER_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
    {
        return Ok(dir);
    }
    Ok(crate::fsutil::home_dir()?.join(".leveler"))
}

pub fn config_path() -> Result<PathBuf, AppError> {
    Ok(data_dir()?.join("config.toml"))
}

pub fn snapshot_paths() -> Vec<(&'static str, PathBuf)> {
    config_path()
        .map(|p| vec![("config.toml", p)])
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct CodelevelerConfigStatus {
    pub config_path: String,
    pub exists: bool,
    pub has_agentgate: bool,
    pub current_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "CodelevelerApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

fn is_ours(content: &str) -> bool {
    [PROVIDER, LEGACY_PROVIDER].iter().any(|name| {
        toml_merge::section_body(content, &format!("providers.{name}"))
            .and_then(|body| toml_merge::top_level_raw_value(&body, "api_key"))
            .is_some_and(|raw| cf::is_local_token(raw.trim_matches('"')))
    })
}

pub fn detect() -> CodelevelerConfigStatus {
    let path = config_path().unwrap_or_default();
    let path_str = path.to_string_lossy().to_string();
    let exists = path.is_file();
    let (has_agentgate, current_model) = if exists {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        (
            is_ours(&content),
            toml_merge::lookup_str(&content, &["default_model"]),
        )
    } else {
        (false, None)
    };
    CodelevelerConfigStatus {
        config_path: path_str,
        exists,
        has_agentgate,
        current_model,
    }
}

pub fn apply(host: &str, port: i64) -> Result<ApplyConfigResult, AppError> {
    let _config_lock = crate::fsutil::lock_client_configs();
    let token = local_token::ensure_token()?;
    let path = config_path()?;
    let path_str = path.to_string_lossy().to_string();
    let existing = cf::read_for_update(&path, codes::LEVELER_CONFIG_WRITE_FAILED)?;
    let mut merged = toml_merge::upsert_top_level_key(
        &existing,
        "default_model",
        &format!("\"{DEFAULT_MODEL}\""),
    );
    let provider_body = format!(
        "protocol = \"openai_chat\"\nbase_url = \"http://{host}:{port}/v1\"\napi_key = \"{token}\"\n"
    );
    merged = toml_merge::upsert_section(&merged, &format!("providers.{PROVIDER}"), &provider_body);
    let model_body = format!(
        "provider = \"{PROVIDER}\"\nmodel_id = \"{MODEL_ALIAS}\"\ncontext_window = {CONTEXT_WINDOW}\n"
    );
    merged = toml_merge::upsert_section(&merged, &format!("models.{MODEL_ALIAS}"), &model_body);
    cf::write_verified(
        &path,
        merged.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::LEVELER_CONFIG_WRITE_FAILED,
    )?;
    Ok(ApplyConfigResult {
        success: true,
        config_path: path_str,
        changed_keys: vec![
            "default_model".into(),
            format!("providers.{PROVIDER}"),
            format!("models.{MODEL_ALIAS}"),
        ],
        warnings: vec![],
    })
}

pub fn open_config() -> Result<(), AppError> {
    let path = config_path()?;
    if !path.exists() {
        return Err(AppError::new(
            codes::LEVELER_CONFIG_NOT_FOUND,
            "CodeLeveler config.toml does not exist",
        ));
    }
    open::that(&path).map_err(|e| {
        AppError::new(
            codes::LEVELER_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

pub fn generate_snippet(host: &str, port: i64) -> String {
    let masked = cf::masked_token_for_snippet();
    format!(
        r#"default_model = "{DEFAULT_MODEL}"

[providers.{PROVIDER}]
protocol = "openai_chat"
base_url = "http://{host}:{port}/v1"
api_key = "{masked}"

[models.{MODEL_ALIAS}]
provider = "{PROVIDER}"
model_id = "{MODEL_ALIAS}"
context_window = {CONTEXT_WINDOW}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn cp() -> PathBuf {
        config_path().unwrap()
    }

    fn isolate() {
        std::env::remove_var("LEVELER_HOME");
    }

    #[test]
    fn detect_reads_default_model_and_ignores_decoys() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        isolate();
        std::fs::create_dir_all(cp().parent().unwrap()).unwrap();
        std::fs::write(
            cp(),
            "# token ag_local_old is not ours\n\ndefault_model = \"deepseek/deepseek-flash\"\n\n[providers.deepseek]\nbase_url = \"https://api.deepseek.com\"\napi_key = \"sk-user\"\n\n[models.deepseek-flash]\nprovider = \"deepseek\"\n",
        )
        .unwrap();
        let status = detect();
        assert!(status.exists);
        assert!(
            !status.has_agentgate,
            "comment and other providers are not MuxLayer"
        );
        assert_eq!(
            status.current_model.as_deref(),
            Some("deepseek/deepseek-flash")
        );
        cleanup(&temp);
    }

    #[test]
    fn apply_writes_openai_chat_provider_and_preserves_the_rest() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        isolate();
        std::fs::create_dir_all(cp().parent().unwrap()).unwrap();
        std::fs::write(
            cp(),
            "# keep me\nthinking = \"high\"\n\n[providers.deepseek]\nbase_url = \"https://api.deepseek.com\"\napi_key = \"sk-user\"\n\n[[mcp_servers]]\nname = \"docs\"\ncommand = \"echo\"\n",
        )
        .unwrap();
        let result = apply("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        let content = std::fs::read_to_string(cp()).unwrap();
        assert!(content.contains("# keep me"));
        assert!(content.contains("thinking = \"high\""));
        assert!(content.contains("sk-user"));
        assert!(content.contains("[[mcp_servers]]"));
        assert!(content.contains("default_model = \"muxlayer/muxlayer\""));
        assert!(content.contains("[providers.muxlayer]"));
        assert!(content.contains("protocol = \"openai_chat\""));
        assert!(content.contains("base_url = \"http://127.0.0.1:9090/v1\""));
        assert!(content.contains("ag_local_"));
        assert!(content.contains("[models.muxlayer]"));
        assert!(content.contains("provider = \"muxlayer\""));
        assert!(content.contains("model_id = \"muxlayer\""));
        assert!(content.contains("context_window = 1048576"));
        assert!(!content.contains("display_name"));
        assert!(!content.contains("max_context_size"));
        assert!(!content.contains("type = \"openai\""));
        assert!(detect().has_agentgate);
        cleanup(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_owner_only_and_refuses_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        isolate();
        apply("127.0.0.1", 9090).unwrap();
        let mode = std::fs::metadata(cp()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let bad: &[u8] = b"default_model = \"x\"\n\xff";
        std::fs::write(cp(), bad).unwrap();
        assert!(apply("127.0.0.1", 9090).is_err());
        assert_eq!(std::fs::read(cp()).unwrap(), bad);
        cleanup(&temp);
    }

    #[test]
    fn apply_uses_leveler_home_when_set() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let home = temp.join("custom-leveler");
        std::env::set_var("LEVELER_HOME", &home);
        apply("127.0.0.1", 9090).unwrap();
        std::env::remove_var("LEVELER_HOME");
        assert!(home.join("config.toml").is_file());
        assert!(!temp.join(".leveler").join("config.toml").exists());
        cleanup(&temp);
    }
}
