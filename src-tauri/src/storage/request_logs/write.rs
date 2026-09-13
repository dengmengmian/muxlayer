//! 写入与清理：插入（网关请求 / 会话日志）、同步去重辅助、删除与保留期清理。

use rusqlite::Connection;
use std::borrow::Cow;

use crate::errors::AppError;

const MAX_LOG_FIELD_BYTES: usize = 1024 * 1024;
const TRUNCATED_MARKER: &str = "\n...[truncated by MuxLayer]";

fn truncate_log_field(value: Option<&str>) -> Option<Cow<'_, str>> {
    let value = value?;
    if value.len() <= MAX_LOG_FIELD_BYTES {
        return Some(Cow::Borrowed(value));
    }

    let keep = MAX_LOG_FIELD_BYTES.saturating_sub(TRUNCATED_MARKER.len());
    let mut end = keep;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = String::with_capacity(MAX_LOG_FIELD_BYTES);
    truncated.push_str(&value[..end]);
    truncated.push_str(TRUNCATED_MARKER);
    Some(Cow::Owned(truncated))
}

fn shrink_database_after_delete(conn: &Connection) -> bool {
    match conn.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE);
         VACUUM;",
    ) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("[log-cleanup] SQLite shrink skipped: {err}");
            false
        }
    }
}

/// 保留期清理后 VACUUM 的最小间隔:VACUUM 会重写整库并持有排他锁,
/// 滚动保留期下每小时都有行过期,不能每次清理都跑。
const CLEANUP_VACUUM_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
/// 空闲页占比超过该阈值才值得 VACUUM。
const CLEANUP_VACUUM_FREE_RATIO: f64 = 0.2;

fn should_vacuum_after_cleanup(
    freelist_count: i64,
    page_count: i64,
    last_vacuum: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    if page_count <= 0 {
        return false;
    }
    if let Some(last) = last_vacuum {
        if now.saturating_duration_since(last) < CLEANUP_VACUUM_MIN_INTERVAL {
            return false;
        }
    }
    freelist_count as f64 / page_count as f64 > CLEANUP_VACUUM_FREE_RATIO
}

/// 删除某个会话在 request_logs 里的全部行，返回删除行数。
pub fn delete_by_session(conn: &Connection, session_id: &str) -> Result<usize, AppError> {
    let n = conn.execute(
        "DELETE FROM request_logs WHERE session_id = ?1",
        [session_id],
    )?;
    if n > 0 {
        super::invalidate_cost_caches();
    }
    Ok(n)
}

#[allow(clippy::too_many_arguments)]
pub fn insert(
    conn: &Connection,
    request_id: &str,
    client: &str,
    provider: &str,
    model: &str,
    route: &str,
    status_code: i64,
    latency_ms: i64,
    raw_request: Option<&str>,
    converted_request: Option<&str>,
    raw_response: Option<&str>,
    converted_response: Option<&str>,
    sse_events: Option<&str>,
    tool_calls: Option<&str>,
    error_message: Option<&str>,
    trace_json: Option<&str>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cost: Option<f64>,
    cache_write_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    source: Option<&str>,
    session_id: Option<&str>,
    external_id: Option<&str>,
) -> Result<(), AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    // 缺省视为 'gateway' —— 旧调用方迁移期间还没传，保持以前的语义。
    let source = source.unwrap_or("gateway");
    let tool_calls = truncate_log_field(tool_calls);
    let error_message = truncate_log_field(error_message);
    let raw_request = truncate_log_field(raw_request);
    let converted_request = truncate_log_field(converted_request);
    let raw_response = truncate_log_field(raw_response);
    let converted_response = truncate_log_field(converted_response);
    let sse_events = truncate_log_field(sse_events);
    let trace_json = truncate_log_field(trace_json);
    let route_profile_id = extract_route_profile_id(trace_json.as_deref());

    conn.execute(
        "INSERT INTO request_logs (id, request_id, timestamp, client, provider, model, route,
                status_code, latency_ms, raw_request, converted_request, raw_response,
                converted_response, sse_events, tool_calls, error_message, trace_json,
                input_tokens, output_tokens, cost, cache_write_tokens, cache_read_tokens,
                source, session_id, external_id, route_profile_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
        rusqlite::params![
            &id, request_id, &now, client, provider, model, route,
            status_code, latency_ms, raw_request.as_deref(), converted_request.as_deref(),
            raw_response.as_deref(), converted_response.as_deref(), sse_events.as_deref(),
            tool_calls.as_deref(), error_message.as_deref(), trace_json.as_deref(),
            input_tokens, output_tokens, cost, cache_write_tokens, cache_read_tokens,
            source, session_id, external_id, route_profile_id.as_deref(),
        ],
    )?;
    super::stats::record_inserted_cost(&now, cost);
    Ok(())
}

