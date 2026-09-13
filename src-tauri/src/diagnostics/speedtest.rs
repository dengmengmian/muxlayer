//! Speedtest — manual provider latency probe.
//!
//! User-triggered (never automatic — costs tokens) measurement of how long
//! a provider takes to respond. Sends a minimal probe to each provider's
//! configured endpoint:
//!   - Anthropic-style providers: a 1-token `messages` request.
//!   - OpenAI-Chat / Responses providers: a 1-token chat/completion.
//!   - Falls back to `HEAD /` when none of the above is configured.
//!
//! Returns three timing numbers per provider:
//!   - **DNS + connect + TLS**: phase to establish the connection (proxy is
//!     transparent to this number).
//!   - **TTFB**: time from first byte received.
//!   - **Total**: full response received.
//!
//! Output is structured (`ProviderSpeedReport`) and pure — caller decides
//! whether to render in GUI, log to file, or store in DB.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[cfg(feature = "desktop")]
use crate::errors::AppError;
use crate::models::provider::Provider;

const PROBE_PROMPT: &str = "hi";
const PROBE_MAX_TOKENS: u32 = 1;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ProviderSpeedReport {
    pub provider_id: String,
    pub provider_name: String,
    pub endpoint: String,
    pub status_code: Option<u16>,
    pub connect_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub total_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}

impl ProviderSpeedReport {
    fn failure(provider: &Provider, endpoint: String, total: Duration, error: String) -> Self {
        Self {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            endpoint,
            status_code: None,
            connect_ms: None,
            ttfb_ms: None,
            total_ms: total.as_millis() as u64,
            success: false,
            error: Some(error),
        }
    }
}

/// Build the probe URL and request body for the provider's preferred shape.
/// Returns (url, body, content_type, auth_header_kind).
fn probe_request(provider: &Provider) -> (String, serde_json::Value, &'static str) {
    let protocols = provider.protocols();
    if protocols.iter().any(|p| p == "anthropic_messages") {
        let base = provider
            .anthropic_base_url
            .clone()
            .unwrap_or_else(|| trim_trailing_slash(&provider.base_url));
        let url = format!("{}/v1/messages", trim_trailing_slash(&base));
        let body = serde_json::json!({
            "model": provider.default_model,
            "max_tokens": PROBE_MAX_TOKENS,
            "messages": [{"role": "user", "content": PROBE_PROMPT}]
        });
        return (url, body, "anthropic_messages");
    }
    if protocols.iter().any(|p| p == "openai_responses") {
        let base = provider
            .responses_base_url
            .clone()
            .unwrap_or_else(|| trim_trailing_slash(&provider.base_url));
        let url = format!("{}/v1/responses", trim_trailing_slash(&base));
        let body = serde_json::json!({
            "model": provider.default_model,
            "input": PROBE_PROMPT,
            "max_output_tokens": PROBE_MAX_TOKENS,
        });
        return (url, body, "openai_responses");
    }
    // Default to OpenAI chat completions.
    let url = format!(
        "{}/v1/chat/completions",
        trim_trailing_slash(&provider.base_url)
    );
    let body = serde_json::json!({
        "model": provider.default_model,
        "max_tokens": PROBE_MAX_TOKENS,
        "messages": [{"role": "user", "content": PROBE_PROMPT}]
    });
    (url, body, "openai_chat_completions")
}

fn trim_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

/// Run the probe for one provider. Pure async function — no DB I/O.
pub async fn probe(provider: &Provider) -> ProviderSpeedReport {
    let start = Instant::now();
    let (url, body, shape) = probe_request(provider);

    let raw_key = match provider.api_key.as_deref() {
        Some(k) if !k.is_empty() => k,
        _ => {
            return ProviderSpeedReport::failure(
                provider,
                url,
                start.elapsed(),
                "Provider has no API key configured".into(),
            );
        }
    };
    // Match adapter.rs: api_key may be JSON array; take the first non-empty key.
    let api_key = parse_first_key(raw_key);
    if api_key.is_empty() {
        return ProviderSpeedReport::failure(
            provider,
            url,
            start.elapsed(),
            "API key is empty".into(),
        );
    }

    let timeout = Duration::from_secs(provider.timeout_seconds as u64).min(DEFAULT_TIMEOUT);
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(e) => {
            return ProviderSpeedReport::failure(provider, url, start.elapsed(), e.to_string())
        }
    };

    let mut req = client.post(&url).json(&body);
    match shape {
        "anthropic_messages" => {
            req = req
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01");
        }
        _ => {
            req = req.header("Authorization", format!("Bearer {api_key}"));
        }
    }
    if let Some(extra) = provider.extra_headers.as_deref() {
        if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, String>>(extra) {
            for (k, v) in map {
                req = req.header(k, v);
            }
        }
    }

    let connect_start = Instant::now();
    let response = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return ProviderSpeedReport::failure(provider, url, start.elapsed(), e.to_string())
        }
    };
    let connect_ms = connect_start.elapsed().as_millis() as u64;
    let status = response.status().as_u16();

    let ttfb_start = Instant::now();
    let body_bytes = response.bytes().await;
    let ttfb_ms = ttfb_start.elapsed().as_millis() as u64;
    let total = start.elapsed();

    let success = (200..400).contains(&status);
    let error = if !success {
        Some(
            body_bytes
                .ok()
                .and_then(|b| String::from_utf8(b.to_vec()).ok())
                .map(|s| {
                    // Truncate noisy upstream bodies so the report stays
                    // readable in the GUI. 200 chars is enough to spot
                    // the error class.
                    if s.len() > 200 {
                        format!("{}…", &s[..200])
                    } else {
                        s
                    }
                })
                .unwrap_or_else(|| format!("HTTP {status}")),
        )
    } else if body_bytes.is_err() {
        Some(body_bytes.unwrap_err().to_string())
    } else {
        None
    };

    ProviderSpeedReport {
        provider_id: provider.id.clone(),
        provider_name: provider.name.clone(),
        endpoint: url,
        status_code: Some(status),
        connect_ms: Some(connect_ms),
        ttfb_ms: Some(ttfb_ms),
        total_ms: total.as_millis() as u64,
        success,
        error,
    }
}

