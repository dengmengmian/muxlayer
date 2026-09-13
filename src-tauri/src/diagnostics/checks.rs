use crate::diagnostics::report::*;
use crate::security::local_token;
use crate::storage;

// ── Health Check ──────────────────────────────────────────────

pub fn health_check(db: &crate::storage::db::DbPool) -> CheckReport {
    let mut checks = Vec::new();

    // Token file
    let tp = local_token::token_path();
    if tp.exists() {
        checks.push(CheckItem::ok(
            "token_file",
            "Token file",
            "Token file exists",
        ));
        match local_token::read_token() {
            Ok(t) if t.starts_with("ag_local_") => {
                checks.push(CheckItem::ok(
                    "token_format",
                    "Token format",
                    "Valid ag_local_ format",
                ));
            }
            Ok(_) => {
                checks.push(CheckItem::warning(
                    "token_format",
                    "Token format",
                    "Token doesn't start with ag_local_",
                ));
            }
            Err(_) => {
                checks.push(CheckItem::failed(
                    "token_read",
                    "Token read",
                    "Cannot read token file",
                ));
            }
        }
    } else {
        checks.push(
            CheckItem::warning("token_file", "Token file", "Token file not found")
                .with_suggestion("Restart MuxLayer to auto-generate"),
        );
    }

    // DB accessible
    match db.get() {
        Ok(conn) => {
            checks.push(CheckItem::ok("db_lock", "Database", "Database accessible"));
            // Tables
            let tables = [
                "providers",
                "gateway_settings",
                "request_logs",
                "route_profiles",
                "config_backups",
                "client_apply_history",
            ];
            for table in &tables {
                let exists: bool = conn
                    .prepare(&format!("SELECT 1 FROM {table} LIMIT 0"))
                    .is_ok();
                if exists {
                    checks.push(CheckItem::ok(
                        &format!("table_{table}"),
                        &format!("Table: {table}"),
                        "Exists",
                    ));
                } else {
                    checks.push(CheckItem::failed(
                        &format!("table_{table}"),
                        &format!("Table: {table}"),
                        "Missing",
                    ));
                }
            }
        }
        Err(_) => {
            checks.push(CheckItem::failed(
                "db_lock",
                "Database",
                "Cannot acquire database lock",
            ));
        }
    }

    CheckReport::new("Health Check", checks)
}

// ── Database Check ────────────────────────────────────────────

pub fn database_check(db: &crate::storage::db::DbPool) -> CheckReport {
    let mut checks = Vec::new();

    let Ok(conn) = db.get() else {
        checks.push(CheckItem::failed("db_lock", "Database lock", "Cannot lock"));
        return CheckReport::new("Database Check", checks);
    };

    // Row counts
    for (table, label) in &[
        ("providers", "Providers"),
        ("route_profiles", "Route profiles"),
        ("request_logs", "Request logs"),
        ("config_backups", "Config backups"),
        ("client_apply_history", "Client apply history"),
    ] {
        match conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
            r.get::<_, i64>(0)
        }) {
            Ok(n) => checks.push(CheckItem::ok(
                &format!("count_{table}"),
                label,
                &format!("{n} rows"),
            )),
            Err(_) => checks.push(CheckItem::failed(
                &format!("count_{table}"),
                label,
                "Cannot query",
            )),
        }
    }

    // Log size check
    if let Ok(n) = conn.query_row("SELECT COUNT(*) FROM request_logs", [], |r| {
        r.get::<_, i64>(0)
    }) {
        if n > 10000 {
            checks.push(
                CheckItem::warning(
                    "logs_size",
                    "Log size",
                    &format!("{n} logs - consider cleanup"),
                )
                .with_suggestion("Reduce log retention in Settings"),
            );
        }
    }

    // Orphan checks
    let orphan_rpp: i64 = conn.query_row(
        "SELECT COUNT(*) FROM route_profile_providers rpp WHERE NOT EXISTS (SELECT 1 FROM providers WHERE id = rpp.provider_id)",
        [], |r| r.get(0),
    ).unwrap_or(0);
    if orphan_rpp > 0 {
        checks.push(CheckItem::warning(
            "orphan_rpp",
            "Orphan route providers",
            &format!("{orphan_rpp} route provider entries reference missing providers"),
        ));
    } else {
        checks.push(CheckItem::ok(
            "orphan_rpp",
            "Route provider integrity",
            "No orphan records",
        ));
    }

    CheckReport::new("Database Check", checks)
}

// ── Gateway Auth Check ────────────────────────────────────────

