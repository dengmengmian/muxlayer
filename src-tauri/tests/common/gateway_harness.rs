//! Isolated AgentGate gateway for offline tests.
//!
//! Spins up an in-memory SQLite DB, inserts a single mock provider that
//! points its upstream URLs at a `MockUpstream`, sets it active, and starts
//! the gateway on an OS-assigned port. Tests then hit the gateway over HTTP
//! exactly like a real client would.

use std::sync::OnceLock;
use std::time::Duration;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

type DbPool = Pool<SqliteConnectionManager>;

use agentgate_lib::gateway::server;
use agentgate_lib::security::local_token;
use agentgate_lib::storage;

use super::mock_upstream::MockUpstream;

/// What kind of provider the harness should impersonate. Provider-specific
/// transform behavior (MiMo image strip, DeepSeek `[1m]` strip, etc.) is
/// keyed off `provider_type`, so picking the right type is what makes a
/// test exercise the right L3 code path.
pub struct ProviderSpec {
    pub provider_type: String,
    pub default_model: String,
    /// JSON array string, e.g. `r#"["openai_chat_completions"]"#`.
    pub protocol: String,
    pub anthropic_base_url: Option<String>,
    pub model_mapping: Option<String>,
    pub model_capabilities: Option<String>,
    pub api_key: Option<String>,
}

impl ProviderSpec {
    pub fn chat_only(provider_type: &str, default_model: &str) -> Self {
        Self {
            provider_type: provider_type.to_string(),
            default_model: default_model.to_string(),
            protocol: r#"["openai_chat_completions"]"#.to_string(),
            anthropic_base_url: None,
            model_mapping: None,
            model_capabilities: None,
            api_key: Some("sk-test-key".to_string()),
        }
    }

    pub fn with_anthropic(mut self, anthropic_url: String) -> Self {
        self.anthropic_base_url = Some(anthropic_url);
        let mut protocols: Vec<String> =
            serde_json::from_str(&self.protocol).unwrap_or_else(|_| vec![self.protocol.clone()]);
        if !protocols.iter().any(|p| p == "anthropic_messages") {
            protocols.push("anthropic_messages".to_string());
        }
        self.protocol = serde_json::to_string(&protocols).expect("encode protocols");
        self
    }

    pub fn with_capabilities(mut self, json: &str) -> Self {
        self.model_capabilities = Some(json.to_string());
        self
    }

    pub fn with_mapping(mut self, json: &str) -> Self {
        self.model_mapping = Some(json.to_string());
        self
    }

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = Some(key.to_string());
        self
    }
}

pub struct GatewayHarness {
    pub gateway_url: String,
    pub token: String,
    pub provider_id: String,
    pub db: DbPool,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_handle: Option<JoinHandle<()>>,
}

impl GatewayHarness {
    /// Start a fresh gateway whose only active provider routes upstream
    /// requests at `mock`. Returns once the gateway is bound and ready.
    pub async fn start(spec: ProviderSpec, mock: &MockUpstream) -> Self {
        let token = init_isolated_home_and_token();

        // 跟生产路径一样:临时目录下的文件库 + WAL 连接池。不能用
        // `SqliteConnectionManager::memory()`——那样池里每个连接各是一个独立的
        // 空库,网关把 DB 读写挪到并发的 blocking 任务后会借到没有表的连接。
        let db_dir = tempfile::tempdir().expect("create tempdir for db");
        let db_path = db_dir.path().to_path_buf();
        std::mem::forget(db_dir);
        let pool: DbPool = storage::db::init_database(&db_path).expect("init temp db");
        let conn = pool.get().expect("borrow setup conn");
        // Migrations seed real providers (e.g. DeepSeek) + default route
        // profiles wired to them. We want a clean slate so the mock provider
        // is the only candidate the gateway can route to.
        conn.execute("DELETE FROM route_profile_providers", [])
            .expect("clear route_profile_providers");
        conn.execute("DELETE FROM providers", [])
            .expect("clear seeded providers");

        let provider_id = uuid::Uuid::new_v4().to_string();
        let base_url = mock.url();
        // `anthropic_base_url` is a routing signal: when set, /v1/messages
        // gateways via Anthropic pass-through; when None, the gateway falls
        // back to Messages → Chat translation. Mirror exactly what the spec
        // asked for — defaulting to base_url here would silently flip the
        // Messages-fallback path off.
        let anthropic_base_url = spec.anthropic_base_url.clone();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO providers (
                id, name, provider_type, base_url, api_key, default_model, reasoning_model,
                supported_models, model_mapping, extra_headers, anthropic_base_url,
                responses_base_url, protocol, timeout_seconds, status, auto_cache_control,
                model_capabilities, enabled, is_active, created_at, updated_at
             ) VALUES (
                ?1, 'mock', ?2, ?3, ?4, ?5, NULL,
                NULL, ?6, NULL, ?7,
                NULL, ?8, 30, 'ok', 0,
                ?9, 1, 0, ?10, ?10
             )",
            params![
                &provider_id,
                &spec.provider_type,
                &base_url,
                &spec.api_key,
                &spec.default_model,
                &spec.model_mapping,
                &anthropic_base_url,
                &spec.protocol,
                &spec.model_capabilities,
                &now,
            ],
        )
        .expect("insert mock provider");

