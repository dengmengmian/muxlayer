use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub mod chat;
pub mod gemini;
pub mod messages;
pub mod meta;
pub mod responses;
pub mod shared;

// 把所有 shared helper 重新导出到本模块命名空间,让 mod.rs 里现存的 endpoint 入口
// 以及 tests 继续按原名直接引用,无需触碰 fn body。
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use shared::{anthropic_request_has_images, chat_request_has_images};
#[allow(unused_imports)]
pub(crate) use shared::{
    anthropic_request_has_images_value, chat_request_has_images_value, check_budget,
    detect_client_from_ua, enrich_trace_with_route_decision, get_active_provider,
    log_request_error, log_request_error_full, log_request_success, native_model_override,
    refine_struct_body, refine_value_body, request_body_or_gateway_error, request_contains_images,
    request_contains_images_pub, route_candidate_skip_reasons, route_fallback_chain, sanitize_body,
    select_providers, trace_with_degradation_events, truncate_str, validate_auth, validate_token,
    GatewayError,
};
#[allow(unused_imports)]
pub(crate) use shared::{host_is_allowed, origin_is_allowed, validate_request_boundary};

// 子模块 endpoint 入口重导出,server.rs 不用改 route 注册路径。
pub use chat::handle_chat_completions;
pub use gemini::handle_gemini_generate;
pub use messages::handle_messages;
pub use meta::{handle_count_tokens, health, list_gemini_models, list_models};
pub use responses::handle_responses;

/// Shared state for the gateway HTTP server.
#[derive(Clone)]
pub struct GatewayState {
    pub db: crate::storage::db::DbPool,
    pub http_client: reqwest::Client,
    pub active_requests: Arc<AtomicU64>,
    /// 请求体上限(字节)。压缩请求解压后也按这个上限封顶。
    pub request_body_limit: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use serde_json::json;

