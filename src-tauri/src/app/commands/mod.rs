use crate::errors::AppError;
use crate::storage;

pub mod clients;
pub mod config;
pub mod diagnostics;
pub mod gateway;
pub mod instructions_skills;
pub mod logs;
pub mod pet;
pub mod pricing;
pub mod providers;
pub mod route_profiles;

pub use clients::*;
pub use config::*;
pub use diagnostics::*;
pub use gateway::*;
pub use instructions_skills::*;
pub use logs::*;
pub use pet::*;
pub use pricing::*;
pub use providers::*;
pub use route_profiles::*;

// ── Client apply history helper ────────────────────────────────

/// 在 blocking 线程池执行同步 I/O(读写客户端配置、spawn 子进程、sleep、阻塞 HTTP)。
/// Tauri v2 的同步 `#[tauri::command]` 跑在主线程,慢 I/O 会冻结整个 UI。
pub(super) async fn run_blocking<T, F>(f: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal(format!("blocking task failed: {e}")))?
}

/// Snapshot the client's on-disk config files **before** the apply/disable/
/// toggle path rewrites them, and append one row to `client_apply_history`.
///
/// 记录失败不阻断 apply(丢一个回滚点不该让接入失败),但必须留下告警:
/// tracing 给 headless / 有订阅者的场景,stderr 给桌面端(未初始化 tracing)。
pub(super) fn record_pre_apply(
    db: &storage::db::DbPool,
    client_id: &str,
    action: &str,
    paths: Vec<(&'static str, std::path::PathBuf)>,
    summary: &str,
) {
    // Read off disk before acquiring the DB lock — file I/O may be slow.
    let snap = storage::apply_history::snapshot_files_at(&paths);
    let result = db
        .get()
        .map_err(|e| AppError::internal(format!("DB pool unavailable: {e}")))
        .and_then(|conn| storage::apply_history::record(&conn, client_id, action, &snap, summary));
    if let Err(e) = result {
        tracing::warn!(client_id, action, error = %e, "failed to record client apply history; rollback point lost");
        eprintln!("[apply-history] failed to record {client_id}/{action}: {e}");
    }
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

    #[test]
    fn record_pre_apply_creates_history_entry() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let state = test_state();
        let cfg_path = temp.join("test-config.toml");
        std::fs::write(&cfg_path, "model_provider = \"openai\"\n").unwrap();

        record_pre_apply(
            &state.db,
            "codex",
            "apply",
            vec![("config.toml", cfg_path)],
            "apply test config",
        );

        let conn = state.db.get().unwrap();
        let history = storage::apply_history::list(&conn, "codex").unwrap();
        assert!(!history.is_empty());
        assert_eq!(history[0].action, "apply");
        assert!(history[0].is_initial);
        cleanup(&temp);
    }
}