        storage::providers::set_active(&conn, &provider_id).expect("set provider active");
        // set_active 内部已调用 ensure_provider_in_default_route_profiles，把 mock provider
        // 加进所有默认 route profile，所以这里不需要再手动 add_provider。

        // setup 阶段拿的 conn 在 server::start 前 drop,归还给 pool。
        drop(conn);
        let (shutdown_tx, server_handle, _counter, port) = server::start(
            "127.0.0.1",
            0,
            pool.clone(),
            None,
            agentgate_lib::wake::WakeManager::new(),
        )
        .await
        .expect("start gateway");

        // axum_server binds asynchronously; give it a tick before clients
        // start sending requests.
        tokio::time::sleep(Duration::from_millis(100)).await;

        Self {
            gateway_url: format!("http://127.0.0.1:{port}"),
            token,
            provider_id,
            db: pool,
            shutdown_tx: Some(shutdown_tx),
            server_handle: Some(server_handle),
        }
    }

    /// Turn this single-provider harness into a 2-provider failover scenario
    /// for vision-routing tests: marks the existing (primary) provider as
    /// **non-vision** (`supports_vision=0`), inserts a **vision-capable**
    /// second provider wired to `vision_mock`, and flips the default route
    /// profiles to `failover` mode so both become candidates. Returns the
    /// vision provider's id.
    ///
    /// Distinct mocks let a test assert *which* provider handled the request:
    /// an image request must land on `vision_mock`, never the primary's mock.
    pub fn add_vision_failover_candidate(
        &self,
        vision_mock: &MockUpstream,
        vision_model: &str,
    ) -> String {
        let conn = self.db.get().expect("borrow conn");

        // Primary becomes explicitly non-vision so vision requests skip it.
        conn.execute(
            "UPDATE providers SET supports_vision = 0 WHERE id = ?1",
            params![self.provider_id],
        )
        .expect("mark primary non-vision");

        let vision_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO providers (
                id, name, provider_type, base_url, api_key, default_model, reasoning_model,
                supported_models, model_mapping, extra_headers, anthropic_base_url,
                responses_base_url, protocol, timeout_seconds, status, auto_cache_control,
                model_capabilities, enabled, is_active, created_at, updated_at, supports_vision
             ) VALUES (
                ?1, 'mock-vision', 'custom', ?2, 'sk-test-key', ?3, NULL,
                NULL, NULL, NULL, NULL,
                NULL, ?4, 30, 'ok', 0,
                NULL, 1, 0, ?5, ?5, 1
             )",
            params![
                &vision_id,
                &vision_mock.url(),
                vision_model,
                r#"["openai_chat_completions","anthropic_messages"]"#,
                &now,
            ],
        )
        .expect("insert vision provider");

        // Wire the vision provider into every default profile at a lower
        // priority (runs after primary) and switch profiles to failover.
        let profile_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM route_profiles WHERE is_default = 1")
                .expect("prepare profile lookup");
            stmt.query_map([], |row| row.get::<_, String>(0))
                .expect("query profiles")
                .filter_map(|r| r.ok())
                .collect()
        };
        for profile_id in &profile_ids {
            storage::route_profiles::add_provider(
                &conn,
                profile_id,
                &vision_id,
                agentgate_lib::models::route_profile::AddProviderToRouteInput {
                    priority: Some(2),
                    model_override: None,
                    cooldown_seconds: Some(0),
                    failover_on_status_codes: None,
                    failover_on_error_keywords: None,
                    routing_conditions: None,
                },
            )
            .expect("add vision provider to route profile");
            conn.execute(
                "UPDATE route_profiles SET mode = 'failover' WHERE id = ?1",
                params![profile_id],
            )
            .expect("set failover mode");
        }

        vision_id
    }

    /// Turn this single-provider harness into a 2-provider **error-failover**
    /// scenario: inserts a second chat provider wired to `mock` at lower
    /// priority and flips default profiles to `failover` mode. Unlike the
    /// vision helper this leaves capabilities untouched — routing is purely by
    /// priority, so the primary is tried first and the secondary only on
    /// failover. Returns the secondary provider's id.
    pub fn add_failover_candidate(&self, mock: &MockUpstream, model: &str) -> String {
        let conn = self.db.get().expect("borrow conn");
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO providers (
                id, name, provider_type, base_url, api_key, default_model, reasoning_model,
                supported_models, model_mapping, extra_headers, anthropic_base_url,
                responses_base_url, protocol, timeout_seconds, status, auto_cache_control,
                model_capabilities, enabled, is_active, created_at, updated_at
             ) VALUES (
                ?1, 'mock-secondary', 'custom', ?2, 'sk-test-key', ?3, NULL,
                NULL, NULL, NULL, NULL,
                NULL, ?4, 30, 'ok', 0,
                NULL, 1, 0, ?5, ?5
             )",
            params![
                &id,
                &mock.url(),
                model,
                r#"["openai_chat_completions"]"#,
                &now,
            ],
        )
        .expect("insert secondary provider");

        let profile_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM route_profiles WHERE is_default = 1")
                .expect("prepare profile lookup");
            stmt.query_map([], |row| row.get::<_, String>(0))
                .expect("query profiles")
                .filter_map(|r| r.ok())
                .collect()
        };
        for profile_id in &profile_ids {
            storage::route_profiles::add_provider(
                &conn,
                profile_id,
                &id,
                agentgate_lib::models::route_profile::AddProviderToRouteInput {
                    priority: Some(2),
                    model_override: None,
                    cooldown_seconds: Some(0),
                    // default failover codes [429,500] — the primary returning
                    // HTTP 500 must trigger failover to this candidate.
                    failover_on_status_codes: None,
                    failover_on_error_keywords: None,
                    routing_conditions: None,
                },
            )
            .expect("add secondary to route profile");
            conn.execute(
                "UPDATE route_profiles SET mode = 'failover' WHERE id = ?1",
                params![profile_id],
            )
            .expect("set failover mode");
        }
        id
    }

    /// 读 provider 的熔断运行态:`(consecutive_failures, last_error_code)`。
    /// 没有记录时视为健康 `(0, None)`。
    pub fn runtime_status(&self, provider_id: &str) -> (i64, Option<String>) {
        let conn = self.db.get().expect("borrow conn");
        conn.query_row(
            "SELECT consecutive_failures, last_error_code FROM provider_runtime_status WHERE provider_id = ?1",
            params![provider_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .unwrap_or((0, None))
    }

    /// 熔断标记是后台写入:轮询到 `consecutive_failures >= min` 再返回运行态,
    /// 超时返回最后一次读到的值(由调用方断言)。
    pub async fn wait_runtime_failures(
        &self,
        provider_id: &str,
        min: i64,
    ) -> (i64, Option<String>) {
        for _ in 0..100 {
            let status = self.runtime_status(provider_id);
            if status.0 >= min {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.runtime_status(provider_id)
    }

    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("build client")
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.gateway_url, path)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.server_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
        }
    }
}

/// Redirect `~/.agentgate/token` to a temp directory and generate the
/// shared local token exactly once per test binary, so parallel tests
/// never race on token file writes. Returns the same token string for
/// every caller. The TempDir handle is intentionally leaked so the
/// directory survives for the test process's lifetime.
fn init_isolated_home_and_token() -> String {
    static INIT: OnceLock<String> = OnceLock::new();
    INIT.get_or_init(|| {
        let dir = tempfile::tempdir().expect("create tempdir for HOME");
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        std::env::set_var("HOME", &path);
        std::env::set_var("USERPROFILE", &path);
        local_token::ensure_token().expect("ensure token")
    })
    .clone()
}
