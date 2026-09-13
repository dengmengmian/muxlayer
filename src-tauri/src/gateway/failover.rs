//! Failover 候选排序:把"主 provider 优先 + 失败转移候选 + vision 过滤 + 会话亲和"
//! 这套排序逻辑收敛到单一来源,供各协议 handler 共用,避免在 routes.rs 里各写一份、
//! 改一处漏一处(vision 过滤此前只在 /v1/responses 实现就是这么漏的)。
//!
//! 尝试循环本身([`run_attempts`])也在这里:四个协议入口共用同一份
//! "尝试 → 记熔断 → 按状态码决定是否切下一个"逻辑,状态码只从 [`upstream_status_of`] 取。

use std::collections::HashMap;
use std::future::Future;

use serde_json::json;

use crate::errors::{codes, AppError};
use crate::gateway::provider_selector::{ProviderCandidate, ProviderSelection};
use crate::models::provider::Provider;
use crate::storage::db::DbPool;

/// AppError → 上游 HTTP 状态码。所有 route 的 failover / 熔断判断只走这一个入口。
///
/// - 网关自己构造的上游错误(pass-through 非 2xx、SSE bootstrap 错误帧)带结构化
///   `upstream` 状态码,直接用;
/// - `providers::adapter` 产出的 `UPSTREAM_*` 错误只在 message 里有
///   "HTTP nnn"(adapter 不在网关模块内),兜底解析;
/// - 连不上上游(连接/读取失败)按 502 处理。
pub fn upstream_status_of(err: &AppError) -> Option<u16> {
    if let Some(status) = err.upstream_status() {
        return Some(status);
    }
    match err.code.as_str() {
        codes::UPSTREAM_NON_STREAM_ERROR | codes::UPSTREAM_STREAM_ERROR => {
            let i = err.message.find("HTTP ")?;
            err.message[i + 5..]
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse::<u16>()
                .ok()
        }
        codes::PROVIDER_REQUEST_FAILED
        | codes::PASS_THROUGH_REQUEST_FAILED
        | codes::PASS_THROUGH_STREAM_FAILED => Some(502),
        _ => None,
    }
}

/// 单次候选尝试的结果,决定驱动器下一步怎么走。
pub enum Attempt<T> {
    /// 上游已成功并开始向客户端返回。流式响应此时字节已经在路上,绝不重试。
    Success(T),
    /// provider 请求失败:记熔断失败,按 `should_failover` 决定是否换下一个。
    ProviderFailed(AppError),
    /// 候选本身不可用(provider 不存在 / 配置错 / 协议不支持):不记熔断,试下一个。
    Skip(AppError),
    /// 请求本身有问题(解析/转换失败):换 provider 也没用,立即结束,不记熔断。
    Abort(AppError),
}

/// 把 handler 返回的错误归类:请求侧(解析 / 转换)错误换 provider 也不会好,
/// 直接结束;其余一律按 provider 失败处理。
pub fn classify_error<T>(err: AppError) -> Attempt<T> {
    match err.code.as_str() {
        codes::RESPONSES_PARSE_ERROR
        | codes::CHAT_PARSE_ERROR
        | codes::MESSAGES_PARSE_ERROR
        | codes::GEMINI_PARSE_ERROR
        | codes::TRANSFORM_ERROR
        | codes::FUNCTION_CALL_OUTPUT_ID_MISSING => Attempt::Abort(err),
        _ => Attempt::ProviderFailed(err),
    }
}

