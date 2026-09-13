use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use crate::errors::codes;
use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;

/// apply 在 `~/.gemini/.env` 里接管的 key。
const ENV_API_KEY: &str = "GEMINI_API_KEY";
const ENV_BASE_URL: &str = "GOOGLE_GEMINI_BASE_URL";
const OWNED_ENV_KEYS: &[&str] = &[ENV_API_KEY, ENV_BASE_URL];
/// 旧版 apply 整份覆盖 .env 时写的头注释;restore 时顺手清掉。
const LEGACY_ENV_HEADER: &str = "# MuxLayer configuration — do not edit manually";
const API_KEY_AUTH_TYPE: &str = "gemini-api-key";

/// Gemini CLI settings.json path
pub fn settings_path() -> Result<PathBuf, AppError> {
    Ok(crate::fsutil::home_dir()?
        .join(".gemini")
        .join("settings.json"))
}

/// Gemini CLI .env file path (Gemini CLI loads env vars from ~/.gemini/.env)
pub fn env_file_path() -> Result<PathBuf, AppError> {
    Ok(crate::fsutil::home_dir()?.join(".gemini").join(".env"))
}

pub fn snapshot_paths() -> Vec<(&'static str, PathBuf)> {
    [
        ("settings.json", settings_path()),
        (".env", env_file_path()),
    ]
    .into_iter()
    .filter_map(|(name, p)| p.ok().map(|p| (name, p)))
    .collect()
}

/// Directory where we save the user's original settings.json / .env.
fn saved_dir() -> PathBuf {
    local_token::token_dir().join("gemini_cli_official")
}

fn saved_settings_path() -> PathBuf {
    saved_dir().join("settings.json")
}

fn saved_env_path() -> PathBuf {
    saved_dir().join(".env")
}

/// 备份 apply 之前的 settings.json / .env(restore 从这里取被覆盖的原值)。
fn save_official_settings() -> Result<(), AppError> {
    cf::backup_file(
        &settings_path()?,
        &saved_settings_path(),
        codes::GEMINI_SAVE_FAILED,
    )?;
    cf::backup_file(
        &env_file_path()?,
        &saved_env_path(),
        codes::GEMINI_SAVE_FAILED,
    )?;
    Ok(())
}

pub fn has_saved_official() -> bool {
    saved_settings_path().exists()
}

// ── .env 行级编辑 ──────────────────────────────────────────────

/// `KEY=value` / `export KEY=value` 行的 key;注释、空行、非法 key 返回 None。
fn env_line_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let key = trimmed[..trimmed.find('=')?].trim();
    let mut chars = key.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(key)
}

/// `.env` 中 `key` 首次出现的整行原文。
fn env_line<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    content.lines().find(|l| env_line_key(l) == Some(key))
}

/// `.env` 中 `key` 的值(去掉首尾空白与成对引号)。
fn env_value(content: &str, key: &str) -> Option<String> {
    let line = env_line(content, key)?;
    let value = line[line.find('=')? + 1..].trim();
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value);
    Some(unquoted.to_string())
}

