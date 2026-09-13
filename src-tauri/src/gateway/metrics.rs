//! Prometheus metrics 暴露层。
//!
//! 指标列表：
//! - `agentgate_requests_total{route,client,provider,status}` — counter
//! - `agentgate_request_duration_seconds{route,client,provider}` — histogram
//! - `agentgate_active_requests` — gauge（实时在飞请求数）
//! - `agentgate_upstream_tokens_total{provider,model,direction}` — counter（input/output）
//! - `agentgate_failover_attempts_total{from_provider,reason}` — counter
//!
//! 接入：`/metrics` endpoint 返回 Prometheus text 格式，监控系统按 scrape 拉。
//! 初始化在 server::start 里调一次 init()；记录点散布在 routes.rs / pass_through.rs /
//! provider_selector.rs。

use axum::response::IntoResponse;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// 全局唯一 PrometheusHandle —— OnceLock 保证 init 幂等（重复调 init 没事）。
static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// 初始化 Prometheus recorder。失败返回 false 但不 panic（已经被另一个 init 占了）。
pub fn init() -> bool {
    if HANDLE.get().is_some() {
        return false;
    }
    let builder = PrometheusBuilder::new();
    match builder.install_recorder() {
        Ok(handle) => {
            // 第一次 set 成功，后续重复 init 直接返回。
            let _ = HANDLE.set(handle);
            true
        }
        Err(_) => {
            // 推荐做法：失败也不阻断 server 启动。/metrics endpoint 会显示空。
            false
        }
    }
}

/// GET /metrics handler —— 返回 Prometheus text format。
pub async fn render() -> impl IntoResponse {
    match HANDLE.get() {
        Some(h) => (
            axum::http::StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; version=0.0.4",
            )],
            h.render(),
        ),
        None => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            "metrics recorder not initialized\n".to_string(),
        ),
    }
}

// ── 标签基数控制 ──
//
// Prometheus 每个不同的标签组合都是一条独立时间序列。model / client 标签的
// 取值来自请求(UA、上游模型名),不设上限时任意客户端都能把 /metrics 撑爆。

/// 单个标签值的最大字符数。
const MAX_LABEL_CHARS: usize = 64;
/// model 标签最多保留的不同取值数,超出后归入 "other"。
const MAX_MODEL_LABELS: usize = 256;

/// 已知客户端(与 `routes::shared::detect_client_from_ua` 的输出及各路由默认值对齐),
/// 其余一律记 "other"。
const KNOWN_CLIENTS: &[&str] = &[
    "Codex",
    "Claude Code",
    "Gemini CLI",
    "Generic",
    "OpenCode",
    "AtomCode",
    "Kimi CLI",
    "Grok Build",
    "DeepSeek Harness",
    "Cursor",
    "Cherry Studio",
    "Continue",
    "Cline",
    "Roo Code",
    "Hermes",
    "Pet",
    "OpenAI SDK",
    "Anthropic SDK",
    "Python SDK",
    "Node SDK",
    "curl",
];

static SEEN_MODELS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn truncate_label(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect()
}

fn client_label(client: &str) -> &'static str {
    KNOWN_CLIENTS
        .iter()
        .find(|known| **known == client)
        .copied()
        .unwrap_or("other")
}

fn model_label(model: &str) -> String {
    let label = truncate_label(model);
    let mut guard = SEEN_MODELS.lock().unwrap_or_else(|e| e.into_inner());
    let seen = guard.get_or_insert_with(HashSet::new);
    if seen.contains(&label) {
        return label;
    }
    if seen.len() >= MAX_MODEL_LABELS {
        return "other".to_string();
    }
    seen.insert(label.clone());
    label
}

// ── Convenience helpers — 让 hotpath 调用点不用记 metric 名 ──

