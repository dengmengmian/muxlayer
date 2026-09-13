use rusqlite::Connection;
use tauri::State;

use crate::app::state::AppState;
use crate::errors::AppError;
use crate::models::request_log::{RequestLogDetail, RequestLogFilter, RequestLogListItem};
use crate::storage;

// ── Logs Commands ──────────────────────────────────────────────
//
// Tauri v2 里非 async 命令跑在主线程;日志表扫描 / 聚合 / VACUUM 会卡 UI。
// 这里的命令都是 async,DB 与文件 IO 放进阻塞线程池执行。

/// 在阻塞线程池里借一个连接执行 `f`。
async fn run_blocking_db<T, F>(state: &State<'_, AppState>, f: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, AppError> + Send + 'static,
{
    let db = state.db.clone();
    run_blocking(move || {
        let conn = db.get().map_err(|_| AppError::internal("DB lock failed"))?;
        f(&conn)
    })
    .await
}

async fn run_blocking<T, F>(f: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal(format!("Blocking task failed: {e}")))?
}

#[tauri::command]
#[specta::specta]
pub async fn list_request_logs(
    filter: RequestLogFilter,
    state: State<'_, AppState>,
) -> Result<Vec<RequestLogListItem>, AppError> {
    run_blocking_db(&state, move |conn| {
        storage::request_logs::list(conn, filter)
    })
    .await
}

/// 日志里出现过的去重模型名——Logs 页「模型」筛选下拉用。
#[tauri::command]
#[specta::specta]
pub async fn list_log_models(state: State<'_, AppState>) -> Result<Vec<String>, AppError> {
    run_blocking_db(&state, storage::request_logs::distinct_models).await
}

/// 读取某个会话的完整对话（会话详情视图用）。直接读本地 jsonl，不走 DB。
/// 先试 Claude Code 日志，找不到再试 Codex 日志。
#[tauri::command]
#[specta::specta]
pub async fn get_session_conversation(
    session_id: String,
) -> Result<Vec<crate::session_sync::claude::ConversationMessage>, AppError> {
    run_blocking(move || {
        if let Ok(msgs) = crate::session_sync::claude::read_conversation(&session_id) {
            if !msgs.is_empty() {
                return Ok(msgs);
            }
        }
        crate::session_sync::codex::read_conversation(&session_id)
    })
    .await
}

