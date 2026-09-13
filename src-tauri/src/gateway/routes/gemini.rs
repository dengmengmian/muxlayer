use axum::body::Body;
use axum::extract::State as AxumState;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde_json::{json, Value};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::errors::AppError;
use crate::gateway::failover::Attempt;
use crate::gateway::provider_selector::ProviderCandidate;
use crate::models::provider::Provider;
use crate::providers::adapter::{self, ProviderConfig};
use crate::transform::gemini_to_chat;

use super::shared::{
    check_budget, client_disconnected_error, detect_client_from_ua, log_request_error,
    log_request_error_full, log_request_success, native_model_override, refine_struct_body,
    request_body_or_gateway_error, sanitize_body, select_providers, stream_error_status,
    trace_with_degradation_events, truncate_str, GatewayError, CLIENT_DISCONNECTED_STATUS,
};
use super::GatewayState;

// ── POST /v1beta/models/:model:generateContent (Gemini CLI input) ──

pub async fn handle_gemini_generate(
    headers: HeaderMap,
    axum::extract::Path(model_path): axum::extract::Path<String>,
    AxumState(state): AxumState<GatewayState>,
    body: Result<bytes::Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Response, GatewayError> {
    // 鉴权 + Host/Origin 边界校验在 server.rs 的中间件里、读 body 之前完成。
    let body = request_body_or_gateway_error(body)?;

    // Gemini 路径形如 "gemini-2.5-flash:generateContent" / ":streamGenerateContent"
    // / ":countTokens"。axum 无法在 router 层按 action 分发，handler 入口分流。
    if model_path.ends_with(":countTokens") {
        let body = crate::gateway::body_decode::decode(&headers, body, state.request_body_limit)
            .map_err(GatewayError)?;
        let v: Value = serde_json::from_str(&body).map_err(|e| {
            GatewayError(AppError::new(
                crate::errors::codes::COUNT_TOKENS_PARSE_ERROR,
                format!("Failed to parse: {e}"),
            ))
        })?;
        let mut chars: usize = 0;
        if let Some(contents) = v.get("contents").and_then(|c| c.as_array()) {
            for c in contents {
                if let Some(parts) = c.get("parts").and_then(|p| p.as_array()) {
                    for part in parts {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                            chars += t.chars().count();
                        }
                    }
                }
            }
        }
        let estimate = ((chars as f64) / 4.0).ceil() as i64;
        return Ok(Json(json!({"totalTokens": estimate})).into_response());
    }

    let start = Instant::now();
    let request_id = format!(
        "req_{}",
        &uuid::Uuid::new_v4().to_string().replace('-', "")[..12]
    );
    let client_type = detect_client_from_ua(&headers, "Gemini CLI");

    let body = crate::gateway::body_decode::decode(&headers, body, state.request_body_limit)
        .map_err(|e| {
            log_request_error(
                &state.db,
                &client_type,
                "/v1beta/generateContent",
                &request_id,
                "",
                None,
                &e,
                start.elapsed().as_millis() as i64,
            );
            GatewayError(e)
        })?;

    // Extract model name from path (e.g. "gemini-2.5-flash" from "gemini-2.5-flash:generateContent")
    let model_name = model_path
        .split(':')
        .next()
        .unwrap_or(&model_path)
        .to_string();
    let is_stream = model_path.contains("streamGenerateContent");

    let force_cheapest = check_budget(&state.db).await?;

    let gemini_body: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        let err = AppError::new(
            crate::errors::codes::GEMINI_PARSE_ERROR,
            format!("Failed to parse Gemini request: {e}"),
        );
        log_request_error(
            &state.db,
            &client_type,
            "/v1beta/generateContent",
            &request_id,
            &sanitize_body(&body),
            None,
            &err,
            start.elapsed().as_millis() as i64,
        );
        err
    })?;

    let analysis = crate::gateway::provider_selector::analyze_gemini_value(&gemini_body);

    // Select provider (use openai_responses route profile since Gemini CLI is a coding agent)
    let selection = select_providers(
        &state.db,
        &["openai_responses", "openai_chat_completions"],
        Some(model_name.clone()),
        analysis.clone(),
        force_cheapest,
    )
    .await
    .map_err(|e| {
        log_request_error(
            &state.db,
            &client_type,
            "/v1beta/generateContent",
            &request_id,
            &sanitize_body(&body),
            None,
            &e,
            start.elapsed().as_millis() as i64,
        );
        GatewayError(e)
    })?;

    // 与其它入口共用排序 + failover 驱动(Gemini 请求不带会话 id,不做亲和)。
    let request_has_images = analysis.has_images;
    let is_failover = selection.mode == "failover" && selection.candidates.len() > 1;
    let attempt_order = crate::gateway::failover::build_attempt_order(
        &selection.candidates,
        &selection.provider.id,
        is_failover,
        request_has_images,
        None,
    );
    let providers = crate::gateway::failover::load_providers(&state.db, &attempt_order)
        .await
        .map_err(GatewayError)?;

    let raw_body = sanitize_body(&body);
    let ctx = GeminiAttemptCtx {
        state: &state,
        gemini_body: &gemini_body,
        raw_body: &raw_body,
        model_name: &model_name,
        is_stream,
        request_id: &request_id,
        start,
        client_type: &client_type,
    };
    let providers = &providers;
    crate::gateway::failover::run_attempts(
        &state.db,
        &attempt_order,
        is_failover,
        |_, candidate| async move {
            match providers.get(&candidate.provider_id) {
                Some(provider) => attempt_gemini_provider(ctx, provider, &candidate).await,
                None => Attempt::Skip(AppError::not_found("Provider", &candidate.provider_id)),
            }
        },
    )
    .await
    .map_err(GatewayError)
}

