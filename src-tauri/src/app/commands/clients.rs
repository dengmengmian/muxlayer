use tauri::State;

use super::{record_pre_apply, run_blocking};
use crate::app::state::AppState;
use crate::errors::AppError;
use crate::models::settings::ToolConfigView;
use crate::storage;
use crate::storage::db::DbPool;

// 读写客户端配置文件、spawn 子进程、等待进程退出的命令都是 `async` + `run_blocking`:
// Tauri v2 的同步命令跑在主线程,慢 I/O 会冻结 UI。命令名 / 参数 / 返回类型不变,
// 前端 bindings 形状不受影响。

/// 一行客户端卡片。路径取各客户端模块自己的 `config_path()`,与 apply 实际写入的
/// 路径一致(含 KIMI_CODE_HOME / GROK_HOME / DSH_HOME 等覆盖);家目录不可用时
/// 路径为空、按不存在展示。
fn tool_view(
    id: &str,
    name: &str,
    icon: &str,
    description: &str,
    path: Result<std::path::PathBuf, AppError>,
) -> ToolConfigView {
    let path = path.unwrap_or_default();
    ToolConfigView {
        id: id.to_string(),
        name: name.to_string(),
        slug: id.replace('_', "-"),
        icon: icon.to_string(),
        config_path: path.to_string_lossy().to_string(),
        description: description.to_string(),
        config_exists: path.is_file(),
    }
}

fn db_conn(db: &DbPool) -> Result<storage::db::DbConn, AppError> {
    db.get().map_err(|_| AppError::internal("DB lock failed"))
}

fn gateway_host_port(db: &DbPool) -> Result<(String, i64), AppError> {
    let conn = db_conn(db)?;
    let settings = storage::gateway_settings::get(&conn)?;
    Ok((settings.host, settings.port))
}

/// 读网关地址;顺带为当前 provider 补全推荐的模型映射(失败不影响接入)。
fn gateway_host_port_with_mappings(
    db: &DbPool,
    profile: storage::recommended_mappings::MappingProfile,
) -> Result<(String, i64), AppError> {
    let conn = db_conn(db)?;
    let _ = storage::recommended_mappings::supplement_active_provider(&conn, profile);
    let settings = storage::gateway_settings::get(&conn)?;
    Ok((settings.host, settings.port))
}

/// 读网关地址 + 当前 provider 的默认模型(没有 active provider 时用 `fallback`)。
fn gateway_host_port_model(db: &DbPool, fallback: &str) -> Result<(String, i64, String), AppError> {
    let conn = db_conn(db)?;
    let settings = storage::gateway_settings::get(&conn)?;
    let provider_id = settings.active_provider_id.clone().unwrap_or_default();
    let model = storage::providers::get_by_id(&conn, &provider_id)
        .ok()
        .map(|p| p.default_model)
        .unwrap_or_else(|| fallback.to_string());
    Ok((settings.host, settings.port, model))
}

// ── Tool Commands ──────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub fn list_tools() -> Result<Vec<ToolConfigView>, AppError> {
    use crate::tools::{
        atomcode, claude_code, codex, deepseek_harness, gemini_cli, grok_build, kimi_cli, opencode,
    };
    Ok(vec![
        tool_view(
            "claude-code",
            "Claude Code",
            "terminal",
            "Anthropic's CLI for Claude. Agentic coding tool with terminal integration.",
            claude_code::settings_path(),
        ),
        tool_view(
            "codex",
            "Codex",
            "code",
            "OpenAI's CLI coding agent. Supports OpenAI Responses API and chat completions.",
            codex::config_path(),
        ),
        tool_view(
            "opencode",
            "OpenCode",
            "braces",
            "Open-source terminal AI coding assistant. Supports multiple providers.",
            opencode::config_path(),
        ),
        tool_view(
            "atomcode",
            "AtomCode",
            "atom",
            "Open-source AI coding agent in your terminal. Uses OpenAI-compatible API.",
            atomcode::config_path(),
        ),
        tool_view(
            "gemini_cli",
            "Gemini CLI",
            "sparkles",
            "Google's AI coding CLI. Uses Gemini API with OpenAI-compatible endpoint support.",
            gemini_cli::settings_path(),
        ),
        tool_view(
            "kimi_cli",
            "Kimi CLI",
            "sparkles",
            "Moonshot Kimi Code CLI. OpenAI-compatible providers in config.toml.",
            kimi_cli::config_path(),
        ),
        tool_view(
            "grok_build",
            "Grok Build",
            "sparkles",
            "xAI Grok Build CLI. Custom models in ~/.grok/config.toml.",
            grok_build::config_path(),
        ),
        tool_view(
            "deepseek_harness",
            "DeepSeek Harness",
            "sparkles",
            "DeepSeek Harness (dsh). Custom OpenAI-compatible provider in settings.yaml.",
            deepseek_harness::settings_path(),
        ),
    ])
}