/// 删除某个会话：删 request_logs 行 + 删 Claude/Codex 本地 jsonl 文件。
/// 一个会话只在一处客户端，另一处 delete_session_file 返回 Ok(false)；删除失败传播 Err。
#[tauri::command]
#[specta::specta]
pub async fn delete_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    run_blocking_db(&state, move |conn| {
        storage::request_logs::delete_by_session(conn, &session_id)?;
        crate::session_sync::claude::delete_session_file(&session_id)?;
        crate::session_sync::codex::delete_session_file(&session_id)?;
        Ok(())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn count_request_logs(
    filter: RequestLogFilter,
    state: State<'_, AppState>,
) -> Result<i64, AppError> {
    run_blocking_db(&state, move |conn| {
        storage::request_logs::count(conn, &filter)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn get_request_log_detail(
    id: String,
    state: State<'_, AppState>,
) -> Result<RequestLogDetail, AppError> {
    run_blocking_db(&state, move |conn| {
        storage::request_logs::get_detail(conn, &id)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn clear_request_logs(state: State<'_, AppState>) -> Result<bool, AppError> {
    // clear 会 VACUUM,必须离开主线程。
    run_blocking_db(&state, storage::request_logs::clear).await
}

/// 按 session_id 聚合用量：Logs 页「按会话分组」视图用。
/// 返回最近 `limit` 个会话，按最后活跃时间倒序排列。
#[tauri::command]
#[specta::specta]
pub async fn aggregate_request_logs_by_session(
    filter: RequestLogFilter,
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::request_log::SessionUsageSummary>, AppError> {
    run_blocking_db(&state, move |conn| {
        storage::request_logs::aggregate_by_session(conn, &filter, limit.unwrap_or(100))
    })
    .await
}

/// days 为 None 时统计全量；Some(n) 时只统计近 n 天（与 Dashboard rangeDays 对齐）。
fn cost_since(days: Option<i64>) -> Option<String> {
    days.map(|d| (chrono::Utc::now() - chrono::Duration::days(d.max(1))).to_rfc3339())
}

/// 按模型聚合成本——成本仪表盘「钱花在哪个模型」用。
#[tauri::command]
#[specta::specta]
pub async fn aggregate_cost_by_model(
    days: Option<i64>,
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::request_log::CostBreakdown>, AppError> {
    let since = cost_since(days);
    run_blocking_db(&state, move |conn| {
        storage::request_logs::aggregate_cost_by_model(conn, since.as_deref(), limit.unwrap_or(50))
    })
    .await
}

/// 按客户端聚合成本——成本仪表盘「哪个客户端花得多」用。
#[tauri::command]
#[specta::specta]
pub async fn aggregate_cost_by_client(
    days: Option<i64>,
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::request_log::CostBreakdown>, AppError> {
    let since = cost_since(days);
    run_blocking_db(&state, move |conn| {
        storage::request_logs::aggregate_cost_by_client(conn, since.as_deref(), limit.unwrap_or(50))
    })
    .await
}

/// Provider 详情页：按模型聚合成功率/成本，并返回最近延迟点。
#[tauri::command]
#[specta::specta]
pub async fn aggregate_provider_detail_stats(
    provider: String,
    days: Option<i64>,
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<crate::models::request_log::ProviderDetailStats, AppError> {
    let since = cost_since(days);
    run_blocking_db(&state, move |conn| {
        storage::request_logs::aggregate_provider_detail_stats(
            conn,
            &provider,
            since.as_deref(),
            limit.unwrap_or(50),
        )
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn aggregate_route_profile_stats(
    days: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::route_profile::RouteProfileStats>, AppError> {
    let since = cost_since(days);
    run_blocking_db(&state, move |conn| {
        storage::request_logs::aggregate_route_profile_stats(conn, since.as_deref())
    })
    .await
}

/// 扫描 ~/.claude/projects 下的 Claude Code 会话日志并写入 request_logs。
/// 幂等：已同步过的 message_id 会被跳过。
#[tauri::command]
#[specta::specta]
pub async fn sync_claude_sessions(
    state: State<'_, AppState>,
) -> Result<crate::session_sync::SyncResult, AppError> {
    // 首次同步可能扫描/写入数万行,放到阻塞线程池,不占 tokio worker。
    let db = state.db.clone();
    run_blocking(move || crate::session_sync::claude::sync(&db)).await
}

/// 扫描 ~/.codex/sessions 下的 Codex 会话日志并写入 request_logs。
/// 幂等：external_id = "{session_id}:{event_index}" 保证再次同步只写新增。
#[tauri::command]
#[specta::specta]
pub async fn sync_codex_sessions(
    state: State<'_, AppState>,
) -> Result<crate::session_sync::SyncResult, AppError> {
    // 首次同步可能扫描/写入数万行,放到阻塞线程池,不占 tokio worker。
    let db = state.db.clone();
    run_blocking(move || crate::session_sync::codex::sync(&db)).await
}

/// 扫描 ~/.gemini/tmp/(session)/chats 下的 Gemini CLI 会话日志并写入 request_logs。
/// 幂等：event 自带 UUID id 作 external_id。
#[tauri::command]
#[specta::specta]
pub async fn sync_gemini_sessions(
    state: State<'_, AppState>,
) -> Result<crate::session_sync::SyncResult, AppError> {
    // 首次同步可能扫描/写入数万行,放到阻塞线程池,不占 tokio worker。
    let db = state.db.clone();
    run_blocking(move || crate::session_sync::gemini::sync(&db)).await
}

// ── Stats Commands ─────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn get_request_stats(
    state: State<'_, AppState>,
) -> Result<crate::storage::request_logs::RequestStats, AppError> {
    run_blocking_db(&state, storage::request_logs::get_stats).await
}

/// Stats over a configurable window (in days). Dashboard date-range tabs
/// (今天/7天/14天/30天) call this with 1/7/14/30 respectively.
#[tauri::command]
#[specta::specta]
pub async fn get_request_stats_range(
    days: i64,
    state: State<'_, AppState>,
) -> Result<crate::storage::request_logs::RequestStats, AppError> {
    run_blocking_db(&state, move |conn| {
        storage::request_logs::get_stats_for_range(conn, days)
    })
    .await
}

/// Live runtime KPIs surfaced in the bottom footer of Dashboard / Routes.
/// Combines runtime-only state (active_requests, uptime) with lifetime
/// aggregate metrics that used to live in a separate "累计" strip. Today
/// stats are intentionally NOT included here — the Dashboard's "今日"
/// strip already covers them, the footer focuses on the long-running view.
#[derive(serde::Serialize, specta::Type)]
pub struct RuntimeKpis {
    /// Currently in-flight requests at the proxy layer.
    pub active_requests: u64,
    /// Seconds since the gateway was started; 0 when stopped.
    pub uptime_seconds: i64,
    pub gateway_running: bool,
    pub gateway_port: u16,
    /// Lifetime totals — folded in from the old "累计" strip so the footer
    /// is the single source of truth for "long-running scoreboard" info.
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub success_rate_lifetime: f64,
}

#[tauri::command]
#[specta::specta]
pub async fn get_runtime_kpis(state: State<'_, AppState>) -> Result<RuntimeKpis, AppError> {
    // 锁守卫限定在块内,不能跨 await 持有(std MutexGuard 非 Send)。
    let (active_requests, gateway_running, gateway_port, uptime_seconds) = {
        let runtime = state
            .gateway_runtime
            .lock()
            .map_err(|_| AppError::internal("Runtime lock failed"))?;
        let active_requests = runtime
            .active_requests
            .as_ref()
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0);
        let uptime_seconds = runtime
            .started_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|started| (chrono::Utc::now() - started.with_timezone(&chrono::Utc)).num_seconds())
            .unwrap_or(0);
        (
            active_requests,
            runtime.running,
            runtime.port,
            uptime_seconds,
        )
    };

    let stats = run_blocking_db(&state, storage::request_logs::get_stats).await?;
    Ok(RuntimeKpis {
        active_requests,
        uptime_seconds,
        gateway_running,
        gateway_port,
        total_requests: stats.total,
        total_tokens: stats.total_input_tokens + stats.total_output_tokens,
        total_cost: stats.total_cost,
        success_rate_lifetime: stats.success_rate,
    })
}
