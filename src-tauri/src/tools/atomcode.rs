use std::path::PathBuf;

use serde::Serialize;

use crate::errors::codes;
use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;
use crate::tools::toml_merge;

const AGENTGATE_MODEL: &str = "muxlayer";
/// apply 写的顶层 key 与值字面量。
const DEFAULT_PROVIDER_KEY: &str = "default_provider";
const DEFAULT_PROVIDER_VALUE: &str = "\"agentgate\"";
/// apply 接管的 provider 段。
const PROVIDER_SECTION: &str = "providers.agentgate";

/// AtomCode config.toml path
pub fn config_path() -> Result<PathBuf, AppError> {
    Ok(crate::fsutil::home_dir()?
        .join(".atomcode")
        .join("config.toml"))
}

pub fn snapshot_paths() -> Vec<(&'static str, PathBuf)> {
    config_path()
        .map(|p| vec![("config.toml", p)])
        .unwrap_or_default()
}

fn saved_dir() -> PathBuf {
    local_token::token_dir().join("atomcode_official")
}

fn saved_config_path() -> PathBuf {
    saved_dir().join("config.toml")
}

pub fn has_saved_official() -> bool {
    saved_config_path().exists()
}

/// `[providers.agentgate]` 段的 api_key 是否是本地 token(结构化识别,注释 /
/// 其它 provider 名里出现 muxlayer / agentgate 字样不算)。
fn is_ours(content: &str) -> bool {
    toml_merge::section_body(content, PROVIDER_SECTION)
        .and_then(|body| toml_merge::top_level_raw_value(&body, "api_key"))
        .is_some_and(|raw| cf::is_local_token(raw.trim_matches('"')))
}

/// 「切回官方」的纯函数核心:只撤销 apply 写过的 `default_provider`(仍是
/// "agentgate" 时;快照有原值写回原值,否则删除)和 `[providers.agentgate]` 段。
/// 用户 apply 之后新增的 provider / 注释逐字节保留。
pub(crate) fn restore_content(current: &str, snapshot: Option<&str>) -> String {
    if !is_ours(current) {
        return current.to_string();
    }
    let snapshot = snapshot.filter(|s| !is_ours(s));
    let mut out = current.to_string();
    if toml_merge::top_level_raw_value(&out, DEFAULT_PROVIDER_KEY).as_deref()
        == Some(DEFAULT_PROVIDER_VALUE)
    {
        let original =
            snapshot.and_then(|s| toml_merge::top_level_raw_value(s, DEFAULT_PROVIDER_KEY));
        out = match original {
            Some(orig) => toml_merge::upsert_top_level_key(&out, DEFAULT_PROVIDER_KEY, &orig),
            None => toml_merge::remove_top_level_key(&out, DEFAULT_PROVIDER_KEY),
        };
    }
    toml_merge::remove_section(&out, PROVIDER_SECTION)
}