#[tauri::command]
#[specta::specta]
pub fn generate_codex_config(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port) = gateway_host_port(&state.db)?;
    let token = crate::security::local_token::ensure_token()?;
    Ok(crate::tools::codex::generate_snippet(&host, port, &token))
}

// ── Codex Config Commands ──────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn detect_codex_config() -> Result<crate::tools::codex::CodexConfigStatus, AppError> {
    run_blocking(|| Ok(crate::tools::codex::detect())).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_codex_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::codex::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port_with_mappings(
            &db,
            storage::recommended_mappings::MappingProfile::Codex,
        )?;
        record_pre_apply(
            &db,
            "codex",
            "apply",
            crate::tools::codex::snapshot_paths(),
            "apply",
        );
        crate::tools::codex::apply(&host, port)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_codex_provider(
    state: State<'_, AppState>,
) -> Result<crate::tools::codex::ToggleResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port_with_mappings(
            &db,
            storage::recommended_mappings::MappingProfile::Codex,
        )?;
        record_pre_apply(
            &db,
            "codex",
            "toggle",
            crate::tools::codex::snapshot_paths(),
            "toggle",
        );
        crate::tools::codex::toggle_provider(&host, port)
    })
    .await
}

/// Restore Codex to its pre-MuxLayer state — surgically removes what apply
/// wrote (restoring the values it overwrote), keeping everything the user
/// added since. Used by the UI's "Switch to native mode" button.
#[tauri::command]
#[specta::specta]
pub async fn disable_codex_agentgate(
    state: State<'_, AppState>,
) -> Result<crate::tools::codex::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        record_pre_apply(
            &db,
            "codex",
            "disable",
            crate::tools::codex::snapshot_paths(),
            "disable",
        );
        crate::tools::codex::disable()
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn open_codex_config() -> Result<bool, AppError> {
    crate::tools::codex::open_config()?;
    Ok(true)
}

// ── Claude Desktop Commands（第一阶段：只读 detect + profile 预览，不写盘）──

#[tauri::command]
#[specta::specta]
pub fn detect_claude_desktop() -> crate::tools::claude_desktop::ClaudeDesktopStatus {
    // 保持同步:返回值不是 Result(改成 async 会改变前端 bindings 形状),
    // 且只读两个小 JSON 文件。
    crate::tools::claude_desktop::detect()
}

/// 生成指向 MuxLayer 网关的 3p profile JSON（pretty），仅供和用户机器上实际的
/// Claude Desktop 3p 配置对比、确认 schema，不写任何文件。
#[tauri::command]
#[specta::specta]
pub fn preview_claude_desktop_profile(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port) = gateway_host_port(&state.db)?;
    let token = crate::security::local_token::ensure_token()?;
    let profile = crate::tools::claude_desktop::generate_profile(&host, port, &token);
    serde_json::to_string_pretty(&profile)
        .map_err(|e| AppError::internal(format!("serialize profile failed: {e}")))
}

/// 接入 Claude Desktop：写 3p profile + 切 appliedId 到 MuxLayer。apply 前先经
/// apply_history 快照 profile/_meta，用户可在客户端历史里一键回滚。
#[tauri::command]
#[specta::specta]
pub async fn apply_claude_desktop_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::claude_desktop::ClaudeDesktopApplyResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port(&db)?;
        let token = crate::security::local_token::ensure_token()?;
        record_pre_apply(
            &db,
            "claude_desktop",
            "apply",
            crate::tools::claude_desktop::snapshot_paths(),
            "apply",
        );
        crate::tools::claude_desktop::apply(&host, port, &token)
    })
    .await
}