fn extract_route_profile_id(trace_json: Option<&str>) -> Option<String> {
    let raw = trace_json?;
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.pointer("/route_decision/profile_id")
        .and_then(|p| p.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 给客户端会话日志同步器用：插入一条来自客户端本地日志的请求记录。
/// 与 `insert` 的差别：
///   - timestamp 来自调用方（文件里的事件时间），不是 now()
///   - 没有 raw_request / converted_request / response / SSE / tool_calls / error_message
///     —— 客户端日志只能给到 usage，不可能反推完整请求
///   - source / session_id / external_id 必填
///
/// 调用方应当先用 `external_ids_for_source` 过滤去重，再批量调这个函数。
#[allow(clippy::too_many_arguments)]
pub fn insert_session_log(
    conn: &Connection,
    timestamp: &str,
    client: &str,
    provider: &str,
    model: &str,
    route: &str,
    source: &str,
    session_id: &str,
    external_id: &str,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cost: Option<f64>,
) -> Result<(), AppError> {
    let timestamp = insert_session_log_row(
        conn,
        &SessionLogRow {
            timestamp: timestamp.to_string(),
            client: client.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            route: route.to_string(),
            source: source.to_string(),
            session_id: session_id.to_string(),
            external_id: external_id.to_string(),
            input_tokens,
            output_tokens,
            cache_write_tokens,
            cache_read_tokens,
            cost,
        },
    )?;
    super::stats::record_inserted_cost(&timestamp, cost);
    Ok(())
}

/// 单行写入,不碰成本缓存(批量路径按批失效)。返回落库的 UTC timestamp。
fn insert_session_log_row(conn: &Connection, row: &SessionLogRow) -> Result<String, AppError> {
    let timestamp = normalize_utc_timestamp(&row.timestamp);
    let id = uuid::Uuid::new_v4().to_string();
    // request_id 我们没有真实值——客户端日志的 message_id 作为 request_id，方便用户在
    // Logs 详情里通过 request_id 列追溯到原文件里的那条消息。external_id 同时填同样的值，
    // 用作幂等 key 防止重复同步。
    conn.execute(
        "INSERT INTO request_logs (id, request_id, timestamp, client, provider, model, route,
                status_code, latency_ms,
                input_tokens, output_tokens, cost, cache_write_tokens, cache_read_tokens,
                source, session_id, external_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 200, 0, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        rusqlite::params![
            &id,
            &row.external_id,
            &timestamp,
            &row.client,
            &row.provider,
            &row.model,
            &row.route,
            row.input_tokens,
            row.output_tokens,
            row.cost,
            row.cache_write_tokens,
            row.cache_read_tokens,
            &row.source,
            &row.session_id,
            &row.external_id,
        ],
    )?;
    Ok(timestamp)
}

/// 会话日志文件里的时间可能带本地偏移(如 `+08:00`)。统一转成 UTC RFC3339 再落库,
/// 保证 timestamp 列的字符串序 = 时间序,按日 / 区间查询才正确。
/// 无法解析时与缺失时间一样回落到 now() 并告警:拒绝写入会让这行用量永久缺失,
/// 且每次同步都重复报错。
fn normalize_utc_timestamp(raw: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(t) => t.with_timezone(&chrono::Utc).to_rfc3339(),
        Err(e) => {
            tracing::warn!("invalid session log timestamp '{raw}', falling back to now: {e}");
            chrono::Utc::now().to_rfc3339()
        }
    }
}

/// 给客户端日志同步器用：从 DB 里查询某个 source 下已存在的 external_id 集合。
/// 同步前先调一次，把扫到的条目和这个集合做差集，避免重复插入。
pub fn external_ids_for_source(
    conn: &Connection,
    source: &str,
    candidates: &[String],
) -> Result<std::collections::HashSet<String>, AppError> {
    if candidates.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    // SQLite 单语句最多约 32k 个 placeholder；这里取 800 为一批，留足余量。
    let mut found = std::collections::HashSet::new();
    for chunk in candidates.chunks(800) {
        let placeholders = (1..=chunk.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT external_id FROM request_logs
             WHERE source = ?1 AND external_id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::types::ToSql> =
            vec![&source as &dyn rusqlite::types::ToSql];
        for c in chunk {
            params.push(c as &dyn rusqlite::types::ToSql);
        }
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
            r.get::<_, String>(0)
        })?;
        for id in rows.flatten() {
            found.insert(id);
        }
    }
    Ok(found)
}