fn restore_official_config() -> Result<(), AppError> {
    let path = config_path()?;
    let current = cf::read_for_update(&path, codes::ATOMCODE_RESTORE_FAILED)?;
    let snapshot = cf::read_optional(&saved_config_path(), codes::ATOMCODE_RESTORE_FAILED)?;
    let restored = restore_content(&current, snapshot.as_deref());
    if restored == current {
        return Ok(());
    }
    cf::write_verified(
        &path,
        restored.as_bytes(),
        None,
        codes::ATOMCODE_RESTORE_FAILED,
    )
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AtomCodeConfigStatus {
    pub config_path: String,
    pub exists: bool,
    pub has_agentgate: bool,
    pub current_model: Option<String>,
    pub has_saved_official: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "AtomCodeApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "AtomCodeToggleResult")]
pub struct ToggleResult {
    pub success: bool,
    pub new_provider: String,
    pub config_path: String,
}

/// 当前模型:顶层 `model`,没有则取 `default_provider` 指向的 provider 段的 `model`。
fn current_model(content: &str) -> Option<String> {
    toml_merge::lookup_str(content, &["model"]).or_else(|| {
        let provider = toml_merge::lookup_str(content, &[DEFAULT_PROVIDER_KEY])?;
        toml_merge::lookup_str(content, &["providers", &provider, "model"])
    })
}

pub fn detect() -> AtomCodeConfigStatus {
    // 只读探测:家目录不可用时路径为空,按「不存在」展示。
    let path = config_path().unwrap_or_default();
    let path_str = path.to_string_lossy().to_string();
    let exists = path.is_file();

    let (has_agentgate, current_model) = if exists {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        (is_ours(&content), current_model(&content))
    } else {
        (false, None)
    };

    AtomCodeConfigStatus {
        config_path: path_str,
        exists,
        has_agentgate,
        current_model,
        has_saved_official: has_saved_official(),
    }
}

pub fn apply(host: &str, port: i64, _model: &str) -> Result<ApplyConfigResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let token = local_token::ensure_token()?;

    let path = config_path()?;
    let path_str = path.to_string_lossy().to_string();
    let warnings = Vec::new();

    // 读不出来(权限 / 编码)直接失败,绝不拿空内容覆盖。
    let existing = cf::read_for_update(&path, codes::ATOMCODE_CONFIG_WRITE_FAILED)?;

    // Save official config for toggle (skip if already agentgate)
    if !is_ours(&existing) {
        cf::backup_file(&path, &saved_config_path(), codes::ATOMCODE_SAVE_FAILED)?;
    }

    // Surgical merge: AtomCode users routinely keep their own
    // [providers.deepseek] / [providers.kimi] / [providers.openai] alongside us.
    // Old code overwrote the whole file and erased them — now we only touch
    // top-level `default_provider` and `[providers.agentgate]`.
    let mut merged =
        toml_merge::upsert_top_level_key(&existing, DEFAULT_PROVIDER_KEY, DEFAULT_PROVIDER_VALUE);
    let section_body = format!(
        "type = \"openai\"\napi_key = \"{token}\"\nmodel = \"{AGENTGATE_MODEL}\"\nbase_url = \"http://{host}:{port}/v1\"\ncontext_window = 1000000\n"
    );
    merged = toml_merge::upsert_section(&merged, PROVIDER_SECTION, &section_body);

    // config.toml 里带着 gateway token → 0600。
    cf::write_verified(
        &path,
        merged.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::ATOMCODE_CONFIG_WRITE_FAILED,
    )?;

    Ok(ApplyConfigResult {
        success: true,
        config_path: path_str,
        changed_keys: vec![PROVIDER_SECTION.to_string()],
        warnings,
    })
}

pub fn toggle(host: &str, port: i64, model: &str) -> Result<ToggleResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let status = detect();
    let config_path = config_path()?.to_string_lossy().to_string();

    if status.has_agentgate {
        restore_official_config()?;
        Ok(ToggleResult {
            success: true,
            new_provider: "official".to_string(),
            config_path,
        })
    } else {
        apply(host, port, model)?;
        Ok(ToggleResult {
            success: true,
            new_provider: "agentgate".to_string(),
            config_path,
        })
    }
}

