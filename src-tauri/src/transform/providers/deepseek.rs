use crate::errors::AppError;
use crate::protocol::chat_completions::{ChatCompletionsRequest, ChatMessage};
use crate::providers::model_id::strip_qualifier;
use crate::transform::{degradation, reasoning_store, tool_calls};
use serde_json::{json, Value};

pub struct DeepSeekProvider;

const MIXED_MODE_REASONING_PLACEHOLDER: &str = "(this turn ran without thinking mode)";

fn is_deepseek_v4_family(model: &str) -> bool {
    matches!(
        strip_qualifier(model),
        "deepseek-v4-pro" | "deepseek-v4-flash"
    )
}

fn is_deepseek_vision_model(model: &str) -> bool {
    strip_qualifier(model) == "deepseek-v4-flash-vision-exp"
}

fn is_thinking_enabled(req: &ChatCompletionsRequest) -> bool {
    req.thinking
        .as_ref()
        .and_then(|t| t.get("type"))
        .and_then(|t| t.as_str())
        != Some("disabled")
}

fn strip_reasoning_content(messages: &mut [ChatMessage]) {
    for msg in messages {
        msg.reasoning_content = None;
    }
}

fn backfill_reasoning_content(messages: &mut [ChatMessage], model: &str) {
    for msg in messages {
        if msg.role != "assistant" || msg.reasoning_content.is_some() {
            continue;
        }

        let text = msg.content.as_ref().and_then(|c| c.as_str()).unwrap_or("");
        let stored = reasoning_store::lookup_by_content(model, text).or_else(|| {
            msg.tool_calls.as_ref().and_then(|tcs| {
                tcs.iter()
                    .find_map(|tc| reasoning_store::lookup_by_tool_call_id(model, &tc.id))
            })
        });
        msg.reasoning_content =
            stored.or_else(|| Some(MIXED_MODE_REASONING_PLACEHOLDER.to_string()));
    }
}

impl super::ProviderTransform for DeepSeekProvider {
    fn process_messages(&self, messages: Vec<ChatMessage>) -> Result<Vec<ChatMessage>, AppError> {
        tool_calls::fix_tool_message_order(messages)
    }

    fn finalize_request(&self, req: &mut ChatCompletionsRequest, _tools: &Option<Vec<Value>>) {
        let model = strip_qualifier(&req.model).to_string();
        let model = model.as_str();

        // DeepSeek's vision experiment accepts the standard Chat Completions
        // image_url block. Keep the old degradation guard for every other
        // DeepSeek model, including historic image turns replayed to a
        // text-only model.
        if !is_deepseek_vision_model(model) {
            req.diagnostic_events
                .extend(degradation::strip_image_parts_with_notice(
                &mut req.messages,
                "deepseek",
                "DeepSeek",
                model,
                "To analyze images, switch to a vision-capable provider/model and re-send the request.",
            ));
        }

        // DeepSeek V4 exposes thinking as an explicit request contract. For
        // unknown DeepSeek-compatible ids we keep transparent proxy semantics:
        // do not invent provider-specific thinking fields.
        if is_deepseek_v4_family(model) {
            if req.thinking.is_none() {
                req.thinking = Some(json!({"type": "enabled"}));
            }
            if is_thinking_enabled(req) {
                if req.reasoning_effort.is_none() {
                    req.reasoning_effort = Some("high".to_string());
                }
                // In thinking mode DeepSeek ignores sampling penalties; strip
                // them client-side so logs match effective upstream behavior.
                req.temperature = None;
                req.top_p = None;
                req.presence_penalty = None;
                req.frequency_penalty = None;
                backfill_reasoning_content(&mut req.messages, model);
            } else {
                req.reasoning_effort = None;
                strip_reasoning_content(&mut req.messages);
            }
        } else {
            // Unknown DeepSeek-compatible model: don't invent thinking fields,
            // but still remove the non-standard "none" effort if a generic
            // disable-thinking path ever produced it.
            if req.reasoning_effort.as_deref() == Some("none") {
                req.reasoning_effort = None;
            }
        }

        // Downgrade json_schema to json_object (DeepSeek doesn't support json_schema)
        if let Some(ref fmt) = req.response_format {
            if fmt.get("type").and_then(|t| t.as_str()) == Some("json_schema") {
                req.response_format = Some(json!({"type": "json_object"}));
            }
        }
    }

    fn clean_schemas(&self) -> bool {
        true
    }

    fn provider_type(&self) -> &str {
        "deepseek"
    }

    fn map_reasoning_effort(&self, effort: &str) -> Option<String> {
        match effort.trim().to_ascii_lowercase().as_str() {
            // DeepSeek documents `high` and `max`; map lower OpenAI/Codex
            // buckets to the lowest accepted thinking effort instead of
            // passing unsupported values through.
            "minimal" | "low" | "medium" | "high" => Some("high".to_string()),
            "xhigh" | "max" | "highest" => Some("max".to_string()),
            "none" | "off" | "auto" | "" => None,
            _ => None,
        }
    }

