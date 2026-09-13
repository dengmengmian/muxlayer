//! request_logs 的测试：拆分前位于单文件尾部，整体集中保留。

use rusqlite::Connection;

use crate::models::request_log::RequestLogFilter;

use super::stats::TodayCostCache;
use super::*;

fn empty_logs_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE request_logs (
            id TEXT PRIMARY KEY,
            request_id TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            client TEXT,
            provider TEXT,
            model TEXT,
            route TEXT,
            status_code INTEGER,
            latency_ms INTEGER,
            input_tokens INTEGER,
            output_tokens INTEGER,
            raw_request TEXT,
            converted_request TEXT,
            raw_response TEXT,
            converted_response TEXT,
            sse_events TEXT,
            tool_calls TEXT,
            error_message TEXT,
            cost REAL,
            trace_json TEXT,
            cache_write_tokens INTEGER,
            cache_read_tokens INTEGER,
            source TEXT,
            session_id TEXT,
            external_id TEXT,
            route_profile_id TEXT
        );
        CREATE TABLE route_profiles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            client_type TEXT,
            input_protocol TEXT NOT NULL,
            mode TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            is_default INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE model_pricing (id TEXT PRIMARY KEY, provider TEXT, model_pattern TEXT,
            input_price REAL, output_price REAL, is_custom INTEGER, updated_at TEXT);",
    )
    .unwrap();
    conn
}

#[test]
fn stats_on_empty_logs_are_zero() {
    let conn = empty_logs_db();
    let stats = get_stats(&conn).unwrap();

    assert_eq!(stats.total, 0);
    assert_eq!(stats.success, 0);
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.today_total, 0);
    assert_eq!(stats.today_errors, 0);
    assert_eq!(stats.total_input_tokens, 0);
    assert_eq!(stats.total_output_tokens, 0);
    assert_eq!(stats.total_cost, 0.0);
    assert_eq!(stats.total_cache_write_tokens, 0);
    assert_eq!(stats.total_cache_read_tokens, 0);
    assert_eq!(stats.today_cache_write_tokens, 0);
    assert_eq!(stats.today_cache_read_tokens, 0);
    assert_eq!(stats.daily.len(), 7);
    assert!(stats.providers.is_empty());
}

#[test]
fn stats_for_range_returns_matching_window_size() {
    let conn = empty_logs_db();
    assert_eq!(get_stats_for_range(&conn, 1).unwrap().daily.len(), 1);
    assert_eq!(get_stats_for_range(&conn, 14).unwrap().daily.len(), 14);
    assert_eq!(get_stats_for_range(&conn, 30).unwrap().daily.len(), 30);
}

#[test]
fn stats_for_range_clamps_negative_and_huge_values() {
    let conn = empty_logs_db();
    // Below 1 clamps to 1, above 365 clamps to 365.
    assert_eq!(get_stats_for_range(&conn, 0).unwrap().daily.len(), 1);
    assert_eq!(get_stats_for_range(&conn, -5).unwrap().daily.len(), 1);
    assert_eq!(get_stats_for_range(&conn, 999).unwrap().daily.len(), 365);
}

#[test]
fn stats_keep_session_imports_out_of_gateway_quality_metrics() {
    let conn = empty_logs_db();
    insert(
        &conn,
        "req_gateway",
        "Codex",
        "LiveProvider",
        "gpt-live",
        "/v1/responses",
        200,
        123,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(10),
        Some(5),
        Some(0.01),
        None,
        None,
        Some("gateway"),
        Some("session_live"),
        Some("req_gateway"),
    )
    .unwrap();
    insert_session_log(
        &conn,
        &chrono::Utc::now().to_rfc3339(),
        "Codex",
        "openai_official",
        "gpt-history",
        "/v1/responses",
        "codex_session",
        "session_history",
        "session_history:1",
        Some(1000),
        Some(100),
        None,
        Some(50),
        Some(0.25),
    )
    .unwrap();

    let stats = get_stats_for_range(&conn, 1).unwrap();
    assert_eq!(stats.total, 1);
    assert_eq!(stats.success, 1);
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.avg_latency_ms, 123);
    assert_eq!(stats.today_total, 1);
    assert_eq!(stats.total_input_tokens, 1010);
    assert_eq!(stats.total_output_tokens, 105);
    assert_eq!(stats.total_cache_read_tokens, 50);
    assert!((stats.total_cost - 0.26).abs() < f64::EPSILON);
    assert_eq!(stats.providers.len(), 1);
    assert_eq!(stats.providers[0].name, "LiveProvider");
    assert_eq!(stats.daily[0].total, 1);
    assert_eq!(stats.daily[0].input_tokens, 1010);
}

