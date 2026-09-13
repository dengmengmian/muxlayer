use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::errors::codes;
use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;

/// apply 写入 settings.json `env` 的 key。restore 时:快照里有原值写回原值,没有就删除。
const OWNED_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];
/// apply 为避免冲突而删除的 key。restore 时只在当前不存在时从快照补回,
/// 用户 apply 之后自己加回来的值不动。
const REMOVED_ENV_KEYS: &[&str] = &["ANTHROPIC_AUTH_TOKEN"];

/// Directory where we save the user's original settings.json.
fn saved_dir() -> PathBuf {
    local_token::token_dir().join("claude_code_official")
}

fn saved_settings_path() -> PathBuf {
    saved_dir().join("settings.json")
}

/// Save original settings.json: restore 从这里取 apply 覆盖掉的原值。
fn save_official_settings() -> Result<(), AppError> {
    cf::backup_file(
        &settings_path()?,
        &saved_settings_path(),
        codes::CLAUDE_SAVE_FAILED,
    )?;
    Ok(())
}

/// Check if saved official settings exist.
pub fn has_saved_official() -> bool {
    saved_settings_path().exists()
}

/// settings 文档是否由 MuxLayer 接管:`env.ANTHROPIC_API_KEY` 是本地 token。
fn doc_is_ours(doc: &Value) -> bool {
    doc.get("env")
        .and_then(|e| e.get("ANTHROPIC_API_KEY"))
        .and_then(|v| v.as_str())
        .is_some_and(cf::is_local_token)
}

/// 「切回官方」的纯函数核心:只撤销 apply 写过的 env key,其它内容(hooks /
/// permissions / mcpServers / 用户 apply 之后新增的 env)全部保留。
/// 返回是否有改动。快照缺失时照样删除我们的 key。
pub(crate) fn restore_settings_doc(current: &mut Value, snapshot: Option<&Value>) -> bool {
    if !doc_is_ours(current) {
        return false;
    }
    let snapshot = snapshot.filter(|s| !doc_is_ours(s));
    let snap_env = snapshot
        .and_then(|s| s.get("env"))
        .and_then(|e| e.as_object());
    let snapshot_had_env = snap_env.is_some();

    let Some(env) = current.get_mut("env").and_then(|e| e.as_object_mut()) else {
        return false;
    };
    for key in OWNED_ENV_KEYS {
        match snap_env.and_then(|e| e.get(*key)) {
            Some(original) => {
                env.insert((*key).to_string(), original.clone());
            }
            None => {
                env.remove(*key);
            }
        }
    }
    for key in REMOVED_ENV_KEYS {
        if env.contains_key(*key) {
            continue;
        }
        if let Some(original) = snap_env.and_then(|e| e.get(*key)) {
            env.insert((*key).to_string(), original.clone());
        }
    }
    let env_empty = env.is_empty();
    if env_empty && !snapshot_had_env {
        if let Some(obj) = current.as_object_mut() {
            obj.remove("env");
        }
    }
    true
}

fn parse_settings(content: &str, code: &'static str) -> Result<Value, AppError> {
    if content.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(content)
        .map_err(|e| AppError::new(code, format!("Cannot parse settings.json: {e}")))
}

/// 读当前 settings.json + 官方备份,做 surgical restore 并写回。
fn restore_official_settings() -> Result<(), AppError> {
    let path = settings_path()?;
    let content = cf::read_for_update(&path, codes::CLAUDE_RESTORE_FAILED)?;
    let mut doc = parse_settings(&content, codes::CLAUDE_CONFIG_PARSE_ERROR)?;
    let snapshot = cf::read_optional(&saved_settings_path(), codes::CLAUDE_RESTORE_FAILED)?
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    if !restore_settings_doc(&mut doc, snapshot.as_ref()) {
        return Ok(());
    }
    let serialized = serde_json::to_string_pretty(&doc).map_err(|e| {
        AppError::new(
            codes::CLAUDE_RESTORE_FAILED,
            format!("Cannot serialize: {e}"),
        )
    })?;
    cf::write_verified(
        &path,
        serialized.as_bytes(),
        None,
        codes::CLAUDE_RESTORE_FAILED,
    )
}