// ── Claude Code Commands ──────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn detect_claude_code_env(
) -> Result<crate::tools::claude_code::ClaudeCodeEnvStatus, AppError> {
    run_blocking(|| Ok(crate::tools::claude_code::detect_env())).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_claude_code_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::claude_code::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port_with_mappings(
            &db,
            storage::recommended_mappings::MappingProfile::ClaudeCode,
        )?;
        record_pre_apply(
            &db,
            "claude_code",
            "apply",
            crate::tools::claude_code::snapshot_paths(),
            "apply",
        );
        crate::tools::claude_code::apply_config(&host, port, "claude-sonnet-4-7")
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_claude_code_provider(
    state: State<'_, AppState>,
) -> Result<crate::tools::claude_code::ToggleResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port_with_mappings(
            &db,
            storage::recommended_mappings::MappingProfile::ClaudeCode,
        )?;
        record_pre_apply(
            &db,
            "claude_code",
            "toggle",
            crate::tools::claude_code::snapshot_paths(),
            "toggle",
        );
        crate::tools::claude_code::toggle_provider(&host, port, "claude-sonnet-4-7")
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn open_claude_code_config() -> Result<bool, AppError> {
    crate::tools::claude_code::open_config()?;
    Ok(true)
}

#[tauri::command]
#[specta::specta]
pub fn generate_claude_code_env(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port) = gateway_host_port(&state.db)?;
    Ok(crate::tools::claude_code::generate_env_snippet(
        &host,
        port,
        "claude-sonnet-4-7",
    ))
}

// ── OpenCode Commands ─────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn detect_opencode_config(
) -> Result<crate::tools::opencode::OpenCodeConfigStatus, AppError> {
    run_blocking(|| Ok(crate::tools::opencode::detect())).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_opencode_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::opencode::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port(&db)?;
        record_pre_apply(
            &db,
            "opencode",
            "apply",
            crate::tools::opencode::snapshot_paths(),
            "apply",
        );
        crate::tools::opencode::apply(&host, port)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn generate_opencode_config(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port) = gateway_host_port(&state.db)?;
    Ok(crate::tools::opencode::generate_snippet(&host, port))
}

#[tauri::command]
#[specta::specta]
pub fn open_opencode_config() -> Result<bool, AppError> {
    crate::tools::opencode::open_config()?;
    Ok(true)
}

// ── Gemini CLI Config Commands ─────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn detect_gemini_config(
) -> Result<crate::tools::gemini_cli::GeminiCliConfigStatus, AppError> {
    run_blocking(|| Ok(crate::tools::gemini_cli::detect())).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_gemini_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::gemini_cli::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port, model) = gateway_host_port_model(&db, "gemini-2.5-flash")?;
        record_pre_apply(
            &db,
            "gemini",
            "apply",
            crate::tools::gemini_cli::snapshot_paths(),
            "apply",
        );
        crate::tools::gemini_cli::apply(&host, port, &model)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn generate_gemini_config(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port) = gateway_host_port(&state.db)?;
    Ok(crate::tools::gemini_cli::generate_snippet(
        &host,
        port,
        "gemini-2.5-flash",
    ))
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_gemini_provider(
    state: State<'_, AppState>,
) -> Result<crate::tools::gemini_cli::ToggleResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port, model) = gateway_host_port_model(&db, "gemini-2.5-flash")?;
        record_pre_apply(
            &db,
            "gemini",
            "toggle",
            crate::tools::gemini_cli::snapshot_paths(),
            "toggle",
        );
        crate::tools::gemini_cli::toggle(&host, port, &model)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn open_gemini_config() -> Result<bool, AppError> {
    crate::tools::gemini_cli::open_config()?;
    Ok(true)
}