/// 所有协议入口共用的 failover 驱动:按 `attempt_order` 逐个尝试,成功记
/// `mark_success`,provider 侧失败记 `mark_failure`(熔断的唯一数据来源,均为后台写),
/// 并按候选的 failover 状态码 / 关键字决定是否继续。
pub async fn run_attempts<T, F, Fut>(
    db: &DbPool,
    attempt_order: &[&ProviderCandidate],
    is_failover: bool,
    mut attempt: F,
) -> Result<T, AppError>
where
    F: FnMut(usize, ProviderCandidate) -> Fut,
    Fut: Future<Output = Attempt<T>>,
{
    let total = attempt_order.len();
    // 耗尽时优先返回最后一次 provider 失败(带上游状态码 / body),其次才是跳过原因。
    let mut last_error: Option<AppError> = None;
    let mut last_skip: Option<AppError> = None;
    let mut attempts_trace: Vec<serde_json::Value> = Vec::new();

    for (idx, candidate) in attempt_order.iter().enumerate() {
        match attempt(idx, (*candidate).clone()).await {
            Attempt::Success(value) => {
                // 熔断标记后台写:不能让首字节等 DB 连接池(日志写入可能占满连接)。
                let provider_id = candidate.provider_id.clone();
                crate::runtime::db_blocking_detached(db, "mark_success", move |conn| {
                    crate::storage::provider_runtime_status::mark_success(conn, &provider_id)
                });
                return Ok(value);
            }
            Attempt::Abort(err) => return Err(err),
            Attempt::Skip(err) => {
                attempts_trace.push(json!({
                    "provider": &candidate.provider_name, "attempt": idx + 1,
                    "error": &err.message, "skipped": true,
                }));
                last_skip = Some(err);
            }
            Attempt::ProviderFailed(err) => {
                let status = upstream_status_of(&err);
                attempts_trace.push(json!({
                    "provider": &candidate.provider_name, "attempt": idx + 1,
                    "error": &err.message, "status": status,
                }));
                // 只有 provider 侧失败才记熔断;请求侧 4xx(prompt 太长 / 404 / 413 / 422)
                // 记了会把主 provider 打进 cooldown,也不重置已有计数。
                if is_provider_side_failure(status, &err.message, candidate) {
                    let (provider_id, code, message, cooldown) = (
                        candidate.provider_id.clone(),
                        err.code.clone(),
                        err.message.clone(),
                        candidate.cooldown_seconds,
                    );
                    crate::runtime::db_blocking_detached(db, "mark_failure", move |conn| {
                        crate::storage::provider_runtime_status::mark_failure(
                            conn,
                            &provider_id,
                            &code,
                            &message,
                            cooldown,
                        )
                    });
                }

                if is_failover
                    && idx + 1 < total
                    && crate::gateway::provider_selector::should_failover(
                        status,
                        &err.message,
                        candidate,
                    )
                {
                    let reason = status
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| err.code.clone());
                    crate::gateway::metrics::record_failover(&candidate.provider_name, &reason);
                    tracing::warn!(
                        provider = %candidate.provider_name,
                        reason = %reason,
                        "provider failed, failing over to next candidate"
                    );
                    last_error = Some(err);
                    continue;
                }
                if attempts_trace.len() > 1 {
                    tracing::warn!(attempts = %serde_json::Value::Array(attempts_trace), "failover gave up");
                }
                return Err(err);
            }
        }
    }

    if !attempts_trace.is_empty() {
        tracing::warn!(attempts = %serde_json::Value::Array(attempts_trace), "all failover candidates exhausted");
    }
    Err(last_error
        .or(last_skip)
        .unwrap_or_else(|| AppError::new(codes::FAILOVER_EXHAUSTED, "All providers failed")))
}

/// 失败是否算 provider 侧(计入熔断):无状态码(连接 / 超时 / 流错误)、408、429、
/// 401/402/403(鉴权 / 额度)、非 4xx(5xx、被拦下的 3xx),或命中候选配置的错误关键字。
/// 其余 4xx 是请求本身的问题。
fn is_provider_side_failure(
    status: Option<u16>,
    message: &str,
    candidate: &ProviderCandidate,
) -> bool {
    match status {
        Some(401 | 402 | 403 | 408 | 429) | None => true,
        Some(s) if (400..500).contains(&s) => {
            // 状态码传 None:只看关键字,不让配置的 failover 状态码把请求侧 4xx 算进熔断。
            crate::gateway::provider_selector::should_failover(None, message, candidate)
        }
        Some(_) => true,
    }
}

/// 全局兜底选路(没有 route profile)不产出候选列表;补一个由选中 provider
/// 构成的候选,让驱动器对它同样记熔断、走同一条路径。
pub fn ensure_candidates(selection: &mut ProviderSelection) {
    if !selection.candidates.is_empty() {
        return;
    }
    selection.candidates.push(ProviderCandidate {
        provider_id: selection.provider.id.clone(),
        provider_name: selection.provider.name.clone(),
        priority: selection.priority,
        model: selection.model.clone(),
        routing_conditions: None,
        in_cooldown: false,
        supports_vision: selection.provider.supports_vision,
        cooldown_seconds: 0,
        failover_on_status_codes: vec![402, 429, 500, 502, 503, 504],
        failover_on_error_keywords: vec![],
    });
}

