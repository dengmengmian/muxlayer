use std::path::{Path, PathBuf};

use crate::errors::codes;
use crate::errors::AppError;
use crate::security::local_token;
use crate::tools::client_files as cf;
use crate::tools::toml_merge;
use serde::Serialize;

/// apply 写入的顶层 key 及其字面量。restore 只撤销「当前值仍等于这里」的 key。
const OWNED_TOP_LEVEL_KEYS: &[(&str, &str)] = &[
    ("model_provider", "\"OpenAI\""),
    ("model_context_window", "1000000"),
    ("model_auto_compact_token_limit", "9000000"),
];
/// apply 接管的 provider 段。
const PROVIDER_SECTION: &str = "model_providers.OpenAI";
/// 旧版 AgentGate 写的 provider 段(`model_provider = "agentgate"`)。
const LEGACY_PROVIDER_SECTION: &str = "model_providers.agentgate";

pub fn config_path() -> Result<PathBuf, AppError> {
    Ok(crate::fsutil::home_dir()?
        .join(".codex")
        .join("config.toml"))
}

pub fn auth_json_path() -> Result<PathBuf, AppError> {
    Ok(crate::fsutil::home_dir()?.join(".codex").join("auth.json"))
}

/// Files captured into a client_apply_history snapshot so a rollback can
/// restore the pre-apply state. Order is what the UI sees in the history
/// drawer. 家目录不可用时返回空列表(随后的 apply 会以同样原因报错)。
pub fn snapshot_paths() -> Vec<(&'static str, PathBuf)> {
    [
        ("config.toml", config_path()),
        ("auth.json", auth_json_path()),
    ]
    .into_iter()
    .filter_map(|(name, p)| p.ok().map(|p| (name, p)))
    .collect()
}

/// Directory where we save the user's original config.toml + auth.json.
fn saved_dir() -> PathBuf {
    local_token::token_dir().join("codex_official")
}

fn saved_config_path() -> PathBuf {
    saved_dir().join("config.toml")
}

fn saved_auth_path() -> PathBuf {
    saved_dir().join("auth.json")
}

/// Save original config.toml + auth.json. The saved config.toml is only used
/// as the source of *values apply overwrote* (e.g. the user's previous
/// `model_provider`); restore never copies it over the live file.
///
/// auth.json is only copied when it currently holds the user's real
/// credentials — i.e. the `OPENAI_API_KEY` doesn't start with `ag_local_`.
/// This protects an existing healthy backup from being overwritten by a
/// previously-polluted live auth.json (older AgentGate builds had stripped
/// the OAuth tokens to just `{"OPENAI_API_KEY":"ag_local_..."}`).
fn save_official_files() -> Result<(), AppError> {
    cf::backup_file(
        &config_path()?,
        &saved_config_path(),
        codes::CODEX_SAVE_FAILED,
    )?;
    let auth = auth_json_path()?;
    if !auth_is_polluted(&auth) {
        cf::backup_file(&auth, &saved_auth_path(), codes::CODEX_SAVE_FAILED)?;
    }
    Ok(())
}

fn auth_is_polluted(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content)
    else {
        return false;
    };
    let current_key = map
        .get("OPENAI_API_KEY")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    cf::is_local_token(current_key) && !map.contains_key("tokens")
}

/// Check if saved official files exist.
fn has_saved_official() -> bool {
    saved_auth_path().exists() || saved_config_path().exists()
}

/// Repair `auth.json` if a previous AgentGate version replaced it with
/// `{"OPENAI_API_KEY": "ag_local_..."}` and stripped the ChatGPT OAuth
/// tokens. The Codex JetBrains / VS Code plugins detect "configured" by
/// reading `auth_mode` + the `tokens` object out of this file — when those
/// are missing the plugin greys out. We restore from `~/.agentgate/codex_official/auth.json`
/// (saved by `save_official_files` the first time we touched the file) and
/// return true so the caller can surface a notice.
///
/// No-op when the current auth.json already has tokens (clean state) or
/// there's no saved backup to restore from.
fn repair_polluted_auth_json() -> Result<bool, AppError> {
    let auth_path = auth_json_path()?;
    if !auth_is_polluted(&auth_path) {
        return Ok(false);
    }
    let Some(saved) = cf::read_optional(&saved_auth_path(), codes::CODEX_RESTORE_FAILED)? else {
        return Ok(false);
    };
    cf::write_verified(
        &auth_path,
        saved.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::CODEX_RESTORE_FAILED,
    )?;
    Ok(true)
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct CodexConfigStatus {
    pub config_path: String,
    pub auth_json_path: String,
    pub exists: bool,
    pub auth_json_exists: bool,
    pub has_agentgate: bool,
    pub has_agentgate_auth: bool,
    pub current_provider: Option<String>,
    pub current_model: Option<String>,
    pub auth_mode: String,
    pub token_path: String,
    /// Whether the provider is currently set to "agentgate" (active) or something else.
    pub is_agentgate_active: bool,
    /// True if OPENAI_API_KEY in auth.json was overwritten with ag_local_ by old AgentGate.
    pub openai_key_polluted: bool,
    /// True if saved official config exists for toggle restore.
    pub has_saved_official: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "CodexApplyConfigResult")]
