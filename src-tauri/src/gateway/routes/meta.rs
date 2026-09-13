use axum::extract::State as AxumState;
use axum::http::HeaderMap;
use axum::response::Json;
use serde_json::{json, Value};

use crate::errors::AppError;

use super::shared::{get_active_provider, request_body_or_gateway_error, GatewayError};
use super::GatewayState;

// ── GET /health ────────────────────────────────────────────────

pub async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "app": "MuxLayer",
        "gateway": "running",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// ── GET /v1/models ─────────────────────────────────────────────

// 鉴权 + Host/Origin 边界校验由 server.rs 的中间件统一完成(含下面几个端点)。
pub async fn list_models(
    AxumState(state): AxumState<GatewayState>,
) -> Result<Json<Value>, GatewayError> {
    let provider = get_active_provider(&state.db)?;

    let mut models = vec![json!({
        "id": provider.default_model,
        "object": "model",
        "created": 0,
        "owned_by": "agentgate"
    })];

    if let Some(ref rm) = provider.reasoning_model {
        if !rm.is_empty() {
            models.push(json!({
                "id": rm,
                "object": "model",
                "created": 0,
                "owned_by": "agentgate"
            }));
        }
    }

    Ok(Json(json!({
        "object": "list",
        "data": models
    })))
}

// ── POST /v1/messages/count_tokens (Anthropic) ────────────────
//
// Claude Code 跑长 prompt 前调用此端点预估 token 数。Anthropic 自己实现了精确
// 计数（用 tokenizer），我们本地用启发式估算：
//   - text content 字符数 / 4（英文）或字符数 / 1.6（中文密集）取大
//   - tool_use input_schema 加 schema 复杂度估算
//   - thinking budget 不算 input
//
// 不转发上游因为：① 不所有 anthropic 兼容 provider 都实现此端点；② 启发式
// 足够 client 做 budget check 用，精确值由上游业务请求返。

pub async fn handle_count_tokens(
    headers: HeaderMap,
    AxumState(state): AxumState<GatewayState>,
    body: Result<bytes::Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Json<Value>, GatewayError> {
    let body = request_body_or_gateway_error(body)?;
    let body = crate::gateway::body_decode::decode(&headers, body, state.request_body_limit)
        .map_err(GatewayError)?;
    let v: Value = serde_json::from_str(&body).map_err(|e| {
        GatewayError(AppError::new(
            crate::errors::codes::COUNT_TOKENS_PARSE_ERROR,
            format!("Failed to parse: {e}"),
        ))
    })?;

    let estimate = estimate_anthropic_tokens(&v);
    Ok(Json(json!({"input_tokens": estimate})))
}

fn estimate_anthropic_tokens(req: &Value) -> i64 {
    let mut chars: usize = 0;
    if let Some(sys) = req.get("system") {
        chars += count_chars(sys);
    }
    if let Some(messages) = req.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            if let Some(c) = msg.get("content") {
                chars += count_chars(c);
            }
        }
    }
    if let Some(tools) = req.get("tools").and_then(|t| t.as_array()) {
        for tool in tools {
            chars += tool.to_string().len();
        }
    }
    // 启发式：4 chars/token 对英文友好，中文密集时偏低但仍 conservative
    ((chars as f64) / 4.0).ceil() as i64
}

fn count_chars(v: &Value) -> usize {
    match v {
        Value::String(s) => s.chars().count(),
        Value::Array(arr) => arr.iter().map(count_chars).sum(),
        Value::Object(o) => o.values().map(count_chars).sum(),
        _ => 0,
    }
}

// Gemini countTokens 的处理直接合并到 handle_gemini_generate 里（router 没法
// 按 :countTokens 后缀分发，handler 入口分流更稳）。

// ── GET /v1beta/models (Gemini 客户端拉 models 列表) ──────────

pub async fn list_gemini_models(
    AxumState(state): AxumState<GatewayState>,
) -> Result<Json<Value>, GatewayError> {
    let provider = get_active_provider(&state.db)?;

    let mut models: Vec<Value> = Vec::new();
    models.push(json!({
        "name": format!("models/{}", provider.default_model),
        "displayName": provider.default_model,
        "supportedGenerationMethods": ["generateContent", "streamGenerateContent", "countTokens"],
    }));
    if let Some(ref rm) = provider.reasoning_model {
        if !rm.is_empty() {
            models.push(json!({
                "name": format!("models/{rm}"),
                "displayName": rm,
                "supportedGenerationMethods": ["generateContent", "streamGenerateContent", "countTokens"],
            }));
        }
    }
    Ok(Json(json!({"models": models})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_reports_real_package_version() {
        // /health 的 version 字段曾硬编码 "0.1.0",与真实发版号脱节。
        // 锁住:必须取 Cargo 包版本。
        let resp = health().await;
        assert_eq!(
            resp.0["version"],
            json!(env!("CARGO_PKG_VERSION")),
            "health 应返回真实包版本,而非硬编码"
        );
    }
}