// ── AtomCode Config Commands ──────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn detect_atomcode_config(
) -> Result<crate::tools::atomcode::AtomCodeConfigStatus, AppError> {
    run_blocking(|| Ok(crate::tools::atomcode::detect())).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_atomcode_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::atomcode::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port, model) = gateway_host_port_model(&db, "gpt-5.5")?;
        record_pre_apply(
            &db,
            "atomcode",
            "apply",
            crate::tools::atomcode::snapshot_paths(),
            "apply",
        );
        crate::tools::atomcode::apply(&host, port, &model)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn detect_kimi_config() -> Result<crate::tools::kimi_cli::KimiCliConfigStatus, AppError> {
    run_blocking(|| Ok(crate::tools::kimi_cli::detect())).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_kimi_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::kimi_cli::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port(&db)?;
        record_pre_apply(
            &db,
            "kimi_cli",
            "apply",
            crate::tools::kimi_cli::snapshot_paths(),
            "apply",
        );
        crate::tools::kimi_cli::apply(&host, port)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn generate_kimi_config(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port) = gateway_host_port(&state.db)?;
    Ok(crate::tools::kimi_cli::generate_snippet(&host, port))
}

#[tauri::command]
#[specta::specta]
pub fn open_kimi_config() -> Result<bool, AppError> {
    crate::tools::kimi_cli::open_config()?;
    Ok(true)
}

#[tauri::command]
#[specta::specta]
pub async fn detect_grok_config(
) -> Result<crate::tools::grok_build::GrokBuildConfigStatus, AppError> {
    run_blocking(|| Ok(crate::tools::grok_build::detect())).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_grok_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::grok_build::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port(&db)?;
        record_pre_apply(
            &db,
            "grok_build",
            "apply",
            crate::tools::grok_build::snapshot_paths(),
            "apply",
        );
        crate::tools::grok_build::apply(&host, port)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn generate_grok_config(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port) = gateway_host_port(&state.db)?;
    Ok(crate::tools::grok_build::generate_snippet(&host, port))
}

#[tauri::command]
#[specta::specta]
pub fn open_grok_config() -> Result<bool, AppError> {
    crate::tools::grok_build::open_config()?;
    Ok(true)
}

#[tauri::command]
#[specta::specta]
pub async fn detect_dsh_config(
) -> Result<crate::tools::deepseek_harness::DeepSeekHarnessConfigStatus, AppError> {
    run_blocking(|| Ok(crate::tools::deepseek_harness::detect())).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_dsh_config(
    state: State<'_, AppState>,
) -> Result<crate::tools::deepseek_harness::ApplyConfigResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port) = gateway_host_port(&db)?;
        record_pre_apply(
            &db,
            "deepseek_harness",
            "apply",
            crate::tools::deepseek_harness::snapshot_paths(),
            "apply",
        );
        crate::tools::deepseek_harness::apply(&host, port)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn generate_dsh_config(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port) = gateway_host_port(&state.db)?;
    Ok(crate::tools::deepseek_harness::generate_snippet(
        &host, port,
    ))
}

#[tauri::command]
#[specta::specta]
pub fn open_dsh_config() -> Result<bool, AppError> {
    crate::tools::deepseek_harness::open_config()?;
    Ok(true)
}

#[tauri::command]
#[specta::specta]
pub fn generate_atomcode_config(state: State<'_, AppState>) -> Result<String, AppError> {
    let (host, port, model) = gateway_host_port_model(&state.db, "gpt-5.5")?;
    Ok(crate::tools::atomcode::generate_snippet(
        &host, port, &model,
    ))
}

/// 每个客户端的 CLI 可执行文件名(精确 basename,见 process_detect 的匹配规则)。
/// 桌面 App(Claude Desktop、Grok、Codex / ChatGPT 桌面端)不在这里——它们不读
/// 这些 CLI 配置,也不该出现在「结束进程」列表里;Codex 桌面端走
/// `restart_codex_desktop`。
fn client_process_names(client_id: &str) -> Result<&'static [&'static str], AppError> {
    match client_id {
        "codex" => Ok(&["codex"]),
        "claude_code" => Ok(&["claude"]),
        "opencode" => Ok(&["opencode"]),
        "gemini" | "gemini_cli" => Ok(&["gemini"]),
        "atomcode" => Ok(&["atomcode"]),
        "kimi_cli" | "kimi" => Ok(&["kimi"]),
        "grok_build" | "grok" => Ok(&["grok"]),
        "deepseek_harness" | "dsh" => Ok(&["dsh"]),
        _ => Err(AppError::validation("unknown client_id")),
    }
}