/// Probe many providers in parallel. Returns a vec in the same order as the
/// input. Errors per-provider are captured in the report's `error` field;
/// the function itself only returns Err for catastrophic setup failure
/// (currently: nothing — kept for API symmetry).
/// 只被桌面端(测速命令 / 后台健康探测)使用,cli 构建不编译。
#[cfg(feature = "desktop")]
pub async fn probe_many(providers: &[Provider]) -> Result<Vec<ProviderSpeedReport>, AppError> {
    let futures: Vec<_> = providers.iter().map(probe).collect();
    Ok(futures::future::join_all(futures).await)
}

fn parse_first_key(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') {
        if let Ok(keys) = serde_json::from_str::<Vec<String>>(trimmed) {
            for k in keys {
                if !k.trim().is_empty() {
                    return k;
                }
            }
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(provider_type: &str, protocols: &str) -> Provider {
        Provider {
            id: "p".into(),
            name: "P".into(),
            provider_type: provider_type.into(),
            base_url: "https://example.test".into(),
            api_key: Some("sk-key".into()),
            default_model: "m1".into(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: protocols.into(),
            timeout_seconds: 30,
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
            is_active: true,
            created_at: "now".into(),
            updated_at: "now".into(),
        }
    }

    #[test]
    fn probe_request_anthropic_uses_messages_endpoint() {
        let p = make_provider("anthropic", r#"["anthropic_messages"]"#);
        let (url, _, shape) = probe_request(&p);
        assert!(url.ends_with("/v1/messages"));
        assert_eq!(shape, "anthropic_messages");
    }

    #[test]
    fn probe_request_responses_uses_responses_endpoint() {
        let p = make_provider("openai", r#"["openai_responses"]"#);
        let (url, _, shape) = probe_request(&p);
        assert!(url.ends_with("/v1/responses"));
        assert_eq!(shape, "openai_responses");
    }

    #[test]
    fn probe_request_falls_back_to_chat_completions() {
        let p = make_provider("custom", r#"["openai_chat_completions"]"#);
        let (url, _, shape) = probe_request(&p);
        assert!(url.ends_with("/v1/chat/completions"));
        assert_eq!(shape, "openai_chat_completions");
    }

    #[test]
    fn probe_request_uses_anthropic_base_url_when_set() {
        let mut p = make_provider(
            "deepseek",
            r#"["anthropic_messages","openai_chat_completions"]"#,
        );
        p.anthropic_base_url = Some("https://api.deepseek.com/anthropic".into());
        let (url, _, _) = probe_request(&p);
        assert_eq!(url, "https://api.deepseek.com/anthropic/v1/messages");
    }

    #[test]
    fn probe_request_strips_trailing_slash_in_base_url() {
        let mut p = make_provider("openai", r#"["openai_chat_completions"]"#);
        p.base_url = "https://api.openai.com/".into();
        let (url, _, _) = probe_request(&p);
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn parse_first_key_returns_plain_string() {
        assert_eq!(parse_first_key("sk-abc"), "sk-abc");
    }

    #[test]
    fn parse_first_key_returns_first_from_array() {
        assert_eq!(parse_first_key(r#"["sk-1","sk-2"]"#), "sk-1");
    }

    #[test]
    fn parse_first_key_skips_empty_keys_in_array() {
        assert_eq!(parse_first_key(r#"["","sk-2"]"#), "sk-2");
    }

    #[tokio::test]
    async fn probe_returns_failure_when_api_key_missing() {
        let mut p = make_provider("openai", r#"["openai_chat_completions"]"#);
        p.api_key = None;
        let report = probe(&p).await;
        assert!(!report.success);
        assert!(report.error.unwrap().contains("API key"));
    }

    #[tokio::test]
    async fn probe_returns_failure_when_api_key_empty_array() {
        let mut p = make_provider("openai", r#"["openai_chat_completions"]"#);
        p.api_key = Some("[]".into());
        let report = probe(&p).await;
        assert!(!report.success);
    }
}