#[test]
fn cost_breakdown_by_model_and_client() {
    let conn = empty_logs_db();
    let ins = |rid: &str, client: &str, model: &str, cost: f64| {
        insert(
            &conn,
            rid,
            client,
            "P",
            model,
            "/v1/x",
            200,
            10,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(100),
            Some(20),
            Some(cost),
            None,
            None,
            Some("gateway"),
            None,
            None,
        )
        .unwrap();
    };
    ins("r1", "Codex", "gpt-live", 0.01);
    ins("r2", "Codex", "gpt-live", 0.02);
    ins("r3", "Claude Code", "deepseek-v4", 0.05);

    // 按模型：deepseek(0.05) 成本最高排第一；gpt-live 两条合并 0.03。
    let by_model = aggregate_cost_by_model(&conn, None, 100).unwrap();
    assert_eq!(by_model.len(), 2);
    assert_eq!(by_model[0].key, "deepseek-v4");
    assert!((by_model[0].cost - 0.05).abs() < 1e-9);
    assert_eq!(by_model[0].request_count, 1);
    assert_eq!(by_model[1].key, "gpt-live");
    assert!((by_model[1].cost - 0.03).abs() < 1e-9);
    assert_eq!(by_model[1].request_count, 2);
    assert_eq!(by_model[1].input_tokens, 200);

    // 按客户端：Claude Code(0.05) 排第一；Codex 两条合并。
    let by_client = aggregate_cost_by_client(&conn, None, 100).unwrap();
    assert_eq!(by_client.len(), 2);
    assert_eq!(by_client[0].key, "Claude Code");
    assert!((by_client[0].cost - 0.05).abs() < 1e-9);
    assert_eq!(by_client[1].key, "Codex");
    assert_eq!(by_client[1].request_count, 2);
}

#[test]
fn cost_breakdown_filters_zero_token_noise() {
    let conn = empty_logs_db();
    let ins = |rid: &str, model: &str, input: Option<i64>, output: Option<i64>| {
        insert(
            &conn,
            rid,
            "Codex",
            "P",
            model,
            "/v1/x",
            200,
            10,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            input,
            output,
            Some(0.0),
            None,
            None,
            Some("gateway"),
            None,
            None,
        )
        .unwrap();
    };
    ins("r1", "real-model", Some(100), Some(20)); // 有 token
    ins("r2", "<synthetic>", Some(0), Some(0)); // 噪音：token=0
    ins("r3", "no-usage", None, None); // 噪音：无 token

    let by_model = aggregate_cost_by_model(&conn, None, 100).unwrap();
    assert_eq!(by_model.len(), 1, "token=0 的条目应被过滤");
    assert_eq!(by_model[0].key, "real-model");
}

#[test]
fn cost_breakdown_marks_missing_price() {
    let conn = empty_logs_db();
    conn.execute_batch(
        "INSERT INTO model_pricing VALUES ('1','p','priced-model', 1.0, 2.0, 0, '');",
    )
    .unwrap();
    let ins = |rid: &str, model: &str| {
        insert(
            &conn,
            rid,
            "Codex",
            "P",
            model,
            "/v1/x",
            200,
            10,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(100),
            Some(20),
            Some(0.0),
            None,
            None,
            Some("gateway"),
            None,
            None,
        )
        .unwrap();
    };
    ins("r1", "priced-model");
    ins("r2", "no-price-model");

    let by_model = aggregate_cost_by_model(&conn, None, 100).unwrap();
    let priced = by_model.iter().find(|x| x.key == "priced-model").unwrap();
    let unpriced = by_model.iter().find(|x| x.key == "no-price-model").unwrap();
    assert!(priced.has_price, "有价模型应标 has_price=true");
    assert!(!unpriced.has_price, "缺价模型应标 has_price=false");
}

