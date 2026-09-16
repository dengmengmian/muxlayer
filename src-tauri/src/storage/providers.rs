use rusqlite::{params, Connection};

use crate::errors::AppError;
use crate::models::provider::{CreateProviderInput, Provider, UpdateProviderInput};

const PROVIDER_COLUMNS: &str = "id, name, provider_type, base_url, api_key, default_model, reasoning_model, supported_models, model_mapping, extra_headers, anthropic_base_url, responses_base_url, \
                                 protocol, timeout_seconds, status, supports_vision, auto_cache_control, supports_cache, model_capabilities, provider_quirks, body_filter_enabled, thinking_rectifier_enabled, error_mapper_enabled, model_degradation_chain, model_context_windows, enabled, is_active, created_at, updated_at";

fn map_provider_row(row: &rusqlite::Row) -> rusqlite::Result<Provider> {
    Ok(Provider {
        id: row.get(0)?,
        name: row.get(1)?,
        provider_type: row.get(2)?,
        base_url: row.get(3)?,
        api_key: row.get(4)?,
        default_model: row.get(5)?,
        reasoning_model: row.get(6)?,
        supported_models: row.get(7)?,
        model_mapping: row.get(8)?,
        extra_headers: row.get(9)?,
        anthropic_base_url: row.get(10)?,
        responses_base_url: row.get(11)?,
        protocol: row.get(12)?,
        timeout_seconds: row.get(13)?,
        status: row.get(14)?,
        supports_vision: row.get(15)?,
        auto_cache_control: row.get(16)?,
        supports_cache: row.get(17)?,
        model_capabilities: row.get(18)?,
        provider_quirks: row.get(19)?,
        body_filter_enabled: row.get(20)?,
        thinking_rectifier_enabled: row.get(21)?,
        error_mapper_enabled: row.get(22)?,
        model_degradation_chain: row.get(23)?,
        model_context_windows: row.get(24)?,
        enabled: row.get(25)?,
        is_active: row.get(26)?,
        created_at: row.get(27)?,
        updated_at: row.get(28)?,
    })
}

pub fn list_all(conn: &Connection) -> Result<Vec<Provider>, AppError> {
    let sql =
        format!("SELECT {PROVIDER_COLUMNS} FROM providers ORDER BY is_active DESC, created_at ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_provider_row)?;
    let mut providers = Vec::new();
    for row in rows {
        providers.push(row?);
    }
    Ok(providers)
}

pub fn get_by_id(conn: &Connection, id: &str) -> Result<Provider, AppError> {
    let sql = format!("SELECT {PROVIDER_COLUMNS} FROM providers WHERE id = ?1");
    conn.query_row(&sql, [id], map_provider_row)
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::not_found("Provider", id),
            other => AppError::database(other),
        })
}

pub fn create(conn: &Connection, mut input: CreateProviderInput) -> Result<Provider, AppError> {
    crate::storage::provider_endpoints::apply_to_create_input(&mut input);
    crate::storage::recommended_mappings::apply_to_create_input(&mut input);

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let timeout = input.timeout_seconds.unwrap_or(120);
    let enabled = input.enabled.unwrap_or(true);

    conn.execute(
        "INSERT INTO providers (id, name, provider_type, base_url, api_key, default_model, reasoning_model,
                                supported_models, model_mapping, extra_headers, anthropic_base_url, responses_base_url, protocol, timeout_seconds, status, auto_cache_control, model_capabilities,
                                provider_quirks, body_filter_enabled, thinking_rectifier_enabled, error_mapper_enabled, model_degradation_chain, model_context_windows,
                                enabled, is_active, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'not_tested', ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, 0, ?24, ?24)",
        params![
            &id,
            &input.name,
            &input.provider_type,
            &input.base_url,
            &input.api_key,
            &input.default_model,
            &input.reasoning_model,
            &input.supported_models,
            &input.model_mapping,
            &input.extra_headers,
            &input.anthropic_base_url,
            &input.responses_base_url,
            &input.protocol,
            timeout,
            &input.auto_cache_control,
            &input.model_capabilities,
            &input.provider_quirks,
            &input.body_filter_enabled,
            &input.thinking_rectifier_enabled,
            &input.error_mapper_enabled,
            &input.model_degradation_chain,
            &input.model_context_windows,
            enabled,
            &now,
        ],
    )?;

    get_by_id(conn, &id)
}

