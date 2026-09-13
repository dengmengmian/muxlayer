use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use crate::errors::codes;
use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;

const AGENTGATE_MODEL: &str = "openai/muxlayer";

pub fn config_path() -> Result<PathBuf, AppError> {
    Ok(crate::fsutil::home_dir()?
        .join(".config")
        .join("opencode")
        .join("opencode.json"))
}

pub fn snapshot_paths() -> Vec<(&'static str, PathBuf)> {
    config_path()
        .map(|p| vec![("opencode.json", p)])
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct OpenCodeConfigStatus {
    pub config_path: String,
    pub exists: bool,
    pub has_agentgate: bool,
    pub current_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "OpenCodeApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

/// 结构化识别:`provider.openai.options.apiKey` 是本地 token。
/// MCP 名 / 其它字段里出现 muxlayer / agentgate 字样不算接入。
fn doc_is_ours(doc: &Value) -> bool {
    doc.pointer("/provider/openai/options/apiKey")
        .and_then(|v| v.as_str())
        .is_some_and(cf::is_local_token)
}

pub fn detect() -> OpenCodeConfigStatus {
    // 只读探测:家目录不可用时路径为空,按「不存在」展示。
    let path = config_path().unwrap_or_default();
    let path_str = path.to_string_lossy().to_string();
    let exists = path.is_file();

    let (has_agentgate, current_model) = if exists {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let doc = serde_json::from_str::<Value>(&content).ok();
        let has_ag = doc.as_ref().is_some_and(doc_is_ours);
        let model = doc
            .as_ref()
            .and_then(|v| v.get("model")?.as_str().map(String::from));
        (has_ag, model)
    } else {
        (false, None)
    };

    OpenCodeConfigStatus {
        config_path: path_str,
        exists,
        has_agentgate,
        current_model,
    }
}

pub fn generate_snippet(host: &str, port: i64) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "model": AGENTGATE_MODEL,
        "provider": {
            "openai": {
                "options": {
                    "apiKey": "<ag_local_token>",
                    "baseURL": format!("http://{host}:{port}/v1")
                }
            }
        }
    }))
    .unwrap_or_default()
}

/// 取 `parent[key]` 的对象;不存在或不是对象时替换成空对象。
fn object_entry<'a>(
    parent: &'a mut serde_json::Map<String, Value>,
    key: &str,
) -> &'a mut serde_json::Map<String, Value> {
    let entry = parent
        .entry(key.to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !entry.is_object() {
        *entry = serde_json::json!({});
    }
    entry
        .as_object_mut()
        .expect("entry was just normalised to an object")
}

pub fn apply(host: &str, port: i64) -> Result<ApplyConfigResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let token = local_token::ensure_token()?;

    let path = config_path()?;
    let path_str = path.to_string_lossy().to_string();
    let warnings = Vec::new();

    // 读不出来(权限 / 编码)直接失败,绝不拿空内容覆盖。
    let existing = cf::read_for_update(&path, codes::OPENCODE_CONFIG_WRITE_FAILED)?;
    let existing = if existing.trim().is_empty() {
        "{}".to_string()
    } else {
        existing
    };

    let mut doc: Value = serde_json::from_str(&existing).map_err(|e| {
        AppError::new(
            codes::OPENCODE_CONFIG_PARSE_ERROR,
            format!("Cannot parse opencode.json: {e}"),
        )
    })?;
    let root = doc.as_object_mut().ok_or_else(|| {
        AppError::new(
            codes::OPENCODE_CONFIG_PARSE_ERROR,
            "opencode.json root must be an object",
        )
    })?;

    // Set model
    root.insert("model".to_string(), serde_json::json!(AGENTGATE_MODEL));

    // 只 upsert provider.openai.options 里我们拥有的两个字段;
    // 用户的 anthropic / ollama / 自定义 provider 以及 openai 下的其它字段保留。
    let options = object_entry(
        object_entry(object_entry(root, "provider"), "openai"),
        "options",
    );
    options.insert("apiKey".to_string(), serde_json::json!(token));
    options.insert(
        "baseURL".to_string(),
        serde_json::json!(format!("http://{host}:{port}/v1")),
    );

    let new_content = serde_json::to_string_pretty(&doc).map_err(|e| {
        AppError::new(
            codes::OPENCODE_CONFIG_WRITE_FAILED,
            format!("Cannot serialize: {e}"),
        )
    })?;

    // opencode.json 里带着 gateway token → 0600。
    cf::write_verified(
        &path,
        format!("{new_content}\n").as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::OPENCODE_CONFIG_WRITE_FAILED,
    )?;

    let changed_keys = vec![
        "model".to_string(),
        "provider.openai.options.apiKey".to_string(),
        "provider.openai.options.baseURL".to_string(),
    ];

    Ok(ApplyConfigResult {
        success: true,
        config_path: path_str,
        changed_keys,
        warnings,
    })
}