#[test]
fn error_type_filter_network_and_protocol() {
    let conn = empty_logs_db();
    let ins = |rid: &str, status: i64, err: &str| {
        insert(
            &conn,
            rid,
            "C",
            "P",
            "m",
            "/x",
            status,
            10,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(err),
            None,
            None,
            None,
            None,
            None,
            None,
            Some("gateway"),
            None,
            None,
        )
        .unwrap();
    };
    ins("r1", 0, "error sending request: connection refused"); // 网络
    ins("r2", 400, "failed to parse upstream response schema"); // 协议
    ins("r3", 401, "unauthorized"); // 认证

    let f = |et: &str| RequestLogFilter {
        client: None,
        provider: None,
        model: None,
        route_profile_id: None,
        status: None,
        error_type: Some(et.to_string()),
        keyword: None,
        source: None,
        session_id: None,
        limit: Some(20),
        offset: Some(0),
    };
    let net = list(&conn, f("network_error")).unwrap();
    assert_eq!(net.len(), 1);
    assert_eq!(net[0].request_id, "r1");
    let proto = list(&conn, f("protocol_error")).unwrap();
    assert_eq!(proto.len(), 1);
    assert_eq!(proto[0].request_id, "r2");
}

#[test]
fn delete_by_session_removes_only_that_session() {
    let conn = empty_logs_db();
    let ins = |rid: &str, sess: &str| {
        insert(
            &conn,
            rid,
            "C",
            "P",
            "m",
            "/x",
            200,
            10,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(10),
            Some(5),
            Some(0.0),
            None,
            None,
            Some("gateway"),
            Some(sess),
            None,
        )
        .unwrap();
    };
    ins("r1", "s1");
    ins("r2", "s1");
    ins("r3", "s2");

    let n = delete_by_session(&conn, "s1").unwrap();
    assert_eq!(n, 2, "应只删 s1 的两行");

    let all = RequestLogFilter {
        client: None,
        provider: None,
        model: None,
        route_profile_id: None,
        status: None,
        error_type: None,
        keyword: None,
        source: None,
        session_id: None,
        limit: Some(100),
        offset: Some(0),
    };
    let remaining = list(&conn, all).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].request_id, "r3");
}

#[test]
fn avg_latency_only_counts_successful_recent() {
    let conn = empty_logs_db();
    let ins = |rid: &str, provider: &str, status: i64, latency: i64| {
        insert(
            &conn,
            rid,
            "Codex",
            provider,
            "m",
            "/v1/x",
            status,
            latency,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("gateway"),
            None,
            None,
        )
        .unwrap();
    };
    ins("r1", "fast", 200, 100);
    ins("r2", "fast", 200, 200); // 均值 150
    ins("r3", "slow", 200, 900);
    ins("r4", "slow", 500, 1); // 失败，不计入

    let map = avg_latency_by_provider(&conn, 24).unwrap();
    assert!((map["fast"] - 150.0).abs() < 1e-9);
    assert!((map["slow"] - 900.0).abs() < 1e-9); // 失败的 r4 被排除
}

#[test]
fn aggregate_route_profile_stats_from_trace_json() {
    let conn = empty_logs_db();
    let trace = |id: &str| {
        serde_json::json!({
            "route_decision": {
                "profile_id": id,
                "profile_name": "Default"
            }
        })
        .to_string()
    };

    insert(
        &conn,
        "r1",
        "Codex",
        "P",
        "m",
        "/v1/responses",
        200,
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&trace("rp1")),
        Some(100),
        Some(20),
        Some(0.03),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();
    insert(
        &conn,
        "r2",
        "Codex",
        "P",
        "m",
        "/v1/responses",
        500,
        300,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("boom"),
        Some(&trace("rp1")),
        Some(50),
        Some(10),
        Some(0.02),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();

    let stats = aggregate_route_profile_stats(&conn, None).unwrap();

    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].route_profile_id, "rp1");
    assert_eq!(stats[0].request_count, 2);
    assert_eq!(stats[0].success_count, 1);
    assert_eq!(stats[0].error_count, 1);
    assert!((stats[0].success_rate - 0.5).abs() < 1e-9);
    assert_eq!(stats[0].avg_latency_ms, 200);
    assert!((stats[0].cost - 0.05).abs() < 1e-9);
}

