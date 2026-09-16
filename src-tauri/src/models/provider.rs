use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_model: String,
    pub reasoning_model: Option<String>,
    pub supported_models: Option<String>,
    pub model_mapping: Option<String>,
    pub extra_headers: Option<String>,
    pub anthropic_base_url: Option<String>, // e.g. https://api.deepseek.com/anthropic
    pub responses_base_url: Option<String>, // e.g. https://api.openai.com (for /v1/responses pass-through)
    pub protocol: String,
    pub timeout_seconds: i64,
    pub status: String,
    pub supports_vision: Option<bool>,
    pub auto_cache_control: Option<bool>,
    pub supports_cache: Option<bool>,
    pub model_capabilities: Option<String>, // JSON: {"model_id": ["text","vision",...]}
    /// Per-provider request-shape quirks consumed by gateway refiners.
    /// Serialized as `ProviderQuirks` JSON. None = no known quirks (refiners
    /// fall back to provider-type defaults seeded in `providers::capabilities`).
    pub provider_quirks: Option<String>,
    /// Per-provider override for the body_filter refiner.
    /// None = inherit gateway_settings.body_filter_global; Some(0)=off; Some(1)=on.
    pub body_filter_enabled: Option<i64>,
    /// Per-provider override for the thinking_rectifier refiner.
    pub thinking_rectifier_enabled: Option<i64>,
    /// Per-provider override for the error_mapper refiner.
    pub error_mapper_enabled: Option<i64>,
    /// JSON: {"requested_model": ["fallback1","fallback2"]}. Walked when the
    /// primary model fails before moving to the next failover provider.
    pub model_degradation_chain: Option<String>,
    /// JSON: {"model_id": context_window_tokens}. 用户覆盖 catalog 的上下文窗口值,
    /// 供 auto_compact 计算压缩阈值;优先级高于内置 catalog。
    pub model_context_windows: Option<String>,
    pub enabled: bool,
    pub is_active: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Known per-provider request quirks. Serialized into Provider.provider_quirks
/// as JSON. Every field optional so partial overrides don't have to spell out
/// the whole struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct ProviderQuirks {
    /// Top-level request fields the provider will 400 on. Body filter strips
    /// these when its switch is enabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported_fields: Vec<String>,
    /// Bounds for Anthropic-style `thinking.budget_tokens`. Thinking rectifier
    /// clamps to this range when enabled. None = no rectification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<RangeI64>,
    /// Accepted values for OpenAI-style `reasoning.effort`. Thinking rectifier
    /// rewrites unrecognized values to the closest match, or drops the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_effort_values: Vec<String>,
    /// Per-error-code overrides — provider-specific error string → mapped code.
    /// Error mapper consults this before falling back to its built-in heuristics.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub error_code_overrides: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
pub struct RangeI64 {
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub masked_api_key: Option<String>,
    pub default_model: String,
    pub reasoning_model: Option<String>,
    pub supported_models: Option<String>,
    pub model_mapping: Option<String>,
    pub extra_headers: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub responses_base_url: Option<String>,
    pub protocol: String,
    pub timeout_seconds: i64,
    pub status: String,
    pub supports_vision: Option<bool>,
    pub auto_cache_control: Option<bool>,
    pub supports_cache: Option<bool>,
    pub model_capabilities: Option<String>,
    pub provider_quirks: Option<String>,
    pub body_filter_enabled: Option<i64>,
    pub thinking_rectifier_enabled: Option<i64>,
    pub error_mapper_enabled: Option<i64>,
    pub model_degradation_chain: Option<String>,
    /// JSON: {"model_id": context_window_tokens}. 用户覆盖 catalog 的上下文窗口值,
    /// 供 auto_compact 计算压缩阈值;优先级高于内置 catalog。
    pub model_context_windows: Option<String>,
    pub enabled: bool,
    pub is_active: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Deserialize, Type)]
pub struct CreateProviderInput {
    pub name: String,
    pub provider_type: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_model: String,
    pub reasoning_model: Option<String>,
    pub supported_models: Option<String>,
    pub model_mapping: Option<String>,
    pub extra_headers: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub responses_base_url: Option<String>,
    pub protocol: String,
    pub timeout_seconds: Option<i64>,
    pub auto_cache_control: Option<bool>,
    pub model_capabilities: Option<String>,
    pub provider_quirks: Option<String>,
    pub body_filter_enabled: Option<i64>,
    pub thinking_rectifier_enabled: Option<i64>,
    pub error_mapper_enabled: Option<i64>,
    pub model_degradation_chain: Option<String>,
    pub model_context_windows: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Type)]