    use crate::errors::AppError;
    use crate::models::provider::Provider;
    use crate::protocol::openai_responses::ResponsesRequest;
    use crate::security::local_token;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};

    #[test]
    fn test_validate_auth_missing() {
        let headers = HeaderMap::new();
        let err = validate_auth(&headers).unwrap_err();
        assert_eq!(err.0.code, "GATEWAY_AUTH_MISSING");
    }

    #[test]
    fn test_validate_auth_invalid() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let _ = local_token::ensure_token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer wrong_token".parse().unwrap());
        let err = validate_auth(&headers).unwrap_err();
        assert_eq!(err.0.code, "GATEWAY_AUTH_INVALID");
        cleanup(&temp);
    }

    #[test]
    fn test_validate_auth_valid() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let token = local_token::ensure_token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        assert!(validate_auth(&headers).is_ok());
        cleanup(&temp);
    }

    #[test]
    fn test_validate_auth_no_bearer_prefix() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let token = local_token::ensure_token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", token.parse().unwrap());
        assert!(validate_auth(&headers).is_ok());
        cleanup(&temp);
    }

    #[test]
    fn test_validate_auth_x_api_key_valid() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let token = local_token::ensure_token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", token.parse().unwrap());
        assert!(validate_auth(&headers).is_ok());
        cleanup(&temp);
    }

    #[test]
    fn test_validate_auth_x_api_key_invalid() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = setup_temp_home();
        let _ = local_token::ensure_token().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", "wrong_token".parse().unwrap());
        let err = validate_auth(&headers).unwrap_err();
        assert_eq!(err.0.code, "GATEWAY_AUTH_INVALID");
        assert!(err.0.suggestion.as_ref().unwrap().contains("x-api-key"));
        cleanup(&temp);
    }

    // ── truncate_str tests ──

    #[test]
    fn test_truncate_str_ascii() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_str_chinese() {
        let s = "你好世界测试";
        // Each Chinese char is 3 bytes. "你好" = 6 bytes.
        // Truncate at 7 should land inside "世" → snap back to 6
        assert_eq!(truncate_str(s, 7), "你好");
        assert_eq!(truncate_str(s, 6), "你好");
        assert_eq!(truncate_str(s, 100), s);
    }

    #[test]
    fn test_truncate_str_emoji() {
        let s = "hello 🎉 world";
        // 🎉 is 4 bytes at position 6..10
        assert_eq!(truncate_str(s, 7), "hello "); // snap back before emoji
        assert_eq!(truncate_str(s, 10), "hello 🎉");
    }

    // ── sanitize_body tests ──

    #[test]
    fn test_sanitize_body_redacts_keys() {
        let body = r#"{"key": "sk-abcdefghij1234567890"}"#;
        let sanitized = sanitize_body(body);
        assert!(!sanitized.contains("abcdefghij1234567890"));
        assert!(sanitized.contains("sk-"));
    }

    #[test]
    fn test_sanitize_body_multiple_keys() {
        let body = r#"sk-firstkeyvalue1 sk-secondkeyvalue2"#;
        let sanitized = sanitize_body(body);
        assert!(!sanitized.contains("firstkeyvalue1"));
        assert!(!sanitized.contains("secondkeyvalue2"));
    }

    #[test]
    fn test_sanitize_body_redacts_x_api_key_and_fields() {
        // 复现 serve 模式落库泄密:非 sk- 形态的密钥之前原文进 request_logs。
        let body = r#"{"x-api-key": "supersecretvalue123", "api_key": "anotherlongsecret456"}"#;
        let sanitized = sanitize_body(body);
        assert!(!sanitized.contains("supersecretvalue123"));
        assert!(!sanitized.contains("anotherlongsecret456"));
    }

    #[test]
    fn test_sanitize_body_short_sk_not_redacted() {
        let body = "sk-short";
        let sanitized = sanitize_body(body);
        assert_eq!(sanitized, "sk-short");
    }

    // ── Host/Origin 边界防护(DNS rebinding)──

    #[test]
    fn host_allows_ip_localhost_rejects_dns_names() {
        // 放行:IP 字面量(带/不带端口)、localhost、IPv6、空(非浏览器客户端)
        assert!(host_is_allowed("127.0.0.1:9090", ""));
        assert!(host_is_allowed("192.168.1.5:9090", ""));
        assert!(host_is_allowed("localhost:9090", ""));
        assert!(host_is_allowed("LOCALHOST", ""));
        assert!(host_is_allowed("[::1]:9090", ""));
        assert!(host_is_allowed("::1", ""));
        assert!(host_is_allowed("", ""));
        // 拒绝:DNS 名(rebinding 攻击载体)
        assert!(!host_is_allowed("evil.com", ""));
        assert!(!host_is_allowed("evil.com:9090", ""));
        // 白名单放行
        assert!(host_is_allowed("my.box.local:9090", "my.box.local"));
        assert!(host_is_allowed(
            "a.example.com",
            "b.example.com, a.example.com"
        ));
    }

    #[test]
    fn origin_rules_reject_cross_site_by_default() {
        // 非浏览器(无 Origin)放行
        assert!(origin_is_allowed("", "", ""));
        // localhost / IP origin 放行(本机 UI、Tauri webview)
        assert!(origin_is_allowed("http://localhost:1420", "", ""));
        assert!(origin_is_allowed("http://127.0.0.1:1420", "", ""));
        assert!(origin_is_allowed("tauri://localhost", "", ""));
        // 跨站默认拒绝
        assert!(!origin_is_allowed("https://evil.com", "", ""));
        assert!(!origin_is_allowed("null", "", ""));
        // 白名单整条放行
        assert!(origin_is_allowed("https://my.app", "https://my.app", ""));
    }

    #[test]
    #[serial_test::serial(env)]
    fn boundary_rejects_rebinding_host_via_headers() {
        std::env::remove_var("AGENTGATE_ALLOWED_HOSTS");
        std::env::remove_var("AGENTGATE_ALLOWED_ORIGINS");
        let mut hm = HeaderMap::new();
        hm.insert("host", "127.0.0.1:9090".parse().unwrap());
        assert!(validate_request_boundary(&hm).is_ok());

        let mut hm = HeaderMap::new();
        hm.insert("host", "evil.com".parse().unwrap());
        assert!(
            validate_request_boundary(&hm).is_err(),
            "DNS 名 Host 默认必须拒绝"
        );
    }

    // ── GatewayError format tests ──

    #[test]
    fn test_gateway_error_has_type_field() {
        let err = GatewayError(
            AppError::new(
                crate::errors::codes::UPSTREAM_STREAM_ERROR,
                "Provider failed",
            )
            .with_detail("HTTP 502"),
        );
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn test_gateway_error_status_mapping() {
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
            GatewayError(AppError::new("UNKNOWN_CODE", "wat"))
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn test_gateway_error_auth_status_codes() {
        assert_eq!(
            GatewayError(AppError::new(
                crate::errors::codes::GATEWAY_AUTH_MISSING,
                "no auth"
            ))
            .into_response()
            .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            GatewayError(AppError::new(
                crate::errors::codes::GATEWAY_AUTH_INVALID,
                "bad token"
            ))
            .into_response()
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn route_candidate_skip_reasons_explain_unavailable_cooldown_and_vision() {
        let provider = crate::models::route_profile::RouteProfileProviderView {
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

    #[test]
    fn route_fallback_chain_marks_primary_backup_and_selected() {
        let mk =
            |priority: i64, name: &str| crate::models::route_profile::RouteProfileProviderView {
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
        assert_eq!(chain[1]["selected"], true);
    }

    fn provider_for_native_model_tests() -> Provider {
        Provider {
            id: "p1".to_string(),
            name: "DeepSeek".to_string(),
            provider_type: "deepseek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key: Some("sk-test".to_string()),
            default_model: "deepseek-flash".to_string(),
            reasoning_model: Some("deepseek-v4-pro".to_string()),
            supported_models: Some(r#"["deepseek-v4-pro","deepseek-flash"]"#.to_string()),
            model_mapping: Some(r#"{"gpt-5.5":"deepseek-v4-pro"}"#.to_string()),
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".to_string(),
            timeout_seconds: 300,
            status: "active".to_string(),
            supports_vision: Some(false),
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

    #[test]
    fn native_model_override_maps_agentgate_virtual_model() {
        let provider = provider_for_native_model_tests();
        assert_eq!(
            native_model_override(&provider, Some("agentgate"), None),
            Some("deepseek-flash".to_string())
        );
    }

    #[test]
    fn native_model_override_maps_prefixed_agentgate_virtual_model() {
        let provider = provider_for_native_model_tests();
        assert_eq!(
            native_model_override(&provider, Some("openai/agentgate"), None),
            Some("deepseek-flash".to_string())
        );
    }

    #[test]
    fn native_model_override_uses_route_selected_model_for_agentgate() {
        let provider = provider_for_native_model_tests();
        assert_eq!(
            native_model_override(&provider, Some("agentgate"), Some("deepseek-v4-pro")),
            Some("deepseek-v4-pro".to_string())
        );
    }

    #[test]
    fn native_model_override_still_prefers_explicit_mapping() {
        let provider = provider_for_native_model_tests();
        assert_eq!(
            native_model_override(&provider, Some("gpt-5.5"), Some("deepseek-flash")),
            Some("deepseek-v4-pro".to_string())
        );
    }

    #[test]
    fn native_model_override_preserves_unmapped_real_model() {
        let provider = provider_for_native_model_tests();
        assert_eq!(
            native_model_override(&provider, Some("mimo-v2.5"), Some("deepseek-flash")),
            None
        );
    }

    #[test]
    fn test_detect_client_from_ua_empty() {
        let headers = HeaderMap::new();
        assert_eq!(detect_client_from_ua(&headers, "Default"), "Default");
    }

    #[test]
    fn test_detect_client_from_ua_claude_code() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "claude-code/0.1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Default"), "Claude Code");
    }

    #[test]
    fn test_detect_client_from_ua_codex() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "codex-cli/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Default"), "Codex");
    }

    #[test]
    fn test_detect_client_from_ua_agentgate_pet() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "AgentGate-Pet/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Default"), "Pet");
    }

    #[test]
    fn test_detect_client_from_ua_openai_sdk() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "openai-python/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Default"), "OpenAI SDK");
    }

    #[test]
    fn test_detect_client_from_ua_openai_sdk_codex_route() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "openai-python/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Codex"), "Codex");
    }

    #[test]
    fn test_detect_client_from_ua_cursor() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "Cursor/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Default"), "Cursor");
    }

    #[test]
    fn test_detect_client_from_ua_python_sdk() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "python-requests/2.28".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Default"), "Python SDK");
    }

    #[test]
    fn test_detect_client_from_ua_node_sdk() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "node-fetch/1.0".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Default"), "Node SDK");
    }

    #[test]
    fn test_detect_client_from_ua_curl() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "curl/7.64.1".parse().unwrap());
        assert_eq!(detect_client_from_ua(&headers, "Default"), "curl");
    }

    #[test]
    fn test_detect_client_from_ua_unknown() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "MyCustomAgent/1.0".parse().unwrap());
        assert_eq!(
            detect_client_from_ua(&headers, "Default"),
            "MyCustomAgent/1.0"
        );
    }

    fn responses_req_with_input(input: serde_json::Value) -> ResponsesRequest {
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

    #[test]
    fn request_contains_images_ignores_historic_image_when_current_turn_text_only() {
        // 反转旧测试：history 有图但当前 turn 是纯文本 → false。
        // 历史 image 由 mimo.rs::finalize_request strip 兜底，不需要 promote。
        // 避免一次发图导致整个会话被强制路由到 vision 模型（128K context）。
        let req = responses_req_with_input(json!([
            {"type": "message", "role": "user", "content": [
                {"type": "input_image", "image_url": {"url": "https://example.com/x.png"}}
            ]},
            {"type": "message", "role": "assistant", "content": "I see a cat."},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "what color is it?"}
            ]}
        ]));
        assert!(
            !request_contains_images(&req),
            "history image must NOT force promotion when current turn is text-only"
        );
    }

    #[test]
    fn request_contains_images_true_when_current_turn_has_image() {
        // 当前 turn 真发图 → 必须 promote 到 vision 模型（图还没被 strip）
        let req = responses_req_with_input(json!([
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "earlier message"}
            ]},
            {"type": "message", "role": "assistant", "content": "ok"},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "look at this"},
                {"type": "input_image", "image_url": {"url": "https://example.com/x.png"}}
            ]}
        ]));
        assert!(
            request_contains_images(&req),
            "current user turn with image must trigger promotion"
        );
    }

    #[test]
    fn request_contains_images_true_for_initial_top_level_content_parts() {
        let req = responses_req_with_input(json!([
            {"type": "input_text", "text": "describe this"},
            {"type": "input_image", "image_url": {"url": "https://example.com/x.png"}}
        ]));
        assert!(
            request_contains_images(&req),
            "initial multimodal content parts must trigger promotion"
        );
    }

    #[test]
    fn request_contains_images_false_when_no_image_anywhere() {
        let req = responses_req_with_input(json!([
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "hi"}
            ]}
        ]));
        assert!(!request_contains_images(&req));
    }

    #[test]
    fn request_contains_images_ignores_assistant_content() {
        let req = responses_req_with_input(json!([
            {"type": "message", "role": "assistant", "content": [
                {"type": "image_url", "image_url": {"url": "x"}}
            ]},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "hi"}
            ]}
        ]));
        assert!(
            !request_contains_images(&req),
            "assistant turns are not user input"
        );
    }

    #[test]
    fn request_contains_images_ignores_tool_outputs_after_user_image() {
        // 当前 turn user 有图，但后面跟了 tool/function_call_output（rev 遍历
        // 跳过非 user message，正确找到最后一条 user）
        let req = responses_req_with_input(json!([
            {"type": "message", "role": "user", "content": [
                {"type": "input_image", "image_url": {"url": "x"}}
            ]},
            {"type": "function_call_output", "call_id": "c1", "output": "stuff"}
        ]));
        assert!(
            request_contains_images(&req),
            "rev iter must skip tool items and find last user message"
        );
    }

    #[test]
    fn chat_request_has_images_true_for_current_user_image_url() {
        let body = json!({"messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "what is this"},
                {"type": "image_url", "image_url": {"url": "x"}}
            ]}
        ]})
        .to_string();
        assert!(chat_request_has_images(&body));
    }

    #[test]
    fn chat_request_has_images_false_for_text_only() {
        let body = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "hi"}]}
        ]})
        .to_string();
        assert!(!chat_request_has_images(&body));
    }

    #[test]
    fn chat_request_has_images_ignores_historic_image() {
        // 历史 turn 发过图,但当前(最后一条)user 是纯文本 → 不算
        let body = json!({"messages": [
            {"role": "user", "content": [{"type": "image_url", "image_url": {"url": "x"}}]},
            {"role": "assistant", "content": "ok"},
            {"role": "user", "content": [{"type": "text", "text": "and now"}]}
        ]})
        .to_string();
        assert!(!chat_request_has_images(&body));
    }

    #[test]
    fn anthropic_request_has_images_true_for_current_user_image_block() {
        let body = json!({"messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "describe"},
                {"type": "image", "source": {"type": "base64", "data": "x"}}
            ]}
        ]})
        .to_string();
        assert!(anthropic_request_has_images(&body));
    }

    #[test]
    fn anthropic_request_has_images_false_for_text_only() {
        let body = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "hi"}]}
        ]})
        .to_string();
        assert!(!anthropic_request_has_images(&body));
    }

    /// 挡住「只加在 Responses」：预算闸、请求分析、选路、共用 failover 驱动
    /// 必须出现在四个入口(预算 / 选路收敛在 shared::check_budget / select_providers)。
    #[test]
    fn protocol_entries_share_budget_analysis_and_selection() {
        let handlers = [
            ("chat", include_str!("chat.rs")),
            ("messages", include_str!("messages.rs")),
            ("responses", include_str!("responses.rs")),
            ("gemini", include_str!("gemini.rs")),
        ];
        for (name, src) in handlers {
            assert!(src.contains("check_budget("), "{name} 入口缺少日预算闸");
            assert!(
                src.contains("select_providers("),
                "{name} 入口缺少共用选路 select_providers"
            );
            assert!(
                src.contains("analysis.clone()"),
                "{name} 入口选路未传入请求分析，条件/视觉会失效"
            );
            assert!(
                src.contains("failover::build_attempt_order"),
                "{name} 入口缺少共用 vision/failover 排序"
            );
            assert!(
                src.contains("failover::run_attempts"),
                "{name} 入口缺少共用 failover 驱动(熔断 / 切换)"
            );
            assert!(
                src.contains("request_has_images"),
                "{name} 入口未把带图标记交给 failover"
            );
        }
        let shared = include_str!("shared.rs");
        assert!(shared.contains("budget::check_new_request_with_conn"));
        assert!(shared.contains("select_for_failover_with_conn"));
        assert!(shared.contains("Some(&analysis)"));
    }
}
