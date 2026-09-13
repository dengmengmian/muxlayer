//! DeepSeek Harness (`dsh`) 一键接入 MuxLayer。
//!
//! 家目录：`$DSH_HOME`，未设时是 `~/.dsh`。
//! 官方：https://github.com/deepseek-ai/deepseek-harness
//!
//! apply 写入：
//!   - `settings.yaml` → `llm-pi-ai.providers.muxlayer`（自定义 OpenAI-compatible 网关）
//!   - `.credentials.yaml` → `MUXLAYER_TOKEN`（settings 只引用，不落明文到 settings）

use std::fs;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde::Serialize;
use serde_yaml::Value;

use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;

const CRED_REF: &str = "MUXLAYER_TOKEN";
const PROVIDER_ID: &str = "muxlayer";
const MODEL_ID: &str = "muxlayer";

pub fn data_dir() -> Result<PathBuf, AppError> {
    if let Some(dir) = std::env::var("DSH_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
    {
        return Ok(dir);
    }
    Ok(crate::fsutil::home_dir()?.join(".dsh"))
}

pub fn settings_path() -> Result<PathBuf, AppError> {
    Ok(data_dir()?.join("settings.yaml"))
}

pub fn credentials_path() -> Result<PathBuf, AppError> {
    Ok(data_dir()?.join(".credentials.yaml"))
}

pub fn snapshot_paths() -> Vec<(&'static str, PathBuf)> {
    [
        ("settings.yaml", settings_path()),
        (".credentials.yaml", credentials_path()),
    ]
    .into_iter()
    .filter_map(|(name, p)| p.ok().map(|p| (name, p)))
    .collect()
}

/// 结构化识别 settings.yaml:`llm-pi-ai.providers.muxlayer.apiKeyEnv == MUXLAYER_TOKEN`。
fn settings_is_ours(content: &str) -> bool {
    serde_yaml::from_str::<Value>(content)
        .ok()
        .and_then(|root| {
            root.get("llm-pi-ai")?
                .get("providers")?
                .get(PROVIDER_ID)?
                .get("apiKeyEnv")?
                .as_str()
                .map(|v| v == CRED_REF)
        })
        .unwrap_or(false)
}

/// 结构化识别 .credentials.yaml:`MUXLAYER_TOKEN`(versioned `refs` 或扁平格式)
/// 的值是本地 token。注释里出现 ag_local_ 不算。
fn credentials_is_ours(content: &str) -> bool {
    let Ok(root) = serde_yaml::from_str::<Value>(content) else {
        return false;
    };
    let token = root
        .get("refs")
        .and_then(|r| r.get(CRED_REF))
        .or_else(|| root.get(CRED_REF))
        .and_then(|v| v.as_str());
    token.is_some_and(cf::is_local_token)
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct DeepSeekHarnessConfigStatus {
    pub config_path: String,
    pub exists: bool,
    pub has_agentgate: bool,
    pub current_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "DeepSeekHarnessApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn detect() -> DeepSeekHarnessConfigStatus {
    // 只读探测:家目录不可用时路径为空,按「不存在」展示。
    let path = settings_path().unwrap_or_default();
    let creds_path = credentials_path().unwrap_or_default();
    let path_str = path.to_string_lossy().to_string();
    let exists = path.is_file() || creds_path.is_file();
    let settings = fs::read_to_string(&path).unwrap_or_default();
    let creds = fs::read_to_string(&creds_path).unwrap_or_default();
    let has_agentgate = settings_is_ours(&settings) || credentials_is_ours(&creds);
    DeepSeekHarnessConfigStatus {
        config_path: path_str,
        exists,
        has_agentgate,
        current_model: None,
    }
}

pub fn apply(host: &str, port: i64) -> Result<ApplyConfigResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let token = local_token::ensure_token()?;
    let dir = data_dir()?;
    fs::create_dir_all(&dir).map_err(|e| {
        AppError::new(
            crate::errors::codes::DSH_CONFIG_WRITE_FAILED,
            format!("Cannot create {dir:?}: {e}"),
        )
    })?;
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }

    // 两个文件都先读:任何一个读不出来(权限 / 编码)都中止,绝不拿空内容覆盖。
    let settings = settings_path()?;
    let creds_path = credentials_path()?;
    let existing = cf::read_for_update(&settings, crate::errors::codes::DSH_CONFIG_WRITE_FAILED)?;
    let creds_existing =
        cf::read_for_update(&creds_path, crate::errors::codes::DSH_CONFIG_WRITE_FAILED)?;
    let merged = merge_settings(&existing, host, port)?;
    let creds = upsert_credential(&creds_existing, CRED_REF, &token)?;

    cf::write_verified(
        &settings,
        merged.as_bytes(),
        None,
        crate::errors::codes::DSH_CONFIG_WRITE_FAILED,
    )?;
    // 凭据文件从创建起就是 0600,没有先宽后收的窗口。
    cf::write_verified(
        &creds_path,
        creds.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        crate::errors::codes::DSH_CONFIG_WRITE_FAILED,
    )?;

    Ok(ApplyConfigResult {
        success: true,
        config_path: settings.to_string_lossy().to_string(),
        changed_keys: vec![
            format!("llm-pi-ai.providers.{PROVIDER_ID}"),
            format!(".credentials.yaml {CRED_REF}"),
        ],
        warnings: vec![
            "settings.yaml is merged as YAML; comments in that file may be rewritten.".into(),
        ],
    })
}