pub fn gateway_auth_check(db: &crate::storage::db::DbPool) -> CheckReport {
    let mut checks = Vec::new();

    // Token exists
    let token = match local_token::read_token() {
        Ok(t) => {
            checks.push(CheckItem::ok(
                "token_exists",
                "Token exists",
                "Local token readable",
            ));
            t
        }
        Err(_) => {
            checks.push(
                CheckItem::failed("token_exists", "Token exists", "Cannot read token")
                    .with_suggestion("Restart MuxLayer to auto-generate"),
            );
            return CheckReport::new("Gateway Auth Check", checks);
        }
    };

    // Format
    if token.starts_with("ag_local_") && token.len() > 20 {
        checks.push(CheckItem::ok(
            "token_format",
            "Token format",
            "Valid format",
        ));
    } else {
        checks.push(CheckItem::warning(
            "token_format",
            "Token format",
            "Unexpected format",
        ));
    }

    // Check logs for token leakage
    if let Ok(conn) = db.get() {
        let leaked: i64 = conn.query_row(
            "SELECT COUNT(*) FROM request_logs WHERE raw_request LIKE ?1 OR converted_request LIKE ?1 OR error_message LIKE ?1",
            [&format!("%{token}%")], |r| r.get(0),
        ).unwrap_or(0);
        if leaked > 0 {
            checks.push(
                CheckItem::failed(
                    "token_leakage",
                    "Token leakage",
                    &format!("Token found in {leaked} log entries!"),
                )
                .with_suggestion("Clear logs and check redaction logic"),
            );
        } else {
            checks.push(CheckItem::ok(
                "token_leakage",
                "Token leakage",
                "No token found in logs",
            ));
        }
    }

    CheckReport::new("Gateway Auth Check", checks)
}

// ── Provider Check ────────────────────────────────────────────

pub fn provider_check(db: &crate::storage::db::DbPool) -> CheckReport {
    let mut checks = Vec::new();

    let Ok(conn) = db.get() else {
        checks.push(CheckItem::failed("db", "Database", "Cannot lock"));
        return CheckReport::new("Provider Check", checks);
    };

    let providers = storage::providers::list_all(&conn).unwrap_or_default();
    let enabled = providers.iter().filter(|p| p.enabled).count();
    checks.push(if enabled > 0 {
        CheckItem::ok(
            "enabled_count",
            "Enabled providers",
            &format!("{enabled} enabled"),
        )
    } else {
        CheckItem::failed("enabled_count", "Enabled providers", "No enabled providers")
            .with_suggestion("Enable at least one provider in Providers page")
    });

    // Active provider
    let active = providers.iter().find(|p| p.is_active);
    match active {
        Some(p) => {
            checks.push(CheckItem::ok("active", "Active provider", &p.name));
            if p.api_key.as_ref().is_none_or(|k| k.is_empty()) {
                checks.push(
                    CheckItem::failed("active_key", "Active provider API key", "API key not set")
                        .with_suggestion("Set API key in Providers page"),
                );
            } else {
                checks.push(CheckItem::ok(
                    "active_key",
                    "Active provider API key",
                    "Set",
                ));
            }
        }
        None => {
            checks.push(CheckItem::warning(
                "active",
                "Active provider",
                "No active provider set",
            ));
        }
    }

    // Cooldown
    let cooldown_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM provider_runtime_status WHERE cooldown_until IS NOT NULL AND cooldown_until > ?1",
        [chrono::Utc::now().to_rfc3339()], |r| r.get(0),
    ).unwrap_or(0);
    if cooldown_count > 0 {
        checks.push(CheckItem::warning(
            "cooldown",
            "Providers in cooldown",
            &format!("{cooldown_count} in cooldown"),
        ));
    } else {
        checks.push(CheckItem::ok(
            "cooldown",
            "Provider cooldowns",
            "None in cooldown",
        ));
    }

    CheckReport::new("Provider Check", checks)
}

// ── Codex Config Check ────────────────────────────────────────

