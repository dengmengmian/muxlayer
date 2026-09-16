use crate::models::provider::{CreateProviderInput, UpdateProviderInput};
use crate::storage::generated_provider_catalog as catalog;

const MIMO_PAYG_BASE_URL: &str = catalog::MIMO_PAYG_BASE_URL;
const MIMO_PAYG_ANTHROPIC_URL: &str = catalog::MIMO_PAYG_ANTHROPIC_URL;
#[cfg(test)]
const MIMO_TOKEN_PLAN_BASE_URL: &str = catalog::MIMO_TOKEN_PLAN_DEFAULT_BASE_URL;
#[cfg(test)]
const MIMO_TOKEN_PLAN_ANTHROPIC_URL: &str = catalog::MIMO_TOKEN_PLAN_DEFAULT_ANTHROPIC_URL;
const DEEPSEEK_BASE_URL: &str = catalog::DEEPSEEK_BASE_URL;
const DEEPSEEK_ANTHROPIC_URL: &str = catalog::DEEPSEEK_ANTHROPIC_URL;
const DEEPSEEK_SUPPORTED_MODELS: &str = catalog::DEEPSEEK_SUPPORTED_MODELS_JSON;
const DEEPSEEK_REASONING_MODEL: &str = catalog::DEEPSEEK_REASONING_MODEL;

#[derive(Debug, Clone, PartialEq, Eq)]
struct EndpointUrls {
    base_url: String,
    anthropic_base_url: String,
}

pub fn apply_to_create_input(input: &mut CreateProviderInput) {
    if let Some(urls) = endpoints_for_provider_key(
        &input.provider_type,
        input.api_key.as_deref(),
        Some(&input.base_url),
        input.anthropic_base_url.as_deref(),
    ) {
        if should_replace_mimo_url(&input.base_url) {
            input.base_url = urls.base_url;
        }
        if input
            .anthropic_base_url
            .as_deref()
            .is_none_or(should_replace_mimo_url)
        {
            input.anthropic_base_url = Some(urls.anthropic_base_url);
        }
        input.protocol = ensure_protocol(&input.protocol, "anthropic_messages");
    } else if is_deepseek_provider_type(&input.provider_type) {
        if should_replace_deepseek_url(&input.base_url) {
            input.base_url = DEEPSEEK_BASE_URL.to_string();
        }
        if input
            .anthropic_base_url
            .as_deref()
            .is_none_or(should_replace_deepseek_url)
        {
            input.anthropic_base_url = Some(DEEPSEEK_ANTHROPIC_URL.to_string());
        }
        if input
            .reasoning_model
            .as_deref()
            .is_none_or(|model| model.trim().is_empty())
        {
            input.reasoning_model = Some(DEEPSEEK_REASONING_MODEL.to_string());
        }
        if input
            .supported_models
            .as_deref()
            .is_none_or(|models| models.trim().is_empty())
        {
            input.supported_models = Some(DEEPSEEK_SUPPORTED_MODELS.to_string());
        }
        input.protocol = ensure_protocol(&input.protocol, "anthropic_messages");
    }
}

pub fn apply_to_update_input(
    provider_type: &str,
    effective_api_key: Option<&str>,
    current_base_url: &str,
    current_anthropic_base_url: Option<&str>,
    current_protocol: &str,
    input: &mut UpdateProviderInput,
) {
    let base_candidate = input.base_url.as_deref().unwrap_or(current_base_url);
    let anthropic_candidate = input
        .anthropic_base_url
        .as_deref()
        .or(current_anthropic_base_url);
    if let Some(urls) = endpoints_for_provider_key(
        provider_type,
        effective_api_key,
        Some(base_candidate),
        anthropic_candidate,
    ) {
        if should_replace_mimo_url(base_candidate) {
            input.base_url = Some(urls.base_url);
        }
        if anthropic_candidate.is_none_or(should_replace_mimo_url) {
            input.anthropic_base_url = Some(urls.anthropic_base_url);
        }
        let protocol = input.protocol.as_deref().unwrap_or(current_protocol);
        input.protocol = Some(ensure_protocol(protocol, "anthropic_messages"));
    } else if is_deepseek_provider_type(provider_type) {
        let base_candidate = input.base_url.as_deref().unwrap_or(current_base_url);
        if should_replace_deepseek_url(base_candidate) {
            input.base_url = Some(DEEPSEEK_BASE_URL.to_string());
        }
        let anthropic_candidate = input
            .anthropic_base_url
            .as_deref()
            .or(current_anthropic_base_url);
        if anthropic_candidate.is_none_or(should_replace_deepseek_url) {
            input.anthropic_base_url = Some(DEEPSEEK_ANTHROPIC_URL.to_string());
        }
        let protocol = input.protocol.as_deref().unwrap_or(current_protocol);
        input.protocol = Some(ensure_protocol(protocol, "anthropic_messages"));
    }
}