pub fn update(
    conn: &Connection,
    id: &str,
    mut input: UpdateProviderInput,
) -> Result<Provider, AppError> {
    let existing = get_by_id(conn, id)?;
    let now = chrono::Utc::now().to_rfc3339();

    let effective_provider_type = input
        .provider_type
        .as_deref()
        .unwrap_or(&existing.provider_type)
        .to_string();
    let effective_api_key = input
        .api_key
        .as_deref()
        .or(existing.api_key.as_deref())
        .map(str::to_string);
    crate::storage::provider_endpoints::apply_to_update_input(
        &effective_provider_type,
        effective_api_key.as_deref(),
        &existing.base_url,
        existing.anthropic_base_url.as_deref(),
        &existing.protocol,
        &mut input,
    );

    let name = input.name.unwrap_or(existing.name);
    let provider_type = input.provider_type.unwrap_or(existing.provider_type);
    let base_url = input.base_url.unwrap_or(existing.base_url);
    let api_key = match input.api_key {
        Some(k) => Some(k),
        None => existing.api_key,
    };
    let default_model = input.default_model.unwrap_or(existing.default_model);
    let reasoning_model = input.reasoning_model.or(existing.reasoning_model);
    let supported_models = input.supported_models.or(existing.supported_models);
    let model_mapping = input.model_mapping.or(existing.model_mapping);
    let extra_headers = input.extra_headers.or(existing.extra_headers);
    let anthropic_base_url = input.anthropic_base_url.or(existing.anthropic_base_url);
    let responses_base_url = input.responses_base_url.or(existing.responses_base_url);
    let protocol = input.protocol.unwrap_or(existing.protocol);
    let timeout_seconds = input.timeout_seconds.unwrap_or(existing.timeout_seconds);
    let auto_cache_control = input.auto_cache_control.or(existing.auto_cache_control);
    let model_capabilities = input.model_capabilities.or(existing.model_capabilities);
    let provider_quirks = input.provider_quirks.or(existing.provider_quirks);
    let body_filter_enabled = input.body_filter_enabled.or(existing.body_filter_enabled);
    let thinking_rectifier_enabled = input
        .thinking_rectifier_enabled
        .or(existing.thinking_rectifier_enabled);
    let error_mapper_enabled = input.error_mapper_enabled.or(existing.error_mapper_enabled);
    let model_degradation_chain = input
        .model_degradation_chain
        .or(existing.model_degradation_chain);
    let model_context_windows = input
        .model_context_windows
        .or(existing.model_context_windows);
    let enabled = input.enabled.unwrap_or(existing.enabled);

    conn.execute(
        "UPDATE providers SET name=?1, provider_type=?2, base_url=?3, api_key=?4, default_model=?5,
                reasoning_model=?6, supported_models=?7, model_mapping=?8, extra_headers=?9, anthropic_base_url=?10, responses_base_url=?11, protocol=?12, timeout_seconds=?13, auto_cache_control=?14, model_capabilities=?15,
                provider_quirks=?16, body_filter_enabled=?17, thinking_rectifier_enabled=?18, error_mapper_enabled=?19, model_degradation_chain=?20, model_context_windows=?21,
                enabled=?22, updated_at=?23
         WHERE id=?24",
        params![
            &name,
            &provider_type,
            &base_url,
            &api_key,
            &default_model,
            &reasoning_model,
            &supported_models,
            &model_mapping,
            &extra_headers,
            &anthropic_base_url,
            &responses_base_url,
            &protocol,
            timeout_seconds,
            auto_cache_control,
            &model_capabilities,
            &provider_quirks,
            body_filter_enabled,
            thinking_rectifier_enabled,
            error_mapper_enabled,
            &model_degradation_chain,
            &model_context_windows,
            enabled,
            &now,
            id,
        ],
    )?;

    get_by_id(conn, id)
}

pub fn delete(conn: &Connection, id: &str) -> Result<bool, AppError> {
    let provider = get_by_id(conn, id)?;
    let was_active = provider.is_active;

    conn.execute("DELETE FROM providers WHERE id = ?1", [id])?;

    // Clean up route_profile_providers references to this provider
    conn.execute(
        "DELETE FROM route_profile_providers WHERE provider_id = ?1",
        [id],
    )?;

    if was_active {
        // Set next enabled provider as active
        let next_id: Option<String> = conn
            .query_row(
                "SELECT id FROM providers WHERE enabled = 1 ORDER BY created_at ASC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();

        if let Some(next) = &next_id {
            conn.execute(
                "UPDATE providers SET is_active = 1, updated_at = ?1 WHERE id = ?2",
                params![chrono::Utc::now().to_rfc3339(), next],
            )?;
        }

        // Update gateway_settings
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE gateway_settings SET active_provider_id = ?1, updated_at = ?2 WHERE id = 1",
            params![next_id, &now],
        )?;
    }

    Ok(true)
}