/// 逐行 upsert:已有 key 原位替换(重复出现的后续行删除),缺失的追加到末尾;
/// 其它行(注释、用户变量)原样保留。`lines` 是完整的 `KEY=value` 行。
fn upsert_env_lines(content: &str, lines: &[(&str, String)]) -> String {
    let mut out = String::new();
    let mut written: Vec<&str> = Vec::new();
    for line in content.lines() {
        if let Some((key, new_line)) =
            env_line_key(line).and_then(|k| lines.iter().find(|(owned, _)| *owned == k))
        {
            if !written.contains(key) {
                out.push_str(new_line);
                out.push('\n');
                written.push(key);
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    for (key, new_line) in lines {
        if !written.contains(key) {
            out.push_str(new_line);
            out.push('\n');
        }
    }
    out
}

/// 删除 `keys` 对应的所有行,其它行原样保留。
fn remove_env_keys(content: &str, keys: &[&str]) -> String {
    let mut out = String::new();
    for line in content.lines() {
        if env_line_key(line).is_some_and(|k| keys.contains(&k)) || line == LEGACY_ENV_HEADER {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn env_is_ours(content: &str) -> bool {
    env_value(content, ENV_API_KEY).is_some_and(|v| cf::is_local_token(&v))
}

/// 旧版把 token 写进 settings.json 的 `env` 字段;仍按接入识别以便切回。
fn legacy_settings_env_is_ours(doc: &Value) -> bool {
    doc.pointer("/env/GEMINI_API_KEY")
        .and_then(|v| v.as_str())
        .is_some_and(cf::is_local_token)
}

/// 「切回官方」.env 部分:我们的 key 有快照原值就写回原行,否则删除。
fn restore_env_content(current: &str, snapshot: Option<&str>) -> String {
    if !env_is_ours(current) {
        return current.to_string();
    }
    let snapshot = snapshot.filter(|s| !env_is_ours(s));
    if let Some(backup) = snapshot.filter(|_| current.lines().any(|l| l == LEGACY_ENV_HEADER)) {
        return restore_legacy_env_content(current, backup);
    }
    let restores: Vec<(&str, String)> = OWNED_ENV_KEYS
        .iter()
        .filter_map(|k| {
            snapshot
                .and_then(|s| env_line(s, k))
                .map(|l| (*k, l.to_string()))
        })
        .collect();
    let removals: Vec<&str> = OWNED_ENV_KEYS
        .iter()
        .copied()
        .filter(|k| !restores.iter().any(|(r, _)| r == k))
        .collect();
    let without = remove_env_keys(current, &removals);
    if restores.is_empty() {
        without
    } else {
        upsert_env_lines(&without, &restores)
    }
}

/// 旧版(≤2.0.5)apply 整份覆盖 .env 并把原文件整份备份,当前文件里只剩头注释 +
/// 我们的两行。此时备份才是用户的完整原文件:以备份为基底(去掉头注释),再把
/// apply 之后用户自己往当前文件里加的行(非头注释、非我们接管的 key)合并进去。
fn restore_legacy_env_content(current: &str, backup: &str) -> String {
    let base = remove_env_keys(backup, &[]);
    let mut keyed: Vec<(&str, String)> = Vec::new();
    let mut extra = String::new();
    for line in current.lines() {
        if line == LEGACY_ENV_HEADER {
            continue;
        }
        match env_line_key(line) {
            Some(k) if OWNED_ENV_KEYS.contains(&k) => {}
            Some(k) => keyed.push((k, line.to_string())),
            None if line.trim().is_empty() || base.lines().any(|b| b == line) => {}
            None => {
                extra.push_str(line);
                extra.push('\n');
            }
        }
    }
    let mut out = if keyed.is_empty() {
        base
    } else {
        upsert_env_lines(&base, &keyed)
    };
    out.push_str(&extra);
    out
}

/// 在 JSON 对象里按路径写回快照原值,或删除该 key 并清理因此变空、且快照里
/// 本来不存在的父对象。
fn restore_json_path(doc: &mut Value, snapshot: Option<&Value>, path: &[&str]) {
    let pointer = format!("/{}", path.join("/"));
    match snapshot.and_then(|s| s.pointer(&pointer)) {
        Some(original) => {
            let mut cur = &mut *doc;
            for seg in &path[..path.len() - 1] {
                if !cur.get(*seg).is_some_and(|v| v.is_object()) {
                    cur[*seg] = serde_json::json!({});
                }
                cur = &mut cur[*seg];
            }
            cur[path[path.len() - 1]] = original.clone();
        }
        None => {
            for depth in (1..=path.len()).rev() {
                let parent_ptr = format!("/{}", path[..depth - 1].join("/"));
                let parent_ptr = if depth == 1 { "" } else { parent_ptr.as_str() };
                let Some(parent) = doc.pointer_mut(parent_ptr).and_then(|v| v.as_object_mut())
                else {
                    return;
                };
                let key = path[depth - 1];
                let removable = depth == path.len()
                    || parent
                        .get(key)
                        .and_then(|v| v.as_object())
                        .is_some_and(|o| o.is_empty());
                let existed_in_snapshot = snapshot
                    .and_then(|s| s.pointer(&format!("/{}", path[..depth].join("/"))))
                    .is_some();
                if removable && !existed_in_snapshot {
                    parent.remove(key);
                } else {
                    return;
                }
            }
        }
    }
}

/// 「切回官方」settings.json 部分:只撤销 apply 写的 `model.name` 与
/// `security.auth.selectedType`(后者仅当仍是 gemini-api-key),以及把 apply
/// 删掉的顶层 `env` 从快照补回。其它字段(mcpServers 等)保留。
fn restore_settings_doc(doc: &mut Value, snapshot: Option<&Value>) {
    let snapshot = snapshot.filter(|s| !legacy_settings_env_is_ours(s));
    if doc.pointer("/model/name").is_some() {
        restore_json_path(doc, snapshot, &["model", "name"]);
    }
    if doc
        .pointer("/security/auth/selectedType")
        .and_then(|v| v.as_str())
        == Some(API_KEY_AUTH_TYPE)
    {
        restore_json_path(doc, snapshot, &["security", "auth", "selectedType"]);
    }
    if legacy_settings_env_is_ours(doc) {
        if let Some(obj) = doc.as_object_mut() {
            obj.remove("env");
        }
    }
    if doc.get("env").is_none() {
        if let (Some(env), Some(obj)) = (snapshot.and_then(|s| s.get("env")), doc.as_object_mut()) {
            obj.insert("env".to_string(), env.clone());
        }
    }
}

fn parse_settings(content: &str) -> Result<Value, AppError> {
    if content.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(content).map_err(|e| {
        AppError::new(
            codes::GEMINI_CONFIG_PARSE_ERROR,
            format!("Cannot parse settings.json: {e}"),
        )
    })
}

fn restore_official_settings() -> Result<(), AppError> {
    // settings.json
    let sp = settings_path()?;
    let content = cf::read_for_update(&sp, codes::GEMINI_RESTORE_FAILED)?;
    if !content.is_empty() {
        let mut doc = parse_settings(&content)?;
        let snapshot = cf::read_optional(&saved_settings_path(), codes::GEMINI_RESTORE_FAILED)?
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let before = doc.clone();
        restore_settings_doc(&mut doc, snapshot.as_ref());
        if doc != before {
            let serialized = serde_json::to_string_pretty(&doc).map_err(|e| {
                AppError::new(
                    codes::GEMINI_RESTORE_FAILED,
                    format!("Cannot serialize: {e}"),
                )
            })?;
            cf::write_verified(
                &sp,
                format!("{serialized}\n").as_bytes(),
                None,
                codes::GEMINI_RESTORE_FAILED,
            )?;
        }
    }

    // .env
    let env_path = env_file_path()?;
    let env_content = cf::read_for_update(&env_path, codes::GEMINI_RESTORE_FAILED)?;
    let env_snapshot = cf::read_optional(&saved_env_path(), codes::GEMINI_RESTORE_FAILED)?;
    let restored = restore_env_content(&env_content, env_snapshot.as_deref());
    if restored != env_content {
        if restored.trim().is_empty() && env_snapshot.is_none() {
            // .env 是 apply 新建的,撤销后只剩空白 → 删除,回到 apply 前「没有 .env」。
            std::fs::remove_file(&env_path).map_err(|e| {
                AppError::new(
                    codes::GEMINI_RESTORE_FAILED,
                    format!("Cannot remove .env: {e}"),
                )
            })?;
        } else {
            cf::write_verified(
                &env_path,
                restored.as_bytes(),
                None,
                codes::GEMINI_RESTORE_FAILED,
            )?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct GeminiCliConfigStatus {
    pub config_path: String,
    pub exists: bool,
    pub has_agentgate: bool,
    pub current_model: Option<String>,
    pub has_saved_official: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "GeminiCliApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "GeminiCliToggleResult")]
pub struct ToggleResult {
    pub success: bool,
    pub new_provider: String,
    pub config_path: String,
}

pub fn detect() -> GeminiCliConfigStatus {
    // 只读探测:家目录不可用时路径为空,按「不存在」展示。
    let sp = settings_path().unwrap_or_default();
    let sp_str = sp.to_string_lossy().to_string();
    let exists = sp.is_file();

    let (has_agentgate, current_model) = if exists {
        let content = std::fs::read_to_string(&sp).unwrap_or_default();
        let doc = serde_json::from_str::<Value>(&content).ok();
        let env_content = env_file_path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        // 结构化识别:.env 的 GEMINI_API_KEY(或旧版 settings.env)是本地 token。
        let has_ag =
            env_is_ours(&env_content) || doc.as_ref().is_some_and(legacy_settings_env_is_ours);
        let model = doc
            .as_ref()
            .and_then(|v| v.get("model")?.get("name")?.as_str().map(String::from));
        (has_ag, model)
    } else {
        (false, None)
    };

    GeminiCliConfigStatus {
        config_path: sp_str,
        exists,
        has_agentgate,
        current_model,
        has_saved_official: has_saved_official(),
    }
}

pub fn apply(host: &str, port: i64, model: &str) -> Result<ApplyConfigResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let token = local_token::ensure_token()?;

    let sp = settings_path()?;
    let env_path = env_file_path()?;
    let sp_str = sp.to_string_lossy().to_string();
    let mut warnings = Vec::new();

    // 读不出来(权限 / 编码)直接失败,绝不拿空内容覆盖。
    let existing = cf::read_for_update(&sp, codes::GEMINI_CONFIG_WRITE_FAILED)?;
    let existing_env = cf::read_for_update(&env_path, codes::GEMINI_CONFIG_WRITE_FAILED)?;
    let mut doc = parse_settings(&existing)?;

    // Save official settings for toggle (skip if already agentgate)
    if !env_is_ours(&existing_env) && !legacy_settings_env_is_ours(&doc) {
        save_official_settings()?;
    }

    // Set model
    if !doc.get("model").is_some_and(|m| m.is_object()) {
        doc["model"] = serde_json::json!({});
    }
    doc["model"]["name"] = serde_json::json!(model);

    // Remove stale "env" field if present (Gemini CLI doesn't use it)
    if let Some(obj) = doc.as_object_mut() {
        obj.remove("env");
    }

    // Set auth type to gemini-api-key (required for GEMINI_API_KEY + GOOGLE_GEMINI_BASE_URL to work)
    // Without this, Gemini CLI may use OAuth and ignore .env completely
    doc["security"]["auth"]["selectedType"] = serde_json::json!(API_KEY_AUTH_TYPE);

    let new_content = serde_json::to_string_pretty(&doc).map_err(|e| {
        AppError::new(
            codes::GEMINI_CONFIG_WRITE_FAILED,
            format!("Cannot serialize: {e}"),
        )
    })?;
    cf::write_verified(
        &sp,
        format!("{new_content}\n").as_bytes(),
        None,
        codes::GEMINI_CONFIG_WRITE_FAILED,
    )?;

    // ~/.gemini/.env:只 upsert 我们的两行,用户的 GOOGLE_CLOUD_PROJECT 等保留;
    // 含 gateway token → 0600。
    let env_content = upsert_env_lines(
        &existing_env,
        &[
            (ENV_API_KEY, format!("{ENV_API_KEY}={token}")),
            (ENV_BASE_URL, format!("{ENV_BASE_URL}=http://{host}:{port}")),
        ],
    );
    cf::write_verified(
        &env_path,
        env_content.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::GEMINI_CONFIG_WRITE_FAILED,
    )?;

    if has_saved_official() {
        warnings.push("Original settings saved. Use toggle to switch back.".to_string());
    }

    Ok(ApplyConfigResult {
        success: true,
        config_path: sp_str,
        changed_keys: vec![
            "model.name".to_string(),
            ".env GEMINI_API_KEY".to_string(),
            ".env GOOGLE_GEMINI_BASE_URL".to_string(),
        ],
        warnings,
    })
}

pub fn toggle(host: &str, port: i64, model: &str) -> Result<ToggleResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let status = detect();
    let config_path = settings_path()?.to_string_lossy().to_string();

    if status.has_agentgate {
        restore_official_settings()?;
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
    let sp = settings_path()?;
    if !sp.exists() {
        return Err(AppError::new(
            codes::GEMINI_CONFIG_NOT_FOUND,
            "Gemini CLI settings.json does not exist",
        ));
    }
    open::that(&sp).map_err(|e| {
        AppError::new(
            codes::GEMINI_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

pub fn generate_snippet(host: &str, port: i64, model: &str) -> String {
    let masked = cf::masked_token_for_snippet();
    format!(
        r#"export GEMINI_API_KEY="{masked}"
export GOOGLE_GEMINI_BASE_URL="http://{host}:{port}"
# Model: {model}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn sp() -> PathBuf {
        settings_path().unwrap()
    }

    fn envp() -> PathBuf {
        env_file_path().unwrap()
    }

    fn read_json(path: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    const OFFICIAL_ENV: &str =
        "# my gcp setup\nGOOGLE_CLOUD_PROJECT=my-proj\nGEMINI_API_KEY=\"real-key\"\n";

    fn seed_official() {
        std::fs::create_dir_all(sp().parent().unwrap()).unwrap();
        std::fs::write(
            sp(),
            r#"{"model":{"name":"gemini-pro"},"security":{"auth":{"selectedType":"oauth-personal"}},"general":{"vimMode":true}}"#,
        )
        .unwrap();
        std::fs::write(envp(), OFFICIAL_ENV).unwrap();
    }

    #[test]
    fn apply_upserts_env_lines_and_keeps_user_lines() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        seed_official();
        apply("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        let env = std::fs::read_to_string(envp()).unwrap();
        assert!(env.contains("# my gcp setup\n"), "comment kept:\n{env}");
        assert!(
            env.contains("GOOGLE_CLOUD_PROJECT=my-proj\n"),
            "user var kept:\n{env}"
        );
        assert!(env.contains("GEMINI_API_KEY=ag_local_"));
        assert!(env.contains("GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:9090\n"));
        assert!(!env.contains("real-key"));
        assert_eq!(env.matches("GEMINI_API_KEY=").count(), 1);
        cleanup(&temp);
    }

    #[test]
    fn toggle_back_keeps_user_additions_and_restores_overwritten_values() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        seed_official();
        toggle("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        // 用户 apply 之后的修改
        let mut doc = read_json(&sp());
        doc["mcpServers"] = serde_json::json!({"github": {"command": "npx"}});
        std::fs::write(sp(), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        let mut env = std::fs::read_to_string(envp()).unwrap();
        env.push_str("FOO=bar\n");
        std::fs::write(envp(), env).unwrap();

        let result = toggle("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        assert_eq!(result.new_provider, "official");
        let doc = read_json(&sp());
        assert_eq!(doc["mcpServers"]["github"]["command"], "npx");
        assert_eq!(doc["general"]["vimMode"], true);
        assert_eq!(doc["model"]["name"], "gemini-pro");
        assert_eq!(doc["security"]["auth"]["selectedType"], "oauth-personal");
        let env = std::fs::read_to_string(envp()).unwrap();
        assert!(env.contains("FOO=bar"), "user addition kept:\n{env}");
        assert!(env.contains("GOOGLE_CLOUD_PROJECT=my-proj"));
        assert!(env.contains("# my gcp setup"));
        assert!(
            env.contains("GEMINI_API_KEY=\"real-key\""),
            "original restored:\n{env}"
        );
        assert!(!env.contains("ag_local_"));
        assert!(!env.contains("GOOGLE_GEMINI_BASE_URL"));
        assert!(!detect().has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn toggle_back_without_snapshot_removes_only_our_keys() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        apply("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        let mut env = std::fs::read_to_string(envp()).unwrap();
        env.push_str("FOO=bar\n");
        std::fs::write(envp(), env).unwrap();
        std::fs::remove_dir_all(saved_dir()).ok();
        let result = toggle("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        assert_eq!(result.new_provider, "official");
        let env = std::fs::read_to_string(envp()).unwrap();
        assert_eq!(env, "FOO=bar\n");
        let doc = read_json(&sp());
        assert!(doc.pointer("/security/auth/selectedType").is_none());
        assert!(doc.pointer("/model/name").is_none());
        cleanup(&temp);
    }

    /// v2.0.5 apply 整份覆盖 .env(带 LEGACY_ENV_HEADER)并把原文件整份备份;
    /// restore 必须以备份为基底,不能只捞回两个 key 而丢掉 GOOGLE_CLOUD_PROJECT。
    #[test]
    fn restore_env_from_legacy_wholesale_file_uses_backup_as_base() {
        let backup = "GOOGLE_CLOUD_PROJECT=my-proj\nGEMINI_API_KEY=real-key";
        let current = format!(
            "{LEGACY_ENV_HEADER}\nGEMINI_API_KEY=ag_local_abc\nGOOGLE_GEMINI_BASE_URL=http://127.0.0.1:9090\n"
        );
        let restored = restore_env_content(&current, Some(backup));
        assert!(
            restored.contains("GOOGLE_CLOUD_PROJECT=my-proj\n"),
            "user var from backup kept:\n{restored}"
        );
        assert!(restored.contains("GEMINI_API_KEY=real-key\n"), "{restored}");
        assert!(!restored.contains(LEGACY_ENV_HEADER));
        assert!(!restored.contains("ag_local_"));
        assert!(!restored.contains("GOOGLE_GEMINI_BASE_URL"));
    }

    #[test]
    fn toggle_back_from_legacy_env_restores_full_backup_and_survives_reapply() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        // 复现 v2.0.5 apply 之后的磁盘状态
        std::fs::create_dir_all(sp().parent().unwrap()).unwrap();
        std::fs::write(
            sp(),
            r#"{"model":{"name":"gemini-2.5-flash"},"security":{"auth":{"selectedType":"gemini-api-key"}}}"#,
        )
        .unwrap();
        std::fs::write(
            envp(),
            format!("{LEGACY_ENV_HEADER}\nGEMINI_API_KEY=ag_local_abc\nGOOGLE_GEMINI_BASE_URL=http://127.0.0.1:9090\n"),
        )
        .unwrap();
        std::fs::create_dir_all(saved_dir()).unwrap();
        std::fs::write(saved_settings_path(), r#"{"model":{"name":"gemini-pro"}}"#).unwrap();
        std::fs::write(
            saved_env_path(),
            "GOOGLE_CLOUD_PROJECT=my-proj\nGEMINI_API_KEY=real-key",
        )
        .unwrap();

        assert_eq!(
            toggle("127.0.0.1", 9090, "gemini-2.5-flash")
                .unwrap()
                .new_provider,
            "official"
        );
        let env = std::fs::read_to_string(envp()).unwrap();
        assert!(env.contains("GOOGLE_CLOUD_PROJECT=my-proj"), "{env}");
        assert!(env.contains("GEMINI_API_KEY=real-key"), "{env}");
        // 再 apply 一次,备份不能被截断
        apply("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        let saved = std::fs::read_to_string(saved_env_path()).unwrap();
        assert!(saved.contains("GOOGLE_CLOUD_PROJECT=my-proj"), "{saved}");
        cleanup(&temp);
    }

    /// 回归:命令改到 spawn_blocking 后并发执行,apply_gemini_config 与
    /// upsert_mcp_server(gemini) 同时读改写 ~/.gemini/settings.json 会丢更新。
    #[test]
    fn concurrent_apply_and_mcp_upsert_on_same_settings_keep_both_updates() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        const N: usize = 25;
        let a = std::thread::spawn(|| {
            for i in 0..N {
                apply("127.0.0.1", 9090, &format!("model-{i}")).unwrap();
            }
        });
        let b = std::thread::spawn(|| {
            for i in 0..N {
                crate::tools::mcp::upsert(crate::tools::mcp::UpsertMcpServerInput {
                    client: "gemini".into(),
                    name: format!("srv{i}"),
                    command: "npx".into(),
                    args: vec![],
                    env: vec![],
                })
                .unwrap();
            }
        });
        a.join().unwrap();
        b.join().unwrap();
        let doc = read_json(&sp());
        assert_eq!(doc["model"]["name"], format!("model-{}", N - 1));
        for i in 0..N {
            assert!(
                doc["mcpServers"].get(format!("srv{i}")).is_some(),
                "lost srv{i}: {doc}"
            );
        }
        cleanup(&temp);
    }

    #[test]
    fn detect_ignores_decoy_names_and_comments() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(sp().parent().unwrap()).unwrap();
        std::fs::write(
            sp(),
            r#"{"mcpServers":{"agentgate-docs":{"command":"npx"}},"note":"ag_local_ is muxlayer"}"#,
        )
        .unwrap();
        std::fs::write(envp(), "# agentgate: ag_local_example\nFOO=1\n").unwrap();
        assert!(!detect().has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn apply_refuses_to_overwrite_unreadable_settings() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(sp().parent().unwrap()).unwrap();
        let original: &[u8] = b"{\"general\":{}} \xff";
        std::fs::write(sp(), original).unwrap();
        assert!(apply("127.0.0.1", 9090, "m").is_err());
        assert_eq!(std::fs::read(sp()).unwrap(), original);
        cleanup(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_env_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        apply("127.0.0.1", 9090, "m").unwrap();
        let mode = std::fs::metadata(envp()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
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
        let result = apply("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        assert!(result.success);
        assert!(settings_path().unwrap().exists());
        assert!(env_file_path().unwrap().exists());
        let settings = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(settings.contains("gemini-2.5-flash"));
        let env = std::fs::read_to_string(env_file_path().unwrap()).unwrap();
        assert!(env.contains("ag_local_"));
        assert!(env.contains("GOOGLE_GEMINI_BASE_URL"));
        cleanup(&temp);
    }

    #[test]
    fn test_apply_preserves_existing_fields() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(settings_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(settings_path().unwrap(), r#"{"general":{"vimMode":true}}"#).unwrap();
        let result = apply("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        assert!(result.success);
        let settings = std::fs::read_to_string(settings_path().unwrap()).unwrap();
        assert!(settings.contains("vimMode"));
        let env = std::fs::read_to_string(env_file_path().unwrap()).unwrap();
        assert!(env.contains("ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn test_toggle_saves_and_restores() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(settings_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            settings_path().unwrap(),
            r#"{"model":{"name":"gemini-pro"}}"#,
        )
        .unwrap();
        std::fs::write(env_file_path().unwrap(), "GEMINI_API_KEY=real-key\n").unwrap();
        // Toggle to agentgate
        let result = toggle("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        assert_eq!(result.new_provider, "agentgate");
        let env = std::fs::read_to_string(env_file_path().unwrap()).unwrap();
        assert!(env.contains("ag_local_"));
        // Toggle back to official
        let result = toggle("127.0.0.1", 9090, "gemini-2.5-flash").unwrap();
        assert_eq!(result.new_provider, "official");
        let env = std::fs::read_to_string(env_file_path().unwrap()).unwrap();
        assert!(env.contains("real-key"));
        cleanup(&temp);
    }
}