const ENV_VARS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];

const SHELL_PROFILES: &[&str] = &[".zshrc", ".bashrc", ".bash_profile", ".profile"];

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ClaudeCodeEnvStatus {
    pub settings_path: String,
    pub settings_exists: bool,
    pub current_env: HashMap<String, String>,
    pub detected_profiles: Vec<ProfileDetection>,
    pub conflicts: Vec<String>,
    pub active_base_url: Option<String>,
    pub active_model: Option<String>,
    pub has_api_key: bool,
    pub has_auth_token: bool,
    pub has_agentgate: bool,
    pub auth_mode: String,
    pub recommendations: Vec<String>,
    pub has_saved_official: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ProfileDetection {
    pub path: String,
    pub exists: bool,
    pub has_anthropic_vars: bool,
    pub var_count: usize,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "ClaudeCodeApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub backup_path: Option<String>,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

// ===== CC 实时状态提醒(信箱文件方式,接收端见 app::cc_notify)=====

const CC_HOOK_MARKER: &str = "agentgate-cc-notify";

/// CC 状态信箱文件:~/.claude/agentgate-cc-notify.json(与 settings.json 同目录)。
pub fn cc_notify_file() -> Result<PathBuf, AppError> {
    Ok(settings_path()?.with_file_name("agentgate-cc-notify.json"))
}

/// 信箱临时文件:hook 先写它再原子 mv 成正式文件,避免接收端读到半截。
pub fn cc_notify_tmp_file() -> Result<PathBuf, AppError> {
    Ok(settings_path()?.with_file_name(".agentgate-cc-notify.tmp"))
}

/// POSIX sh 单引号转义:整体包在 `'…'` 里,内部的 `'` 写成 `'\''`。
fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// hook 命令:stdin 写临时文件后原子 mv 成信箱文件。路径做 POSIX 引号转义,
/// 用户名 / 目录含空格或引号时不会拆词。
fn cc_hook_command(tmp: &Path, target: &Path) -> String {
    let tmp = sh_single_quote(&tmp.to_string_lossy());
    let target = sh_single_quote(&target.to_string_lossy());
    format!("cat > {tmp} && mv {tmp} {target}")
}

fn entry_has_marker(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|inner| {
            inner.iter().any(|c| {
                c.get("command")
                    .and_then(|cmd| cmd.as_str())
                    .is_some_and(|x| x.contains(CC_HOOK_MARKER))
            })
        })
        .unwrap_or(false)
}

/// 把 CC 状态 hook 合并/移除到 settings 文档。多个状态事件写同一信箱,
/// 后端按 hook_event_name 区分 working/waiting/done。完全不碰 env。
/// 开启时先清掉旧版(路径未加引号)的同标记 hook,再写入 `command`。
fn apply_cc_hook_in_doc(settings: &mut Value, enabled: bool, command: &str) {
    let with_matcher = ["Notification", "PreToolUse"];
    let no_matcher = ["UserPromptSubmit", "Stop"];

    if enabled {
        let hooks = match settings
            .as_object_mut()
            .map(|o| o.entry("hooks").or_insert_with(|| serde_json::json!({})))
            .and_then(|h| h.as_object_mut())
        {
            Some(h) => h,
            None => return,
        };
        for ev in with_matcher.iter().chain(no_matcher.iter()) {
            let arr = match hooks
                .entry(*ev)
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
            {
                Some(a) => a,
                None => continue,
            };
            arr.retain(|e| !entry_has_marker(e));
            let entry = if with_matcher.contains(ev) {
                serde_json::json!({ "matcher": "", "hooks": [{ "type": "command", "command": command }] })
            } else {
                serde_json::json!({ "hooks": [{ "type": "command", "command": command }] })
            };
            arr.push(entry);
        }
    } else if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for ev in with_matcher.iter().chain(no_matcher.iter()) {
            if let Some(arr) = hooks.get_mut(*ev).and_then(|n| n.as_array_mut()) {
                arr.retain(|e| !entry_has_marker(e));
            }
        }
    }
}

