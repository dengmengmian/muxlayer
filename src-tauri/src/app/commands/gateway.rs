use tauri::State;
use tauri_specta::Event;

use crate::app::events::{PetBubble, PetGatewayStateChanged};
use crate::app::state::AppState;
use crate::errors::AppError;
use crate::gateway;
use crate::models::gateway::{GatewaySettings, GatewayStatus, UpdateGatewaySettingsInput};
use crate::storage;

// ── Gateway Commands ───────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub fn get_gateway_status(state: State<'_, AppState>) -> Result<GatewayStatus, AppError> {
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    let settings = storage::gateway_settings::get(&conn)?;
    let runtime = state
        .gateway_runtime
        .lock()
        .map_err(|_| AppError::internal("Runtime lock failed"))?;

    let active_provider = if let Some(ref pid) = settings.active_provider_id {
        storage::providers::get_by_id(&conn, pid)
            .ok()
            .map(|p| p.name)
    } else {
        None
    };

    Ok(GatewayStatus {
        running: runtime.running,
        host: if runtime.running {
            runtime.host.clone()
        } else {
            settings.host
        },
        port: if runtime.running {
            runtime.port as i64
        } else {
            settings.port
        },
        active_provider,
        input_protocol: settings.input_protocol,
        output_protocol: settings.output_protocol,
        started_at: runtime.started_at.clone(),
    })
}

#[tauri::command]
#[specta::specta]
pub fn get_gateway_settings(state: State<'_, AppState>) -> Result<GatewaySettings, AppError> {
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    storage::gateway_settings::get(&conn)
}

#[tauri::command]
#[specta::specta]
pub fn update_gateway_settings(
    input: UpdateGatewaySettingsInput,
    state: State<'_, AppState>,
) -> Result<GatewaySettings, AppError> {
    let wake_changed = input.wake_enabled.is_some()
        || input.wake_request_control.is_some()
        || input.wake_cooldown_seconds.is_some()
        || input.wake_keep_display_awake.is_some();
    let conn = state
        .db
        .get()
        .map_err(|_| AppError::internal("DB lock failed"))?;
    let settings = storage::gateway_settings::update(&conn, input)?;
    if wake_changed {
        state.wake.set_config(wake_config_from_settings(&settings));
    }
    Ok(settings)
}

#[tauri::command]
#[specta::specta]
pub fn get_wake_status(state: State<'_, AppState>) -> Result<crate::wake::WakeStatus, AppError> {
    Ok(state.wake.status())
}

pub(crate) fn wake_config_from_settings(settings: &GatewaySettings) -> crate::wake::WakeConfig {
    crate::wake::WakeConfig {
        enabled: settings.wake_enabled,
        request_control: settings.wake_request_control,
        cooldown_seconds: settings.wake_cooldown_seconds.clamp(0, 86_400) as u64,
        keep_display_awake: settings.wake_keep_display_awake,
    }
}

