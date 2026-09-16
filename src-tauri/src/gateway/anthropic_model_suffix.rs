//! Model name handling for the Anthropic Messages passthrough path.
//!
//! MiMo / DeepSeek previously used `[1m]` suffixed model IDs in recommended
//! Claude Code mappings, but the real upstream can reject those IDs with
//! "Not supported model". The gateway now strips that legacy suffix before
//! forwarding to those providers.

/// Rewrite the outgoing model field for the Anthropic passthrough path. If
/// the provider has no CC-only rewrite rule, returns the model unchanged.
pub fn for_anthropic(provider_type: &str, model: &str) -> String {
    let pt = provider_type.trim().to_lowercase();
    if (pt == "mimo" || pt == "xiaomi" || pt.contains("mimo") || pt == "deepseek")
        && model.ends_with("[1m]")
    {
        return model.trim_end_matches("[1m]").to_string();
    }
    // Copilot 上游只接受 dot 形式的 Claude 4.x 模型 ID(claude-sonnet-4.6),
    // Claude Code 发 dash 形式会 400 model_not_supported。
    if crate::providers::copilot::is_copilot(&pt) {
        if let Some(normalized) = crate::providers::copilot::normalize_model(model) {
            return normalized;
        }
    }
    model.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── MiMo ──

    #[test]
    fn mimo_v25_pro_passes_through() {
        assert_eq!(for_anthropic("mimo", "mimo-v2.5-pro"), "mimo-v2.5-pro");
    }

    #[test]
    fn mimo_v2_pro_passes_through() {
        assert_eq!(for_anthropic("mimo", "mimo-v2-pro"), "mimo-v2-pro");
    }

    #[test]
    fn mimo_v25_passes_through() {
        assert_eq!(for_anthropic("mimo", "mimo-v2.5"), "mimo-v2.5");
    }

    #[test]
    fn mimo_flash_does_not_get_suffix() {
        assert_eq!(for_anthropic("mimo", "mimo-v2-flash"), "mimo-v2-flash");
    }

    #[test]
    fn mimo_omni_does_not_get_suffix() {
        // Omni tops at 256K.
        assert_eq!(for_anthropic("mimo", "mimo-v2-omni"), "mimo-v2-omni");
    }

    #[test]
    fn mimo_legacy_1m_suffix_is_stripped() {
        assert_eq!(for_anthropic("mimo", "mimo-v2.5-pro[1m]"), "mimo-v2.5-pro");
    }

    #[test]
    fn mimo_other_qualifier_respected() {
        // If user explicitly wrote [128k] or anything else, don't touch.
        assert_eq!(
            for_anthropic("mimo", "mimo-v2.5-pro[128k]"),
            "mimo-v2.5-pro[128k]"
        );
    }

    #[test]
    fn xiaomi_alias_recognized() {
        assert_eq!(for_anthropic("xiaomi", "mimo-v2.5-pro"), "mimo-v2.5-pro");
    }

    // ── DeepSeek ──

    #[test]
    fn deepseek_v4_pro_passes_through() {
        assert_eq!(
            for_anthropic("deepseek", "deepseek-v4-pro"),
            "deepseek-v4-pro"
        );
    }

    #[test]
    fn deepseek_flash_passes_through() {
        assert_eq!(
            for_anthropic("deepseek", "deepseek-flash"),
            "deepseek-flash"
        );
    }

    #[test]
    fn deepseek_legacy_1m_suffix_is_stripped() {
        assert_eq!(
            for_anthropic("deepseek", "deepseek-v4-pro[1m]"),
            "deepseek-v4-pro"
        );
    }

    // ── Copilot ──

    #[test]
    fn copilot_dash_model_normalized_to_dot() {
        assert_eq!(
            for_anthropic("copilot", "claude-sonnet-4-6"),
            "claude-sonnet-4.6"
        );
        assert_eq!(
            for_anthropic("copilot", "claude-sonnet-4-6[1m]"),
            "claude-sonnet-4.6-1m"
        );
    }

    #[test]
    fn copilot_already_dotted_untouched() {
        assert_eq!(
            for_anthropic("copilot", "claude-sonnet-4.6"),
            "claude-sonnet-4.6"
        );
        assert_eq!(for_anthropic("copilot", "gpt-5"), "gpt-5");
    }

    // ── Other providers ──

    #[test]
    fn anthropic_native_unaffected() {
        assert_eq!(
            for_anthropic("anthropic", "claude-sonnet-4-6"),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn openai_unaffected() {
        assert_eq!(for_anthropic("openai", "gpt-4o"), "gpt-4o");
    }

    #[test]
    fn unknown_provider_unaffected() {
        assert_eq!(
            for_anthropic("custom_openai_compatible", "anything"),
            "anything"
        );
    }
}
