//! Capability seed for the model_capabilities matrix.
//!
//! Encodes what we know about specific model IDs so the UI can offer
//! "auto-detect" instead of forcing users to tick every box by hand.
//! Provider-specific rules first (MiMo / DeepSeek), then a generic
//! fallback that infers caps from the model name (anything with "vision"
//! / "tts" / etc. in the id).
//!
//! Canonical capability strings (keep stable — DB content depends on these):
//!   - text       text input/output (almost every chat model)
//!   - vision     image input
//!   - audio_in   audio input (omni)
//!   - tts        audio output / synthesis
//!   - video_in   video input
//!   - reasoning  thinking mode
//!   - tools      function/tool calling
//!   - web_search native web search builtin
//!   - cache      Anthropic-style prompt caching

use crate::providers::model_id::strip_qualifier;
use crate::storage::generated_provider_catalog as catalog;

pub const CAP_TEXT: &str = "text";
pub const CAP_VISION: &str = "vision";
pub const CAP_AUDIO_IN: &str = "audio_in";
pub const CAP_TTS: &str = "tts";
pub const CAP_VIDEO_IN: &str = "video_in";
pub const CAP_REASONING: &str = "reasoning";
pub const CAP_TOOLS: &str = "tools";
pub const CAP_WEB_SEARCH: &str = "web_search";

/// All canonical capability strings, for UI rendering. Order matters — this is
/// also the column order shown in the matrix editor.
pub const ALL_CAPABILITIES: &[&str] = &[
    CAP_TEXT,
    CAP_VISION,
    CAP_AUDIO_IN,
    CAP_TTS,
    CAP_VIDEO_IN,
    CAP_REASONING,
    CAP_TOOLS,
    CAP_WEB_SEARCH,
];

/// Auto-derive a model's capabilities given its provider type and id.
/// Returns a sorted, de-duplicated vec of capability strings.
pub fn seed_for_model(provider_type: &str, model_id: &str) -> Vec<String> {
    let pt = provider_type.to_ascii_lowercase();
    let mid = model_id.trim().to_ascii_lowercase();

    if let Some(catalog_provider) = catalog_provider_type(&pt) {
        let base = strip_qualifier(&mid);
        if is_catalog_deprecated_model(catalog_provider, base) {
            return Vec::new();
        }
        if let Some(caps) = catalog_capabilities(catalog_provider, base) {
            return caps;
        }
    }

    // Try remaining provider-specific rules first.
    let caps = match pt.as_str() {
        p if p == "kimi" || p == "moonshot" || p.contains("moonshot") => seed_kimi(&mid),
        _ => Vec::new(),
    };
    if !caps.is_empty() {
        return dedup_sort(caps);
    }

    // Generic fallback: pattern-match the model name
    dedup_sort(seed_generic(&mid))
}

fn seed_kimi(mid: &str) -> Vec<String> {
    let base = strip_qualifier(mid);
    // Kimi web_search is provided server-side via the `$web_search` builtin
    // (AgentGate already translates Codex's web_search_preview → builtin_function
    // in tool_calls.rs), so we mark web_search universally for Kimi models.
    match base {
        // K3 flagship (Platform: kimi-k3, Kimi Code: k3) — vision + reasoning + 1M.
        "k3" | "kimi-k3" => vec![
            CAP_TEXT.into(),
            CAP_VISION.into(),
            CAP_VIDEO_IN.into(),
            CAP_REASONING.into(),
            CAP_TOOLS.into(),
            CAP_WEB_SEARCH.into(),
        ],
        // Coding models (Platform + Kimi Code IDs, incl. highspeed).
        "kimi-for-coding"
        | "kimi-for-coding-highspeed"
        | "kimi-k2.7-code"
        | "kimi-k2.7-code-highspeed" => vec![
            CAP_TEXT.into(),
            CAP_VISION.into(),
            CAP_REASONING.into(),
            CAP_TOOLS.into(),
            CAP_WEB_SEARCH.into(),
        ],
        // Moonshot's explicit vision models.
        m if m.contains("vision") => vec![
            CAP_TEXT.into(),
            CAP_VISION.into(),
            CAP_TOOLS.into(),
            CAP_WEB_SEARCH.into(),
        ],
        // Kimi K-series + standard Moonshot chat models. K2 / K2.6 support
        // vision natively; web_search builtin available on all.
        "kimi-k2" | "kimi-k2.6" => vec![
            CAP_TEXT.into(),
            CAP_VISION.into(),
            CAP_TOOLS.into(),
            CAP_WEB_SEARCH.into(),
        ],
        _ => vec![CAP_TEXT.into(), CAP_TOOLS.into(), CAP_WEB_SEARCH.into()],
    }
}

