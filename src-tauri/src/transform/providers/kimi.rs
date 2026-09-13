use crate::protocol::chat_completions::ChatCompletionsRequest;
use crate::transform::tool_calls;
use serde_json::{json, Value};

pub struct KimiProvider;

impl super::ProviderTransform for KimiProvider {
    fn finalize_request(&self, req: &mut ChatCompletionsRequest, tools: &Option<Vec<Value>>) {
        if is_k3_model(&req.model) {
            // K3 always reasons via top-level reasoning_effort. Do not send the
            // K2.x `thinking` object — upstream docs explicitly forbid it.
            req.thinking = None;
            match req.reasoning_effort.as_deref().map(str::trim) {
                // Kimi Code docs: none → disable thinking. Platform currently
                // only documents max, but the coding endpoint accepts this.
                Some("none") | Some("off") => {
                    req.reasoning_effort = None;
                    req.thinking = Some(json!({"type": "disabled"}));
                }
                Some(effort) if !effort.is_empty() => {
                    // Currently only max is live; fold aliases / future buckets
                    // down to max so clients don't get HTTP 400 for low/high.
                    req.reasoning_effort = Some(map_k3_effort(effort));
                }
                _ => {
                    // null / empty → default max
                    req.reasoning_effort = Some("max".into());
                }
            }
            return;
        }

        // K2.x / coding models: thinking is controlled by thinking:{type},
        // not reasoning_effort. Drop any effort field that slipped through.
        req.reasoning_effort = None;

        // Disable thinking when $web_search tool is present
        if let Some(ref tools) = tools {
            if tool_calls::contains_kimi_web_search(tools) {
                req.thinking = Some(json!({"type": "disabled"}));
            }
        }
    }

    fn provider_type(&self) -> &str {
        "kimi"
    }

    /// K2.x does not accept reasoning_effort (thinking is on/off only).
    /// K3 does — finalize_request maps the value after convert fills it.
    /// We pass a provisional value here so convert keeps the client's effort
    /// for K3; non-K3 models strip it in finalize_request.
    fn map_reasoning_effort(&self, effort: &str) -> Option<String> {
        match effort.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some("none".into()),
            "auto" | "" => None,
            other if !other.is_empty() => Some(map_k3_effort(other)),
            _ => None,
        }
    }

    fn enhance_error(&self, status: u16, body: &str) -> Option<String> {
        use crate::transform::providers as p;
        if p::detect_insufficient_balance(status, body) {
            return Some(
                "Kimi 账户余额 / 配额不足。\n\
                 • 充值入口：https://platform.moonshot.cn/console/account\n\
                 • 用量查询：https://platform.moonshot.cn/console/usage\n\
                 • Kimi Code 会员：https://www.kimi.com/code/#pricing\n\
                 • MuxLayer 会自动 failover 到其它非冷却 provider。"
                    .to_string(),
            );
        }
        if p::detect_auth_error(status, body) {
            return Some(
                "Kimi API key 无效 / 过期，或当前套餐无权调用该模型（如 K3 需 Moderato+）。\n\
                 • Platform key：https://platform.moonshot.cn/console/api-keys\n\
                 • Kimi Code key：https://www.kimi.com/code/console\n\
                 • 模型权限说明：https://www.kimi.com/code/docs/kimi-code/models.html"
                    .to_string(),
            );
        }
        if p::detect_rate_limit(status, body) {
            return Some(
                "Kimi 触发限流。RPM / TPM 上限因账户级别不同：\n\
                 • https://platform.moonshot.cn/console/info 查看你的速率配额\n\
                 • MuxLayer 已冷却该 provider，路由会优先尝试其它候选"
                    .to_string(),
            );
        }
        p::detect_common_400(status, body)
    }
}

/// Platform ID `kimi-k3` and Kimi Code ID `k3` (plus optional `[1m]` qualifier).
fn is_k3_model(model: &str) -> bool {
    matches!(
        crate::providers::model_id::strip_qualifier(model.trim()),
        "k3" | "kimi-k3"
    )
}

