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

#[cfg(test)]
use super::shared::native_model_override;
use super::shared::{
    check_budget, detect_client_from_ua, log_request_error, log_request_error_full,
    log_request_success, native_model_override_for_images, refine_value_body,
    request_body_or_gateway_error, sanitize_body, select_providers, trace_with_degradation_events,
    truncate_str, GatewayError,
};
use super::GatewayState;

// ── POST /v1/chat/completions ──────────────────────────────────

pub async fn handle_chat_completions(
    headers: HeaderMap,
    AxumState(state): AxumState<GatewayState>,
    body: Result<bytes::Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Response, GatewayError> {
    // 鉴权 + Host/Origin 边界校验在 server.rs 的中间件里、读 body 之前完成。
    let body = request_body_or_gateway_error(body)?;
    // Daily budget hard gate: block new requests or force cheapest (streams mid-flight not cut).
    let force_cheapest = check_budget(&state.db).await?;
    let start = Instant::now();
    let request_id = format!(
        "req_{}",
        &uuid::Uuid::new_v4().to_string().replace('-', "")[..12]
    );
    let client_type = detect_client_from_ua(&headers, "Generic");

    let body = crate::gateway::body_decode::decode(&headers, body, state.request_body_limit)
        .map_err(|e| {
            log_request_error(
                &state.db,
                &client_type,
                "/v1/chat/completions",
                &request_id,
                "",
                None,
                &e,
                start.elapsed().as_millis() as i64,
            );
            GatewayError(e)
        })?;

    let body_json: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({}));
    let requested_model = body_json
        .get("model")
        .and_then(|m| m.as_str())
        .map(str::to_string);
    let analysis = crate::gateway::provider_selector::analyze_chat_value(&body_json);

    // Provider 选取：先按 openai_chat_completions 路由 profile 选；选不到再
    // fallback 到 anthropic_messages —— 让只配了 Anthropic 端点的 provider
    // 也能服务 Chat 客户端，下面 anthropic 分支负责协议转换。
    let selection = select_providers(
        &state.db,
        &["openai_chat_completions", "anthropic_messages"],
        requested_model.clone(),
        analysis.clone(),
        force_cheapest,
    )
    .await
    .map_err(|e| {
        log_request_error(
            &state.db,
            &client_type,
            "/v1/chat/completions",
            &request_id,
            &sanitize_body(&body),
            None,
            &e,
            start.elapsed().as_millis() as i64,
        );
        GatewayError(e)
    })?;

    let is_failover = selection.mode == "failover" && selection.candidates.len() > 1;
    let raw_body = sanitize_body(&body);

    // 会话亲和：与 /v1/responses 对齐，cache-hit 后粘住同一上游以保 prompt cache。
    // force_cheapest 时禁用亲和重排——否则会把贵上游提到队首，抵消预算闸。
    let session_id = crate::gateway::session_affinity::derive_from_chat_body(&body_json);
    let affinity_sid = if force_cheapest {
        None
    } else {
        session_id.as_deref()
    };

    // 带图请求跳过显式不支持 vision 的 provider(与 /v1/responses 对齐)。
    let request_has_images = analysis.has_images;
    let attempt_order = crate::gateway::failover::build_attempt_order(
        &selection.candidates,
        &selection.provider.id,
        is_failover,
        request_has_images,
        affinity_sid,
    );
    let providers = crate::gateway::failover::load_providers(&state.db, &attempt_order)
        .await
        .map_err(GatewayError)?;

    let ctx = ChatAttemptCtx {
        state: &state,
        headers: &headers,
        body: &body,
        raw_body: &raw_body,
        requested_model: requested_model.as_deref(),
        request_has_images,
        request_id: &request_id,
        start,
        client_type: &client_type,
        session_id: session_id.as_deref(),
    };
    let providers = &providers;
    crate::gateway::failover::run_attempts(
        &state.db,
        &attempt_order,
        is_failover,
        |_, candidate| async move {
            match providers.get(&candidate.provider_id) {
                Some(provider) => attempt_chat_provider(ctx, provider, &candidate).await,
                None => Attempt::Skip(AppError::not_found("Provider", &candidate.provider_id)),
            }
        },
    )
    .await
    .map_err(GatewayError)
}