/// After a client's config is rewritten, look up matching live CLI processes
/// so the UI can warn the user that the existing session needs to be
/// restarted to pick up the new config. The caller treats an empty list as
/// "couldn't detect", not "OK".
#[tauri::command]
#[specta::specta]
pub async fn detect_client_running(
    client_id: String,
) -> Result<Vec<crate::tools::process_detect::RunningProcess>, AppError> {
    let names = client_process_names(&client_id)?;
    run_blocking(move || Ok(crate::tools::process_detect::find_running(names))).await
}

/// Stop one process that `detect_client_running` currently lists for this
/// client. Re-checks the live list so a stale dialog PID cannot kill something
/// else. Never auto-called; only the PostApply "结束进程" button fires this.
#[tauri::command]
#[specta::specta]
pub async fn kill_client_process(client_id: String, pid: u32) -> Result<(), AppError> {
    let names = client_process_names(&client_id)?;
    run_blocking(move || {
        let live = crate::tools::process_detect::find_running(names);
        match crate::tools::process_detect::kill_check(pid, &live) {
            crate::tools::process_detect::KillCheck::Ok => {}
            crate::tools::process_detect::KillCheck::InvalidPid
            | crate::tools::process_detect::KillCheck::SelfProcess => {
                return Err(AppError::new(
                    crate::errors::codes::CLIENT_PROCESS_KILL_DENIED,
                    "Refusing to kill this process",
                ));
            }
            crate::tools::process_detect::KillCheck::NotFound => {
                return Err(AppError::new(
                    crate::errors::codes::CLIENT_PROCESS_NOT_FOUND,
                    format!("PID {pid} is no longer a {client_id} process"),
                ));
            }
        }
        crate::tools::process_detect::terminate(pid).map_err(|e| {
            AppError::new(
                crate::errors::codes::CLIENT_PROCESS_KILL_FAILED,
                format!("Failed to stop PID {pid}: {e}"),
            )
        })
    })
    .await
}

/// Restart Codex Desktop so freshly-written config.toml / auth.json take
/// effect. macOS / Windows only — returns `supported: false` elsewhere and the
/// UI hides the button. Never kills the ChatGPT desktop app; see
/// `chatgpt_needs_manual_restart`.
#[tauri::command]
#[specta::specta]
pub async fn restart_codex_desktop(
) -> Result<crate::tools::codex_restart::CodexRestartResult, AppError> {
    run_blocking(crate::tools::codex_restart::restart).await
}

/// 「重启 Codex」按钮是否可用:macOS 装了 Codex.app(/Applications 或
/// ~/Applications)、Windows 找到 Codex.exe 时为 true,其它平台 false。只做 stat。
#[tauri::command]
#[specta::specta]
pub fn codex_desktop_available() -> bool {
    crate::tools::codex_restart::desktop_available()
}

/// 读取各客户端(Codex / Claude Code)现有的 MCP server 配置，汇总展示。
/// 以客户端文件为真相源，只读不写；env 只返回 key 不返回 value。
#[tauri::command]
#[specta::specta]
pub async fn list_mcp_servers() -> Result<Vec<crate::tools::mcp::McpServer>, AppError> {
    run_blocking(crate::tools::mcp::list_all).await
}

/// 添加或更新指定客户端的 MCP server。只写入一个客户端配置文件，不做跨客户端同步。
#[tauri::command]
#[specta::specta]
pub async fn upsert_mcp_server(
    input: crate::tools::mcp::UpsertMcpServerInput,
) -> Result<crate::tools::mcp::McpServer, AppError> {
    run_blocking(move || crate::tools::mcp::upsert(input)).await
}

/// 删除指定客户端的 MCP server。文件或 server 不存在时返回 false。
#[tauri::command]
#[specta::specta]
pub async fn delete_mcp_server(client: String, name: String) -> Result<bool, AppError> {
    run_blocking(move || crate::tools::mcp::delete(&client, &name)).await
}

/// 将一个客户端里的 MCP server 显式同步到一个或多个目标客户端。
#[tauri::command]
#[specta::specta]
pub async fn sync_mcp_server(
    input: crate::tools::mcp::SyncMcpServerInput,
) -> Result<Vec<crate::tools::mcp::McpServer>, AppError> {
    run_blocking(move || crate::tools::mcp::sync(input)).await
}

/// 导出 MCP server 配置。默认由前端传 include_secrets=false，不导出 env value。
#[tauri::command]
#[specta::specta]
pub async fn export_mcp_servers(include_secrets: bool) -> Result<String, AppError> {
    run_blocking(move || crate::tools::mcp::export_config(include_secrets)).await
}