/// 一次性加载尝试顺序里所有候选的 provider(一个 blocking 任务、一条连接),
/// 避免每次尝试都在 async worker 上单独查库。查不到的 provider 不在 map 里;
/// 其它 DB 错误直接返回。
pub async fn load_providers(
    db: &DbPool,
    attempt_order: &[&ProviderCandidate],
) -> Result<HashMap<String, Provider>, AppError> {
    let ids: Vec<String> = attempt_order
        .iter()
        .map(|c| c.provider_id.clone())
        .collect();
    crate::runtime::db_blocking(db, move |conn| {
        let mut map = HashMap::new();
        for id in ids {
            match crate::storage::providers::get_by_id(conn, &id) {
                Ok(p) => {
                    map.insert(id, p);
                }
                // provider 已被删:不进 map,由调用方按 Skip 处理。
                Err(e) if e.code == "NOT_FOUND" => {}
                // DB 故障(连接池超时、表损坏)不能伪装成"provider 不存在"。
                Err(e) => return Err(e),
            }
        }
        Ok(map)
    })
    .await
}

/// 构建实际尝试顺序:
/// 1. 主 provider 优先(带图请求且主 provider 明确不支持 vision 时跳过);
/// 2. failover 模式下追加其余非 cooldown 候选(同样按 vision 过滤);
/// 3. 若全部被 vision 过滤掉,退回不带 vision 过滤的原始顺序(vision 是提示不是硬约束);
/// 4. 会话亲和:上一轮命中上游 prompt 缓存的 provider 提到队首(命中且不在 cooldown 时)。
pub fn build_attempt_order<'a>(
    candidates: &'a [ProviderCandidate],
    primary_id: &str,
    is_failover: bool,
    request_has_images: bool,
    session_id: Option<&str>,
) -> Vec<&'a ProviderCandidate> {
    let mut attempt_order: Vec<&ProviderCandidate> = Vec::new();

    // 主 provider
    if let Some(primary) = candidates.iter().find(|c| c.provider_id == primary_id) {
        if !request_has_images || primary.supports_vision != Some(false) {
            attempt_order.push(primary);
        }
    }
    // 其余候选(failover)
    if is_failover {
        for c in candidates {
            if c.provider_id != primary_id && !c.in_cooldown {
                if request_has_images && c.supports_vision == Some(false) {
                    continue; // 显式不支持 vision 的 provider 跳过
                }
                attempt_order.push(c);
            }
        }
    }
    // 全部被 vision 过滤掉时,退回原始顺序(不带 vision 过滤)
    if attempt_order.is_empty() {
        if let Some(primary) = candidates.iter().find(|c| c.provider_id == primary_id) {
            attempt_order.push(primary);
        }
        if is_failover {
            for c in candidates {
                if c.provider_id != primary_id && !c.in_cooldown {
                    attempt_order.push(c);
                }
            }
        }
    }

    // 会话亲和:把上一轮命中缓存的 provider 提到队首。亲和是提示,
    // 候选集里没有或在 cooldown 时忽略。
    if let Some(sid) = session_id {
        if let Some(entry) = crate::gateway::session_affinity::lookup(sid) {
            if let Some(pos) = attempt_order
                .iter()
                .position(|c| c.provider_id == entry.provider_id && !c.in_cooldown)
            {
                if pos > 0 {
                    let preferred = attempt_order.remove(pos);
                    attempt_order.insert(0, preferred);
                }
            }
        }
    }

    attempt_order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, in_cooldown: bool, vision: Option<bool>) -> ProviderCandidate {
        ProviderCandidate {
            provider_id: id.to_string(),
            provider_name: id.to_string(),
            priority: 0,
            model: "m".to_string(),
            routing_conditions: None,
            in_cooldown,
            supports_vision: vision,
            cooldown_seconds: 0,
            failover_on_status_codes: vec![],
            failover_on_error_keywords: vec![],
        }
    }

    fn failover_cand(id: &str) -> ProviderCandidate {
        let mut c = cand(id, false, None);
        c.cooldown_seconds = 60;
        c.failover_on_status_codes = vec![429, 500, 502, 503];
        c
    }

    fn test_pool() -> DbPool {
        let manager = r2d2_sqlite::SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(manager).unwrap();
        crate::storage::migrations::run_migrations(&pool.get().unwrap()).unwrap();
        pool
    }

    fn failures_now(pool: &DbPool, id: &str) -> i64 {
        crate::storage::provider_runtime_status::get(&pool.get().unwrap(), id)
            .unwrap()
            .consecutive_failures
    }

    /// 熔断标记是后台写入:期望非 0 时轮询到落盘;期望 0 时先等一段再读,
    /// 避免"还没写进去"被误判成"没写"。
    async fn failures(pool: &DbPool, id: &str, expected: i64) -> i64 {
        if expected == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            return failures_now(pool, id);
        }
        for _ in 0..100 {
            let n = failures_now(pool, id);
            if n == expected {
                return n;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        failures_now(pool, id)
    }

    fn upstream(status: u16) -> AppError {
        AppError::new(codes::UPSTREAM_NON_STREAM_ERROR, format!("HTTP {status}"))
            .with_upstream_response(status, "{}")
    }

    #[test]
    fn upstream_status_mapping_covers_every_upstream_error_shape() {
        // 结构化字段优先
        assert_eq!(upstream_status_of(&upstream(429)), Some(429));
        // adapter 形态:message 里的 "HTTP nnn"
        let adapter = AppError::new(
            codes::UPSTREAM_STREAM_ERROR,
            "Provider returned HTTP 500 Internal Server Error",
        );
        assert_eq!(upstream_status_of(&adapter), Some(500));
        let bootstrap = AppError::new(
            codes::UPSTREAM_STREAM_ERROR,
            "Provider returned HTTP 429: quota",
        );
        assert_eq!(upstream_status_of(&bootstrap), Some(429));
        // 连接失败一律 502(之前 PASS_THROUGH_* 落到 None,不触发 failover)
        for code in [
            codes::PROVIDER_REQUEST_FAILED,
            codes::PASS_THROUGH_REQUEST_FAILED,
            codes::PASS_THROUGH_STREAM_FAILED,
        ] {
            assert_eq!(
                upstream_status_of(&AppError::new(code, "x")),
                Some(502),
                "{code}"
            );
        }
        assert_eq!(
            upstream_status_of(&AppError::new(codes::TRANSFORM_ERROR, "HTTP 500")),
            None
        );
    }

    #[tokio::test]
    async fn driver_fails_over_marks_failure_then_success() {
        let pool = test_pool();
        let cands = [failover_cand("a"), failover_cand("b")];
        let order: Vec<&ProviderCandidate> = cands.iter().collect();
        let mut calls = Vec::new();
        let result = run_attempts(&pool, &order, true, |_, c| {
            calls.push(c.provider_id.clone());
            async move {
                if c.provider_id == "a" {
                    Attempt::ProviderFailed(upstream(503))
                } else {
                    Attempt::Success(c.provider_id)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), "b");
        assert_eq!(calls, vec!["a", "b"]);
        assert_eq!(failures(&pool, "a", 1).await, 1);
        assert_eq!(failures(&pool, "b", 0).await, 0);
    }

    #[tokio::test]
    async fn driver_stops_on_non_failover_status_and_after_success() {
        let pool = test_pool();
        let cands = [failover_cand("a"), failover_cand("b")];
        let order: Vec<&ProviderCandidate> = cands.iter().collect();

        // 400 不在 failover 状态码里:只打一次,错误原样返回;请求侧 4xx 不记熔断
        let mut n = 0;
        let err = run_attempts(&pool, &order, true, |_, _| {
            n += 1;
            async { Attempt::<()>::ProviderFailed(upstream(400)) }
        })
        .await
        .unwrap_err();
        assert_eq!(n, 1);
        assert_eq!(err.upstream_status(), Some(400));
        assert_eq!(failures(&pool, "a", 0).await, 0);

        // 流已开始(Success)后绝不再尝试下一个
        let mut n = 0;
        run_attempts(&pool, &order, true, |_, _| {
            n += 1;
            async { Attempt::Success(()) }
        })
        .await
        .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn exhausted_failover_returns_provider_error_not_later_skip_reason() {
        // a 返回 503(可 failover)后 b 被跳过:客户端应拿到 a 的上游错误(含原始
        // 状态码 / body),而不是 "provider 不存在" 之类的跳过原因。
        let pool = test_pool();
        let cands = [failover_cand("a"), failover_cand("b")];
        let order: Vec<&ProviderCandidate> = cands.iter().collect();
        let err = run_attempts(&pool, &order, true, |_, c| async move {
            if c.provider_id == "a" {
                Attempt::<()>::ProviderFailed(upstream(503))
            } else {
                Attempt::Skip(AppError::not_found("Provider", "b"))
            }
        })
        .await
        .unwrap_err();
        assert_eq!(err.upstream_status(), Some(503));
    }

    #[tokio::test]
    async fn driver_skip_does_not_mark_and_abort_stops() {
        let pool = test_pool();
        let cands = [failover_cand("a"), failover_cand("b")];
        let order: Vec<&ProviderCandidate> = cands.iter().collect();

        let result = run_attempts(&pool, &order, true, |_, c| async move {
            if c.provider_id == "a" {
                Attempt::Skip(AppError::internal("no config"))
            } else {
                Attempt::Success(())
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(failures(&pool, "a", 0).await, 0);

        let mut n = 0;
        let err = run_attempts(&pool, &order, true, |_, _| {
            n += 1;
            async { Attempt::<()>::Abort(AppError::new(codes::TRANSFORM_ERROR, "bad")) }
        })
        .await
        .unwrap_err();
        assert_eq!(n, 1);
        assert_eq!(err.code, codes::TRANSFORM_ERROR);
        assert_eq!(failures(&pool, "a", 0).await, 0);
    }

    #[tokio::test]
    async fn only_provider_side_failures_mark_circuit_breaker() {
        // 请求侧 4xx(prompt 太长 / 模型不存在 / body 太大 / 参数错)换 provider 也不会好,
        // 不能把主 provider 打进 cooldown;provider 侧失败照常记。
        let pool = test_pool();
        let cases: Vec<(&str, AppError, i64)> = vec![
            ("s400", upstream(400), 0),
            ("s404", upstream(404), 0),
            ("s413", upstream(413), 0),
            ("s422", upstream(422), 0),
            ("s401", upstream(401), 1),
            ("s402", upstream(402), 1),
            ("s403", upstream(403), 1),
            ("s408", upstream(408), 1),
            ("s429", upstream(429), 1),
            ("s500", upstream(500), 1),
            ("s529", upstream(529), 1),
            // 没有状态码(流读取失败等)按 provider 侧失败处理
            (
                "none",
                AppError::new(codes::UPSTREAM_STREAM_ERROR, "stream idle"),
                1,
            ),
        ];
        for (id, err, expected) in cases {
            let c = [failover_cand(id)];
            let order: Vec<&ProviderCandidate> = c.iter().collect();
            let _ = run_attempts(&pool, &order, false, |_, _| {
                let err = err.clone();
                async move { Attempt::<()>::ProviderFailed(err) }
            })
            .await;
            assert_eq!(failures(&pool, id, expected).await, expected, "{id}");
        }

        // 配置的错误关键字命中时,即使是 4xx 也按 provider 失败记
        let mut kw = failover_cand("kw404");
        kw.failover_on_error_keywords = vec!["model_not_available".to_string()];
        let c = [kw];
        let order: Vec<&ProviderCandidate> = c.iter().collect();
        let _ = run_attempts(&pool, &order, false, |_, _| async {
            Attempt::<()>::ProviderFailed(
                AppError::new(
                    codes::UPSTREAM_NON_STREAM_ERROR,
                    "HTTP 404 model_not_available",
                )
                .with_upstream_response(404, "{}"),
            )
        })
        .await;
        assert_eq!(failures(&pool, "kw404", 1).await, 1);
    }

    #[tokio::test]
    async fn request_side_4xx_does_not_reset_existing_failures() {
        let pool = test_pool();
        crate::storage::provider_runtime_status::mark_failure(
            &pool.get().unwrap(),
            "a",
            "X",
            "boom",
            0,
        )
        .unwrap();
        let c = [failover_cand("a")];
        let order: Vec<&ProviderCandidate> = c.iter().collect();
        let _ = run_attempts(&pool, &order, false, |_, _| async {
            Attempt::<()>::ProviderFailed(upstream(404))
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(failures_now(&pool, "a"), 1);
    }

    #[tokio::test]
    async fn circuit_breaker_marks_do_not_wait_for_db_pool() {
        // 连接池被占满时(日志写入占着连接),熔断标记不能卡住首字节返回。
        let manager = r2d2_sqlite::SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder()
            .max_size(1)
            .connection_timeout(std::time::Duration::from_secs(2))
            .build(manager)
            .unwrap();
        crate::storage::migrations::run_migrations(&pool.get().unwrap()).unwrap();
        let c = [failover_cand("a")];
        let order: Vec<&ProviderCandidate> = c.iter().collect();

        let held = pool.get().unwrap();
        let ok = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            run_attempts(&pool, &order, false, |_, _| async { Attempt::Success(()) }),
        )
        .await;
        assert!(
            ok.is_ok(),
            "Success must return without waiting for a DB connection"
        );

        let failed = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            run_attempts(&pool, &order, false, |_, _| async {
                Attempt::<()>::ProviderFailed(upstream(503))
            }),
        )
        .await;
        assert!(
            failed.is_ok(),
            "ProviderFailed must return without waiting for a DB connection"
        );
        drop(held);
    }

    #[tokio::test]
    async fn load_providers_propagates_db_errors_but_skips_missing_ids() {
        let pool = test_pool();
        let c = [failover_cand("missing")];
        let order: Vec<&ProviderCandidate> = c.iter().collect();
        let map = load_providers(&pool, &order).await.unwrap();
        assert!(map.is_empty(), "missing provider is simply absent");

        pool.get()
            .unwrap()
            .execute_batch("DROP TABLE providers")
            .unwrap();
        let err = load_providers(&pool, &order).await.unwrap_err();
        assert_eq!(err.code, "DATABASE_ERROR");
    }

    fn ids(order: &[&ProviderCandidate]) -> Vec<String> {
        order.iter().map(|c| c.provider_id.clone()).collect()
    }

    #[test]
    fn primary_first_then_failover_candidates() {
        let cands = vec![cand("a", false, None), cand("b", false, None)];
        let order = build_attempt_order(&cands, "a", true, false, None);
        assert_eq!(ids(&order), vec!["a", "b"]);
    }

    #[test]
    fn no_failover_keeps_primary_only() {
        let cands = vec![cand("a", false, None), cand("b", false, None)];
        let order = build_attempt_order(&cands, "a", false, false, None);
        assert_eq!(ids(&order), vec!["a"]);
    }

    #[test]
    fn cooldown_candidates_skipped_in_failover() {
        let cands = vec![cand("a", false, None), cand("b", true, None)];
        let order = build_attempt_order(&cands, "a", true, false, None);
        assert_eq!(ids(&order), vec!["a"]);
    }

    #[test]
    fn images_skip_non_vision_providers() {
        let cands = vec![cand("a", false, Some(false)), cand("b", false, Some(true))];
        let order = build_attempt_order(&cands, "a", true, true, None);
        assert_eq!(ids(&order), vec!["b"]);
    }

    #[test]
    fn images_fall_back_to_original_order_when_all_filtered() {
        // 全部 provider 都不支持 vision → 退回原始顺序而非空
        let cands = vec![cand("a", false, Some(false)), cand("b", false, Some(false))];
        let order = build_attempt_order(&cands, "a", true, true, None);
        assert_eq!(ids(&order), vec!["a", "b"]);
    }

    #[test]
    fn unknown_vision_capability_not_filtered() {
        // supports_vision = None(未知)不应被过滤
        let cands = vec![cand("a", false, None), cand("b", false, None)];
        let order = build_attempt_order(&cands, "a", true, true, None);
        assert_eq!(ids(&order), vec!["a", "b"]);
    }

    #[test]
    fn session_affinity_promotes_cached_provider_to_front() {
        // cache-hit 过的 backup 应提到队首（亲和是提示，候选集内且非 cooldown）
        crate::gateway::session_affinity::clear();
        crate::gateway::session_affinity::record("sa_failover_test", "b");
        let cands = vec![cand("a", false, None), cand("b", false, None)];
        let order = build_attempt_order(&cands, "a", true, false, Some("sa_failover_test"));
        assert_eq!(ids(&order), vec!["b", "a"]);
        crate::gateway::session_affinity::clear();
    }

    #[test]
    fn force_cheapest_disables_affinity_when_session_id_none() {
        // 预算闸 force_cheapest 路径：调用方传 session_id=None，亲和不得把贵上游提到队首。
        // 主是 cheap(a)，亲和绑定 expensive(b)——传 None 后仍 a 优先。
        crate::gateway::session_affinity::clear();
        crate::gateway::session_affinity::record("sa_budget_force", "b");
        let cands = vec![cand("a", false, None), cand("b", false, None)];
        let order = build_attempt_order(&cands, "a", true, false, None);
        assert_eq!(
            ids(&order),
            vec!["a", "b"],
            "force_cheapest must keep primary/cheapest first, not affinity provider"
        );
        // 对照：若误传 affinity sid，会把 b 提到队首——证明 None 是有效开关。
        let order_with_aff = build_attempt_order(&cands, "a", true, false, Some("sa_budget_force"));
        assert_eq!(ids(&order_with_aff), vec!["b", "a"]);
        crate::gateway::session_affinity::clear();
    }
}
