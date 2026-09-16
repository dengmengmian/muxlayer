use axum::extract::rejection::BytesRejection;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use bytes::Bytes;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::errors::AppError;
use crate::models::provider::Provider;
use crate::protocol::openai_responses::ResponsesRequest;
use crate::security::local_token;

/// Run refiner pipeline on a Value-shaped outbound request body, mutating it
/// in place. Returns the RefinerLog (current callers ignore it pending the
/// trace_json wiring change; once that lands, every handler should stash it
/// into the request log). Failing to lock the DB or read settings degrades
/// to no-op — the gateway should still forward the request transparently.
pub(crate) fn refine_value_body(
    db: &crate::storage::db::DbPool,
    provider: &Provider,
    body: &mut Value,
) -> crate::gateway::refiner_log::RefinerLog {
    let settings = match db
        .get()
        .ok()
        .and_then(|c| crate::storage::gateway_settings::get(&c).ok())
    {
        Some(s) => s,
        None => return crate::gateway::refiner_log::RefinerLog::default(),
    };
    crate::gateway::refiners::runtime::apply_request(provider, &settings, body)
}

/// Convenience wrapper: serde-ify a serializable request struct, run the
/// refiner pipeline against the JSON view, then ask serde to materialise the
/// modified struct back. If either serde leg fails the original struct is
/// returned untouched — refiner errors must never block the request.
pub(crate) fn refine_struct_body<T>(
    db: &crate::storage::db::DbPool,
    provider: &Provider,
    req: &mut T,
) -> crate::gateway::refiner_log::RefinerLog
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut body = match serde_json::to_value(&*req) {
        Ok(v) => v,
        Err(_) => return crate::gateway::refiner_log::RefinerLog::default(),
    };
    let log = refine_value_body(db, provider, &mut body);
    if !log.is_empty() {
        if let Ok(new) = serde_json::from_value::<T>(body) {
            *req = new;
        }
    }
    log
}

/// Best-effort client identification from the request's User-Agent header.
/// Falls back to a route-default label when UA is empty / unknown so that
/// at least the protocol is conveyed (e.g. Codex is the only common client
/// using /v1/responses today).
///
/// Common patterns:
///   - Codex CLI / desktop:   "OpenAI/Python" or "codex"
///   - Claude Code:           "claude-cli" / "claude-code"
///   - OpenCode:              "opencode"
///   - AtomCode:              "atomcode"
///   - Kimi CLI:              "KimiCLI/1.40.0"
///   - Cursor:                "Cursor/..."
///   - Cherry Studio:         "Cherry-Studio"
///   - Continue.dev:          "continue"
///   - AgentGate Pet:         "AgentGate-Pet/..."
///   - generic SDKs:          "Python/requests", "node-fetch", "axios", etc.
pub(crate) fn detect_client_from_ua(headers: &HeaderMap, route_default: &str) -> String {
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    if ua.is_empty() {
        return route_default.to_string();
    }
    let lower = ua.to_ascii_lowercase();
    // Order matters: more specific matches first.
    if lower.contains("agentgate-pet") {
        return "Pet".to_string();
    }
    if lower.contains("claude-code")
        || lower.contains("claude-cli")
        || lower.contains("claude code")
    {
        return "Claude Code".to_string();
    }
    if lower.contains("codex-cli") || lower.starts_with("codex/") {
        return "Codex".to_string();
    }
    if lower.contains("opencode") {
        return "OpenCode".to_string();
    }
    if lower.contains("atomcode") {
        return "AtomCode".to_string();
    }
    if lower.contains("kimicli") || lower.contains("kimi-cli") || lower.contains("kimi cli") {
        return "Kimi CLI".to_string();
    }
    if lower.contains("grok-build") || lower.starts_with("grok/") {
        return "Grok Build".to_string();
    }
    if lower.contains("deepseek-harness") || lower.starts_with("dsh/") {
        return "DeepSeek Harness".to_string();
    }
    if lower.contains("cursor") {
        return "Cursor".to_string();
    }
    if lower.contains("cherry") {
        return "Cherry Studio".to_string();
    }
    if lower.contains("continue") {
        return "Continue".to_string();
    }
    if lower.contains("cline") {
        return "Cline".to_string();
    }
    if lower.contains("roo") {
        return "Roo Code".to_string();
    }
    if lower.contains("hermes") {
        return "Hermes".to_string();
    }
    if lower.contains("opencode") {
        return "OpenCode".to_string();
    }
    if lower.starts_with("openai/") || lower.contains("openai-python") {
        // Codex CLI desktop reports "OpenAI/Python ..." too; treat as Codex
        // when the route is the Responses API.
        if route_default == "Codex" {
            return "Codex".to_string();
        }
        return "OpenAI SDK".to_string();
    }
    if lower.contains("anthropic-sdk") || lower.starts_with("anthropic/") {
        return "Anthropic SDK".to_string();
    }
    if lower.starts_with("python") || lower.contains("python-requests") || lower.contains("httpx") {
        return "Python SDK".to_string();
    }
    if lower.starts_with("node")
        || lower.contains("node-fetch")
        || lower.contains("axios")
        || lower.contains("undici")
    {
        return "Node SDK".to_string();
    }
    if lower.starts_with("curl") {
        return "curl".to_string();
    }
    // Unknown — surface the raw first token (helps users identify new clients)
    let token: String = ua
        .split_whitespace()
        .next()
        .unwrap_or(ua)
        .chars()
        .take(40)
        .collect();
    if token.is_empty() {
        route_default.to_string()
    } else {
        token
    }
}

/// 浏览器侧边界防护(DNS rebinding):Host 必须是 IP 字面量 / localhost /
/// `AGENTGATE_ALLOWED_HOSTS` 白名单;带 Origin 的请求(浏览器跨站)按同规则
/// 校验 origin 的 host 部分,`AGENTGATE_ALLOWED_ORIGINS` 可整条放行。
/// CLI 客户端 Host 是 IP/localhost 且不发 Origin,不受影响。
pub(crate) fn validate_request_boundary(headers: &HeaderMap) -> Result<(), GatewayError> {
    let hosts_allow = crate::compat::env_value("MUXLAYER_ALLOWED_HOSTS", "AGENTGATE_ALLOWED_HOSTS")
        .unwrap_or_default();
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !host_is_allowed(host, &hosts_allow) {
        return Err(GatewayError(
            AppError::new(
                crate::errors::codes::GATEWAY_HOST_REJECTED,
                format!("Host '{host}' is not allowed"),
            )
            .with_detail("仅接受 IP 字面量 / localhost 的 Host,防 DNS rebinding")
            .with_suggestion("如确需域名访问,设置 AGENTGATE_ALLOWED_HOSTS=your.host"),
        ));
    }
    let origins_allow =
        crate::compat::env_value("MUXLAYER_ALLOWED_ORIGINS", "AGENTGATE_ALLOWED_ORIGINS")
            .unwrap_or_default();
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !origin_is_allowed(origin, &origins_allow, &hosts_allow) {
        return Err(GatewayError(
            AppError::new(
                crate::errors::codes::GATEWAY_ORIGIN_REJECTED,
                format!("Origin '{origin}' is not allowed"),
            )
            .with_detail("网关默认拒绝浏览器跨站请求")
            .with_suggestion("如确需浏览器调用,设置 AGENTGATE_ALLOWED_ORIGINS=https://your.app"),
        ));
    }
    Ok(())
}

