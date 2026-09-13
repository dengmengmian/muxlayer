//! Grok Build (`grok`) 一键接入 MuxLayer。
//!
//! 配置文件：`$GROK_HOME/config.toml`，未设时是 `~/.grok/config.toml`。
//! 官方自定义模型写法：https://docs.x.ai/build/overview#custom-models
//!
//! apply 只动：
//!   - `[models] default = "muxlayer"`（保留 web_search 等其它键）
//!   - `[model.muxlayer]` 指向网关 `/v1`，`api_backend = "responses"`

use std::path::PathBuf;

use serde::Serialize;

use crate::errors::codes;
use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;
use crate::tools::toml_merge;

const MODEL_ID: &str = "muxlayer";
/// 旧版写入的模型段名,仍按接入识别。
const LEGACY_MODEL_ID: &str = "agentgate";
const UPSTREAM_MODEL: &str = "muxlayer";

pub fn data_dir() -> Result<PathBuf, AppError> {
    if let Some(dir) = std::env::var("GROK_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
    {
        return Ok(dir);
    }
    Ok(crate::fsutil::home_dir()?.join(".grok"))
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
pub struct GrokBuildConfigStatus {
    pub config_path: String,
    pub exists: bool,
    pub has_agentgate: bool,
    pub current_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "GrokBuildApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

/// 结构化识别:`[model.muxlayer]`(或旧版 `[model.agentgate]`)段的 api_key
/// 是本地 token。注释 / 其它段里出现同样字样不算。
fn is_ours(content: &str) -> bool {
    [MODEL_ID, LEGACY_MODEL_ID].iter().any(|name| {
        toml_merge::section_body(content, &format!("model.{name}"))
            .and_then(|body| toml_merge::top_level_raw_value(&body, "api_key"))
            .is_some_and(|raw| cf::is_local_token(raw.trim_matches('"')))
    })
}

pub fn detect() -> GrokBuildConfigStatus {
    // 只读探测:家目录不可用时路径为空,按「不存在」展示。
    let path = config_path().unwrap_or_default();
    let path_str = path.to_string_lossy().to_string();
    let exists = path.is_file();
    let (has_agentgate, current_model) = if exists {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        (
            is_ours(&content),
            toml_merge::lookup_str(&content, &["models", "default"]),
        )
    } else {
        (false, None)
    };
    GrokBuildConfigStatus {
        config_path: path_str,
        exists,
        has_agentgate,
        current_model,
    }
}

pub fn apply(host: &str, port: i64) -> Result<ApplyConfigResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let token = local_token::ensure_token()?;
    let path = config_path()?;
    let path_str = path.to_string_lossy().to_string();
    // 读不出来(权限 / 编码)直接失败,绝不拿空内容覆盖。
    let existing = cf::read_for_update(&path, codes::GROK_CONFIG_WRITE_FAILED)?;
    let mut merged =
        toml_merge::upsert_section_key(&existing, "models", "default", &format!("\"{MODEL_ID}\""));
    let body = format!(
        "model = \"{UPSTREAM_MODEL}\"\n\
         base_url = \"http://{host}:{port}/v1\"\n\
         name = \"MuxLayer\"\n\
         description = \"Local MuxLayer gateway\"\n\
         api_key = \"{token}\"\n\
         api_backend = \"responses\"\n"
    );
    merged = toml_merge::upsert_section(&merged, &format!("model.{MODEL_ID}"), &body);
    // config.toml 里带着 gateway token → 0600。
    cf::write_verified(
        &path,
        merged.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::GROK_CONFIG_WRITE_FAILED,
    )?;
    Ok(ApplyConfigResult {
        success: true,
        config_path: path_str,
        changed_keys: vec!["models.default".into(), format!("model.{MODEL_ID}")],
        warnings: vec![],
    })
}

pub fn open_config() -> Result<(), AppError> {
    let path = config_path()?;
    if !path.exists() {
        return Err(AppError::new(
            codes::GROK_CONFIG_NOT_FOUND,
            "Grok Build config.toml does not exist",
        ));
    }
    open::that(&path).map_err(|e| {
        AppError::new(
            codes::GROK_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

pub fn generate_snippet(host: &str, port: i64) -> String {
    let masked = cf::masked_token_for_snippet();
    format!(
        r#"[models]
default = "{MODEL_ID}"

[model.{MODEL_ID}]
model = "{UPSTREAM_MODEL}"
base_url = "http://{host}:{port}/v1"
name = "MuxLayer"
api_key = "{masked}"
api_backend = "responses""#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn cp() -> PathBuf {
        config_path().unwrap()
    }

    #[test]
    fn detect_ignores_decoys_and_reads_models_default_section_aware() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::env::remove_var("GROK_HOME");
        std::fs::create_dir_all(cp().parent().unwrap()).unwrap();
        std::fs::write(
            cp(),
            "# agentgate was here: ag_local_old\n[ui]\ndefault = \"dark\"\n\n[models]\ndefault = \"grok-build\"\n",
        )
        .unwrap();
        let status = detect();
        assert!(!status.has_agentgate);
        assert_eq!(status.current_model.as_deref(), Some("grok-build"));
        cleanup(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_owner_only_and_refuses_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::env::remove_var("GROK_HOME");
        apply("127.0.0.1", 9090).unwrap();
        let mode = std::fs::metadata(cp()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let bad: &[u8] = b"[models]\n\xff";
        std::fs::write(cp(), bad).unwrap();
        assert!(apply("127.0.0.1", 9090).is_err());
        assert_eq!(std::fs::read(cp()).unwrap(), bad);
        cleanup(&temp);
    }

    #[test]
    fn test_detect_no_config() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let status = detect();
        assert!(!status.exists);
        assert!(!status.has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn test_apply_creates_config() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let result = apply("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(content.contains("[models]"));
        assert!(content.contains("default = \"muxlayer\""));
        assert!(content.contains("[model.muxlayer]"));
        assert!(content.contains("model = \"muxlayer\""));
        assert!(
            !content.contains("agentgate"),
            "new apply must not write legacy virtual model agentgate"
        );
        assert!(content.contains("api_backend = \"responses\""));
        assert!(content.contains("127.0.0.1:9090/v1"));
        assert!(content.contains("ag_local_"));
        assert!(detect().has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn test_apply_keeps_web_search_default() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            config_path().unwrap(),
            "[models]\ndefault = \"grok-build\"\nweb_search = \"grok-4.6\"\n",
        )
        .unwrap();
        apply("127.0.0.1", 9090).unwrap();
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(content.contains("web_search = \"grok-4.6\""));
        assert!(content.contains("default = \"muxlayer\""));
        cleanup(&temp);
    }
}