#[test]
fn aggregate_route_profile_stats_maps_legacy_gateway_logs_to_default_profile() {
    let conn = empty_logs_db();
    conn.execute(
        "INSERT INTO route_profiles (id, name, client_type, input_protocol, mode, enabled, is_default, created_at, updated_at)
         VALUES ('rp-chat', 'Chat Completions Default', '', 'openai_chat_completions', 'manual', 1, 1, '', '')",
        [],
    )
    .unwrap();

    insert(
        &conn,
        "r1",
        "OpenCode",
        "DeepSeek",
        "deepseek-v4-pro",
        "/v1/chat/completions",
        200,
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(r#"{"mode":"native_pass_through_model_mapping"}"#),
        Some(100),
        Some(20),
        Some(0.03),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();
    insert(
        &conn,
        "r2",
        "OpenCode",
        "DeepSeek",
        "deepseek-v4-pro",
        "/v1/chat/completions",
        500,
        300,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("boom"),
        Some(r#"{"mode":"native_pass_through_model_mapping"}"#),
        Some(50),
        Some(10),
        Some(0.02),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();

    let stats = aggregate_route_profile_stats(&conn, None).unwrap();

    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].route_profile_id, "rp-chat");
    assert_eq!(stats[0].request_count, 2);
    assert_eq!(stats[0].success_count, 1);
    assert_eq!(stats[0].error_count, 1);
    assert_eq!(stats[0].avg_latency_ms, 200);
    assert!((stats[0].cost - 0.05).abs() < 1e-9);
}

#[test]
fn aggregate_provider_detail_stats_by_model_and_latency() {
    let conn = empty_logs_db();
    insert(
        &conn,
        "r1",
        "Codex",
        "P",
        "m1",
        "/v1/responses",
        200,
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(100),
        Some(20),
        Some(0.03),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();
    insert(
        &conn,
        "r2",
        "Codex",
        "P",
        "m1",
        "/v1/responses",
        500,
        300,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("boom"),
        None,
        Some(50),
        Some(10),
        Some(0.02),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();
    insert(
        &conn,
        "r3",
        "Codex",
        "P",
        "m2",
        "/v1/responses",
        200,
        500,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(70),
        Some(30),
        Some(0.05),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();
    insert(
        &conn,
        "r4",
        "Codex",
        "Other",
        "m1",
        "/v1/responses",
        200,
        1,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(1),
        Some(1),
        Some(9.0),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();

    let stats = aggregate_provider_detail_stats(&conn, "P", None, 20).unwrap();

    assert_eq!(stats.model_stats.len(), 2);
    assert_eq!(stats.model_stats[0].model, "m1");
    assert_eq!(stats.model_stats[0].request_count, 2);
    assert_eq!(stats.model_stats[0].success_count, 1);
    assert_eq!(stats.model_stats[0].error_count, 1);
    assert!((stats.model_stats[0].success_rate - 0.5).abs() < 1e-9);
    assert_eq!(stats.model_stats[0].avg_latency_ms, 200);
    assert!((stats.model_stats[0].cost - 0.05).abs() < 1e-9);
    assert_eq!(stats.model_stats[1].model, "m2");
    assert_eq!(stats.model_stats[1].request_count, 1);
    assert_eq!(stats.model_stats[1].success_count, 1);
    assert!((stats.model_stats[1].success_rate - 1.0).abs() < 1e-9);
    assert!((stats.model_stats[1].cost - 0.05).abs() < 1e-9);
    assert_eq!(stats.latency_points.len(), 3);
    assert!(stats.latency_points.iter().all(|p| p.latency_ms > 0));
}

#[test]
fn list_filters_by_route_profile_model_and_error_type() {
    let conn = empty_logs_db();
    let trace = |id: &str| {
        serde_json::json!({
            "route_decision": {
                "profile_id": id,
                "profile_name": "Default"
            }
        })
        .to_string()
    };

    insert(
        &conn,
        "r1",
        "Codex",
        "P",
        "m1",
        "/v1/responses",
        429,
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("rate limit exceeded"),
        Some(&trace("rp1")),
        Some(10),
        Some(1),
        Some(0.01),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();
    insert(
        &conn,
        "r2",
        "Codex",
        "P",
        "m2",
        "/v1/responses",
        401,
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("Unauthorized"),
        Some(&trace("rp1")),
        Some(10),
        Some(1),
        Some(0.01),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();
    insert(
        &conn,
        "r3",
        "Codex",
        "P",
        "m1",
        "/v1/responses",
        500,
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("server error"),
        Some(&trace("rp2")),
        Some(10),
        Some(1),
        Some(0.01),
        None,
        None,
        Some("gateway"),
        None,
        None,
    )
    .unwrap();

    let logs = list(
        &conn,
        RequestLogFilter {
            client: None,
            provider: None,
            model: Some("m1".to_string()),
            route_profile_id: Some("rp1".to_string()),
            status: None,
            error_type: Some("rate_limited".to_string()),
            keyword: None,
            source: None,
            session_id: None,
            limit: Some(20),
            offset: Some(0),
        },
    )
    .unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].request_id, "r1");

    let auth_count = count(
        &conn,
        &RequestLogFilter {
            client: None,
            provider: None,
            model: None,
            route_profile_id: None,
            status: None,
            error_type: Some("auth_failed".to_string()),
            keyword: None,
            source: None,
            session_id: None,
            limit: None,
            offset: None,
        },
    )
    .unwrap();
    assert_eq!(auth_count, 1);
}

#[test]
fn detail_includes_cost_and_cache_tokens() {
    let conn = empty_logs_db();
    insert(
        &conn,
        "r1",
        "Codex",
        "P",
        "m1",
        "/v1/responses",
        200,
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(100),
        Some(20),
        Some(0.03),
        Some(11),
        Some(22),
        Some("gateway"),
        None,
        None,
    )
    .unwrap();

    let item = list(
        &conn,
        RequestLogFilter {
            client: None,
            provider: None,
            model: None,
            route_profile_id: None,
            status: None,
            error_type: None,
            keyword: None,
            source: None,
            session_id: None,
            limit: Some(1),
            offset: Some(0),
        },
    )
    .unwrap()
    .remove(0);
    let detail = get_detail(&conn, &item.id).unwrap();

    assert_eq!(detail.input_tokens, Some(100));
    assert_eq!(detail.output_tokens, Some(20));
    assert_eq!(detail.cache_write_tokens, Some(11));
    assert_eq!(detail.cache_read_tokens, Some(22));
    assert_eq!(detail.cost, Some(0.03));
}

#[test]
fn extract_cache_tokens_anthropic_format() {
    let usage = serde_json::json!({
        "input_tokens": 100,
        "cache_creation_input_tokens": 80,
        "cache_read_input_tokens": 20,
    });
    let (w, r) = extract_cache_tokens(&usage);
    assert_eq!(w, Some(80));
    assert_eq!(r, Some(20));
}

#[test]
fn extract_cache_tokens_openai_responses_format() {
    let usage = serde_json::json!({
        "input_tokens": 100,
        "input_tokens_details": {"cached_tokens": 60},
        "output_tokens": 30,
    });
    let (w, r) = extract_cache_tokens(&usage);
    assert_eq!(w, None, "OpenAI Responses doesn't surface cache writes");
    assert_eq!(r, Some(60));
}

#[test]
fn extract_cache_tokens_openai_chat_completions_format() {
    let usage = serde_json::json!({
        "prompt_tokens": 100,
        "prompt_tokens_details": {"cached_tokens": 45},
        "completion_tokens": 20,
    });
    let (w, r) = extract_cache_tokens(&usage);
    assert_eq!(w, None);
    assert_eq!(r, Some(45));
}

#[test]
fn extract_cache_tokens_bare_field() {
    let usage = serde_json::json!({"cached_tokens": 7});
    let (w, r) = extract_cache_tokens(&usage);
    assert_eq!(w, None);
    assert_eq!(r, Some(7));
}

#[test]
fn extract_cache_tokens_empty_usage_returns_none() {
    let usage = serde_json::json!({});
    let (w, r) = extract_cache_tokens(&usage);
    assert_eq!(w, None);
    assert_eq!(r, None);
}

#[test]
fn extract_cache_tokens_prefers_anthropic_over_openai_when_both_present() {
    // Pathological: provider that emits both keys. Anthropic Write field
    // is unambiguous; Read fields are equivalent so we don't care which
    // wins for Read as long as both are non-null.
    let usage = serde_json::json!({
        "cache_creation_input_tokens": 50,
        "cache_read_input_tokens": 25,
        "input_tokens_details": {"cached_tokens": 99},
    });
    let (w, r) = extract_cache_tokens(&usage);
    assert_eq!(w, Some(50));
    assert_eq!(
        r,
        Some(25),
        "anthropic cache_read takes priority over openai cached_tokens"
    );
}

#[test]
fn provider_health_on_empty_logs_is_zero() {
    let conn = empty_logs_db();
    let health = get_provider_health(&conn, "DeepSeek").unwrap();

    assert_eq!(health.h1_total, 0);
    assert_eq!(health.h1_success, 0);
    assert_eq!(health.h1_success_rate, 0.0);
    assert_eq!(health.h1_avg_latency_ms, 0);
    assert_eq!(health.h1_p95_latency_ms, 0);
    assert_eq!(health.h24_total, 0);
    assert_eq!(health.h24_success, 0);
    assert_eq!(health.h24_success_rate, 0.0);
    assert_eq!(health.h24_avg_latency_ms, 0);
    assert!(health.recent_errors.is_empty());
}

fn utc(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn insert_gateway_row_at(conn: &Connection, id: &str, ts: &str, status: i64, cost: f64) {
    conn.execute(
        "INSERT INTO request_logs (id, request_id, timestamp, provider, model, status_code, latency_ms,
                input_tokens, output_tokens, cost, source)
         VALUES (?1, ?1, ?2, 'P', 'm', ?3, 10, 10, 5, ?4, 'gateway')",
        rusqlite::params![id, ts, status, cost],
    )
    .unwrap();
}

#[test]
fn today_cost_and_daily_buckets_use_local_day() {
    // UTC+8 用户:本地 06-02 09:00(= 06-02T01:00Z)。
    // A = 06-01T17:00Z → 本地 06-02 01:00(今天);B = 06-01T15:00Z → 本地 06-01 23:00(昨天)。
    // 旧实现按 UTC 切日:A、B 都算 06-01,"今日花费"为 0,预算闸门 08:00 才重置。
    let conn = empty_logs_db();
    let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let now = utc("2026-06-02T01:00:00Z");
    insert_gateway_row_at(&conn, "A", "2026-06-01T17:00:00+00:00", 200, 1.0);
    insert_gateway_row_at(&conn, "B", "2026-06-01T15:00:00+00:00", 500, 2.0);
    // 次日本地 00:00 = 06-02T16:00Z,不应计入今天。
    insert_gateway_row_at(&conn, "C", "2026-06-02T16:00:00+00:00", 200, 4.0);

    let today = today_cost_at(&conn, now, &tz).unwrap();
    assert!((today - 1.0).abs() < 1e-9, "today_cost={today}");

    let stats = get_stats_for_range_at(&conn, 2, now, &tz).unwrap();
    assert!((stats.today_cost - 1.0).abs() < 1e-9);
    assert_eq!(stats.today_total, 1);
    assert_eq!(stats.today_errors, 0);
    assert_eq!(stats.daily.len(), 2);
    assert_eq!(stats.daily[0].date, "2026-06-01");
    assert_eq!(stats.daily[0].total, 1);
    assert_eq!(stats.daily[0].errors, 1);
    assert!((stats.daily[0].cost - 2.0).abs() < 1e-9);
    assert_eq!(stats.daily[1].date, "2026-06-02");
    assert_eq!(stats.daily[1].total, 1);
    assert!((stats.daily[1].cost - 1.0).abs() < 1e-9);
}

#[test]
fn today_cost_counts_rows_stored_with_z_suffix() {
    // 旧版本会话同步写入的 "…Z" 格式时间戳也必须按区间正确比较。
    let conn = empty_logs_db();
    let tz = chrono::FixedOffset::east_opt(0).unwrap();
    let now = utc("2026-06-02T10:00:00Z");
    insert_gateway_row_at(&conn, "z0", "2026-06-02T00:00:00Z", 200, 1.0);
    insert_gateway_row_at(&conn, "z1", "2026-06-02T00:00:00.123Z", 200, 1.0);
    insert_gateway_row_at(&conn, "y", "2026-06-01T23:59:59.999Z", 200, 5.0);
    insert_gateway_row_at(&conn, "n", "2026-06-03T00:00:00Z", 200, 7.0);
    let today = today_cost_at(&conn, now, &tz).unwrap();
    assert!((today - 2.0).abs() < 1e-9, "today_cost={today}");
}

#[test]
fn today_cost_cache_adds_inserted_cost_within_cached_day() {
    let mut cache = TodayCostCache {
        start: "2026-06-01T16:00:00+00:00".to_string(),
        end: "2026-06-02T16:00:00+00:00".to_string(),
        at: std::time::Instant::now(),
        cost: 1.0,
    };
    cache.apply_insert("2026-06-02T01:00:00.5+00:00", Some(0.25));
    assert!((cache.cost - 1.25).abs() < 1e-9);
    cache.apply_insert("2026-06-02T16:00:00+00:00", Some(10.0)); // 次日,不计
    cache.apply_insert("2026-06-01T15:59:59+00:00", Some(10.0)); // 前一日,不计
    cache.apply_insert("2026-06-02T02:00:00+00:00", None);
    assert!((cache.cost - 1.25).abs() < 1e-9);
}

#[test]
fn stats_propagate_db_errors_instead_of_zeroing() {
    // 缺 trace_json 列:codex_compact 计数查询失败,旧实现 unwrap_or(0) 静默吞掉。
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE request_logs (
            id TEXT PRIMARY KEY, request_id TEXT, timestamp TEXT, provider TEXT, model TEXT,
            status_code INTEGER, latency_ms INTEGER, input_tokens INTEGER, output_tokens INTEGER,
            cost REAL, cache_write_tokens INTEGER, cache_read_tokens INTEGER, source TEXT
        );",
    )
    .unwrap();
    assert!(get_stats_for_range(&conn, 7).is_err());
}

#[test]
fn stats_providers_propagate_row_errors() {
    let conn = empty_logs_db();
    conn.execute(
        "INSERT INTO request_logs (id, request_id, timestamp, provider, source, status_code)
         VALUES ('x', 'x', ?1, X'00FF', 'gateway', 200)",
        [chrono::Utc::now().to_rfc3339()],
    )
    .unwrap();
    assert!(
        get_stats_for_range(&conn, 7).is_err(),
        "坏行不能被 filter_map 静默丢弃"
    );
}

#[test]
fn provider_health_propagates_recent_error_row_errors() {
    let conn = empty_logs_db();
    conn.execute(
        "INSERT INTO request_logs (id, request_id, timestamp, provider, source, status_code, error_message)
         VALUES ('x', 'x', ?1, 'DeepSeek', 'gateway', 500, X'00FF')",
        [chrono::Utc::now().to_rfc3339()],
    )
    .unwrap();
    assert!(get_provider_health(&conn, "DeepSeek").is_err());
}

fn keyword_filter(keyword: &str) -> RequestLogFilter {
    RequestLogFilter {
        client: None,
        provider: None,
        model: None,
        route_profile_id: None,
        status: None,
        error_type: None,
        keyword: Some(keyword.to_string()),
        source: None,
        session_id: None,
        limit: Some(100),
        offset: Some(0),
    }
}

#[test]
fn keyword_search_treats_like_wildcards_literally() {
    let conn = empty_logs_db();
    for (id, msg) in [
        ("r1", "quota 100% used"),
        ("r2", "quota 1000 used"),
        ("r3", "a_b"),
        ("r4", "axb"),
        ("r5", "path\\to"),
    ] {
        conn.execute(
            "INSERT INTO request_logs (id, request_id, timestamp, error_message) VALUES (?1, ?1, '2026-01-01T00:00:00+00:00', ?2)",
            rusqlite::params![id, msg],
        )
        .unwrap();
    }
    let ids = |kw: &str| -> Vec<String> {
        let mut v: Vec<String> = list(&conn, keyword_filter(kw))
            .unwrap()
            .into_iter()
            .map(|r| r.request_id)
            .collect();
        v.sort();
        v
    };
    assert_eq!(ids("100%"), vec!["r1"]);
    assert_eq!(ids("a_b"), vec!["r3"]);
    assert_eq!(ids("h\\t"), vec!["r5"]);
    assert_eq!(count(&conn, &keyword_filter("100%")).unwrap(), 1);
}

#[test]
fn route_profile_filter_uses_partial_index() {
    let conn = Connection::open_in_memory().unwrap();
    crate::storage::migrations::run_migrations(&conn).unwrap();
    let filter = RequestLogFilter {
        client: None,
        provider: None,
        model: None,
        route_profile_id: Some("rp1".to_string()),
        status: None,
        error_type: None,
        keyword: None,
        source: None,
        session_id: None,
        limit: None,
        offset: None,
    };
    let mut sql = String::from("SELECT COUNT(*) FROM request_logs WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut idx = 1;
    super::query::apply_log_filter(&filter, &mut sql, &mut params, &mut idx);
    let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let plan: Vec<String> = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map(refs.as_slice(), |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let plan = plan.join(" | ");
    assert!(
        plan.contains("idx_request_logs_route_profile_id"),
        "route_profile_id 过滤应走部分索引,实际计划: {plan}"
    );
}
