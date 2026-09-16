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
use crate::gateway::sse::SseAccumulator;
use crate::gateway::sse_anthropic::AnthropicSseAccumulator;
use crate::gateway::sse_gemini::GeminiSseAccumulator;
use crate::protocol::chat_completions::{ChatCompletionResponse, ChatMessage};
use crate::protocol::openai_responses::ResponsesRequest;
use crate::providers::adapter::{self, ProviderConfig};
use crate::transform::{responses_to_anthropic, responses_to_chat, responses_to_gemini};

use super::shared::{
    check_budget, detect_client_from_ua, log_request_error, log_request_error_full,
    log_request_success, native_model_override_for_images, refine_struct_body, refine_value_body,
    request_body_or_gateway_error, request_contains_images, sanitize_body, select_providers,
    stream_error_status, stream_task_error, trace_with_degradation_events, truncate_str,
    GatewayError,
};
use super::GatewayState;

mod anthropic;
mod chat;
mod gemini;
use anthropic::*;
use chat::*;
use gemini::*;

/// Whether a request may be passed through to the provider's native Responses
/// API. Providers listed in `RESPONSES_NATIVE_MODELS` only support a subset of
/// their models there. Image requests are allowed only when the selected model
/// is also marked `vision` in the generated catalog; other image requests fall
/// back to Responses→Chat conversion, which applies the provider's image
/// degradation policy.
///
/// 工具同理：DeepSeek 的 Responses API 只接受 `apply_patch` 这一个 custom 工具,
/// 其它一律 400（实测 `{"type":"custom","name":"exec"}` → "Unsupported custom
/// tool"）。Codex 0.152+ 把 code-mode `exec` 放进 `{type:namespace,name:functions}`
/// 里，只扫顶层 custom 会误判为可直通，模型侧就看不到 shell。带不支持的
/// custom 工具（含 namespace 嵌套）时必须回落到转换路径。
///
/// 真正的 OpenAI Responses 认 custom exec，可以直通。其它不在表里的类型
///（`custom_openai_compatible` 等）多数不认，直通会丢掉命名空间里的 `exec`，
/// 一律按「不允许任何 custom 工具」处理。
fn native_responses_allowed(
    provider_type: &str,
    model: &str,
    has_images: bool,
    tools: Option<&[Value]>,
) -> bool {
    use crate::storage::generated_provider_catalog as catalog;
    let Some((_, models)) = catalog::RESPONSES_NATIVE_MODELS
        .iter()
        .find(|(ty, _)| *ty == provider_type)
    else {
        if provider_type == "openai" {
            return true;
        }
        return !crate::transform::tool_calls::contains_disallowed_custom_tool(
            tools.unwrap_or(&[]),
            &[],
        );
    };
    if !models.contains(&model) {
        return false;
    }
    if has_images {
        let model_base = crate::transform::tool_calls::model_base(model);
        let supports_vision = catalog::MODEL_CAPABILITIES.iter().any(|(ty, id, caps)| {
            *ty == provider_type
                && *id == model_base
                && caps.contains(&crate::providers::capabilities::CAP_VISION)
        });
        if !supports_vision {
            return false;
        }
    }
    let Some((_, allowed)) = catalog::RESPONSES_NATIVE_CUSTOM_TOOLS
        .iter()
        .find(|(ty, _)| *ty == provider_type)
    else {
        return true;
    };
    !crate::transform::tool_calls::contains_disallowed_custom_tool(tools.unwrap_or(&[]), allowed)
}

/// The model that `pass_through::handle` will actually send upstream:
/// explicit override (model_mapping / virtual model) > client model > provider default.
fn native_pass_through_model<'a>(
    model_override: Option<&'a str>,
    requested: Option<&'a str>,
    default_model: &'a str,
) -> &'a str {
    model_override
        .or_else(|| requested.filter(|m| !m.trim().is_empty()))
        .unwrap_or(default_model)
}

// ── POST /v1/responses ─────────────────────────────────────────