/// 从 JSON 文本导入 MCP server 配置到指定客户端。
#[tauri::command]
#[specta::specta]
pub async fn import_mcp_servers(
    payload: String,
    target_clients: Vec<String>,
) -> Result<Vec<crate::tools::mcp::McpServer>, AppError> {
    run_blocking(move || crate::tools::mcp::import_config(&payload, target_clients)).await
}

/// 曾经 apply 过配置的客户端 id 列表。前端用来判断「配置漂移」：客户端 detected
/// 但 id 在这个列表里，说明接入过又被改回去了，提示重新应用。
#[tauri::command]
#[specta::specta]
pub fn clients_with_apply_history(state: State<'_, AppState>) -> Result<Vec<String>, AppError> {
    let conn = db_conn(&state.db)?;
    storage::apply_history::distinct_clients(&conn)
}

/// 列出某客户端的 apply/disable/toggle 历史（按时间倒序）。前端用来
/// 渲染历史抽屉。
#[tauri::command]
#[specta::specta]
pub fn list_client_apply_history(
    state: State<'_, AppState>,
    client_id: String,
) -> Result<Vec<storage::apply_history::HistoryEntry>, AppError> {
    let conn = db_conn(&state.db)?;
    storage::apply_history::list(&conn, &client_id)
}

/// 回滚到某条历史记录所代表的盘上状态。snapshot 反序列化后按 file 写回原
/// absolute_path（不存在的文件被删除）。回滚本身**不**
/// 记录新历史，避免反复回滚把保留窗撑满。
#[tauri::command]
#[specta::specta]
pub async fn rollback_client_apply(
    state: State<'_, AppState>,
    history_id: String,
) -> Result<storage::apply_history::HistoryEntry, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let entry = {
            let conn = db_conn(&db)?;
            storage::apply_history::get(&conn, &history_id)?
        };
        let snapshot: storage::apply_history::ClientSnapshot =
            serde_json::from_str(&entry.snapshot_json)
                .map_err(|e| AppError::internal(format!("snapshot deserialise failed: {e}")))?;
        storage::apply_history::restore_files(&snapshot)?;
        Ok(entry)
    })
    .await
}

