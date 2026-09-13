//! Shared upstream HTTP client: timeouts, pool, optional outbound proxy.
//!
//! Loopback (`127.0.0.1` / `localhost` / `::1`) is always excluded from the
//! configured proxy so pet / local probes stay on-box. When the UI proxy is
//! off, reqwest still honours `HTTP(S)_PROXY` from the environment.

use reqwest::{Client, Proxy};
use std::time::Duration;

use crate::errors::AppError;
use crate::models::gateway::GatewaySettings;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POOL_IDLE: Duration = Duration::from_secs(30);
const TCP_KEEPALIVE: Duration = Duration::from_secs(20);
const NO_PROXY_HOSTS: &str = "localhost,127.0.0.1,::1";
/// 上游重定向最多跟 5 跳,且只跟同源(scheme + host + port)或同 host 的
/// http→https 升级跳转。跨 host 跳转不跟:reqwest 跨 host 只剥 Authorization,
/// x-api-key / x-goog-api-key 会被带到第三方主机。停下后 3xx 作为上游错误交给 failover。
const MAX_REDIRECTS: usize = 5;

/// 非流式上游响应 body 上限。`resp.text()` 无上限,异常上游能把网关内存打满。
pub const MAX_NON_STREAM_BODY_BYTES: usize = 64 * 1024 * 1024;

/// 读完非流式上游响应 body,超过 [`MAX_NON_STREAM_BODY_BYTES`] 返回明确错误。
pub async fn read_text_capped(resp: reqwest::Response) -> Result<String, AppError> {
    read_text_with_cap(resp, MAX_NON_STREAM_BODY_BYTES).await
}