pub fn codex_config_check(db: &crate::storage::db::DbPool) -> CheckReport {
    let mut checks = Vec::new();

    let status = crate::tools::codex::detect();

    // Skip check entirely if Codex is not installed or not configured for AgentGate
    if !status.exists || !status.has_agentgate {
        checks.push(CheckItem::ok(
            "not_configured",
            "Codex",
            "Not configured for MuxLayer (skipped)",
        ));
        return CheckReport::new("Codex Config Check", checks);
    }

    checks.push(CheckItem::ok("config_exists", "Config file", "Exists"));
    checks.push(CheckItem::ok(
        "agentgate_provider",
        "MuxLayer provider",
        "Configured",
    ));

    // Codex 1.1.0+ 改成"劫持 OpenAI provider + requires_openai_auth = true"：
    //   model_provider = "OpenAI"        ← 让 IDE 插件 / 配额查询都把它当官方 OpenAI
    //   [model_providers.OpenAI]         ← base_url 指向本地网关
    //   requires_openai_auth = true
    // 旧设计是 model_provider = "agentgate"，两种都算正确，都不该 warn。
    match status.current_provider.as_deref() {
        Some("OpenAI") => checks.push(CheckItem::ok(
            "model_provider",
            "model_provider",
            "Set to OpenAI (hijack mode — IDE plugins compatible)",
        )),
        Some("agentgate") => checks.push(CheckItem::ok(
            "model_provider",
            "model_provider",
            "Set to agentgate (legacy mode)",
        )),
        Some(other) => checks.push(
            CheckItem::warning(
                "model_provider",
                "model_provider",
                &format!("Unexpected value: {other}"),
            )
            .with_suggestion("Re-apply Codex config from Tools page"),
        ),
        None => checks.push(
            CheckItem::warning("model_provider", "model_provider", "Not set")
                .with_suggestion("Re-apply Codex config from Tools page"),
        ),
    }

    // auth.json 三种正常状态：
    //   1) 不存在——Codex 还没登过 ChatGPT，纯 key 模式跑也 OK，AgentGate token 走 config.toml
    //   2) 存在且未污染——ChatGPT OAuth tokens 保留（新设计），或 ag_local_ 单独占位（legacy），都 OK
    //   3) 污染态——auth.json 里同时有 ag_local_ 和 ChatGPT tokens，旧版本写坏了
    // 真"AgentGate 已激活"的标志是 `is_agentgate_active`（config.toml 里有 ag_local_ bearer 或 legacy
    // model_provider = "agentgate"），跟 auth.json 是两码事。
    if !status.auth_json_exists {
        // auth.json 不存在不算问题——Codex 没登 ChatGPT 也能跑
        checks.push(CheckItem::ok("auth_json", "auth.json", "Not present (Codex not signed in to ChatGPT — fine, MuxLayer token lives in config.toml)"));
    } else if status.openai_key_polluted {
        checks.push(
            CheckItem::warning(
                "auth_json",
                "auth.json",
                "Polluted: contains both MuxLayer token and ChatGPT tokens (legacy bug)",
            )
            .with_suggestion(
                "Re-apply Codex config from Tools page — MuxLayer will restore from saved backup",
            ),
        );
    } else {
        checks.push(CheckItem::ok(
            "auth_json",
            "auth.json",
            "Clean (ChatGPT login state preserved, MuxLayer token in config.toml)",
        ));
    }

    // Base URL check
    if let Ok(conn) = db.get() {
        if let Ok(settings) = storage::gateway_settings::get(&conn) {
            let expected_url = format!("http://{}:{}/v1", settings.host, settings.port);
            let content =
                std::fs::read_to_string(crate::tools::codex::config_path().unwrap_or_default())
                    .unwrap_or_default();
            if content.contains(&expected_url) {
                checks.push(CheckItem::ok(
                    "base_url",
                    "Base URL",
                    &format!("Matches gateway: {expected_url}"),
                ));
            } else {
                checks.push(
                    CheckItem::warning(
                        "base_url",
                        "Base URL",
                        "May not match current gateway settings",
                    )
                    .with_suggestion("Re-apply config if gateway host/port changed"),
                );
            }
        }
    }

    CheckReport::new("Codex Config Check", checks)
}

// ── Claude Code Config Check ──────────────────────────────────

pub fn claude_code_config_check(_db: &crate::storage::db::DbPool) -> CheckReport {
    let mut checks = Vec::new();

    let status = crate::tools::claude_code::detect_env();

    // Skip check entirely if AgentGate is not configured for Claude Code
    if !status.has_agentgate {
        checks.push(CheckItem::ok(
            "not_configured",
            "Claude Code",
            "Not configured for MuxLayer (skipped)",
        ));
        return CheckReport::new("Claude Code Config Check", checks);
    }

    // AgentGate is configured — verify it's correct
    checks.push(CheckItem::ok(
        "agentgate_token",
        "MuxLayer token",
        "Found in settings",
    ));

    // Verify token matches
    if let Ok(current_token) = local_token::read_token() {
        let content =
            std::fs::read_to_string(crate::tools::claude_code::settings_path().unwrap_or_default())
                .unwrap_or_default();
        if content.contains(&current_token) {
            checks.push(CheckItem::ok(
                "token_match",
                "Token match",
                "Settings token matches current",
            ));
        } else {
            checks.push(
                CheckItem::failed(
                    "token_match",
                    "Token match",
                    "Token in settings doesn't match current token",
                )
                .with_suggestion(
                    "Re-apply Claude Code config from Tools page (token was regenerated)",
                ),
            );
        }
    }

    // Base URL
    if let Some(ref url) = status.active_base_url {
        if url.contains("127.0.0.1") || url.contains("localhost") {
            checks.push(CheckItem::ok("base_url", "Base URL", url));
        } else {
            checks.push(CheckItem::warning(
                "base_url",
                "Base URL",
                &format!("Points to {url}, not MuxLayer"),
            ));
        }
    }

    // Conflicts
    if status.conflicts.is_empty() {
        checks.push(CheckItem::ok("conflicts", "Env conflicts", "No conflicts"));
    } else {
        for c in &status.conflicts {
            checks.push(CheckItem::warning("conflict", "Env conflict", c));
        }
    }

    CheckReport::new("Claude Code Config Check", checks)
}