/// 记一次完整请求的结果：route + client + provider + status_code。
pub fn record_request(route: &str, client: &str, provider: &str, status: u16, latency_secs: f64) {
    let client = client_label(client);
    let provider = truncate_label(provider);
    metrics::counter!(
        "agentgate_requests_total",
        "route" => route.to_string(),
        "client" => client,
        "provider" => provider.clone(),
        "status" => status.to_string(),
    )
    .increment(1);
    metrics::histogram!(
        "agentgate_request_duration_seconds",
        "route" => route.to_string(),
        "client" => client,
        "provider" => provider,
    )
    .record(latency_secs);
}

/// 记 upstream token 用量（input / output / cache_read / cache_creation 分别记）。
/// `model` 应传路由解析 / 映射后实际发给上游的模型名,标签做长度与基数封顶。
pub fn record_tokens(provider: &str, model: &str, direction: &str, count: i64) {
    if count <= 0 {
        return;
    }
    metrics::counter!(
        "agentgate_upstream_tokens_total",
        "provider" => truncate_label(provider),
        "model" => model_label(model),
        "direction" => direction.to_string(),
    )
    .increment(count as u64);
}

/// 记 failover 尝试：上一条 provider 挂的原因（http 状态码或 net-error 类）。
pub fn record_failover(from_provider: &str, reason: &str) {
    metrics::counter!(
        "agentgate_failover_attempts_total",
        "from_provider" => truncate_label(from_provider),
        "reason" => reason.to_string(),
    )
    .increment(1);
}

/// 实时活跃请求数（gauge），从 server.rs::CountingBody 的 AtomicU64 镜像而来。
/// 在 /metrics render 之前 sync 一次。
pub fn set_active_requests(n: u64) {
    metrics::gauge!("agentgate_active_requests").set(n as f64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn init_second_call_returns_false() {
        // First call may have already happened in another test in the same binary,
        // so we only assert on the idempotent property.
        let _ = init();
        assert!(!init());
    }

    #[tokio::test]
    #[serial]
    async fn render_after_init_returns_ok() {
        // Other tests may have already installed the global recorder; init is idempotent.
        let _ = init();
        record_request("/v1/chat/completions", "Codex", "openai", 200, 0.123);
        record_tokens("openai", "gpt-5", "input", 100);
        record_failover("openai", "429");
        set_active_requests(3);

        let response = render().await;
        let (parts, body) = response.into_response().into_parts();
        assert_eq!(parts.status, axum::http::StatusCode::OK);
        let body_text = body_to_string(body).await;
        assert!(body_text.contains("agentgate_requests_total"));
        assert!(body_text.contains("agentgate_upstream_tokens_total"));
        assert!(body_text.contains("agentgate_failover_attempts_total"));
        assert!(body_text.contains("agentgate_active_requests"));
    }

    async fn body_to_string(body: axum::body::Body) -> String {
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[test]
    fn client_label_maps_unknown_user_agents_to_other() {
        assert_eq!(client_label("Claude Code"), "Claude Code");
        assert_eq!(client_label("Codex"), "Codex");
        assert_eq!(client_label("MyCustomAgent/1.0"), "other");
        assert_eq!(client_label(&"x".repeat(40)), "other");
    }

    #[test]
    #[serial]
    fn model_labels_are_truncated_and_cardinality_capped() {
        let long = "m".repeat(500);
        assert_eq!(model_label(&long).chars().count(), MAX_LABEL_CHARS);

        let mut labels = HashSet::new();
        for i in 0..(MAX_MODEL_LABELS * 2) {
            labels.insert(model_label(&format!("attacker-model-{i}")));
        }
        assert!(
            labels.len() <= MAX_MODEL_LABELS + 1,
            "distinct model labels must be bounded, got {}",
            labels.len()
        );
        assert!(labels.contains("other"));
    }

    #[test]
    #[serial]
    fn record_tokens_ignores_non_positive_counts() {
        // Just ensure it does not panic; recorder may or may not be installed.
        record_tokens("p", "m", "input", 0);
        record_tokens("p", "m", "input", -5);
    }
}