fn catalog_provider_type(provider_type: &str) -> Option<&'static str> {
    match provider_type {
        p if p == "mimo" || p == "xiaomi" || p.contains("mimo") => Some("mimo"),
        "deepseek" => Some("deepseek"),
        _ => None,
    }
}

fn catalog_capabilities(provider_type: &str, model_id: &str) -> Option<Vec<String>> {
    catalog::MODEL_CAPABILITIES
        .iter()
        .find(|(provider, model, _)| *provider == provider_type && *model == model_id)
        .map(|(_, _, caps)| dedup_sort(caps.iter().map(|cap| (*cap).to_string()).collect()))
}

/// 查模型的上下文窗口(token 数),数据来自 provider-catalog。
/// 未收录的模型返回 None,调用方自行决定 fallback。
pub fn context_window_for(provider_type: &str, model_id: &str) -> Option<u32> {
    let pt = catalog_provider_type(provider_type).unwrap_or(provider_type);
    catalog::MODEL_CONTEXT_WINDOW
        .iter()
        .find(|(provider, model, _)| *provider == pt && *model == model_id)
        .map(|(_, _, window)| *window)
}

fn is_catalog_deprecated_model(provider_type: &str, model_id: &str) -> bool {
    catalog::DEPRECATED_MODELS
        .iter()
        .any(|(provider, model)| *provider == provider_type && *model == model_id)
}

fn seed_generic(mid: &str) -> Vec<String> {
    let mut caps: Vec<String> = vec![CAP_TEXT.into(), CAP_TOOLS.into()];
    if mid.contains("vision") || mid.contains("-vl") || mid.contains("omni") {
        caps.push(CAP_VISION.into());
    }
    if mid.contains("omni") {
        caps.extend([CAP_AUDIO_IN.into(), CAP_VIDEO_IN.into()]);
    }
    if mid.contains("tts") || mid.contains("speech") {
        return vec![CAP_TTS.into()];
    }
    if mid.contains("reason")
        || mid.contains("think")
        || mid.contains("-r1")
        || mid.contains("o1")
        || mid.contains("o3")
    {
        caps.push(CAP_REASONING.into());
    }
    caps
}

