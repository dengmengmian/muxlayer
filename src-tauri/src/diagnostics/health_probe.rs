//! 后台主动健康探测：定期对启用的 provider 发 1-token 探测请求（复用 speedtest），
//! 结果写入 provider_runtime_status 的 last_probe_* 列。**仅用于展示，不影响路由**
//! ——绝不碰 available / cooldown。受 gateway_settings.health_probe_enabled 控制，
//! 默认关（开启才探测，会消耗少量额度）。

use std::time::Duration;

/// 启动延迟：让网关 / DB 先就绪。
const STARTUP_DELAY: Duration = Duration::from_secs(30);
/// 探测间隔。固定 10 分钟——1-token 探测开销极小，10 分钟粒度足够发现 provider 掉线。
const INTERVAL: Duration = Duration::from_secs(600);

/// 启动后台探测循环。db 句柄由 AppState 传入。
/// 用 tauri::async_runtime::spawn（与项目其它后台任务一致）——Tauri setup 是同步
/// 上下文，直接 tokio::spawn 会 panic "no reactor running"。
pub fn spawn(db: crate::storage::db::DbPool) {
    crate::runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            run_once(&db).await;
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

async fn run_once(db: &crate::storage::db::DbPool) {
    // 短暂持锁读开关 + provider 列表；持锁期间绝不 await。
    let providers = {
        let conn = match db.get() {
            Ok(c) => c,
            Err(_) => return,
        };
        match crate::storage::gateway_settings::get(&conn) {
            Ok(s) if s.health_probe_enabled => {}
            Ok(_) => return, // 未开启：正常跳过
            Err(e) => {
                eprintln!("[health_probe] 读取设置失败，跳过本轮: {e}");
                return;
            }
        }
        match crate::storage::providers::list_all(&conn) {
            Ok(ps) => ps.into_iter().filter(|p| p.enabled).collect::<Vec<_>>(),
            Err(e) => {
                eprintln!("[health_probe] 读取 provider 列表失败，跳过本轮: {e}");
                return;
            }
        }
    };
    if providers.is_empty() {
        return;
    }

    // 探测阶段不持锁（要发 HTTP）。复用 speedtest 的 1-token 探测。
    let reports = match crate::diagnostics::speedtest::probe_many(&providers).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[health_probe] 探测失败，跳过本轮: {e}");
            return;
        }
    };

    // 短暂持锁写结果。
    let conn = match db.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    for r in reports {
        if let Err(e) = crate::storage::provider_runtime_status::record_probe(
            &conn,
            &r.provider_id,
            r.success,
            r.total_ms as i64,
            r.error.as_deref(),
        ) {
            eprintln!("[health_probe] 写入探测结果失败 ({}): {e}", r.provider_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::gateway::UpdateGatewaySettingsInput;
    use crate::models::provider::UpdateProviderInput;
    use crate::storage::db::DbPool;
    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;
    use std::time::Duration;

    fn setup_db_pool() -> (DbPool, std::path::PathBuf) {
        let temp = std::env::temp_dir().join(format!(
            "agentgate_health_probe_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let db_path = temp.join("test.db");
        let manager = SqliteConnectionManager::file(&db_path);
        let pool = Pool::builder().max_size(2).build(manager).unwrap();
        let conn = pool.get().unwrap();
        crate::storage::migrations::run_migrations(&conn).unwrap();
        (pool, temp)
    }

    fn set_health_probe_enabled(pool: &DbPool, enabled: bool) {
        let conn = pool.get().unwrap();
        crate::storage::gateway_settings::update(
            &conn,
            UpdateGatewaySettingsInput {
                health_probe_enabled: Some(enabled),
                ..Default::default()
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn run_once_skips_when_disabled() {
        let (pool, db_temp) = setup_db_pool();
        // Default gateway settings have health_probe_enabled = false.
        let conn = pool.get().unwrap();
        let provider_ids: Vec<String> = crate::storage::providers::list_all(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        drop(conn);

        run_once(&pool).await;

        let conn = pool.get().unwrap();
        for id in provider_ids {
            let s = crate::storage::provider_runtime_status::get(&conn, &id).unwrap();
            assert!(
                s.last_probe_at.is_none(),
                "probe should not run when disabled"
            );
        }
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[tokio::test]
    async fn run_once_skips_when_no_enabled_providers() {
        let (pool, db_temp) = setup_db_pool();
        set_health_probe_enabled(&pool, true);

        let conn = pool.get().unwrap();
        for p in crate::storage::providers::list_all(&conn).unwrap() {
            crate::storage::providers::update(
                &conn,
                &p.id,
                UpdateProviderInput {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        drop(conn);

        run_once(&pool).await;

        let conn = pool.get().unwrap();
        let statuses = crate::storage::provider_runtime_status::list_all(&conn).unwrap();
        for s in statuses {
            assert!(
                s.last_probe_at.is_none(),
                "probe should not run with no enabled providers"
            );
        }
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[tokio::test]
    async fn run_once_records_probe_result_for_enabled_provider() {
        let (pool, db_temp) = setup_db_pool();
        set_health_probe_enabled(&pool, true);

        let conn = pool.get().unwrap();
        // Leave the default enabled provider in place but ensure it has no API key
        // so the probe fails immediately without making a network request.
        let enabled = crate::storage::providers::list_all(&conn)
            .unwrap()
            .into_iter()
            .find(|p| p.enabled)
            .expect("expected an enabled provider");
        crate::storage::providers::update(
            &conn,
            &enabled.id,
            UpdateProviderInput {
                api_key: Some("".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        run_once(&pool).await;

        let conn = pool.get().unwrap();
        let s = crate::storage::provider_runtime_status::get(&conn, &enabled.id).unwrap();
        assert!(
            s.last_probe_at.is_some(),
            "probe timestamp should be recorded"
        );
        assert_eq!(s.last_probe_ok, Some(false), "probe should record failure");
        assert!(
            s.last_probe_error.is_some(),
            "probe error should be recorded"
        );
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[tokio::test]
    async fn run_once_returns_early_on_db_lock_failure() {
        let temp = std::env::temp_dir().join(format!(
            "agentgate_health_probe_broken_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let db_path = temp.join("test.db");
        let manager = SqliteConnectionManager::file(&db_path);
        let pool = Pool::builder()
            .max_size(1)
            .connection_timeout(Duration::from_millis(1))
            .build(manager)
            .unwrap();
        // Hold the only connection so run_once cannot acquire one.
        let _conn = pool.get().unwrap();

        // Should not panic and should return immediately.
        run_once(&pool).await;
        let _ = std::fs::remove_dir_all(&temp);
    }
}
