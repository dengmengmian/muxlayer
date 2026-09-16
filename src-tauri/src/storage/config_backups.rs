//! 配置导出/导入 —— 单文件 JSON 形式。
//!
//! 设计原则：
//! - **没有 scheduler、没有 rotation、没有自动 restore**。这是用户主动触发的
//!   一次性操作，价值是迁移机器和"配错了恢复一下"两个场景。
//! - **API key 默认不导出**。需要勾选"含密钥"才会写入。这避免用户把导出文件
//!   分享/截图/丢仓库时泄露密钥。导入时若 JSON 没带 key，保留为空字符串，
//!   用户在新机器上重新填一遍——这本来就是迁移时该做的事。
//! - **导入采用 replace 语义**：providers / route_profiles / route_profile_providers
//!   三张表清空后从 JSON 恢复。用户应该清楚"导入 = 覆盖"。
//! - **request_logs / provider_runtime_status / pricing 等运行时表不参与**，
//!   它们是积累出来的，不是用户手动配置的，没必要也不应该跨机器同步。
//! - **app_settings 不参与**：当前只存 `pet_memory`，是 transient 数据。
//!
//! Schema 版本字段 `version`：1 = 当前。未来如果 schema 变了，import 时按版本
//! 走对应迁移路径。
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::errors::AppError;
use crate::models::provider::Provider;
use crate::models::route_profile::RouteProfile;