#[tauri::command]
#[specta::specta]
pub async fn start_gateway(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<GatewayStatus, AppError> {
    // Check if already running
    {
        let runtime = state
            .gateway_runtime
            .lock()
            .map_err(|_| AppError::internal("Runtime lock failed"))?;
        if runtime.running {
            return Err(AppError::new(
                crate::errors::codes::GATEWAY_ALREADY_RUNNING,
                "Gateway is already running",
            ));
        }
    }

    // Read settings
    let (host, port) = {
        let conn = state
            .db
            .get()
            .map_err(|_| AppError::internal("DB lock failed"))?;
        let settings = storage::gateway_settings::get(&conn)?;
        (settings.host, settings.port as u16)
    };

    // Start real HTTP server. GUI 走纯 HTTP（127.0.0.1 本地通信，无 TLS 需求）。
    let (shutdown_tx, server_handle, active_requests, _bound_port) =
        gateway::server::start(&host, port, state.db.clone(), None, state.wake.clone()).await?;

    // Update runtime state
    {
        let mut runtime = state
            .gateway_runtime
            .lock()
            .map_err(|_| AppError::internal("Runtime lock failed"))?;
        runtime.running = true;
        runtime.host = host;
        runtime.port = port;
        runtime.started_at = Some(chrono::Utc::now().to_rfc3339());
        runtime.shutdown_tx = Some(shutdown_tx);
        runtime.server_handle = Some(server_handle);
        runtime.active_requests = Some(active_requests);
    }

    let _ = PetBubble {
        text: "Gateway started".into(),
        text_zh: Some("网关已启动".into()),
        r#type: "success".into(),
    }
    .emit(&app_handle);
    let _ = PetGatewayStateChanged("running".into()).emit(&app_handle);
    crate::app::tray::refresh_tray_blocking(&app_handle).await;
    get_gateway_status(state)
}

#[tauri::command]
#[specta::specta]
pub async fn stop_gateway(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<GatewayStatus, AppError> {
    let (shutdown_tx, server_handle) = {
        let mut runtime = state
            .gateway_runtime
            .lock()
            .map_err(|_| AppError::internal("Runtime lock failed"))?;
        if !runtime.running {
            return Err(AppError::new(
                crate::errors::codes::GATEWAY_NOT_RUNNING,
                "Gateway is not running",
            ));
        }
        runtime.running = false;
        runtime.started_at = None;
        (runtime.shutdown_tx.take(), runtime.server_handle.take())
    };

    // Send shutdown signal
    if let Some(tx) = shutdown_tx {
        let _ = tx.send(());
    }

    // Wait for server to finish (with timeout)
    if let Some(handle) = server_handle {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    let _ = PetBubble {
        text: "Gateway stopped".into(),
        text_zh: Some("网关已停止".into()),
        r#type: "info".into(),
    }
    .emit(&app_handle);
    let _ = PetGatewayStateChanged("stopped".into()).emit(&app_handle);
    crate::app::tray::refresh_tray_blocking(&app_handle).await;
    get_gateway_status(state)
}

#[tauri::command]
#[specta::specta]
pub async fn restart_gateway(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<GatewayStatus, AppError> {
    // Stop if running
    {
        let is_running = {
            let runtime = state
                .gateway_runtime
                .lock()
                .map_err(|_| AppError::internal("Runtime lock failed"))?;
            runtime.running
        };
        if is_running {
            let (shutdown_tx, server_handle) = {
                let mut runtime = state
                    .gateway_runtime
                    .lock()
                    .map_err(|_| AppError::internal("Runtime lock failed"))?;
                runtime.running = false;
                runtime.started_at = None;
                (runtime.shutdown_tx.take(), runtime.server_handle.take())
            };
            if let Some(tx) = shutdown_tx {
                let _ = tx.send(());
            }
            if let Some(handle) = server_handle {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
            }
        }
    }

    // Small delay for port release
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Start again
    start_gateway(app_handle, state).await
}

// ── Gateway Auth Commands ──────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub fn get_gateway_auth_settings(
) -> Result<crate::security::local_token::GatewayAuthSettings, AppError> {
    Ok(crate::security::local_token::get_auth_settings())
}

#[tauri::command]
#[specta::specta]
pub fn regenerate_local_access_token(
) -> Result<crate::security::local_token::GatewayAuthSettings, AppError> {
    crate::security::local_token::regenerate_token()?;
    Ok(crate::security::local_token::get_auth_settings())
}

#[tauri::command]
#[specta::specta]
pub fn ensure_local_access_token(
) -> Result<crate::security::local_token::GatewayAuthSettings, AppError> {
    crate::security::local_token::ensure_token()?;
    Ok(crate::security::local_token::get_auth_settings())
}

#[tauri::command]
#[specta::specta]
pub fn get_local_access_token() -> Result<String, AppError> {
    crate::security::local_token::read_token()
}

#[tauri::command]
#[specta::specta]
pub fn open_token_folder() -> Result<bool, AppError> {
    let dir = crate::security::local_token::token_dir();
    open::that(&dir).map_err(|e| AppError::internal(format!("Failed to open folder: {e}")))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;

    use super::*;
    use crate::app::state::AppState;
    use crate::models::gateway::GatewayRuntimeState;
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
            gateway_runtime: Arc::new(Mutex::new(GatewayRuntimeState::default())),
            wake: crate::wake::WakeManager::new(),
            pet_click_through: Arc::new(Mutex::new(false)),
        }
    }

    unsafe fn as_state<'r>(state: &'r AppState) -> tauri::State<'r, AppState> {
        std::mem::transmute(state)
    }

    #[test]
    fn get_gateway_settings_returns_defaults() {
        let state = test_state();
        let settings = get_gateway_settings(unsafe { as_state(&state) }).unwrap();
        assert_eq!(settings.host, "127.0.0.1");
        assert_eq!(settings.port, 9090);
    }

    #[test]
    fn update_gateway_settings_persists_changes() {
        let state = test_state();
        let updated = update_gateway_settings(
            UpdateGatewaySettingsInput {
                host: Some("0.0.0.0".to_string()),
                port: Some(8080),
                ..Default::default()
            },
            unsafe { as_state(&state) },
        )
        .unwrap();
        assert_eq!(updated.host, "0.0.0.0");
        assert_eq!(updated.port, 8080);

        let fetched = get_gateway_settings(unsafe { as_state(&state) }).unwrap();
        assert_eq!(fetched.host, "0.0.0.0");
        assert_eq!(fetched.port, 8080);
    }

    #[test]
    fn update_gateway_settings_applies_wake_changes_to_runtime() {
        let state = test_state();
        update_gateway_settings(
            UpdateGatewaySettingsInput {
                wake_enabled: Some(false),
                wake_request_control: Some(true),
                wake_cooldown_seconds: Some(30),
                wake_keep_display_awake: Some(true),
                ..Default::default()
            },
            unsafe { as_state(&state) },
        )
        .unwrap();

        assert_eq!(
            state.wake.config(),
            crate::wake::WakeConfig {
                enabled: false,
                request_control: true,
                cooldown_seconds: 30,
                keep_display_awake: true,
            }
        );
    }

    #[test]
    fn get_gateway_status_reflects_runtime_and_settings() {
        let state = test_state();
        let status = get_gateway_status(unsafe { as_state(&state) }).unwrap();
        assert!(!status.running);
        assert_eq!(status.host, "127.0.0.1");
        assert_eq!(status.port, 9090);
    }

    #[test]
    fn get_gateway_auth_settings_reports_enabled_mode() {
        let settings = get_gateway_auth_settings().unwrap();
        assert!(settings.gateway_auth_enabled);
        assert_eq!(settings.auth_mode, "local_token_file");
    }

    #[test]
    fn ensure_local_access_token_generates_token() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let settings = ensure_local_access_token().unwrap();
        assert!(settings.gateway_auth_enabled);
        assert!(!settings.masked_token.is_empty());

        let token = get_local_access_token().unwrap();
        assert!(token.starts_with("ag_local_"));
        cleanup(&temp);
    }

    #[test]
    fn regenerate_local_access_token_changes_token() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let first = ensure_local_access_token().unwrap();
        let second = regenerate_local_access_token().unwrap();
        assert_ne!(first.masked_token, second.masked_token);
        cleanup(&temp);
    }
}