/// 开/关 CC 状态提醒:把 hook 写入/移除 settings.json(完全不碰 env)。
///
/// Windows:仓库内没有 Claude Code 在 Windows 上用哪个 shell 执行 hook 的可靠依据
/// (cmd / PowerShell 下 `cat` / `mv` 与单引号语义都不成立),写进去只会是坏 hook,
/// 因此开启时明确返回不支持;关闭(清理旧 hook)仍然允许。
pub fn set_cc_hook(enabled: bool) -> Result<(), String> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    if enabled && cfg!(windows) {
        return Err("Claude Code 状态提醒 hook 暂不支持 Windows".to_string());
    }
    let path = settings_path().map_err(|e| e.message)?;
    let content = crate::fsutil::read_config_or_empty(&path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let mut settings: Value = if content.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&content).map_err(|e| format!("settings.json 解析失败: {e}"))?
    };
    let command = cc_hook_command(
        &cc_notify_tmp_file().map_err(|e| e.message)?,
        &cc_notify_file().map_err(|e| e.message)?,
    );
    apply_cc_hook_in_doc(&mut settings, enabled, &command);
    let serialized = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    cf::write_verified(
        &path,
        serialized.as_bytes(),
        None,
        codes::CLAUDE_CONFIG_WRITE_FAILED,
    )
    .map_err(|e| e.message)
}

