//! Kimi Code CLI (`kimi`) 一键接入 MuxLayer。
//!
//! 配置文件：当前 Kimi Code CLI 读 `$KIMI_CODE_HOME`（未设时 `~/.kimi-code`）。
//! 旧版 `$KIMI_SHARE_DIR` / `~/.kimi` 仍兼容。
//! 官方文档：https://moonshotai.github.io/kimi-cli/en/configuration/config-files.html
//!
//! apply 只动三处，其它 provider / model / mcp 不动：
//!   - 顶级 `default_model = "muxlayer"`
//!   - `[providers.muxlayer]` type=openai（Chat Completions 兼容）
//!   - `[models.muxlayer]` 指向网关虚拟模型 `muxlayer`（旧配置里的 `agentgate` 仍可用）

use std::path::PathBuf;

use serde::Serialize;

use crate::errors::codes;
use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;
use crate::tools::toml_merge;

const PROVIDER: &str = "muxlayer";
/// 旧版写入的 provider 名,仍按接入识别。
const LEGACY_PROVIDER: &str = "agentgate";
const MODEL_ALIAS: &str = "muxlayer";
const UPSTREAM_MODEL: &str = "muxlayer";
const MAX_CONTEXT_SIZE: u32 = 1_048_576;

pub fn data_dir() -> Result<PathBuf, AppError> {
    if let Some(dir) = std::env::var("KIMI_CODE_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
    {
        return Ok(dir);
    }
    if let Some(dir) = std::env::var("KIMI_SHARE_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
    {
        return Ok(dir);
    }
    let home = crate::fsutil::home_dir()?;
    let kimi_code = home.join(".kimi-code");
    Ok(if kimi_code.is_dir() {
        kimi_code
    } else {
        home.join(".kimi")
    })
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
pub struct KimiCliConfigStatus {
    pub config_path: String,
    pub exists: bool,
    pub has_agentgate: bool,
    pub current_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "KimiCliApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

/// 结构化识别:`[providers.muxlayer]`(或旧版 `[providers.agentgate]`)段的
/// api_key 是本地 token。注释 / 其它段名里出现同样字样不算。
fn is_ours(content: &str) -> bool {
    [PROVIDER, LEGACY_PROVIDER].iter().any(|name| {
        toml_merge::section_body(content, &format!("providers.{name}"))
            .and_then(|body| toml_merge::top_level_raw_value(&body, "api_key"))
            .is_some_and(|raw| cf::is_local_token(raw.trim_matches('"')))
    })
}

pub fn detect() -> KimiCliConfigStatus {
    // 只读探测:家目录不可用时路径为空,按「不存在」展示。
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
    KimiCliConfigStatus {
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
    let existing = cf::read_for_update(&path, codes::KIMI_CONFIG_WRITE_FAILED)?;
    let mut merged =
        toml_merge::upsert_top_level_key(&existing, "default_model", &format!("\"{MODEL_ALIAS}\""));
    let provider_body = format!(
        "type = \"openai\"\nbase_url = \"http://{host}:{port}/v1\"\napi_key = \"{token}\"\n"
    );
    merged = toml_merge::upsert_section(&merged, &format!("providers.{PROVIDER}"), &provider_body);
    let model_body = format!(
        "provider = \"{PROVIDER}\"\nmodel = \"{UPSTREAM_MODEL}\"\nmax_context_size = {MAX_CONTEXT_SIZE}\ndisplay_name = \"MuxLayer\"\n"
    );
    merged = toml_merge::upsert_section(&merged, &format!("models.{MODEL_ALIAS}"), &model_body);
    // config.toml 里带着 gateway token → 0600。
    cf::write_verified(
        &path,
        merged.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::KIMI_CONFIG_WRITE_FAILED,
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
            codes::KIMI_CONFIG_NOT_FOUND,
            "Kimi CLI config.toml does not exist",
        ));
    }
    open::that(&path).map_err(|e| {
        AppError::new(
            codes::KIMI_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

pub fn generate_snippet(host: &str, port: i64) -> String {
    let masked = cf::masked_token_for_snippet();
    format!(
        r#"default_model = "{MODEL_ALIAS}"

[providers.{PROVIDER}]
type = "openai"
base_url = "http://{host}:{port}/v1"
api_key = "{masked}"

[models.{MODEL_ALIAS}]
provider = "{PROVIDER}"
model = "{UPSTREAM_MODEL}"
max_context_size = {MAX_CONTEXT_SIZE}
display_name = "MuxLayer""#
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
    fn detect_ignores_decoys_and_reads_default_model_section_aware() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::env::remove_var("KIMI_CODE_HOME");
        std::env::remove_var("KIMI_SHARE_DIR");
        std::fs::create_dir_all(cp().parent().unwrap()).unwrap();
        std::fs::write(
            cp(),
            "# tried agentgate once, token ag_local_old\n[models.default_model_x]\ndefault_model = \"inner\"\n\n[mcp.agentgate-docs]\ncommand = \"x\"\n",
        )
        .unwrap();
        let status = detect();
        assert!(!status.has_agentgate);
        assert_eq!(status.current_model, None, "section key is not top-level");
        cleanup(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_owner_only_and_refuses_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::env::remove_var("KIMI_CODE_HOME");
        std::env::remove_var("KIMI_SHARE_DIR");
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
        assert!(content.contains("default_model = \"muxlayer\""));
        assert!(content.contains("[providers.muxlayer]"));
        assert!(content.contains("type = \"openai\""));
        assert!(content.contains("127.0.0.1:9090/v1"));
        assert!(content.contains("ag_local_"));
        assert!(content.contains("[models.muxlayer]"));
        assert!(content.contains("model = \"muxlayer\""));
        assert!(
            content.contains("max_context_size = 1048576"),
            "Kimi CLI refuses models without a positive max_context_size"
        );
        assert!(
            !content.contains("agentgate"),
            "new apply must not write legacy virtual model agentgate"
        );
        assert!(detect().has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn test_apply_preserves_other_providers() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            config_path().unwrap(),
            r#"default_model = "kimi-for-coding"

[providers.kimi-for-coding]
type = "kimi"
base_url = "https://api.kimi.com/coding/v1"
api_key = "sk-user"

[models.kimi-for-coding]
provider = "kimi-for-coding"
model = "kimi-for-coding"
"#,
        )
        .unwrap();
        apply("127.0.0.1", 9090).unwrap();
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(content.contains("[providers.kimi-for-coding]"));
        assert!(content.contains("sk-user"));
        assert!(content.contains("[providers.muxlayer]"));
        assert!(content.contains("default_model = \"muxlayer\""));
        cleanup(&temp);
    }

    #[test]
    fn test_apply_writes_kimi_code_home_when_present() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::env::remove_var("KIMI_CODE_HOME");
        std::env::remove_var("KIMI_SHARE_DIR");
        let kimi_code = temp.join(".kimi-code");
        std::fs::create_dir_all(&kimi_code).unwrap();
        apply("127.0.0.1", 9090).unwrap();
        let new_path = kimi_code.join("config.toml");
        let old_path = temp.join(".kimi").join("config.toml");
        assert!(new_path.exists(), "current kimi CLI reads ~/.kimi-code");
        assert!(
            !old_path.exists(),
            "must not write the migrated ~/.kimi path"
        );
        let content = std::fs::read_to_string(&new_path).unwrap();
        assert!(content.contains("default_model = \"muxlayer\""));
        assert!(content.contains("[providers.muxlayer]"));
        cleanup(&temp);
    }
}