    fn enhance_error(&self, status: u16, body: &str) -> Option<String> {
        use crate::transform::providers as p;
        if p::detect_insufficient_balance(status, body) {
            return Some(
                "DeepSeek 账户余额不足。\n\
                 • 充值入口：https://platform.deepseek.com/top_up\n\
                 • 用量查询：https://platform.deepseek.com/usage\n\
                 • 或临时切换到 MuxLayer 里其它 provider（路由会自动 failover 到非冷却状态的候选）。"
                    .to_string(),
            );
        }
        if p::detect_auth_error(status, body) {
            return Some(
                "DeepSeek API key 无效 / 过期。\n\
                 • 查看 / 重建 key：https://platform.deepseek.com/api_keys\n\
                 • 检查 MuxLayer provider 配置里的 key 是否粘贴完整。"
                    .to_string(),
            );
        }
        if p::detect_rate_limit(status, body) {
            return Some(
                "DeepSeek 触发限流。MuxLayer 已自动冷却该 provider 一段时间；\n\
                 • 路由 profile 有其它 candidate 时会自动 failover\n\
                 • 配额信息：https://platform.deepseek.com/usage"
                    .to_string(),
            );
        }
        // Fall back to shared context-overflow detection.
        p::detect_common_400(status, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::chat_completions::{
        ChatCompletionsRequest, ChatMessage, ToolCall, ToolCallFunction,
    };
    use crate::transform::providers::ProviderTransform;
    use serde_json::json;

    fn req() -> ChatCompletionsRequest {
        ChatCompletionsRequest {
            model: "deepseek-v4-pro".into(),
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
    fn deepseek_v4_defaults_thinking_enabled() {
        let mut r = req();
        r.model = "deepseek-v4-pro".into();
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert_eq!(r.thinking, Some(json!({"type": "enabled"})));
    }

    #[test]
    fn deepseek_v4_defaults_high_reasoning_effort() {
        let mut r = req();
        r.model = "deepseek-v4-flash".into();
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn deepseek_v4_preserves_explicit_max_reasoning_effort() {
        let mut r = req();
        r.model = "deepseek-v4-pro".into();
        r.reasoning_effort = Some("max".into());
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert_eq!(r.reasoning_effort.as_deref(), Some("max"));
    }

    #[test]
    fn deepseek_v4_thinking_mode_strips_sampling_knobs() {
        let mut r = req();
        r.model = "deepseek-v4-pro".into();
        r.temperature = Some(0.7);
        r.top_p = Some(0.9);
        r.presence_penalty = Some(0.1);
        r.frequency_penalty = Some(0.2);
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert!(r.temperature.is_none());
        assert!(r.top_p.is_none());
        assert!(r.presence_penalty.is_none());
        assert!(r.frequency_penalty.is_none());
    }

    #[test]
    fn deepseek_v4_backfills_reasoning_for_plain_assistant_in_thinking_mode() {
        let mut r = req();
        r.model = "deepseek-v4-pro".into();
        r.messages = vec![ChatMessage {
            role: "assistant".into(),
            content: Some(json!("old answer")),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert_eq!(
            r.messages[0].reasoning_content.as_deref(),
            Some(MIXED_MODE_REASONING_PLACEHOLDER)
        );
    }

    #[test]
    fn deepseek_v4_backfills_reasoning_for_tool_call_assistant_in_thinking_mode() {
        let mut r = req();
        r.model = "deepseek-v4-flash".into();
        r.messages = vec![ChatMessage {
            role: "assistant".into(),
            content: Some(json!("text")),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCall {
                id: "tc-1".into(),
                call_type: "function".into(),
                function: ToolCallFunction {
                    name: "f".into(),
                    arguments: "{}".into(),
                },
            }]),
            tool_call_id: None,
            name: None,
        }];
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert_eq!(
            r.messages[0].reasoning_content.as_deref(),
            Some(MIXED_MODE_REASONING_PLACEHOLDER)
        );
    }

    #[test]
    fn deepseek_v4_thinking_disabled_strips_reasoning_content() {
        let mut r = req();
        r.model = "deepseek-v4-pro".into();
        r.thinking = Some(json!({"type": "disabled"}));
        r.reasoning_effort = Some("high".into());
        r.messages = vec![ChatMessage {
            role: "assistant".into(),
            content: Some(json!("old answer")),
            reasoning_content: Some("old trace".into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert!(r.reasoning_effort.is_none());
        assert!(r.messages[0].reasoning_content.is_none());
    }

    #[test]
    fn deepseek_unknown_model_does_not_invent_reasoning_backfill() {
        let mut r = req();
        r.model = "deepseek-future-vision".into();
        r.messages = vec![ChatMessage {
            role: "assistant".into(),
            content: Some(json!("old answer")),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert!(r.thinking.is_none());
        assert!(r.messages[0].reasoning_content.is_none());
    }

    #[test]
    fn deepseek_deprecated_aliases_are_not_special_cased() {
        let mut r = req();
        r.model = "deepseek-chat".into();
        r.thinking = None;
        r.reasoning_effort = Some("none".into());
        r.messages = vec![ChatMessage {
            role: "assistant".into(),
            content: Some(json!("old answer")),
            reasoning_content: Some("old trace".into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert!(r.thinking.is_none());
        assert!(r.reasoning_effort.is_none());
        assert_eq!(
            r.messages[0].reasoning_content.as_deref(),
            Some("old trace")
        );

        let mut r = req();
        r.model = "deepseek-reasoner".into();
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert!(r.thinking.is_none());
        assert!(r.reasoning_effort.is_none());
    }

    #[test]
    fn deepseek_maps_reasoning_effort_to_supported_values() {
        assert_eq!(
            DeepSeekProvider.map_reasoning_effort("low"),
            Some("high".into())
        );
        assert_eq!(
            DeepSeekProvider.map_reasoning_effort("medium"),
            Some("high".into())
        );
        assert_eq!(
            DeepSeekProvider.map_reasoning_effort("max"),
            Some("max".into())
        );
        assert_eq!(
            DeepSeekProvider.map_reasoning_effort("xhigh"),
            Some("max".into())
        );
        assert_eq!(DeepSeekProvider.map_reasoning_effort("auto"), None);
    }

    #[test]
    fn deepseek_downgrades_json_schema() {
        let mut r = req();
        r.response_format = Some(json!({"type": "json_schema", "schema": {}}));
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert_eq!(r.response_format, Some(json!({"type": "json_object"})));
    }

    #[test]
    fn deepseek_keeps_json_object() {
        let mut r = req();
        r.response_format = Some(json!({"type": "json_object"}));
        DeepSeekProvider.finalize_request(&mut r, &None);
        assert_eq!(r.response_format, Some(json!({"type": "json_object"})));
    }

    #[test]
    fn deepseek_finalize_request_strips_image_url_with_notice() {
        let mut r = req();
        r.model = "deepseek-v4-pro".into();
        r.messages = vec![ChatMessage {
            role: "user".into(),
            content: Some(json!([
                {"type": "text", "text": "look"},
                {"type": "image_url", "image_url": {"url": "http://example.com/img.png"}}
            ])),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        DeepSeekProvider.finalize_request(&mut r, &None);
        let arr = r.messages[0].content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
        assert!(arr[0]["text"].as_str().unwrap().contains("image stripped"));
        assert!(arr[0]["text"].as_str().unwrap().contains("deepseek-v4-pro"));
        assert_eq!(r.diagnostic_events.len(), 1);
        assert_eq!(r.diagnostic_events[0].capability, "vision");
        assert_eq!(r.diagnostic_events[0].provider.as_deref(), Some("deepseek"));
    }

    #[test]
    fn deepseek_finalize_request_image_only_becomes_notice_text() {
        let mut r = req();
        r.model = "deepseek-v4-flash".into();
        r.messages = vec![ChatMessage {
            role: "user".into(),
            content: Some(json!([
                {"type": "image_url", "image_url": {"url": "http://example.com/img.png"}}
            ])),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        DeepSeekProvider.finalize_request(&mut r, &None);
        let arr = r.messages[0].content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
        assert!(arr[0]["text"].as_str().unwrap().contains("vision-capable"));
    }

    #[test]
    fn deepseek_vision_model_preserves_image_url() {
        let mut r = req();
        r.model = "deepseek-v4-flash-vision-exp".into();
        r.messages = vec![ChatMessage {
            role: "user".into(),
            content: Some(json!([
                {"type": "text", "text": "look"},
                {"type": "image_url", "image_url": {"url": "http://example.com/img.png", "detail": "high"}}
            ])),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];
        DeepSeekProvider.finalize_request(&mut r, &None);
        let arr = r.messages[0].content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["type"], "image_url");
        assert_eq!(arr[1]["image_url"]["detail"], "high");
        assert!(r.diagnostic_events.is_empty());
    }

    #[test]
    fn deepseek_provider_type() {
        assert_eq!(DeepSeekProvider.provider_type(), "deepseek");
    }

    #[test]
    fn deepseek_clean_schemas() {
        assert!(DeepSeekProvider.clean_schemas());
    }

    #[test]
    fn deepseek_enhances_insufficient_balance() {
        let hint = DeepSeekProvider
            .enhance_error(402, r#"{"error":{"code":"insufficient_balance"}}"#)
            .unwrap();
        assert!(hint.contains("DeepSeek"));
        assert!(hint.contains("top_up"));
    }

    #[test]
    fn deepseek_enhances_invalid_key() {
        let hint = DeepSeekProvider
            .enhance_error(401, "Invalid API key")
            .unwrap();
        assert!(hint.contains("api_keys"));
    }

    #[test]
    fn deepseek_enhances_rate_limit() {
        let hint = DeepSeekProvider.enhance_error(429, "").unwrap();
        assert!(hint.contains("限流") || hint.contains("usage"));
    }

    #[test]
    fn deepseek_falls_back_to_context_overflow() {
        let hint = DeepSeekProvider
            .enhance_error(400, "context_length_exceeded")
            .unwrap();
        assert!(hint.contains("compact") || hint.contains("上下文"));
    }
}