/// 菜单勾选态:settings.json 里任一 CC 事件存在我们的 hook 即为开。
pub fn cc_hook_enabled() -> bool {
    let Ok(path) = settings_path() else {
        return false;
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    let Some(hooks) = v.get("hooks") else {
        return false;
    };
    ["Notification", "PreToolUse", "UserPromptSubmit", "Stop"]
        .iter()
        .any(|ev| {
            hooks
                .get(*ev)
                .and_then(|n| n.as_array())
                .is_some_and(|arr| arr.iter().any(entry_has_marker))
        })
}

pub fn settings_path() -> Result<PathBuf, AppError> {
    Ok(crate::fsutil::home_dir()?
        .join(".claude")
        .join("settings.json"))
}

pub fn snapshot_paths() -> Vec<(&'static str, PathBuf)> {
    settings_path()
        .map(|p| vec![("settings.json", p)])
        .unwrap_or_default()
}

pub fn detect_env() -> ClaudeCodeEnvStatus {
    // 只读探测:家目录不可用时路径为空,按「不存在」展示。
    let home = crate::fsutil::home_dir().unwrap_or_default();

    let sp = settings_path().unwrap_or_default();
    let sp_str = sp.to_string_lossy().to_string();
    let settings_exists = sp.is_file();

    // 结构化识别:只看 env.ANTHROPIC_API_KEY 是否是本地 token,
    // permissions / MCP 名 / 其它字段里出现 ag_local_ 文本不算接入。
    let has_agentgate = settings_exists
        && std::fs::read_to_string(&sp)
            .ok()
            .and_then(|c| serde_json::from_str::<Value>(&c).ok())
            .is_some_and(|doc| doc_is_ours(&doc));

    // Current process env (masked)
    let mut current_env = HashMap::new();
    for var in ENV_VARS {
        if let Ok(val) = std::env::var(var) {
            let masked = if var.contains("KEY") || var.contains("TOKEN") {
                mask_value(&val)
            } else {
                val
            };
            current_env.insert(var.to_string(), masked);
        }
    }

    // Shell profiles
    let mut detected_profiles = Vec::new();
    for profile in SHELL_PROFILES {
        let path = home.join(profile);
        let exists = path.exists();
        let (has_vars, var_count) = if exists {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let count = ENV_VARS.iter().filter(|v| content.contains(*v)).count();
            (count > 0, count)
        } else {
            (false, 0)
        };
        detected_profiles.push(ProfileDetection {
            path: path.to_string_lossy().to_string(),
            exists,
            has_anthropic_vars: has_vars,
            var_count,
        });
    }

    // Conflicts
    let mut conflicts = Vec::new();
    let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let has_auth_token = std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok();
    if has_api_key && has_auth_token {
        conflicts.push("BOTH_API_KEY_AND_AUTH_TOKEN: Both are set. Claude Code may use AUTH_TOKEN preferentially.".to_string());
    }

    let active_base_url = std::env::var("ANTHROPIC_BASE_URL").ok();
    let active_model = std::env::var("ANTHROPIC_MODEL").ok();

    let mut recommendations = Vec::new();
    if !has_api_key && !has_auth_token && !has_agentgate {
        recommendations
            .push("No credentials found. Apply MuxLayer config to set up Claude Code.".to_string());
    }
    if has_api_key && has_auth_token {
        recommendations.push(
            "Remove one of ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN to avoid conflicts."
                .to_string(),
        );
    }

    ClaudeCodeEnvStatus {
        settings_path: sp_str,
        settings_exists,
        current_env,
        detected_profiles,
        conflicts,
        active_base_url,
        active_model,
        has_api_key,
        has_auth_token,
        has_agentgate,
        auth_mode: "inline_token".to_string(),
        recommendations,
        has_saved_official: has_saved_official(),
    }
}

pub fn apply_config(host: &str, port: i64, model: &str) -> Result<ApplyConfigResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let token = local_token::ensure_token()?;

    let sp = settings_path()?;
    let sp_str = sp.to_string_lossy().to_string();
    let mut warnings = Vec::new();

    // 读不出来(权限 / 编码)直接失败,绝不拿空内容覆盖。
    let existing = cf::read_for_update(&sp, codes::CLAUDE_CONFIG_WRITE_FAILED)?;
    let mut doc = parse_settings(&existing, codes::CLAUDE_CONFIG_PARSE_ERROR)?;

    // Save official settings for toggle restore (skip if already agentgate)
    // settings.json 不存在时 backup_file 会删除旧备份,避免 restore 复活陈旧值。
    if !doc_is_ours(&doc) {
        save_official_settings()?;
    }

    if doc.get("env").is_none() {
        doc["env"] = serde_json::json!({});
    }

    let env = doc["env"].as_object_mut().ok_or_else(|| {
        AppError::new(
            codes::CLAUDE_CONFIG_PARSE_ERROR,
            "env field is not an object",
        )
    })?;

    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        serde_json::json!(format!("http://{host}:{port}")),
    );
    env.insert("ANTHROPIC_API_KEY".to_string(), serde_json::json!(token));
    env.insert("ANTHROPIC_MODEL".to_string(), serde_json::json!(model));
    env.insert(
        "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
        serde_json::json!(model),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
        serde_json::json!(model),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
        serde_json::json!(model),
    );

    if env.remove("ANTHROPIC_AUTH_TOKEN").is_some() {
        warnings.push(
            "Removed ANTHROPIC_AUTH_TOKEN to avoid conflict with ANTHROPIC_API_KEY".to_string(),
        );
    }

    let new_content = serde_json::to_string_pretty(&doc).map_err(|e| {
        AppError::new(
            codes::CLAUDE_CONFIG_WRITE_FAILED,
            format!("Cannot serialize: {e}"),
        )
    })?;

    // settings.json 里带着 gateway token → 0600。
    cf::write_verified(
        &sp,
        new_content.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::CLAUDE_CONFIG_WRITE_FAILED,
    )?;

    if has_saved_official() {
        warnings.push("Original settings saved. Use toggle to switch back.".to_string());
    }

    let changed_keys = vec![
        "ANTHROPIC_BASE_URL".to_string(),
        "ANTHROPIC_API_KEY".to_string(),
        "ANTHROPIC_MODEL".to_string(),
    ];

    Ok(ApplyConfigResult {
        success: true,
        config_path: sp_str,
        backup_path: None,
        changed_keys,
        warnings,
    })
}

