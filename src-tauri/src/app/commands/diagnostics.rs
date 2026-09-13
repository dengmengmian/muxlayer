use tauri::State;

use crate::app::state::AppState;
use crate::errors::AppError;

// ── Diagnostics Commands ───────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub fn run_health_check(
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::CheckReport, AppError> {
    Ok(crate::diagnostics::checks::health_check(&state.db))
}

#[tauri::command]
#[specta::specta]
pub fn run_database_check(
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::CheckReport, AppError> {
    Ok(crate::diagnostics::checks::database_check(&state.db))
}

#[tauri::command]
#[specta::specta]
pub fn run_gateway_auth_check(
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::CheckReport, AppError> {
    Ok(crate::diagnostics::checks::gateway_auth_check(&state.db))
}

#[tauri::command]
#[specta::specta]
pub fn run_provider_check(
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::CheckReport, AppError> {
    Ok(crate::diagnostics::checks::provider_check(&state.db))
}

#[tauri::command]
#[specta::specta]
pub fn run_codex_config_check(
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::CheckReport, AppError> {
    Ok(crate::diagnostics::checks::codex_config_check(&state.db))
}

#[tauri::command]
#[specta::specta]
pub fn run_claude_code_config_check(
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::CheckReport, AppError> {
    Ok(crate::diagnostics::checks::claude_code_config_check(
        &state.db,
    ))
}

#[tauri::command]
#[specta::specta]
pub fn run_route_profile_check(
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::CheckReport, AppError> {
    Ok(crate::diagnostics::checks::route_profile_check(&state.db))
}

/// 全量自检要跑多条 SQL + 读客户端配置文件,放到 blocking 线程。
#[tauri::command]
#[specta::specta]
pub async fn run_full_self_test(
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::FullSelfTestReport, AppError> {
    let db = state.db.clone();
    super::run_blocking(move || Ok(crate::diagnostics::checks::full_self_test(&db))).await
}

/// 导出诊断包包含全量自检 + 写多个文件,同样不占主线程。
#[tauri::command]
#[specta::specta]
pub async fn export_diagnostic_bundle(
    include_logs: Option<bool>,
    max_logs: Option<u32>,
    state: State<'_, AppState>,
) -> Result<crate::diagnostics::report::ExportResult, AppError> {
    let db = state.db.clone();
    super::run_blocking(move || {
        crate::diagnostics::checks::export_bundle(
            &db,
            include_logs.unwrap_or(true),
            max_logs.unwrap_or(50) as usize,
        )
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub fn open_app_data_dir() -> Result<bool, AppError> {
    let dir = crate::security::local_token::token_dir();
    open::that(&dir).map_err(|e| AppError::internal(format!("Cannot open: {e}")))?;
    Ok(true)
}
