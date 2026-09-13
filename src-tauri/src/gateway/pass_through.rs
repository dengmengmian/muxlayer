use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use futures::StreamExt;
use rusqlite::Connection;
use serde_json::json;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::errors::AppError;
use crate::providers::adapter::ProviderConfig;

const MAX_LOG_BODY: usize = 50_000;
const MAX_SSE_LOG: usize = 1_000_000;

/// Anthropic 透传路径需要把客户端的几个 protocol-level header 透到上游，
/// 否则用户没法启用 context-1m / prompt-caching 这类 beta 能力。
/// 名单严格白名单：authorization / host / hop-by-hop / cookie 类**不**列入。
const ANTHROPIC_FORWARD_HEADERS: &[&str] = &["anthropic-beta", "anthropic-version"];

/// OpenAI 兼容透传路径转发的客户端 header 白名单。
const OPENAI_FORWARD_HEADERS: &[&str] = &["openai-beta", "openai-organization", "openai-project"];

// 注：pass_through 当前**不**转发上游响应 header（只显式设 Content-Type），
// 所以不需要 hop-by-hop 黑名单。未来若加 anthropic-ratelimit-* / x-request-id
// 等上游响应 header 转发，必须先过滤掉 RFC 7230 hop-by-hop 头：
// connection / keep-alive / proxy-authenticate / proxy-authorization / te /
// trailer / transfer-encoding / upgrade。

fn forward_client_headers(
    mut builder: reqwest::RequestBuilder,
    client_headers: Option<&HeaderMap>,
    whitelist: &[&str],
) -> reqwest::RequestBuilder {
    let Some(headers) = client_headers else {
        return builder;
    };
    for name in whitelist {
        if let Some(v) = headers.get(*name).and_then(|h| h.to_str().ok()) {
            if !v.is_empty() {
                builder = builder.header(*name, v);
            }
        }
    }
    builder
}

/// Handle a native upstream pass-through request (stream or non-stream).
///
/// `session_id` + `provider_id`：可选会话亲和上下文。上游 usage 报告
/// cache hit 时写入 affinity，供后续同会话 failover 排序粘住该上游。
#[allow(clippy::too_many_arguments)]
pub async fn handle(
    http_client: &reqwest::Client,
    db: &crate::storage::db::DbPool,
    config: &ProviderConfig,
    target_url: &str,
    route: &str,
    client_protocol: &str,
    raw_body: &str,
    model_override: Option<&str>,
    request_id: &str,
    start: Instant,
    client_type: &str,
    client_headers: Option<&HeaderMap>,
    session_id: Option<&str>,
    provider_id: Option<&str>,
) -> Result<Response, AppError> {
    let mut body_json: serde_json::Value =
        serde_json::from_str(raw_body).unwrap_or(serde_json::json!({}));

    let is_stream = body_json
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Native pass-through is transparent by default: preserve the request model.
    // Only rewrite when the caller supplies an explicit override (for example,
    // model_mapping). If the client omitted model entirely, fall back to default.
    let requested = body_json
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (model, model_resolution) =
        resolve_native_model(requested, model_override, &config.default_model);
    let trace_mode = native_trace_mode(model_override);
    body_json["model"] = serde_json::json!(&model);
    let rewritten_body = body_json.to_string();

    if is_stream {
        handle_stream(
            http_client,
            db,
            config,
            target_url,
            route,
            client_protocol,
            &rewritten_body,
            request_id,
            &model,
            trace_mode,
            model_resolution,
            start,
            client_type,
            client_headers,
            session_id,
            provider_id,
        )
        .await
    } else {
        handle_non_stream(
            http_client,
            db,
            config,
            target_url,
            route,
            client_protocol,
            &rewritten_body,
            request_id,
            &model,
            trace_mode,
            model_resolution,
            start,
            client_type,
            client_headers,
            session_id,
            provider_id,
        )
        .await
    }
}

fn resolve_native_model(
    requested: &str,
    model_override: Option<&str>,
    default_model: &str,
) -> (String, &'static str) {
    if let Some(mapped) = model_override {
        return (mapped.to_string(), "model_mapping");
    }
    if requested.is_empty() {
        return (default_model.to_string(), "default_model");
    }
    (requested.to_string(), "request_model")
}