/// Toggle between AgentGate and official config.
pub fn toggle_provider(host: &str, port: i64, model: &str) -> Result<ToggleResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let status = detect_env();
    let config_path = settings_path()?.to_string_lossy().to_string();

    if status.has_agentgate {
        // Switching TO official: surgically undo only what apply wrote.
        restore_official_settings()?;
        Ok(ToggleResult {
            success: true,
            new_provider: "official".to_string(),
            config_path,
        })
    } else {
        // Switching TO agentgate: save current, apply agentgate
        apply_config(host, port, model)?;
        Ok(ToggleResult {
            success: true,
            new_provider: "agentgate".to_string(),
            config_path,
        })
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "ClaudeCodeToggleResult")]
pub struct ToggleResult {
    pub success: bool,
    pub new_provider: String,
    pub config_path: String,
}

pub fn open_config() -> Result<(), AppError> {
    let sp = settings_path()?;
    if !sp.exists() {
        return Err(AppError::new(
            codes::CLAUDE_CONFIG_NOT_FOUND,
            "Claude Code settings.json does not exist",
        ));
    }
    open::that(&sp).map_err(|e| {
        AppError::new(
            codes::CLAUDE_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

pub fn generate_env_snippet(host: &str, port: i64, model: &str) -> String {
    let masked = cf::masked_token_for_snippet();
    format!(
        r#"export ANTHROPIC_BASE_URL="http://{host}:{port}"
export ANTHROPIC_API_KEY="{masked}"
export ANTHROPIC_MODEL="{model}"
export ANTHROPIC_DEFAULT_SONNET_MODEL="{model}"
export ANTHROPIC_DEFAULT_OPUS_MODEL="{model}"
export ANTHROPIC_DEFAULT_HAIKU_MODEL="{model}"
unset ANTHROPIC_AUTH_TOKEN"#
    )
}

pub(crate) fn mask_value(val: &str) -> String {
    if val.len() <= 8 {
        "*".repeat(val.len())
    } else {
        let prefix = &val[..4];
        let suffix = &val[val.len() - 4..];
        format!("{prefix}****{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn sp() -> PathBuf {
        settings_path().unwrap()
    }

    fn read_json(path: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    const OFFICIAL_SETTINGS: &str = r#"{"env":{"ANTHROPIC_BASE_URL":"https://corp-proxy.example.com","ANTHROPIC_AUTH_TOKEN":"corp-token","DISABLE_TELEMETRY":"1"},"permissions":{"allow":["Bash(ls)"]}}"#;

    fn user_edits_after_apply() {
        let mut doc = read_json(&sp());
        doc["hooks"] =
            serde_json::json!({"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]});
        doc["env"]["MY_NEW_VAR"] = serde_json::json!("keep");
        std::fs::write(sp(), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    }

    #[test]
    fn toggle_back_keeps_user_additions_and_restores_overwritten_env() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(sp().parent().unwrap()).unwrap();
        std::fs::write(sp(), OFFICIAL_SETTINGS).unwrap();
        apply_config("127.0.0.1", 9090, "model").unwrap();
        user_edits_after_apply();

        let result = toggle_provider("127.0.0.1", 9090, "model").unwrap();
        assert_eq!(result.new_provider, "official");
        let doc = read_json(&sp());
        // 用户 apply 之后的新增保留
        assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
        assert_eq!(doc["env"]["MY_NEW_VAR"], "keep");
        assert_eq!(doc["permissions"]["allow"][0], "Bash(ls)");
        // 被覆盖 / 删除的原值回来
        assert_eq!(
            doc["env"]["ANTHROPIC_BASE_URL"],
            "https://corp-proxy.example.com"
        );
        assert_eq!(doc["env"]["ANTHROPIC_AUTH_TOKEN"], "corp-token");
        assert_eq!(doc["env"]["DISABLE_TELEMETRY"], "1");
        // 我们写的 key 消失
        for k in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
        ] {
            assert!(doc["env"].get(k).is_none(), "{k} should be removed");
        }
        assert!(!detect_env().has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn toggle_back_without_snapshot_removes_only_our_keys() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        apply_config("127.0.0.1", 9090, "model").unwrap();
        user_edits_after_apply();
        std::fs::remove_dir_all(saved_dir()).ok();
        let result = toggle_provider("127.0.0.1", 9090, "model").unwrap();
        assert_eq!(result.new_provider, "official");
        let doc = read_json(&sp());
        assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
        assert_eq!(doc["env"]["MY_NEW_VAR"], "keep");
        assert!(doc["env"].get("ANTHROPIC_API_KEY").is_none());
        assert!(doc["env"].get("ANTHROPIC_BASE_URL").is_none());
        cleanup(&temp);
    }

    /// settings.json 不存在时 apply 也要清掉旧备份(与 cf::backup_file 一致),
    /// 否则切回官方会把几个月前的 BASE_URL / key 复活。
    #[test]
    fn apply_without_settings_drops_stale_backup_so_restore_does_not_resurrect_it() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(saved_dir()).unwrap();
        std::fs::write(
            saved_settings_path(),
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://stale.example","ANTHROPIC_API_KEY":"sk-stale","ANTHROPIC_AUTH_TOKEN":"stale-token"}}"#,
        )
        .unwrap();
        assert!(!sp().exists());
        apply_config("127.0.0.1", 9090, "model").unwrap();
        let result = toggle_provider("127.0.0.1", 9090, "model").unwrap();
        assert_eq!(result.new_provider, "official");
        let raw = std::fs::read_to_string(sp()).unwrap();
        assert!(!raw.contains("stale"), "stale backup resurrected:\n{raw}");
        assert!(!raw.contains("ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn detect_ignores_token_text_outside_env_api_key() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(sp().parent().unwrap()).unwrap();
        std::fs::write(
            sp(),
            r#"{"permissions":{"allow":["Bash(echo ag_local_decoy)"]},"mcpServers":{"agentgate-docs":{}}}"#,
        )
        .unwrap();
        assert!(!detect_env().has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn apply_refuses_to_overwrite_unreadable_settings() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(sp().parent().unwrap()).unwrap();
        let original: &[u8] = b"{\"env\":{}} \xff";
        std::fs::write(sp(), original).unwrap();
        assert!(apply_config("127.0.0.1", 9090, "model").is_err());
        assert_eq!(std::fs::read(sp()).unwrap(), original);
        cleanup(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_settings_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        apply_config("127.0.0.1", 9090, "model").unwrap();
        let mode = std::fs::metadata(sp()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        cleanup(&temp);
    }

    #[test]
    fn cc_hook_command_quotes_paths_for_posix_sh() {
        let cmd = cc_hook_command(
            std::path::Path::new("/Users/Jane Doe/.claude/.agentgate-cc-notify.tmp"),
            std::path::Path::new("/Users/Jane Doe/.claude/agentgate-cc-notify.json"),
        );
        assert_eq!(
            cmd,
            "cat > '/Users/Jane Doe/.claude/.agentgate-cc-notify.tmp' && mv '/Users/Jane Doe/.claude/.agentgate-cc-notify.tmp' '/Users/Jane Doe/.claude/agentgate-cc-notify.json'"
        );
        let tricky = cc_hook_command(
            std::path::Path::new("/tmp/it's/a.tmp"),
            std::path::Path::new("/tmp/it's/agentgate-cc-notify.json"),
        );
        assert!(tricky.contains("'/tmp/it'\\''s/a.tmp'"), "got {tricky}");
    }

    #[test]
    fn cc_hook_enable_replaces_stale_unquoted_hook() {
        let mut doc = serde_json::json!({"hooks":{"Stop":[
            {"hooks":[{"type":"command","command":"cat > /old/.agentgate-cc-notify.tmp && mv /old/.agentgate-cc-notify.tmp /old/agentgate-cc-notify.json"}]},
            {"hooks":[{"type":"command","command":"say done"}]}
        ]}});
        apply_cc_hook_in_doc(
            &mut doc,
            true,
            "cat > 'x' && mv 'x' 'agentgate-cc-notify.json'",
        );
        let stop = doc["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert!(stop.iter().any(|e| e["hooks"][0]["command"] == "say done"));
        assert!(stop
            .iter()
            .any(|e| e["hooks"][0]["command"] == "cat > 'x' && mv 'x' 'agentgate-cc-notify.json'"));
    }

    #[test]
    fn test_generate_env_snippet_format() {
        let snippet = generate_env_snippet("127.0.0.1", 9090, "claude-sonnet");
        assert!(snippet.contains("export ANTHROPIC_BASE_URL=\"http://127.0.0.1:9090\""));
        assert!(snippet.contains("export ANTHROPIC_MODEL=\"claude-sonnet\""));
        assert!(snippet.contains("unset ANTHROPIC_AUTH_TOKEN"));
    }

    #[test]
    fn test_mask_value_short() {
        assert_eq!(mask_value("abc"), "***");
        assert_eq!(mask_value("abcdefgh"), "********");
    }

    #[test]
    fn test_mask_value_long() {
        let val = "sk-abcdefghijklmnopqrstuvwxyz";
        let masked = mask_value(val);
        assert!(masked.starts_with("sk-a"));
        assert!(masked.ends_with("wxyz"));
        assert!(masked.contains("****"));
    }

    #[test]
    fn test_apply_config_creates_new_file() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let result = apply_config("127.0.0.1", 9090, "claude-model").unwrap();
        assert!(result.success);
        assert!(settings_path().unwrap().exists());
        let content = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(content.contains("ANTHROPIC_BASE_URL"));
        assert!(content.contains("ANTHROPIC_API_KEY"));
        assert!(content.contains("claude-model"));
        cleanup(&temp);
    }

    #[test]
    fn test_apply_saves_and_toggle_restores() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        // Pre-create settings with official config
        std::fs::create_dir_all(settings_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            settings_path().unwrap(),
            r#"{"env":{"ANTHROPIC_API_KEY":"sk-real"}}"#,
        )
        .unwrap();
        // Apply agentgate
        apply_config("127.0.0.1", 9090, "model").unwrap();
        let content = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(content.contains("ag_local_"));
        assert!(has_saved_official());
        // Toggle back to official
        let result = toggle_provider("127.0.0.1", 9090, "model").unwrap();
        assert_eq!(result.new_provider, "official");
        let content = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(content.contains("sk-real"));
        // Toggle to agentgate again
        let result = toggle_provider("127.0.0.1", 9090, "model").unwrap();
        assert_eq!(result.new_provider, "agentgate");
        let content = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(content.contains("ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn test_apply_config_removes_auth_token() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(settings_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            settings_path().unwrap(),
            r#"{"env":{"ANTHROPIC_AUTH_TOKEN":"secret"}}"#,
        )
        .unwrap();
        let result = apply_config("127.0.0.1", 9090, "model").unwrap();
        assert!(result.success);
        assert!(result
            .warnings
            .iter()
            .any(|w| w.contains("ANTHROPIC_AUTH_TOKEN")));
        let content = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(!content.contains("ANTHROPIC_AUTH_TOKEN"));
        cleanup(&temp);
    }
}
