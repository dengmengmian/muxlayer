//! request_logs 目录模块：请求日志存储层，按职责拆为查询、写入、聚合统计三个子模块。
//! 对外路径保持 `crate::storage::request_logs::XXX` 不变，统一在这里重导出。

mod query;
mod stats;
mod write;

#[cfg(test)]
mod tests;

pub use query::{count, distinct_models, get_detail, list};
pub use stats::{
    aggregate_by_session, aggregate_cost_by_client, aggregate_cost_by_model,
    aggregate_provider_detail_stats, aggregate_route_profile_stats, avg_latency_by_provider,
    get_provider_health, get_stats, get_stats_for_range, get_stats_for_range_at,
    invalidate_cost_caches, today_cost, today_cost_at, today_cost_cached, DailyStat,
    ProviderHealth, ProviderStat, RecentError, RequestStats,
};
pub use write::{
    cleanup_older_than, clear, delete_by_session, external_ids_for_source, extract_cache_tokens,
    insert, insert_session_log, insert_session_logs, SessionLogBatchOutcome, SessionLogRow,
    SESSION_LOG_BATCH_SIZE,
};
