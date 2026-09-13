//! Runtime entry points for the refiner pipeline.
//!
//! Wraps the three refiners so request handlers can apply them with one
//! call. Each function:
//!   - Reads the effective switches (global + per-provider) and bails when
//!     all three are off (the common case, since defaults are all off).
//!   - Calls the matching refiner only when its switch is on.
//!   - Accumulates structured `RefinerLog` entries that the caller stamps
//!     into `request_logs.trace_json` at log-write time.
//!
//! Keep this module thin — refiner-specific logic belongs in the
//! individual refiner modules, not here.

use serde_json::Value;

use crate::gateway::refiner_log::RefinerLog;
use crate::models::gateway::GatewaySettings;
use crate::models::provider::Provider;

use super::{body_filter, error_mapper, thinking_rectifier, EffectiveSwitches};

/// Apply request-side refiners (body_filter + thinking_rectifier) to the
/// outbound body. Mutates `body` in place and returns a `RefinerLog`
/// describing what changed. Returns `RefinerLog::default()` when both
/// switches are off — caller can call `is_empty()` to skip trace writes.
pub fn apply_request(
    provider: &Provider,
    settings: &GatewaySettings,
    body: &mut Value,
) -> RefinerLog {
    let switches = EffectiveSwitches::for_request(provider, settings);
    let mut log = RefinerLog::default();

    if switches.body_filter {
        if let Some(action) = body_filter::apply(provider, body) {
            log.body_filter = Some(action);
        }
    }
    if switches.thinking_rectifier {
        let actions = thinking_rectifier::apply(provider, body);
        // Multiple param edits can happen in one request (e.g. clamp budget
        // AND normalise effort). The schema currently stores one action per
        // request; concatenate the reasons so users can see both rewrites.
        if !actions.is_empty() {
            let first = &actions[0];
            log.thinking_rectifier = Some(crate::gateway::refiner_log::ThinkingRectifierAction {
                field: actions
                    .iter()
                    .map(|a| a.field.clone())
                    .collect::<Vec<_>>()
                    .join(","),
                from: first.from.clone(),
                to: first.to.clone(),
                reason: actions
                    .iter()
                    .map(|a| a.reason.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
    }

    log
}

/// Apply response-side error_mapper to a 4xx/5xx upstream body. Returns
/// `Some((mapped_body, RefinerLog))` when the body was rewritten; `None`
/// means leave the response unchanged. Caller is responsible for merging
/// the returned log into any existing log from the request path.
pub fn apply_response_error(
    provider: &Provider,
    settings: &GatewaySettings,
    body: &Value,
    status: u16,
    client_protocol: &str,
) -> Option<(Value, RefinerLog)> {
    let switches = EffectiveSwitches::for_request(provider, settings);
    if !switches.error_mapper {
        return None;
    }
    let mapped = error_mapper::apply(provider, body, status, client_protocol)?;
    let log = RefinerLog {
        error_mapper: Some(mapped.action),
        ..Default::default()
    };
    Some((mapped.body, log))
}

/// Merge `extra` into `base`, with later writers winning on conflicts.
/// Used when the response path produces an error_mapper action that must
/// join the request path's body_filter / thinking_rectifier log.
pub fn merge_logs(base: &mut RefinerLog, extra: RefinerLog) {
    if extra.body_filter.is_some() {
        base.body_filter = extra.body_filter;
    }
    if extra.thinking_rectifier.is_some() {
        base.thinking_rectifier = extra.thinking_rectifier;
    }
    if extra.error_mapper.is_some() {
        base.error_mapper = extra.error_mapper;
    }
    if extra.circuit_breaker.is_some() {
        base.circuit_breaker = extra.circuit_breaker;
    }
    if extra.degradation.is_some() {
        base.degradation = extra.degradation;
    }
}

/// Serialise `RefinerLog` for `trace_json` storage. Returns `None` when
/// the log is empty so the column stays NULL (saves disk + keeps the GUI
/// from rendering an empty "refiner" panel).
pub fn to_trace_json(log: &RefinerLog) -> Option<String> {
    if log.is_empty() {
        return None;
    }
    serde_json::to_string(log).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider(provider_type: &str) -> Provider {
        Provider {
            id: "p".into(),
            name: "P".into(),
            provider_type: provider_type.into(),
            base_url: "https://x".into(),
            api_key: Some("sk".into()),
            default_model: "m".into(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".into(),
            timeout_seconds: 60,
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

    fn settings(bf: bool, tr: bool, em: bool) -> GatewaySettings {
        GatewaySettings {
            id: 1,
            host: "127.0.0.1".into(),
            port: 9090,
            active_provider_id: None,
            input_protocol: "openai_responses".into(),
            output_protocol: "openai_chat_completions".into(),
            auto_start: false,
            log_retention_days: 14,
            body_filter_global: bf,
            thinking_rectifier_global: tr,
            error_mapper_global: em,
            health_probe_enabled: false,
            codex_compact_enabled: true,
            codex_compact_summary_max_tokens: 1500,
            request_body_limit_mb: 32,
            cost_alert_enabled: false,
            cost_alert_threshold: None,
            cost_budget_enabled: false,
            cost_budget_threshold: None,
            cost_budget_strategy: "notify_only".into(),
            auto_compact_enabled: true,
            auto_compact_usage_percent: 85,
            wake_enabled: true,
            wake_request_control: false,
            wake_cooldown_seconds: 900,
            wake_keep_display_awake: false,
            outbound_proxy_enabled: false,
            outbound_proxy_url: None,
            updated_at: "now".into(),
        }
    }

    #[test]
    fn request_refiners_off_returns_empty_log() {
        let p = provider("deepseek");
        let s = settings(false, false, false);
        let mut body = json!({"web_search": true, "thinking": {"budget_tokens": 100}});
        let log = apply_request(&p, &s, &mut body);
        assert!(log.is_empty());
        // Body should be unchanged since switches are off.
        assert_eq!(body["web_search"], json!(true));
        assert_eq!(body["thinking"]["budget_tokens"], json!(100));
    }

    #[test]
    fn request_refiners_on_strips_and_clamps() {
        // DeepSeek strips web_search; MiMo clamps thinking.budget_tokens.
        // Combine both to exercise both refiners in one call.
        let mut p = provider("deepseek");
        p.provider_quirks = Some(r#"{"thinking_budget":{"min":1024,"max":4096}}"#.into());
        let s = settings(true, true, false);
        let mut body = json!({
            "web_search": true,
            "thinking": {"budget_tokens": 100},
        });
        let log = apply_request(&p, &s, &mut body);
        assert!(!log.is_empty());
        assert!(log.body_filter.is_some());
        assert!(log.thinking_rectifier.is_some());
        assert!(body.get("web_search").is_none());
        assert_eq!(body["thinking"]["budget_tokens"], json!(1024));
    }

    #[test]
    fn error_mapper_off_returns_none() {
        let p = provider("deepseek");
        let s = settings(true, true, false);
        let body = json!({"error": {"message": "boom", "code": "rate_limit"}});
        assert!(apply_response_error(&p, &s, &body, 429, "openai_responses").is_none());
    }

    #[test]
    fn error_mapper_on_rewrites_to_target_protocol() {
        let p = provider("deepseek");
        let s = settings(false, false, true);
        let body = json!({"error": {"message": "boom", "code": "rate_limit"}});
        let (mapped, log) = apply_response_error(&p, &s, &body, 429, "anthropic_messages").unwrap();
        assert_eq!(mapped["type"], "error");
        assert!(log.error_mapper.is_some());
    }

    #[test]
    fn merge_logs_combines_request_and_response() {
        let mut req_log = RefinerLog {
            body_filter: Some(crate::gateway::refiner_log::BodyFilterAction {
                stripped_fields: vec!["web_search".into()],
                reason: "test".into(),
            }),
            ..Default::default()
        };
        let resp_log = RefinerLog {
            error_mapper: Some(crate::gateway::refiner_log::ErrorMapperAction {
                upstream_code: Some("x".into()),
                upstream_message: None,
                mapped_code: "rate_limit".into(),
                mapped_message: "boom".into(),
            }),
            ..Default::default()
        };
        merge_logs(&mut req_log, resp_log);
        assert!(req_log.body_filter.is_some());
        assert!(req_log.error_mapper.is_some());
    }

    #[test]
    fn to_trace_json_returns_none_for_empty_log() {
        let log = RefinerLog::default();
        assert!(to_trace_json(&log).is_none());
    }

    #[test]
    fn to_trace_json_serializes_populated_log() {
        let log = RefinerLog {
            body_filter: Some(crate::gateway::refiner_log::BodyFilterAction {
                stripped_fields: vec!["web_search".into()],
                reason: "test".into(),
            }),
            ..Default::default()
        };
        let json = to_trace_json(&log).unwrap();
        assert!(json.contains("web_search"));
        assert!(json.contains("body_filter"));
    }
}
