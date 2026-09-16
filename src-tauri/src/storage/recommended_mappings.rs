use std::collections::BTreeMap;

use rusqlite::{params, Connection};

use crate::errors::AppError;
use crate::models::provider::{CreateProviderInput, Provider};
use crate::storage::generated_provider_catalog as catalog;

const CODEX_MODELS: &[&str] = &[
    "gpt-5.6",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.3-codex",
    "gpt-5.2",
];
const CODEX_MINI_MODELS: &[&str] = &["gpt-5.6-luna", "gpt-5.4-mini"];
const CLAUDE_PRIMARY_MODELS: &[&str] = &[
    "claude-sonnet-4-6",
    "claude-sonnet-4-7",
    "claude-opus-4-6",
    "claude-opus-4-7",
    "claude-opus-4-8",
];
const CLAUDE_SMALL_MODELS: &[&str] = &["claude-haiku-4-5-20251001"];

#[derive(Debug, Clone, Copy)]
pub enum MappingProfile {
    All,
    Codex,
    ClaudeCode,
}

pub fn apply_to_create_input(input: &mut CreateProviderInput) {
    let current = input.model_mapping.as_deref();
    let merged = merge_mapping(
        current,
        &input.provider_type,
        input.default_model.as_str(),
        input.reasoning_model.as_deref(),
        MappingProfile::All,
    );
    input.model_mapping = mapping_to_json_option(&merged);
}