fn endpoints_for_provider_key(
    provider_type: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
    anthropic_base_url: Option<&str>,
) -> Option<EndpointUrls> {
    if !is_mimo_provider_type(provider_type) {
        return None;
    }
    let key = first_api_key(api_key?)?;
    if key.starts_with("tp-") {
        let region = preferred_token_plan_region(base_url, anthropic_base_url);
        Some(token_plan_urls(region))
    } else if key.starts_with("sk-") {
        Some(EndpointUrls {
            base_url: MIMO_PAYG_BASE_URL.to_string(),
            anthropic_base_url: MIMO_PAYG_ANTHROPIC_URL.to_string(),
        })
    } else {
        None
    }
}

fn is_mimo_provider_type(provider_type: &str) -> bool {
    let pt = provider_type.trim().to_lowercase();
    pt == "mimo" || pt == "xiaomi" || pt.contains("mimo")
}

fn is_deepseek_provider_type(provider_type: &str) -> bool {
    provider_type.trim().eq_ignore_ascii_case("deepseek")
}

fn should_replace_mimo_url(url: &str) -> bool {
    let url = url.trim();
    url.is_empty()
        || url == MIMO_PAYG_BASE_URL
        || url == MIMO_PAYG_ANTHROPIC_URL
        || token_plan_region_from_url(url).is_some()
}

fn token_plan_urls(region: &str) -> EndpointUrls {
    if let Some((_, base_url, anthropic_base_url)) = catalog::MIMO_TOKEN_PLAN_ENDPOINTS
        .iter()
        .find(|(candidate, _, _)| *candidate == region)
    {
        return EndpointUrls {
            base_url: (*base_url).to_string(),
            anthropic_base_url: (*anthropic_base_url).to_string(),
        };
    }
    token_plan_urls("cn")
}

fn preferred_token_plan_region(
    base_url: Option<&str>,
    anthropic_base_url: Option<&str>,
) -> &'static str {
    let base_region = base_url.and_then(token_plan_region_from_url);
    let anthropic_region = anthropic_base_url.and_then(token_plan_region_from_url);
    if let Some(region) = base_region {
        if region != "cn" || anthropic_region.is_none() {
            return region;
        }
    }
    anthropic_region.or(base_region).unwrap_or("cn")
}

fn token_plan_region_from_url(url: &str) -> Option<&'static str> {
    let normalized = url.trim().trim_end_matches('/').to_ascii_lowercase();
    catalog::MIMO_TOKEN_PLAN_ENDPOINTS
        .iter()
        .find(|(_, base_url, anthropic_base_url)| {
            normalized == base_url.to_ascii_lowercase()
                || normalized == anthropic_base_url.to_ascii_lowercase()
        })
        .map(|(region, _, _)| *region)
}

fn should_replace_deepseek_url(url: &str) -> bool {
    let url = url.trim();
    url.is_empty() || url == DEEPSEEK_BASE_URL || url == DEEPSEEK_ANTHROPIC_URL
}

fn first_api_key(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with('[') {
        if let Ok(keys) = serde_json::from_str::<Vec<String>>(value) {
            return keys
                .into_iter()
                .find(|key| !key.trim().is_empty())
                .map(|key| key.trim().to_string());
        }
    }
    Some(value.to_string())
}