pub fn set_active(conn: &Connection, id: &str) -> Result<Provider, AppError> {
    let _provider = get_by_id(conn, id)?;
    let now = chrono::Utc::now().to_rfc3339();

    // Clear all active
    conn.execute(
        "UPDATE providers SET is_active = 0, updated_at = ?1 WHERE is_active = 1",
        [&now],
    )?;

    // Set new active
    conn.execute(
        "UPDATE providers SET is_active = 1, updated_at = ?1 WHERE id = ?2",
        params![&now, id],
    )?;

    // Sync gateway_settings
    conn.execute(
        "UPDATE gateway_settings SET active_provider_id = ?1, updated_at = ?2 WHERE id = 1",
        params![id, &now],
    )?;

    ensure_provider_in_default_route_profiles(conn, id, &now)?;

    // Sync all default route profiles
    conn.execute(
        "UPDATE route_profiles SET active_provider_id = ?1, updated_at = ?2 WHERE is_default = 1",
        params![id, &now],
    )?;

    get_by_id(conn, id)
}

fn ensure_provider_in_default_route_profiles(
    conn: &Connection,
    provider_id: &str,
    now: &str,
) -> Result<(), AppError> {
    let mut stmt = conn.prepare("SELECT id FROM route_profiles WHERE is_default = 1")?;
    let profile_ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for profile_id in profile_ids {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM route_profile_providers WHERE route_profile_id = ?1 AND provider_id = ?2",
            params![&profile_id, provider_id],
            |row| row.get(0),
        )?;
        if exists > 0 {
            continue;
        }

        let priority: i64 = conn.query_row(
            "SELECT COALESCE(MAX(priority), 0) + 1 FROM route_profile_providers WHERE route_profile_id = ?1",
            [&profile_id],
            |row| row.get(0),
        )?;
        let default_codes = "[402,429,500,502,503,504]";

        conn.execute(
            "INSERT INTO route_profile_providers (
                id, route_profile_id, provider_id, priority, enabled,
                cooldown_seconds, failover_on_status_codes,
                created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, 1, 600, ?5, ?6, ?6)",
            params![
                uuid::Uuid::new_v4().to_string(),
                &profile_id,
                provider_id,
                priority,
                default_codes,
                now,
            ],
        )?;
    }

    Ok(())
}

pub fn update_status(conn: &Connection, id: &str, status: &str) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE providers SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status, &now, id],
    )?;
    Ok(())
}

pub fn update_supports_vision(
    conn: &Connection,
    id: &str,
    supports_vision: bool,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE providers SET supports_vision = ?1, updated_at = ?2 WHERE id = ?3",
        params![supports_vision, &now, id],
    )?;
    Ok(())
}

pub fn update_supports_cache(
    conn: &Connection,
    id: &str,
    supports_cache: bool,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE providers SET supports_cache = ?1, updated_at = ?2 WHERE id = ?3",
        params![supports_cache, &now, id],
    )?;
    Ok(())
}

