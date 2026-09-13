//! Build a single-request redacted repro package for Issue filing.
//! Secrets are always masked via `redaction::redact_text`.

use serde_json::{json, Value};

use super::redaction::redact_text_with_secrets;

/// Inputs for one gateway log export. All body fields optional.
#[derive(Debug, Clone)]
pub struct ReproExportInput {
    pub request_id: String,
    pub timestamp: Option<String>,
    pub client: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub route: Option<String>,
    pub status_code: Option<i64>,
    pub latency_ms: Option<i64>,
    pub error_message: Option<String>,
    pub trace_json: Option<String>,
    pub raw_request: Option<String>,
    pub converted_request: Option<String>,
    pub raw_response: Option<String>,
    pub converted_response: Option<String>,
    /// App version string (e.g. from Cargo package version).
    pub app_version: String,
    /// When false, body fields are omitted entirely (metadata-only export).
    pub include_bodies: bool,
    /// 当前存储的 provider 密钥(`redaction::provider_secrets`),构建导出包时按原值
    /// 精确脱敏——没有 sk- 等前缀的 key 靠启发式脱敏会漏。
    pub known_secrets: Vec<String>,
}

/// Build a JSON object (pretty-printed string) safe to paste into an Issue.
/// API keys / tokens in any included text are redacted.
pub fn build_repro_package(input: &ReproExportInput) -> String {
    let mut body = json!({
        "format": "agentgate-repro-v1",
        "app_version": input.app_version,
        "request_id": input.request_id,
        "timestamp": input.timestamp,
        "client": input.client,
        "provider": input.provider,
        "model": input.model,
        "route": input.route,
        "status_code": input.status_code,
        "latency_ms": input.latency_ms,
        "error_message": input.error_message.as_ref().map(|s| redact(input, s)),
        "secrets_redacted": true,
    });

    if let Some(trace) = &input.trace_json {
        body["trace_json"] = Value::String(redact(input, trace));
        // Try to surface route_decision / error chain without raw dump noise.
        if let Ok(v) = serde_json::from_str::<Value>(trace) {
            if let Some(rd) = v.get("route_decision") {
                body["route_decision"] = rd.clone();
            }
            if let Some(em) = v.get("error_mapper") {
                body["error_mapper"] = em.clone();
            }
        }
    }

    if input.include_bodies {
        body["raw_request"] = json_opt_redacted(input, &input.raw_request);
        body["converted_request"] = json_opt_redacted(input, &input.converted_request);
        body["raw_response"] = json_opt_redacted(input, &input.raw_response);
        body["converted_response"] = json_opt_redacted(input, &input.converted_response);
    }

    serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".to_string())
}

fn redact(input: &ReproExportInput, text: &str) -> String {
    redact_text_with_secrets(text, &input.known_secrets)
}

fn json_opt_redacted(input: &ReproExportInput, s: &Option<String>) -> Value {
    match s {
        Some(t) => Value::String(redact(input, t)),
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(include_bodies: bool) -> ReproExportInput {
        ReproExportInput {
            request_id: "req_abc".into(),
            timestamp: Some("2026-07-30T00:00:00Z".into()),
            client: Some("Codex".into()),
            provider: Some("deepseek".into()),
            model: Some("deepseek-chat".into()),
            route: Some("/v1/responses".into()),
            status_code: Some(401),
            latency_ms: Some(120),
            error_message: Some("Invalid API key sk-secretkey1234567890xyz".into()),
            trace_json: Some(
                r#"{"route_decision":{"profile_name":"default"},"error_mapper":{"mapped_code":"AUTH"}}"#.into(),
            ),
            raw_request: Some(
                r#"{"model":"x","messages":[{"role":"user","content":"hi"}],"api_key":"sk-abcdefghijklmnopqrstuvwxyz"}"#.into(),
            ),
            converted_request: Some(r#"{"Authorization":"Bearer sk-abcdefghijklmnopqrstuvwxyz"}"#.into()),
            raw_response: Some(r#"{"error":"bad key sk-abcdefghijklmnopqrstuvwxyz"}"#.into()),
            converted_response: None,
            app_version: "1.5.1".into(),
            include_bodies,
            known_secrets: vec![],
        }
    }

    #[test]
    fn export_redacts_known_secrets_without_prefix() {
        let mut input = sample(true);
        input.known_secrets = vec!["AIzaSyNoPrefixKey0123456789".into()];
        input.error_message = Some("bad key AIzaSyNoPrefixKey0123456789".into());
        input.raw_response = Some(r#"{"error":"AIzaSyNoPrefixKey0123456789 invalid"}"#.into());
        let pkg = build_repro_package(&input);
        assert!(!pkg.contains("AIzaSyNoPrefixKey0123456789"), "{pkg}");
    }

    #[test]
    fn export_redacts_api_keys_in_bodies() {
        let pkg = build_repro_package(&sample(true));
        assert!(
            !pkg.contains("sk-abcdefghijklmnopqrstuvwxyz"),
            "raw key must not leak: {pkg}"
        );
        assert!(
            !pkg.contains("sk-secretkey1234567890xyz"),
            "error key must not leak: {pkg}"
        );
        assert!(pkg.contains("secrets_redacted"));
        assert!(pkg.contains("req_abc"));
        assert!(pkg.contains("route_decision") || pkg.contains("profile_name"));
    }

    #[test]
    fn export_without_bodies_omits_payloads() {
        let pkg = build_repro_package(&sample(false));
        assert!(!pkg.contains("messages"));
        assert!(!pkg.contains("\"raw_request\"") || pkg.contains("\"raw_request\": null"));
        // metadata-only: include_bodies false means keys absent or null
        let v: Value = serde_json::from_str(&pkg).unwrap();
        assert!(v.get("raw_request").is_none());
        assert!(v.get("converted_request").is_none());
    }

    #[test]
    fn export_strips_bearer_tokens() {
        let mut input = sample(true);
        input.raw_request = Some("Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz\n".into());
        let pkg = build_repro_package(&input);
        assert!(!pkg.contains("sk-abcdefghijklmnopqrstuvwxyz"));
    }
}