pub struct ApplyConfigResult {
    pub success: bool,
    pub config_path: String,
    pub auth_json_path: String,
    pub backup_path: Option<String>,
    pub auth_backup_path: Option<String>,
    pub token_path: String,
    pub changed_keys: Vec<String>,
    pub warnings: Vec<String>,
}

/// `[model_providers.OpenAI]` 段里是否是我们的 `experimental_bearer_token = "ag_local_…"`。
/// 行级解析,不依赖整份 TOML 合法,也不会被注释里的同名文本骗到。
fn has_our_bearer(content: &str) -> bool {
    toml_merge::section_body(content, PROVIDER_SECTION)
        .and_then(|body| toml_merge::top_level_raw_value(&body, "experimental_bearer_token"))
        .is_some_and(|raw| cf::is_local_token(raw.trim_matches('"')))
}

fn has_legacy_section(content: &str) -> bool {
    toml_merge::section_body(content, LEGACY_PROVIDER_SECTION).is_some()
}

pub fn detect() -> CodexConfigStatus {
    // 只读探测:家目录不可用时路径为空,按「不存在」展示。
    let path = config_path().unwrap_or_default();
    let auth_path = auth_json_path().unwrap_or_default();
    let path_str = path.to_string_lossy().to_string();
    let auth_str = auth_path.to_string_lossy().to_string();
    let tp = local_token::token_path().to_string_lossy().to_string();

    let exists = path.is_file();
    let auth_json_exists = auth_path.is_file();

    // The "hijack OpenAI provider" config has `model_provider = "OpenAI"` with
    // the OpenAI provider's bearer being our `ag_local_` token. The legacy
    // `[model_providers.agentgate]` shape is still recognised so users
    // upgrading from older builds see correct status before re-apply.
    // 只认结构化标记(段 + key),注释 / MCP 名里出现同样文本不算。
    let content = if exists {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };
    let current_provider = toml_merge::lookup_str(&content, &["model_provider"]);
    let current_model = toml_merge::lookup_str(&content, &["model"]);
    let openai_hijack = has_our_bearer(&content);
    let has_agentgate = openai_hijack || has_legacy_section(&content);

    let (has_agentgate_auth, openai_key_polluted) = if auth_json_exists {
        let auth_content = std::fs::read_to_string(&auth_path).unwrap_or_default();
        if let Ok(map) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&auth_content)
        {
            let current_key = map
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_ag = cf::is_local_token(current_key);
            // Polluted = has ag_local_ but ALSO has OAuth tokens (old version mess)
            // OR is the clean ag-only form (legacy stripped state).
            let is_clean_ag = is_ag && !map.contains_key("tokens");
            let polluted = is_ag && !is_clean_ag && !has_saved_official();
            (is_clean_ag, polluted)
        } else {
            (false, false)
        }
    } else {
        (false, false)
    };

    // "AgentGate active" now means: our hijack snippet has been written
    // (we can tell from the ag_local_ bearer token).
    let is_agentgate_active = openai_hijack || current_provider.as_deref() == Some("agentgate"); // legacy

    CodexConfigStatus {
        config_path: path_str,
        auth_json_path: auth_str,
        exists,
        auth_json_exists,
        has_agentgate,
        has_agentgate_auth,
        current_provider,
        current_model,
        auth_mode: "key_swap".to_string(),
        token_path: tp,
        is_agentgate_active,
        openai_key_polluted,
        has_saved_official: has_saved_official(),
    }
}

pub fn generate_snippet(host: &str, port: i64, bearer_token: &str) -> String {
    // The "hijack OpenAI provider + requires_openai_auth" pattern:
    //
    //   model_provider = "OpenAI"   ← official provider name, NOT a custom one
    //
    //   [model_providers.OpenAI]
    //   base_url = "http://localhost:9090/v1"  ← override points to us
    //   requires_openai_auth = true             ← keep ChatGPT auth state live
    //
    // `name = "OpenAI"` is load-bearing and MUST stay "OpenAI":
    //   1. IDE feature gate — Codex.app lights up the plugin marketplace,
    //      Browser, Computer Use and quota only when the active provider's
    //      `name == "OpenAI"` (`is_openai()`). Rename it and the marketplace
    //      vanishes (learned the hard way).
    //   2. It also switches remote compaction v2 ON. That needs OpenAI's
    //      encrypted context which third-party upstreams (MiMo etc.) can't
    //      produce, so Codex would hang mid-turn — EXCEPT we declare a huge
    //      `model_context_window = 1000000`, pushing the compaction trigger to
    //      ~950k tokens a normal session never reaches. The gateway's own
    //      auto_compact tidies the real ~128k window long before that.
    //
    // Net: keep `name = "OpenAI"` (IDE alive) + big window (compaction never
    // fires). This is cc-switch's approach. `requires_openai_auth = true`
    // keeps the ChatGPT auth-aware UI / mobile login alive; overriding
    // `base_url` routes the actual traffic through AgentGate.
    //
    // `experimental_bearer_token` carries the local AgentGate access token
    // — Codex sends `Authorization: Bearer ag_local_…` to us. auth.json is
    // never touched: ChatGPT OAuth (`tokens.access_token`, `auth_mode:
    // chatgpt`) stays intact and continues to drive the IDE login state.
    //
    // `model` is intentionally NOT set at the top level. The IDE / CLI
    // picker chooses the model display name; AgentGate routes by whatever
    // name comes in via its own per-model capability matrix.
    //
    // Credit: the `requires_openai_auth` discovery comes from a CSDN post
    // by "硅基新手村" (alex_yangchuansheng) — this is the only known way
    // to route Codex through a custom base_url while preserving the
    // official ChatGPT IDE features.
    format!(
        r#"model_provider = "OpenAI"
model_context_window = 1000000
model_auto_compact_token_limit = 9000000

[model_providers.OpenAI]
name = "OpenAI"
base_url = "http://{host}:{port}/v1"
wire_api = "responses"
experimental_bearer_token = "{bearer_token}"
requires_openai_auth = true"#,
    )
}