/// Pull `(cache_write_tokens, cache_read_tokens)` out of any supported upstream
/// usage shape. Returns `(None, None)` when neither is present so the row
/// keeps "unknown" semantics rather than misleading zeroes.
///
/// Recognised shapes:
///   - Anthropic Messages: `cache_creation_input_tokens` / `cache_read_input_tokens`
///   - OpenAI Responses: `input_tokens_details.cached_tokens` (Read only)
///   - OpenAI Chat Completions: `prompt_tokens_details.cached_tokens` (Read only)
///   - Bare field used by some Chinese providers: `cached_tokens` (Read only)
pub fn extract_cache_tokens(usage: &serde_json::Value) -> (Option<i64>, Option<i64>) {
    let write = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_i64());
    let read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(|v| v.as_i64())
        })
        .or_else(|| {
            usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(|v| v.as_i64())
        })
        .or_else(|| usage.get("cached_tokens").and_then(|v| v.as_i64()));
    (write, read)
}

/// 用户主动清空:可以 VACUUM 回收空间,但调用方必须在阻塞线程池里调用(不能在 UI 主线程)。
pub fn clear(conn: &Connection) -> Result<bool, AppError> {
    let deleted = conn.execute("DELETE FROM request_logs", [])?;
    if deleted > 0 {
        super::invalidate_cost_caches();
        shrink_database_after_delete(conn);
    }
    Ok(true)
}

/// Delete logs older than `retention_days`. Returns the number of deleted rows.
pub fn cleanup_older_than(conn: &Connection, retention_days: i64) -> Result<usize, AppError> {
    cleanup_with_vacuum_state(conn, retention_days, &LAST_CLEANUP_VACUUM).map(|(n, _)| n)
}

static LAST_CLEANUP_VACUUM: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

/// 返回 (删除行数, 是否执行了 VACUUM)。`last_vacuum` 记录上次清理触发 VACUUM 的时刻,
/// 由调用方持有以便测试隔离。
fn cleanup_with_vacuum_state(
    conn: &Connection,
    retention_days: i64,
    last_vacuum: &std::sync::Mutex<Option<std::time::Instant>>,
) -> Result<(usize, bool), AppError> {
    // 0 / 负数会把全部日志当作过期删掉;设置层已拒绝,这里再挡一次库里的历史脏值。
    if retention_days < 1 {
        return Err(AppError::validation(format!(
            "log retention days must be >= 1, got {retention_days}"
        )));
    }
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days)).to_rfc3339();
    let deleted = conn.execute("DELETE FROM request_logs WHERE timestamp < ?1", [&cutoff])?;
    if deleted == 0 {
        return Ok((0, false));
    }
    super::invalidate_cost_caches();

    let freelist: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let mut guard = last_vacuum
        .lock()
        .map_err(|_| AppError::internal("cleanup vacuum state poisoned"))?;
    let now = std::time::Instant::now();
    if !should_vacuum_after_cleanup(freelist, pages, *guard, now) {
        return Ok((deleted, false));
    }
    let vacuumed = shrink_database_after_delete(conn);
    if vacuumed {
        *guard = Some(now);
    }
    Ok((deleted, vacuumed))
}