pub fn open_config() -> Result<(), AppError> {
    let path = config_path()?;
    if !path.exists() {
        return Err(AppError::new(
            codes::ATOMCODE_CONFIG_NOT_FOUND,
            "AtomCode config.toml does not exist",
        ));
    }
    open::that(&path).map_err(|e| {
        AppError::new(
            codes::ATOMCODE_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

pub fn generate_snippet(host: &str, port: i64, _model: &str) -> String {
    let masked = cf::masked_token_for_snippet();
    format!(
        r#"default_provider = "agentgate"

[providers.agentgate]
type           = "openai"
api_key        = "{masked}"
model          = "{AGENTGATE_MODEL}"
base_url       = "http://{host}:{port}/v1"
context_window = 1000000"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn cp() -> PathBuf {
        config_path().unwrap()
    }

    const OFFICIAL: &str = "# notes\ndefault_provider = \"deepseek\"\n\n[providers.deepseek]\ntype = \"openai\"\napi_key = \"sk-real\"\nmodel = \"deepseek-chat\"\n";

    #[test]
    fn toggle_back_keeps_user_additions_and_restores_default_provider() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(cp().parent().unwrap()).unwrap();
        std::fs::write(cp(), OFFICIAL).unwrap();
        toggle("127.0.0.1", 9090, "m").unwrap();
        let mut cur = std::fs::read_to_string(cp()).unwrap();
        cur.push_str("\n[providers.kimi]\ntype = \"openai\"\napi_key = \"sk-kimi\"\n");
        std::fs::write(cp(), cur).unwrap();

        let result = toggle("127.0.0.1", 9090, "m").unwrap();
        assert_eq!(result.new_provider, "official");
        let content = std::fs::read_to_string(cp()).unwrap();
        let doc = content.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(doc["default_provider"].as_str(), Some("deepseek"));
        assert!(doc["providers"].get("agentgate").is_none(), "{content}");
        assert_eq!(
            doc["providers"]["kimi"]["api_key"].as_str(),
            Some("sk-kimi")
        );
        assert_eq!(
            doc["providers"]["deepseek"]["api_key"].as_str(),
            Some("sk-real")
        );
        assert!(content.starts_with("# notes\n"));
        assert!(!content.contains("ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn toggle_back_without_snapshot_removes_our_keys() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        apply("127.0.0.1", 9090, "m").unwrap();
        std::fs::remove_dir_all(saved_dir()).ok();
        toggle("127.0.0.1", 9090, "m").unwrap();
        let content = std::fs::read_to_string(cp()).unwrap();
        let doc = content.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(doc.get("default_provider").is_none());
        assert!(!content.contains("ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn detect_ignores_decoys_and_reads_model_section_aware() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(cp().parent().unwrap()).unwrap();
        std::fs::write(
            cp(),
            "# switched from muxlayer / agentgate, token was ag_local_old\nmodel_context_window = 128000\ndefault_provider = \"deepseek\"\n\n[providers.muxlayer-docs]\ntype = \"openai\"\n\n[providers.deepseek]\nmodel = \"deepseek-chat\"\n",
        )
        .unwrap();
        let status = detect();
        assert!(!status.has_agentgate);
        assert_eq!(status.current_model.as_deref(), Some("deepseek-chat"));
        cleanup(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_owner_only_and_refuses_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        apply("127.0.0.1", 9090, "m").unwrap();
        let mode = std::fs::metadata(cp()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let bad: &[u8] = b"default_provider = \"x\"\n\xff";
        std::fs::write(cp(), bad).unwrap();
        assert!(apply("127.0.0.1", 9090, "m").is_err());
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
        let result = apply("127.0.0.1", 9090, "gpt-5.5").unwrap();
        assert!(result.success);
        assert!(config_path().unwrap().exists());
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(content.contains("ag_local_"));
        assert!(content.contains("127.0.0.1:9090"));
        // surgical merge 后 key = value 不再带 padding 对齐空格
        assert!(content.contains(r#"model = "muxlayer""#));
        assert!(content.contains("[providers.agentgate]"));
        cleanup(&temp);
    }

    #[test]
    fn test_apply_preserves_user_other_providers() {
        // 回归测试：surgical merge 不应该擦掉用户已有的 [providers.*] 段
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        let user_config = r#"# my notes
default_provider = "deepseek"

[providers.deepseek]
type = "openai"
api_key = "sk-user-key"
model = "deepseek-chat"
base_url = "https://api.deepseek.com/v1"

[providers.kimi]
type = "openai"
api_key = "sk-kimi-key"
"#;
        std::fs::write(config_path().unwrap(), user_config).unwrap();

        apply("127.0.0.1", 9090, "gpt-5.5").unwrap();
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();

        // 我们的改动落地了
        assert!(content.contains("default_provider = \"agentgate\""));
        assert!(content.contains("[providers.agentgate]"));
        assert!(content.contains("ag_local_"));
        // 用户的其他 provider 完整保留
        assert!(content.contains("[providers.deepseek]"));
        assert!(content.contains("api_key = \"sk-user-key\""));
        assert!(content.contains("[providers.kimi]"));
        assert!(content.contains("api_key = \"sk-kimi-key\""));
        // 注释保留
        assert!(content.contains("# my notes"));
        cleanup(&temp);
    }

    #[test]
    fn test_toggle_saves_and_restores() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            config_path().unwrap(),
            r#"[providers.deepseek]
type = "openai"
api_key = "sk-real"
model = "deepseek-flash"
"#,
        )
        .unwrap();
        // Toggle to agentgate
        let result = toggle("127.0.0.1", 9090, "gpt-5.5").unwrap();
        assert_eq!(result.new_provider, "agentgate");
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(content.contains("ag_local_"));
        // Toggle back to official
        let result = toggle("127.0.0.1", 9090, "gpt-5.5").unwrap();
        assert_eq!(result.new_provider, "official");
        let content = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(content.contains("sk-real"));
        cleanup(&temp);
    }
}