pub async fn handle_responses(
    headers: HeaderMap,
    AxumState(state): AxumState<GatewayState>,
    body: Result<bytes::Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Response, GatewayError> {
    // 鉴权 + Host/Origin 边界校验在 server.rs 的中间件里、读 body 之前完成。
    let body = request_body_or_gateway_error(body)?;
    let force_cheapest = check_budget(&state.db).await?;
    let start = Instant::now();
    let request_id = format!(
        "req_{}",
        &uuid::Uuid::new_v4().to_string().replace('-', "")[..12]
    );
    let client_type = detect_client_from_ua(&headers, "Codex");

    // Decompress if needed — Codex.app with `requires_openai_auth = true`
    // gzip-compresses the request body to match the production OpenAI flow.
    let body = crate::gateway::body_decode::decode(&headers, body, state.request_body_limit)
        .map_err(|e| {
            log_request_error(
                &state.db,
                &client_type,
                "/v1/responses",
                &request_id,
                "",
                None,
                &e,
                start.elapsed().as_millis() as i64,
            );
            GatewayError(e)
        })?;

    // 1. Parse request
    let mut req: ResponsesRequest = serde_json::from_str(&body).map_err(|e| {
        let err = AppError::new(
            crate::errors::codes::RESPONSES_PARSE_ERROR,
            format!("Failed to parse request: {e}"),
        );
        // Log the error
        log_request_error(
            &state.db,
            &client_type,
            "/v1/responses",
            &request_id,
            &sanitize_body(&body),
            None,
            &err,
            start.elapsed().as_millis() as i64,
        );
        err
    })?;
    // Codex gpt-5.6+ 把工具放在 input 的 additional_tools 项里,提升到顶层
    // tools,让下游 chat/anthropic/gemini 转换无感支持(否则工具整批丢失)。
    req.hoist_additional_tools();
    // 原生直通转发的是 body 原文,结构体上的提升对它无效 —— 同一份提升要落到
    // body 上,否则上游看不到任何工具,模型只会在正文里编造工具调用。
    let body = match serde_json::from_str::<Value>(&body) {
        Ok(mut v) => {
            crate::protocol::openai_responses::hoist_additional_tools_in_body(&mut v);
            v.to_string()
        }
        Err(_) => body,
    };

    // 2. Select provider via route profile (with failover candidates)
    let analysis = crate::gateway::provider_selector::analyze_request(&req);
    let selection = select_providers(
        &state.db,
        &["openai_responses"],
        req.model.clone(),
        analysis.clone(),
        force_cheapest,
    )
    .await
    .map_err(|e| {
        log_request_error(
            &state.db,
            &client_type,
            "/v1/responses",
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

    // Derive a stable session-affinity key. Used at two points: candidate
    // reordering (prefer the provider that hit the upstream prompt cache last
    // time) and post-response recording (write affinity when cached_tokens>0).
    // force_cheapest 时禁用亲和重排，避免把贵上游提到队首。
    let session_id = crate::gateway::session_affinity::derive_from_responses(&req);
    let affinity_sid = if force_cheapest {
        None
    } else {
        session_id.as_deref()
    };

    // Detect if request contains images (for vision-aware routing)
    let request_has_images = request_contains_images(&req);

    // 主 provider 优先 + failover 候选 + vision 过滤 + 会话亲和,统一由 failover 模块构建。
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

    let ctx = ResponsesAttemptCtx {
        state: &state,
        headers: &headers,
        req: &req,
        body: &body,
        raw_body: &raw_body,
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
                Some(provider) => attempt_responses_provider(ctx, provider, &candidate).await,
                None => Attempt::Skip(AppError::not_found("Provider", &candidate.provider_id)),
            }
        },
    )
    .await
    .map_err(GatewayError)
}

/// 一次 /v1/responses 请求在所有候选间共享的只读上下文。
#[derive(Clone, Copy)]
struct ResponsesAttemptCtx<'a> {
    state: &'a GatewayState,
    headers: &'a HeaderMap,
    req: &'a ResponsesRequest,
    body: &'a str,
    raw_body: &'a str,
    request_has_images: bool,
    request_id: &'a str,
    start: Instant,
    client_type: &'a str,
    session_id: Option<&'a str>,
}

/// Responses → 各上游协议的转换会按 `previous_response_id` 查 session_store
/// (L2 是全局 Mutex + 同步 SQLite)。带 previous_response_id 时整个转换挪到
/// blocking 线程池;不带时没有 IO,原地转换省一次请求体克隆。
async fn convert_off_worker<T, F>(req: &ResponsesRequest, convert: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce(&ResponsesRequest) -> Result<T, AppError> + Send + 'static,
{
    if req.previous_response_id.is_none() {
        return convert(req);
    }
    let owned = req.clone();
    tokio::task::spawn_blocking(move || convert(&owned))
        .await
        .map_err(|e| AppError::internal(format!("request conversion task failed: {e}")))?
}

/// 对单个候选发起一次 Responses 请求(直通 / Anthropic / Gemini / Chat 转换)。
async fn attempt_responses_provider(
    ctx: ResponsesAttemptCtx<'_>,
    provider: &crate::models::provider::Provider,
    candidate: &crate::gateway::provider_selector::ProviderCandidate,
) -> Attempt<Response> {
    let state = ctx.state;
    let req = ctx.req;
    let request_id = ctx.request_id.to_string();
    let raw_body = ctx.raw_body.to_string();
    let client_type = ctx.client_type.to_string();
    let session_id = ctx.session_id.map(str::to_string);
    let start = ctx.start;
    let request_has_images = ctx.request_has_images;

    let config = match ProviderConfig::from_provider(provider) {
        Ok(c) => c,
        Err(e) => return Attempt::Skip(e),
    };

    let model = candidate.model.clone();

    let model_override = native_model_override_for_images(
        provider,
        req.model.as_deref(),
        Some(&model),
        request_has_images,
    );
    let native_model = native_pass_through_model(
        model_override.as_deref(),
        req.model.as_deref(),
        &config.default_model,
    );
    let native_responses = config.has_responses_url()
        && native_responses_allowed(
            &provider.provider_type,
            native_model,
            request_has_images,
            req.tools.as_deref(),
        );

    let result = if native_responses {
        // Pass-through: provider has explicit Responses API endpoint
        let target_url = config.responses_url();
        crate::gateway::pass_through::handle(
            &state.http_client,
            &state.db,
            &config,
            &target_url,
            "/v1/responses",
            "openai_responses",
            ctx.body,
            model_override.as_deref(),
            &request_id,
            start,
            &client_type,
            Some(ctx.headers),
            ctx.session_id,
            Some(candidate.provider_id.as_str()),
        )
        .await
        .map_err(GatewayError)
    } else if config.is_anthropic() {
        // Claude Messages API conversion (only for Anthropic-type providers)
        // auto_cache_control: default true unless provider explicitly set false
        let auto_cache = provider.auto_cache_control.unwrap_or(true);
        let convert_model = model.clone();
        let mut anthropic_body = match convert_off_worker(req, move |r| {
            responses_to_anthropic::convert(r, &convert_model, auto_cache)
        })
        .await
        {
            Ok(b) => b,
            Err(e) => return Attempt::Abort(e),
        };
        let _refiner_log = refine_value_body(&state.db, provider, &mut anthropic_body);
        let converted_json = serde_json::to_string(&anthropic_body).unwrap_or_default();
        let is_stream = req.stream.unwrap_or(false);
        if is_stream {
            handle_anthropic_stream_response(
                state.clone(),
                config.clone(),
                anthropic_body,
                request_id.clone(),
                raw_body.clone(),
                converted_json,
                model.clone(),
                start,
                client_type.clone(),
                session_id.clone(),
                candidate.provider_id.clone(),
            )
            .await
        } else {
            handle_anthropic_non_stream_response(
                state.clone(),
                config.clone(),
                anthropic_body,
                request_id.clone(),
                raw_body.clone(),
                converted_json,
                model.clone(),
                start,
                client_type.clone(),
                session_id.clone(),
                candidate.provider_id.clone(),
            )
            .await
        }
    } else if config.is_gemini() {
        // Gemini API conversion
        let convert_model = model.clone();
        let mut gemini_body = match convert_off_worker(req, move |r| {
            responses_to_gemini::convert(r, &convert_model)
        })
        .await
        {
            Ok(b) => b,
            Err(e) => return Attempt::Abort(e),
        };
        let _refiner_log = refine_value_body(&state.db, provider, &mut gemini_body);
        let converted_json = serde_json::to_string(&gemini_body).unwrap_or_default();
        let is_stream = req.stream.unwrap_or(false);
        if is_stream {
            handle_gemini_stream_response(
                state.clone(),
                config.clone(),
                gemini_body,
                request_id.clone(),
                raw_body.clone(),
                converted_json,
                model.clone(),
                start,
                client_type.clone(),
                session_id.clone(),
                candidate.provider_id.clone(),
            )
            .await
        } else {
            handle_gemini_non_stream_response(
                state.clone(),
                config.clone(),
                gemini_body,
                request_id.clone(),
                raw_body.clone(),
                converted_json,
                model.clone(),
                start,
                client_type.clone(),
                session_id.clone(),
                candidate.provider_id.clone(),
            )
            .await
        }
    } else {
        // Chat Completions path (default: transform Responses → Chat Completions)
        let provider_transform = crate::transform::providers::for_config(&config);
        // Per-model capability matrix 直接取已加载的 provider(不再按 id 重查一次库)。
        // Empty map → fall back to legacy "always emit web_search for MiMo" behavior.
        let matrix = provider
            .model_capabilities
            .as_deref()
            .and_then(|s| {
                serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(s).ok()
            })
            .unwrap_or_default();
        let convert_model = model.clone();
        let mut chat_req = match convert_off_worker(req, move |r| {
            responses_to_chat::convert_with_provider_matrix(
                r,
                &convert_model,
                provider_transform.as_ref(),
                &matrix,
            )
        })
        .await
        {
            Ok(r) => r,
            Err(e) => return Attempt::Abort(e),
        };
        // 长历史自压缩:超阈值时摘要中段历史,落回上游窗口内。默认开启,阈值按模型
        // 上下文窗口 × usage% 自适应(详见 auto_compact),内部按需额外调一次上游。
        let compact_policy = match crate::runtime::db_blocking(&state.db, |conn| {
            crate::storage::gateway_settings::get(conn)
        })
        .await
        {
            Ok(s) => crate::gateway::auto_compact::CompactPolicy::from_settings(
                s.auto_compact_enabled,
                s.auto_compact_usage_percent,
            ),
            Err(e) => {
                tracing::warn!(error = %e, "read gateway settings for auto-compact failed; using default policy");
                crate::gateway::auto_compact::CompactPolicy::default()
            }
        };
        crate::gateway::auto_compact::maybe_compact_with_policy(
            &state.http_client,
            &config,
            &mut chat_req,
            compact_policy,
        )
        .await;
        let _refiner_log = refine_struct_body(&state.db, provider, &mut chat_req);
        let converted_json = serde_json::to_string(&chat_req).unwrap_or_default();
        let is_stream = chat_req.stream;
        if is_stream {
            handle_stream_response(
                state.clone(),
                config.clone(),
                chat_req,
                request_id.clone(),
                raw_body.clone(),
                converted_json,
                model.clone(),
                start,
                client_type.clone(),
                session_id.clone(),
                candidate.provider_id.clone(),
            )
            .await
        } else {
            handle_non_stream_response(
                state.clone(),
                config.clone(),
                chat_req,
                req.clone(),
                request_id.clone(),
                raw_body.clone(),
                converted_json,
                model.clone(),
                start,
                client_type.clone(),
                session_id.clone(),
                candidate.provider_id.clone(),
            )
            .await
        }
    };

    match result {
        Ok(response) => Attempt::Success(response),
        Err(GatewayError(err)) => crate::gateway::failover::classify_error(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::chat_completions::{CompletionChoice, CompletionMessage};
    use serde_json::json;

    // ── native Responses pass-through gating ──────────────────────

    #[test]
    fn native_responses_allowed_for_whitelisted_model() {
        assert!(native_responses_allowed(
            "deepseek",
            "deepseek-flash",
            false,
            None
        ));
    }

    #[test]
    fn native_responses_blocked_for_unlisted_model_of_restricted_provider() {
        // deepseek-v4-pro has no Responses API support yet → must fall back
        // to the Responses→Chat conversion path instead of passing through.
        assert!(!native_responses_allowed(
            "deepseek",
            "deepseek-v4-pro",
            false,
            None
        ));
    }

    #[test]
    fn native_responses_allows_images_for_deepseek_flash() {
        assert!(native_responses_allowed(
            "deepseek",
            "deepseek-flash",
            true,
            None
        ));
    }

    // Codex gpt-5.6+ 发的 `exec` 是 custom 工具,DeepSeek 只收 apply_patch,
    // 直通会被上游 400 —— 必须回落到转换路径。
    #[test]
    fn native_responses_blocked_for_unsupported_custom_tool() {
        let tools = vec![
            json!({"type": "function", "name": "shell"}),
            json!({"type": "custom", "name": "exec"}),
        ];
        assert!(!native_responses_allowed(
            "deepseek",
            "deepseek-flash",
            false,
            Some(&tools)
        ));
    }

    #[test]
    fn native_responses_blocked_for_exec_nested_in_functions_namespace() {
        // Codex 0.152.1 把 code-mode exec 包进 functions 命名空间。
        // 只扫顶层 type=custom 会误判为可直通，DeepSeek 收不到可调用的
        // exec，模型就会说当前会话没有 shell。
        let tools = vec![json!({
            "type": "namespace",
            "name": "functions",
            "tools": [
                {"type": "custom", "name": "exec", "description": "run js"},
                {"type": "function", "name": "wait", "parameters": {"type": "object"}}
            ]
        })];
        assert!(!native_responses_allowed(
            "deepseek",
            "deepseek-flash",
            false,
            Some(&tools)
        ));
    }

    #[test]
    fn native_responses_allows_supported_custom_tool() {
        let tools = vec![
            json!({"type": "function", "name": "shell"}),
            json!({"type": "custom", "name": "apply_patch"}),
        ];
        assert!(native_responses_allowed(
            "deepseek",
            "deepseek-flash",
            false,
            Some(&tools)
        ));
    }

    #[test]
    fn native_responses_custom_tool_rule_skips_unrestricted_provider() {
        let tools = vec![json!({"type": "custom", "name": "exec"})];
        assert!(native_responses_allowed(
            "openai",
            "gpt-5.6-terra",
            false,
            Some(&tools)
        ));
    }

    fn namespaced_exec_tools() -> Vec<Value> {
        vec![json!({
            "type": "namespace",
            "name": "functions",
            "tools": [
                {"type": "custom", "name": "exec", "description": "run js"},
                {"type": "function", "name": "wait", "parameters": {"type": "object"}}
            ]
        })]
    }

    #[test]
    fn native_responses_openai_allows_namespaced_exec() {
        // Real OpenAI Responses 认 custom exec，可以直通。
        assert!(native_responses_allowed(
            "openai",
            "gpt-5.6-terra",
            false,
            Some(&namespaced_exec_tools())
        ));
    }

    #[test]
    fn native_responses_blocked_for_namespaced_exec_on_third_party() {
        // custom_openai_compatible 等第三方 Responses 不认 Codex 的 custom exec，
        // 直通会把命名空间里的工具丢掉。必须回落到转换路径。
        assert!(!native_responses_allowed(
            "custom_openai_compatible",
            "some-model",
            false,
            Some(&namespaced_exec_tools())
        ));
    }

    #[test]
    fn native_responses_allows_third_party_without_custom_tools() {
        assert!(native_responses_allowed(
            "custom_openai_compatible",
            "some-model",
            false,
            None
        ));
        let tools = vec![json!({"type": "function", "name": "wait"})];
        assert!(native_responses_allowed(
            "custom_openai_compatible",
            "some-model",
            false,
            Some(&tools)
        ));
    }

    #[test]
    fn native_responses_unrestricted_provider_always_allowed() {
        // Providers without a responsesModels entry keep the previous behaviour.
        assert!(native_responses_allowed(
            "openai",
            "gpt-5.6-terra",
            false,
            None
        ));
        assert!(native_responses_allowed(
            "openai",
            "gpt-5.6-terra",
            true,
            None
        ));
    }

    #[test]
    fn native_pass_through_model_prefers_override_then_request_then_default() {
        assert_eq!(
            native_pass_through_model(Some("mapped"), Some("requested"), "default"),
            "mapped"
        );
        assert_eq!(
            native_pass_through_model(None, Some("requested"), "default"),
            "requested"
        );
        assert_eq!(
            native_pass_through_model(None, Some("  "), "default"),
            "default"
        );
        assert_eq!(native_pass_through_model(None, None, "default"), "default");
    }

    #[test]
    fn chat_non_stream_response_envelope_preserves_responses_metadata() {
        let req = ResponsesRequest {
            model: Some("client-model".into()),
            input: json!("hello"),
            instructions: Some("Be concise".into()),
            tools: Some(vec![json!({"type": "function", "name": "shell"})]),
            tool_choice: Some(json!("auto")),
            temperature: Some(0.2),
            top_p: Some(0.9),
            max_output_tokens: Some(123),
            parallel_tool_calls: Some(false),
            reasoning: Some(json!({"effort": "high"})),
            text: Some(json!({"format": {"type": "json_object"}})),
            metadata: Some(json!({"trace": "abc"})),
            previous_response_id: Some("resp_prev".into()),
            ..Default::default()
        };
        let chat_resp = ChatCompletionResponse {
            id: Some("chatcmpl_1".into()),
            usage: Some(json!({
                "prompt_tokens": 10,
                "completion_tokens": 3,
                "total_tokens": 13
            })),
            choices: Some(vec![CompletionChoice {
                finish_reason: Some("length".into()),
                message: Some(CompletionMessage {
                    role: Some("assistant".into()),
                    content: Some("partial".into()),
                    reasoning_content: None,
                    tool_calls: None,
                }),
            }]),
        };

        let resp = build_chat_non_stream_responses_response(
            "resp_test",
            "mimo-v2.5-pro",
            &req,
            &chat_resp,
            vec![json!({
                "id": "msg_test",
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "partial"}]
            })],
        );

        assert_eq!(resp["status"], "incomplete");
        assert_eq!(
            resp["incomplete_details"],
            json!({"reason": "max_output_tokens"})
        );
        assert_eq!(resp["usage"]["input_tokens"], 10);
        assert_eq!(resp["usage"]["output_tokens"], 3);
        assert_eq!(resp["usage"]["total_tokens"], 13);
        assert_eq!(resp["parallel_tool_calls"], false);
        assert_eq!(resp["tool_choice"], json!("auto"));
        assert_eq!(
            resp["reasoning"],
            json!({"effort": "high", "summary": null})
        );
        assert_eq!(resp["text"], json!({"format": {"type": "json_object"}}));
        assert_eq!(resp["metadata"], json!({"trace": "abc"}));
        assert_eq!(resp["previous_response_id"], "resp_prev");
        assert_eq!(resp["instructions"], "Be concise");
        assert_eq!(resp["temperature"], 0.2);
        assert_eq!(resp["top_p"], 0.9);
        assert_eq!(resp["max_output_tokens"], 123);
        assert_eq!(
            resp["tools"],
            json!([{"type": "function", "name": "shell"}])
        );
        assert_eq!(resp["truncation"], "disabled");
    }
}
