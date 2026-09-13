//! 后台任务 spawn 的运行时抽象。
//!
//! desktop:用 Tauri 托管的 runtime —— Tauri 的 `setup` 是同步上下文,在那里直接
//! `tokio::spawn` 会 panic「no reactor running」,必须走 `tauri::async_runtime`。
//! headless:`agentgate-serve` 本身在 `#[tokio::main]` 下,直接 `tokio::spawn`。

use std::future::Future;

use crate::errors::AppError;

#[cfg(feature = "desktop")]
pub fn spawn<F>(future: F)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tauri::async_runtime::spawn(future);
}

#[cfg(not(feature = "desktop"))]
pub fn spawn<F>(future: F)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(future);
}

/// 在 blocking 线程池里借一个 DB 连接跑同步 rusqlite 操作并等待结果。
///
/// 网关热路径(选路 / 熔断标记 / 预算闸)都在 tokio worker 上,直接 `db.get()` +
/// 同步查询会卡住 worker;池只有 8 个连接,高并发时还会排队阻塞整个 runtime。
pub async fn db_blocking<T, F>(db: &crate::storage::db::DbPool, f: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> Result<T, AppError> + Send + 'static,
{
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db
            .get()
            .map_err(|e| AppError::internal(format!("DB pool unavailable: {e}")))?;
        f(&conn)
    })
    .await
    .map_err(|e| AppError::internal(format!("DB blocking task failed: {e}")))?
}

/// fire-and-forget 版本:用于请求日志这类不该让客户端等的写入。
/// 不等结果,但失败必须打 warn,不能静默吞掉。
pub fn db_blocking_detached<F>(db: &crate::storage::db::DbPool, what: &'static str, f: F)
where
    F: FnOnce(&rusqlite::Connection) -> Result<(), AppError> + Send + 'static,
{
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        let result = db
            .get()
            .map_err(|e| AppError::internal(format!("DB pool unavailable: {e}")))
            .and_then(|conn| f(&conn));
        if let Err(e) = result {
            tracing::warn!(task = what, error = %e, "background DB write failed");
        }
    });
}
