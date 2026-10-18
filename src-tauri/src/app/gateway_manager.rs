//! Unified startup coordination for the two independent gateways.
//!
//! The Model Gateway is an in-process HTTP server owned by `AppState`. The
//! Agent Gateway is a separate `agentgate-serve agent-service` process owned by
//! its own single-instance lock. This module only sequences startup and reports
//! status; it does not merge their business logic or lifecycles.

use serde::Serialize;
use tauri::Manager;

use crate::app::agent_gateway as management;
use crate::app::commands::gateway::{gateway_status, start_gateway_inner};
use crate::app::state::AppState;
use crate::errors::AppError;
use crate::models::gateway::GatewayStatus;
use crate::storage::db::DbPool;

pub const AGENT_AUTO_START_SETTING: &str = "agent_gateway.auto_start";

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct GatewayManagerStatus {
    pub model: Option<GatewayStatus>,
    pub model_error: Option<String>,
    pub model_auto_start: bool,
    pub agent: management::AgentGatewayServiceStatus,
    pub agent_auto_start: bool,
    pub bundled_cli: Option<String>,
}

pub fn load_agent_auto_start(db: &DbPool) -> Result<bool, AppError> {
    let conn = db
        .get()
        .map_err(|error| AppError::internal(format!("DB connection failed: {error}")))?;
    let value = crate::storage::app_settings::get(&conn, AGENT_AUTO_START_SETTING)?;
    Ok(value.as_deref() != Some("false"))
}

pub fn save_agent_auto_start(db: &DbPool, enabled: bool) -> Result<bool, AppError> {
    let conn = db
        .get()
        .map_err(|error| AppError::internal(format!("DB connection failed: {error}")))?;
    crate::storage::app_settings::set(
        &conn,
        AGENT_AUTO_START_SETTING,
        if enabled { "true" } else { "false" },
    )?;
    Ok(enabled)
}

fn model_auto_start(db: &DbPool) -> bool {
    db.get()
        .ok()
        .and_then(|conn| crate::storage::gateway_settings::get(&conn).ok())
        .map(|settings| settings.auto_start)
        .unwrap_or(false)
}

pub async fn manager_status(app_handle: tauri::AppHandle) -> GatewayManagerStatus {
    let state = app_handle.state::<AppState>();
    let model = gateway_status(&state);
    let (model, model_error) = match model {
        Ok(status) => (Some(status), None),
        Err(error) => (None, Some(error.message)),
    };
    let model_auto_start = model_auto_start(&state.db);
    let agent_auto_start = load_agent_auto_start(&state.db).unwrap_or(true);

    let dir = management::data_dir().ok();
    let serve = management::load_serve_setting(&state.db).ok().flatten();
    let agent = match dir {
        Some(dir) => {
            let cli = management::probe_serve(serve.as_deref()).await;
            management::service_status(&dir, &cli).await
        }
        None => management::AgentGatewayServiceStatus {
            installed: false,
            cli_path: None,
            cli_version: None,
            data_dir: String::new(),
            running: false,
            service_version: None,
            running_tasks: Vec::new(),
            agents: Vec::new(),
            last_error: None,
            error: Some("Agent Gateway data directory is unavailable".into()),
            install_hint: management::install_hint(),
            download_url: management::DOWNLOAD_URL.into(),
        },
    };
    let bundled_cli = crate::app::bundled_cli::managed_cli_path()
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string());

    GatewayManagerStatus {
        model,
        model_error,
        model_auto_start,
        agent,
        agent_auto_start,
        bundled_cli,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_auto_start_defaults_true_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::storage::db::init_database(&dir.path().to_path_buf()).unwrap();

        assert!(load_agent_auto_start(&db).unwrap());
        save_agent_auto_start(&db, false).unwrap();
        assert!(!load_agent_auto_start(&db).unwrap());
        save_agent_auto_start(&db, true).unwrap();
        assert!(load_agent_auto_start(&db).unwrap());
    }
}

/// Called once from Desktop setup. Each gateway starts independently; a failure
/// in one is logged and never blocks the other or the UI.
pub async fn start_enabled(app_handle: tauri::AppHandle) {
    let state = app_handle.state::<AppState>();
    if model_auto_start(&state.db) {
        let already_running = gateway_status(&state).map(|s| s.running).unwrap_or(false);
        if !already_running {
            match start_gateway_inner(&app_handle, &state).await {
                Ok(_) => eprintln!("[gateway-manager] Model Gateway started"),
                Err(error) => eprintln!(
                    "[gateway-manager] Model Gateway start failed: {}",
                    error.message
                ),
            }
        }
    }

    let agent_enabled = load_agent_auto_start(&state.db).unwrap_or(true);
    if !agent_enabled {
        return;
    }
    let dir = match management::data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!(
                "[gateway-manager] Agent Gateway data directory unavailable: {}",
                error.message
            );
            return;
        }
    };
    let serve = management::load_serve_setting(&state.db).ok().flatten();
    let cli = management::probe_serve(serve.as_deref()).await;
    match management::start_service(&dir, &cli).await {
        Ok(status) if status.running => {
            eprintln!(
                "[gateway-manager] Agent Gateway ready ({})",
                status.service_version.as_deref().unwrap_or("unknown")
            )
        }
        Ok(_) => eprintln!("[gateway-manager] Agent Gateway did not become ready"),
        Err(error) => eprintln!(
            "[gateway-manager] Agent Gateway start failed: {}",
            error.message
        ),
    }
}