/// 一次 chat 请求在所有候选间共享的只读上下文。
#[derive(Clone, Copy)]
struct ChatAttemptCtx<'a> {
    state: &'a GatewayState,
    headers: &'a HeaderMap,
    body: &'a str,
    raw_body: &'a str,
    requested_model: Option<&'a str>,
    request_has_images: bool,
    request_id: &'a str,
    start: Instant,
    client_type: &'a str,
    session_id: Option<&'a str>,
}

/// 对单个候选发起一次 chat 请求,结果交给 failover 驱动决定是否换下一个。
async fn attempt_chat_provider(
    ctx: ChatAttemptCtx<'_>,
    provider: &Provider,
    candidate: &ProviderCandidate,
) -> Attempt<Response> {
    let config = match ProviderConfig::from_provider(provider) {
        Ok(c) => c,
        Err(e) => return Attempt::Skip(e),
    };

    // Chat → Anthropic 转换分支：provider 是 anthropic 且配了 anthropic_base_url。
    // 走 client_chat_to_anthropic_handle 转换请求体、调上游 Anthropic、再把响应/SSE
    // 翻译成 Chat 形态发回。
    if config.is_anthropic() && config.has_anthropic_url() {
        let model_override = native_model_override_for_images(
            provider,
            ctx.requested_model,
            Some(&candidate.model),
            ctx.request_has_images,
        );
        let model = model_override.unwrap_or_else(|| candidate.model.clone());
        return match client_chat_to_anthropic_handle(
            ctx.state.clone(),
            config,
            provider.clone(),
            ctx.body,
            model,
            ctx.request_id.to_string(),
            ctx.raw_body.to_string(),
            ctx.start,
            ctx.client_type.to_string(),
            ctx.session_id.map(str::to_string),
            candidate.provider_id.clone(),
        )
        .await
        {
            Ok(response) => Attempt::Success(response),
            Err(GatewayError(err)) => match crate::gateway::failover::classify_error(err) {
                // 转换分支里不带 "HTTP nnn" 的上游错误(响应解析失败、Copilot token
                // 交换失败)按 502 参与 failover 判断,与改造前一致。
                Attempt::ProviderFailed(err)
                    if crate::gateway::failover::upstream_status_of(&err).is_none() =>
                {
                    Attempt::ProviderFailed(err.with_upstream_status(502))
                }
                other => other,
            },
        };
    }

    let decision = match crate::gateway::route_decision::decide(
        "/v1/chat/completions",
        &provider.protocol,
        &config.base_url,
    ) {
        Ok(d) => d,
        Err(e) => return Attempt::Skip(e),
    };

    if decision.mode != crate::gateway::route_decision::RouteMode::PassThrough {
        return Attempt::Skip(AppError::new(
            crate::errors::codes::PROTOCOL_TRANSFORM_NOT_SUPPORTED,
            "Not a pass-through provider",
        ));
    }

    let model_override = native_model_override_for_images(
        provider,
        ctx.requested_model,
        Some(&candidate.model),
        ctx.request_has_images,
    );
    match crate::gateway::pass_through::handle(
        &ctx.state.http_client,
        &ctx.state.db,
        &config,
        &decision.target_url,
        "/v1/chat/completions",
        "openai_chat_completions",
        ctx.body,
        model_override.as_deref(),
        ctx.request_id,
        ctx.start,
        ctx.client_type,
        Some(ctx.headers),
        ctx.session_id,
        Some(candidate.provider_id.as_str()),
    )
    .await
    {
        Ok(response) => Attempt::Success(response),
        Err(err) => Attempt::ProviderFailed(err),
    }
}

