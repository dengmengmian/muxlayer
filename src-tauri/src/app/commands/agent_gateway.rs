//! Tauri commands for the Agent Gateway Desktop pages.

use tauri::State;

use crate::app::agent_gateway as management;
use crate::app::state::AppState;
use crate::errors::{codes, AppError};

fn data_dir() -> Result<std::path::PathBuf, AppError> {
    management::data_dir()
}

#[tauri::command]
#[specta::specta]
pub async fn get_agent_gateway_overview(
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayOverview, AppError> {
    let dir = data_dir()?;
    let serve = management::load_serve_setting(&state.db)?;
    management::overview(&dir, serve.as_deref()).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_agent_gateway_agent_path(
    agent_id: String,
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayPathResult, AppError> {
    let dir = data_dir()?;
    let serve = management::load_serve_setting(&state.db)?;
    let path = path
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from);
    management::save_agent_path(&dir, &agent_id, path, serve.as_deref()).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_agent_gateway_serve_path(
    path: Option<String>,
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayCliStatus, AppError> {
    let path = path
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from);
    management::save_serve_setting(&state.db, path).await
}

#[tauri::command]
#[specta::specta]
pub async fn test_agent_gateway_agent(
    agent_id: String,
    confirmed: bool,
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayTestResult, AppError> {
    if !confirmed {
        return Err(AppError::validation(
            "Confirm that the connection test may consume model usage",
        ));
    }
    let dir = data_dir()?;
    let serve = management::load_serve_setting(&state.db)?;
    let cli = management::probe_serve(serve.as_deref()).await;
    management::test_agent(&agent_id, &dir, &cli, true).await
}

#[tauri::command]
#[specta::specta]
pub async fn start_agent_gateway_service(
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayServiceStatus, AppError> {
    let dir = data_dir()?;
    let serve = management::load_serve_setting(&state.db)?;
    let cli = management::probe_serve(serve.as_deref()).await;
    management::start_service(&dir, &cli).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_agent_gateway_service_status(
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayServiceStatus, AppError> {
    let dir = data_dir()?;
    let serve = management::load_serve_setting(&state.db)?;
    let cli = management::probe_serve(serve.as_deref()).await;
    Ok(management::service_status(&dir, &cli).await)
}

#[tauri::command]
#[specta::specta]
pub async fn stop_agent_gateway_service(
    force: bool,
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayServiceStatus, AppError> {
    let dir = data_dir()?;
    let serve = management::load_serve_setting(&state.db)?;
    let cli = management::probe_serve(serve.as_deref()).await;
    management::stop_service(&dir, &cli, force).await
}

#[tauri::command]
#[specta::specta]
pub async fn restart_agent_gateway_service(
    force: bool,
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayServiceStatus, AppError> {
    let dir = data_dir()?;
    let serve = management::load_serve_setting(&state.db)?;
    let cli = management::probe_serve(serve.as_deref()).await;
    management::restart_service(&dir, &cli, force).await
}

#[tauri::command]
#[specta::specta]
pub async fn configure_agent_gateway_mcp(
    client: String,
    confirmed: bool,
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayMcpResult, AppError> {
    let serve = management::load_serve_setting(&state.db)?;
    management::configure_client(&client, serve.as_deref(), confirmed).await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_agent_gateway_mcp(
    client: String,
    confirmed: bool,
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayMcpResult, AppError> {
    let serve = management::load_serve_setting(&state.db)?;
    management::remove_client(&client, serve.as_deref(), confirmed).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_agent_gateway_mcp_copy(
    client: String,
    state: State<'_, AppState>,
) -> Result<management::AgentGatewayMcpCopy, AppError> {
    let setting = management::load_serve_setting(&state.db)?;
    let cli = management::probe_serve(setting.as_deref()).await;
    management::mcp_copy(&client, cli.path.as_deref())
}

#[tauri::command]
#[specta::specta]
pub async fn get_agent_gateway_diagnostics(state: State<'_, AppState>) -> Result<String, AppError> {
    let dir = data_dir()?;
    let serve = management::load_serve_setting(&state.db)?;
    management::diagnostics(&dir, serve.as_deref()).await
}

#[tauri::command]
#[specta::specta]
pub fn open_agent_gateway_data_dir() -> Result<bool, AppError> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|error| {
        AppError::new(
            codes::AGENT_GATEWAY_PATH_INVALID,
            format!("Cannot create Agent Gateway data directory: {error}"),
        )
    })?;
    open::that(&dir).map_err(|error| AppError::internal(format!("Cannot open folder: {error}")))?;
    Ok(true)
}