fn native_trace_mode(model_override: Option<&str>) -> &'static str {
    if model_override.is_some() {
        "native_pass_through_model_mapping"
    } else {
        "native_pass_through"
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_non_stream(
    http_client: &reqwest::Client,
    db: &crate::storage::db::DbPool,
    config: &ProviderConfig,
    target_url: &str,
    route: &str,
    client_protocol: &str,
    raw_body: &str,
    request_id: &str,
    model: &str,
    trace_mode: &str,
    model_resolution: &str,
    start: Instant,
    client_type: &str,
    client_headers: Option<&HeaderMap>,
    session_id: Option<&str>,
    provider_id: Option<&str>,
) -> Result<Response, AppError> {
    let trace = json!({
        "mode": trace_mode,
        "client_protocol": client_protocol,
        "provider_protocol": client_protocol,
        "model_resolution": model_resolution,
        "route": route,
        "target_url": target_url,
    });
    let log = PassThroughLog {
        db,
        client_type,
        route,
        request_id,
        provider: &config.name,
        model,
        raw_request: &sanitize(raw_body, config.api_key()),
        start,
    };

    let sent = crate::providers::adapter::send_with_net_retry(
        || {
            let b = http_client
                .post(target_url)
                .header(
                    "Authorization",
                    format!("Bearer {}", config.select_api_key()),
                )
                .header("Content-Type", "application/json");
            forward_client_headers(b, client_headers, OPENAI_FORWARD_HEADERS)
                .body(raw_body.to_string())
        },
        1,
    )
    .await;
    let resp = match sent {
        Ok(r) => r,
        Err(e) => {
            let err = AppError::new(
                crate::errors::codes::PASS_THROUGH_REQUEST_FAILED,
                format!("Failed to connect to provider: {e}"),
            );
            return Err(log.error(err, &trace));
        }
    };

    let upstream_status = resp.status();
    let body_text = match crate::gateway::http_client::read_text_capped(resp).await {
        Ok(t) => t,
        Err(err) => return Err(log.error(err, &trace)),
    };
    let sanitized_response = sanitize(&body_text, config.api_key());
    let latency = start.elapsed().as_millis() as i64;

    let mut trace = trace;
    trace["upstream_status"] = json!(upstream_status.as_u16());
    let trace = trace.to_string();

    if !upstream_status.is_success() {
        let err = upstream_http_error(
            crate::errors::codes::UPSTREAM_NON_STREAM_ERROR,
            upstream_status,
            &sanitized_response,
            body_text,
        );
        log_to_db(
            db,
            client_type,
            route,
            request_id,
            &config.name,
            model,
            log.raw_request,
            &sanitized_response,
            Some(&truncate(&sanitized_response, 2000)),
            &trace,
            upstream_status.as_u16() as i64,
            latency,
            Default::default(),
        );
        return Err(err);
    }

    {
        if let (Some(sid), Some(pid)) = (session_id, provider_id) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body_text) {
                if let Some(usage) = v.get("usage") {
                    crate::gateway::session_affinity::record_if_cache_hit(sid, pid, usage);
                }
            }
        }
    }

    log_to_db(
        db,
        client_type,
        route,
        request_id,
        &config.name,
        model,
        log.raw_request,
        &sanitized_response,
        None,
        &trace,
        upstream_status.as_u16() as i64,
        latency,
        usage_from_json_body(
            &body_text,
            crate::gateway::usage::InputCacheSemantics::IncludesCacheRead,
        ),
    );

    Ok(Response::builder()
        .status(upstream_status.as_u16())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body_text))
        .unwrap())
}

/// 上游非 2xx:当作 provider 失败交给 route 的 failover 驱动(记熔断、按状态码决定
/// 是否切下一个)。原始状态码 + body 挂在错误上,最后一跳时原样回给客户端。
fn upstream_http_error(
    code: &str,
    status: reqwest::StatusCode,
    sanitized_body: &str,
    raw_body: String,
) -> AppError {
    AppError::new(code, format!("Provider returned HTTP {status}"))
        .with_detail(truncate(sanitized_body, 2000))
        .with_upstream_response(status.as_u16(), raw_body)
}

/// 直通请求在拿到上游正常响应前就失败(连不上、读 body 失败、bootstrap 错误帧)
/// 时的日志上下文。route 不再重复记这些错误,由这里统一落一条。
struct PassThroughLog<'a> {
    db: &'a crate::storage::db::DbPool,
    client_type: &'a str,
    route: &'a str,
    request_id: &'a str,
    provider: &'a str,
    model: &'a str,
    raw_request: &'a str,
    start: Instant,
}

