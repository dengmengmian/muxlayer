//! Periodic background sync of all client session logs.
//!
//! Lifecycle:
//!   1. **Startup boost**: 5 seconds after the gateway process starts, run one
//!      full sync. This catches the case where the user installs AgentGate
//!      for the first time after months of Claude / Codex / Gemini use — they
//!      see historical usage immediately on first launch.
//!   2. **Hourly tick**: every hour after that, run again. New conversations
//!      get reflected within an hour without the user clicking anything.
//!
//! Errors do not surface anywhere: this is a "best effort" task. The manual
//! sync buttons in the Logs page are the user's hand-on-the-wheel override.
//!
//! Spawned from `lib.rs::setup` alongside the existing `log-cleanup` loop —
//! see that for the architectural pattern.

use std::time::Duration;

use super::SyncResult;

const STARTUP_DELAY: Duration = Duration::from_secs(5);
const INTERVAL: Duration = Duration::from_secs(3600);

/// Run all three sync paths once and aggregate the result.
fn sync_all_now(db: &crate::storage::db::DbPool) -> SyncResult {
    let mut total = SyncResult::default();
    if let Ok(r) = super::claude::sync(db) {
        total.merge(r);
    }
    if let Ok(r) = super::codex::sync(db) {
        total.merge(r);
    }
    if let Ok(r) = super::gemini::sync(db) {
        total.merge(r);
    }
    total
}

/// Spawn the periodic loop. Caller passes the DB handle from AppState.
///
/// Loop body:
///   - sleep `STARTUP_DELAY` (5s), then run once
///   - then loop: sleep `INTERVAL` (1h), run once
///
/// The task runs for the lifetime of the process; there's no shutdown
/// signal because the work is purely advisory (and DB writes are short
/// and atomic — a SIGINT mid-sync won't corrupt state).
pub fn spawn(db: crate::storage::db::DbPool) {
    crate::runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            // 同步要扫大量文件并批量写库,放到阻塞线程池,不占 tokio worker。
            let task_db = db.clone();
            let result = match tokio::task::spawn_blocking(move || sync_all_now(&task_db)).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[session-sync] sync task failed: {e}");
                    tokio::time::sleep(INTERVAL).await;
                    continue;
                }
            };
            if result.imported > 0 {
                eprintln!(
                    "[session-sync] imported {} (scanned {} files, skipped {}, errors {})",
                    result.imported,
                    result.files_scanned,
                    result.skipped,
                    result.errors.len(),
                );
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}