// ── Chat 客户端 + Anthropic provider 协议转换 ──────────────────
//
// Client 发 /v1/chat/completions 但 provider 只有 anthropic_base_url（没 Chat
// 端点）时走这条路径：
// 1. Chat 请求体 → Anthropic Messages 请求体（`chat_to_anthropic::convert`）
// 2. send_anthropic_stream / non_stream 调上游 Anthropic
// 3. 上游 Anthropic 响应 → Chat 响应：
//    - 非流式：`anthropic_to_chat::convert_with_events` 一次性转（丢弃的块记降级事件）
//    - 流式：`AnthropicToChatStream` 增量转译 SSE 帧
async fn client_chat_to_anthropic_handle(
    state: GatewayState,
    config: ProviderConfig,
    provider: Provider,
    body: &str,
    model: String,
    request_id: String,
    raw_request: String,
    start: Instant,
    client_type: String,
    session_id: Option<String>,
    provider_id: String,
) -> Result<Response, GatewayError> {
    use crate::protocol::chat_completions::ChatCompletionsRequest;

    // 1. 解析 Chat 请求
    let mut chat_req: ChatCompletionsRequest = serde_json::from_str(body).map_err(|e| {
        let err = AppError::new(
            crate::errors::codes::CHAT_PARSE_ERROR,
            format!("Failed to parse chat request: {e}"),
        );
        log_request_error(
            &state.db,
            &client_type,
            "/v1/chat/completions",
            &request_id,
            &raw_request,
            None,
            &err,
            start.elapsed().as_millis() as i64,
        );
        GatewayError(err)
    })?;
    // model_mapping 覆盖
    chat_req.model = model.clone();

    let want_stream = chat_req.stream;

    // 2. Chat → Anthropic 转换
    let mut anthropic_body =
        crate::transform::chat_to_anthropic::convert(&chat_req).map_err(|e| {
            log_request_error(
                &state.db,
                &client_type,
                "/v1/chat/completions",
                &request_id,
                &raw_request,
                None,
                &e,
                start.elapsed().as_millis() as i64,
            );
            GatewayError(e)
        })?;
    // 3. 网关精炼层：开关全关时是 no-op，开了才会按 quirks 改写 outbound body
    let _refiner_log = refine_value_body(&state.db, &provider, &mut anthropic_body);
    let converted_request = serde_json::to_string(&anthropic_body).unwrap_or_default();

    if want_stream {
        return client_chat_to_anthropic_stream(
            state,
            config,
            anthropic_body,
            model,
            request_id,
            raw_request,
            converted_request,
            start,
            client_type,
            session_id,
            provider_id,
        )
        .await;
    }

    // 3. 非流式：发 Anthropic non-stream 请求
    let result =
        adapter::send_anthropic_non_stream(&state.http_client, &config, &anthropic_body).await;
    match result {
        Ok(upstream_json) => {
            let (chat_resp, degradation_events) =
                crate::transform::anthropic_to_chat::convert_with_events(&upstream_json, &model);
            let latency = start.elapsed().as_millis() as i64;
            // usage 同时含 Anthropic + OpenAI 两形态字段，extract_cache_tokens 都识别
            let (in_tok, out_tok) = (
                chat_resp
                    .get("usage")
                    .and_then(|u| u.get("prompt_tokens"))
                    .and_then(|v| v.as_i64()),
                chat_resp
                    .get("usage")
                    .and_then(|u| u.get("completion_tokens"))
                    .and_then(|v| v.as_i64()),
            );
            let (cache_w, cache_r) = chat_resp
                .get("usage")
                .map(crate::storage::request_logs::extract_cache_tokens)
                .unwrap_or((None, None));
            if let Some(ref sid) = session_id {
                if let Some(usage) = chat_resp.get("usage") {
                    crate::gateway::session_affinity::record_if_cache_hit(sid, &provider_id, usage);
                }
            }
            let trace = trace_with_degradation_events(
                json!({"mode": "transform", "protocol": "chat_to_anthropic", "stream": false}),
                &degradation_events,
            );
            log_request_success(
                &state.db,
                &client_type,
                "/v1/chat/completions",
                &request_id,
                &raw_request,
                &converted_request,
                &serde_json::to_string(&upstream_json).unwrap_or_default(),
                &serde_json::to_string(&chat_resp).unwrap_or_default(),
                None,
                &config.name,
                &model,
                200,
                latency,
                Some(&trace),
                crate::gateway::usage::TokenUsage {
                    input: in_tok,
                    output: out_tok,
                    cache_write: cache_w,
                    cache_read: cache_r,
                    input_semantics: crate::gateway::usage::InputCacheSemantics::ExcludesCache,
                },
            );
            Ok(Json(chat_resp).into_response())
        }
        Err(err) => {
            let latency = start.elapsed().as_millis() as i64;
            log_request_error_full(
                &state.db,
                &client_type,
                "/v1/chat/completions",
                &request_id,
                &raw_request,
                &converted_request,
                &config.name,
                &model,
                &err,
                502,
                latency,
            );
            Err(GatewayError(err))
        }
    }
}