/// Host 是否放行:空(非浏览器客户端)/ localhost / IP 字面量 / 白名单。
/// DNS 名默认拒绝——rebinding 攻击的载体必然是攻击者可控的域名。
pub(crate) fn host_is_allowed(host: &str, allowlist: &str) -> bool {
    let h = host.trim();
    if h.is_empty() {
        return true;
    }
    let bare = strip_host_port(h);
    if bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if bare.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    allowlist.split(',').any(|a| {
        let a = a.trim();
        !a.is_empty() && (a.eq_ignore_ascii_case(bare) || a.eq_ignore_ascii_case(h))
    })
}

/// 去掉 Host 的端口部分。支持 `host:port`、`[v6]:port`、裸 IPv6。
fn strip_host_port(h: &str) -> &str {
    if let Some(rest) = h.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return &rest[..end];
        }
    }
    match h.rfind(':') {
        // 仅当冒号后全是数字、且前段不含冒号(排除裸 IPv6)时才视作端口。
        Some(i)
            if !h[i + 1..].is_empty()
                && h[i + 1..].bytes().all(|b| b.is_ascii_digit())
                && !h[..i].contains(':') =>
        {
            &h[..i]
        }
        _ => h,
    }
}

/// Origin 是否放行:空(非浏览器)/ 整条白名单 / host 部分按 host 规则。
pub(crate) fn origin_is_allowed(
    origin: &str,
    origin_allowlist: &str,
    host_allowlist: &str,
) -> bool {
    let o = origin.trim();
    if o.is_empty() {
        return true;
    }
    if origin_allowlist.split(',').any(|a| {
        let a = a.trim();
        !a.is_empty() && a.eq_ignore_ascii_case(o)
    }) {
        return true;
    }
    // 取 scheme:// 后的 authority;无 scheme(如 "null")一律拒。
    let Some((_, after)) = o.split_once("://") else {
        return false;
    };
    let authority = after.split('/').next().unwrap_or("");
    !authority.is_empty() && host_is_allowed(authority, host_allowlist)
}

pub(crate) fn validate_auth(headers: &HeaderMap) -> Result<(), GatewayError> {
    validate_request_boundary(headers)?;
    validate_token(headers)
}

/// 只校验本地 token,不看 Host/Origin。仅用于 /metrics:Prometheus 在 Docker 里按
/// 服务名抓取(Host 是域名),token 本身已足以挡住 DNS rebinding 读取。
pub(crate) fn validate_token(headers: &HeaderMap) -> Result<(), GatewayError> {
    // 1. Try standard Authorization: Bearer <token>
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let (token, source) = if auth_header.is_empty() {
        // 2. Fallback to x-api-key (used by some Anthropic SDK versions / Claude Code)
        let x_api_key = headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        (x_api_key, "x-api-key")
    } else {
        (
            auth_header.strip_prefix("Bearer ").unwrap_or(auth_header),
            "authorization",
        )
    };

    if token.is_empty() {
        return Err(GatewayError(
            AppError::new(
                crate::errors::codes::GATEWAY_AUTH_MISSING,
                "Gateway access token is missing",
            )
            .with_detail(
                "The request does not include Authorization: Bearer <token> or X-Api-Key <token>",
            )
            .with_suggestion(
                "Re-apply the tool configuration from MuxLayer or check the token file",
            ),
        ));
    }

    if !local_token::validate_token(token) {
        return Err(GatewayError(AppError::new(
            crate::errors::codes::GATEWAY_AUTH_INVALID,
            "Gateway access token is invalid",
        ).with_suggestion(format!("Token received via '{source}' header does not match. Regenerate the token and re-apply tool configuration"))));
    }

    Ok(())
}

pub(crate) fn request_body_or_gateway_error(
    body: Result<Bytes, BytesRejection>,
) -> Result<Bytes, GatewayError> {
    body.map_err(|e| {
        GatewayError(
            AppError::new(
                crate::errors::codes::REQUEST_BODY_TOO_LARGE,
                "请求内容过大，MuxLayer 无法接收本次上下文",
            )
            .with_detail(format!(
                "通常是对话历史太长、粘贴了大文件/日志、图片或工具输出过长。原始错误: {e}"
            ))
            .with_suggestion("请新开会话或减少本次发送内容；也可在设置里调大请求体上限，或设置 AGENTGATE_REQUEST_BODY_LIMIT_MB 后重启网关"),
        )
    })
}

pub(crate) fn get_active_provider(
    db: &crate::storage::db::DbPool,
) -> Result<Provider, GatewayError> {
    let conn = db
        .get()
        .map_err(|_| GatewayError(AppError::internal("DB lock failed")))?;
    let settings = crate::storage::gateway_settings::get(&conn)?;

    let provider_id = settings.active_provider_id.ok_or_else(|| {
        GatewayError(
            AppError::new(
                crate::errors::codes::ACTIVE_PROVIDER_NOT_FOUND,
                "No active provider configured",
            )
            .with_suggestion("Set an active provider in the Providers page"),
        )
    })?;

    let provider = crate::storage::providers::get_by_id(&conn, &provider_id).map_err(|_| {
        GatewayError(
            AppError::new(
                crate::errors::codes::ACTIVE_PROVIDER_NOT_FOUND,
                "Active provider not found in database",
            )
            .with_suggestion("Set a new active provider in the Providers page"),
        )
    })?;

    Ok(provider)
}

// Canonical virtual model is muxlayer. agentgate remains so old client configs keep working.
const VIRTUAL_MODEL: &str = "muxlayer";
const VIRTUAL_MODEL_LEGACY: &str = "agentgate";

pub(crate) fn native_model_override(
    provider: &Provider,
    requested_model: Option<&str>,
    resolved_model: Option<&str>,
) -> Option<String> {
    let requested = requested_model?.trim();
    if requested.is_empty() {
        return None;
    }

    if is_gateway_virtual_model(requested) {
        return Some(
            resolved_model
                .unwrap_or(&provider.default_model)
                .to_string(),
        );
    }

    explicit_model_mapping(provider, requested)
}

