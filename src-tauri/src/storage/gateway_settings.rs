use rusqlite::{params, Connection};

use crate::errors::AppError;
use crate::models::gateway::{GatewaySettings, UpdateGatewaySettingsInput};

const MAX_REQUEST_BODY_LIMIT_MB: i64 = 128;
const MAX_WAKE_COOLDOWN_SECONDS: i64 = 86_400;
pub const MIN_LOG_RETENTION_DAYS: i64 = 1;
pub const MAX_LOG_RETENTION_DAYS: i64 = 3650;

pub fn get(conn: &Connection) -> Result<GatewaySettings, AppError> {
    conn.query_row(
        "SELECT id, host, port, active_provider_id, input_protocol, output_protocol,
                auto_start, log_retention_days, body_filter_global, thinking_rectifier_global,
                error_mapper_global, updated_at, health_probe_enabled,
                codex_compact_enabled, codex_compact_summary_max_tokens,
                request_body_limit_mb, cost_alert_enabled, cost_alert_threshold,
                wake_enabled, wake_request_control, wake_cooldown_seconds,
                wake_keep_display_awake,
                cost_budget_enabled, cost_budget_threshold, cost_budget_strategy,
                auto_compact_enabled, auto_compact_usage_percent,
                outbound_proxy_enabled, outbound_proxy_url
         FROM gateway_settings WHERE id = 1",
        [],
        |row| {
            Ok(GatewaySettings {
                id: row.get(0)?,
                host: row.get(1)?,
                port: row.get(2)?,
                active_provider_id: row.get(3)?,
                input_protocol: row.get(4)?,
                output_protocol: row.get(5)?,
                auto_start: row.get(6)?,
                log_retention_days: row.get(7)?,
                body_filter_global: row.get(8)?,
                thinking_rectifier_global: row.get(9)?,
                error_mapper_global: row.get(10)?,
                updated_at: row.get(11)?,
                health_probe_enabled: row.get(12)?,
                codex_compact_enabled: row.get(13)?,
                codex_compact_summary_max_tokens: row.get(14)?,
                request_body_limit_mb: row.get(15)?,
                cost_alert_enabled: row.get(16)?,
                cost_alert_threshold: row.get(17)?,
                wake_enabled: row.get(18)?,
                wake_request_control: row.get(19)?,
                wake_cooldown_seconds: row.get(20)?,
                wake_keep_display_awake: row.get(21)?,
                cost_budget_enabled: row.get(22)?,
                cost_budget_threshold: row.get(23)?,
                cost_budget_strategy: row
                    .get::<_, Option<String>>(24)?
                    .unwrap_or_else(|| "notify_only".into()),
                auto_compact_enabled: row.get(25)?,
                auto_compact_usage_percent: row.get(26)?,
                outbound_proxy_enabled: row.get::<_, Option<i64>>(27)?.unwrap_or(0) != 0,
                outbound_proxy_url: row.get(28)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            AppError::internal("Gateway settings not initialized")
        }
        other => AppError::database(other),
    })
}

pub fn update(
    conn: &Connection,
    input: UpdateGatewaySettingsInput,
) -> Result<GatewaySettings, AppError> {
    let existing = get(conn)?;
    let now = chrono::Utc::now().to_rfc3339();

    let host = input.host.unwrap_or(existing.host);
    let port = input.port.unwrap_or(existing.port);
    let active_provider_id = match input.active_provider_id {
        Some(id) => Some(id),
        None => existing.active_provider_id,
    };
    let input_protocol = input.input_protocol.unwrap_or(existing.input_protocol);
    let output_protocol = input.output_protocol.unwrap_or(existing.output_protocol);
    let auto_start = input.auto_start.unwrap_or(existing.auto_start);
    // 0 / 负数会让每小时的保留期清理删光全部日志,必须拒绝而不是静默改值。
    // 只校验本次传入的值:库里历史脏值不应阻塞用户修改其它字段(清理侧另有兜底)。
    if let Some(days) = input.log_retention_days {
        if !(MIN_LOG_RETENTION_DAYS..=MAX_LOG_RETENTION_DAYS).contains(&days) {
            return Err(AppError::validation(format!(
                "log_retention_days must be between {MIN_LOG_RETENTION_DAYS} and {MAX_LOG_RETENTION_DAYS}, got {days}"
            )));
        }
    }
    let log_retention_days = input
        .log_retention_days
        .unwrap_or(existing.log_retention_days);
    let body_filter_global = input
        .body_filter_global
        .unwrap_or(existing.body_filter_global);
    let thinking_rectifier_global = input
        .thinking_rectifier_global
        .unwrap_or(existing.thinking_rectifier_global);
    let error_mapper_global = input
        .error_mapper_global
        .unwrap_or(existing.error_mapper_global);
    let health_probe_enabled = input
        .health_probe_enabled
        .unwrap_or(existing.health_probe_enabled);
    let codex_compact_enabled = input
        .codex_compact_enabled
        .unwrap_or(existing.codex_compact_enabled);
    let codex_compact_summary_max_tokens = input
        .codex_compact_summary_max_tokens
        .unwrap_or(existing.codex_compact_summary_max_tokens);
    let request_body_limit_mb = input
        .request_body_limit_mb
        .unwrap_or(existing.request_body_limit_mb)
        .clamp(1, MAX_REQUEST_BODY_LIMIT_MB);
    let cost_alert_enabled = input
        .cost_alert_enabled
        .unwrap_or(existing.cost_alert_enabled);
    let cost_alert_threshold = match input.cost_alert_threshold {
        Some(v) => Some(v),
        None => existing.cost_alert_threshold,
    };
    let cost_budget_enabled = input
        .cost_budget_enabled
        .unwrap_or(existing.cost_budget_enabled);
    let cost_budget_threshold = match input.cost_budget_threshold {
        Some(v) => Some(v),
        None => existing.cost_budget_threshold,
    };
    let cost_budget_strategy = input
        .cost_budget_strategy
        .map(|s| crate::gateway::budget::normalize_strategy(&s))
        .unwrap_or(existing.cost_budget_strategy);
    let auto_compact_enabled = input
        .auto_compact_enabled
        .unwrap_or(existing.auto_compact_enabled);
    let auto_compact_usage_percent = input
        .auto_compact_usage_percent
        .unwrap_or(existing.auto_compact_usage_percent)
        .clamp(1, 95);
    let wake_enabled = input.wake_enabled.unwrap_or(existing.wake_enabled);
    let wake_request_control = input
        .wake_request_control
        .unwrap_or(existing.wake_request_control);
    let wake_cooldown_seconds = input
        .wake_cooldown_seconds
        .unwrap_or(existing.wake_cooldown_seconds)
        .clamp(0, MAX_WAKE_COOLDOWN_SECONDS);
    let wake_keep_display_awake = input
        .wake_keep_display_awake
        .unwrap_or(existing.wake_keep_display_awake);
    let outbound_proxy_enabled = input
        .outbound_proxy_enabled
        .unwrap_or(existing.outbound_proxy_enabled);
    let outbound_proxy_url = match input.outbound_proxy_url {
        Some(raw) => crate::gateway::http_client::parse_proxy_url(&raw)?,
        None => existing.outbound_proxy_url,
    };
    conn.execute(
        "UPDATE gateway_settings SET host=?1, port=?2, active_provider_id=?3,
                input_protocol=?4, output_protocol=?5, auto_start=?6,
                log_retention_days=?7, body_filter_global=?8,
                thinking_rectifier_global=?9, error_mapper_global=?10,
                health_probe_enabled=?11, codex_compact_enabled=?12,
                codex_compact_summary_max_tokens=?13,
                request_body_limit_mb=?14,
                cost_alert_enabled=?15, cost_alert_threshold=?16,
                wake_enabled=?17, wake_request_control=?18,
                wake_cooldown_seconds=?19, wake_keep_display_awake=?20,
                cost_budget_enabled=?21, cost_budget_threshold=?22,
                cost_budget_strategy=?23,
                auto_compact_enabled=?24, auto_compact_usage_percent=?25,
                outbound_proxy_enabled=?26, outbound_proxy_url=?27,
                updated_at=?28
         WHERE id = 1",
        params![
            &host,
            port,
            &active_provider_id,
            &input_protocol,
            &output_protocol,
            auto_start,
            log_retention_days,
            body_filter_global,
            thinking_rectifier_global,
            error_mapper_global,
            health_probe_enabled,
            codex_compact_enabled,
            codex_compact_summary_max_tokens,
            request_body_limit_mb,
            cost_alert_enabled,
            cost_alert_threshold,
            wake_enabled,
            wake_request_control,
            wake_cooldown_seconds,
            wake_keep_display_awake,
            cost_budget_enabled,
            cost_budget_threshold,
            &cost_budget_strategy,
            auto_compact_enabled,
            auto_compact_usage_percent,
            outbound_proxy_enabled,
            &outbound_proxy_url,
            &now,
        ],
    )?;

    get(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::gateway::UpdateGatewaySettingsInput;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::storage::migrations::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn test_get_gateway_settings() {
        let conn = setup_db();
        let settings = get(&conn).unwrap();
        assert_eq!(settings.id, 1);
        assert_eq!(settings.host, "127.0.0.1");
        assert_eq!(settings.port, 9090);
        assert_eq!(settings.request_body_limit_mb, 32);
        assert!(!settings.cost_budget_enabled);
        assert_eq!(settings.cost_budget_strategy, "notify_only");
        assert!(!settings.outbound_proxy_enabled);
        assert!(settings.outbound_proxy_url.is_none());
    }

    #[test]
    fn outbound_proxy_roundtrip_and_rejects_socks() {
        let conn = setup_db();
        let updated = update(
            &conn,
            UpdateGatewaySettingsInput {
                outbound_proxy_enabled: Some(true),
                outbound_proxy_url: Some("http://127.0.0.1:7890".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(updated.outbound_proxy_enabled);
        assert_eq!(
            updated.outbound_proxy_url.as_deref(),
            Some("http://127.0.0.1:7890")
        );

        let err = update(
            &conn,
            UpdateGatewaySettingsInput {
                outbound_proxy_url: Some("socks5://127.0.0.1:1080".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
    }

    #[test]
    fn budget_fields_roundtrip() {
        let conn = setup_db();
        let updated = update(
            &conn,
            UpdateGatewaySettingsInput {
                cost_budget_enabled: Some(true),
                cost_budget_threshold: Some(12.5),
                cost_budget_strategy: Some("block".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(updated.cost_budget_enabled);
        assert_eq!(updated.cost_budget_threshold, Some(12.5));
        assert_eq!(updated.cost_budget_strategy, "block");
    }

    #[test]
    fn migration_v9_adds_budget_to_existing_v8_db() {
        // Fresh DB after migrations lands on CURRENT schema with budget + compact columns.
        let conn = Connection::open_in_memory().unwrap();
        crate::storage::migrations::run_migrations(&conn).unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 12);
        let ok = conn
            .prepare(
                "SELECT cost_budget_enabled, cost_budget_strategy, auto_compact_enabled, auto_compact_usage_percent FROM gateway_settings LIMIT 0",
            )
            .is_ok();
        assert!(ok);
    }

    #[test]
    fn test_update_gateway_settings() {
        let conn = setup_db();
        let updated = update(
            &conn,
            UpdateGatewaySettingsInput {
                host: Some("0.0.0.0".to_string()),
                port: Some(8080),
                input_protocol: Some("openai_chat_completions".to_string()),
                auto_start: Some(true),
                log_retention_days: Some(7),
                request_body_limit_mb: Some(32),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.host, "0.0.0.0");
        assert_eq!(updated.port, 8080);
        assert_eq!(updated.input_protocol, "openai_chat_completions");
        assert!(updated.auto_start);
        assert_eq!(updated.log_retention_days, 7);
        assert_eq!(updated.request_body_limit_mb, 32);
    }

    #[test]
    fn test_partial_update_preserves_existing() {
        let conn = setup_db();
        let original = get(&conn).unwrap();
        let updated = update(
            &conn,
            UpdateGatewaySettingsInput {
                host: Some("0.0.0.0".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.host, "0.0.0.0");
        assert_eq!(updated.port, original.port);
        assert_eq!(updated.input_protocol, original.input_protocol);
        assert_eq!(
            updated.request_body_limit_mb,
            original.request_body_limit_mb
        );
    }

    #[test]
    fn request_body_limit_is_clamped_before_save() {
        let conn = setup_db();
        let updated = update(
            &conn,
            UpdateGatewaySettingsInput {
                request_body_limit_mb: Some(4096),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.request_body_limit_mb, 128);
    }

    #[test]
    fn wake_settings_are_persisted() {
        let conn = setup_db();
        let updated = update(
            &conn,
            UpdateGatewaySettingsInput {
                wake_enabled: Some(false),
                wake_request_control: Some(true),
                wake_cooldown_seconds: Some(30),
                wake_keep_display_awake: Some(true),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(!updated.wake_enabled);
        assert!(updated.wake_request_control);
        assert_eq!(updated.wake_cooldown_seconds, 30);
        assert!(updated.wake_keep_display_awake);
    }

    #[test]
    fn wake_cooldown_is_clamped_before_save() {
        let conn = setup_db();
        let updated = update(
            &conn,
            UpdateGatewaySettingsInput {
                wake_cooldown_seconds: Some(i64::MAX),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(updated.wake_cooldown_seconds, 86_400);
    }

    #[test]
    fn log_retention_days_out_of_range_is_rejected() {
        // 0 / 负数会让每小时清理删光全部日志;超上限无意义。拒绝而不是静默改值。
        let conn = setup_db();
        for bad in [0, -3, 3651] {
            let err = update(
                &conn,
                UpdateGatewaySettingsInput {
                    log_retention_days: Some(bad),
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(err.code, "VALIDATION_ERROR", "value {bad}");
        }
        assert_eq!(get(&conn).unwrap().log_retention_days, 14, "拒绝后不落库");
        for ok in [1, 3650] {
            let s = update(
                &conn,
                UpdateGatewaySettingsInput {
                    log_retention_days: Some(ok),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(s.log_retention_days, ok);
        }
    }
}