pub const CONFIG_EXPORT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigExport {
    pub version: u32,
    pub exported_at: String,
    pub agentgate_version: String,
    pub include_secrets: bool,
    pub providers: Vec<Provider>,
    pub route_profiles: Vec<RouteProfileExport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteProfileExport {
    pub profile: RouteProfile,
    pub members: Vec<RouteProfileMemberExport>,
}

/// `route_profile_providers` 一行的原始可序列化形态。区别于 `RouteProfileProviderView`：
/// 后者带 runtime 字段（available / cooldown_until 等），导入时不应回填。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteProfileMemberExport {
    pub id: String,
    pub provider_id: String,
    pub priority: i64,
    pub enabled: bool,
    pub model_override: Option<String>,
    pub cooldown_seconds: i64,
    pub failover_on_status_codes: Option<String>,
    pub failover_on_error_keywords: Option<String>,
    pub routing_conditions: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ImportSummary {
    pub providers_imported: usize,
    pub route_profiles_imported: usize,
    pub members_imported: usize,
    /// API keys 是否随导入回填了：true = 导入文件携带了密钥并写入；
    /// false = 没带或用户选了不导入密钥。
    pub secrets_applied: bool,
}

pub fn export(conn: &Connection, include_secrets: bool) -> Result<ConfigExport, AppError> {
    let mut providers = crate::storage::providers::list_all(conn)?;
    if !include_secrets {
        for p in &mut providers {
            p.api_key = None;
        }
    }

    let profile_rows = crate::storage::route_profiles::list_all(conn)?;
    let mut route_profiles = Vec::with_capacity(profile_rows.len());
    for view in profile_rows {
        let profile = crate::storage::route_profiles::get_by_id(conn, &view.id)?;
        let members = load_route_members(conn, &view.id)?;
        route_profiles.push(RouteProfileExport { profile, members });
    }

    Ok(ConfigExport {
        version: CONFIG_EXPORT_VERSION,
        exported_at: chrono::Utc::now().to_rfc3339(),
        agentgate_version: env!("CARGO_PKG_VERSION").to_string(),
        include_secrets,
        providers,
        route_profiles,
    })
}

fn load_route_members(
    conn: &Connection,
    profile_id: &str,
) -> Result<Vec<RouteProfileMemberExport>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_id, priority, enabled, model_override, cooldown_seconds,
                failover_on_status_codes, failover_on_error_keywords, routing_conditions,
                created_at, updated_at
         FROM route_profile_providers
         WHERE route_profile_id = ?1
         ORDER BY priority ASC",
    )?;
    let rows = stmt.query_map([profile_id], |row| {
        Ok(RouteProfileMemberExport {
            id: row.get(0)?,
            provider_id: row.get(1)?,
            priority: row.get(2)?,
            enabled: row.get(3)?,
            model_override: row.get(4)?,
            cooldown_seconds: row.get(5)?,
            failover_on_status_codes: row.get(6)?,
            failover_on_error_keywords: row.get(7)?,
            routing_conditions: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub fn import(conn: &mut Connection, payload: &ConfigExport) -> Result<ImportSummary, AppError> {
    if payload.version != CONFIG_EXPORT_VERSION {
        return Err(AppError::new(
            crate::errors::codes::CONFIG_IMPORT_VERSION_MISMATCH,
            format!(
                "Unsupported export version {} (this build expects {CONFIG_EXPORT_VERSION})",
                payload.version
            ),
        ));
    }

    let tx = conn.transaction()?;

    // Truncate. `provider_runtime_status` is FK-loose runtime data — wipe it too
    // so we don't end up pointing at provider_ids that no longer exist.
    tx.execute("DELETE FROM route_profile_providers", [])?;
    tx.execute("DELETE FROM route_profiles", [])?;
    tx.execute("DELETE FROM provider_runtime_status", [])?;
    tx.execute(
        "UPDATE gateway_settings SET active_provider_id = NULL WHERE id = 1",
        [],
    )?;
    tx.execute("DELETE FROM providers", [])?;

    // Providers first (route_profile_providers depends on them via provider_id).
    let mut secrets_applied = false;
    for p in &payload.providers {
        let api_key = match &p.api_key {
            Some(k) if !k.is_empty() => {
                secrets_applied = true;
                Some(k.as_str())
            }
            _ => None,
        };
        tx.execute(
            "INSERT INTO providers (id, name, provider_type, base_url, api_key, default_model, reasoning_model, supported_models, model_mapping, extra_headers, anthropic_base_url, responses_base_url,
                                    protocol, timeout_seconds, status, supports_vision, auto_cache_control, supports_cache, model_capabilities, enabled, is_active, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                p.id, p.name, p.provider_type, p.base_url, api_key,
                p.default_model, p.reasoning_model, p.supported_models, p.model_mapping, p.extra_headers,
                p.anthropic_base_url, p.responses_base_url, p.protocol, p.timeout_seconds, p.status,
                p.supports_vision, p.auto_cache_control, p.supports_cache, p.model_capabilities,
                p.enabled, p.is_active, p.created_at, p.updated_at,
            ],
        )?;
    }

    // Route profiles.
    for rpe in &payload.route_profiles {
        let rp = &rpe.profile;
        tx.execute(
            "INSERT INTO route_profiles (id, name, client_type, input_protocol, mode, active_provider_id, enabled, is_default, created_at, updated_at)
             VALUES (?1, ?2, '', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                rp.id, rp.name, rp.input_protocol, rp.mode, rp.active_provider_id,
                rp.enabled, rp.is_default, rp.created_at, rp.updated_at,
            ],
        )?;
    }

    // Route profile members.
    let mut members_imported = 0usize;
    for rpe in &payload.route_profiles {
        for m in &rpe.members {
            tx.execute(
                "INSERT INTO route_profile_providers (id, route_profile_id, provider_id, priority, enabled, model_override, cooldown_seconds, failover_on_status_codes, failover_on_error_keywords, routing_conditions, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    m.id, rpe.profile.id, m.provider_id, m.priority, m.enabled,
                    m.model_override, m.cooldown_seconds, m.failover_on_status_codes,
                    m.failover_on_error_keywords, m.routing_conditions, m.created_at, m.updated_at,
                ],
            )?;
            members_imported += 1;
        }
    }

    // Sync gateway_settings.active_provider_id to whichever provider is_active.
    let now = chrono::Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE gateway_settings SET active_provider_id = (SELECT id FROM providers WHERE is_active = 1 LIMIT 1), updated_at = ?1 WHERE id = 1",
        params![&now],
    )?;

    tx.commit()?;

    Ok(ImportSummary {
        providers_imported: payload.providers.len(),
        route_profiles_imported: payload.route_profiles.len(),
        members_imported,
        secrets_applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::provider::CreateProviderInput;
    use crate::models::route_profile::{AddProviderToRouteInput, CreateRouteProfileInput};

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::storage::migrations::run_migrations(&conn).unwrap();
        // 清掉迁移种子（默认 2 个 provider + 3 个 route profile），让每个测试
        // 都从空白状态开始——配置导入语义就是 replace，断言期待空数据起步。
        conn.execute("DELETE FROM route_profile_providers", [])
            .unwrap();
        conn.execute("DELETE FROM route_profiles", []).unwrap();
        conn.execute("DELETE FROM providers", []).unwrap();
        conn.execute(
            "UPDATE gateway_settings SET active_provider_id = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        conn
    }

    fn create_provider(conn: &Connection, name: &str, api_key: &str) -> String {
        let p = crate::storage::providers::create(
            conn,
            CreateProviderInput {
                name: name.into(),
                provider_type: "deepseek".into(),
                base_url: "https://api.deepseek.com".into(),
                api_key: Some(api_key.into()),
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
            },
        )
        .unwrap();
        p.id
    }

    #[test]
    fn export_without_secrets_strips_api_keys() {
        let conn = setup_db();
        create_provider(&conn, "ds", "sk-real-secret");
        let dump = export(&conn, false).unwrap();
        assert!(!dump.include_secrets);
        assert_eq!(dump.providers.len(), 1);
        assert!(dump.providers[0].api_key.is_none());
    }

    #[test]
    fn export_with_secrets_keeps_api_keys() {
        let conn = setup_db();
        create_provider(&conn, "ds", "sk-real-secret");
        let dump = export(&conn, true).unwrap();
        assert!(dump.include_secrets);
        assert_eq!(dump.providers[0].api_key.as_deref(), Some("sk-real-secret"));
    }

    #[test]
    fn export_includes_route_profiles_with_members() {
        let conn = setup_db();
        let pid = create_provider(&conn, "ds", "sk-x");
        let rp = crate::storage::route_profiles::create(
            &conn,
            CreateRouteProfileInput {
                name: "Default".into(),
                input_protocol: "openai_responses".into(),
                mode: Some("failover".into()),
            },
        )
        .unwrap();
        crate::storage::route_profiles::add_provider(
            &conn,
            &rp.id,
            &pid,
            AddProviderToRouteInput {
                priority: Some(1),
                model_override: None,
                cooldown_seconds: Some(600),
                failover_on_status_codes: None,
                failover_on_error_keywords: None,
                routing_conditions: None,
            },
        )
        .unwrap();

        let dump = export(&conn, false).unwrap();
        assert_eq!(dump.route_profiles.len(), 1);
        assert_eq!(dump.route_profiles[0].members.len(), 1);
        assert_eq!(dump.route_profiles[0].members[0].provider_id, pid);
    }

    #[test]
    fn import_round_trip_restores_providers_and_routes() {
        let conn = setup_db();
        let pid = create_provider(&conn, "ds", "sk-x");
        let rp = crate::storage::route_profiles::create(
            &conn,
            CreateRouteProfileInput {
                name: "Default".into(),
                input_protocol: "openai_responses".into(),
                mode: Some("manual".into()),
            },
        )
        .unwrap();
        crate::storage::route_profiles::add_provider(
            &conn,
            &rp.id,
            &pid,
            AddProviderToRouteInput {
                priority: Some(1),
                model_override: None,
                cooldown_seconds: Some(600),
                failover_on_status_codes: None,
                failover_on_error_keywords: None,
                routing_conditions: None,
            },
        )
        .unwrap();
        let dump = export(&conn, true).unwrap();

        let mut fresh = setup_db();
        let summary = import(&mut fresh, &dump).unwrap();
        assert_eq!(summary.providers_imported, 1);
        assert_eq!(summary.route_profiles_imported, 1);
        assert_eq!(summary.members_imported, 1);
        assert!(summary.secrets_applied);

        let restored = crate::storage::providers::list_all(&fresh).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, pid);
        assert_eq!(restored[0].api_key.as_deref(), Some("sk-x"));

        let restored_rp = crate::storage::route_profiles::list_all(&fresh).unwrap();
        assert_eq!(restored_rp.len(), 1);
        assert_eq!(restored_rp[0].providers_count, 1);
    }

    #[test]
    fn import_without_secrets_leaves_api_key_blank() {
        let conn = setup_db();
        create_provider(&conn, "ds", "sk-real");
        let dump = export(&conn, false).unwrap();

        let mut fresh = setup_db();
        let summary = import(&mut fresh, &dump).unwrap();
        assert!(!summary.secrets_applied);
        let restored = crate::storage::providers::list_all(&fresh).unwrap();
        assert!(restored[0].api_key.is_none());
    }

    #[test]
    fn import_replaces_existing_providers() {
        let conn = setup_db();
        create_provider(&conn, "old-ds", "sk-old");
        let dump = export(&conn, true).unwrap();

        let mut target = setup_db();
        create_provider(&target, "polluted", "sk-leftover");
        assert_eq!(
            crate::storage::providers::list_all(&target).unwrap().len(),
            1
        );

        import(&mut target, &dump).unwrap();
        let after = crate::storage::providers::list_all(&target).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].name, "old-ds"); // replaced, not merged
    }

    #[test]
    fn import_rejects_unknown_version() {
        let mut conn = setup_db();
        let bad = ConfigExport {
            version: 9999,
            exported_at: "x".into(),
            agentgate_version: "x".into(),
            include_secrets: false,
            providers: vec![],
            route_profiles: vec![],
        };
        let err = import(&mut conn, &bad).unwrap_err();
        assert_eq!(err.code, "CONFIG_IMPORT_VERSION_MISMATCH");
    }
}