/// Resolve the model override for a request that may need a promoted vision
/// model. Explicit mappings remain authoritative for text requests, but an
/// image request must not map a vision-capable candidate back to a text-only
/// model.
pub(crate) fn native_model_override_for_images(
    provider: &Provider,
    requested_model: Option<&str>,
    resolved_model: Option<&str>,
    has_images: bool,
) -> Option<String> {
    let mapped = native_model_override(provider, requested_model, resolved_model);
    if !has_images {
        return mapped;
    }

    let Some(resolved) = resolved_model else {
        return mapped;
    };
    let capabilities = provider.parse_capabilities();
    let resolved_base = crate::transform::tool_calls::model_base(resolved);
    let resolved_is_vision = capabilities.get(resolved_base).is_some_and(|caps| {
        caps.iter()
            .any(|cap| cap == crate::providers::capabilities::CAP_VISION)
    });
    if resolved_is_vision
        && mapped
            .as_deref()
            .map(crate::transform::tool_calls::model_base)
            != Some(resolved_base)
    {
        return Some(resolved.to_string());
    }

    mapped
}

fn is_gateway_virtual_model(requested: &str) -> bool {
    let model = requested
        .rsplit_once('/')
        .map(|(_, model)| model)
        .unwrap_or(requested);
    model.eq_ignore_ascii_case(VIRTUAL_MODEL) || model.eq_ignore_ascii_case(VIRTUAL_MODEL_LEGACY)
}

fn explicit_model_mapping(provider: &Provider, requested: &str) -> Option<String> {
    let mapping = provider.model_mapping.as_ref()?;
    serde_json::from_str::<std::collections::HashMap<String, String>>(mapping)
        .ok()
        .and_then(|m| m.get(requested).cloned())
}

/// Check if the request contains image content anywhere in the conversation
/// (current turn or replayed history).
pub fn request_contains_images_pub(req: &ResponsesRequest) -> bool {
    request_contains_images(req)
}

pub(crate) fn request_contains_images(req: &ResponsesRequest) -> bool {
    fn content_has_images(v: &Value) -> bool {
        match v {
            Value::Array(arr) => arr.iter().any(|item| {
                let t = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                t == "input_image" || t == "image_url"
            }),
            _ => false,
        }
    }

    // 只看**最后一条** user message 是否含 image。历史 image 不算。
    //
    // 旧实现扫整个 history（"any historic image → promote"），为了避免 MiMo 上游
    // 看到 history image_url 后 404。但我们后来给 mimo.rs::finalize_request
    // 加了 image_url 自动剥离 + 注 OCR notice（#6 修复）兜底，404 不再发生。
    // 这条保护过时了，反而成了**副作用源**：
    //   - 用户某轮发过图 → 整个会话剩余请求被强制 promote 到 vision 模型
    //   - mimo-v2.5-pro (1M ctx) → mimo-v2.5 (128K ctx) 降级
    //   - 大会话进入 95%+ window 紧张区间 → 模型短回复 stop
    //
    // 第一性原理：vision 需求 = 模型现在需要看到一张图 = 当前 turn 有图。
    // 历史 image 已经被 strip 兜底，不需要为它牺牲 context window。
    match &req.input {
        Value::Array(items) => {
            // 找最后一条 user message（不是最后一条 message——尾部可能是
            // tool 结果或 function_call 等）
            let last_user_has_images = items
                .iter()
                .rev()
                .find(|item| {
                    item.get("type").and_then(|t| t.as_str()) == Some("message")
                        && item.get("role").and_then(|r| r.as_str()) == Some("user")
                })
                .and_then(|item| item.get("content"))
                .map(content_has_images)
                .unwrap_or(false);
            if last_user_has_images {
                return true;
            }

            // 新会话首条多模态输入可能直接是 content parts 数组：
            // [{"type":"input_text",...},{"type":"input_image",...}]
            // 这种格式没有 message wrapper，但仍是当前 user turn。
            let has_message_items = items
                .iter()
                .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("message"));
            !has_message_items && content_has_images(&req.input)
        }
        _ => false,
    }
}

/// 取 `body.messages` 里最后一条 `role=="user"` 的 content,判断是否含指定 type 的图片块。
/// 语义与 [`request_contains_images`] 对齐:只看当前 turn,历史图片不算
/// ——避免某轮发图后整个会话被强制路由到 vision 模型。
fn last_user_content_has_images(body: &Value, image_types: &[&str]) -> bool {
    let messages = match body.get("messages").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return false,
    };
    let last_user = messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"));
    let content = match last_user.and_then(|m| m.get("content")) {
        Some(c) => c,
        None => return false,
    };
    match content {
        Value::Array(arr) => arr.iter().any(|item| {
            let t = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            image_types.contains(&t)
        }),
        _ => false,
    }
}

/// Chat Completions 请求体最后一条 user message 是否含图片(`image_url`)。
#[cfg(test)]
pub(crate) fn chat_request_has_images(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .map(|v| chat_request_has_images_value(&v))
        .unwrap_or(false)
}

pub(crate) fn chat_request_has_images_value(body: &Value) -> bool {
    last_user_content_has_images(body, &["image_url"])
}

/// Anthropic Messages 请求体最后一条 user message 是否含图片(`image` block)。
#[cfg(test)]
pub(crate) fn anthropic_request_has_images(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .map(|v| anthropic_request_has_images_value(&v))
        .unwrap_or(false)
}

pub(crate) fn anthropic_request_has_images_value(body: &Value) -> bool {
    last_user_content_has_images(body, &["image"])
}

pub(crate) fn sanitize_body(body: &str) -> String {
    // 统一走 security::redaction(sk- / ag_local_ / Bearer / x-api-key /
    // api_key 字段 / ?key= 查询参数等)。之前只认 sk- 前缀,serve 模式下
    // 其他形态的密钥会原文落进 request_logs。
    // 上限与 request_logs write.rs 的 MAX_LOG_FIELD_BYTES(1MB)对齐:
    // 之前 50KB 会把 Codex gpt-5.6+ 的大 tools 定义切掉,排查时误导。
    truncate_str(&crate::security::redaction::redact_text(body), 1_000_000)
}

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    crate::gateway::stream_utf8::truncate_at_char_boundary(s, max).to_string()
}

pub(crate) fn trace_with_degradation_events(
    mut trace: serde_json::Value,
    events: &[crate::protocol::chat_completions::CapabilityDegradationEvent],
) -> String {
    if !events.is_empty() {
        trace["degradation_events"] = serde_json::json!(events);
    }
    trace.to_string()
}

fn protocol_for_log_route(route: &str) -> Option<&'static str> {
    match route {
        "/v1/responses" => Some("openai_responses"),
        "/v1/chat/completions" => Some("openai_chat_completions"),
        "/v1/messages" => Some("anthropic_messages"),
        _ => None,
    }
}