/// 一次 Gemini 请求在所有候选间共享的只读上下文。
#[derive(Clone, Copy)]
struct GeminiAttemptCtx<'a> {
    state: &'a GatewayState,
    gemini_body: &'a Value,
    raw_body: &'a str,
    model_name: &'a str,
    is_stream: bool,
    request_id: &'a str,
    start: Instant,
    client_type: &'a str,
}

/// 对单个候选发起一次 Gemini 请求:google_gemini 原生直通,否则转 Chat。
async fn attempt_gemini_provider(
    ctx: GeminiAttemptCtx<'_>,
    provider: &Provider,
    candidate: &ProviderCandidate,
) -> Attempt<Response> {
    let state = ctx.state;
    let is_stream = ctx.is_stream;
    let start = ctx.start;
    let config = match ProviderConfig::from_provider(provider) {
        Ok(c) => c,
        Err(e) => {
            // 配置错(如缺 API key)要在日志页可见,再跳到下一个候选。
            log_request_error(
                &state.db,
                ctx.client_type,
                "/v1beta/generateContent",
                ctx.request_id,
                ctx.raw_body,
                None,
                &e,
                start.elapsed().as_millis() as i64,
            );
            return Attempt::Skip(e);
        }
    };
    let resolved_model = candidate.model.clone();
    let log_upstream_error = |converted: &str, model: &str, err: &AppError| {
        log_request_error_full(
            &state.db,
            ctx.client_type,
            "/v1beta/generateContent",
            ctx.request_id,
            ctx.raw_body,
            converted,
            &config.name,
            model,
            err,
            502,
            start.elapsed().as_millis() as i64,
        );
    };

    // Gemini → Gemini passthrough：选中的 provider 是 google_gemini 原生上游，
    // 直接调上游 generateContent / streamGenerateContent，body 原样转发回 client。
    // 不绕 Chat 转换，避免丢 thinking / grounding / safetySettings 这些 Gemini-only 字段。
    if config.is_gemini() {
        // Override body 里的 model（如有 mapping）
        let model_override =
            native_model_override(provider, Some(ctx.model_name), Some(&resolved_model));
        let final_model = model_override.unwrap_or(resolved_model.clone());

        if is_stream {
            let upstream_resp = match adapter::send_gemini_stream(
                &state.http_client,
                &config,
                ctx.gemini_body,
                &final_model,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    log_upstream_error("", &final_model, &e);
                    return Attempt::ProviderFailed(e);
                }
            };
            let boot = match crate::gateway::sse_bootstrap::bootstrap_detect(upstream_resp).await {
                Ok(b) => b,
                Err(e) => {
                    log_upstream_error("", &final_model, &e);
                    return Attempt::ProviderFailed(e);
                }
            };
            let (tx, rx) = mpsc::channel::<String>(256);
            let db = state.db.clone();
            let provider_name = config.name.clone();
            let req_id = ctx.request_id.to_string();
            let raw_req = ctx.raw_body.to_string();
            let model_clone = final_model.clone();
            let client_type_owned = ctx.client_type.to_string();
            tokio::spawn(async move {
                use futures::StreamExt;
                let mut utf8_pending: Vec<u8> = Vec::new();
                let mut prefix_text = String::new();
                crate::gateway::stream_utf8::append_utf8_safe(
                    &mut prefix_text,
                    &mut utf8_pending,
                    &boot.prefix,
                );
                // 区分两种提前结束：客户端断开(tx.send 失败)时立刻停读上游、不再烧 token,
                // 日志记 client disconnected；上游流 Err 才是真失败，必须记成非 2xx，
                // 否则日志层假成功、污染成本/健康统计。
                let mut stream_err: Option<AppError> = None;
                if !prefix_text.is_empty() && tx.send(prefix_text).await.is_err() {
                    stream_err = Some(client_disconnected_error());
                }
                let mut stream = boot.stream;
                while stream_err.is_none() {
                    let Some(chunk) = stream.next().await else {
                        break;
                    };
                    match chunk {
                        Ok(b) => {
                            let mut text = String::new();
                            crate::gateway::stream_utf8::append_utf8_safe(
                                &mut text,
                                &mut utf8_pending,
                                &b,
                            );
                            if tx.send(text).await.is_err() {
                                stream_err = Some(client_disconnected_error());
                            }
                        }
                        Err(e) => {
                            stream_err = Some(AppError::new(
                                crate::errors::codes::UPSTREAM_STREAM_ERROR,
                                format!("Gemini 上游流中断: {e}"),
                            ));
                        }
                    }
                }
                let latency = start.elapsed().as_millis() as i64;
                let trace =
                    json!({"mode": "native_pass_through", "protocol": "gemini", "stream": true})
                        .to_string();
                if let Some(err) = stream_err {
                    log_request_error_full(
                        &db,
                        &client_type_owned,
                        "/v1beta/generateContent",
                        &req_id,
                        &raw_req,
                        "",
                        &provider_name,
                        &model_clone,
                        &err,
                        stream_error_status(&err),
                        latency,
                    );
                } else {
                    log_request_success(
                        &db,
                        &client_type_owned,
                        "/v1beta/generateContent",
                        &req_id,
                        &raw_req,
                        "",
                        "",
                        "",
                        None,
                        &provider_name,
                        &model_clone,
                        200,
                        latency,
                        Some(&trace),
                        crate::gateway::usage::TokenUsage::default(),
                    );
                }
            });
            let stream = ReceiverStream::new(rx);
            let body = Body::from_stream(tokio_stream::StreamExt::map(stream, |s| {
                Ok::<_, std::convert::Infallible>(s)
            }));
            return Attempt::Success(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .body(body)
                    .unwrap(),
            );
        } else {
            let result = adapter::send_gemini_non_stream(
                &state.http_client,
                &config,
                ctx.gemini_body,
                &final_model,
            )
            .await;
            match result {
                Ok(upstream_json) => {
                    let latency = start.elapsed().as_millis() as i64;
                    let (in_tok, out_tok) = crate::gateway::usage::extract_gemini(&upstream_json);
                    let trace = json!({"mode": "native_pass_through", "protocol": "gemini", "stream": false}).to_string();
                    log_request_success(
                        &state.db,
                        ctx.client_type,
                        "/v1beta/generateContent",
                        ctx.request_id,
                        ctx.raw_body,
                        "",
                        &serde_json::to_string(&upstream_json).unwrap_or_default(),
                        &serde_json::to_string(&upstream_json).unwrap_or_default(),
                        None,
                        &config.name,
                        &final_model,
                        200,
                        latency,
                        Some(&trace),
                        crate::gateway::usage::TokenUsage {
                            input: in_tok,
                            output: out_tok,
                            cache_write: None,
                            cache_read: None,
                            input_semantics:
                                crate::gateway::usage::InputCacheSemantics::IncludesCacheRead,
                        },
                    );
                    return Attempt::Success(Json(upstream_json).into_response());
                }
                Err(err) => {
                    log_upstream_error("", &final_model, &err);
                    return Attempt::ProviderFailed(err);
                }
            }
        }
    }

    // Convert Gemini → Chat Completions
    let mut chat_req = match gemini_to_chat::convert(ctx.gemini_body, &resolved_model) {
        Ok(r) => r,
        Err(e) => {
            log_request_error(
                &state.db,
                ctx.client_type,
                "/v1beta/generateContent",
                ctx.request_id,
                ctx.raw_body,
                None,
                &e,
                start.elapsed().as_millis() as i64,
            );
            return Attempt::Abort(e);
        }
    };
    chat_req.stream = is_stream;
    if !is_stream {
        chat_req.stream_options = None;
    }

    let _refiner_log = refine_struct_body(&state.db, provider, &mut chat_req);
    let mut converted_json = serde_json::to_string(&chat_req).unwrap_or_default();

    if is_stream {
        // Stream: Chat Completions SSE → convert each chunk to Gemini SSE format
        let upstream_resp =
            match adapter::send_stream(&state.http_client, &config, &mut chat_req).await {
                Ok(r) => r,
                Err(e) => {
                    log_upstream_error(&converted_json, &resolved_model, &e);
                    return Attempt::ProviderFailed(e);
                }
            };
        converted_json = serde_json::to_string(&chat_req).unwrap_or_default();

        // Bootstrap-validate the upstream Chat Completions stream before
        // committing to forwarding the converted Gemini SSE back to the client.
        let boot = match crate::gateway::sse_bootstrap::bootstrap_detect(upstream_resp).await {
            Ok(b) => b,
            Err(e) => {
                log_upstream_error(&converted_json, &resolved_model, &e);
                return Attempt::ProviderFailed(e);
            }
        };

        let (tx, rx) = mpsc::channel::<String>(256);
        let db = state.db.clone();
        let provider_name = config.name.clone();
        let model_clone = resolved_model.clone();
        let req_id = ctx.request_id.to_string();
        let raw_req = ctx.raw_body.to_string();
        let conv_req = converted_json.clone();
        let diagnostic_events = chat_req.diagnostic_events.clone();
        let client_type = ctx.client_type.to_string();

        tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = boot.stream;
            let mut utf8_pending: Vec<u8> = Vec::new();
            let mut buffer = String::new();
            crate::gateway::stream_utf8::append_utf8_safe(
                &mut buffer,
                &mut utf8_pending,
                &boot.prefix,
            );
            buffer = buffer.replace("\r\n", "\n");
            let mut full_text = String::new();
            let mut bootstrap_replayed = false;
            let mut gemini_stream = gemini_to_chat::GeminiStreamState::default();
            // 客户端断开时立刻停读上游(不再烧 token),日志记 client disconnected。
            let mut client_gone = false;

            'stream: loop {
                if bootstrap_replayed {
                    let chunk = match stream.next().await {
                        Some(Ok(b)) => b,
                        Some(Err(e)) => {
                            let msg = crate::gateway::sse_bootstrap::describe_stream_error(&e);
                            let payload = format!(
                                "data: {}\n\n",
                                json!({"error": {"code": 500, "status": "INTERNAL", "message": msg}})
                            );
                            let _ = tx.send(payload).await;
                            break;
                        }
                        None => break,
                    };
                    crate::gateway::stream_utf8::append_utf8_safe(
                        &mut buffer,
                        &mut utf8_pending,
                        &chunk,
                    );
                    buffer = buffer.replace("\r\n", "\n");
                }
                bootstrap_replayed = true;

                for line in crate::gateway::sse_buffer::take_complete_lines(&mut buffer) {
                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }
                    let Some(data) = line.strip_prefix("data:").map(|d| d.trim()) else {
                        continue;
                    };
                    if data == "[DONE]" {
                        // 上游明确结束:跳出外层读循环,不再等上游关连接。
                        break 'stream;
                    }

                    if let Ok(chunk_json) = serde_json::from_str::<Value>(data) {
                        // Accumulate text
                        if let Some(delta) = chunk_json
                            .get("choices")
                            .and_then(|c| c.as_array())
                            .and_then(|a| a.first())
                            .and_then(|c| c.get("delta"))
                        {
                            if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                                full_text.push_str(text);
                            }
                        }

                        if let Some(gemini_sse) = gemini_stream.chunk_to_gemini(&chunk_json) {
                            if tx.send(gemini_sse).await.is_err() {
                                client_gone = true;
                                break 'stream;
                            }
                        }
                    }
                }
            }
            // 上游没发 finish_reason 就结束时，补发仍在缓冲中的 functionCall
            if !client_gone {
                if let Some(gemini_sse) = gemini_stream.finish() {
                    let _ = tx.send(gemini_sse).await;
                }
            }

            let latency = start.elapsed().as_millis() as i64;
            if client_gone {
                log_request_error_full(
                    &db,
                    &client_type,
                    "/v1beta/generateContent",
                    &req_id,
                    &raw_req,
                    &conv_req,
                    &provider_name,
                    &model_clone,
                    &client_disconnected_error(),
                    CLIENT_DISCONNECTED_STATUS,
                    latency,
                );
                return;
            }
            let trace = trace_with_degradation_events(
                json!({"response_id": &req_id, "stream": true, "protocol": "gemini_input"}),
                &diagnostic_events,
            );
            log_request_success(
                &db,
                &client_type,
                "/v1beta/generateContent",
                &req_id,
                &raw_req,
                &conv_req,
                "",
                &truncate_str(&full_text, 10000),
                None,
                &provider_name,
                &model_clone,
                200,
                latency,
                Some(&trace),
                crate::gateway::usage::TokenUsage::default(),
            );
        });

        let stream = ReceiverStream::new(rx);
        let body = Body::from_stream(tokio_stream::StreamExt::map(stream, |s| {
            Ok::<_, std::convert::Infallible>(s)
        }));

        Attempt::Success(
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(body)
                .unwrap(),
        )
    } else {
        // Non-stream
        chat_req.stream = false;
        let result = adapter::send_non_stream(&state.http_client, &config, &mut chat_req).await;

        match result {
            Ok(upstream_json) => {
                converted_json = serde_json::to_string(&chat_req).unwrap_or_default();
                let gemini_resp =
                    gemini_to_chat::response_to_gemini(&upstream_json, &resolved_model);
                let latency = start.elapsed().as_millis() as i64;
                let (in_tok, out_tok) = crate::gateway::usage::extract_chat(&upstream_json);
                let trace = trace_with_degradation_events(
                    json!({"protocol": "gemini_input"}),
                    &chat_req.diagnostic_events,
                );
                log_request_success(
                    &state.db,
                    ctx.client_type,
                    "/v1beta/generateContent",
                    ctx.request_id,
                    ctx.raw_body,
                    &converted_json,
                    &serde_json::to_string(&upstream_json).unwrap_or_default(),
                    &serde_json::to_string(&gemini_resp).unwrap_or_default(),
                    None,
                    &config.name,
                    &resolved_model,
                    200,
                    latency,
                    Some(&trace),
                    crate::gateway::usage::TokenUsage {
                        input: in_tok,
                        output: out_tok,
                        cache_write: None,
                        cache_read: None,
                        input_semantics:
                            crate::gateway::usage::InputCacheSemantics::IncludesCacheRead,
                    },
                );
                Attempt::Success(Json(gemini_resp).into_response())
            }
            Err(err) => {
                log_upstream_error(&converted_json, &resolved_model, &err);
                Attempt::ProviderFailed(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use serde_json::{json, Value};
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::models::provider::Provider;
    use crate::storage;
    use crate::storage::db::DbPool;

    /// 鉴权已挪到 server.rs 的中间件(读 body 之前),handler 本身不再读 token
    /// 文件——单测直接调 handler 不需要 token,也就不用抢 FS_LOCK / 改 HOME。
    fn no_auth_headers() -> axum::http::HeaderMap {
        axum::http::HeaderMap::new()
    }

    fn db_pool() -> DbPool {
        let manager = r2d2_sqlite::SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(manager).unwrap();
        let conn = pool.get().unwrap();
        storage::migrations::run_migrations(&conn).unwrap();
        // Clear seeded defaults so each test starts from a known empty state.
        conn.execute("DELETE FROM route_profile_providers", [])
            .unwrap();
        conn.execute("DELETE FROM route_profiles", []).unwrap();
        conn.execute("DELETE FROM providers", []).unwrap();
        conn.execute("UPDATE gateway_settings SET active_provider_id = NULL", [])
            .unwrap();
        pool
    }

    fn gateway_state(db: DbPool) -> GatewayState {
        GatewayState {
            db,
            http_client: reqwest::Client::new(),
            active_requests: Arc::new(AtomicU64::new(0)),
            request_body_limit: 32 * 1024 * 1024,
        }
    }

    /// 干净的内存库 GatewayState。之前这里持有 FS_LOCK 的 std MutexGuard 跨
    /// await(clippy await_holding_lock),handler 不再读 token 后已无需加锁。
    fn setup_test() -> GatewayState {
        gateway_state(db_pool())
    }

    fn create_provider(
        conn: &rusqlite::Connection,
        id: &str,
        name: &str,
        provider_type: &str,
        base_url: &str,
        protocol: &str,
        default_model: &str,
        model_mapping: Option<&str>,
    ) -> Provider {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO providers (
                id, name, provider_type, base_url, api_key, default_model,
                protocol, timeout_seconds, status, enabled, is_active,
                created_at, updated_at, model_mapping
             ) VALUES (?1, ?2, ?3, ?4, 'sk-test', ?5, ?6, 120, 'ok', 1, 1, ?7, ?7, ?8)",
            rusqlite::params![
                id,
                name,
                provider_type,
                base_url,
                default_model,
                protocol,
                now,
                model_mapping
            ],
        )
        .unwrap();
        storage::providers::get_by_id(conn, id).unwrap()
    }

    fn create_route_profile(conn: &rusqlite::Connection, id: &str, input_protocol: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO route_profiles (id, name, client_type, input_protocol, mode, enabled, is_default, created_at, updated_at)
             VALUES (?1, ?2, '', ?3, 'manual', 1, 1, ?4, ?4)",
            rusqlite::params![id, id, input_protocol, now],
        )
        .unwrap();
    }

    fn link_provider_to_profile(
        conn: &rusqlite::Connection,
        profile_id: &str,
        provider_id: &str,
        priority: i64,
        model_override: Option<&str>,
    ) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO route_profile_providers (
                id, route_profile_id, provider_id, priority, enabled,
                model_override, cooldown_seconds, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, 600, ?6, ?6)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                profile_id,
                provider_id,
                priority,
                model_override,
                now
            ],
        )
        .unwrap();
    }

    async fn body_to_json(resp: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn body_to_string(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn count_tokens_estimates_from_text_parts() {
        let state = setup_test();
        let body = json!({
            "contents": [
                {"role": "user", "parts": [{"text": "hello"}, {"text": "世界"}]},
                {"role": "model", "parts": [{"text": "!"}]}
            ]
        })
        .to_string();

        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:countTokens".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_to_json(resp).await;
        // hello(5) + 世界(2) + !(1) = 8 chars; ceil(8/4) = 2
        assert_eq!(v["totalTokens"], 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn count_tokens_empty_contents_returns_zero() {
        let state = setup_test();
        let body = json!({"contents": []}).to_string();

        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:countTokens".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_to_json(resp).await;
        assert_eq!(v["totalTokens"], 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn count_tokens_invalid_json_returns_parse_error() {
        let state = setup_test();

        let err = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:countTokens".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from("not json")),
        )
        .await
        .unwrap_err();

        assert_eq!(err.0.code, crate::errors::codes::COUNT_TOKENS_PARSE_ERROR);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_extracts_model_name_and_stream_flag() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/v1beta/models/[^/]+:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"parts": [{"text": "ok"}]}}]
            })))
            .mount(&server)
            .await;

        let state = setup_test();
        let conn = state.db.get().unwrap();
        create_provider(
            &conn,
            "p1",
            "Gemini",
            "google_gemini",
            &server.uri(),
            "openai_responses",
            "gemini-2.5-flash",
            None,
        );
        create_route_profile(&conn, "rp1", "openai_responses");
        link_provider_to_profile(&conn, "rp1", "p1", 1, None);
        drop(conn);

        let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string();
        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:generateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_to_json(resp).await;
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "ok");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert!(received[0]
            .url
            .path()
            .contains("/v1beta/models/gemini-2.5-flash:generateContent"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_stream_uses_stream_generate_content() {
        let server = MockServer::start().await;
        let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"streamed\"}]}}]}\n\ndata: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path_regex("/v1beta/models/[^/]+:streamGenerateContent"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse.as_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let state = setup_test();
        let conn = state.db.get().unwrap();
        create_provider(
            &conn,
            "p1",
            "Gemini",
            "google_gemini",
            &server.uri(),
            "openai_responses",
            "gemini-2.5-flash",
            None,
        );
        create_route_profile(&conn, "rp1", "openai_responses");
        link_provider_to_profile(&conn, "rp1", "p1", 1, None);
        drop(conn);

        let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string();
        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:streamGenerateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let text = body_to_string(resp).await;
        assert!(text.contains("streamed"));

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert!(received[0].url.path().contains(":streamGenerateContent"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_gemini_native_passthrough_non_stream() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/v1beta/models/[^/]+:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"parts": [{"text": "native"}]}}]
            })))
            .mount(&server)
            .await;

        let state = setup_test();
        let conn = state.db.get().unwrap();
        create_provider(
            &conn,
            "p1",
            "Gemini",
            "google_gemini",
            &server.uri(),
            "openai_responses",
            "gemini-2.5-flash",
            None,
        );
        create_route_profile(&conn, "rp1", "openai_responses");
        link_provider_to_profile(&conn, "rp1", "p1", 1, None);
        drop(conn);

        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            "safetySettings": [{"category": "HARM_CATEGORY_SEXUAL", "threshold": "BLOCK_NONE"}]
        })
        .to_string();
        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:generateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_to_json(resp).await;
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "native");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let upstream_body: Value = serde_json::from_slice(&received[0].body).unwrap();
        // Gemini-only field should be preserved in native passthrough.
        assert!(upstream_body.get("safetySettings").is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_openai_chat_conversion_non_stream() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-mock",
                "model": "gpt-4",
                "choices": [{"message": {"role": "assistant", "content": "chat"}}],
                "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6}
            })))
            .mount(&server)
            .await;

        let state = setup_test();
        let conn = state.db.get().unwrap();
        create_provider(
            &conn,
            "p1",
            "OpenAI",
            "openai",
            &server.uri(),
            "openai_chat_completions",
            "gpt-4",
            None,
        );
        create_route_profile(&conn, "rp1", "openai_responses");
        link_provider_to_profile(&conn, "rp1", "p1", 1, None);
        drop(conn);

        let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string();
        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:generateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_to_json(resp).await;
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "chat");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let upstream_body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(upstream_body["model"], "gpt-4");
        assert_eq!(upstream_body["messages"][0]["role"], "user");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_openai_chat_conversion_stream() {
        let server = MockServer::start().await;
        let sse = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({"id":"c1","choices":[{"index":0,"delta":{"role":"assistant","content":"chat"}}]}),
            json!({"id":"c1","choices":[{"index":0,"delta":{"content":" stream"}}]})
        );
        Mock::given(method("POST"))
            .and(path_regex("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let state = setup_test();
        let conn = state.db.get().unwrap();
        create_provider(
            &conn,
            "p1",
            "OpenAI",
            "openai",
            &server.uri(),
            "openai_chat_completions",
            "gpt-4",
            None,
        );
        create_route_profile(&conn, "rp1", "openai_responses");
        link_provider_to_profile(&conn, "rp1", "p1", 1, None);
        drop(conn);

        let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string();
        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:streamGenerateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let text = body_to_string(resp).await;
        assert!(
            text.contains("chat") && text.contains(" stream"),
            "stream text missing: {}",
            text
        );

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let upstream_body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(upstream_body["stream"], true);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_falls_back_to_chat_completions_profile() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-mock",
                "model": "gpt-4",
                "choices": [{"message": {"role": "assistant", "content": "fallback"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })))
            .mount(&server)
            .await;

        let state = setup_test();
        let conn = state.db.get().unwrap();
        create_provider(
            &conn,
            "p1",
            "OpenAI",
            "openai",
            &server.uri(),
            "openai_chat_completions",
            "gpt-4",
            None,
        );
        // Only a chat_completions profile exists; openai_responses profile is missing.
        create_route_profile(&conn, "rp1", "openai_chat_completions");
        link_provider_to_profile(&conn, "rp1", "p1", 1, None);
        drop(conn);

        let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string();
        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:generateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_to_json(resp).await;
        assert_eq!(
            v["candidates"][0]["content"]["parts"][0]["text"],
            "fallback"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_model_mapping_override() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/v1beta/models/mapped-gemini:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"parts": [{"text": "mapped"}]}}]
            })))
            .mount(&server)
            .await;

        let state = setup_test();
        let conn = state.db.get().unwrap();
        create_provider(
            &conn,
            "p1",
            "Gemini",
            "google_gemini",
            &server.uri(),
            "openai_responses",
            "gemini-default",
            Some(r#"{"gemini-2.5-flash": "mapped-gemini"}"#),
        );
        create_route_profile(&conn, "rp1", "openai_responses");
        link_provider_to_profile(&conn, "rp1", "p1", 1, None);
        drop(conn);

        let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string();
        let resp = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:generateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_or_else(|e| panic!("handler failed: {} - {}", e.0.code, e.0.message))
        .into_response();

        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_to_json(resp).await;
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "mapped");

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert!(received[0]
            .url
            .path()
            .contains("/v1beta/models/mapped-gemini:generateContent"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_invalid_json_returns_gemini_parse_error() {
        let state = setup_test();

        let err = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:generateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from("not json")),
        )
        .await
        .unwrap_err();

        assert_eq!(err.0.code, crate::errors::codes::GEMINI_PARSE_ERROR);
        let response = err.into_response();
        // GEMINI_PARSE_ERROR is not in the IntoResponse BAD_REQUEST list.
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_content_no_provider_returns_error() {
        let state = setup_test();
        let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string();

        let err = handle_gemini_generate(
            no_auth_headers(),
            axum::extract::Path("gemini-2.5-flash:generateContent".to_string()),
            axum::extract::State(state),
            Ok(bytes::Bytes::from(body)),
        )
        .await
        .unwrap_err();

        assert_eq!(err.0.code, crate::errors::codes::ACTIVE_PROVIDER_NOT_FOUND);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