impl PassThroughLog<'_> {
    /// 记一条错误日志,再把错误原样交还给调用方。
    fn error(&self, err: AppError, trace: &serde_json::Value) -> AppError {
        let mut trace = trace.clone();
        trace["error_code"] = json!(err.code);
        let status = crate::gateway::failover::upstream_status_of(&err).unwrap_or(502);
        log_to_db(
            self.db,
            self.client_type,
            self.route,
            self.request_id,
            self.provider,
            self.model,
            self.raw_request,
            "",
            Some(&format!(
                "{}: {}",
                err.message,
                err.detail.as_deref().unwrap_or("")
            )),
            &trace.to_string(),
            status as i64,
            self.start.elapsed().as_millis() as i64,
            Default::default(),
        );
        err
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_stream(
    http_client: &reqwest::Client,
    db: &crate::storage::db::DbPool,
    config: &ProviderConfig,
    target_url: &str,
    route: &str,
    client_protocol: &str,
    raw_body: &str,
    request_id: &str,
    model: &str,
    trace_mode: &str,
    model_resolution: &str,
    start: Instant,
    client_type: &str,
    client_headers: Option<&HeaderMap>,
    session_id: Option<&str>,
    provider_id: Option<&str>,
) -> Result<Response, AppError> {
    let trace = json!({
        "mode": trace_mode,
        "client_protocol": client_protocol,
        "provider_protocol": client_protocol,
        "model_resolution": model_resolution,
        "route": route,
        "target_url": target_url,
    });
    let log = PassThroughLog {
        db,
        client_type,
        route,
        request_id,
        provider: &config.name,
        model,
        raw_request: &sanitize(raw_body, config.api_key()),
        start,
    };
    let sent = crate::providers::adapter::send_with_net_retry(
        || {
            let b = http_client
                .post(target_url)
                .header(
                    "Authorization",
                    format!("Bearer {}", config.select_api_key()),
                )
                .header("Content-Type", "application/json")
                .header("Accept", "text/event-stream");
            forward_client_headers(b, client_headers, OPENAI_FORWARD_HEADERS)
                .body(raw_body.to_string())
        },
        1,
    )
    .await;
    let resp = match sent {
        Ok(r) => r,
        Err(e) => {
            let err = AppError::new(
                crate::errors::codes::PASS_THROUGH_STREAM_FAILED,
                format!("Failed to connect to provider: {e}"),
            );
            return Err(log.error(err, &trace));
        }
    };

    let upstream_status = resp.status();
    if !upstream_status.is_success() {
        let body_text = match crate::gateway::http_client::read_text_capped(resp).await {
            Ok(t) => t,
            Err(err) => return Err(log.error(err, &trace)),
        };
        let sanitized = sanitize(&body_text, config.api_key());
        let mut trace = trace;
        trace["upstream_status"] = json!(upstream_status.as_u16());
        log_to_db(
            db,
            client_type,
            route,
            request_id,
            &config.name,
            model,
            log.raw_request,
            "",
            Some(&truncate(&sanitized, 2000)),
            &trace.to_string(),
            upstream_status.as_u16() as i64,
            start.elapsed().as_millis() as i64,
            Default::default(),
        );
        return Err(upstream_http_error(
            crate::errors::codes::UPSTREAM_STREAM_ERROR,
            upstream_status,
            &sanitized,
            body_text,
        ));
    }

    // Bootstrap-validate the stream before committing to forwarding: catches
    // HTTP-200-with-error-frame failures (quota / rate-limit emitted mid-
    // stream by GLM / MiMo even on direct pass-through) and turns them into
    // a clean Err so the outer route loop can fail over.
    let boot = match crate::gateway::sse_bootstrap::bootstrap_detect(resp).await {
        Ok(b) => b,
        Err(err) => return Err(log.error(err, &trace)),
    };

    // Stream: pipe upstream SSE to client, log asynchronously
    let (tx, rx) = mpsc::channel::<String>(512);
    let db_clone = db.clone();
    let provider_name = config.name.clone();
    let model_clone = model.to_string();
    let req_id = request_id.to_string();
    let raw_req = sanitize(raw_body, config.api_key());
    let target = target_url.to_string();
    let route_owned = route.to_string();
    let client_protocol_owned = client_protocol.to_string();
    let trace_mode_owned = trace_mode.to_string();
    let model_resolution_owned = model_resolution.to_string();
    let api_key = config.api_key().to_string();
    let client_type_owned = client_type.to_string();
    let session_id_owned = session_id.map(str::to_string);
    let provider_id_owned = provider_id.map(str::to_string);

    tokio::spawn(async move {
        let mut utf8_pending: Vec<u8> = Vec::new();
        let mut prefix_text = String::new();
        crate::gateway::stream_utf8::append_utf8_safe(
            &mut prefix_text,
            &mut utf8_pending,
            &boot.prefix,
        );
        // 短响应可能整段都在 bootstrap 前缀里(含 usage 终块),usage 解析要带上前缀。
        let usage_tail_from_prefix = prefix_text.clone();
        let mut sse_log = String::new();
        let mut sse_size: usize = 0;

        // Replay the bootstrap prefix first so any bytes already pulled
        // during the scan reach the client.
        if !prefix_text.is_empty() {
            let slice =
                crate::gateway::stream_utf8::truncate_at_char_boundary(&prefix_text, MAX_SSE_LOG);
            sse_log.push_str(slice);
            sse_size += slice.len();
            if tx.send(prefix_text).await.is_err() {
                // Client dropped before first byte landed—放弃 stream，避免
                // 继续把上游 token 灌进黑洞。
                return;
            }
        }

        let mut stream = boot.stream;
        // 末尾缓冲：usage chunk 在流末尾，而 sse_log 到上限就丢后面、会截掉 usage。
        // 单独保留最后 ~16KB 专门解析 usage（够装最后的 usage chunk）。旁路，不碰转发。
        let mut usage_tail = usage_tail_from_prefix;
        let mut client_gone = false;
        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    let mut text = String::new();
                    crate::gateway::stream_utf8::append_utf8_safe(
                        &mut text,
                        &mut utf8_pending,
                        &bytes,
                    );
                    // Log (limited)
                    if sse_size < MAX_SSE_LOG {
                        let slice = crate::gateway::stream_utf8::truncate_at_char_boundary(
                            &text,
                            MAX_SSE_LOG - sse_size,
                        );
                        sse_log.push_str(slice);
                        sse_size += slice.len();
                    }
                    usage_tail.push_str(&text);
                    if usage_tail.len() > 16384 {
                        let cut = usage_tail.len() - 16384;
                        let mut b = cut;
                        while b < usage_tail.len() && !usage_tail.is_char_boundary(b) {
                            b += 1;
                        }
                        usage_tail.drain(..b);
                    }
                    // tx.send 在 client 已断开（mpsc receiver drop）时返回 Err。
                    // 显式 break——不然 reqwest 仍在从上游读，浪费 token + 占
                    // 用 keep-alive 连接。
                    if tx.send(text).await.is_err() {
                        client_gone = true;
                        break;
                    }
                }
                Err(e) => {
                    let msg = crate::gateway::sse_bootstrap::describe_stream_error(&e);
                    let payload = format!(
                        "data: {}\n\n",
                        serde_json::json!({"error": {"message": msg, "type": "upstream_stream_idle"}})
                    );
                    let _ = tx.send(payload).await;
                    break;
                }
            }
        }

        let latency = start.elapsed().as_millis() as i64;
        let mut trace = serde_json::json!({
            "mode": &trace_mode_owned,
            "client_protocol": &client_protocol_owned,
            "provider_protocol": &client_protocol_owned,
            "model_resolution": &model_resolution_owned,
            "route": &route_owned,
            "target_url": &target,
            "stream": true,
            "sse_bytes": sse_size,
        });
        let (status, error_message) = stream_end_status(client_gone, &mut trace);
        let trace = trace.to_string();

        let sanitized_sse = sanitize(&sse_log, &api_key);
        if let (Some(sid), Some(pid), Some(usage)) = (
            session_id_owned.as_deref(),
            provider_id_owned.as_deref(),
            parse_chat_usage_value(&usage_tail),
        ) {
            crate::gateway::session_affinity::record_if_cache_hit(sid, pid, &usage);
        }
        // 旁路解析直通响应里的 token usage（流已原样转发，这里只读不改），
        // 有则记 token + 算成本；解析不出保持现状（None）。
        let (inp, out) = match parse_chat_usage(&usage_tail) {
            Some((i, o)) => (Some(i), Some(o)),
            None => (None, None),
        };
        // Chat / Responses 直通:input 已含 cached_tokens(OpenAI 口径)。
        let cache_read = parse_chat_usage_value(&usage_tail)
            .as_ref()
            .and_then(|u| crate::storage::request_logs::extract_cache_tokens(u).1);
        let usage = crate::gateway::usage::TokenUsage {
            input: inp,
            output: out,
            cache_write: None,
            cache_read,
            input_semantics: crate::gateway::usage::InputCacheSemantics::IncludesCacheRead,
        };
        let sse_events = truncate(&sanitized_sse, MAX_SSE_LOG);
        crate::runtime::db_blocking_detached(&db_clone, "pass_through_stream_log", move |conn| {
            let cost = if inp.is_some() || out.is_some() {
                crate::gateway::usage::cost_for_request(conn, &provider_name, &model_clone, &usage)
            } else {
                None
            };
            crate::storage::request_logs::insert(
                conn,
                &req_id,
                &client_type_owned,
                &provider_name,
                &model_clone,
                &route_owned,
                status,
                latency,
                Some(&raw_req),
                None,
                None,
                None,
                Some(&sse_events),
                None,
                error_message.as_deref(),
                Some(&with_route_decision(
                    conn,
                    &route_owned,
                    &provider_name,
                    &model_clone,
                    &raw_req,
                    &trace,
                )),
                inp,
                out,
                cost,
                None,
                cache_read,
                Some("gateway"),
                None,
                Some(&req_id),
            )
        });
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

/// 直通流结束时的日志状态:客户端中途断开记 499 + CLIENT_DISCONNECTED(与转换路径一致,
/// 不算上游失败),否则 200。usage / 成本照常按已解析的记。
fn stream_end_status(client_gone: bool, trace: &mut serde_json::Value) -> (i64, Option<String>) {
    if !client_gone {
        return (200, None);
    }
    let err = crate::gateway::routes::shared::client_disconnected_error();
    trace["error_code"] = json!(err.code);
    (
        crate::gateway::routes::shared::CLIENT_DISCONNECTED_STATUS,
        Some(format!(
            "{}: {}",
            err.message,
            err.detail.as_deref().unwrap_or("")
        )),
    )
}

/// 给直通日志的 trace 补 route_decision（按协议反推默认 profile），让「按策略」统计
/// 能拿到数据——直通路径之前不写路由决策。纯日志旁路：enrich 失败保留原 trace，
/// 绝不碰转发/转换/路由。
fn with_route_decision(
    conn: &Connection,
    route: &str,
    provider: &str,
    model: &str,
    raw_request: &str,
    trace: &str,
) -> String {
    crate::gateway::routes::enrich_trace_with_route_decision(
        conn,
        route,
        provider,
        model,
        raw_request,
        Some(trace),
    )
    .unwrap_or_else(|| trace.to_string())
}

/// 从 Chat Completions 流式响应尾部 SSE 文本里解析最后一条非 null 的 usage，
/// 返回 (prompt_tokens, completion_tokens)。直通模式不转换流，这里只**旁路**读
/// usage 做 token/成本统计——不碰转发、解析失败返回 None 保持现状。
fn parse_chat_usage(sse_tail: &str) -> Option<(i64, i64)> {
    let usage = parse_chat_usage_value(sse_tail)?;
    // Chat Completions 用 prompt_/completion_tokens,Responses 用 input_/output_tokens。
    let inp = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(serde_json::Value::as_i64);
    let out = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(serde_json::Value::as_i64);
    if inp.is_some() || out.is_some() {
        Some((inp.unwrap_or(0), out.unwrap_or(0)))
    } else {
        None
    }
}

/// 与 `parse_chat_usage` 同源，返回完整 usage 对象（给 session affinity 读 cache）。
fn parse_chat_usage_value(sse_tail: &str) -> Option<serde_json::Value> {
    for line in sse_tail.lines().rev() {
        let data = line
            .strip_prefix("data:")
            .map(str::trim)
            .unwrap_or_else(|| line.trim());
        if data.is_empty() || data == "[DONE]" || !data.contains("\"usage\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            // Chat 的 usage 在帧顶层;Responses 的在 response.completed 帧的 response 下。
            if let Some(usage) = v
                .get("usage")
                .or_else(|| v.pointer("/response/usage"))
                .filter(|u| !u.is_null())
            {
                return Some(usage.clone());
            }
        }
    }
    None
}

/// Anthropic SSE 里 usage 常在 message_start / message_delta 的 data 帧；
/// 合并所有含 usage 的片段字段，供 cache_read_input_tokens 亲和记录。
fn parse_anthropic_usage_value(sse_tail: &str) -> Option<serde_json::Value> {
    let mut merged = serde_json::Map::new();
    for line in sse_tail.lines() {
        let data = line
            .strip_prefix("data:")
            .map(str::trim)
            .unwrap_or_else(|| line.trim());
        if data.is_empty() || !data.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        let usage = v
            .get("usage")
            .or_else(|| v.pointer("/message/usage"))
            .filter(|u| u.is_object());
        if let Some(serde_json::Value::Object(obj)) = usage {
            for (k, val) in obj {
                merged.insert(k.clone(), val.clone());
            }
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(merged))
    }
}

fn sanitize(text: &str, api_key: &str) -> String {
    let mut s = text.to_string();
    if api_key.len() > 4 {
        s = s.replace(api_key, "sk-***REDACTED***");
    }
    truncate(&s, MAX_LOG_BODY)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    format!(
        "{}...(truncated)",
        crate::gateway::stream_utf8::truncate_at_char_boundary(s, max)
    )
}

/// 从直通的非流式响应 body 里读 token usage(只读不改)。解析不出返回 Default(全 None)。
fn usage_from_json_body(
    body_text: &str,
    semantics: crate::gateway::usage::InputCacheSemantics,
) -> crate::gateway::usage::TokenUsage {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body_text) else {
        return Default::default();
    };
    let Some(u) = v.get("usage") else {
        return Default::default();
    };
    usage_from_value(u, semantics)
}