pub(crate) fn enrich_trace_with_route_decision(
    conn: &Connection,
    route: &str,
    provider_name: &str,
    model: &str,
    raw_request: &str,
    trace_json: Option<&str>,
) -> Option<String> {
    let mut trace = trace_json
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_else(|| json!({}));
    if trace.get("route_decision").is_some() {
        return Some(trace.to_string());
    }

    let protocol = protocol_for_log_route(route)?;
    let profile = crate::storage::route_profiles::get_default_for_protocol(conn, protocol)
        .ok()
        .flatten()?;
    let providers = crate::storage::route_profiles::list_providers(conn, &profile.id).ok()?;
    let selected = providers.iter().find(|p| p.provider_name == provider_name);
    // 每条日志都要走这里;raw_request 最大 1MB,整段反序列化只为判断有没有图太贵。
    // 图片块必然含 "input_image" / "image_url" 字面量,先做子串预检,绝大多数纯
    // 文本请求直接跳过解析。
    let request_has_images = route == "/v1/responses"
        && (raw_request.contains("input_image") || raw_request.contains("image_url"))
        && serde_json::from_str::<ResponsesRequest>(raw_request)
            .map(|req| request_contains_images(&req))
            .unwrap_or(false);
    let matched_conditions = selected
        .and_then(|p| p.routing_conditions.as_ref())
        .and_then(|s| serde_json::from_str::<Value>(s).ok());

    trace["route_decision"] = json!({
        "profile_id": profile.id,
        "profile_name": profile.name,
        "mode": profile.mode,
        "selected_provider_id": selected.map(|p| p.provider_id.as_str()),
        "selected_provider_name": provider_name,
        "selected_model": model,
        "selected_priority": selected.map(|p| p.priority),
        "matched_conditions": matched_conditions,
        "fallback_chain": route_fallback_chain(&providers, provider_name),
        "candidates": providers.iter().map(|p| {
            json!({
                "provider_id": p.provider_id,
                "provider_name": p.provider_name,
                "priority": p.priority,
                "model": p.model_override,
                "in_cooldown": p.cooldown_until.as_ref().map(|until| {
                    chrono::DateTime::parse_from_rfc3339(until)
                        .map(|cd| cd > chrono::Utc::now())
                        .unwrap_or(false)
                }).unwrap_or(false),
                "supports_vision": p.supports_vision,
                "has_conditions": p.routing_conditions.is_some(),
                "skip_reasons": route_candidate_skip_reasons(p, request_has_images),
            })
        }).collect::<Vec<_>>(),
    });
    Some(trace.to_string())
}

pub(crate) fn route_fallback_chain(
    providers: &[crate::models::route_profile::RouteProfileProviderView],
    selected_provider_name: &str,
) -> Vec<Value> {
    providers
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            json!({
                "provider_id": p.provider_id,
                "provider_name": p.provider_name,
                "priority": p.priority,
                "role": if idx == 0 { "primary" } else { "fallback" },
                "step": idx + 1,
                "selected": p.provider_name == selected_provider_name,
            })
        })
        .collect()
}