pub fn apply(host: &str, port: i64) -> Result<ApplyConfigResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let token = local_token::ensure_token()?;

    let path = config_path()?;
    let auth_path = auth_json_path()?;
    let path_str = path.to_string_lossy().to_string();
    let auth_str = auth_path.to_string_lossy().to_string();
    let tp = local_token::token_path().to_string_lossy().to_string();
    let mut warnings = Vec::new();
    let mut changed_keys = vec!["config.toml".to_string()];

    // 先读当前配置:读不出来(权限 / 编码)直接失败,绝不拿空内容覆盖。
    let existing = cf::read_for_update(&path, codes::CODEX_CONFIG_WRITE_FAILED)?;

    // Save original config.toml + auth.json the FIRST time we touch them, so
    // a future "switch back to official" knows the values apply overwrote.
    // Skipped when we're already pointing at AgentGate (the saved copy
    // would otherwise overwrite the user's true original with our config).
    if !detect().is_agentgate_active {
        save_official_files()?;
    }

    // === Repair: if a previous AgentGate version stripped auth.json down to
    // just our local token, restore the ChatGPT OAuth tokens from the saved
    // backup. The Codex IDE plugin reads auth.json to detect "configured" —
    // wiping the OAuth fields was what greyed it out. New apply() never
    // writes auth.json so once repaired it stays good.
    if repair_polluted_auth_json()? {
        warnings.push(
            "已修复被旧版 AgentGate 清空的 auth.json，Codex IDE 插件应该恢复可用。".to_string(),
        );
        changed_keys.push("auth.json (restored)".to_string());
    }

    // === Surgical merge into config.toml. auth.json is intentionally left
    // alone — the bearer travels via config.toml's [model_providers.OpenAI]
    // table. Only the owned top-level keys and the `[model_providers.OpenAI]`
    // table body move; everything else is preserved byte-for-byte.
    let mut merged = existing;
    for (key, raw) in OWNED_TOP_LEVEL_KEYS {
        merged = toml_merge::upsert_top_level_key(&merged, key, raw);
    }
    // name 保持 "OpenAI":插件市场/IDE 门控只认 name=="OpenAI";compaction 由
    // 上面的大 window 挡住,不靠改 name(改了会丢插件市场)。
    let section_body = format!(
        "name = \"OpenAI\"\nbase_url = \"http://{host}:{port}/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"{token}\"\nrequires_openai_auth = true\n"
    );
    merged = toml_merge::upsert_section(&merged, PROVIDER_SECTION, &section_body);

    // config.toml 里带着 gateway token → 0600。
    cf::write_verified(
        &path,
        merged.as_bytes(),
        Some(cf::SECRET_FILE_MODE),
        codes::CODEX_CONFIG_WRITE_FAILED,
    )?;

    if has_saved_official() {
        warnings.push("已备份原始 config.toml，可随时切换回官方配置。".to_string());
    }

    // Heads-up about the hijack-OpenAI design choice. `model_provider =
    // "OpenAI"` + `requires_openai_auth = true` keeps the IDE plugin entries
    // (Browser, Computer-Use, Mobile, quota query) alive because Codex.app
    // sees the official provider name and still demands a valid ChatGPT
    // login. Conversation requests, however, hit AgentGate's localhost
    // endpoint instead of api.openai.com. The user should know that they
    // need to keep `codex login` valid for the IDE bits to keep working.
    warnings.push(
        "已切换到代理模式：对话请求走 MuxLayer，但 Codex.app 内嵌插件（Browser / \
         Computer-Use / Mobile / 配额查询）仍走 ChatGPT 官方登录态 —— 都可用。\
         如果之前没登录过 Codex，请先执行 `codex login` 完成 ChatGPT 认证。"
            .to_string(),
    );

    Ok(ApplyConfigResult {
        success: true,
        config_path: path_str,
        auth_json_path: auth_str,
        backup_path: None,
        auth_backup_path: None,
        token_path: tp,
        changed_keys,
        warnings,
    })
}