fn dedup_sort(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

/// Auto-derive capabilities for a whole list of models. Returns a map suitable
/// for serializing into the `model_capabilities` JSON column.
pub fn seed_for_models(
    provider_type: &str,
    model_ids: &[String],
) -> std::collections::HashMap<String, Vec<String>> {
    model_ids
        .iter()
        .map(|m| (m.clone(), seed_for_model(provider_type, m)))
        .collect()
}

/// Default-known request-shape quirks for a provider type. Empty by default —
/// only seeded for providers where AgentGate has direct evidence (404/400 in
/// the wild). Refiner consumers should treat the returned struct as a starting
/// point that the user can override via `provider_quirks` JSON.
///
/// Keep this list narrow on purpose: incorrectly listing a field as unsupported
/// silently drops it when body_filter is on, which breaks features. Only add a
/// field once we've confirmed the provider 400s on it.
pub fn default_quirks_for_provider(provider_type: &str) -> crate::models::provider::ProviderQuirks {
    use crate::models::provider::{ProviderQuirks, RangeI64};
    let pt = provider_type.to_ascii_lowercase();
    let mut q = ProviderQuirks::default();
    match pt.as_str() {
        // DeepSeek's OpenAI-compatible endpoint silently ignores `web_search`
        // top-level (no native builtin); the Anthropic-compatible endpoint
        // 400s on `cache_control`. We strip the worst offender by default
        // and let the user opt out per-provider if they know better.
        "deepseek" => {
            q.unsupported_fields = vec!["web_search".into(), "web_search_options".into()];
        }
        // MiMo: Anthropic-compatible endpoint accepts cache_control; the
        // OpenAI-compatible endpoint rejects it with 400. The body filter
        // is route-agnostic so we only strip on confirmed-bad shapes.
        // (Server-side `$web_search` works via the Plugin path — see
        // adapter.rs MIMO_WEB_SEARCH_DISABLED cache.)
        "mimo" | "xiaomi" => {
            q.thinking_budget = Some(RangeI64 {
                min: 1024,
                max: 32_768,
            });
        }
        // Anthropic Messages: reasoning effort is via `thinking.budget_tokens`
        // (Sonnet/Haiku >= 1024, Opus >= 2048). Clamp upper bound to avoid
        // wasting credits.
        "anthropic" => {
            q.thinking_budget = Some(RangeI64 {
                min: 1024,
                max: 64_000,
            });
        }
        // OpenAI Responses: reasoning.effort takes "minimal" / "low" / "medium" / "high".
        "openai" | "azure_openai" => {
            q.reasoning_effort_values = vec![
                "minimal".into(),
                "low".into(),
                "medium".into(),
                "high".into(),
            ];
        }
        // Kimi: rejects unknown reasoning-style fields; the $web_search
        // builtin works via the Plugin name, not the OpenAI `web_search_options`.
        // K3 accepts top-level reasoning_effort (currently only "max").
        "kimi" | "moonshot" => {
            q.unsupported_fields = vec!["web_search_options".into(), "reasoning".into()];
            q.reasoning_effort_values = vec!["max".into()];
        }
        _ => {}
    }
    q
}

#[cfg(test)]
mod quirks_tests {
    use super::*;

    #[test]
    fn deepseek_default_drops_web_search() {
        let q = default_quirks_for_provider("deepseek");
        assert!(q.unsupported_fields.iter().any(|f| f == "web_search"));
    }

    #[test]
    fn mimo_default_has_thinking_budget_range() {
        let q = default_quirks_for_provider("mimo");
        let r = q
            .thinking_budget
            .expect("MiMo should advertise thinking range");
        assert_eq!(r.min, 1024);
        assert!(r.max >= r.min);
    }

    #[test]
    fn openai_default_lists_reasoning_effort_values() {
        let q = default_quirks_for_provider("openai");
        assert!(q.reasoning_effort_values.contains(&"medium".to_string()));
        assert!(q.reasoning_effort_values.contains(&"high".to_string()));
    }

    #[test]
    fn unknown_provider_returns_empty_quirks() {
        let q = default_quirks_for_provider("some-new-thing");
        assert!(q.unsupported_fields.is_empty());
        assert!(q.thinking_budget.is_none());
        assert!(q.reasoning_effort_values.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps_for(provider: &str, model: &str) -> Vec<String> {
        seed_for_model(provider, model)
    }

    fn contains(caps: &[String], expected: &str) -> bool {
        caps.iter().any(|c| c == expected)
    }

    // ── MiMo ──

    #[test]
    fn mimo_v25_pro_has_reasoning_no_vision() {
        let c = caps_for("mimo", "mimo-v2.5-pro");
        assert!(contains(&c, CAP_REASONING));
        assert!(contains(&c, CAP_WEB_SEARCH));
        assert!(!contains(&c, CAP_VISION));
        assert!(!contains(&c, CAP_TTS));
    }

    #[test]
    fn mimo_v25_has_vision() {
        let c = caps_for("mimo", "mimo-v2.5");
        assert!(contains(&c, CAP_TEXT));
        assert!(contains(&c, CAP_VISION));
        assert!(contains(&c, CAP_REASONING));
    }

    #[test]
    fn mimo_v2_omni_has_vision_audio_video_but_no_reasoning() {
        let c = caps_for("mimo", "mimo-v2-omni");
        assert!(contains(&c, CAP_VISION));
        assert!(contains(&c, CAP_AUDIO_IN));
        assert!(contains(&c, CAP_VIDEO_IN));
        assert!(
            !contains(&c, CAP_REASONING),
            "omni doesn't have thinking mode per docs"
        );
    }

    #[test]
    fn mimo_v2_flash_has_reasoning_no_vision() {
        let c = caps_for("mimo", "mimo-v2-flash");
        assert!(contains(&c, CAP_REASONING));
        assert!(!contains(&c, CAP_VISION));
    }

    #[test]
    fn mimo_tts_models_are_tts_only() {
        for tts in [
            "mimo-v2.5-tts",
            "mimo-v2.5-tts-voiceclone",
            "mimo-v2.5-tts-voicedesign",
            "mimo-v2-tts",
        ] {
            let c = caps_for("mimo", tts);
            assert_eq!(c, vec![CAP_TTS], "{tts} should be tts-only, got {c:?}");
        }
    }

    #[test]
    fn mimo_with_1m_qualifier_resolves_same_as_base() {
        assert_eq!(
            caps_for("mimo", "mimo-v2.5-pro[1m]"),
            caps_for("mimo", "mimo-v2.5-pro")
        );
    }

    #[test]
    fn xiaomi_alias_works() {
        assert!(contains(&caps_for("xiaomi", "mimo-v2.5"), CAP_VISION));
    }

    // ── DeepSeek ──

    #[test]
    fn deepseek_v4_pro_has_reasoning_no_vision() {
        let c = caps_for("deepseek", "deepseek-v4-pro");
        assert!(contains(&c, CAP_REASONING));
        assert!(
            !contains(&c, CAP_WEB_SEARCH),
            "DeepSeek has no native web_search builtin"
        );
        assert!(!contains(&c, CAP_VISION));
    }

    #[test]
    fn deepseek_flash_has_vision_reasoning_no_web_search() {
        let c = caps_for("deepseek", "deepseek-flash");
        assert!(contains(&c, CAP_TEXT));
        assert!(contains(&c, CAP_TOOLS));
        assert!(contains(&c, CAP_REASONING));
        assert!(contains(&c, CAP_VISION));
        assert!(
            !contains(&c, CAP_WEB_SEARCH),
            "DeepSeek has no native web_search builtin"
        );
    }

    #[test]
    fn deepseek_with_1m_qualifier() {
        assert_eq!(
            caps_for("deepseek", "deepseek-v4-pro[1m]"),
            caps_for("deepseek", "deepseek-v4-pro")
        );
    }

    #[test]
    fn deepseek_deprecated_aliases_are_not_seeded() {
        assert!(caps_for("deepseek", "deepseek-chat").is_empty());
        assert!(caps_for("deepseek", "deepseek-reasoner").is_empty());
        assert!(caps_for("deepseek", "deepseek-v4-flash").is_empty());
        assert!(caps_for("deepseek", "deepseek-v4-flash-vision-exp").is_empty());
    }

    // ── Kimi / Moonshot ──

    #[test]
    fn kimi_for_coding_has_vision_and_web_search() {
        let c = caps_for("kimi", "kimi-for-coding");
        assert!(
            contains(&c, CAP_VISION),
            "kimi-for-coding accepts image input per upstream confirmation"
        );
        assert!(
            contains(&c, CAP_WEB_SEARCH),
            "Kimi $web_search builtin is universally translated"
        );
        assert!(contains(&c, CAP_TOOLS));
        assert!(contains(&c, CAP_REASONING));
    }

    #[test]
    fn kimi_k3_has_vision_reasoning_and_1m_context() {
        for mid in ["kimi-k3", "k3", "k3[1m]"] {
            let c = caps_for("kimi", mid);
            assert!(contains(&c, CAP_VISION), "{mid} vision");
            assert!(contains(&c, CAP_REASONING), "{mid} reasoning");
            assert!(contains(&c, CAP_TOOLS), "{mid} tools");
            assert!(contains(&c, CAP_WEB_SEARCH), "{mid} web_search");
        }
        assert_eq!(
            context_window_for("kimi", "kimi-k3"),
            Some(1_048_576),
            "catalog must seed K3 1M context"
        );
        assert_eq!(context_window_for("kimi", "k3"), Some(1_048_576));
    }

    #[test]
    fn kimi_highspeed_coding_models_seeded() {
        for mid in ["kimi-for-coding-highspeed", "kimi-k2.7-code-highspeed"] {
            let c = caps_for("kimi", mid);
            assert!(contains(&c, CAP_VISION), "{mid}");
            assert!(contains(&c, CAP_REASONING), "{mid}");
        }
        assert_eq!(context_window_for("kimi", "kimi-for-coding"), Some(262_144));
    }

    #[test]
    fn kimi_k2_has_vision() {
        let c = caps_for("kimi", "kimi-k2");
        assert!(contains(&c, CAP_VISION));
    }

    #[test]
    fn kimi_default_quirks_list_max_effort() {
        let q = default_quirks_for_provider("kimi");
        assert!(q.reasoning_effort_values.contains(&"max".to_string()));
    }

    #[test]
    fn moonshot_vision_models_have_vision() {
        let c = caps_for("moonshot", "moonshot-v1-8k-vision-preview");
        assert!(contains(&c, CAP_VISION));
    }

    #[test]
    fn kimi_generic_chat_has_web_search_no_vision() {
        let c = caps_for("kimi", "moonshot-v1-32k");
        assert!(contains(&c, CAP_WEB_SEARCH));
        assert!(
            !contains(&c, CAP_VISION),
            "conservative default — user opts in per model"
        );
    }

    // ── Generic ──

    #[test]
    fn generic_vision_keyword_triggers_vision() {
        let c = caps_for("openai", "gpt-4-vision");
        assert!(contains(&c, CAP_VISION));
    }

    #[test]
    fn generic_omni_triggers_vision_audio_video() {
        let c = caps_for("openai", "gpt-4o-omni");
        assert!(contains(&c, CAP_VISION));
        assert!(contains(&c, CAP_AUDIO_IN));
        assert!(contains(&c, CAP_VIDEO_IN));
    }

    #[test]
    fn generic_tts_is_tts_only() {
        let c = caps_for("openai", "tts-1");
        assert_eq!(c, vec![CAP_TTS]);
    }

    #[test]
    fn generic_o1_has_reasoning() {
        let c = caps_for("openai", "o1-preview");
        assert!(contains(&c, CAP_REASONING));
    }

    // ── Batch helper ──

    #[test]
    fn seed_for_models_returns_full_matrix() {
        let models = vec![
            "mimo-v2.5-pro".to_string(),
            "mimo-v2.5".to_string(),
            "mimo-v2.5-tts".to_string(),
        ];
        let matrix = seed_for_models("mimo", &models);
        assert_eq!(matrix.len(), 3);
        assert!(matrix
            .get("mimo-v2.5-pro")
            .unwrap()
            .contains(&CAP_REASONING.to_string()));
        assert!(matrix
            .get("mimo-v2.5")
            .unwrap()
            .contains(&CAP_VISION.to_string()));
        assert_eq!(
            matrix.get("mimo-v2.5-tts").unwrap(),
            &vec![CAP_TTS.to_string()]
        );
    }
}