/// 删除一条配置历史记录(初始快照受保护,不可删)。客户端配置历史和全局指令
/// 历史共用 `client_apply_history` 表,故两处删除都走这里。
#[tauri::command]
#[specta::specta]
pub fn delete_client_apply_history(
    state: State<'_, AppState>,
    history_id: String,
) -> Result<(), AppError> {
    let conn = db_conn(&state.db)?;
    storage::apply_history::delete(&conn, &history_id)
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_atomcode_provider(
    state: State<'_, AppState>,
) -> Result<crate::tools::atomcode::ToggleResult, AppError> {
    let db = state.db.clone();
    run_blocking(move || {
        let (host, port, model) = gateway_host_port_model(&db, "gpt-5.5")?;
        record_pre_apply(
            &db,
            "atomcode",
            "toggle",
            crate::tools::atomcode::snapshot_paths(),
            "toggle",
        );
        crate::tools::atomcode::toggle(&host, port, &model)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn open_atomcode_config() -> Result<bool, AppError> {
    crate::tools::atomcode::open_config()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;

    use super::*;
    use crate::app::state::AppState;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn test_state() -> AppState {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder().max_size(1).build(manager).unwrap();
        {
            let conn = pool.get().unwrap();
            crate::storage::migrations::run_migrations(&conn).unwrap();
        }
        AppState {
            db: pool,
            gateway_runtime: Arc::new(Mutex::new(
                crate::models::gateway::GatewayRuntimeState::default(),
            )),
            wake: crate::wake::WakeManager::new(),
            pet_click_through: Arc::new(Mutex::new(false)),
        }
    }

    /// Convert a borrowed `AppState` into Tauri's `State` wrapper.
    /// This is test-only: `State` is a single-field struct around `&AppState`.
    unsafe fn as_state<'r>(state: &'r AppState) -> tauri::State<'r, AppState> {
        std::mem::transmute(state)
    }

    /// 命令是 async 的;测试在同步上下文里(持有 FS_LOCK)驱动它们。
    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    #[test]
    fn list_tools_returns_all_eight_clients() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let tools = list_tools().unwrap();
        let ids: Vec<_> = tools.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "claude-code",
                "codex",
                "opencode",
                "atomcode",
                "gemini_cli",
                "kimi_cli",
                "grok_build",
                "deepseek_harness",
            ]
        );
        cleanup(&temp);
    }

    /// 回归:list_tools 硬编码 ~/.kimi / ~/.grok / ~/.dsh,忽略 KIMI_CODE_HOME /
    /// GROK_HOME / DSH_HOME 覆盖,展示的路径和实际写入的路径不一致。
    #[test]
    fn list_tools_uses_module_config_paths_with_env_overrides() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let kimi = temp.join("custom-kimi");
        let grok = temp.join("custom-grok");
        let dsh = temp.join("custom-dsh");
        std::fs::create_dir_all(&kimi).unwrap();
        std::fs::write(kimi.join("config.toml"), "").unwrap();
        std::env::set_var("KIMI_CODE_HOME", &kimi);
        std::env::set_var("GROK_HOME", &grok);
        std::env::set_var("DSH_HOME", &dsh);
        let tools = list_tools().unwrap();
        std::env::remove_var("KIMI_CODE_HOME");
        std::env::remove_var("GROK_HOME");
        std::env::remove_var("DSH_HOME");
        let by_id = |id: &str| tools.iter().find(|t| t.id == id).unwrap().clone();
        assert_eq!(
            by_id("kimi_cli").config_path,
            kimi.join("config.toml").to_string_lossy()
        );
        assert!(by_id("kimi_cli").config_exists);
        assert_eq!(
            by_id("grok_build").config_path,
            grok.join("config.toml").to_string_lossy()
        );
        assert_eq!(
            by_id("deepseek_harness").config_path,
            dsh.join("settings.yaml").to_string_lossy()
        );
        cleanup(&temp);
    }

    #[test]
    fn list_tools_reflects_config_existence() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let codex_dir = temp.join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(codex_dir.join("config.toml"), "[model_providers.OpenAI]\n").unwrap();

        let tools = list_tools().unwrap();
        let codex = tools.iter().find(|t| t.id == "codex").unwrap();
        assert!(codex.config_exists);
        cleanup(&temp);
    }

    #[test]
    fn generate_codex_config_uses_gateway_settings_and_token() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        let snippet = generate_codex_config(unsafe { as_state(&state) }).unwrap();
        assert!(snippet.contains("model_provider = \"OpenAI\""));
        assert!(snippet.contains("requires_openai_auth = true"));
        assert!(snippet.contains("experimental_bearer_token = \"ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn detect_codex_config_reports_missing_config() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let status = block_on(detect_codex_config()).unwrap();
        assert!(!status.exists);
        assert!(!status.has_agentgate);
        cleanup(&temp);
    }

    #[test]
    fn detect_codex_config_recognises_agentgate_snippet() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let codex_dir = temp.join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(
            codex_dir.join("config.toml"),
            crate::tools::codex::generate_snippet("127.0.0.1", 9090, "ag_local_testtoken"),
        )
        .unwrap();
        let status = block_on(detect_codex_config()).unwrap();
        assert!(status.exists);
        assert!(status.has_agentgate);
        assert!(status.is_agentgate_active);
        assert_eq!(status.current_provider, Some("OpenAI".to_string()));
        cleanup(&temp);
    }

    #[test]
    fn apply_codex_config_creates_hijack_config() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        let result = block_on(apply_codex_config(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        assert!(crate::tools::codex::config_path().unwrap().exists());
        let cfg = std::fs::read_to_string(crate::tools::codex::config_path().unwrap()).unwrap();
        assert!(cfg.contains("model_provider = \"OpenAI\""));
        assert!(cfg.contains("requires_openai_auth = true"));
        cleanup(&temp);
    }

    #[test]
    fn apply_codex_config_records_history() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        let result = block_on(apply_codex_config(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        let clients = clients_with_apply_history(unsafe { as_state(&state) }).unwrap();
        assert!(clients.contains(&"codex".to_string()));
        let history =
            list_client_apply_history(unsafe { as_state(&state) }, "codex".to_string()).unwrap();
        assert!(!history.is_empty());
        assert_eq!(history[0].action, "apply");
        cleanup(&temp);
    }

    #[test]
    fn toggle_codex_provider_round_trip() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        // Start from an official config so apply saves a restore point.
        std::fs::create_dir_all(
            crate::tools::codex::config_path()
                .unwrap()
                .parent()
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            crate::tools::codex::config_path().unwrap(),
            "model_provider = \"openai\"\n",
        )
        .unwrap();

        let result = block_on(apply_codex_config(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        assert!(crate::tools::codex::detect().is_agentgate_active);

        let result = block_on(toggle_codex_provider(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        assert!(!crate::tools::codex::detect().is_agentgate_active);

        let result = block_on(toggle_codex_provider(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        assert_eq!(result.new_provider, "agentgate");
        assert!(crate::tools::codex::detect().is_agentgate_active);
        cleanup(&temp);
    }

    #[test]
    fn disable_codex_agentgate_restores_official_config() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        std::fs::create_dir_all(
            crate::tools::codex::config_path()
                .unwrap()
                .parent()
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            crate::tools::codex::config_path().unwrap(),
            "model_provider = \"openai\"\n[plugins.\"browser@openai-bundled\"]\nenabled = true\n",
        )
        .unwrap();

        let result = block_on(apply_codex_config(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        assert!(crate::tools::codex::detect().is_agentgate_active);

        let result = block_on(disable_codex_agentgate(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        let cfg = std::fs::read_to_string(crate::tools::codex::config_path().unwrap()).unwrap();
        assert!(cfg.contains("model_provider = \"openai\""));
        assert!(cfg.contains("browser@openai-bundled"));
        cleanup(&temp);
    }

    #[test]
    fn clients_with_apply_history_includes_codex_after_apply() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        let result = block_on(apply_codex_config(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        let clients = clients_with_apply_history(unsafe { as_state(&state) }).unwrap();
        assert!(clients.contains(&"codex".to_string()));
        cleanup(&temp);
    }

    #[test]
    fn list_client_apply_history_returns_entries() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        let result = block_on(apply_codex_config(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        let history =
            list_client_apply_history(unsafe { as_state(&state) }, "codex".to_string()).unwrap();
        assert!(!history.is_empty());
        cleanup(&temp);
    }

    #[test]
    fn delete_client_apply_history_removes_entry() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        // First apply creates a protected initial snapshot.
        let result = block_on(apply_codex_config(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        // Second apply creates a deletable entry.
        let result = block_on(apply_codex_config(unsafe { as_state(&state) })).unwrap();
        assert!(result.success);
        let history =
            list_client_apply_history(unsafe { as_state(&state) }, "codex".to_string()).unwrap();
        assert!(history.len() >= 2);
        let id = history
            .iter()
            .find(|h| !h.is_initial)
            .map(|h| h.id.clone())
            .unwrap();
        delete_client_apply_history(unsafe { as_state(&state) }, id.clone()).unwrap();
        let history =
            list_client_apply_history(unsafe { as_state(&state) }, "codex".to_string()).unwrap();
        assert!(!history.iter().any(|h| h.id == id));
        cleanup(&temp);
    }

    #[test]
    fn detect_client_running_rejects_unknown_client() {
        let err = block_on(detect_client_running("unknown".to_string())).unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn detect_client_running_returns_list_for_known_client() {
        let result = block_on(detect_client_running("codex".to_string())).unwrap();
        // On macOS/Linux pgrep may find nothing in CI; empty is a valid shape.
        assert!(result.is_empty() || result.iter().all(|p| !p.command.is_empty()));
    }

    #[test]
    fn kill_client_process_rejects_unknown_client() {
        let err = block_on(kill_client_process("unknown".to_string(), 4242)).unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn kill_client_process_rejects_stale_or_init_pid() {
        let missing =
            block_on(kill_client_process("deepseek_harness".to_string(), 999_999)).unwrap_err();
        assert_eq!(missing.code, crate::errors::codes::CLIENT_PROCESS_NOT_FOUND);
        let init = block_on(kill_client_process("deepseek_harness".to_string(), 1)).unwrap_err();
        assert_eq!(init.code, crate::errors::codes::CLIENT_PROCESS_KILL_DENIED);
    }
}