// ── Route Profile Check ───────────────────────────────────────

pub fn route_profile_check(db: &crate::storage::db::DbPool) -> CheckReport {
    let mut checks = Vec::new();

    let Ok(conn) = db.get() else {
        checks.push(CheckItem::failed("db", "Database", "Cannot lock"));
        return CheckReport::new("Route Profile Check", checks);
    };

    let profiles = storage::route_profiles::list_all(&conn).unwrap_or_default();
    if profiles.is_empty() {
        checks.push(CheckItem::warning(
            "profiles",
            "Route profiles",
            "No profiles exist",
        ));
        return CheckReport::new("Route Profile Check", checks);
    }

    checks.push(CheckItem::ok(
        "profiles",
        "Route profiles",
        &format!("{} profiles", profiles.len()),
    ));

    let default = profiles.iter().find(|p| p.is_default);
    match default {
        Some(d) => {
            checks.push(CheckItem::ok("default", "Default profile", &d.name));
            if d.providers_count == 0 {
                checks.push(
                    CheckItem::failed(
                        "default_providers",
                        "Default providers",
                        "No providers in default profile",
                    )
                    .with_suggestion("Add providers in Routes page"),
                );
            } else {
                checks.push(CheckItem::ok(
                    "default_providers",
                    "Default providers",
                    &format!("{} providers", d.providers_count),
                ));
            }
            if d.mode == "failover" && d.providers_count < 2 {
                checks.push(CheckItem::warning(
                    "failover_count",
                    "Failover mode",
                    "Less than 2 providers for failover",
                ));
            }
        }
        None => {
            checks.push(CheckItem::warning(
                "default",
                "Default profile",
                "No default profile set",
            ));
        }
    }

    CheckReport::new("Route Profile Check", checks)
}

// ── Full Self Test ────────────────────────────────────────────

pub fn full_self_test(db: &crate::storage::db::DbPool) -> FullSelfTestReport {
    let reports = vec![
        health_check(db),
        database_check(db),
        gateway_auth_check(db),
        provider_check(db),
        route_profile_check(db),
        codex_config_check(db),
        claude_code_config_check(db),
    ];

    let has_failed = reports.iter().any(|r| r.status == "failed");
    let has_warning = reports.iter().any(|r| r.status == "warning");
    let overall = if has_failed {
        "failed"
    } else if has_warning {
        "warning"
    } else {
        "ok"
    };

    let ok_count = reports.iter().filter(|r| r.status == "ok").count();
    let summary = format!("{}/{} checks passed", ok_count, reports.len());

    FullSelfTestReport {
        overall_status: overall.to_string(),
        reports,
        summary,
        created_at: chrono::Utc::now().to_rfc3339(),
    }
}

// ── Diagnostic Bundle Export ──────────────────────────────────

/// 写一个诊断包文件;失败直接返回错误,成功才记入 `files`(不再「写失败也报成功」)。
fn write_bundle_file(
    dir: &std::path::Path,
    name: &str,
    content: &str,
    files: &mut Vec<String>,
) -> Result<(), crate::errors::AppError> {
    std::fs::write(dir.join(name), content).map_err(|e| {
        crate::errors::AppError::new(
            crate::errors::codes::DIAGNOSTIC_EXPORT_FAILED,
            format!("Cannot write {name}: {e}"),
        )
    })?;
    files.push(name.to_string());
    Ok(())
}

fn to_pretty_json<T: serde::Serialize>(
    value: &T,
    name: &str,
) -> Result<String, crate::errors::AppError> {
    serde_json::to_string_pretty(value).map_err(|e| {
        crate::errors::AppError::new(
            crate::errors::codes::DIAGNOSTIC_EXPORT_FAILED,
            format!("Cannot serialize {name}: {e}"),
        )
    })
}