pub fn open_config() -> Result<(), AppError> {
    let path = config_path()?;
    if !path.exists() {
        return Err(AppError::new(
            codes::OPENCODE_CONFIG_NOT_FOUND,
            "OpenCode config file does not exist",
        ));
    }
    open::that(&path).map_err(|e| {
        AppError::new(
            codes::OPENCODE_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn cp() -> PathBuf {
        config_path().unwrap()
    }

    #[test]
    fn apply_upserts_only_openai_options_and_keeps_other_providers() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(cp().parent().unwrap()).unwrap();
        std::fs::write(
            cp(),
            r#"{"provider":{"anthropic":{"options":{"apiKey":"sk-ant"}},"ollama":{"npm":"@ai-sdk/openai-compatible","options":{"baseURL":"http://localhost:11434/v1"}},"openai":{"models":{"gpt-x":{}},"options":{"timeout":600000,"apiKey":"sk-openai"}}}}"#,
        )
        .unwrap();
        apply("127.0.0.1", 9090).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(cp()).unwrap()).unwrap();
        assert_eq!(doc["provider"]["anthropic"]["options"]["apiKey"], "sk-ant");
        assert_eq!(
            doc["provider"]["ollama"]["options"]["baseURL"],
            "http://localhost:11434/v1"
        );
        assert!(doc["provider"]["openai"]["models"].get("gpt-x").is_some());
        assert_eq!(doc["provider"]["openai"]["options"]["timeout"], 600000);
        assert!(doc["provider"]["openai"]["options"]["apiKey"]
            .as_str()
            .unwrap()
            .starts_with("ag_local_"));
        assert_eq!(
            doc["provider"]["openai"]["options"]["baseURL"],
            "http://127.0.0.1:9090/v1"
        );
        cleanup(&temp);
    }

    #[test]
    fn detect_ignores_decoy_mcp_name() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(cp().parent().unwrap()).unwrap();
        std::fs::write(
            cp(),
            r#"{"mcp":{"muxlayer-docs":{"type":"local","command":["agentgate"]}},"model":"anthropic/claude"}"#,
        )
        .unwrap();
        assert!(!detect().has_agentgate);
        cleanup(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_owner_only_and_refuses_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        apply("127.0.0.1", 9090).unwrap();
        let mode = std::fs::metadata(cp()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let bad: &[u8] = b"{\"provider\":{}} \xff";
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
        assert!(config_path().unwrap().exists());
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(content.contains("ag_local_"));
        assert!(content.contains("127.0.0.1:9090"));
        assert!(content.contains("openai/muxlayer"));
        cleanup(&temp);
    }

    #[test]
    fn test_apply_preserves_existing_fields() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            config_path().unwrap(),
            r#"{"autoupdate":true,"tools":{"bash":true}}"#,
        )
        .unwrap();
        let result = apply("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(content.contains("autoupdate"));
        assert!(content.contains("ag_local_"));
        cleanup(&temp);
    }
}