pub struct UpdateProviderInput {
    pub name: Option<String>,
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub reasoning_model: Option<String>,
    pub supported_models: Option<String>,
    pub model_mapping: Option<String>,
    pub extra_headers: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub responses_base_url: Option<String>,
    pub auto_cache_control: Option<bool>,
    pub model_capabilities: Option<String>,
    pub provider_quirks: Option<String>,
    pub body_filter_enabled: Option<i64>,
    pub thinking_rectifier_enabled: Option<i64>,
    pub error_mapper_enabled: Option<i64>,
    pub model_degradation_chain: Option<String>,
    pub model_context_windows: Option<String>,
    pub protocol: Option<String>,
    pub timeout_seconds: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct ProviderTestResult {
    pub success: bool,
    pub status: String,
    pub message: String,
    pub latency_ms: Option<u64>,
    pub supports_vision: Option<bool>,
    /// Structured failure diagnosis (only present on failure paths).
    /// Older clients that ignore this field still see the legacy `message`
    /// string verbatim — backward compatible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<crate::diagnostics::test_failure::TestDiagnostic>,
}

impl Provider {
    pub fn to_view(&self) -> ProviderView {
        ProviderView {
            id: self.id.clone(),
            name: self.name.clone(),
            provider_type: self.provider_type.clone(),
            base_url: self.base_url.clone(),
            masked_api_key: self.api_key.as_ref().map(|k| mask_api_key(k)),
            default_model: self.default_model.clone(),
            reasoning_model: self.reasoning_model.clone(),
            supported_models: self.supported_models.clone(),
            model_mapping: self.model_mapping.clone(),
            extra_headers: self.extra_headers.clone(),
            anthropic_base_url: self.anthropic_base_url.clone(),
            responses_base_url: self.responses_base_url.clone(),
            protocol: self.protocol.clone(),
            timeout_seconds: self.timeout_seconds,
            status: self.status.clone(),
            supports_vision: self.supports_vision,
            auto_cache_control: self.auto_cache_control,
            supports_cache: self.supports_cache,
            model_capabilities: self.model_capabilities.clone(),
            provider_quirks: self.provider_quirks.clone(),
            body_filter_enabled: self.body_filter_enabled,
            thinking_rectifier_enabled: self.thinking_rectifier_enabled,
            error_mapper_enabled: self.error_mapper_enabled,
            model_degradation_chain: self.model_degradation_chain.clone(),
            model_context_windows: self.model_context_windows.clone(),
            enabled: self.enabled,
            is_active: self.is_active,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }

    /// Parse `provider_quirks` JSON into a `ProviderQuirks` struct.
    /// Returns the default (empty) struct on missing/invalid JSON so callers
    /// can chain methods without unwrap noise.
    pub fn parse_quirks(&self) -> ProviderQuirks {
        self.provider_quirks
            .as_deref()
            .and_then(|s| serde_json::from_str::<ProviderQuirks>(s).ok())
            .unwrap_or_default()
    }

    /// Parse `model_degradation_chain` JSON into `{model → [fallbacks]}`.
    /// Returns empty map on missing/invalid JSON.
    pub fn parse_degradation_chain(&self) -> std::collections::HashMap<String, Vec<String>> {
        self.model_degradation_chain
            .as_deref()
            .and_then(|s| {
                serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(s).ok()
            })
            .unwrap_or_default()
    }

    /// Resolve effective on/off for a refiner switch. `per_provider` is the
    /// 3-state column (None/0/1); `global` is the gateway-wide default
    /// (0/1). The decision rule is "global is the master kill — when global
    /// is off, no per-provider opt-in can turn the refiner on":
    ///   - global=0  → always off (per-provider opt-in ignored)
    ///   - global=1, per_provider=None → on (inherit)
    ///   - global=1, per_provider=Some(0) → off (per-provider opt-out)
    ///   - global=1, per_provider=Some(1) → on
    pub fn refiner_effective(per_provider: Option<i64>, global: bool) -> bool {
        if !global {
            return false;
        }
        match per_provider {
            None => true,
            Some(0) => false,
            Some(_) => true,
        }
    }

    /// Parse the protocol field as a JSON array. Falls back to treating it as a single value.
    #[allow(dead_code)]
    pub fn protocols(&self) -> Vec<String> {
        if let Ok(list) = serde_json::from_str::<Vec<String>>(&self.protocol) {
            list
        } else {
            vec![self.protocol.clone()]
        }
    }

    /// Check if this provider supports a given protocol.
    #[allow(dead_code)]
    pub fn supports_protocol(&self, protocol: &str) -> bool {
        self.protocols().iter().any(|p| p == protocol)
    }

    /// Check if a model name is known to this provider.
    pub fn is_model_supported(&self, model: &str) -> bool {
        if model == self.default_model {
            return true;
        }
        if self.reasoning_model.as_deref() == Some(model) {
            return true;
        }
        if let Some(ref sm) = self.supported_models {
            if let Ok(list) = serde_json::from_str::<Vec<String>>(sm) {
                if list.iter().any(|m| m == model) {
                    return true;
                }
            }
        }
        false
    }

    /// Parse `model_capabilities` JSON into a {model_id → capability set} map.
    /// Returns empty map on missing/invalid JSON.
    pub fn parse_capabilities(&self) -> std::collections::HashMap<String, Vec<String>> {
        self.model_capabilities
            .as_deref()
            .and_then(|s| {
                serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(s).ok()
            })
            .unwrap_or_default()
    }

    /// Parse `model_context_windows` JSON into a {model_id → window_tokens} map.
    /// Returns empty map on missing/invalid JSON.
    pub fn parse_context_windows(&self) -> std::collections::HashMap<String, u32> {
        self.model_context_windows
            .as_deref()
            .and_then(|s| serde_json::from_str::<std::collections::HashMap<String, u32>>(s).ok())
            .unwrap_or_default()
    }

    /// List models declared to support the given capability, preserving the
    /// `supported_models` order so the caller can pick the highest-priority
    /// candidate. Falls back to default_model / reasoning_model when those
    /// happen to be listed in the matrix.
    pub fn models_with_capability(&self, capability: &str) -> Vec<String> {
        let caps = self.parse_capabilities();
        if caps.is_empty() {
            return Vec::new();
        }
        // Use supported_models order if available, else any order from the map
        let order: Vec<String> = if let Some(ref sm) = self.supported_models {
            serde_json::from_str::<Vec<String>>(sm).unwrap_or_default()
        } else {
            caps.keys().cloned().collect()
        };
        order
            .into_iter()
            .filter(|m| {
                caps.get(m)
                    .map(|c| c.iter().any(|x| x == capability))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Pick the first model from supported_models that has the given capability.
    /// Returns None if nothing matches; callers should fall back to default_model
    /// and accept that the request may fail upstream.
    pub fn pick_model_for_capability(&self, capability: &str) -> Option<String> {
        self.models_with_capability(capability).into_iter().next()
    }

    /// Does ANY model on this provider support the given capability? Used by
    /// the card UI to show capability icons without having to inspect each model.
    pub fn any_model_supports(&self, capability: &str) -> bool {
        self.parse_capabilities()
            .values()
            .any(|caps| caps.iter().any(|c| c == capability))
    }

    /// Resolve a client model name to the provider's actual model name.
    /// Priority: model_mapping → supported_models match → default_model.
    pub fn resolve_model(&self, requested: &str) -> String {
        // 1. Check model_mapping first
        if let Some(ref mm) = self.model_mapping {
            if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, String>>(mm) {
                if let Some(mapped) = map.get(requested) {
                    return mapped.clone();
                }
            }
        }
        // 2. If the requested model is natively supported, use it directly
        if self.is_model_supported(requested) {
            return requested.to_string();
        }
        // 3. Fallback to default_model
        self.default_model.clone()
    }
}

pub fn mask_api_key(raw: &str) -> String {
    let trimmed = raw.trim();
    // JSON array: show first key masked + count
    if trimmed.starts_with('[') {
        if let Ok(keys) = serde_json::from_str::<Vec<String>>(trimmed) {
            let valid: Vec<&String> = keys.iter().filter(|k| !k.is_empty()).collect();
            if valid.is_empty() {
                return "***".to_string();
            }
            let first = mask_single_key(valid[0]);
            if valid.len() == 1 {
                return first;
            }
            return format!("{first} (+{} more)", valid.len() - 1);
        }
    }
    mask_single_key(trimmed)
}

fn mask_single_key(key: &str) -> String {
    if key.len() <= 8 {
        return "*".repeat(key.len());
    }
    let prefix = &key[..4];
    let suffix = &key[key.len() - 4..];
    format!("{prefix}****{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_api_key_short() {
        assert_eq!(mask_api_key("abc"), "***");
        assert_eq!(mask_api_key("abcdefgh"), "********");
    }

    #[test]
    fn test_mask_api_key_long() {
        let key = "sk-1234567890abcdef";
        let masked = mask_api_key(key);
        assert_eq!(masked, "sk-1****cdef");
    }

    #[test]
    fn test_provider_is_model_supported() {
        let provider = Provider {
            id: "1".to_string(),
            name: "Test".to_string(),
            provider_type: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: None,
            default_model: "gpt-4".to_string(),
            reasoning_model: Some("o1".to_string()),
            supported_models: Some("[\"gpt-3\", \"gpt-4-turbo\"]".to_string()),
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".to_string(),
            timeout_seconds: 60,
            status: "ok".to_string(),
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
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
        };

        assert!(provider.is_model_supported("gpt-4"));
        assert!(provider.is_model_supported("o1"));
        assert!(provider.is_model_supported("gpt-3"));
        assert!(provider.is_model_supported("gpt-4-turbo"));
        assert!(!provider.is_model_supported("unknown"));
    }

    #[test]
    fn test_provider_resolve_model_mapping() {
        let provider = Provider {
            id: "1".to_string(),
            name: "Test".to_string(),
            provider_type: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: None,
            default_model: "gpt-4".to_string(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: Some(r#"{"gpt-5": "gpt-4-turbo", "custom": "gpt-3"}"#.to_string()),
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".to_string(),
            timeout_seconds: 60,
            status: "ok".to_string(),
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
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
        };

        assert_eq!(provider.resolve_model("gpt-5"), "gpt-4-turbo");
        assert_eq!(provider.resolve_model("custom"), "gpt-3");
    }

    #[test]
    fn test_provider_resolve_model_supported() {
        let provider = Provider {
            id: "1".to_string(),
            name: "Test".to_string(),
            provider_type: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: None,
            default_model: "gpt-4".to_string(),
            reasoning_model: None,
            supported_models: Some("[\"gpt-3\"]".to_string()),
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".to_string(),
            timeout_seconds: 60,
            status: "ok".to_string(),
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
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
        };

        assert_eq!(provider.resolve_model("gpt-3"), "gpt-3");
        assert_eq!(provider.resolve_model("unknown"), "gpt-4"); // fallback
    }

    #[test]
    fn test_provider_to_view() {
        let provider = Provider {
            id: "1".to_string(),
            name: "Test".to_string(),
            provider_type: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: Some("sk-secret123".to_string()),
            default_model: "gpt-4".to_string(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".to_string(),
            timeout_seconds: 60,
            status: "ok".to_string(),
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
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
        };

        let view = provider.to_view();
        assert_eq!(view.masked_api_key, Some("sk-s****t123".to_string()));
    }

    #[test]
    fn test_protocols_json_array() {
        let provider = Provider {
            id: "1".to_string(),
            name: "Test".to_string(),
            provider_type: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: None,
            default_model: "gpt-4".to_string(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: r#"["openai_chat_completions","openai_responses"]"#.to_string(),
            timeout_seconds: 60,
            status: "ok".to_string(),
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
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
        };
        assert_eq!(
            provider.protocols(),
            vec!["openai_chat_completions", "openai_responses"]
        );
        assert!(provider.supports_protocol("openai_chat_completions"));
        assert!(provider.supports_protocol("openai_responses"));
        assert!(!provider.supports_protocol("anthropic_messages"));
    }

    #[test]
    fn test_protocols_single_string_fallback() {
        let provider = Provider {
            id: "1".to_string(),
            name: "Test".to_string(),
            provider_type: "deepseek".to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            api_key: None,
            default_model: "deepseek-flash".to_string(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: "openai_chat_completions".to_string(),
            timeout_seconds: 60,
            status: "ok".to_string(),
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
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
        };
        assert_eq!(provider.protocols(), vec!["openai_chat_completions"]);
        assert!(provider.supports_protocol("openai_chat_completions"));
        assert!(!provider.supports_protocol("openai_responses"));
    }

    #[test]
    fn test_protocols_three_protocols() {
        let provider = Provider {
            id: "1".to_string(),
            name: "NewAPI".to_string(),
            provider_type: "custom_openai_compatible".to_string(),
            base_url: "https://newapi.example.com".to_string(),
            api_key: None,
            default_model: "gpt-4o".to_string(),
            reasoning_model: None,
            supported_models: None,
            model_mapping: None,
            extra_headers: None,
            anthropic_base_url: None,
            responses_base_url: None,
            protocol: r#"["openai_chat_completions","openai_responses","anthropic_messages"]"#
                .to_string(),
            timeout_seconds: 60,
            status: "ok".to_string(),
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
            created_at: "2024-01-01".to_string(),
            updated_at: "2024-01-01".to_string(),
        };
        assert!(provider.supports_protocol("openai_chat_completions"));
        assert!(provider.supports_protocol("openai_responses"));
        assert!(provider.supports_protocol("anthropic_messages"));
    }
}