/// 流式：上游 Anthropic SSE → 增量翻译 → 给客户端发 Chat SSE。
/// 与 handle_anthropic_fallback_stream 镜像对称。
async fn client_chat_to_anthropic_stream(
    state: GatewayState,
    config: ProviderConfig,
    anthropic_body: Value,
    model: String,
    request_id: String,
    raw_request: String,
    converted_request: String,
    start: Instant,
    client_type: String,
    session_id: Option<String>,
    provider_id: String,
) -> Result<Response, GatewayError> {
    use futures::StreamExt;

    let upstream = adapter::send_anthropic_stream(&state.http_client, &config, &anthropic_body)
        .await
        .map_err(|e| {
            let latency = start.elapsed().as_millis() as i64;
            log_request_error_full(
                &state.db,
                &client_type,
                "/v1/chat/completions",
                &request_id,
                &raw_request,
                &converted_request,
                &config.name,
                &model,
                &e,
                502,
                latency,
            );
            GatewayError(e)
        })?;

    // Bootstrap 校验：HTTP 200 + 错误帧的情况能被识别并报错触发 failover
    let boot = crate::gateway::sse_bootstrap::bootstrap_detect(upstream)
        .await
        .map_err(|e| {
            let latency = start.elapsed().as_millis() as i64;
            log_request_error_full(
                &state.db,
                &client_type,
                "/v1/chat/completions",
                &request_id,
                &raw_request,
                &converted_request,
                &config.name,
                &model,
                &e,
                502,
                latency,
            );
            GatewayError(e)
        })?;

    let (tx, rx) = mpsc::channel::<String>(512);
    let db = state.db.clone();
    let provider_name = config.name.clone();
    let model_clone = model.clone();
    let req_id = request_id.clone();
    let raw_req = raw_request.clone();
    let conv_req = converted_request.clone();
    let client_type_owned = client_type.clone();
    let session_id_owned = session_id;
    let provider_id_owned = provider_id;

    tokio::spawn(async move {
        use crate::transform::anthropic_to_chat_stream::AnthropicToChatStream;

        let mut converter = AnthropicToChatStream::new(model_clone.clone());
        let mut utf8_pending: Vec<u8> = Vec::new();
        let mut buffer = String::new();
        crate::gateway::stream_utf8::append_utf8_safe(&mut buffer, &mut utf8_pending, &boot.prefix);
        buffer = buffer.replace("\r\n", "\n");
        let mut stream = boot.stream;
        let mut bootstrap_replayed = false;
        let mut total_text = String::new();
        let mut final_usage_json: Option<Value> = None;
        let mut stream_err: Option<String> = None;

        loop {
            if bootstrap_replayed {
                let chunk = match stream.next().await {
                    Some(Ok(b)) => b,
                    Some(Err(e)) => {
                        stream_err = Some(crate::gateway::sse_bootstrap::describe_stream_error(&e));
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

            for frame in crate::gateway::sse_buffer::take_complete_frames(&mut buffer) {
                let mut event_type = String::new();
                let mut data_str = String::new();
                for line in frame.lines() {
                    if let Some(et) = line.strip_prefix("event:").map(|s| s.trim()) {
                        event_type = et.to_string();
                    } else if let Some(d) = line.strip_prefix("data:").map(|s| s.trim()) {
                        if !data_str.is_empty() {
                            data_str.push('\n');
                        }
                        data_str.push_str(d);
                    }
                }

                if event_type.is_empty() || data_str.is_empty() {
                    continue;
                }

                let data: Value = match serde_json::from_str(&data_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // 累计文本用于 log 日志摘要
                if event_type == "content_block_delta" {
                    if let Some(delta) = data.get("delta") {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            total_text.push_str(text);
                        }
                    }
                }
                // 收尾 usage：message_delta 携带 output_tokens；message_start 携带 input_tokens
                if event_type == "message_delta" {
                    if let Some(u) = data.get("usage") {
                        final_usage_json.get_or_insert_with(|| json!({}));
                        if let Some(obj) = final_usage_json.as_mut().and_then(|u| u.as_object_mut())
                        {
                            if let Some(ot) = u.get("output_tokens") {
                                obj.insert("output_tokens".into(), ot.clone());
                            }
                        }
                    }
                }
                if event_type == "message_start" {
                    if let Some(u) = data.get("message").and_then(|m| m.get("usage")) {
                        final_usage_json.get_or_insert_with(|| json!({}));
                        if let Some(obj) = final_usage_json.as_mut().and_then(|u| u.as_object_mut())
                        {
                            if let Some(it) = u.get("input_tokens") {
                                obj.insert("input_tokens".into(), it.clone());
                            }
                            if let Some(c) = u.get("cache_read_input_tokens") {
                                obj.insert("cache_read_input_tokens".into(), c.clone());
                            }
                            if let Some(c) = u.get("cache_creation_input_tokens") {
                                obj.insert("cache_creation_input_tokens".into(), c.clone());
                            }
                        }
                    }
                }

                let chat_chunks = converter.process_event(&event_type, &data);
                for chunk in chat_chunks {
                    if tx.send(chunk).await.is_err() {
                        // client 断开
                        return;
                    }
                }
            }
        }

        // 收尾
        let final_chunks = converter.finalize();
        for chunk in final_chunks {
            if tx.send(chunk).await.is_err() {
                return;
            }
        }

        let latency = start.elapsed().as_millis() as i64;
        match stream_err {
            None => {
                let trace = json!({
                    "mode": "transform", "protocol": "chat_to_anthropic", "stream": true,
                    "text_len": total_text.len(),
                })
                .to_string();
                let (in_tok, out_tok) = final_usage_json
                    .as_ref()
                    .map(|u| {
                        (
                            u.get("input_tokens").and_then(|v| v.as_i64()),
                            u.get("output_tokens").and_then(|v| v.as_i64()),
                        )
                    })
                    .unwrap_or((None, None));
                let (cache_w, cache_r) = final_usage_json
                    .as_ref()
                    .map(crate::storage::request_logs::extract_cache_tokens)
                    .unwrap_or((None, None));
                if let (Some(sid), Some(usage)) =
                    (session_id_owned.as_deref(), final_usage_json.as_ref())
                {
                    crate::gateway::session_affinity::record_if_cache_hit(
                        sid,
                        &provider_id_owned,
                        usage,
                    );
                }
                log_request_success(
                    &db,
                    &client_type_owned,
                    "/v1/chat/completions",
                    &req_id,
                    &raw_req,
                    &conv_req,
                    "",
                    &truncate_str(&total_text, 10000),
                    None,
                    &provider_name,
                    &model_clone,
                    200,
                    latency,
                    Some(&trace),
                    crate::gateway::usage::TokenUsage {
                        input: in_tok,
                        output: out_tok,
                        cache_write: cache_w,
                        cache_read: cache_r,
                        input_semantics: crate::gateway::usage::InputCacheSemantics::ExcludesCache,
                    },
                );
            }
            Some(msg) => {
                let err = AppError::new(crate::errors::codes::UPSTREAM_STREAM_ERROR, &msg);
                log_request_error_full(
                    &db,
                    &client_type_owned,
                    "/v1/chat/completions",
                    &req_id,
                    &raw_req,
                    &conv_req,
                    &provider_name,
                    &model_clone,
                    &err,
                    502,
                    latency,
                );
            }
        }
    });

    let stream = ReceiverStream::new(rx);
    let body = Body::from_stream(tokio_stream::StreamExt::map(stream, |s| {
        Ok::<_, std::convert::Infallible>(s)
    }));

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(body)
        .unwrap())
}

#[cfg(test)]
mod tests {
    use crate::errors::codes;
    use crate::gateway::provider_selector::{should_failover, ProviderCandidate};
    use crate::gateway::routes::GatewayState;
    use crate::models::provider::{CreateProviderInput, Provider};
    use crate::models::route_profile::{
        AddProviderToRouteInput, CreateRouteProfileInput, RouteProfile,
    };
    use crate::providers::adapter::ProviderConfig;
    use crate::storage::{self, providers as providers_storage, route_profiles};
    use axum::http::{HeaderMap, HeaderName};
    use bytes::Bytes;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use std::time::Instant;

    fn in_memory_pool() -> crate::storage::db::DbPool {
        let manager = r2d2_sqlite::SqliteConnectionManager::memory();
        r2d2::Pool::builder().max_size(1).build(manager).unwrap()
    }

    fn gateway_state(pool: crate::storage::db::DbPool) -> GatewayState {
        GatewayState {
            db: pool,
            http_client: reqwest::Client::new(),
            active_requests: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            request_body_limit: 32 * 1024 * 1024,
        }
    }

    fn create_test_provider(
        conn: &rusqlite::Connection,
        name: &str,
        provider_type: &str,
        base_url: &str,
        api_key: &str,
        protocol: &str,
        default_model: &str,
    ) -> Provider {
        providers_storage::create(
            conn,
            CreateProviderInput {
                name: name.to_string(),
                provider_type: provider_type.to_string(),
                base_url: base_url.to_string(),
                api_key: Some(api_key.to_string()),
                default_model: default_model.to_string(),
                reasoning_model: None,
                supported_models: None,
                model_mapping: None,
                extra_headers: None,
                anthropic_base_url: if provider_type == "anthropic" {
                    Some(base_url.to_string())
                } else {
                    None
                },
                responses_base_url: None,
                protocol: protocol.to_string(),
                timeout_seconds: Some(120),
                auto_cache_control: None,
                model_capabilities: None,
                provider_quirks: None,
                body_filter_enabled: None,
                thinking_rectifier_enabled: None,
                error_mapper_enabled: None,
                model_degradation_chain: None,
                model_context_windows: None,
                enabled: Some(true),
            },
        )
        .unwrap()
    }

    fn create_failover_profile(conn: &rusqlite::Connection, input_protocol: &str) -> RouteProfile {
        let profile = route_profiles::create(
            conn,
            CreateRouteProfileInput {
                name: format!("{} test", input_protocol),
                input_protocol: input_protocol.to_string(),
                mode: Some("failover".to_string()),
            },
        )
        .unwrap();
        route_profiles::set_default(conn, &profile.id).unwrap();
        profile
    }

    fn add_provider_input(priority: i64) -> AddProviderToRouteInput {
        AddProviderToRouteInput {
            priority: Some(priority),
            model_override: None,
            cooldown_seconds: None,
            failover_on_status_codes: None,
            failover_on_error_keywords: None,
            routing_conditions: None,
        }
    }

    fn chat_provider_for_override() -> Provider {
        Provider {
            id: "p-override".into(),
            name: "Override".into(),
            provider_type: "openai".into(),
            base_url: "https://api.openai.com".into(),
            api_key: Some("sk-test".into()),
            default_model: "gpt-4o-mini".into(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: r#"["openai_chat_completions"]"#.into(),
            timeout_seconds: 120,
            status: "ok".into(),
            supports_vision: None,
            auto_cache_control: None,
            supports_cache: None,
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            enabled: true,
            is_active: false,
            created_at: "2024-01-01".into(),
            updated_at: "2024-01-01".into(),
        }
    }

    fn anthropic_provider() -> Provider {
        Provider {
            id: "p-anthropic".into(),
            name: "Anthropic".into(),
            provider_type: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: Some("sk-ant-test".into()),
            default_model: "claude-3-5-sonnet-latest".into(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: Some("https://api.anthropic.com".into()),
            responses_base_url: None,
            protocol: r#"["anthropic_messages"]"#.into(),
            timeout_seconds: 120,
            status: "ok".into(),
            supports_vision: None,
            auto_cache_control: None,
            supports_cache: None,
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            enabled: true,
            is_active: false,
            created_at: "2024-01-01".into(),
            updated_at: "2024-01-01".into(),
        }
    }

    fn extract_requested_model(body: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(str::to_string))
    }

    fn hdrs(encoding: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(enc) = encoding {
            h.insert(
                HeaderName::from_static("content-encoding"),
                enc.parse().unwrap(),
            );
        }
        h
    }

    // ── native_model_override (chat-specific) ─────────────────────

    #[test]
    fn native_model_override_maps_agentgate_to_default() {
        let provider = chat_provider_for_override();
        assert_eq!(
            super::native_model_override(&provider, Some("agentgate"), None),
            Some("gpt-4o-mini".to_string())
        );
    }

    #[test]
    fn native_model_override_uses_candidate_model_for_agentgate() {
        let provider = chat_provider_for_override();
        assert_eq!(
            super::native_model_override(
                &provider,
                Some("agentgate"),
                Some("claude-3-5-sonnet-latest")
            ),
            Some("claude-3-5-sonnet-latest".to_string())
        );
    }

    #[test]
    fn native_model_override_maps_explicit_alias() {
        let mut provider = chat_provider_for_override();
        provider.model_mapping = Some(r#"{"gpt-4o":"custom-gpt-4o"}"#.into());
        assert_eq!(
            super::native_model_override(&provider, Some("gpt-4o"), None),
            Some("custom-gpt-4o".to_string())
        );
    }

    #[test]
    fn native_model_override_leaves_unmapped_real_model_alone() {
        let provider = chat_provider_for_override();
        assert_eq!(
            super::native_model_override(&provider, Some("gpt-4o"), None),
            None
        );
    }

    // ── body decode + model extraction ────────────────────────────

    #[test]
    fn body_decode_plain_and_extract_model() {
        let body = Bytes::from_static(br#"{"model":"gpt-4o","messages":[]}"#);
        let decoded = crate::gateway::body_decode::decode(&hdrs(None), body, 1024 * 1024).unwrap();
        assert_eq!(
            extract_requested_model(&decoded),
            Some("gpt-4o".to_string())
        );
    }

    #[test]
    fn body_decode_gzip_and_extract_model() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(br#"{"model":"gpt-4","messages":[]}"#)
            .unwrap();
        let compressed = Bytes::from(encoder.finish().unwrap());
        let decoded =
            crate::gateway::body_decode::decode(&hdrs(Some("gzip")), compressed, 1024 * 1024)
                .unwrap();
        assert_eq!(extract_requested_model(&decoded), Some("gpt-4".to_string()));
    }

    #[test]
    fn requested_model_none_for_malformed_json() {
        assert_eq!(extract_requested_model("not json"), None);
    }

    #[test]
    fn requested_model_none_when_model_missing() {
        assert_eq!(extract_requested_model(r#"{"messages":[]}"#), None);
    }

    // ── provider selection fallback ───────────────────────────────

    #[test]
    fn provider_selection_falls_back_from_openai_chat_to_anthropic_messages() {
        let pool = in_memory_pool();
        let conn = pool.get().unwrap();
        storage::migrations::run_migrations(&conn).unwrap();

        // Force the openai_chat_completions lookup to fail so the chat handler
        // would fall back to the anthropic_messages profile.
        conn.execute(
            "UPDATE route_profiles SET enabled = 0 WHERE input_protocol = 'openai_chat_completions'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE gateway_settings SET active_provider_id = NULL", [])
            .unwrap();

        let anthropic = create_test_provider(
            &conn,
            "Fallback Anthropic",
            "anthropic",
            "https://api.anthropic.com",
            "sk-ant-test",
            r#"["anthropic_messages"]"#,
            "claude-3-5-sonnet-latest",
        );
        let profile = create_failover_profile(&conn, "anthropic_messages");
        route_profiles::add_provider(&conn, &profile.id, &anthropic.id, add_provider_input(1))
            .unwrap();
        // Release the single in-memory connection before select_for_failover
        // tries to borrow another one from the pool.
        drop(conn);

        let first = crate::gateway::provider_selector::select_for_failover(
            &pool,
            "openai_chat_completions",
            Some("claude-3-5-sonnet-latest"),
            None,
        );
        assert!(first.is_err());

        let second = crate::gateway::provider_selector::select_for_failover(
            &pool,
            "anthropic_messages",
            Some("claude-3-5-sonnet-latest"),
            None,
        )
        .unwrap();
        assert_eq!(second.provider.id, anthropic.id);
        assert_eq!(second.provider.provider_type, "anthropic");
    }

    // ── failover / attempt loop decision logic ────────────────────

    #[test]
    fn failover_attempt_order_prefers_primary_then_backup() {
        let pool = in_memory_pool();
        let conn = pool.get().unwrap();
        storage::migrations::run_migrations(&conn).unwrap();

        let primary = create_test_provider(
            &conn,
            "Primary",
            "openai",
            "https://api.openai.com",
            "sk-primary",
            r#"["openai_chat_completions"]"#,
            "gpt-4o",
        );
        let backup = create_test_provider(
            &conn,
            "Backup",
            "openai",
            "https://api.openai.com",
            "sk-backup",
            r#"["openai_chat_completions"]"#,
            "gpt-4o-mini",
        );

        let profile = create_failover_profile(&conn, "openai_chat_completions");
        route_profiles::add_provider(&conn, &profile.id, &primary.id, add_provider_input(1))
            .unwrap();
        route_profiles::add_provider(&conn, &profile.id, &backup.id, add_provider_input(2))
            .unwrap();
        // Release the single in-memory connection before select_for_failover
        // tries to borrow another one from the pool.
        drop(conn);

        let selection = crate::gateway::provider_selector::select_for_failover(
            &pool,
            "openai_chat_completions",
            Some("gpt-4o"),
            None,
        )
        .unwrap();
        assert_eq!(selection.mode, "failover");
        assert_eq!(selection.candidates.len(), 2);

        let order = crate::gateway::failover::build_attempt_order(
            &selection.candidates,
            &selection.provider.id,
            true,
            false,
            None,
        );
        let ids: Vec<String> = order.iter().map(|c| c.provider_id.clone()).collect();
        assert_eq!(ids, vec![primary.id.clone(), backup.id.clone()]);
    }

    #[test]
    fn failover_attempt_order_skips_cooldown_backup() {
        let candidates = vec![
            ProviderCandidate {
                provider_id: "p1".into(),
                provider_name: "Primary".into(),
                priority: 1,
                model: "m1".into(),
                routing_conditions: None,
                in_cooldown: false,
                supports_vision: None,
                cooldown_seconds: 60,
                failover_on_status_codes: vec![],
                failover_on_error_keywords: vec![],
            },
            ProviderCandidate {
                provider_id: "p2".into(),
                provider_name: "Backup".into(),
                priority: 2,
                model: "m2".into(),
                routing_conditions: None,
                in_cooldown: true,
                supports_vision: None,
                cooldown_seconds: 60,
                failover_on_status_codes: vec![],
                failover_on_error_keywords: vec![],
            },
        ];

        let order =
            crate::gateway::failover::build_attempt_order(&candidates, "p1", true, false, None);
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].provider_id, "p1");
    }

    #[test]
    fn should_failover_uses_candidate_config() {
        let candidate = ProviderCandidate {
            provider_id: "p1".into(),
            provider_name: "Test".into(),
            priority: 1,
            model: "m".into(),
            routing_conditions: None,
            in_cooldown: false,
            supports_vision: None,
            cooldown_seconds: 60,
            failover_on_status_codes: vec![418],
            failover_on_error_keywords: vec!["quota".into()],
        };
        assert!(should_failover(Some(418), "ok", &candidate));
        assert!(!should_failover(Some(500), "ok", &candidate));
        assert!(should_failover(None, "insufficient quota", &candidate));
    }

    // ── Anthropic branch decision ─────────────────────────────────

    #[test]
    fn anthropic_branch_triggered_for_anthropic_provider_with_url() {
        let provider = anthropic_provider();
        let config = ProviderConfig::from_provider(&provider).unwrap();
        assert!(config.is_anthropic() && config.has_anthropic_url());
    }

    #[test]
    fn anthropic_branch_not_triggered_for_openai_provider() {
        let mut provider = anthropic_provider();
        provider.provider_type = "openai".into();
        provider.anthropic_base_url = None;
        let config = ProviderConfig::from_provider(&provider).unwrap();
        assert!(!(config.is_anthropic() && config.has_anthropic_url()));
    }

    // ── internal error mapping ────────────────────────────────────

    #[tokio::test]
    async fn malformed_chat_body_returns_chat_parse_error() {
        // log_request_success/error record Prometheus metrics; ensure a recorder
        // is installed so the test path does not panic.
        let _ = crate::gateway::metrics::init();

        let pool = in_memory_pool();
        let conn = pool.get().unwrap();
        storage::migrations::run_migrations(&conn).unwrap();

        let provider = create_test_provider(
            &conn,
            "Anthropic Error",
            "anthropic",
            "https://api.anthropic.com",
            "sk-ant-test",
            r#"["anthropic_messages"]"#,
            "claude-3-5-sonnet-latest",
        );
        let config = ProviderConfig::from_provider(&provider).unwrap();
        let state = gateway_state(pool);

        let err = super::client_chat_to_anthropic_handle(
            state,
            config,
            provider,
            "not valid json",
            "claude-3-5-sonnet-latest".into(),
            "req_test".into(),
            "not valid json".into(),
            Instant::now(),
            "test".into(),
            None,
            "provider_test".into(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0.code, codes::CHAT_PARSE_ERROR);
    }
}