pub fn update_model_capabilities(
    conn: &Connection,
    id: &str,
    matrix_json: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE providers SET model_capabilities = ?1, updated_at = ?2 WHERE id = ?3",
        params![matrix_json, &now, id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::provider::{CreateProviderInput, UpdateProviderInput};

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::storage::migrations::run_migrations(&conn).unwrap();
        conn
    }

    fn create_test_provider(conn: &Connection, name: &str) -> Provider {
        create(
            conn,
            CreateProviderInput {
                name: name.to_string(),
                provider_type: "openai".to_string(),
                base_url: "https://api.openai.com".to_string(),
                api_key: Some("sk-test".to_string()),
                default_model: "gpt-4".to_string(),
                reasoning_model: None,
                supported_models: None,
                model_mapping: None,
                extra_headers: None,
                anthropic_base_url: None,
                responses_base_url: None,
                protocol: r#"["openai_chat_completions"]"#.to_string(),
                timeout_seconds: Some(120),
                auto_cache_control: None,
                model_capabilities: None,
                model_context_windows: None,
                provider_quirks: None,
                body_filter_enabled: None,
                thinking_rectifier_enabled: None,
                error_mapper_enabled: None,
                model_degradation_chain: None,
                enabled: Some(true),
            },
        )
        .unwrap()
    }

    #[test]
    fn test_list_all_returns_single_default() {
        let conn = setup_db();
        let providers = list_all(&conn).unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].provider_type, "deepseek");
    }

    #[test]
    fn test_create_and_get_provider() {
        let conn = setup_db();
        let p = create_test_provider(&conn, "TestProvider");
        assert_eq!(p.name, "TestProvider");
        assert_eq!(p.provider_type, "openai");

        let fetched = get_by_id(&conn, &p.id).unwrap();
        assert_eq!(fetched.id, p.id);
        assert_eq!(fetched.name, "TestProvider");
    }

    #[test]
    fn test_set_active_adds_provider_to_default_route_candidates() {
        let conn = setup_db();
        let provider = create_test_provider(&conn, "QuickstartProvider");

        set_active(&conn, &provider.id).unwrap();

        let profiles = crate::storage::route_profiles::list_all(&conn).unwrap();
        let default_profiles: Vec<_> = profiles
            .into_iter()
            .filter(|profile| profile.is_default)
            .collect();
        assert!(!default_profiles.is_empty());

        for profile in default_profiles {
            let providers =
                crate::storage::route_profiles::list_providers(&conn, &profile.id).unwrap();
            assert!(
                providers
                    .iter()
                    .any(|candidate| candidate.provider_id == provider.id),
                "active provider should be available in default route profile {}",
                profile.name
            );
        }
    }

    #[test]
    fn test_create_deepseek_fills_v4_defaults_before_mapping() {
        let conn = setup_db();
        let p = create(
            &conn,
            CreateProviderInput {
                name: "DeepSeek CLI".to_string(),
                provider_type: "deepseek".to_string(),
                base_url: "https://api.deepseek.com".to_string(),
                api_key: Some("sk-test".to_string()),
                default_model: "deepseek-flash".to_string(),
                reasoning_model: None,
                supported_models: None,
                model_mapping: None,
                extra_headers: None,
                anthropic_base_url: None,
                responses_base_url: None,
                protocol: r#"["openai_chat_completions"]"#.to_string(),
                timeout_seconds: Some(120),
                auto_cache_control: None,
                model_capabilities: None,
                model_context_windows: None,
                provider_quirks: None,
                body_filter_enabled: None,
                thinking_rectifier_enabled: None,
                error_mapper_enabled: None,
                model_degradation_chain: None,
                enabled: Some(true),
            },
        )
        .unwrap();
        assert_eq!(p.reasoning_model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(
            p.supported_models.as_deref(),
            Some(r#"["deepseek-flash","deepseek-v4-pro"]"#)
        );
        assert_eq!(
            p.anthropic_base_url.as_deref(),
            Some("https://api.deepseek.com/anthropic")
        );
        let mapping: serde_json::Value =
            serde_json::from_str(p.model_mapping.as_deref().unwrap()).unwrap();
        assert_eq!(mapping["gpt-5.5"], "deepseek-v4-pro");
        assert_eq!(mapping["gpt-5.4-mini"], "deepseek-flash");
    }

    #[test]
    fn test_get_by_id_not_found() {
        let conn = setup_db();
        let err = get_by_id(&conn, "nonexistent").unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    #[test]
    fn test_update_provider() {
        let conn = setup_db();
        let p = create_test_provider(&conn, "Original");
        let updated = update(
            &conn,
            &p.id,
            UpdateProviderInput {
                name: Some("Updated".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.name, "Updated");
    }

    #[test]
    fn test_delete_provider() {
        let conn = setup_db();
        let p = create_test_provider(&conn, "ToDelete");
        let result = delete(&conn, &p.id).unwrap();
        assert!(result);
        assert!(get_by_id(&conn, &p.id).is_err());
    }

    #[test]
    fn test_set_active_provider() {
        let conn = setup_db();
        let p1 = create_test_provider(&conn, "P1");
        let p2 = create_test_provider(&conn, "P2");
        set_active(&conn, &p1.id).unwrap();
        let active1 = get_by_id(&conn, &p1.id).unwrap();
        assert!(active1.is_active);

        set_active(&conn, &p2.id).unwrap();
        let active2 = get_by_id(&conn, &p2.id).unwrap();
        assert!(active2.is_active);
        let inactive1 = get_by_id(&conn, &p1.id).unwrap();
        assert!(!inactive1.is_active);
    }

    #[test]
    fn test_update_status() {
        let conn = setup_db();
        let p = create_test_provider(&conn, "StatusTest");
        update_status(&conn, &p.id, "ok").unwrap();
        let updated = get_by_id(&conn, &p.id).unwrap();
        assert_eq!(updated.status, "ok");
    }

    #[test]
    fn test_update_supports_vision() {
        let conn = setup_db();
        let p = create_test_provider(&conn, "VisionTest");
        update_supports_vision(&conn, &p.id, true).unwrap();
        let updated = get_by_id(&conn, &p.id).unwrap();
        assert_eq!(updated.supports_vision, Some(true));
    }

    #[test]
    fn test_update_supports_cache() {
        let conn = setup_db();
        let p = create_test_provider(&conn, "CacheTest");
        update_supports_cache(&conn, &p.id, true).unwrap();
        let updated = get_by_id(&conn, &p.id).unwrap();
        assert_eq!(updated.supports_cache, Some(true));
    }

    #[test]
    fn test_update_model_capabilities() {
        let conn = setup_db();
        let p = create_test_provider(&conn, "CapTest");
        let matrix = r#"{"gpt-4":["text","vision"]}"#;
        update_model_capabilities(&conn, &p.id, matrix).unwrap();
        let updated = get_by_id(&conn, &p.id).unwrap();
        assert_eq!(updated.model_capabilities, Some(matrix.to_string()));
    }
}