pub(crate) fn route_candidate_skip_reasons(
    provider: &crate::models::route_profile::RouteProfileProviderView,
    request_has_images: bool,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if !provider.enabled {
        reasons.push("disabled".to_string());
    }
    if !provider.runtime_available {
        reasons.push("runtime_unavailable".to_string());
    }
    let in_cooldown = provider
        .cooldown_until
        .as_ref()
        .map(|until| {
            chrono::DateTime::parse_from_rfc3339(until)
                .map(|cd| cd > chrono::Utc::now())
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if in_cooldown {
        reasons.push("cooldown".to_string());
    }
    if request_has_images && provider.supports_vision == Some(false) {
        reasons.push("unsupported_vision".to_string());
    }
    reasons
}

pub(crate) fn log_request_error(
    db: &crate::storage::db::DbPool,
    client_type: &str,
    route: &str,
    request_id: &str,
    raw_request: &str,
    converted_request: Option<&str>,
    err: &AppError,
    latency_ms: i64,
) {
    log_request_error_full(
        db,
        client_type,
        route,
        request_id,
        raw_request,
        converted_request.unwrap_or(""),
        "",
        "",
        err,
        if err.code == "RESPONSES_PARSE_ERROR" {
            400
        } else if err.code == "PROVIDER_API_KEY_MISSING" {
            401
        } else {
            500
        },
        latency_ms,
    );
}

/// 流式响应中途客户端断开时写入请求日志的状态码(沿用 nginx 的 499 约定),
/// 和上游失败(502)区分开,不污染 provider 健康统计。
pub(crate) const CLIENT_DISCONNECTED_STATUS: i64 = 499;

/// 客户端断开的日志标记错误。
pub(crate) fn client_disconnected_error() -> AppError {
    AppError::new(
        crate::errors::codes::CLIENT_DISCONNECTED,
        "client disconnected",
    )
    .with_detail("客户端在流式响应结束前断开,网关已停止读取上游")
}

/// SSE 转换任务返回的错误文本 → 日志用 AppError:客户端断开单独标记,其余是上游流错误。
pub(crate) fn stream_task_error(message: &str) -> AppError {
    if message == crate::gateway::sse::CLIENT_DISCONNECTED {
        client_disconnected_error()
    } else {
        AppError::new(crate::errors::codes::UPSTREAM_STREAM_ERROR, message)
    }
}

/// 流式任务结束时的错误 → 日志状态码:客户端断开记 499,其余按上游失败记 502。
pub(crate) fn stream_error_status(err: &AppError) -> i64 {
    if err.code == crate::errors::codes::CLIENT_DISCONNECTED {
        CLIENT_DISCONNECTED_STATUS
    } else {
        502
    }
}

/// 请求入口的日预算闸(同步 SQLite)放到 blocking 线程池里查。
/// 返回 true 表示需要强制最便宜候选。
pub(crate) async fn check_budget(db: &crate::storage::db::DbPool) -> Result<bool, GatewayError> {
    crate::runtime::db_blocking(db, crate::gateway::budget::check_new_request_with_conn)
        .await
        .map_err(GatewayError)
}

/// 选路:按 `protocols` 顺序找第一个能选出 provider 的路由档位(后一个是兜底),
/// 预算超限时重排为最便宜优先,全局兜底选路补上候选。一次 blocking 任务、
/// 一条连接完成,不占 tokio worker。
pub(crate) async fn select_providers(
    db: &crate::storage::db::DbPool,
    protocols: &'static [&'static str],
    requested_model: Option<String>,
    analysis: crate::gateway::provider_selector::RequestAnalysis,
    force_cheapest: bool,
) -> Result<crate::gateway::provider_selector::ProviderSelection, AppError> {
    crate::runtime::db_blocking(db, move |conn| {
        let mut result = Err(AppError::internal("no input protocol to select"));
        for protocol in protocols {
            result = crate::gateway::provider_selector::select_for_failover_with_conn(
                conn,
                protocol,
                requested_model.as_deref(),
                Some(&analysis),
            );
            if result.is_ok() {
                break;
            }
        }
        let mut selection = result?;
        if force_cheapest {
            if let Err(e) =
                crate::gateway::budget::apply_force_cheapest_with_conn(conn, &mut selection)
            {
                tracing::warn!(error = %e, "force_cheapest reorder failed; keeping route order");
            }
        }
        crate::gateway::failover::ensure_candidates(&mut selection);
        Ok(selection)
    })
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn log_request_success(
    db: &crate::storage::db::DbPool,
    client_type: &str,
    route: &str,
    request_id: &str,
    raw_request: &str,
    converted_request: &str,
    raw_response: &str,
    converted_response: &str,
    tool_calls: Option<&str>,
    provider: &str,
    model: &str,
    status_code: i64,
    latency_ms: i64,
    trace_json: Option<&str>,
    usage: crate::gateway::usage::TokenUsage,
) {
    let crate::gateway::usage::TokenUsage {
        input: input_tokens,
        output: output_tokens,
        cache_write: cache_write_tokens,
        cache_read: cache_read_tokens,
        input_semantics: _,
    } = usage;
    // Prometheus 指标(纯内存,留在调用线程)
    crate::gateway::metrics::record_request(
        route,
        client_type,
        provider,
        status_code as u16,
        latency_ms as f64 / 1000.0,
    );
    for (direction, tokens) in [
        ("input", input_tokens),
        ("output", output_tokens),
        ("cache_read", cache_read_tokens),
        ("cache_write", cache_write_tokens),
    ] {
        if let Some(t) = tokens {
            crate::gateway::metrics::record_tokens(provider, model, direction, t);
        }
    }

    // 定价查询 + route_profiles 查询 + 最多 6×1MB 的 INSERT 全部挪到 blocking
    // 线程池,不占 tokio worker;客户端不等日志落盘。
    let client_type = client_type.to_string();
    let route = route.to_string();
    let request_id = request_id.to_string();
    let raw_request = raw_request.to_string();
    let converted_request = converted_request.to_string();
    let raw_response = raw_response.to_string();
    let converted_response = converted_response.to_string();
    let tool_calls = tool_calls.map(str::to_string);
    let provider = provider.to_string();
    let model = model.to_string();
    let trace_json = trace_json.map(str::to_string);
    crate::runtime::db_blocking_detached(db, "log_request_success", move |conn| {
        let cost = crate::gateway::usage::cost_for_request(conn, &provider, &model, &usage);
        let trace_json = enrich_trace_with_route_decision(
            conn,
            &route,
            &provider,
            &model,
            &raw_request,
            trace_json.as_deref(),
        );
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
            Some(&converted_request),
            (!raw_response.is_empty()).then_some(raw_response.as_str()),
            (!converted_response.is_empty()).then_some(converted_response.as_str()),
            None,
            tool_calls.as_deref(),
            None,
            trace_json.as_deref(),
            input_tokens,
            output_tokens,
            cost,
            cache_write_tokens,
            cache_read_tokens,
            Some("gateway"),
            None,
            Some(&request_id),
        )
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn log_request_error_full(
    db: &crate::storage::db::DbPool,
    client_type: &str,
    route: &str,
    request_id: &str,
    raw_request: &str,
    converted_request: &str,
    provider: &str,
    model: &str,
    err: &AppError,
    status_code: i64,
    latency_ms: i64,
) {
    // Surface suggestion alongside the raw detail so users see actionable hints
    // (e.g. MiMo's "go activate the Web Search Plugin") right in the log card,
    // not buried in the JSON trace.
    let mut error_msg = format!("{}: {}", err.message, err.detail.as_deref().unwrap_or(""));
    if let Some(ref sug) = err.suggestion {
        error_msg.push_str("\n\n💡 ");
        error_msg.push_str(sug);
    }
    let trace = json!({
        "error_code": err.code,
        "suggestion": err.suggestion,
    })
    .to_string();
    let provider = if provider.is_empty() {
        "unknown"
    } else {
        provider
    };
    // Prometheus 指标（错误也算一次请求）
    crate::gateway::metrics::record_request(
        route,
        client_type,
        provider,
        status_code as u16,
        latency_ms as f64 / 1000.0,
    );

    let client_type = client_type.to_string();
    let route = route.to_string();
    let request_id = request_id.to_string();
    let raw_request = raw_request.to_string();
    let converted_request = converted_request.to_string();
    let provider = provider.to_string();
    let model = if model.is_empty() { "unknown" } else { model }.to_string();
    crate::runtime::db_blocking_detached(db, "log_request_error", move |conn| {
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
            (!converted_request.is_empty()).then_some(converted_request.as_str()),
            None,
            None,
            None,
            None,
            Some(&error_msg),
            Some(&trace),
            None,
            None,
            None, // no cost for errors
            None,
            None, // no cache tokens for errors
            Some("gateway"),
            None,
            Some(&request_id),
        )
    });
}

/// /v1/messages 的最终错误出口。流 bootstrap 识别出的上游错误帧(有分类状态码、
/// 没有原始 body)回 Anthropic 形态 `{"type":"error","error":{type,message}}`:
/// overloaded_error → 529、rate_limit_error → 429,其余用分类状态码(非 4xx/5xx 按 502),
/// 让 Claude Code 走自己的 overloaded / 限流重试。其它错误照旧交给 [`GatewayError`]。
pub(crate) fn messages_error_response(err: AppError) -> Result<Response, GatewayError> {
    let Some(status) = err
        .upstream
        .as_deref()
        .filter(|u| u.body.is_none())
        .map(|u| u.status)
    else {
        return Err(GatewayError(err));
    };
    let (frame_type, frame_message) = crate::gateway::sse_bootstrap::error_frame_fields(&err);
    let status = match frame_type.as_deref() {
        Some("overloaded_error") => 529,
        Some("rate_limit_error") => 429,
        _ if (400..600).contains(&status) => status,
        _ => 502,
    };
    let error_type = frame_type.unwrap_or_else(|| {
        match status {
            401 => "authentication_error",
            403 => "permission_error",
            413 => "request_too_large",
            429 => "rate_limit_error",
            529 => "overloaded_error",
            _ => "api_error",
        }
        .to_string()
    });
    let body = json!({
        "type": "error",
        "error": {
            "type": error_type,
            "message": frame_message.unwrap_or(err.message),
        }
    });
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    Ok((status, Json(body)).into_response())
}

// ── Error type for axum ────────────────────────────────────────

pub struct GatewayError(pub AppError);

impl From<AppError> for GatewayError {
    fn from(e: AppError) -> Self {
        Self(e)
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        // 直通上游的非 2xx:failover 用完后原样回上游状态码 + body,保持透传语义
        // (客户端 SDK 按上游原始错误格式解析)。1xx/3xx 不是合法的最终错误,按 502。
        if let Some(crate::errors::UpstreamFailure {
            status,
            body: Some(body),
        }) = self.0.upstream.map(|u| *u)
        {
            let status = StatusCode::from_u16(status)
                .ok()
                .filter(|s| s.is_client_error() || s.is_server_error())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            return Response::builder()
                .status(status)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body))
                .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
        }
        let status = match self.0.code.as_str() {
            "RESPONSES_PARSE_ERROR"
            | "TRANSFORM_ERROR"
            | "TOOL_OUTPUT_NOT_FOUND"
            | "TOOL_CALL_NOT_FOUND" => StatusCode::BAD_REQUEST,
            "PROVIDER_API_KEY_MISSING" | "GATEWAY_AUTH_MISSING" | "GATEWAY_AUTH_INVALID" => {
                StatusCode::UNAUTHORIZED
            }
            "REQUEST_BODY_TOO_LARGE" => StatusCode::PAYLOAD_TOO_LARGE,
            "ACTIVE_PROVIDER_NOT_FOUND" => StatusCode::SERVICE_UNAVAILABLE,
            c if c.starts_with("UPSTREAM") => StatusCode::BAD_GATEWAY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        // Build error message with detail for better client display
        let full_message = match &self.0.detail {
            Some(detail) if !detail.is_empty() => format!("{}: {}", self.0.message, detail),
            _ => self.0.message.clone(),
        };

        // Use OpenAI-compatible error format so clients (Codex, Claude Code, etc.)
        // can parse and display the error message correctly.
        // OpenAI expects: {"error": {"message": "...", "type": "...", "code": "..."}}
        let body = json!({
            "error": {
                "message": full_message,
                "type": self.0.code,
                "code": self.0.code,
                "detail": self.0.detail,
                "suggestion": self.0.suggestion,
            }
        });

        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;
    use serde_json::{json, Value};

    use crate::errors::AppError;
    use crate::models::gateway::UpdateGatewaySettingsInput;
    use crate::models::provider::Provider;
    use crate::models::route_profile::RouteProfileProviderView;
    use crate::protocol::openai_responses::ResponsesRequest;
    use crate::security::local_token;
    use crate::storage::migrations::run_migrations;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    fn test_provider() -> Provider {
        Provider {
            id: "p1".to_string(),
            name: "Test".to_string(),
            provider_type: "deepseek".to_string(),
            base_url: "https://api.test.com".to_string(),
            api_key: Some("sk-test".to_string()),
            default_model: "test-model".to_string(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".to_string(),
            timeout_seconds: 60,
            status: "active".to_string(),
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
            is_active: true,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        }
    }

    fn provider_with_mapping() -> Provider {
        Provider {
            model_mapping: Some(r#"{"gpt-5.5":"deepseek-v4-pro"}"#.to_string()),
            default_model: "deepseek-flash".to_string(),
            ..test_provider()
        }
    }

    fn responses_req_with_input(input: Value) -> ResponsesRequest {
        ResponsesRequest {
            model: Some("gpt-5".into()),
            input,
            instructions: None,
            system: None,
            previous_response_id: None,
            tools: None,
            tool_choice: None,
            stream: Some(false),
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            parallel_tool_calls: None,
            reasoning: None,
            text: None,
            metadata: None,
            seed: None,
            stop: None,
            frequency_penalty: None,
            presence_penalty: None,
            extra: Default::default(),
        }
    }

    fn memory_pool_with_refiners() -> crate::storage::db::DbPool {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder().max_size(1).build(manager).unwrap();
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        crate::storage::gateway_settings::update(
            &conn,
            UpdateGatewaySettingsInput {
                body_filter_global: Some(true),
                thinking_rectifier_global: Some(true),
                error_mapper_global: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);
        pool
    }

    // ── detect_client_from_ua ──

    #[test]
    fn detect_client_from_ua_empty_uses_route_default() {
        let headers = HeaderMap::new();
        assert_eq!(detect_client_from_ua(&headers, "Codex"), "Codex");
    }

    #[test]
    fn detect_client_from_ua_codex_patterns() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "codex-cli/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Codex");

        let mut h = HeaderMap::new();
        h.insert("user-agent", "codex/1.0.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Codex");

        let mut h = HeaderMap::new();
        h.insert("user-agent", "OpenAI/Python 1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "OpenAI SDK");

        let mut h = HeaderMap::new();
        h.insert("user-agent", "OpenAI/Python 1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Codex"), "Codex");
    }

    #[test]
    fn detect_client_from_ua_claude_code() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "claude-code/0.1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Claude Code");
    }

    #[test]
    fn detect_client_from_ua_kimi() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "KimiCLI/1.40.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Kimi CLI");
    }

    #[test]
    fn detect_client_from_ua_grok_build() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "grok-build/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Grok Build");
    }

    #[test]
    fn detect_client_from_ua_deepseek_harness() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "dsh/0.1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "DeepSeek Harness");
    }

    #[test]
    fn detect_client_from_ua_cursor() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "Cursor/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Cursor");
    }

    #[test]
    fn detect_client_from_ua_openai_sdk() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "openai-python/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "OpenAI SDK");
    }

    #[test]
    fn detect_client_from_ua_anthropic_sdk() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "anthropic-sdk/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Anthropic SDK");
    }

    #[test]
    fn detect_client_from_ua_python_and_node_sdks() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "python-requests/2.28".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Python SDK");

        let mut h = HeaderMap::new();
        h.insert("user-agent", "node-fetch/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "Node SDK");
    }

    #[test]
    fn detect_client_from_ua_curl() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "curl/7.64.1".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "curl");
    }

    #[test]
    fn detect_client_from_ua_unknown_returns_first_token() {
        let mut h = HeaderMap::new();
        h.insert("user-agent", "MyCustomAgent/1.0 extra".parse().unwrap());
        assert_eq!(detect_client_from_ua(&h, "Default"), "MyCustomAgent/1.0");
    }

    // ── validate_request_boundary / host_is_allowed / origin_is_allowed ──

    #[test]
    fn host_is_allowed_accepts_empty_ip_localhost() {
        assert!(host_is_allowed("", ""));
        assert!(host_is_allowed("127.0.0.1", ""));
        assert!(host_is_allowed("127.0.0.1:9090", ""));
        assert!(host_is_allowed("localhost", ""));
        assert!(host_is_allowed("localhost:9090", ""));
        assert!(host_is_allowed("[::1]:9090", ""));
        assert!(host_is_allowed("::1", ""));
    }

    #[test]
    fn host_is_allowed_rejects_dns_without_whitelist() {
        assert!(!host_is_allowed("evil.com", ""));
        assert!(!host_is_allowed("evil.com:9090", ""));
    }

    #[test]
    fn host_is_allowed_respects_whitelist() {
        assert!(host_is_allowed("my.app", "my.app"));
        assert!(host_is_allowed("my.app:9090", "my.app"));
        assert!(host_is_allowed(
            "a.example.com",
            "b.example.com, a.example.com"
        ));
    }

    #[test]
    fn origin_is_allowed_accepts_empty_localhost_ip_and_whitelist() {
        assert!(origin_is_allowed("", "", ""));
        assert!(origin_is_allowed("http://localhost:1420", "", ""));
        assert!(origin_is_allowed("http://127.0.0.1:1420", "", ""));
        assert!(origin_is_allowed("tauri://localhost", "", ""));
        assert!(origin_is_allowed("https://my.app", "https://my.app", ""));
    }

    #[test]
    fn origin_is_allowed_rejects_cross_site_and_null() {
        assert!(!origin_is_allowed("https://evil.com", "", ""));
        assert!(!origin_is_allowed("null", "", ""));
        assert!(!origin_is_allowed(
            "https://evil.com",
            "https://other.app",
            ""
        ));
    }

    #[test]
    #[serial_test::serial(env)]
    fn validate_request_boundary_allows_localhost_and_rejects_dns() {
        std::env::remove_var("AGENTGATE_ALLOWED_HOSTS");
        std::env::remove_var("AGENTGATE_ALLOWED_ORIGINS");

        let mut headers = HeaderMap::new();
        headers.insert("host", "127.0.0.1:9090".parse().unwrap());
        assert!(validate_request_boundary(&headers).is_ok());

        let mut headers = HeaderMap::new();
        headers.insert("host", "evil.com".parse().unwrap());
        let err = validate_request_boundary(&headers).unwrap_err();
        assert_eq!(err.0.code, "GATEWAY_HOST_REJECTED");
    }

    // ── validate_auth ──

    #[test]
    fn validate_auth_missing() {
        let headers = HeaderMap::new();
        let err = validate_auth(&headers).unwrap_err();
        assert_eq!(err.0.code, "GATEWAY_AUTH_MISSING");
    }

    #[test]
    fn validate_auth_invalid_format() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let _ = local_token::ensure_token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "not-a-valid-token".parse().unwrap());
        let err = validate_auth(&headers).unwrap_err();
        assert_eq!(err.0.code, "GATEWAY_AUTH_INVALID");
        cleanup(&temp);
    }

    #[test]
    fn validate_auth_valid_bearer() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let token = local_token::ensure_token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        assert!(validate_auth(&headers).is_ok());
        cleanup(&temp);
    }

    #[test]
    fn validate_auth_valid_x_api_key() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let token = local_token::ensure_token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", token.parse().unwrap());
        assert!(validate_auth(&headers).is_ok());
        cleanup(&temp);
    }

    // ── sanitize_body / truncate_str ──

    #[test]
    fn truncate_str_ascii_and_multibyte_boundaries() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello");
        let s = "你好世界";
        assert_eq!(truncate_str(s, 4), "你");
        assert_eq!(truncate_str(s, 5), "你");
        assert_eq!(truncate_str(s, 6), "你好");
        assert_eq!(truncate_str(s, 100), s);
    }

    #[test]
    fn sanitize_body_redacts_sensitive_values() {
        let body = r#"{"key": "sk-abcdefghij1234567890", "x-api-key": "supersecretvalue123"}"#;
        let sanitized = sanitize_body(body);
        assert!(!sanitized.contains("abcdefghij1234567890"));
        assert!(!sanitized.contains("supersecretvalue123"));
        assert!(sanitized.contains("sk-"));
    }

    #[test]
    fn sanitize_body_keeps_bodies_up_to_1mb() {
        // Codex gpt-5.6+ 的 tools 定义就超过 50KB,50KB 截断会让日志里的
        // raw_request 缺失关键排查信息(工具"看起来不存在"其实是被切了)。
        // 与 request_logs write.rs 的 1MB 上限对齐。
        let body = "a".repeat(200_000);
        let sanitized = sanitize_body(&body);
        assert_eq!(sanitized.len(), 200_000);
    }

    #[test]
    fn sanitize_body_truncates_beyond_1mb() {
        let body = "a".repeat(1_100_000);
        let sanitized = sanitize_body(&body);
        assert!(sanitized.len() <= 1_000_000);
    }

    // ── native_model_override ──

    #[test]
    fn native_model_override_muxlayer_virtual_model() {
        let provider = provider_with_mapping();
        assert_eq!(
            native_model_override(&provider, Some("muxlayer"), None),
            Some("deepseek-flash".to_string())
        );
        assert_eq!(
            native_model_override(&provider, Some("openai/muxlayer"), None),
            Some("deepseek-flash".to_string())
        );
        assert_eq!(
            native_model_override(&provider, Some("muxlayer"), Some("deepseek-v4-pro")),
            Some("deepseek-v4-pro".to_string())
        );
    }

    #[test]
    fn native_model_override_agentgate_virtual_model_alias() {
        let provider = provider_with_mapping();
        assert_eq!(
            native_model_override(&provider, Some("agentgate"), None),
            Some("deepseek-flash".to_string())
        );
        assert_eq!(
            native_model_override(&provider, Some("openai/agentgate"), None),
            Some("deepseek-flash".to_string())
        );
        assert_eq!(
            native_model_override(&provider, Some("agentgate"), Some("deepseek-v4-pro")),
            Some("deepseek-v4-pro".to_string())
        );
    }

    #[test]
    fn native_model_override_explicit_mapping_wins_and_unmapped_returns_none() {
        let provider = provider_with_mapping();
        assert_eq!(
            native_model_override(&provider, Some("gpt-5.5"), Some("deepseek-flash")),
            Some("deepseek-v4-pro".to_string())
        );
        assert_eq!(
            native_model_override(&provider, Some("mimo-v2.5"), Some("deepseek-flash")),
            None
        );
        assert_eq!(native_model_override(&provider, Some(""), None), None);
        assert_eq!(native_model_override(&provider, None, None), None);
    }

    #[test]
    fn image_request_prefers_promoted_vision_model_over_text_mapping() {
        let mut provider = provider_with_mapping();
        provider.supported_models = Some(r#"["deepseek-v4-pro","deepseek-flash"]"#.to_string());
        provider.model_capabilities = Some(
            r#"{
                "deepseek-v4-pro":["text","reasoning","tools"],
                "deepseek-flash":["text","reasoning","tools","vision"]
            }"#
            .to_string(),
        );

        assert_eq!(
            native_model_override_for_images(
                &provider,
                Some("gpt-5.5"),
                Some("deepseek-flash"),
                true,
            ),
            Some("deepseek-flash".to_string())
        );
    }

    #[test]
    fn text_request_keeps_explicit_model_mapping() {
        let provider = provider_with_mapping();
        assert_eq!(
            native_model_override_for_images(
                &provider,
                Some("gpt-5.5"),
                Some("deepseek-flash"),
                false,
            ),
            Some("deepseek-v4-pro".to_string())
        );
    }

    // ── chat_request_has_images / request_contains_images ──

    #[test]
    fn request_contains_images_current_turn_only() {
        let text_only = responses_req_with_input(json!([
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "hi"}
            ]}
        ]));
        assert!(!request_contains_images(&text_only));

        let current_image = responses_req_with_input(json!([
            {"type": "message", "role": "user", "content": [
                {"type": "input_image", "image_url": {"url": "x"}}
            ]}
        ]));
        assert!(request_contains_images(&current_image));
    }

    #[test]
    fn request_contains_images_ignores_historic_image() {
        let req = responses_req_with_input(json!([
            {"type": "message", "role": "user", "content": [
                {"type": "input_image", "image_url": {"url": "x"}}
            ]},
            {"type": "message", "role": "assistant", "content": "ok"},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "and now"}
            ]}
        ]));
        assert!(!request_contains_images(&req));
    }

    #[test]
    fn request_contains_images_top_level_content_parts() {
        let req = responses_req_with_input(json!([
            {"type": "input_text", "text": "describe this"},
            {"type": "input_image", "image_url": {"url": "x"}}
        ]));
        assert!(request_contains_images(&req));
    }

    #[test]
    fn chat_request_has_images_current_turn_only() {
        let current_image = json!({"messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "what"},
                {"type": "image_url", "image_url": {"url": "x"}}
            ]}
        ]})
        .to_string();
        assert!(chat_request_has_images(&current_image));

        let historic_image = json!({"messages": [
            {"role": "user", "content": [{"type": "image_url", "image_url": {"url": "x"}}]},
            {"role": "assistant", "content": "ok"},
            {"role": "user", "content": [{"type": "text", "text": "and now"}]}
        ]})
        .to_string();
        assert!(!chat_request_has_images(&historic_image));

        let text_only = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "hi"}]}
        ]})
        .to_string();
        assert!(!chat_request_has_images(&text_only));
    }

    // ── route_fallback_chain / route_candidate_skip_reasons ──

    #[test]
    fn route_fallback_chain_marks_roles_and_selected() {
        let mk = |priority: i64, name: &str| RouteProfileProviderView {
            id: format!("rpp{priority}"),
            provider_id: format!("p{priority}"),
            provider_name: name.into(),
            provider_type: "openai".into(),
            provider_protocol: "openai_responses".into(),
            has_anthropic_url: false,
            supports_vision: None,
            model_capabilities: None,
            priority,
            enabled: true,
            model_override: None,
            cooldown_seconds: 600,
            failover_on_status_codes: None,
            failover_on_error_keywords: None,
            routing_conditions: None,
            runtime_available: true,
            cooldown_until: None,
            consecutive_failures: 0,
        };
        let providers = vec![mk(1, "Primary"), mk(2, "Backup")];
        let chain = route_fallback_chain(&providers, "Backup");
        assert_eq!(chain[0]["role"], "primary");
        assert_eq!(chain[1]["role"], "fallback");
        assert_eq!(chain[0]["step"], 1);
        assert_eq!(chain[1]["step"], 2);
        assert_eq!(chain[1]["selected"], true);
        assert_eq!(chain[0]["selected"], false);
    }

    #[test]
    fn route_candidate_skip_reasons_collects_skip_reasons() {
        let provider = RouteProfileProviderView {
            id: "rpp1".into(),
            provider_id: "p1".into(),
            provider_name: "NoVision".into(),
            provider_type: "openai".into(),
            provider_protocol: "openai_responses".into(),
            has_anthropic_url: false,
            supports_vision: Some(false),
            model_capabilities: None,
            priority: 1,
            enabled: false,
            model_override: None,
            cooldown_seconds: 600,
            failover_on_status_codes: None,
            failover_on_error_keywords: None,
            routing_conditions: None,
            runtime_available: false,
            cooldown_until: Some((chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()),
            consecutive_failures: 3,
        };
        let reasons = route_candidate_skip_reasons(&provider, true);
        assert!(reasons.contains(&"disabled".to_string()));
        assert!(reasons.contains(&"runtime_unavailable".to_string()));
        assert!(reasons.contains(&"cooldown".to_string()));
        assert!(reasons.contains(&"unsupported_vision".to_string()));
    }

    // ── refine_value_body / refine_struct_body ──

    #[test]
    fn refine_value_body_no_op_when_db_unavailable() {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder()
            .max_size(1)
            .connection_timeout(Duration::from_millis(100))
            .build(manager)
            .unwrap();
        // Hold the only connection on another thread so refine_value_body cannot acquire one.
        let pool2 = pool.clone();
        let handle = std::thread::spawn(move || {
            let _conn = pool2.get().unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
        std::thread::sleep(Duration::from_millis(50));
        let mut body = json!({"web_search": true});
        let log = refine_value_body(&pool, &test_provider(), &mut body);
        assert!(log.is_empty());
        assert!(body.get("web_search").is_some());
        handle.join().unwrap();
    }

    #[test]
    fn refine_value_body_applies_refiners_when_settings_available() {
        let pool = memory_pool_with_refiners();
        let mut provider = test_provider();
        provider.provider_type = "deepseek".to_string();
        let mut body = json!({"web_search": true, "model": "m"});
        let log = refine_value_body(&pool, &provider, &mut body);
        assert!(log.body_filter.is_some());
        assert!(body.get("web_search").is_none());
    }

    #[test]
    fn refine_struct_body_applies_refiners_and_deserializes_back() {
        let pool = memory_pool_with_refiners();
        let mut provider = test_provider();
        provider.provider_type = "deepseek".to_string();

        #[derive(serde::Serialize, serde::Deserialize, Debug)]
        struct Body {
            #[serde(default)]
            pub web_search: bool,
            pub model: String,
        }

        let mut req = Body {
            web_search: true,
            model: "m".to_string(),
        };
        let log = refine_struct_body(&pool, &provider, &mut req);
        assert!(log.body_filter.is_some());
        assert!(!req.web_search);
        assert_eq!(req.model, "m");
    }

    // ── GatewayError IntoResponse ──

    #[test]
    fn gateway_error_maps_status_codes() {
        assert_eq!(
            GatewayError(AppError::new(
                crate::errors::codes::RESPONSES_PARSE_ERROR,
                "bad"
            ))
            .into_response()
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            GatewayError(AppError::new(
                crate::errors::codes::PROVIDER_API_KEY_MISSING,
                "no key"
            ))
            .into_response()
            .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            GatewayError(AppError::new(
                crate::errors::codes::ACTIVE_PROVIDER_NOT_FOUND,
                "none"
            ))
            .into_response()
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            GatewayError(AppError::new("UPSTREAM_STREAM_ERROR", "err"))
                .into_response()
                .status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            GatewayError(AppError::new("REQUEST_BODY_TOO_LARGE", "too large"))
                .into_response()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            GatewayError(AppError::new("UNKNOWN_CODE", "err"))
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn gateway_error_body_has_openai_shape() {
        let err = GatewayError(
            AppError::new(crate::errors::codes::GATEWAY_AUTH_INVALID, "bad token")
                .with_detail("token mismatch")
                .with_suggestion("regenerate"),
        );
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