fn merge_settings(existing: &str, host: &str, port: i64) -> Result<String, AppError> {
    let mut root = if existing.trim().is_empty() {
        Value::Mapping(serde_yaml::Mapping::new())
    } else {
        serde_yaml::from_str(existing).map_err(|e| {
            AppError::new(
                crate::errors::codes::DSH_CONFIG_PARSE_ERROR,
                format!("Cannot parse settings.yaml: {e}"),
            )
        })?
    };
    let mapping = root.as_mapping_mut().ok_or_else(|| {
        AppError::new(
            crate::errors::codes::DSH_CONFIG_PARSE_ERROR,
            "settings.yaml root must be a mapping",
        )
    })?;
    let llm = mapping
        .entry(Value::String("llm-pi-ai".into()))
        .or_insert_with(|| Value::Mapping(serde_yaml::Mapping::new()));
    let llm_map = llm.as_mapping_mut().ok_or_else(|| {
        AppError::new(
            crate::errors::codes::DSH_CONFIG_PARSE_ERROR,
            "llm-pi-ai must be a mapping",
        )
    })?;
    let providers = llm_map
        .entry(Value::String("providers".into()))
        .or_insert_with(|| Value::Mapping(serde_yaml::Mapping::new()));
    let providers_map = providers.as_mapping_mut().ok_or_else(|| {
        AppError::new(
            crate::errors::codes::DSH_CONFIG_PARSE_ERROR,
            "llm-pi-ai.providers must be a mapping",
        )
    })?;
    providers_map.insert(
        Value::String(PROVIDER_ID.into()),
        serde_yaml::from_value(
            serde_yaml::to_value(serde_json::json!({
                "apiKeyEnv": CRED_REF,
                "api": "openai-completions",
                "baseURL": format!("http://{host}:{port}/v1"),
                "compat": {
                    "supportsDeveloperRole": false,
                    "maxTokensField": "max_tokens",
                },
                "models": [{ "id": MODEL_ID, "input": ["text", "image"] }],
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    // 新会话默认走 muxlayer，避免还用官方 DeepSeek 卡（缺 DEEPSEEK_API_KEY 会报 API key is invalid）。
    mapping.insert(
        Value::String("agent-default-model".into()),
        serde_yaml::from_value(
            serde_yaml::to_value(serde_json::json!({
                "provider": PROVIDER_ID,
                "model": MODEL_ID,
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    serde_yaml::to_string(&root).map_err(|e| {
        AppError::new(
            crate::errors::codes::DSH_CONFIG_WRITE_FAILED,
            format!("Cannot serialize settings.yaml: {e}"),
        )
    })
}

fn upsert_credential(existing: &str, key: &str, value: &str) -> Result<String, AppError> {
    let mut root = if existing.trim().is_empty() {
        let mut m = serde_yaml::Mapping::new();
        m.insert(Value::String("version".into()), Value::Number(1.into()));
        m.insert(
            Value::String("refs".into()),
            Value::Mapping(serde_yaml::Mapping::new()),
        );
        Value::Mapping(m)
    } else {
        serde_yaml::from_str(existing).map_err(|e| {
            AppError::new(
                crate::errors::codes::DSH_CONFIG_PARSE_ERROR,
                format!("Cannot parse .credentials.yaml: {e}"),
            )
        })?
    };
    let mapping = root.as_mapping_mut().ok_or_else(|| {
        AppError::new(
            crate::errors::codes::DSH_CONFIG_PARSE_ERROR,
            ".credentials.yaml root must be a mapping",
        )
    })?;
    let versioned = mapping.contains_key(Value::String("refs".into()))
        || mapping.contains_key(Value::String("version".into()));
    if versioned {
        let refs = mapping
            .entry(Value::String("refs".into()))
            .or_insert_with(|| Value::Mapping(serde_yaml::Mapping::new()));
        let refs_map = refs.as_mapping_mut().ok_or_else(|| {
            AppError::new(
                crate::errors::codes::DSH_CONFIG_PARSE_ERROR,
                ".credentials.yaml refs must be a mapping",
            )
        })?;
        refs_map.insert(Value::String(key.into()), Value::String(value.into()));
    } else {
        mapping.insert(Value::String(key.into()), Value::String(value.into()));
    }
    serde_yaml::to_string(&root).map_err(|e| {
        AppError::new(
            crate::errors::codes::DSH_CONFIG_WRITE_FAILED,
            format!("Cannot serialize .credentials.yaml: {e}"),
        )
    })
}

pub fn open_config() -> Result<(), AppError> {
    let path = settings_path()?;
    if !path.exists() {
        return Err(AppError::new(
            crate::errors::codes::DSH_CONFIG_NOT_FOUND,
            "DeepSeek Harness settings.yaml does not exist",
        ));
    }
    open::that(&path).map_err(|e| {
        AppError::new(
            crate::errors::codes::DSH_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

pub fn generate_snippet(host: &str, port: i64) -> String {
    format!(
        "llm-pi-ai:\n  providers:\n    {PROVIDER_ID}:\n      apiKeyEnv: {CRED_REF}\n      api: openai-completions\n      baseURL: http://{host}:{port}/v1\n      models:\n        - id: {MODEL_ID}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    #[test]
    fn detect_ignores_decoy_muxlayer_mentions() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::env::remove_var("DSH_HOME");
        std::fs::create_dir_all(data_dir().unwrap()).unwrap();
        std::fs::write(
            settings_path().unwrap(),
            "# muxlayer notes\nmcp:\n  muxlayer-docs:\n    command: npx\n",
        )
        .unwrap();
        std::fs::write(
            credentials_path().unwrap(),
            "# ag_local_ tokens go here\nDEEPSEEK_API_KEY: sk-x\n",
        )
        .unwrap();
        assert!(!detect().has_agentgate);
        cleanup(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn apply_refuses_to_overwrite_unreadable_settings() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::env::remove_var("DSH_HOME");
        std::fs::create_dir_all(data_dir().unwrap()).unwrap();
        let bad: &[u8] = b"llm-pi-ai: {}\n\xff";
        std::fs::write(settings_path().unwrap(), bad).unwrap();
        assert!(apply("127.0.0.1", 9090).is_err());
        assert_eq!(std::fs::read(settings_path().unwrap()).unwrap(), bad);
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
    fn test_apply_writes_settings_and_credentials() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let result = apply("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        let settings = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(settings.contains("muxlayer"));
        assert!(settings.contains("openai-completions"));
        assert!(settings.contains("127.0.0.1:9090/v1"));
        assert!(settings.contains("agent-default-model"));
        assert!(settings.contains("provider: muxlayer"));
        assert!(
            settings.contains("model: muxlayer"),
            "new apply must write canonical virtual model muxlayer"
        );
        assert!(
            !settings.contains("agentgate"),
            "new apply must not write legacy virtual model agentgate"
        );
        assert!(
            !settings.contains("ag_local_"),
            "token must not land in settings.yaml"
        );
        let creds = std::fs::read_to_string(credentials_path().unwrap()).unwrap();
        assert!(creds.contains("MUXLAYER_TOKEN"));
        assert!(creds.contains("ag_local_"));
        assert!(detect().has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn test_apply_keeps_other_pi_ai_providers() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(data_dir().unwrap()).unwrap();
        std::fs::write(
            settings_path().unwrap(),
            "llm-pi-ai:\n  providers:\n    anthropic:\n      apiKeyEnv: ANTHROPIC_API_KEY\n",
        )
        .unwrap();
        apply("127.0.0.1", 9090).unwrap();
        let settings = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(settings.contains("anthropic"));
        assert!(settings.contains("ANTHROPIC_API_KEY"));
        assert!(settings.contains("muxlayer"));
        cleanup(&temp);
    }

    #[test]
    fn test_credentials_flat_format_kept() {
        let out = upsert_credential("DEEPSEEK_API_KEY: sk-old\n", CRED_REF, "ag_local_x").unwrap();
        assert!(out.contains("DEEPSEEK_API_KEY"));
        assert!(out.contains("MUXLAYER_TOKEN"));
        assert!(out.contains("ag_local_x"));
    }
}