/// usage 对象 → TokenUsage。Chat(prompt/completion_tokens)与 Responses / Anthropic
/// (input/output_tokens)两套字段名都认;缓存字段交给 extract_cache_tokens。
fn usage_from_value(
    u: &serde_json::Value,
    semantics: crate::gateway::usage::InputCacheSemantics,
) -> crate::gateway::usage::TokenUsage {
    let read = |keys: &[&str]| keys.iter().find_map(|k| u.get(*k).and_then(|v| v.as_i64()));
    let (cache_write, cache_read) = crate::storage::request_logs::extract_cache_tokens(u);
    crate::gateway::usage::TokenUsage {
        input: read(&["prompt_tokens", "input_tokens"]),
        output: read(&["completion_tokens", "output_tokens"]),
        cache_write,
        cache_read,
        input_semantics: semantics,
    }
}

fn log_to_db(
    db: &crate::storage::db::DbPool,
    client_type: &str,
    route: &str,
    request_id: &str,
    provider: &str,
    model: &str,
    raw_request: &str,
    raw_response: &str,
    error_message: Option<&str>,
    trace_json: &str,
    status_code: i64,
    latency_ms: i64,
    usage: crate::gateway::usage::TokenUsage,
) {
    // 异步 fire-and-forget：把 SQLite INSERT 挪出响应路径，
    // 不让客户端等几毫秒的盘 IO，也不卡 tokio async worker。
    let client_type = client_type.to_string();
    let route = route.to_string();
    let request_id = request_id.to_string();
    let provider = provider.to_string();
    let model = model.to_string();
    let raw_request = raw_request.to_string();
    let raw_response = raw_response.to_string();
    let error_message = error_message.map(|s| s.to_string());
    let trace_json = trace_json.to_string();

    crate::runtime::db_blocking_detached(db, "pass_through_log", move |conn| {
        let enriched =
            with_route_decision(conn, &route, &provider, &model, &raw_request, &trace_json);
        let cost = if usage.input.is_some() || usage.output.is_some() {
            crate::gateway::usage::cost_for_request(conn, &provider, &model, &usage)
        } else {
            None
        };
        crate::storage::request_logs::insert(
            conn,
            &request_id,
            &client_type,
            &provider,
            &model,
            &route,
            status_code,
            latency_ms,
            Some(&raw_request),
            None,
            if raw_response.is_empty() {
                None
            } else {
                Some(raw_response.as_str())
            },
            None,
            None,
            None,
            error_message.as_deref(),
            Some(&enriched),
            usage.input,
            usage.output,
            cost,
            usage.cache_write,
            usage.cache_read,
            Some("gateway"),
            None,
            Some(&request_id),
        )
    });
}