pub fn supplement_provider(
    conn: &Connection,
    provider_id: &str,
    profile: MappingProfile,
) -> Result<usize, AppError> {
    let provider = crate::storage::providers::get_by_id(conn, provider_id)?;
    let before = parse_mapping(provider.model_mapping.as_deref());
    let after = merge_for_provider(&provider, profile);
    if after == before {
        return Ok(0);
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mapping_json = serde_json::to_string(&after)
        .map_err(|e| AppError::internal(format!("Failed to serialize model mapping: {e}")))?;
    conn.execute(
        "UPDATE providers SET model_mapping=?1, updated_at=?2 WHERE id=?3",
        params![mapping_json, now, provider_id],
    )?;
    Ok(after.len().saturating_sub(before.len()))
}

pub fn supplement_active_provider(
    conn: &Connection,
    profile: MappingProfile,
) -> Result<usize, AppError> {
    let settings = crate::storage::gateway_settings::get(conn)?;
    let Some(provider_id) = settings.active_provider_id else {
        return Ok(0);
    };
    supplement_provider(conn, &provider_id, profile)
}

fn merge_for_provider(provider: &Provider, profile: MappingProfile) -> BTreeMap<String, String> {
    merge_mapping(
        provider.model_mapping.as_deref(),
        &provider.provider_type,
        &provider.default_model,
        provider.reasoning_model.as_deref(),
        profile,
    )
}

fn merge_mapping(
    current_json: Option<&str>,
    provider_type: &str,
    default_model: &str,
    reasoning_model: Option<&str>,
    profile: MappingProfile,
) -> BTreeMap<String, String> {
    let mut mapping = parse_mapping(current_json);
    repair_legacy_1m_recommendations(&mut mapping, provider_type, default_model, reasoning_model);
    for (client, upstream) in
        recommended_pairs(provider_type, default_model, reasoning_model, profile)
    {
        mapping.entry(client).or_insert(upstream);
    }
    mapping
}

fn recommended_pairs(
    provider_type: &str,
    default_model: &str,
    reasoning_model: Option<&str>,
    profile: MappingProfile,
) -> Vec<(String, String)> {
    let Some(profile_def) = mapping_profile_for_provider(provider_type) else {
        return Vec::new();
    };

    let mut pairs = Vec::new();
    if matches!(profile, MappingProfile::All | MappingProfile::Codex) {
        let codex_primary = target_model(
            profile_def.codex_primary_target,
            default_model,
            reasoning_model,
        );
        let codex_small = target_model(
            profile_def.codex_mini_target,
            default_model,
            reasoning_model,
        );
        for model in CODEX_MODELS {
            pairs.push(((*model).to_string(), codex_primary.clone()));
        }
        for model in CODEX_MINI_MODELS {
            pairs.push(((*model).to_string(), codex_small.clone()));
        }
    }

    if matches!(profile, MappingProfile::All | MappingProfile::ClaudeCode) {
        let claude_primary = target_model(
            profile_def.claude_primary_target,
            default_model,
            reasoning_model,
        );
        let claude_small = target_model(
            profile_def.claude_small_target,
            default_model,
            reasoning_model,
        );
        for model in CLAUDE_PRIMARY_MODELS {
            pairs.push(((*model).to_string(), claude_primary.clone()));
        }
        for model in CLAUDE_SMALL_MODELS {
            pairs.push(((*model).to_string(), claude_small.clone()));
        }
    }

    pairs
}

fn repair_legacy_1m_recommendations(
    mapping: &mut BTreeMap<String, String>,
    provider_type: &str,
    default_model: &str,
    reasoning_model: Option<&str>,
) {
    let Some(profile_def) = mapping_profile_for_provider(provider_type) else {
        return;
    };
    if !profile_def.repair_legacy_1m {
        return;
    }
    let primary = target_model(
        profile_def.claude_primary_target,
        default_model,
        reasoning_model,
    );
    if primary.is_empty() {
        return;
    }

    let legacy = format!("{primary}[1m]");
    for model in CLAUDE_PRIMARY_MODELS {
        if mapping.get(*model).map(|v| v.as_str()) == Some(legacy.as_str()) {
            mapping.insert((*model).to_string(), primary.clone());
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CatalogMappingProfile {
    codex_primary_target: &'static str,
    codex_mini_target: &'static str,
    claude_primary_target: &'static str,
    claude_small_target: &'static str,
    repair_legacy_1m: bool,
}

fn mapping_profile_for_provider(provider_type: &str) -> Option<CatalogMappingProfile> {
    let catalog_provider = catalog_provider_type(provider_type)?;
    catalog::RECOMMENDED_MAPPING_PROFILES
        .iter()
        .find(|(provider, _, _, _, _, _)| *provider == catalog_provider)
        .map(
            |(
                _,
                codex_primary_target,
                codex_mini_target,
                claude_primary_target,
                claude_small_target,
                repair_legacy_1m,
            )| CatalogMappingProfile {
                codex_primary_target,
                codex_mini_target,
                claude_primary_target,
                claude_small_target,
                repair_legacy_1m: *repair_legacy_1m,
            },
        )
}

fn catalog_provider_type(provider_type: &str) -> Option<&'static str> {
    let pt = provider_type.trim().to_lowercase();
    match pt.as_str() {
        p if p == "mimo" || p == "xiaomi" || p.contains("mimo") => Some("mimo"),
        "deepseek" => Some("deepseek"),
        _ => None,
    }
}

fn target_model(target: &str, default_model: &str, reasoning_model: Option<&str>) -> String {
    match target {
        "reasoning" => reasoning_model
            .filter(|model| !model.trim().is_empty())
            .unwrap_or(default_model)
            .to_string(),
        _ => default_model.to_string(),
    }
}

fn parse_mapping(json: Option<&str>) -> BTreeMap<String, String> {
    json.and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(s).ok())
        .unwrap_or_default()
}

fn mapping_to_json_option(mapping: &BTreeMap<String, String>) -> Option<String> {
    if mapping.is_empty() {
        None
    } else {
        serde_json::to_string(mapping).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_recommends_codex_and_claude_without_1m() {
        let mapping = merge_mapping(
            None,
            "mimo",
            "mimo-v2.5-pro",
            Some("mimo-v2.5-pro"),
            MappingProfile::All,
        );
        assert_eq!(mapping.get("gpt-5.5").unwrap(), "mimo-v2.5-pro");
        assert_eq!(mapping.get("gpt-5.4-mini").unwrap(), "mimo-v2.5-pro");
        assert_eq!(mapping.get("claude-sonnet-4-6").unwrap(), "mimo-v2.5-pro");
        assert_eq!(
            mapping.get("claude-haiku-4-5-20251001").unwrap(),
            "mimo-v2.5-pro"
        );
    }

    #[test]
    fn deepseek_routes_mini_to_flash_and_primary_to_reasoning_for_claude() {
        let mapping = merge_mapping(
            None,
            "deepseek",
            "deepseek-flash",
            Some("deepseek-v4-pro"),
            MappingProfile::All,
        );
        assert_eq!(mapping.get("gpt-5.5").unwrap(), "deepseek-v4-pro");
        assert_eq!(mapping.get("gpt-5.4-mini").unwrap(), "deepseek-flash");
        assert_eq!(mapping.get("claude-opus-4-6").unwrap(), "deepseek-v4-pro");
        assert_eq!(
            mapping.get("claude-haiku-4-5-20251001").unwrap(),
            "deepseek-flash"
        );
    }

    #[test]
    fn gpt_5_6_family_covered_by_codex_recommendation() {
        let mapping = merge_mapping(
            None,
            "deepseek",
            "deepseek-flash",
            Some("deepseek-v4-pro"),
            MappingProfile::Codex,
        );
        assert_eq!(mapping.get("gpt-5.6").unwrap(), "deepseek-v4-pro");
        assert_eq!(mapping.get("gpt-5.6-sol").unwrap(), "deepseek-v4-pro");
        assert_eq!(mapping.get("gpt-5.6-terra").unwrap(), "deepseek-v4-pro");
        assert_eq!(mapping.get("gpt-5.6-luna").unwrap(), "deepseek-flash");
    }

    #[test]
    fn existing_user_mapping_wins() {
        let existing = r#"{"gpt-5.5":"custom-model"}"#;
        let mapping = merge_mapping(
            existing.into(),
            "deepseek",
            "deepseek-flash",
            Some("deepseek-v4-pro"),
            MappingProfile::Codex,
        );
        assert_eq!(mapping.get("gpt-5.5").unwrap(), "custom-model");
        assert_eq!(mapping.get("gpt-5.4").unwrap(), "deepseek-v4-pro");
    }

    #[test]
    fn claude_code_recommendation_preserves_existing_qualifier_on_default_model() {
        let mapping = merge_mapping(
            None,
            "mimo",
            "mimo-v2.5-pro[128k]",
            None,
            MappingProfile::ClaudeCode,
        );
        assert_eq!(
            mapping.get("claude-sonnet-4-6").unwrap(),
            "mimo-v2.5-pro[128k]"
        );
    }

    #[test]
    fn legacy_auto_1m_mapping_is_repaired() {
        let mapping = merge_mapping(
            Some(r#"{"claude-sonnet-4-6":"mimo-v2.5-pro[1m]"}"#),
            "mimo",
            "mimo-v2.5-pro",
            Some("mimo-v2.5-pro"),
            MappingProfile::ClaudeCode,
        );
        assert_eq!(mapping.get("claude-sonnet-4-6").unwrap(), "mimo-v2.5-pro");
    }
}