/// 「切回官方」的纯函数核心:只撤销 apply 写过的东西。
///
/// - 顶层 key:当前值仍等于 apply 写入的值才处理;快照(apply 前的官方配置)里
///   有原值就写回原值,没有就删除。用户在 apply 之后改过的值不动。
/// - `[model_providers.OpenAI]`:只有段里是我们的 `ag_local_` bearer 才处理;
///   快照里有用户自己的同名段就写回,否则整段删除。
/// - 旧版 `[model_providers.agentgate]` + `model_provider = "agentgate"` 同理。
///
/// 其它内容(apply 之后用户加的 MCP server / projects 信任 / 注释)逐字节保留。
/// 快照缺失时照样删除我们的 key——得到的就是 Codex 默认的官方配置。
pub(crate) fn restore_content(current: &str, snapshot: Option<&str>) -> String {
    let snapshot = snapshot.filter(|s| !has_our_bearer(s) && !has_legacy_section(s));
    let mut out = current.to_string();

    let mut owned: Vec<(&str, &str)> = OWNED_TOP_LEVEL_KEYS.to_vec();
    if has_legacy_section(&out) {
        owned.push(("model_provider", "\"agentgate\""));
    }
    for (key, ours) in owned {
        if toml_merge::top_level_raw_value(&out, key).as_deref() != Some(ours) {
            continue;
        }
        // 快照已排除被我们污染的情况;用户原值恰好等于我们的值(如本来就是
        // model_provider = "OpenAI" 指向自己的代理)时也照原样写回,不能删。
        let original = snapshot.and_then(|s| toml_merge::top_level_raw_value(s, key));
        out = match original {
            Some(orig) => toml_merge::upsert_top_level_key(&out, key, &orig),
            None => toml_merge::remove_top_level_key(&out, key),
        };
    }

    if has_our_bearer(&out) {
        out = match snapshot.and_then(|s| toml_merge::section_body(s, PROVIDER_SECTION)) {
            Some(body) => toml_merge::upsert_section(&out, PROVIDER_SECTION, &body),
            None => toml_merge::remove_section(&out, PROVIDER_SECTION),
        };
    }
    if has_legacy_section(&out) {
        out = toml_merge::remove_section(&out, LEGACY_PROVIDER_SECTION);
    }
    out
}

/// 读当前 config.toml + 官方备份,做 surgical restore 并写回。返回是否有改动。
fn restore_official_config() -> Result<bool, AppError> {
    let path = config_path()?;
    let current = cf::read_for_update(&path, codes::CODEX_RESTORE_FAILED)?;
    let snapshot = cf::read_optional(&saved_config_path(), codes::CODEX_RESTORE_FAILED)?;
    let restored = restore_content(&current, snapshot.as_deref());
    if restored == current {
        return Ok(false);
    }
    cf::write_verified(
        &path,
        restored.as_bytes(),
        None,
        codes::CODEX_RESTORE_FAILED,
    )?;
    Ok(true)
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[specta(rename = "CodexToggleResult")]
pub struct ToggleResult {
    pub success: bool,
    pub new_provider: String,
    pub config_path: String,
}

/// Toggle between AgentGate and the original official config.
/// Restores / writes ONLY config.toml — auth.json is intentionally untouched
/// so the Codex IDE plugin (which probes auth.json for `auth_mode` + `tokens`)
/// stays alive throughout the switch.
pub fn toggle_provider(host: &str, port: i64) -> Result<ToggleResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let status = detect();
    let config_path_str = config_path()?.to_string_lossy().to_string();

    if status.is_agentgate_active {
        // Switching TO official: surgically undo only what apply wrote.
        restore_official_config()?;
        let new_provider = detect()
            .current_provider
            .unwrap_or_else(|| "openai".to_string());
        Ok(ToggleResult {
            success: true,
            new_provider,
            config_path: config_path_str,
        })
    } else {
        // Switching TO agentgate: same path as `apply()`. Defer to it so the
        // auth-repair safety net runs here too.
        apply(host, port)?;
        Ok(ToggleResult {
            success: true,
            new_provider: "agentgate".to_string(),
            config_path: config_path_str,
        })
    }
}

/// Switch Codex back to its pre-AgentGate "native" mode by surgically
/// removing what apply wrote (and restoring the values it overwrote from the
/// saved official config.toml). Keeps `auth.json` untouched (new code never
/// modified it; old polluted state was already cleaned up by `apply`). The
/// saved backup itself is kept on disk.
///
/// With the new "hijack OpenAI + requires_openai_auth" config the IDE
/// plugins stay alive in compat mode too, so this is now mostly a "stop
/// routing through AgentGate" switch — useful when the user wants Codex
/// CLI to talk to api.openai.com directly again.
pub fn disable() -> Result<ApplyConfigResult, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    let path = config_path()?;
    let auth_path = auth_json_path()?;
    let path_str = path.to_string_lossy().to_string();
    let auth_str = auth_path.to_string_lossy().to_string();
    let tp = local_token::token_path().to_string_lossy().to_string();
    let mut warnings = Vec::new();
    let mut changed_keys = Vec::new();

    if restore_official_config()? {
        changed_keys.push("config.toml (restored)".to_string());
    }

    // Defense-in-depth: even though new `apply` no longer touches auth.json,
    // restore the OAuth backup if the live file got mangled by an older build
    // and a healthy backup is available.
    if repair_polluted_auth_json()? {
        warnings.push(
            "auth.json 被旧版 AgentGate 清空过的部分已恢复，Codex IDE 插件应可用。".to_string(),
        );
        changed_keys.push("auth.json (restored)".to_string());
    }

    warnings.push(
        "已切回原生模式：Codex 直连 ChatGPT 官方，对话请求不再经过 MuxLayer。\
         如需重新使用第三方模型路由，点击「应用配置」即可切回代理模式。"
            .to_string(),
    );

    Ok(ApplyConfigResult {
        success: true,
        config_path: path_str,
        auth_json_path: auth_str,
        backup_path: None,
        auth_backup_path: None,
        token_path: tp,
        changed_keys,
        warnings,
    })
}