pub fn export_bundle(
    db: &crate::storage::db::DbPool,
    include_logs: bool,
    max_logs: usize,
) -> Result<ExportResult, crate::errors::AppError> {
    use crate::security::redaction;
    use std::fs;

    let export_dir = local_token::token_dir().join("diagnostics");
    fs::create_dir_all(&export_dir).map_err(|e| {
        crate::errors::AppError::new(
            crate::errors::codes::DIAGNOSTIC_EXPORT_FAILED,
            format!("Cannot create dir: {e}"),
        )
    })?;

    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let bundle_dir = export_dir.join(format!("agentgate-diag-{ts}"));
    fs::create_dir_all(&bundle_dir).map_err(|e| {
        crate::errors::AppError::new(
            crate::errors::codes::DIAGNOSTIC_EXPORT_FAILED,
            format!("Cannot create bundle dir: {e}"),
        )
    })?;

    let mut files = Vec::new();
    let warnings = Vec::new();

    // 1. Self test report
    let report = full_self_test(db);
    write_bundle_file(
        &bundle_dir,
        "self_test_report.json",
        &to_pretty_json(&report, "self test report")?,
        &mut files,
    )?;

    // 2. Gateway status
    if let Ok(conn) = db.get() {
        if let Ok(settings) = storage::gateway_settings::get(&conn) {
            write_bundle_file(
                &bundle_dir,
                "gateway_settings.json",
                &to_pretty_json(&settings, "gateway settings")?,
                &mut files,
            )?;
        }

        // 3. Providers (redacted)
        let providers = storage::providers::list_all(&conn).unwrap_or_default();
        // 所有当前存储的 key / 自定义 header 值,用于日志文本的精确脱敏。
        let known_secrets = redaction::provider_secrets(&providers);
        let redacted: Vec<serde_json::Value> = providers
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id, "name": p.name, "provider_type": p.provider_type,
                    "base_url": p.base_url, "default_model": p.default_model,
                    "protocol": p.protocol, "status": p.status,
                    "enabled": p.enabled, "is_active": p.is_active,
                    "api_key": p.api_key.as_ref().map(|k| redaction::redact_value(k)),
                })
            })
            .collect();
        write_bundle_file(
            &bundle_dir,
            "providers.redacted.json",
            &to_pretty_json(&redacted, "providers")?,
            &mut files,
        )?;

        // 4. Route profiles
        let profiles = storage::route_profiles::list_all(&conn).unwrap_or_default();
        write_bundle_file(
            &bundle_dir,
            "route_profiles.json",
            &to_pretty_json(&profiles, "route profiles")?,
            &mut files,
        )?;

        // 5. Recent logs (redacted)
        if include_logs {
            let filter = crate::models::request_log::RequestLogFilter {
                client: None,
                provider: None,
                model: None,
                route_profile_id: None,
                status: None,
                error_type: None,
                keyword: None,
                source: None,
                session_id: None,
                limit: Some(max_logs as i64),
                offset: None,
            };
            let logs = storage::request_logs::list(&conn, filter).unwrap_or_default();
            let redacted_logs: Vec<serde_json::Value> = logs
                .iter()
                .map(|l| {
                    serde_json::json!({
                        "request_id": l.request_id, "timestamp": l.timestamp,
                        "client": l.client, "provider": l.provider, "model": l.model,
                        "route": l.route, "status_code": l.status_code, "latency_ms": l.latency_ms,
                        "error_message": l.error_message.as_ref()
                            .map(|m| redaction::redact_text_with_secrets(m, &known_secrets)),
                    })
                })
                .collect();
            write_bundle_file(
                &bundle_dir,
                "recent_logs.redacted.json",
                &to_pretty_json(&redacted_logs, "recent logs")?,
                &mut files,
            )?;
        }
    }

    // 6. Config summaries
    let codex = crate::tools::codex::detect();
    let claude = crate::tools::claude_code::detect_env();
    let config_summary = serde_json::json!({
        "codex": { "exists": codex.exists, "has_agentgate": codex.has_agentgate, "auth_mode": codex.auth_mode },
        "claude_code": { "exists": claude.settings_exists, "has_agentgate": claude.has_agentgate, "conflicts": claude.conflicts.len() },
    });
    write_bundle_file(
        &bundle_dir,
        "config_summaries.json",
        &to_pretty_json(&config_summary, "config summaries")?,
        &mut files,
    )?;

    // 7. README
    let readme = "MuxLayer Diagnostic Bundle\n\
        ==========================\n\n\
        This bundle contains diagnostic information for troubleshooting.\n\
        All API keys and tokens have been redacted.\n\n\
        DO NOT share this bundle publicly if it contains request payloads.\n";
    write_bundle_file(&bundle_dir, "README.txt", readme, &mut files)?;

    // 8. Manifest(不计入 files 列表本身)
    let manifest = serde_json::json!({
        "app_version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "files": files,
        "redaction_enabled": true,
    });
    let mut manifest_files = Vec::new();
    write_bundle_file(
        &bundle_dir,
        "manifest.json",
        &to_pretty_json(&manifest, "manifest")?,
        &mut manifest_files,
    )?;

    let bundle_path = bundle_dir.to_string_lossy().to_string();

    Ok(ExportResult {
        success: true,
        path: bundle_path,
        files,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::provider::UpdateProviderInput;
    use crate::models::route_profile::{AddProviderToRouteInput, CreateRouteProfileInput};
    use crate::storage::db::DbPool;
    use crate::storage::route_profiles;
    use crate::test_utils::{cleanup, setup_temp_home, FS_LOCK};
    use r2d2::Pool;
    use r2d2_sqlite::SqliteConnectionManager;
    use rusqlite::params;
    use std::time::Duration;

    fn setup_db_pool() -> (DbPool, std::path::PathBuf) {
        let temp = std::env::temp_dir().join(format!(
            "agentgate_checks_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let db_path = temp.join("test.db");
        let manager = SqliteConnectionManager::file(&db_path);
        let pool = Pool::builder().max_size(2).build(manager).unwrap();
        let conn = pool.get().unwrap();
        crate::storage::migrations::run_migrations(&conn).unwrap();
        (pool, temp)
    }

    fn broken_db_pool() -> (DbPool, std::path::PathBuf) {
        let temp = std::env::temp_dir().join(format!(
            "agentgate_checks_broken_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let db_path = temp.join("test.db");
        let manager = SqliteConnectionManager::file(&db_path);
        let pool = Pool::builder()
            .max_size(1)
            .connection_timeout(Duration::from_millis(1))
            .build(manager)
            .unwrap();
        (pool, temp)
    }

    fn write_token(token: &str) {
        let dir = crate::security::local_token::token_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(crate::security::local_token::token_path(), token).unwrap();
    }

    fn find_check<'a>(report: &'a CheckReport, id: &'a str) -> Option<&'a CheckItem> {
        report.checks.iter().find(|c| c.id == id)
    }

    fn assert_check_status(report: &CheckReport, id: &str, expected: &str) {
        let item = find_check(report, id).unwrap_or_else(|| panic!("missing check {id}"));
        assert_eq!(
            item.status, expected,
            "check {id} expected {expected}, got {}",
            item.status
        );
    }

    // ── Health Check ──────────────────────────────────────────────

    #[test]
    fn health_check_warns_when_token_missing() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        let _ = std::fs::remove_file(crate::security::local_token::token_path());
        let (pool, db_temp) = setup_db_pool();

        let report = health_check(&pool);
        assert_check_status(&report, "token_file", "warning");
        assert_check_status(&report, "db_lock", "ok");
        assert_check_status(&report, "table_providers", "ok");

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn health_check_ok_when_token_valid() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        write_token("ag_local_1234567890123456789012345678901234567890");
        let (pool, db_temp) = setup_db_pool();

        let report = health_check(&pool);
        assert_check_status(&report, "token_file", "ok");
        assert_check_status(&report, "token_format", "ok");
        assert_check_status(&report, "db_lock", "ok");

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn health_check_warns_when_token_format_invalid() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        write_token("not_an_agentgate_token");
        let (pool, db_temp) = setup_db_pool();

        let report = health_check(&pool);
        assert_check_status(&report, "token_file", "ok");
        assert_check_status(&report, "token_format", "warning");

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn health_check_fails_when_db_lock_unavailable() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        write_token("ag_local_1234567890123456789012345678901234567890");
        let (pool, db_temp) = broken_db_pool();
        // Create and hold the only connection so subsequent get() calls time out.
        let conn = pool.get().unwrap();
        crate::storage::migrations::run_migrations(&conn).unwrap();

        let report = health_check(&pool);
        assert_check_status(&report, "db_lock", "failed");

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    // ── Database Check ────────────────────────────────────────────

    #[test]
    fn database_check_reports_row_counts() {
        let (pool, db_temp) = setup_db_pool();
        let report = database_check(&pool);
        assert_check_status(&report, "count_providers", "ok");
        assert_check_status(&report, "count_route_profiles", "ok");
        assert_check_status(&report, "count_request_logs", "ok");
        assert_check_status(&report, "count_config_backups", "ok");
        assert_check_status(&report, "count_client_apply_history", "ok");
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn database_check_warns_when_logs_exceed_threshold() {
        let (pool, db_temp) = setup_db_pool();
        let mut conn = pool.get().unwrap();
        let tx = conn.transaction().unwrap();
        for i in 0..10001 {
            tx.execute(
                "INSERT INTO request_logs (id, request_id, timestamp) VALUES (?1, ?2, ?3)",
                params![
                    format!("id-{i}"),
                    format!("req-{i}"),
                    "2024-01-01T00:00:00Z"
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let report = database_check(&pool);
        assert_check_status(&report, "logs_size", "warning");
        assert!(find_check(&report, "logs_size")
            .unwrap()
            .message
            .contains("10001"));
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn database_check_warns_on_orphan_route_providers() {
        let (pool, db_temp) = setup_db_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO route_profile_providers (id, route_profile_id, provider_id, priority, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params!["rpp-orphan", "rp-missing", "provider-missing", 1, "2024-01-01T00:00:00Z"],
        )
        .unwrap();

        let report = database_check(&pool);
        assert_check_status(&report, "orphan_rpp", "warning");
        assert!(find_check(&report, "orphan_rpp")
            .unwrap()
            .message
            .contains("1"));
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    // ── Gateway Auth Check ────────────────────────────────────────

    #[test]
    fn gateway_auth_check_passes_with_valid_token() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        write_token("ag_local_1234567890123456789012345678901234567890");
        let (pool, db_temp) = setup_db_pool();

        let report = gateway_auth_check(&pool);
        assert_check_status(&report, "token_exists", "ok");
        assert_check_status(&report, "token_format", "ok");
        assert_check_status(&report, "token_leakage", "ok");

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn gateway_auth_check_warns_on_invalid_token_format() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        write_token("ag_local_x");
        let (pool, db_temp) = setup_db_pool();

        let report = gateway_auth_check(&pool);
        assert_check_status(&report, "token_exists", "ok");
        assert_check_status(&report, "token_format", "warning");

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn gateway_auth_check_fails_when_token_leaked_in_logs() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        let token = "ag_local_1234567890123456789012345678901234567890";
        write_token(token);
        let (pool, db_temp) = setup_db_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO request_logs (id, request_id, timestamp, raw_request) VALUES (?1, ?2, ?3, ?4)",
            params!["log-1", "req-1", "2024-01-01T00:00:00Z", format!("bearer {token}")],
        )
        .unwrap();

        let report = gateway_auth_check(&pool);
        assert_check_status(&report, "token_leakage", "failed");
        assert!(find_check(&report, "token_leakage")
            .unwrap()
            .message
            .contains("1"));

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    // ── Provider Check ────────────────────────────────────────────

    #[test]
    fn provider_check_reports_default_providers() {
        let (pool, db_temp) = setup_db_pool();
        let report = provider_check(&pool);
        assert_check_status(&report, "enabled_count", "ok");
        assert_check_status(&report, "active", "ok");
        // Default active provider has no API key.
        assert_check_status(&report, "active_key", "failed");
        assert_check_status(&report, "cooldown", "ok");
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn provider_check_fails_when_no_providers_enabled() {
        let (pool, db_temp) = setup_db_pool();
        let conn = pool.get().unwrap();
        for p in crate::storage::providers::list_all(&conn).unwrap() {
            crate::storage::providers::update(
                &conn,
                &p.id,
                UpdateProviderInput {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        }

        let report = provider_check(&pool);
        assert_check_status(&report, "enabled_count", "failed");
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn provider_check_ok_when_active_provider_has_key() {
        let (pool, db_temp) = setup_db_pool();
        let conn = pool.get().unwrap();
        let active = crate::storage::providers::list_all(&conn)
            .unwrap()
            .into_iter()
            .find(|p| p.is_active)
            .expect("no active provider");
        crate::storage::providers::update(
            &conn,
            &active.id,
            UpdateProviderInput {
                api_key: Some("sk-test".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let report = provider_check(&pool);
        assert_check_status(&report, "active_key", "ok");
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    // ── Route Profile Check ───────────────────────────────────────

    #[test]
    fn route_profile_check_reports_defaults() {
        let (pool, db_temp) = setup_db_pool();
        let report = route_profile_check(&pool);
        assert_check_status(&report, "profiles", "ok");
        assert_check_status(&report, "default", "ok");
        assert_check_status(&report, "default_providers", "ok");
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn route_profile_check_warns_when_no_profiles() {
        let (pool, db_temp) = setup_db_pool();
        let conn = pool.get().unwrap();
        conn.execute("DELETE FROM route_profiles", []).unwrap();

        let report = route_profile_check(&pool);
        assert_check_status(&report, "profiles", "warning");
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn route_profile_check_warns_on_failover_with_insufficient_providers() {
        let (pool, db_temp) = setup_db_pool();
        let conn = pool.get().unwrap();
        conn.execute("DELETE FROM route_profiles", []).unwrap();

        let profile = route_profiles::create(
            &conn,
            CreateRouteProfileInput {
                name: "Failover".to_string(),
                input_protocol: "openai_chat_completions".to_string(),
                mode: Some("failover".to_string()),
            },
        )
        .unwrap();
        route_profiles::set_default(&conn, &profile.id).unwrap();

        let provider = crate::storage::providers::list_all(&conn)
            .unwrap()
            .pop()
            .unwrap();
        route_profiles::add_provider(
            &conn,
            &profile.id,
            &provider.id,
            AddProviderToRouteInput {
                priority: Some(1),
                model_override: None,
                cooldown_seconds: None,
                failover_on_status_codes: None,
                failover_on_error_keywords: None,
                routing_conditions: None,
            },
        )
        .unwrap();

        let report = route_profile_check(&pool);
        assert_check_status(&report, "profiles", "ok");
        assert_check_status(&report, "failover_count", "warning");
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    // ── Codex / Claude Code Config Checks ─────────────────────────

    #[test]
    fn codex_config_check_skips_when_not_configured() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        let (pool, db_temp) = setup_db_pool();

        let report = codex_config_check(&pool);
        assert_check_status(&report, "not_configured", "ok");
        assert_eq!(report.status, "ok");

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn claude_code_config_check_skips_when_not_configured() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        let (pool, db_temp) = setup_db_pool();

        let report = claude_code_config_check(&pool);
        assert_check_status(&report, "not_configured", "ok");
        assert_eq!(report.status, "ok");

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    // ── Full Self Test ────────────────────────────────────────────

    #[test]
    fn full_self_test_returns_reports() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        let (pool, db_temp) = setup_db_pool();

        let report = full_self_test(&pool);
        assert!(!report.reports.is_empty());
        assert!(!report.summary.is_empty());
        assert!(!report.created_at.is_empty());

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    // ── Diagnostic Bundle Export ──────────────────────────────────

    #[test]
    fn export_bundle_creates_files() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        let (pool, db_temp) = setup_db_pool();

        let result = export_bundle(&pool, false, 10).unwrap();
        assert!(result.success);
        assert!(!result.path.is_empty());
        assert!(result.files.contains(&"self_test_report.json".to_string()));
        assert!(result.files.contains(&"README.txt".to_string()));
        assert!(std::path::Path::new(&result.path).exists());

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[test]
    fn export_bundle_manifest_uses_real_app_version() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        let (pool, db_temp) = setup_db_pool();

        let result = export_bundle(&pool, false, 10).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(std::path::Path::new(&result.path).join("manifest.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["app_version"], env!("CARGO_PKG_VERSION"));
        // 列出的每个文件都必须真实存在(不能写失败了还报成功)
        for f in &result.files {
            assert!(std::path::Path::new(&result.path).join(f).is_file(), "{f}");
        }

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }

    #[cfg(unix)]
    #[test]
    fn write_bundle_file_propagates_errors_and_does_not_list_failed_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        // root 无视目录权限时该用例无意义
        let writable = std::fs::write(dir.path().join("probe"), "").is_ok();
        let mut files = Vec::new();
        let res = write_bundle_file(dir.path(), "x.json", "{}", &mut files);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        if writable {
            return;
        }
        assert!(res.is_err(), "write failure must be reported");
        assert!(files.is_empty());
    }

    /// 回归:没有 sk- / ag_local_ 前缀的 provider key(Google AIza…、Kimi、自定义
    /// header 值)出现在 error_message 里时,前缀启发式脱敏漏掉,诊断包直接泄露。
    #[test]
    fn export_bundle_redacts_exact_provider_secrets_in_logs() {
        let _guard = FS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = setup_temp_home();
        let (pool, db_temp) = setup_db_pool();
        let conn = pool.get().unwrap();
        let provider = crate::storage::providers::list_all(&conn)
            .unwrap()
            .pop()
            .unwrap();
        crate::storage::providers::update(
            &conn,
            &provider.id,
            UpdateProviderInput {
                api_key: Some(r#"["AIzaSyNoPrefixKey0123456789","kimi9f8e7d6c5b4a"]"#.to_string()),
                extra_headers: Some(r#"{"X-Custom-Auth":"hdr-secret-value-42"}"#.to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_logs (id, request_id, timestamp, error_message) VALUES (?1, ?2, ?3, ?4)",
            params![
                "log-secret",
                "req-secret",
                "2024-01-01T00:00:00Z",
                "upstream 401: key AIzaSyNoPrefixKey0123456789 / kimi9f8e7d6c5b4a / hdr-secret-value-42 rejected"
            ],
        )
        .unwrap();
        drop(conn);

        let result = export_bundle(&pool, true, 10).unwrap();
        let logs = std::fs::read_to_string(
            std::path::Path::new(&result.path).join("recent_logs.redacted.json"),
        )
        .unwrap();
        for secret in [
            "AIzaSyNoPrefixKey0123456789",
            "kimi9f8e7d6c5b4a",
            "hdr-secret-value-42",
        ] {
            assert!(!logs.contains(secret), "{secret} leaked:\n{logs}");
        }
        assert!(logs.contains("rejected"));

        cleanup(&home);
        let _ = std::fs::remove_dir_all(&db_temp);
    }
}