fn ensure_protocol(protocol: &str, wanted: &str) -> String {
    let mut protocols = serde_json::from_str::<Vec<String>>(protocol)
        .unwrap_or_else(|_| vec![protocol.to_string()]);
    if !protocols.iter().any(|p| p == wanted) {
        protocols.push(wanted.to_string());
    }
    serde_json::to_string(&protocols).unwrap_or_else(|_| protocol.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(api_key: &str) -> CreateProviderInput {
        CreateProviderInput {
            name: "MiMo".into(),
            provider_type: "mimo".into(),
            base_url: MIMO_PAYG_BASE_URL.into(),
            api_key: Some(api_key.into()),
            default_model: "mimo-v2.5-pro".into(),
            reasoning_model: Some("mimo-v2.5-pro".into()),
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: Some(MIMO_PAYG_ANTHROPIC_URL.into()),
            responses_base_url: None,
            protocol: "openai_chat_completions".into(),
            timeout_seconds: Some(120),
            auto_cache_control: Some(true),
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            enabled: Some(true),
        }
    }

    #[test]
    fn token_plan_key_uses_token_plan_hosts() {
        let mut input = input("tp-xxxxx");
        apply_to_create_input(&mut input);
        assert_eq!(input.base_url, MIMO_TOKEN_PLAN_BASE_URL);
        assert_eq!(
            input.anthropic_base_url.as_deref(),
            Some(MIMO_TOKEN_PLAN_ANTHROPIC_URL)
        );
        assert!(input.protocol.contains("anthropic_messages"));
    }

    #[test]
    fn token_plan_sgp_base_url_is_preserved_and_anthropic_matches_region() {
        let mut input = input("tp-xxxxx");
        input.base_url = "https://token-plan-sgp.xiaomimimo.com/v1".into();
        input.anthropic_base_url = None;
        apply_to_create_input(&mut input);
        assert_eq!(input.base_url, "https://token-plan-sgp.xiaomimimo.com/v1");
        assert_eq!(
            input.anthropic_base_url.as_deref(),
            Some("https://token-plan-sgp.xiaomimimo.com/anthropic")
        );
    }

    #[test]
    fn token_plan_ams_anthropic_url_drives_matching_chat_url() {
        let mut input = input("tp-xxxxx");
        input.base_url = MIMO_TOKEN_PLAN_BASE_URL.into();
        input.anthropic_base_url = Some("https://token-plan-ams.xiaomimimo.com/anthropic".into());
        apply_to_create_input(&mut input);
        assert_eq!(input.base_url, "https://token-plan-ams.xiaomimimo.com/v1");
        assert_eq!(
            input.anthropic_base_url.as_deref(),
            Some("https://token-plan-ams.xiaomimimo.com/anthropic")
        );
    }

    #[test]
    fn payg_key_uses_regular_hosts() {
        let mut input = input("sk-xxxxx");
        input.base_url = MIMO_TOKEN_PLAN_BASE_URL.into();
        input.anthropic_base_url = Some(MIMO_TOKEN_PLAN_ANTHROPIC_URL.into());
        apply_to_create_input(&mut input);
        assert_eq!(input.base_url, MIMO_PAYG_BASE_URL);
        assert_eq!(
            input.anthropic_base_url.as_deref(),
            Some(MIMO_PAYG_ANTHROPIC_URL)
        );
    }

    #[test]
    fn custom_mimo_proxy_is_preserved() {
        let mut input = input("tp-xxxxx");
        input.base_url = "https://proxy.example.com/v1".into();
        input.anthropic_base_url = Some("https://proxy.example.com/anthropic".into());
        apply_to_create_input(&mut input);
        assert_eq!(input.base_url, "https://proxy.example.com/v1");
        assert_eq!(
            input.anthropic_base_url.as_deref(),
            Some("https://proxy.example.com/anthropic")
        );
    }

    #[test]
    fn json_key_array_uses_first_key() {
        let mut input = input(r#"["tp-first","sk-second"]"#);
        apply_to_create_input(&mut input);
        assert_eq!(input.base_url, MIMO_TOKEN_PLAN_BASE_URL);
    }

    #[test]
    fn update_preserves_existing_custom_proxy() {
        let mut input = UpdateProviderInput {
            name: Some("Renamed".into()),
            provider_type: None,
            base_url: None,
            api_key: None,
            default_model: None,
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            auto_cache_control: None,
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            protocol: None,
            timeout_seconds: None,
            enabled: None,
        };
        apply_to_update_input(
            "mimo",
            Some("tp-xxxxx"),
            "https://proxy.example.com/v1",
            Some("https://proxy.example.com/anthropic"),
            r#"["openai_chat_completions"]"#,
            &mut input,
        );
        assert_eq!(input.base_url, None);
        assert_eq!(input.anthropic_base_url, None);
        assert_eq!(
            input.protocol.as_deref(),
            Some(r#"["openai_chat_completions","anthropic_messages"]"#)
        );
    }

    #[test]
    fn update_preserves_existing_anthropic_protocol() {
        let mut input = UpdateProviderInput {
            name: Some("Renamed".into()),
            provider_type: None,
            base_url: None,
            api_key: None,
            default_model: None,
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            auto_cache_control: None,
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            protocol: Some(r#"["openai_chat_completions","anthropic_messages"]"#.into()),
            timeout_seconds: None,
            enabled: None,
        };
        apply_to_update_input(
            "mimo",
            Some("tp-xxxxx"),
            MIMO_PAYG_BASE_URL,
            Some(MIMO_PAYG_ANTHROPIC_URL),
            r#"["openai_chat_completions"]"#,
            &mut input,
        );
        assert_eq!(
            input.protocol.as_deref(),
            Some(r#"["openai_chat_completions","anthropic_messages"]"#)
        );
    }

    #[test]
    fn deepseek_uses_official_v4_models_and_anthropic_endpoint() {
        let mut input = CreateProviderInput {
            name: "DeepSeek".into(),
            provider_type: "deepseek".into(),
            base_url: DEEPSEEK_BASE_URL.into(),
            api_key: Some("sk-test".into()),
            default_model: "deepseek-flash".into(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".into(),
            timeout_seconds: Some(120),
            auto_cache_control: None,
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            enabled: Some(true),
        };
        apply_to_create_input(&mut input);
        assert_eq!(
            input.anthropic_base_url.as_deref(),
            Some(DEEPSEEK_ANTHROPIC_URL)
        );
        assert_eq!(input.reasoning_model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(
            input.supported_models.as_deref(),
            Some(DEEPSEEK_SUPPORTED_MODELS)
        );
        assert_eq!(
            input.protocol,
            r#"["openai_chat_completions","anthropic_messages"]"#
        );
    }

    #[test]
    fn deepseek_preserves_custom_proxy_urls() {
        let mut input = CreateProviderInput {
            name: "DeepSeek Proxy".into(),
            provider_type: "deepseek".into(),
            base_url: "https://proxy.example.com/v1".into(),
            api_key: Some("sk-test".into()),
            default_model: "deepseek-flash".into(),
            reasoning_model: Some("deepseek-v4-pro".into()),
            supported_models: Some(r#"["custom-deepseek"]"#.into()),
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: Some("https://proxy.example.com/anthropic".into()),
            responses_base_url: None,
            protocol: r#"["openai_chat_completions"]"#.into(),
            timeout_seconds: Some(120),
            auto_cache_control: None,
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            enabled: Some(true),
        };
        apply_to_create_input(&mut input);
        assert_eq!(input.base_url, "https://proxy.example.com/v1");
        assert_eq!(
            input.anthropic_base_url.as_deref(),
            Some("https://proxy.example.com/anthropic")
        );
        assert_eq!(
            input.supported_models.as_deref(),
            Some(r#"["custom-deepseek"]"#)
        );
        // 自定义代理不注入官方 Responses 端点,否则会绕过代理直连 DeepSeek
        assert_eq!(input.responses_base_url.as_deref(), None);
        assert!(!input.protocol.contains("openai_responses"));
    }

    fn deepseek_create_input() -> CreateProviderInput {
        CreateProviderInput {
            name: "DeepSeek".into(),
            provider_type: "deepseek".into(),
            base_url: DEEPSEEK_BASE_URL.into(),
            api_key: Some("sk-test".into()),
            default_model: "deepseek-flash".into(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".into(),
            timeout_seconds: Some(120),
            auto_cache_control: None,
            model_capabilities: None,
            provider_quirks: None,
            body_filter_enabled: None,
            thinking_rectifier_enabled: None,
            error_mapper_enabled: None,
            model_degradation_chain: None,
            model_context_windows: None,
            enabled: Some(true),
        }
    }

    // Responses 直通不是默认能力:catalog 不给 DeepSeek 配 responsesBaseUrl,
    // 这里也不许偷偷补 —— 想直通得用户自己在 provider 里填。
    #[test]
    fn deepseek_create_does_not_add_responses_endpoint() {
        let mut input = deepseek_create_input();
        apply_to_create_input(&mut input);
        assert_eq!(input.responses_base_url.as_deref(), None);
        assert!(!input.protocol.contains("openai_responses"));
    }

    #[test]
    fn deepseek_create_preserves_custom_responses_endpoint() {
        let mut input = deepseek_create_input();
        input.responses_base_url = Some("https://proxy.example.com/responses".into());
        apply_to_create_input(&mut input);
        assert_eq!(
            input.responses_base_url.as_deref(),
            Some("https://proxy.example.com/responses")
        );
    }

    #[test]
    fn deepseek_update_does_not_add_responses_endpoint() {
        let mut input = UpdateProviderInput::default();
        apply_to_update_input(
            "deepseek",
            Some("sk-test"),
            DEEPSEEK_BASE_URL,
            Some(DEEPSEEK_ANTHROPIC_URL),
            r#"["openai_chat_completions","anthropic_messages"]"#,
            &mut input,
        );
        assert_eq!(input.responses_base_url.as_deref(), None);
        assert_eq!(
            input.protocol.as_deref(),
            Some(r#"["openai_chat_completions","anthropic_messages"]"#)
        );
    }
}
