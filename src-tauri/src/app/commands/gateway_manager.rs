//! Tauri commands for the unified Gateway status entry.

use tauri::State;

use crate::app::gateway_manager as manager;
use crate::app::state::AppState;
use crate::errors::AppError;

#[tauri::command]
#[specta::specta]
pub async fn get_gateway_manager_status(
    app_handle: tauri::AppHandle,
) -> Result<manager::GatewayManagerStatus, AppError> {
    Ok(manager::manager_status(app_handle).await)
}

#[tauri::command]
#[specta::specta]
pub fn set_agent_gateway_auto_start(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<bool, AppError> {
    manager::save_agent_auto_start(&state.db, enabled)
}