pub fn open_config() -> Result<(), AppError> {
    let path = config_path()?;
    if !path.exists() {
        return Err(AppError::new(
            codes::CODEX_CONFIG_NOT_FOUND,
            "Codex config file does not exist",
        ));
    }
    open::that(&path).map_err(|e| {
        AppError::new(
            codes::CODEX_CONFIG_OPEN_FAILED,
            format!("Failed to open: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn cfg_path() -> PathBuf {
        config_path().unwrap()
    }

    fn write_cfg(content: &str) {
        std::fs::create_dir_all(cfg_path().parent().unwrap()).unwrap();
        std::fs::write(cfg_path(), content).unwrap();
    }

    fn append_cfg(extra: &str) {
        let mut cur = std::fs::read_to_string(cfg_path()).unwrap();
        cur.push_str(extra);
        std::fs::write(cfg_path(), cur).unwrap();
    }

    const USER_ADDED: &str = "\n[mcp_servers.added_later]\ncommand = \"my-mcp\"\n\n[projects.\"/Users/me/new\"]\ntrust_level = \"trusted\"\n";

    fn assert_restored_surgically(cfg: &str) {
        let doc = cfg
            .parse::<toml_edit::DocumentMut>()
            .expect("restored config must be valid TOML");
        // 用户在 apply 之后加的东西必须保留
        assert!(cfg.contains("[mcp_servers.added_later]"), "got:\n{cfg}");
        assert!(cfg.contains("[projects.\"/Users/me/new\"]"), "got:\n{cfg}");
        // 我们写的东西必须消失
        assert!(!cfg.contains("ag_local_"), "bearer must be gone:\n{cfg}");
        assert!(doc.get("model_auto_compact_token_limit").is_none());
        // 被 apply 覆盖的原值回来
        assert_eq!(doc["model_provider"].as_str(), Some("openai"));
        assert_eq!(doc["model_context_window"].as_integer(), Some(200000));
        assert_eq!(
            doc["model_providers"]["OpenAI"]["base_url"].as_str(),
            Some("https://proxy.example.com/v1")
        );
    }

    const OFFICIAL_WITH_OVERWRITTEN_VALUES: &str = "# mine\nmodel_provider = \"openai\"\nmodel_context_window = 200000\n\n[model_providers.OpenAI]\nname = \"OpenAI\"\nbase_url = \"https://proxy.example.com/v1\"\n";

    #[test]
    fn toggle_back_keeps_user_additions_made_after_apply() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        write_cfg(OFFICIAL_WITH_OVERWRITTEN_VALUES);
        apply("127.0.0.1", 9090).unwrap();
        append_cfg(USER_ADDED);
        let result = toggle_provider("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        let cfg = std::fs::read_to_string(cfg_path()).unwrap();
        assert_restored_surgically(&cfg);
        assert!(cfg.starts_with("# mine\n"), "comments preserved:\n{cfg}");
        cleanup(&temp);
    }

    #[test]
    fn disable_keeps_user_additions_made_after_apply() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        write_cfg(OFFICIAL_WITH_OVERWRITTEN_VALUES);
        apply("127.0.0.1", 9090).unwrap();
        append_cfg(USER_ADDED);
        disable().unwrap();
        let cfg = std::fs::read_to_string(cfg_path()).unwrap();
        assert_restored_surgically(&cfg);
        cleanup(&temp);
    }

    #[test]
    fn disable_without_snapshot_still_removes_our_keys() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        // 没有预先存在的 config.toml → 不会有官方备份
        apply("127.0.0.1", 9090).unwrap();
        append_cfg(USER_ADDED);
        std::fs::remove_dir_all(saved_dir()).ok();
        let result = disable().unwrap();
        assert!(result.success);
        let cfg = std::fs::read_to_string(cfg_path()).unwrap();
        let doc = cfg.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(!cfg.contains("ag_local_"));
        assert!(doc.get("model_provider").is_none());
        assert!(doc.get("model_context_window").is_none());
        assert!(cfg.contains("[mcp_servers.added_later]"));
        assert!(!detect().is_agentgate_active);
        cleanup(&temp);
    }

    #[test]
    fn restore_does_not_touch_values_user_changed_after_apply() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        write_cfg(OFFICIAL_WITH_OVERWRITTEN_VALUES);
        apply("127.0.0.1", 9090).unwrap();
        let cur = std::fs::read_to_string(cfg_path()).unwrap();
        std::fs::write(
            cfg_path(),
            cur.replace(
                "model_context_window = 1000000",
                "model_context_window = 64000",
            ),
        )
        .unwrap();
        disable().unwrap();
        let doc = std::fs::read_to_string(cfg_path())
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(doc["model_context_window"].as_integer(), Some(64000));
        cleanup(&temp);
    }

    #[test]
    fn detect_ignores_decoy_comment_and_reads_model_section_aware() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        write_cfg(
            "# experimental_bearer_token = \"ag_local_decoy\"\n# [model_providers.agentgate]\nmodel_context_window = 5\n\n[profiles.fast]\nmodel = \"inner\"\n",
        );
        let status = detect();
        assert!(!status.has_agentgate, "comment must not count as connected");
        assert!(!status.is_agentgate_active);
        assert_eq!(
            status.current_model, None,
            "section key is not top-level model"
        );
        cleanup(&temp);
    }

    #[test]
    fn restore_keeps_original_value_even_when_it_equals_ours() {
        // 用户原配置本来就是 model_provider = "OpenAI" + 自己的 [model_providers.OpenAI]
        let original = "model_provider = \"OpenAI\"\n\n[model_providers.OpenAI]\nname = \"OpenAI\"\nbase_url = \"https://my-proxy/v1\"\n";
        let applied = toml_merge::upsert_section(
            original,
            PROVIDER_SECTION,
            "name = \"OpenAI\"\nbase_url = \"http://127.0.0.1:9090/v1\"\nexperimental_bearer_token = \"ag_local_x\"\n",
        );
        let restored = restore_content(&applied, Some(original));
        let doc = restored.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some("OpenAI"));
        assert_eq!(
            doc["model_providers"]["OpenAI"]["base_url"].as_str(),
            Some("https://my-proxy/v1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_config_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        write_cfg("model = \"gpt-5\"\n");
        std::fs::set_permissions(cfg_path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        apply("127.0.0.1", 9090).unwrap();
        let mode = std::fs::metadata(cfg_path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config.toml carries the gateway token");
        cleanup(&temp);
    }

    #[test]
    fn test_generate_snippet() {
        let snippet = generate_snippet("127.0.0.1", 9090, "ag_local_abc123");

        // The whole reason for this format: hijack the OpenAI provider so
        // the IDE plugins (which gate on `model_provider == openai/chatgpt`)
        // stay alive.
        assert!(
            snippet.contains("model_provider = \"OpenAI\""),
            "must use the official OpenAI provider name (not a custom one)"
        );
        assert!(snippet.contains("[model_providers.OpenAI]"));
        assert!(
            snippet.contains("name = \"OpenAI\""),
            "name MUST be OpenAI — the IDE plugin marketplace gates on is_openai()"
        );
        assert!(
            snippet.contains("model_context_window = 1000000"),
            "large window keeps Codex from ever triggering remote compaction"
        );
        assert!(snippet.contains("base_url = \"http://127.0.0.1:9090/v1\""));
        assert!(snippet.contains("wire_api = \"responses\""));
        assert!(
            snippet.contains("requires_openai_auth = true"),
            "requires_openai_auth = true is what keeps the IDE plugin alive — \
             the whole point of this config shape"
        );
        assert!(
            snippet.contains("experimental_bearer_token = \"ag_local_abc123\""),
            "bearer travels via config so auth.json can stay untouched"
        );

        // Regression guards: don't reintroduce known-broken shapes.
        assert!(
            !snippet.contains("model_provider = \"agentgate\""),
            "custom model_provider (legacy) triggered IDE plugin grey-out"
        );
        assert!(
            !snippet.contains("[model_providers.agentgate]"),
            "legacy block name must not be reintroduced"
        );
        assert!(
            !snippet.contains("gpt-5.5"),
            "synthetic gpt-5.5 model name triggered IDE picker rejection"
        );
    }

    #[test]
    fn test_lookup_top_level_values() {
        let content = "model = \"gpt-4\"\nmodel_provider = \"agentgate\"\n";
        assert_eq!(
            toml_merge::lookup_str(content, &["model"]),
            Some("gpt-4".to_string())
        );
        assert_eq!(
            toml_merge::lookup_str(content, &["model_provider"]),
            Some("agentgate".to_string())
        );
        assert_eq!(toml_merge::lookup_str(content, &["missing"]), None);
    }

    #[test]
    fn test_apply_creates_config() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let result = apply("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        assert!(config_path().unwrap().exists());
        // No pre-existing auth.json + nothing to repair → auth.json should
        // NOT be created by apply. Bearer travels in config.toml.
        assert!(
            !auth_json_path().unwrap().exists(),
            "apply must not create auth.json"
        );
        let cfg = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(
            cfg.contains("model_provider = \"OpenAI\""),
            "new hijack-OpenAI format"
        );
        assert!(cfg.contains("requires_openai_auth = true"));
        assert!(cfg.contains("model_context_window = 1000000"));
        assert!(cfg.contains("name = \"OpenAI\""));
        assert!(cfg.contains("experimental_bearer_token = \"ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn test_apply_preserves_user_config_toml() {
        // 回归测试：surgical merge 不应该擦掉用户已有的 [projects] / [mcp_servers] /
        // 顶级 approval_policy / 注释。
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        let user_config = r#"# My Codex notes
approval_policy = "on-request"
model_reasoning_effort = "high"

[projects."/Users/me/repo"]
trust_level = "trusted"

[mcp_servers.local]
command = "my-mcp-server"
args = ["--port", "1234"]
"#;
        std::fs::write(config_path().unwrap(), user_config).unwrap();

        apply("127.0.0.1", 9090).unwrap();
        let cfg = std::fs::read_to_string(config_path().unwrap()).unwrap();

        // 我们的改动落地
        assert!(cfg.contains("model_provider = \"OpenAI\""));
        assert!(cfg.contains("[model_providers.OpenAI]"));
        assert!(cfg.contains("experimental_bearer_token = \"ag_local_"));
        // 用户的所有其他内容完整保留
        assert!(cfg.contains("# My Codex notes"), "comments preserved");
        assert!(cfg.contains("approval_policy = \"on-request\""));
        assert!(cfg.contains("model_reasoning_effort = \"high\""));
        assert!(cfg.contains("[projects.\"/Users/me/repo\"]"));
        assert!(cfg.contains("trust_level = \"trusted\""));
        assert!(cfg.contains("[mcp_servers.local]"));
        assert!(cfg.contains("command = \"my-mcp-server\""));
        cleanup(&temp);
    }

    #[test]
    fn test_apply_idempotent_second_run_is_no_op() {
        // 再次 apply 应该和首次完全一样，不会重复追加 section / 改字段。
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        apply("127.0.0.1", 9090).unwrap();
        let after_first = std::fs::read_to_string(config_path().unwrap()).unwrap();
        apply("127.0.0.1", 9090).unwrap();
        let after_second = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert_eq!(after_first, after_second, "second apply must be a no-op");
        cleanup(&temp);
    }

    #[test]
    fn test_apply_preserves_chatgpt_oauth_tokens_in_auth_json() {
        // The whole reason for this refactor: applying AgentGate must NOT
        // strip the OAuth tokens from auth.json, because the Codex IDE
        // plugin reads them to detect "configured". Failing this test means
        // the IDE plugin will grey out for users running AgentGate.
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            config_path().unwrap(),
            "model = \"gpt-4\"\nmodel_provider = \"openai\"\n",
        )
        .unwrap();
        let original_auth =
            r#"{"OPENAI_API_KEY":"sk-real","auth_mode":"chatgpt","tokens":{"access_token":"jwt"}}"#;
        std::fs::write(auth_json_path().unwrap(), original_auth).unwrap();
        let result = apply("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        let auth = std::fs::read_to_string(auth_json_path().unwrap()).unwrap();
        assert!(auth.contains("chatgpt"), "auth_mode must survive apply");
        assert!(auth.contains("access_token"), "tokens must survive apply");
        assert!(
            auth.contains("sk-real"),
            "original OPENAI_API_KEY must survive apply"
        );
        assert!(
            has_saved_official(),
            "config.toml backup still saved for restore"
        );
        cleanup(&temp);
    }

    #[test]
    fn test_apply_repairs_old_polluted_auth_json() {
        // Migration path: a previous AgentGate version replaced auth.json
        // with just `{"OPENAI_API_KEY":"ag_local_..."}`. On the next apply
        // we should restore the original from the saved backup.
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(saved_dir()).unwrap();
        std::fs::create_dir_all(auth_json_path().unwrap().parent().unwrap()).unwrap();
        let original_auth =
            r#"{"OPENAI_API_KEY":"sk-real","auth_mode":"chatgpt","tokens":{"access_token":"jwt"}}"#;
        // Saved backup from the time AgentGate first ran.
        std::fs::write(saved_auth_path(), original_auth).unwrap();
        // Currently-polluted live auth.json (no tokens, no auth_mode).
        std::fs::write(
            auth_json_path().unwrap(),
            r#"{"OPENAI_API_KEY":"ag_local_xyz"}"#,
        )
        .unwrap();

        let result = apply("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        let auth = std::fs::read_to_string(auth_json_path().unwrap()).unwrap();
        assert!(auth.contains("access_token"), "repair must restore tokens");
        assert!(auth.contains("chatgpt"), "repair must restore auth_mode");
        cleanup(&temp);
    }

    #[test]
    fn test_toggle_provider_restores_config_only() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            config_path().unwrap(),
            "model = \"gpt-4\"\nmodel_provider = \"openai\"\n",
        )
        .unwrap();
        let original_auth =
            r#"{"OPENAI_API_KEY":"sk-real","auth_mode":"chatgpt","tokens":{"access_token":"jwt"}}"#;
        std::fs::write(auth_json_path().unwrap(), original_auth).unwrap();
        apply("127.0.0.1", 9090).unwrap();
        assert!(detect().is_agentgate_active);
        // Toggle back to official
        let result = toggle_provider("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        let auth = std::fs::read_to_string(auth_json_path().unwrap()).unwrap();
        assert!(
            auth.contains("chatgpt"),
            "auth.json still untouched on toggle"
        );
        assert!(auth.contains("access_token"));
        let cfg = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(cfg.contains("model_provider = \"openai\""));
        // Toggle back to agentgate
        let result = toggle_provider("127.0.0.1", 9090).unwrap();
        assert!(result.success);
        assert_eq!(result.new_provider, "agentgate");
        // auth.json is STILL the original OAuth — never touched by toggle.
        let auth = std::fs::read_to_string(auth_json_path().unwrap()).unwrap();
        assert!(
            auth.contains("access_token"),
            "auth.json survives round-trip"
        );
        assert!(auth.contains("chatgpt"));
        // config.toml has agentgate's bearer token, not auth.json.
        let cfg = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(cfg.contains("experimental_bearer_token = \"ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn test_disable_restores_official_config() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        // Start with an official-style config (what a fresh Codex login produces).
        let original_cfg =
            "model_provider = \"openai\"\n[plugins.\"browser@openai-bundled\"]\nenabled = true\n";
        std::fs::write(config_path().unwrap(), original_cfg).unwrap();
        let oauth_auth =
            r#"{"OPENAI_API_KEY":null,"auth_mode":"chatgpt","tokens":{"access_token":"jwt"}}"#;
        std::fs::write(auth_json_path().unwrap(), oauth_auth).unwrap();

        // Apply AgentGate config (compat mode).
        apply("127.0.0.1", 9090).unwrap();
        assert!(detect().is_agentgate_active);

        // Disable: should restore the official block.
        let result = disable().unwrap();
        assert!(result.success);
        let cfg = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(
            cfg.contains("model_provider = \"openai\""),
            "official model_provider restored"
        );
        assert!(
            cfg.contains("browser@openai-bundled"),
            "official plugin block restored"
        );
        assert!(!cfg.contains("agentgate"), "agentgate block should be gone");

        // auth.json must still be the OAuth one — disable never touches it
        // unless the polluted-repair branch fires (which doesn't apply here).
        let auth = std::fs::read_to_string(auth_json_path().unwrap()).unwrap();
        assert!(auth.contains("access_token"));
        cleanup(&temp);
    }

    #[test]
    fn test_disable_without_muxlayer_config_is_a_no_op() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        // 没接入过 MuxLayer:surgical restore 没有可撤销的东西,既不报错也不
        // 创建 / 改动配置文件(旧实现这里依赖备份,现在不再需要备份)。
        let result = disable().unwrap();
        assert!(result.changed_keys.is_empty());
        assert!(!config_path().unwrap().exists());
        write_cfg("model_provider = \"openai\"\n");
        disable().unwrap();
        assert_eq!(
            std::fs::read_to_string(config_path().unwrap()).unwrap(),
            "model_provider = \"openai\"\n"
        );
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
    fn test_detect_with_new_hijack_config() {
        // The new hijack-OpenAI format: model_provider = "OpenAI" but the
        // OpenAI block points at localhost with our ag_local_ bearer token.
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        let snippet = generate_snippet("127.0.0.1", 9090, "ag_local_xyz");
        std::fs::write(config_path().unwrap(), snippet).unwrap();
        let status = detect();
        assert!(status.exists);
        assert!(
            status.has_agentgate,
            "ag_local_ bearer token marks our hijack snippet"
        );
        assert!(status.is_agentgate_active);
        assert_eq!(status.current_provider, Some("OpenAI".to_string()));
        cleanup(&temp);
    }

    #[test]
    fn test_detect_with_legacy_agentgate_config() {
        // Backward compat: old AgentGate builds wrote `model_provider = "agentgate"`.
        // detect() should still recognise those rows so the upgrade flow can
        // surface "needs re-apply" guidance.
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            config_path().unwrap(),
            "model_provider = \"agentgate\"\nmodel = \"gpt-5\"\n[model_providers.agentgate]\nbase_url = \"http://x\"\n",
        )
        .unwrap();
        let status = detect();
        assert!(status.exists);
        assert!(status.has_agentgate);
        assert_eq!(status.current_provider, Some("agentgate".to_string()));
        assert!(status.is_agentgate_active);
        cleanup(&temp);
    }

    #[test]
    fn test_detect_with_real_openai_config_is_not_agentgate() {
        // A user with a genuine [model_providers.OpenAI] block pointing at
        // api.openai.com (no ag_local_ bearer) must NOT be flagged as
        // AgentGate-active.
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        std::fs::create_dir_all(config_path().unwrap().parent().unwrap()).unwrap();
        std::fs::write(
            config_path().unwrap(),
            "model_provider = \"OpenAI\"\n[model_providers.OpenAI]\nbase_url = \"https://api.openai.com/v1\"\n",
        )
        .unwrap();
        let status = detect();
        assert!(status.exists);
        assert!(!status.has_agentgate);
        assert!(!status.is_agentgate_active);
        cleanup(&temp);
    }
}