/// Anthropic Messages API pass-through — forward directly to provider's Anthropic endpoint.
/// Used when provider has `anthropic_base_url` set (e.g. DeepSeek, Kimi).
#[allow(clippy::too_many_arguments)]
pub async fn handle_anthropic(
    http_client: &reqwest::Client,
    db: &crate::storage::db::DbPool,
    config: &ProviderConfig,
    target_url: &str,
    raw_body: &str,
    model_override: Option<&str>,
    auto_cache: bool,
    request_id: &str,
    start: Instant,
    client_type: &str,
    client_headers: Option<&HeaderMap>,
    session_id: Option<&str>,
    provider_id: Option<&str>,
) -> Result<Response, AppError> {
    let is_stream = serde_json::from_str::<serde_json::Value>(raw_body)
        .ok()
        .and_then(|v| v.get("stream")?.as_bool())
        .unwrap_or(false);

    let mut body_json: serde_json::Value =
        serde_json::from_str(raw_body).unwrap_or(serde_json::json!({}));
    let requested = body_json
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (base_model, model_resolution) =
        resolve_native_model(requested, model_override, &config.default_model);
    let trace_mode = native_trace_mode(model_override);
    // Provider-specific final model cleanup for Anthropic passthrough.
    // OpenAI/Codex paths use their own resolved model value before reaching
    // this handler.
    let model =
        crate::gateway::anthropic_model_suffix::for_anthropic(&config.provider_type, &base_model);
    body_json["model"] = serde_json::json!(&model);
    // Prompt cache 断点注入(预算感知:Claude Code 自带的断点优先,只补剩余
    // 预算)。长对话省钱直接体现在成本仪表盘的 cache_read 上。
    if auto_cache {
        crate::transform::responses_to_anthropic::inject_cache_control(&mut body_json);
    }
    let trace = json!({"mode":trace_mode,"target":target_url,"model_resolution":model_resolution});
    let log = PassThroughLog {
        db,
        client_type,
        route: "/v1/messages",
        request_id,
        provider: &config.name,
        model: &model,
        raw_request: &sanitize(raw_body, config.api_key()),
        start,
    };
    // Copilot:api_key 字段存的是 GitHub OAuth token(gho_/ghu_),先交换成
    // 短期 Copilot bearer token(进程内缓存);同时按请求体分类 x-initiator
    // ——agent(工具续写/压缩)不计 premium 额度。交换失败直接返回带建议的
    // AppError,不静默降级。
    let copilot_auth = if crate::providers::copilot::is_copilot(&config.provider_type) {
        let token = match crate::providers::copilot::get_copilot_token(
            http_client,
            config.select_api_key(),
        )
        .await
        {
            Ok(t) => t,
            Err(err) => return Err(log.error(err, &trace)),
        };
        let initiator = crate::providers::copilot::classify_initiator(&body_json);
        Some((token, initiator))
    } else {
        None
    };
    let rewritten_body = body_json.to_string();

    // Anthropic uses x-api-key header instead of Bearer.
    // Builder is reconstructed inside the retry closure so a transient connect
    // failure (e.g. a dead keep-alive connection returned by the pool) can be
    // retried with a fresh connection.
    let build_request = || {
        let mut b = http_client
            .post(target_url)
            .header("content-type", "application/json")
            // 默认 anthropic-version；若 client 显式带了同名 header，
            // 下面 forward_client_headers 会覆盖这条——reqwest 同 header
            // 重复 set 会保留最后一次的值。
            .header("anthropic-version", "2023-06-01");
        match &copilot_auth {
            Some((token, initiator)) => {
                // Copilot 用 Bearer 鉴权(替代 x-api-key),并带上模拟
                // VS Code Copilot Chat 的必备 headers + 计费分类。
                b = b.header("Authorization", format!("Bearer {token}"));
                b = crate::providers::copilot::apply_request_headers(b);
                b = b.header("x-initiator", *initiator);
            }
            None => {
                b = b.header("x-api-key", config.select_api_key());
            }
        }
        for (k, v) in &config.extra_headers {
            b = b.header(k.as_str(), v.as_str());
        }
        // 把 client 的 anthropic-beta（如 context-1m-2025-08-07）+
        // anthropic-version（如果 client 想用更新版本）透到上游。其它
        // header 一律不转发（host / authorization 等敏感字段不能漏出去）。
        b = forward_client_headers(b, client_headers, ANTHROPIC_FORWARD_HEADERS);
        b.body(rewritten_body.clone())
    };

    if is_stream {
        // Stream pass-through
        let resp = match crate::providers::adapter::send_with_net_retry(&build_request, 1).await {
            Ok(r) => r,
            Err(e) => {
                let err = AppError::new(
                    crate::errors::codes::PASS_THROUGH_STREAM_FAILED,
                    format!("Failed: {e}"),
                );
                return Err(log.error(err, &trace));
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let body_text = match crate::gateway::http_client::read_text_capped(resp).await {
                Ok(t) => t,
                Err(err) => return Err(log.error(err, &trace)),
            };
            let sanitized = sanitize(&body_text, config.api_key());
            log_to_db(
                db,
                client_type,
                "/v1/messages",
                request_id,
                &config.name,
                &model,
                log.raw_request,
                "",
                Some(&truncate(&sanitized, 2000)),
                &trace.to_string(),
                status.as_u16() as i64,
                start.elapsed().as_millis() as i64,
                Default::default(),
            );
            return Err(upstream_http_error(
                crate::errors::codes::UPSTREAM_STREAM_ERROR,
                status,
                &sanitized,
                body_text,
            ));
        }

        // 与 OpenAI 直通流一致:先扫首批字节。HTTP 200 + 首帧 `event: error`
        // (overloaded / rate limit)变成可 failover 的 Err,而不是把坏流当 200 透传。
        let boot = match crate::gateway::sse_bootstrap::bootstrap_detect(resp).await {
            Ok(b) => b,
            Err(err) => return Err(log.error(err, &trace)),
        };

        // Pipe SSE stream
        let (tx, rx) = mpsc::channel::<String>(512);
        let db_clone = db.clone();
        let provider_name = config.name.clone();
        let model = model.clone();
        let req_id = request_id.to_string();
        let raw_req = log.raw_request.to_string();
        let target = target_url.to_string();
        let api_key = config.api_key().to_string();
        let client_type_owned = client_type.to_string();
        let trace_mode_owned = trace_mode.to_string();
        let model_resolution_owned = model_resolution.to_string();
        let session_id_owned = session_id.map(str::to_string);
        let provider_id_owned = provider_id.map(str::to_string);

        tokio::spawn(async move {
            let mut utf8_pending: Vec<u8> = Vec::new();
            let mut sse_log = String::new();
            let mut sse_size: usize = 0;
            let mut usage_tail = String::new();
            let mut pending_chunk = Some(bytes::Bytes::from(boot.prefix));
            let mut stream = boot.stream;
            let mut client_gone = false;
            loop {
                // bootstrap 已经读走的前缀先回放,再继续拉活流。
                let chunk_result = match pending_chunk.take() {
                    Some(prefix) => Ok(prefix),
                    None => match stream.next().await {
                        Some(r) => r,
                        None => break,
                    },
                };
                match chunk_result {
                    Ok(bytes) => {
                        let mut text = String::new();
                        crate::gateway::stream_utf8::append_utf8_safe(
                            &mut text,
                            &mut utf8_pending,
                            &bytes,
                        );
                        if text.is_empty() {
                            continue;
                        }
                        if sse_size < MAX_SSE_LOG {
                            let slice = crate::gateway::stream_utf8::truncate_at_char_boundary(
                                &text,
                                MAX_SSE_LOG - sse_size,
                            );
                            sse_log.push_str(slice);
                            sse_size += slice.len();
                        }
                        usage_tail.push_str(&text);
                        if usage_tail.len() > 16384 {
                            let cut = usage_tail.len() - 16384;
                            let mut b = cut;
                            while b < usage_tail.len() && !usage_tail.is_char_boundary(b) {
                                b += 1;
                            }
                            usage_tail.drain(..b);
                        }
                        // Client 断开则提前退出，省 upstream token。
                        if tx.send(text).await.is_err() {
                            client_gone = true;
                            break;
                        }
                    }
                    Err(e) => {
                        let msg = crate::gateway::sse_bootstrap::describe_stream_error(&e);
                        let payload = format!(
                            "event: error\ndata: {}\n\n",
                            json!({"type":"error","error":{"type":"upstream_stream_idle","message":msg}})
                        );
                        let _ = tx.send(payload).await;
                        break;
                    }
                }
            }
            let latency = start.elapsed().as_millis() as i64;
            let mut trace = json!({"mode":&trace_mode_owned,"target":&target,"model_resolution":&model_resolution_owned,"stream":true});
            let (status, error_message) = stream_end_status(client_gone, &mut trace);
            let trace = trace.to_string();
            let sanitized_sse = sanitize(&sse_log, &api_key);
            if let (Some(sid), Some(pid), Some(usage)) = (
                session_id_owned.as_deref(),
                provider_id_owned.as_deref(),
                parse_anthropic_usage_value(&usage_tail),
            ) {
                crate::gateway::session_affinity::record_if_cache_hit(sid, pid, &usage);
            }
            let sse_events = truncate(&sanitized_sse, MAX_SSE_LOG);
            // message_start(input / 缓存 token)在流开头,长流时早已滚出 usage_tail;
            // 开头部分在 sse_log 里。先读开头再读尾部,message_delta 的 output_tokens 覆盖。
            let usage = parse_anthropic_usage_value(&format!("{sse_log}\n{usage_tail}"))
                .map(|u| {
                    usage_from_value(
                        &u,
                        crate::gateway::usage::InputCacheSemantics::ExcludesCache,
                    )
                })
                .unwrap_or_default();
            crate::runtime::db_blocking_detached(
                &db_clone,
                "anthropic_pass_through_stream_log",
                move |conn| {
                    let cost = if usage.input.is_some() || usage.output.is_some() {
                        crate::gateway::usage::cost_for_request(
                            conn,
                            &provider_name,
                            &model,
                            &usage,
                        )
                    } else {
                        None
                    };
                    crate::storage::request_logs::insert(
                        conn,
                        &req_id,
                        client_type_owned.as_str(),
                        &provider_name,
                        &model,
                        "/v1/messages",
                        status,
                        latency,
                        Some(&raw_req),
                        None,
                        None,
                        None,
                        Some(&sse_events),
                        None,
                        error_message.as_deref(),
                        Some(&with_route_decision(
                            conn,
                            "/v1/messages",
                            &provider_name,
                            &model,
                            &raw_req,
                            &trace,
                        )),
                        usage.input,
                        usage.output,
                        cost,
                        usage.cache_write,
                        usage.cache_read,
                        Some("gateway"),
                        None,
                        Some(&req_id),
                    )
                },
            );
        });

        let stream = ReceiverStream::new(rx);
        let body = Body::from_stream(tokio_stream::StreamExt::map(stream, |s| {
            Ok::<_, std::convert::Infallible>(s)
        }));
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(body)
            .unwrap())
    } else {
        // Non-stream
        let sent = crate::providers::adapter::send_with_net_retry(&build_request, 1).await;
        let resp = match sent {
            Ok(r) => r,
            Err(e) => {
                let err = AppError::new(
                    crate::errors::codes::PASS_THROUGH_REQUEST_FAILED,
                    format!("Failed: {e}"),
                );
                return Err(log.error(err, &trace));
            }
        };
        let status = resp.status();
        let body_text = match crate::gateway::http_client::read_text_capped(resp).await {
            Ok(t) => t,
            Err(err) => return Err(log.error(err, &trace)),
        };
        let sanitized = sanitize(&body_text, config.api_key());
        let latency = start.elapsed().as_millis() as i64;
        let err_msg = if status.is_success() {
            None
        } else {
            Some(truncate(&sanitized, 2000))
        };
        if status.is_success() {
            if let (Some(sid), Some(pid)) = (session_id, provider_id) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body_text) {
                    if let Some(usage) = v.get("usage") {
                        crate::gateway::session_affinity::record_if_cache_hit(sid, pid, usage);
                    }
                }
            }
        }
        log_to_db(
            db,
            client_type,
            "/v1/messages",
            request_id,
            &config.name,
            &model,
            log.raw_request,
            &sanitized,
            err_msg.as_deref(),
            &trace.to_string(),
            status.as_u16() as i64,
            latency,
            if status.is_success() {
                usage_from_json_body(
                    &body_text,
                    crate::gateway::usage::InputCacheSemantics::ExcludesCache,
                )
            } else {
                Default::default()
            },
        );
        if !status.is_success() {
            return Err(upstream_http_error(
                crate::errors::codes::UPSTREAM_NON_STREAM_ERROR,
                status,
                &sanitized,
                body_text,
            ));
        }
        Ok(Response::builder()
            .status(status.as_u16())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body_text))
            .unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_usage_picks_final_non_null_chunk() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}],\"usage\":null}\n\n\
                   data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":42}}\n\n\
                   data: [DONE]\n\n";
        assert_eq!(parse_chat_usage(sse), Some((100, 42)));
    }

    #[test]
    fn parse_usage_none_when_all_null() {
        let sse = "data: {\"usage\":null}\n\ndata: [DONE]\n\n";
        assert_eq!(parse_chat_usage(sse), None);
    }

    // Responses API 直通：usage 在 response.completed 帧的 `response.usage` 下,
    // 字段名是 input_tokens / output_tokens（不是 Chat 的 prompt_/completion_）。
    #[test]
    fn parse_usage_reads_responses_completed_frame() {
        let sse = "event: response.created\n\
                   data: {\"type\":\"response.created\",\"response\":{\"usage\":null}}\n\n\
                   event: response.completed\n\
                   data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":88,\"input_tokens_details\":{\"cached_tokens\":64},\"output_tokens\":17,\"output_tokens_details\":{\"reasoning_tokens\":15},\"total_tokens\":105}}}\n\n";
        assert_eq!(parse_chat_usage(sse), Some((88, 17)));
        let usage = parse_chat_usage_value(sse).expect("usage value");
        assert_eq!(
            usage.pointer("/input_tokens_details/cached_tokens"),
            Some(&serde_json::json!(64))
        );
    }

    #[test]
    fn native_model_mapping_wins() {
        assert_eq!(
            resolve_native_model(
                "claude-sonnet-4-6",
                Some("mimo-v2.5-pro[1m]"),
                "mimo-v2.5-pro"
            ),
            ("mimo-v2.5-pro[1m]".to_string(), "model_mapping")
        );
    }

    #[test]
    fn native_model_preserves_request_when_unmapped() {
        assert_eq!(
            resolve_native_model("mimo-v2.5-pro", None, "mimo-v2.5"),
            ("mimo-v2.5-pro".to_string(), "request_model")
        );
    }

    #[test]
    fn native_model_uses_default_only_when_missing() {
        assert_eq!(
            resolve_native_model("", None, "mimo-v2.5-pro"),
            ("mimo-v2.5-pro".to_string(), "default_model")
        );
    }

    #[test]
    fn test_sanitize_replaces_api_key() {
        let text = "error: sk-abc123def456 is invalid";
        let result = sanitize(text, "sk-abc123def456");
        assert!(result.contains("sk-***REDACTED***"));
        assert!(!result.contains("sk-abc123def456"));
    }

    #[test]
    fn test_sanitize_no_change_if_key_short() {
        let text = "error: sk- is invalid";
        let result = sanitize(text, "sk-");
        // api_key.len() == 3 <= 4, so no replacement
        assert_eq!(result, text);
    }

    #[test]
    fn test_sanitize_truncates_long_text() {
        let text = "x".repeat(MAX_LOG_BODY + 100);
        let result = sanitize(&text, "sk-key");
        assert!(result.ends_with("...(truncated)"));
        assert!(result.len() < text.len());
    }

    #[test]
    fn test_truncate_within_limit() {
        let s = "short text";
        assert_eq!(truncate(s, 100), "short text");
    }

    #[test]
    fn test_truncate_exceeds_limit() {
        let s = "x".repeat(200);
        let result = truncate(&s, 100);
        assert!(result.starts_with("xxxxxxxxxx"));
        assert!(result.ends_with("...(truncated)"));
    }

    #[test]
    fn test_truncate_exact_limit() {
        let s = "x".repeat(50);
        assert_eq!(truncate(&s, 50), s);
    }

    #[test]
    fn test_truncate_chinese_boundary() {
        let s = "你好世界"; // 4 chars, 12 bytes
                            // Truncate at byte 7 — inside "世" (bytes 6..9) → snap back to 6
        let result = truncate(s, 7);
        assert_eq!(result, "你好...(truncated)");
    }

    #[test]
    fn test_truncate_emoji_boundary() {
        let s = "hi🎉ok"; // "hi" 2B + 🎉 4B + "ok" 2B = 8B
                          // Truncate at 3 — inside 🎉 → snap back to 2
        let result = truncate(s, 3);
        assert_eq!(result, "hi...(truncated)");
    }
}