async fn read_text_with_cap(mut resp: reqwest::Response, cap: usize) -> Result<String, AppError> {
    let too_large = || {
        AppError::new(
            crate::errors::codes::UPSTREAM_NON_STREAM_ERROR,
            format!(
                "Upstream response body exceeds the {} MiB limit",
                cap / (1024 * 1024)
            ),
        )
        .with_suggestion("上游返回了异常大的响应,已中止读取以保护网关内存")
    };
    if resp.content_length().is_some_and(|len| len as usize > cap) {
        return Err(too_large());
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| {
        AppError::new(
            crate::errors::codes::PASS_THROUGH_REQUEST_FAILED,
            format!("Failed to read upstream response body: {e}"),
        )
    })? {
        if buf.len() + chunk.len() > cap {
            return Err(too_large());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 是否跟随这一跳:host 必须相同,且 scheme + 端口不变,或是同 host 的
/// http→https 升级(默认端口 80→443,或显式同端口)。https→http 降级不跟。
fn redirect_allowed(origin: &reqwest::Url, target: &reqwest::Url) -> bool {
    if origin.host_str() != target.host_str() {
        return false;
    }
    let (from_port, to_port) = (
        origin.port_or_known_default(),
        target.port_or_known_default(),
    );
    if origin.scheme() == target.scheme() {
        return from_port == to_port;
    }
    origin.scheme() == "http"
        && target.scheme() == "https"
        && ((from_port == Some(80) && to_port == Some(443)) || from_port == to_port)
}

fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let Some(origin) = attempt.previous().first() else {
            return attempt.stop();
        };
        if redirect_allowed(origin, attempt.url()) && attempt.previous().len() <= MAX_REDIRECTS {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// Normalize and reject non-http(s) proxy URLs. Empty / whitespace → None.
pub fn parse_proxy_url(raw: &str) -> Result<Option<String>, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let url = reqwest::Url::parse(trimmed).map_err(|e| {
        AppError::validation(format!("Invalid outbound proxy URL: {e}"))
            .with_suggestion("Use http://127.0.0.1:7890 or https://proxy.example:8080")
    })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(AppError::validation(format!(
            "Outbound proxy must be http:// or https://, got {}",
            url.scheme()
        ))
        .with_suggestion("SOCKS is not supported; point at a local HTTP proxy (Clash / V2Ray)."));
    }
    if url.host_str().is_none() {
        return Err(AppError::validation("Outbound proxy URL is missing a host"));
    }
    Ok(Some(trimmed.to_string()))
}

pub fn proxy_from_settings(settings: &GatewaySettings) -> Result<Option<Proxy>, AppError> {
    if !settings.outbound_proxy_enabled {
        return Ok(None);
    }
    let Some(url) = parse_proxy_url(settings.outbound_proxy_url.as_deref().unwrap_or(""))? else {
        return Ok(None);
    };
    let proxy = Proxy::all(&url)
        .map_err(|e| AppError::validation(format!("Cannot apply outbound proxy: {e}")))?
        .no_proxy(reqwest::NoProxy::from_string(NO_PROXY_HOSTS));
    Ok(Some(proxy))
}

pub fn build_upstream_client(proxy: Option<Proxy>) -> Result<Client, AppError> {
    let mut builder = Client::builder()
        .read_timeout(Duration::from_secs(
            crate::gateway::sse_bootstrap::STREAM_READ_IDLE_HINT_SECS,
        ))
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(same_origin_redirect_policy())
        .pool_idle_timeout(POOL_IDLE)
        .tcp_keepalive(TCP_KEEPALIVE);
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| AppError::internal(format!("Failed to create HTTP client: {e}")))
}

pub fn build_upstream_client_from_settings(settings: &GatewaySettings) -> Result<Client, AppError> {
    build_upstream_client(proxy_from_settings(settings)?)
}

pub fn apply_optional_proxy(
    builder: reqwest::ClientBuilder,
    proxy: Option<Proxy>,
) -> reqwest::ClientBuilder {
    match proxy {
        Some(proxy) => builder.proxy(proxy),
        None => builder,
    }
}

pub fn proxy_from_db(db: &crate::storage::db::DbPool) -> Option<Proxy> {
    let conn = db.get().ok()?;
    let settings = crate::storage::gateway_settings::get(&conn).ok()?;
    proxy_from_settings(&settings).ok().flatten()
}

pub fn build_upstream_client_from_db(db: &crate::storage::db::DbPool) -> Result<Client, AppError> {
    let settings = db
        .get()
        .ok()
        .and_then(|conn| crate::storage::gateway_settings::get(&conn).ok());
    match settings {
        Some(s) => build_upstream_client_from_settings(&s),
        None => build_upstream_client(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cross_origin_redirect_is_not_followed() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let other = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("leaked"))
            .mount(&other)
            .await;
        let origin = MockServer::start().await;
        // 同源跳转照常跟随
        Mock::given(method("POST"))
            .and(path("/same"))
            .respond_with(ResponseTemplate::new(307).insert_header("location", "/final"))
            .mount(&origin)
            .await;
        Mock::given(method("POST"))
            .and(path("/final"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&origin)
            .await;
        // 跨源跳转(另一个端口 + 另一个 host 名)
        let other_url = other.uri().replace("127.0.0.1", "localhost");
        Mock::given(method("POST"))
            .and(path("/cross"))
            .respond_with(
                ResponseTemplate::new(307).insert_header("location", format!("{other_url}/steal")),
            )
            .mount(&origin)
            .await;

        let client = build_upstream_client(None).unwrap();
        let same = client
            .post(format!("{}/same", origin.uri()))
            .header("x-api-key", "sk-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(same.status().as_u16(), 200);

        let cross = client
            .post(format!("{}/cross", origin.uri()))
            .header("x-api-key", "sk-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(
            cross.status().as_u16(),
            307,
            "cross-origin redirect must stop"
        );
        assert!(
            other.received_requests().await.unwrap().is_empty(),
            "x-api-key must never reach the redirect target"
        );
    }

    #[test]
    fn redirect_allows_same_host_https_upgrade_but_not_cross_host_or_downgrade() {
        let u = |s: &str| reqwest::Url::parse(s).unwrap();
        // 同源
        assert!(redirect_allowed(
            &u("https://api.example.com/v1"),
            &u("https://api.example.com/v2")
        ));
        // 同 host 的 http→https 升级:默认端口 80→443、或显式同端口
        assert!(redirect_allowed(
            &u("http://api.example.com/v1"),
            &u("https://api.example.com/v1")
        ));
        assert!(redirect_allowed(
            &u("http://api.example.com:80/v1"),
            &u("https://api.example.com:443/v1")
        ));
        assert!(redirect_allowed(
            &u("http://10.0.0.2:8080/v1"),
            &u("https://10.0.0.2:8080/v1")
        ));
        // 升级但端口乱跳、https→http 降级、跨 host:一律不跟
        assert!(!redirect_allowed(
            &u("http://api.example.com/v1"),
            &u("https://api.example.com:8443/v1")
        ));
        assert!(!redirect_allowed(
            &u("https://api.example.com/v1"),
            &u("http://api.example.com/v1")
        ));
        assert!(!redirect_allowed(
            &u("http://api.example.com/v1"),
            &u("https://evil.example.com/v1")
        ));
        assert!(!redirect_allowed(
            &u("https://api.example.com/v1"),
            &u("https://api.example.com:8443/v1")
        ));
    }

    #[tokio::test]
    async fn capped_reader_rejects_oversized_body() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(2048)))
            .mount(&server)
            .await;
        let client = build_upstream_client(None).unwrap();

        let resp = client.get(server.uri()).send().await.unwrap();
        let err = read_text_with_cap(resp, 1024).await.unwrap_err();
        assert!(err.message.contains("exceeds"), "{}", err.message);

        let resp = client.get(server.uri()).send().await.unwrap();
        assert_eq!(read_text_with_cap(resp, 4096).await.unwrap().len(), 2048);
    }

    #[test]
    fn empty_url_is_none() {
        assert_eq!(parse_proxy_url("").unwrap(), None);
        assert_eq!(parse_proxy_url("   ").unwrap(), None);
    }

    #[test]
    fn accepts_http_and_https() {
        assert_eq!(
            parse_proxy_url("http://127.0.0.1:7890").unwrap().as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert!(parse_proxy_url("https://proxy.local:8080")
            .unwrap()
            .is_some());
    }

    #[test]
    fn rejects_socks_and_garbage() {
        assert!(parse_proxy_url("socks5://127.0.0.1:1080").is_err());
        assert!(parse_proxy_url("not a url").is_err());
        assert!(parse_proxy_url("ftp://x").is_err());
    }

    #[test]
    fn disabled_settings_yield_no_proxy() {
        let mut s = sample_settings();
        s.outbound_proxy_enabled = false;
        s.outbound_proxy_url = Some("http://127.0.0.1:7890".into());
        assert!(proxy_from_settings(&s).unwrap().is_none());
    }

    #[test]
    fn enabled_settings_build_proxy() {
        let mut s = sample_settings();
        s.outbound_proxy_enabled = true;
        s.outbound_proxy_url = Some("http://127.0.0.1:7890".into());
        assert!(proxy_from_settings(&s).unwrap().is_some());
    }

    fn sample_settings() -> GatewaySettings {
        GatewaySettings {
            id: 1,
            host: "127.0.0.1".into(),
            port: 9090,
            active_provider_id: None,
            input_protocol: "responses".into(),
            output_protocol: "chat".into(),
            auto_start: false,
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
            updated_at: "now".into(),
        }
    }
}