/// 会话日志同步的一行待写入数据。
#[derive(Debug, Clone)]
pub struct SessionLogRow {
    pub timestamp: String,
    pub client: String,
    pub provider: String,
    pub model: String,
    pub route: String,
    pub source: String,
    pub session_id: String,
    pub external_id: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cost: Option<f64>,
}

/// 批量写入结果。`errors` 为 (external_id, 错误),单行失败不影响同批其它行。
#[derive(Debug, Default)]
pub struct SessionLogBatchOutcome {
    pub imported: u32,
    pub errors: Vec<(String, AppError)>,
    /// 实际提交的事务数(每 SESSION_LOG_BATCH_SIZE 行一个)。
    pub transactions: usize,
}

pub const SESSION_LOG_BATCH_SIZE: usize = 500;

pub fn insert_session_logs(
    conn: &Connection,
    rows: &[SessionLogRow],
) -> Result<SessionLogBatchOutcome, AppError> {
    let mut out = SessionLogBatchOutcome::default();
    for chunk in rows.chunks(SESSION_LOG_BATCH_SIZE) {
        let tx = conn.unchecked_transaction()?;
        for r in chunk {
            match insert_session_log_row(&tx, r) {
                Ok(_) => out.imported += 1,
                Err(e) => out.errors.push((r.external_id.clone(), e)),
            }
        }
        tx.commit()?;
        out.transactions += 1;
        // 每批只失效一次成本缓存,而不是每行一次。
        super::invalidate_cost_caches();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::request_log::RequestLogFilter;
    use crate::storage::request_logs::query;
    use rusqlite::Connection;

    fn empty_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE request_logs (
                id TEXT PRIMARY KEY,
                request_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                client TEXT,
                provider TEXT,
                model TEXT,
                route TEXT,
                status_code INTEGER,
                latency_ms INTEGER,
                input_tokens INTEGER,
                output_tokens INTEGER,
                raw_request TEXT,
                converted_request TEXT,
                raw_response TEXT,
                converted_response TEXT,
                sse_events TEXT,
                tool_calls TEXT,
                error_message TEXT,
                cost REAL,
                trace_json TEXT,
                cache_write_tokens INTEGER,
                cache_read_tokens INTEGER,
                source TEXT,
                session_id TEXT,
                external_id TEXT,
                route_profile_id TEXT
            );",
        )
        .unwrap();
        conn
    }

    fn empty_filter() -> RequestLogFilter {
        RequestLogFilter {
            client: None,
            provider: None,
            model: None,
            route_profile_id: None,
            status: None,
            error_type: None,
            keyword: None,
            source: None,
            session_id: None,
            limit: Some(100),
            offset: Some(0),
        }
    }

    #[test]
    fn insert_creates_gateway_row_with_defaults() {
        let conn = empty_db();
        insert(
            &conn,
            "req-1",
            "Codex",
            "openai_official",
            "gpt-5",
            "/v1/responses",
            200,
            120,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(10),
            Some(20),
            Some(0.001),
            Some(1),
            Some(2),
            None,
            Some("sess-1"),
            None,
        )
        .unwrap();

        let rows = query::list(&conn, empty_filter()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].request_id, "req-1");
        assert_eq!(rows[0].source, Some("gateway".to_string())); // default source
        assert_eq!(rows[0].session_id, Some("sess-1".to_string()));
    }

    #[test]
    fn insert_copies_route_profile_id_from_trace() {
        let conn = empty_db();
        insert(
            &conn,
            "req-rp",
            "Codex",
            "DeepSeek",
            "deepseek-chat",
            "/v1/responses",
            200,
            10,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(r#"{"route_decision":{"profile_id":"rp-1"}}"#),
            None,
            None,
            None,
            None,
            None,
            Some("gateway"),
            None,
            None,
        )
        .unwrap();
        let id: String = conn
            .query_row(
                "SELECT route_profile_id FROM request_logs WHERE request_id='req-rp'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(id, "rp-1");
    }

    #[test]
    fn insert_truncates_large_log_bodies() {
        let conn = empty_db();
        let big = "x".repeat(2 * 1024 * 1024);
        insert(
            &conn,
            "req-big",
            "Codex",
            "openai_official",
            "gpt-5",
            "/v1/responses",
            200,
            120,
            Some(&big),
            Some(&big),
            Some(&big),
            Some(&big),
            Some(&big),
            None,
            Some(&big),
            None,
            Some(10),
            Some(20),
            Some(0.001),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let raw_len: i64 = conn
            .query_row(
                "SELECT length(raw_request) FROM request_logs WHERE request_id='req-big'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(raw_len <= 1024 * 1024);
    }

    #[test]
    fn insert_session_log_creates_client_session_row() {
        let conn = empty_db();
        insert_session_log(
            &conn,
            "2026-06-01T12:00:00Z",
            "Codex",
            "openai_official",
            "gpt-5",
            "/v1/responses",
            "codex_session",
            "sess-a",
            "ext-1",
            Some(5),
            Some(15),
            Some(0),
            Some(1),
            Some(0.0005),
        )
        .unwrap();

        let rows = query::list(&conn, empty_filter()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].client, Some("Codex".to_string()));
        assert_eq!(rows[0].source, Some("codex_session".to_string()));
        assert_eq!(rows[0].status_code, Some(200));
        assert_eq!(rows[0].latency_ms, Some(0));
    }

    #[test]
    fn delete_by_session_removes_only_target_session() {
        let conn = empty_db();
        insert(
            &conn,
            "r1",
            "c",
            "p",
            "m",
            "/r",
            200,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("s1"),
            None,
        )
        .unwrap();
        insert(
            &conn,
            "r2",
            "c",
            "p",
            "m",
            "/r",
            200,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("s2"),
            None,
        )
        .unwrap();

        let deleted = delete_by_session(&conn, "s1").unwrap();
        assert_eq!(deleted, 1);

        let rows = query::list(&conn, empty_filter()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, Some("s2".to_string()));
    }

    #[test]
    fn external_ids_for_source_returns_existing_ids() {
        let conn = empty_db();
        insert_session_log(
            &conn,
            "2026-06-01T12:00:00Z",
            "c",
            "p",
            "m",
            "/r",
            "codex_session",
            "s",
            "ext-a",
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        insert_session_log(
            &conn,
            "2026-06-01T12:00:01Z",
            "c",
            "p",
            "m",
            "/r",
            "codex_session",
            "s",
            "ext-b",
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let found = external_ids_for_source(
            &conn,
            "codex_session",
            &["ext-a".to_string(), "ext-c".to_string()],
        )
        .unwrap();
        assert!(found.contains("ext-a"));
        assert!(!found.contains("ext-b"));
        assert!(!found.contains("ext-c"));
    }

    #[test]
    fn external_ids_for_source_empty_candidates_returns_empty() {
        let conn = empty_db();
        let found = external_ids_for_source(&conn, "codex_session", &[]).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn clear_removes_all_rows() {
        let conn = empty_db();
        insert(
            &conn, "r1", "c", "p", "m", "/r", 200, 0, None, None, None, None, None, None, None,
            None, None, None, None, None, None, None, None, None,
        )
        .unwrap();
        assert!(clear(&conn).unwrap());
        let rows = query::list(&conn, empty_filter()).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn cleanup_older_than_deletes_only_old_rows() {
        let conn = empty_db();
        let old = (chrono::Utc::now() - chrono::Duration::days(10)).to_rfc3339();
        let recent = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();

        insert_session_log(
            &conn,
            &old,
            "c",
            "p",
            "m",
            "/r",
            "codex_session",
            "s1",
            "old-1",
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        insert_session_log(
            &conn,
            &recent,
            "c",
            "p",
            "m",
            "/r",
            "codex_session",
            "s2",
            "new-1",
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let deleted = cleanup_older_than(&conn, 7).unwrap();
        assert_eq!(deleted, 1);

        let rows = query::list(&conn, empty_filter()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, Some("s2".to_string()));
    }

    fn file_db(dir: &std::path::Path) -> Connection {
        let conn = Connection::open(dir.join("logs.db")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let mem = empty_db();
        let ddl: String = mem
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='request_logs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute_batch(&ddl).unwrap();
        conn
    }

    fn seed_rows(conn: &Connection, prefix: &str, old: usize, recent: usize) {
        let old_ts = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let new_ts = chrono::Utc::now().to_rfc3339();
        let body = "x".repeat(2000);
        let tx = conn.unchecked_transaction().unwrap();
        for i in 0..(old + recent) {
            let ts = if i < old { &old_ts } else { &new_ts };
            tx.execute(
                "INSERT INTO request_logs (id, request_id, timestamp, raw_request) VALUES (?1, ?1, ?2, ?3)",
                rusqlite::params![format!("{prefix}-{i}"), ts, &body],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }

    fn page_count(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn cleanup_with_few_deletions_does_not_vacuum() {
        // 滚动保留期下每小时都有少量过期行;每次都 VACUUM 会整库重写并长时间持锁。
        let tmp = tempfile::tempdir().unwrap();
        let conn = file_db(tmp.path());
        seed_rows(&conn, "a", 10, 2000);
        let before = page_count(&conn);
        let state = std::sync::Mutex::new(None);
        let (deleted, vacuumed) = cleanup_with_vacuum_state(&conn, 7, &state).unwrap();
        assert_eq!(deleted, 10);
        assert!(!vacuumed, "空闲页占比很小时不应 VACUUM");
        assert_eq!(page_count(&conn), before);
    }

    #[test]
    fn cleanup_vacuums_when_free_pages_exceed_threshold_at_most_daily() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = file_db(tmp.path());
        seed_rows(&conn, "a", 1500, 500);
        let before = page_count(&conn);
        let state = std::sync::Mutex::new(None);
        let (deleted, vacuumed) = cleanup_with_vacuum_state(&conn, 7, &state).unwrap();
        assert_eq!(deleted, 1500);
        assert!(vacuumed, "空闲页超过阈值应 VACUUM");
        assert!(page_count(&conn) < before);

        // 同一天内再次大量删除:不再 VACUUM。
        seed_rows(&conn, "b", 1500, 0);
        let (deleted, vacuumed) = cleanup_with_vacuum_state(&conn, 7, &state).unwrap();
        assert_eq!(deleted, 1500);
        assert!(!vacuumed, "24h 内最多 VACUUM 一次");
    }

    #[test]
    fn cleanup_rejects_non_positive_retention() {
        let conn = empty_db();
        insert(
            &conn, "r1", "c", "p", "m", "/r", 200, 0, None, None, None, None, None, None, None,
            None, None, None, None, None, None, None, None, None,
        )
        .unwrap();
        for days in [0, -1] {
            let err = cleanup_older_than(&conn, days).unwrap_err();
            assert_eq!(err.code, "VALIDATION_ERROR");
        }
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM request_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "非法保留期不能删除任何行");
    }

    #[test]
    fn insert_truncates_tool_calls_and_error_message() {
        let conn = empty_db();
        let big = "y".repeat(2 * 1024 * 1024);
        insert(
            &conn,
            "req-big2",
            "c",
            "p",
            "m",
            "/r",
            500,
            0,
            None,
            None,
            None,
            None,
            None,
            Some(&big),
            Some(&big),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let (tc, em): (i64, i64) = conn
            .query_row(
                "SELECT length(tool_calls), length(error_message) FROM request_logs WHERE request_id='req-big2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(tc <= MAX_LOG_FIELD_BYTES as i64, "tool_calls {tc}");
        assert!(em <= MAX_LOG_FIELD_BYTES as i64, "error_message {em}");
    }

    #[test]
    fn insert_session_log_normalizes_timestamp_to_utc() {
        let conn = empty_db();
        insert_session_log(
            &conn,
            "2026-06-01T20:00:00+08:00",
            "c",
            "p",
            "m",
            "/r",
            "claude_session",
            "s",
            "ext-tz",
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let ts: String = conn
            .query_row("SELECT timestamp FROM request_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ts, "2026-06-01T12:00:00+00:00");
    }

    #[test]
    fn insert_session_log_falls_back_to_now_for_unparseable_timestamp() {
        // 解析不了的时间不能让这行永久丢失、每次同步都重复报错:与缺失时间一样取 now()。
        let conn = empty_db();
        let before = chrono::Utc::now();
        insert_session_log(
            &conn,
            "not-a-time",
            "c",
            "p",
            "m",
            "/r",
            "gemini_session",
            "s",
            "ext-bad",
            Some(10),
            Some(5),
            None,
            None,
            None,
        )
        .expect("无法解析的时间应回落到 now() 并落库");
        let after = chrono::Utc::now();
        let (ts, input): (String, i64) = conn
            .query_row(
                "SELECT timestamp, input_tokens FROM request_logs WHERE external_id = 'ext-bad'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(input, 10);
        let stored = chrono::DateTime::parse_from_rfc3339(&ts).expect("落库时间必须是 RFC3339");
        assert_eq!(stored.offset().local_minus_utc(), 0, "落库时间必须是 UTC");
        assert!(stored >= before && stored <= after, "{ts}");
    }

    fn session_row(i: usize) -> SessionLogRow {
        SessionLogRow {
            timestamp: format!("2026-06-01T12:{:02}:{:02}Z", (i / 60) % 60, i % 60),
            client: "Claude Code".into(),
            provider: "anthropic_official".into(),
            model: "claude-x".into(),
            route: "/v1/messages".into(),
            source: "claude_session".into(),
            session_id: format!("s{}", i % 3),
            external_id: format!("ext-{i}"),
            input_tokens: Some(i as i64),
            output_tokens: Some(1),
            cache_write_tokens: None,
            cache_read_tokens: Some(2),
            cost: Some(0.5),
        }
    }

    type DumpRow = (
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<f64>,
    );

    fn dump_session_rows(c: &Connection) -> Vec<DumpRow> {
        c.prepare(
            "SELECT external_id, timestamp, session_id, input_tokens, cache_read_tokens, cost
             FROM request_logs ORDER BY external_id",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    #[test]
    fn insert_session_logs_batches_rows_into_chunked_transactions() {
        // 首次同步几万行时逐行 autocommit = 几万次 fsync;批量按 500 行一个事务提交。
        let conn = empty_db();
        let rows: Vec<SessionLogRow> = (0..1201).map(session_row).collect();
        let outcome = insert_session_logs(&conn, &rows).unwrap();
        assert_eq!(outcome.imported, 1201);
        assert!(outcome.errors.is_empty());
        assert_eq!(outcome.transactions, 3);
        assert!(conn.is_autocommit());

        // 结果与逐行 insert_session_log 一致。
        let single = empty_db();
        for r in &rows {
            insert_session_log(
                &single,
                &r.timestamp,
                &r.client,
                &r.provider,
                &r.model,
                &r.route,
                &r.source,
                &r.session_id,
                &r.external_id,
                r.input_tokens,
                r.output_tokens,
                r.cache_write_tokens,
                r.cache_read_tokens,
                r.cost,
            )
            .unwrap();
        }
        assert_eq!(dump_session_rows(&conn), dump_session_rows(&single));
    }

    #[test]
    fn insert_session_logs_reports_bad_rows_and_keeps_good_ones() {
        let conn = empty_db();
        // 注入单行写入失败,验证逐行报错不影响同批其它行。
        conn.execute_batch(
            "CREATE TRIGGER fail_row BEFORE INSERT ON request_logs
             WHEN NEW.external_id = 'ext-2'
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
        let mut rows: Vec<SessionLogRow> = (0..3).map(session_row).collect();
        rows[1].timestamp = "garbage".into();
        let outcome = insert_session_logs(&conn, &rows).unwrap();
        assert_eq!(outcome.imported, 2, "时间无法解析的行也应落库");
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].0, "ext-2");
        assert!(conn.is_autocommit());
    }
}