/// Map client effort vocabulary onto K3's documented buckets.
/// Live API currently only accepts `max`; fold everything else to `max` so
/// Codex/Claude Code clients don't 400 while low/high are still gated.
fn map_k3_effort(effort: &str) -> String {
    match effort.trim().to_ascii_lowercase().as_str() {
        "none" | "off" => "none".into(),
        // low / high / medium / ultra / xhigh → max until upstream opens them.
        _ => "max".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::chat_completions::ChatCompletionsRequest;
    use crate::transform::providers::ProviderTransform;
    use serde_json::json;

    fn req(model: &str) -> ChatCompletionsRequest {
        ChatCompletionsRequest {
            model: model.into(),
            messages: vec![],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_completion_tokens: None,
            thinking: None,
            stream_options: None,
            response_format: None,
            reasoning_effort: None,
            seed: None,
            stop: None,
            frequency_penalty: None,
            presence_penalty: None,
            parallel_tool_calls: None,
            diagnostic_events: Vec::new(),
        }
    }

    #[test]
    fn kimi_maps_effort_for_k3_pipeline() {
        // Provisional values for convert → finalize; non-K3 still strips them.
        assert_eq!(KimiProvider.map_reasoning_effort("max"), Some("max".into()));
        assert_eq!(
            KimiProvider.map_reasoning_effort("xhigh"),
            Some("max".into())
        );
        assert_eq!(KimiProvider.map_reasoning_effort("low"), Some("max".into()));
        assert_eq!(
            KimiProvider.map_reasoning_effort("none"),
            Some("none".into())
        );
        assert_eq!(KimiProvider.map_reasoning_effort("auto"), None);
        assert_eq!(KimiProvider.map_reasoning_effort(""), None);
    }

    #[test]
    fn k3_defaults_reasoning_effort_to_max_and_drops_thinking() {
        let mut r = req("kimi-k3");
        r.thinking = Some(json!({"type": "enabled"}));
        KimiProvider.finalize_request(&mut r, &None);
        assert_eq!(r.reasoning_effort.as_deref(), Some("max"));
        assert!(r.thinking.is_none());
    }

    #[test]
    fn k3_code_id_and_1m_qualifier_recognized() {
        for model in ["k3", "k3[1m]", "kimi-k3[1m]"] {
            let mut r = req(model);
            r.reasoning_effort = Some("high".into());
            KimiProvider.finalize_request(&mut r, &None);
            assert_eq!(
                r.reasoning_effort.as_deref(),
                Some("max"),
                "model {model} should map effort to max"
            );
            assert!(r.thinking.is_none(), "model {model} must not send thinking");
        }
    }

    #[test]
    fn k3_none_effort_disables_thinking() {
        let mut r = req("k3");
        r.reasoning_effort = Some("none".into());
        KimiProvider.finalize_request(&mut r, &None);
        assert!(r.reasoning_effort.is_none());
        assert_eq!(r.thinking, Some(json!({"type": "disabled"})));
    }

    #[test]
    fn k2_coding_drops_reasoning_effort() {
        let mut r = req("kimi-for-coding");
        r.reasoning_effort = Some("max".into());
        r.thinking = Some(json!({"type": "enabled"}));
        KimiProvider.finalize_request(&mut r, &None);
        assert!(r.reasoning_effort.is_none());
        assert_eq!(r.thinking, Some(json!({"type": "enabled"})));
    }

    #[test]
    fn kimi_disables_thinking_with_web_search_on_k2() {
        let mut r = req("kimi-k2.6");
        let tools = Some(vec![
            json!({"type": "builtin_function", "function": {"name": "$web_search"}}),
        ]);
        r.tools = tools.clone();
        KimiProvider.finalize_request(&mut r, &tools);
        assert_eq!(r.thinking, Some(json!({"type": "disabled"})));
    }

    #[test]
    fn k3_web_search_does_not_force_thinking_object() {
        // K3 must keep reasoning_effort path; do not inject thinking disabled.
        let mut r = req("kimi-k3");
        let tools = Some(vec![
            json!({"type": "builtin_function", "function": {"name": "$web_search"}}),
        ]);
        r.tools = tools.clone();
        KimiProvider.finalize_request(&mut r, &tools);
        assert_eq!(r.reasoning_effort.as_deref(), Some("max"));
        assert!(r.thinking.is_none());
    }

    #[test]
    fn kimi_keeps_thinking_without_web_search() {
        let mut r = req("kimi-k2");
        r.thinking = Some(json!({"type": "enabled"}));
        let tools = Some(vec![
            json!({"type": "function", "function": {"name": "get_weather"}}),
        ]);
        r.tools = tools.clone();
        KimiProvider.finalize_request(&mut r, &tools);
        assert_eq!(r.thinking, Some(json!({"type": "enabled"})));
    }

    #[test]
    fn kimi_provider_type() {
        assert_eq!(KimiProvider.provider_type(), "kimi");
    }

    #[test]
    fn strip_qualifier_helpers() {
        assert!(is_k3_model("k3"));
        assert!(is_k3_model("kimi-k3"));
        assert!(is_k3_model("k3[1m]"));
        assert!(!is_k3_model("kimi-k2.6"));
        assert!(!is_k3_model("kimi-for-coding"));
    }
}
