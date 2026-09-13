use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GatewaySettings {
    pub id: i64,
    pub host: String,
    pub port: i64,
    pub active_provider_id: Option<String>,
    pub input_protocol: String,
    pub output_protocol: String,
    pub auto_start: bool,
    pub log_retention_days: i64,
    /// Global master switches for the refiner pipeline. When off, every
    /// per-provider opt-in is ignored — the gateway stays byte-level
    /// transparent. Default off for both, so the upgrade is silent.
    pub body_filter_global: bool,
    pub thinking_rectifier_global: bool,
    pub error_mapper_global: bool,
    /// 后台主动健康探测开关（默认关——开启后按间隔发 1-token 探测，消耗少量额度；
    /// 结果仅用于展示，不影响路由）。
    pub health_probe_enabled: bool,
    /// Codex remote compaction v2 本地实现开关。默认开,但只在请求带 v2 探嗅
    /// 信号(`x-codex-beta-features` 含 `remote_compaction_v2` / metadata 标 compaction
    /// / URL `/compact`)时才介入,对其他 client 透明无影响。
    pub codex_compact_enabled: bool,
    /// codex_compact 触发后给上游 summary 调用的 max_completion_tokens 上限。
    pub codex_compact_summary_max_tokens: i64,
    /// 网关接收单次请求体的上限(MB)。可被 AGENTGATE_REQUEST_BODY_LIMIT_MB 覆盖。
    pub request_body_limit_mb: i64,
    /// 今日花费预警开关(默认关)。
    pub cost_alert_enabled: bool,
    /// 今日花费预警阈值(USD)。None / <=0 视为未设。
    pub cost_alert_threshold: Option<f64>,
    /// 每日花费硬闸开关(默认关)。与 cost_alert 独立：alert 只通知，budget 可 block / force_cheapest。
    pub cost_budget_enabled: bool,
    /// 每日花费硬闸阈值(USD)。None / <=0 视为未设。
    pub cost_budget_threshold: Option<f64>,
    /// `notify_only` | `block` | `force_cheapest`
    pub cost_budget_strategy: String,
    /// 长历史自压缩总开关。默认开；可被 AGENTGATE_AUTO_COMPACT=off 强制关。
    pub auto_compact_enabled: bool,
    /// 上下文窗口用于 history 的百分比(默认 85，留 15% 给 reasoning/output)。1–95。
    pub auto_compact_usage_percent: i64,
    /// 防休眠总开关。默认开启。
    pub wake_enabled: bool,
    /// false=MuxLayer 生命周期持续保持，true=仅按生成请求控制。
    pub wake_request_control: bool,
    /// 最后一个生成请求结束后的保持秒数。
    pub wake_cooldown_seconds: i64,
    /// 在保持系统唤醒的同时保持显示器常亮。
    pub wake_keep_display_awake: bool,
    /// 出站 HTTP 代理开关。关时仍尊重环境变量 HTTP(S)_PROXY。
    pub outbound_proxy_enabled: bool,
    /// 例如 `http://127.0.0.1:7890`。仅 http/https。
    pub outbound_proxy_url: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Deserialize, Type)]
pub struct UpdateGatewaySettingsInput {
    pub host: Option<String>,
    pub port: Option<i64>,
    pub active_provider_id: Option<String>,
    pub input_protocol: Option<String>,
    pub output_protocol: Option<String>,
    pub auto_start: Option<bool>,
    pub log_retention_days: Option<i64>,
    pub body_filter_global: Option<bool>,
    pub thinking_rectifier_global: Option<bool>,
    pub error_mapper_global: Option<bool>,
    pub health_probe_enabled: Option<bool>,
    pub codex_compact_enabled: Option<bool>,
    pub codex_compact_summary_max_tokens: Option<i64>,
    pub request_body_limit_mb: Option<i64>,
    pub cost_alert_enabled: Option<bool>,
    pub cost_alert_threshold: Option<f64>,
    pub cost_budget_enabled: Option<bool>,
    pub cost_budget_threshold: Option<f64>,
    pub cost_budget_strategy: Option<String>,
    pub auto_compact_enabled: Option<bool>,
    pub auto_compact_usage_percent: Option<i64>,
    pub wake_enabled: Option<bool>,
    pub wake_request_control: Option<bool>,
    pub wake_cooldown_seconds: Option<i64>,
    pub wake_keep_display_awake: Option<bool>,
    pub outbound_proxy_enabled: Option<bool>,
    pub outbound_proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct GatewayStatus {
    pub running: bool,
    pub host: String,
    pub port: i64,
    pub active_provider: Option<String>,
    pub input_protocol: String,
    pub output_protocol: String,
    pub started_at: Option<String>,
}

/// Runtime state for the gateway HTTP server.
/// The shutdown_tx and server_handle cannot be Clone,
/// so they live behind Option and are taken when stopping.
#[derive(Default)]
pub struct GatewayRuntimeState {
    pub running: bool,
    pub host: String,
    pub port: u16,
    pub started_at: Option<String>,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
    pub server_handle: Option<JoinHandle<()>>,
    pub active_requests: Option<Arc<AtomicU64>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_settings_roundtrip() {
        let s = GatewaySettings {
            id: 1,
            host: "127.0.0.1".into(),
            port: 9090,
            active_provider_id: Some("p1".into()),
            input_protocol: "responses".into(),
            output_protocol: "chat".into(),
            auto_start: true,
            log_retention_days: 14,
            body_filter_global: false,
            thinking_rectifier_global: false,
            error_mapper_global: false,
            health_probe_enabled: false,
            codex_compact_enabled: true,
            codex_compact_summary_max_tokens: 1500,
            request_body_limit_mb: 32,
            cost_alert_enabled: false,
            cost_alert_threshold: None,
            cost_budget_enabled: false,
            cost_budget_threshold: None,
            cost_budget_strategy: "notify_only".into(),
            auto_compact_enabled: true,
            auto_compact_usage_percent: 85,
            wake_enabled: true,
            wake_request_control: false,
            wake_cooldown_seconds: 900,
            wake_keep_display_awake: false,
            outbound_proxy_enabled: false,
            outbound_proxy_url: None,
            updated_at: "2024-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("127.0.0.1"));
        assert!(json.contains("9090"));
        let de: GatewaySettings = serde_json::from_str(&json).unwrap();
        assert_eq!(de.port, 9090);
        assert!(de.auto_start);
        assert_eq!(de.request_body_limit_mb, 32);
    }

    #[test]
    fn update_gateway_settings_input_deserialization() {
        let json =
            r#"{"host":"0.0.0.0","port":8080,"auto_start":false,"request_body_limit_mb":32}"#;
        let input: UpdateGatewaySettingsInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.host, Some("0.0.0.0".into()));
        assert_eq!(input.port, Some(8080));
        assert_eq!(input.auto_start, Some(false));
        assert_eq!(input.request_body_limit_mb, Some(32));
        assert!(input.active_provider_id.is_none());
    }

    #[test]
    fn gateway_status_serialization() {
        let s = GatewayStatus {
            running: true,
            host: "127.0.0.1".into(),
            port: 9090,
            active_provider: Some("deepseek".into()),
            input_protocol: "responses".into(),
            output_protocol: "chat".into(),
            started_at: Some("2024-01-01T00:00:00Z".into()),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("running"));
        assert!(json.contains("deepseek"));
    }

    #[test]
    fn gateway_runtime_state_default() {
        let s = GatewayRuntimeState::default();
        assert!(!s.running);
        assert_eq!(s.port, 0);
        assert!(s.started_at.is_none());
    }
}
