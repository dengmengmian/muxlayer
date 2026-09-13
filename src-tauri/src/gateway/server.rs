use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use http_body::{Body as HttpBody, Frame, SizeHint};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::oneshot;

use crate::errors::AppError;
use crate::gateway::routes::{self, GatewayState};

/// TLS 配置——同时提供 cert + key 文件路径才启 HTTPS，缺一退回 HTTP。
#[derive(Clone, Debug)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

/// 优雅 shutdown 等 in-flight 完成的最大时长。SSE 长流式响应仍可能超时被切，
/// 但 30s 给"正常 chat completion + 短 SSE"留够余地。
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_REQUEST_BODY_LIMIT_MB: i64 = 32;
const MAX_REQUEST_BODY_LIMIT_MB: i64 = 128;
const BYTES_PER_MIB: usize = 1024 * 1024;

fn is_wake_managed_request(method: &axum::http::Method, path: &str) -> bool {
    if method != axum::http::Method::POST {
        return false;
    }

    matches!(
        path,
        "/v1/responses"
            | "/responses"
            | "/v1/responses/compact"
            | "/responses/compact"
            | "/v1/chat/completions"
            | "/chat/completions"
            | "/v1/messages"
            | "/messages"
    ) || (path.starts_with("/v1beta/models/")
        && (path.ends_with(":generateContent") || path.ends_with(":streamGenerateContent")))
}

/// 包一层 Body，使 active-requests 计数器在 **body 真正流完**（或被
/// drop——客户端断开）时才 decrement。
///
/// 修复的 bug：axum 的 `middleware::from_fn` 模式里 `next.run(req).await`
/// 在 response **headers 发完**就返回，**body 还没流给客户端**——对 99%
/// 是 SSE 流式请求的 AI gateway 来说，这个 await 几毫秒就完事，counter
/// 在 dashboard 3 秒一拉之前早就回 0 了，"活跃连接"永远显示 0。
///
/// 现在 increment 在 middleware 入口、decrement 在这个 wrapper 的 Drop
/// 里——body 完整流完或客户端断开后才触发，反映真实在飞的请求数。
struct CountingBody {
    inner: Body,
    counter: Arc<AtomicU64>,
    wake: Option<Arc<crate::wake::WakeManager>>,
    decremented: bool,
}

impl Drop for CountingBody {
    fn drop(&mut self) {
        if !self.decremented {
            self.counter.fetch_sub(1, Ordering::Relaxed);
            if let Some(wake) = &self.wake {
                wake.request_finished();
            }
            self.decremented = true;
        }
    }
}

impl HttpBody for CountingBody {
    type Data = bytes::Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Start the gateway HTTP/HTTPS server. Returns shutdown sender, join handle,
/// active-request counter, and the actually-bound port (useful when callers
/// pass `port=0` to let the OS pick — integration tests rely on this).
///
/// `tls`: `Some` 启 HTTPS（用 rustls 加载 cert/key），`None` 启 HTTP。
/// shutdown signal 通过 oneshot 接，内部桥接到 `axum_server::Handle`，触发
/// 后等 `GRACEFUL_SHUTDOWN_TIMEOUT` in-flight 完成再强收。
pub async fn start(
    host: &str,
    port: u16,
    db: crate::storage::db::DbPool,
    tls: Option<TlsConfig>,
    wake: Arc<crate::wake::WakeManager>,
) -> Result<
    (
        oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
        Arc<AtomicU64>,
        u16,
    ),
    AppError,
> {
    let http_client = crate::gateway::http_client::build_upstream_client_from_db(&db)?;

    let active_requests = Arc::new(AtomicU64::new(0));

    let configured_body_limit_mb = db
        .get()
        .ok()
        .and_then(|conn| crate::storage::gateway_settings::get(&conn).ok())
        .map(|settings| settings.request_body_limit_mb)
        .unwrap_or(DEFAULT_REQUEST_BODY_LIMIT_MB);
    let body_limit_mb = effective_request_body_limit_mb(configured_body_limit_mb);
    let body_limit_bytes = request_body_limit_bytes(body_limit_mb);

    let state = GatewayState {
        db,
        http_client,
        active_requests: active_requests.clone(),
        request_body_limit: body_limit_bytes,
    };

    // ── 主动延迟探测循环(喂 fastest 路由的冷启动)──
    // 默认关:探测发的是真实最小补全(speedtest::probe),会产生少量 token
    // 费用,不能静默烧钱。设 AGENTGATE_LATENCY_PROBE_MINUTES=N(分钟)开启;
    // 开启后也只在存在 selection_strategy="fastest" 的路由档位时才真正探测。
    if let Some(minutes) = crate::compat::env_value(
        "MUXLAYER_LATENCY_PROBE_MINUTES",
        "AGENTGATE_LATENCY_PROBE_MINUTES",
    )
    .and_then(|v| v.trim().parse::<u64>().ok())
    .filter(|m| *m >= 1)
    {
        let probe_db = state.db.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(minutes * 60)).await;
                let providers = {
                    let Ok(conn) = probe_db.get() else { continue };
                    let has_fastest = crate::storage::route_profiles::list_all(&conn)
                        .map(|ps| ps.iter().any(|p| p.selection_strategy == "fastest"))
                        .unwrap_or(false);
                    if !has_fastest {
                        continue;
                    }
                    crate::storage::providers::list_all(&conn)
                        .map(|ps| ps.into_iter().filter(|p| p.enabled).collect::<Vec<_>>())
                        .unwrap_or_default()
                };
                for p in &providers {
                    let report = crate::diagnostics::speedtest::probe(p).await;
                    if report.success {
                        crate::gateway::probe_latency::record(&p.name, report.total_ms as f64);
                    }
                }
            }
        });
    }

    // metrics recorder 幂等初始化（重复调 init 直接返回 false 不报错）。
    crate::gateway::metrics::init();

    let counter = active_requests.clone();
    // /metrics 只要本地 token:Docker 里 Prometheus 按服务名抓取(Host 是域名),
    // token 已挡住 DNS rebinding 读取,不再叠加 Host/Origin 边界。
    let metrics = Router::new()
        .route(
            "/metrics",
            get({
                // 渲染前同步当前 active_requests gauge —— gauge 平时由 SSE 流入流出
                // 的 CountingBody 增减，但 metrics 系统不直接访问 AtomicU64，渲染前
                // 镜像一次。
                let counter = active_requests.clone();
                move || {
                    let n = counter.load(Ordering::Relaxed);
                    crate::gateway::metrics::set_active_requests(n);
                    crate::gateway::metrics::render()
                }
            }),
        )
        .route_layer(axum::middleware::from_fn(require_metrics_token));
    // 除 /health、/metrics 外所有端点都要本地 token + Host/Origin 边界校验,并且在
    // body 提取之前完成:否则未鉴权的请求也能让网关先缓冲满 32MB 请求体。
    let protected = Router::new()
        .route("/v1/models", get(routes::list_models))
        .route("/v1/responses", post(routes::handle_responses))
        .route("/responses", post(routes::handle_responses))
        // Codex remote compaction v2:旧 Codex 把 compact 请求发到 sub-path,
        // 新版用 header,但路径还是兜底注册一下,handle_responses 入口探嗅 header / path 决定走 compact 分支。
        .route("/v1/responses/compact", post(routes::handle_responses))
        .route("/responses/compact", post(routes::handle_responses))
        .route(
            "/v1/chat/completions",
            post(routes::handle_chat_completions),
        )
        .route("/chat/completions", post(routes::handle_chat_completions))
        .route("/v1/messages", post(routes::handle_messages))
        .route("/messages", post(routes::handle_messages))
        .route(
            "/v1/messages/count_tokens",
            post(routes::handle_count_tokens),
        )
        .route("/messages/count_tokens", post(routes::handle_count_tokens))
        .route("/v1beta/models", get(routes::list_gemini_models))
        .route(
            "/v1beta/models/:model_action",
            post(routes::handle_gemini_generate),
        )
        .route_layer(axum::middleware::from_fn(require_gateway_auth));
    let app = Router::new()
        .route("/health", get(routes::health))
        .merge(metrics)
        .merge(protected)
        .layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let counter = counter.clone();
                let wake = wake.clone();
                async move {
                    let manage_wake = is_wake_managed_request(req.method(), req.uri().path());
                    counter.fetch_add(1, Ordering::Relaxed);
                    if manage_wake {
                        wake.request_started();
                    }
                    let response = next.run(req).await;
                    // 不在这 decrement —— next.run 对 streaming 响应几毫秒就返回，
                    // body 还没流给 client。包一层 CountingBody，body Drop 时才减。
                    let (parts, body) = response.into_parts();
                    let wrapped = CountingBody {
                        inner: body,
                        counter,
                        wake: manage_wake.then_some(wake),
                        decremented: false,
                    };
                    axum::http::Response::from_parts(parts, Body::new(wrapped))
                }
            },
        ))
        .layer(DefaultBodyLimit::max(body_limit_bytes))
        .with_state(state);

    // 可选 per-IP 限流(默认关)。AGENTGATE_RATE_LIMIT = 每 IP 每秒最大请求数。
    // 在请求入口计一次,不占整条 SSE 流。默认按 TCP 对端 IP 计:X-Forwarded-For /
    // X-Real-IP 客户端可随意伪造,信任它们等于限流形同虚设,还会让 IP 桶无界增长。
    // 只有明确部署在反代后面时才设 MUXLAYER_TRUST_PROXY=1 改用转发头。
    let rate = crate::compat::env_value("MUXLAYER_RATE_LIMIT", "AGENTGATE_RATE_LIMIT")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|n| *n > 0);
    let app = if let Some(r) = rate {
        use tower_governor::governor::GovernorConfigBuilder;
        use tower_governor::key_extractor::{PeerIpKeyExtractor, SmartIpKeyExtractor};
        use tower_governor::GovernorLayer;
        let key_mode = rate_limit_key_mode(crate::compat::env_flag(
            "MUXLAYER_TRUST_PROXY",
            "AGENTGATE_TRUST_PROXY",
        ));
        let period = Duration::from_nanos(1_000_000_000u64 / r as u64);
        // 两种提取器是不同类型,只能分支各建一份 config;清理任务定期回收过期 IP 桶,
        // 防止内存随不同 IP 数无界增长。
        let app = match key_mode {
            RateLimitKeyMode::PeerIp => {
                let conf = Arc::new(
                    GovernorConfigBuilder::default()
                        .period(period)
                        .burst_size(r)
                        .key_extractor(PeerIpKeyExtractor)
                        .finish()
                        .expect("build governor config"),
                );
                let limiter = conf.limiter().clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        limiter.retain_recent();
                    }
                });
                app.layer(GovernorLayer { config: conf })
            }
            RateLimitKeyMode::ForwardedHeaders => {
                let conf = Arc::new(
                    GovernorConfigBuilder::default()
                        .period(period)
                        .burst_size(r)
                        .key_extractor(SmartIpKeyExtractor)
                        .finish()
                        .expect("build governor config"),
                );
                let limiter = conf.limiter().clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        limiter.retain_recent();
                    }
                });
                app.layer(GovernorLayer { config: conf })
            }
        };
        tracing::info!(per_ip_rps = r, key = ?key_mode, "per-IP rate limiting enabled");
        app
    } else {
        app
    };

    let addr: SocketAddr = format!("{host}:{port}").parse().map_err(|e| {
        AppError::new(
            crate::errors::codes::GATEWAY_BIND_ERROR,
            format!("Invalid address: {e}"),
        )
    })?;

    // axum_server 接受 std::net::TcpListener。先用 std bind 拿到 bound_port（port=0
    // 时 OS 才分配），set_nonblocking 让 tokio runtime 能 poll。
    let std_listener = std::net::TcpListener::bind(addr).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            AppError::new(
                crate::errors::codes::GATEWAY_PORT_IN_USE,
                "Gateway port is already in use",
            )
            .with_detail(format!("{host}:{port}"))
            .with_suggestion(
                "Change the gateway port in Settings or stop the process using this port",
            )
        } else {
            AppError::new(
                crate::errors::codes::GATEWAY_BIND_ERROR,
                format!("Failed to bind: {e}"),
            )
        }
    })?;
    std_listener.set_nonblocking(true).map_err(|e| {
        AppError::new(
            crate::errors::codes::GATEWAY_BIND_ERROR,
            format!("set_nonblocking failed: {e}"),
        )
    })?;

    let bound_port = std_listener.local_addr().map(|a| a.port()).unwrap_or(port);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_handle = axum_server::Handle::new();

    // 桥接 oneshot → axum_server::Handle::graceful_shutdown。
    // shutdown_tx.send(()) 触发后，axum_server 停接新连接、等 in-flight 完成
    // 直到 GRACEFUL_SHUTDOWN_TIMEOUT 再强切——比原 oneshot+5s 强杀 SSE 友好得多。
    {
        let sh = server_handle.clone();
        tokio::spawn(async move {
            let _ = shutdown_rx.await;
            sh.graceful_shutdown(Some(GRACEFUL_SHUTDOWN_TIMEOUT));
        });
    }

    // with_connect_info:限流按 peer IP 计(信任反代时 SmartIp 也回落到 peer IP)。
    let make_service = app.into_make_service_with_connect_info::<SocketAddr>();

    tracing::info!(
        host = %host,
        port = bound_port,
        tls = tls.is_some(),
        request_body_limit_mb = body_limit_mb,
        "gateway listening"
    );

    let join_handle = if let Some(tls_cfg) = tls {
        // HTTPS：加载 cert/key，启 axum_server with TLS
        let rustls = RustlsConfig::from_pem_file(&tls_cfg.cert_path, &tls_cfg.key_path)
            .await
            .map_err(|e| {
                AppError::new(
                    crate::errors::codes::GATEWAY_TLS_LOAD_FAILED,
                    format!("Failed to load TLS cert/key: {e}"),
                )
                .with_detail(format!(
                    "cert={} key={}",
                    tls_cfg.cert_path.display(),
                    tls_cfg.key_path.display()
                ))
                .with_suggestion("Verify both files exist and are valid PEM-encoded")
            })?;
        tokio::spawn(async move {
            let _ = axum_server::from_tcp_rustls(std_listener, rustls)
                .handle(server_handle)
                .serve(make_service)
                .await;
        })
    } else {
        // HTTP
        tokio::spawn(async move {
            let _ = axum_server::from_tcp(std_listener)
                .handle(server_handle)
                .serve(make_service)
                .await;
        })
    };

    Ok((shutdown_tx, join_handle, active_requests, bound_port))
}

/// 鉴权中间件:本地 token + Host/Origin 边界校验。挂在除 /health、/metrics 以外的路由上,
/// 在 handler 提取 body 之前执行。
async fn require_gateway_auth(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(err) = routes::validate_auth(req.headers()) {
        return err.into_response();
    }
    next.run(req).await
}

/// /metrics 鉴权中间件:只校验本地 token。
async fn require_metrics_token(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Err(err) = routes::validate_token(req.headers()) {
        return err.into_response();
    }
    next.run(req).await
}

/// 限流 key 的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RateLimitKeyMode {
    /// TCP 对端 IP(默认,不可伪造)。
    PeerIp,
    /// X-Forwarded-For / X-Real-IP / Forwarded(仅在可信反代后面使用)。
    ForwardedHeaders,
}

fn rate_limit_key_mode(trust_proxy: bool) -> RateLimitKeyMode {
    if trust_proxy {
        RateLimitKeyMode::ForwardedHeaders
    } else {
        RateLimitKeyMode::PeerIp
    }
}

fn effective_request_body_limit_mb(configured_mb: i64) -> i64 {
    let configured_mb = configured_mb.clamp(1, MAX_REQUEST_BODY_LIMIT_MB);
    crate::compat::env_value(
        "MUXLAYER_REQUEST_BODY_LIMIT_MB",
        "AGENTGATE_REQUEST_BODY_LIMIT_MB",
    )
    .and_then(|v| v.trim().parse::<i64>().ok())
    .filter(|v| *v > 0)
    .map(|v| v.clamp(1, MAX_REQUEST_BODY_LIMIT_MB))
    .unwrap_or(configured_mb)
}

fn request_body_limit_bytes(limit_mb: i64) -> usize {
    (limit_mb.clamp(1, MAX_REQUEST_BODY_LIMIT_MB) as usize).saturating_mul(BYTES_PER_MIB)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn request_body_limit_prefers_valid_env() {
        let _guard = env_lock();
        std::env::set_var("AGENTGATE_REQUEST_BODY_LIMIT_MB", "4096");
        assert_eq!(effective_request_body_limit_mb(32), 128);
        std::env::remove_var("AGENTGATE_REQUEST_BODY_LIMIT_MB");
    }

    #[test]
    fn request_body_limit_ignores_invalid_env() {
        let _guard = env_lock();
        std::env::set_var("AGENTGATE_REQUEST_BODY_LIMIT_MB", "0");
        assert_eq!(effective_request_body_limit_mb(32), 32);
        std::env::set_var("AGENTGATE_REQUEST_BODY_LIMIT_MB", "abc");
        assert_eq!(effective_request_body_limit_mb(32), 32);
        std::env::remove_var("AGENTGATE_REQUEST_BODY_LIMIT_MB");
    }

    #[test]
    fn request_body_limit_clamps_configured_value() {
        let _guard = env_lock();
        std::env::remove_var("AGENTGATE_REQUEST_BODY_LIMIT_MB");
        assert_eq!(effective_request_body_limit_mb(4096), 128);
        assert_eq!(request_body_limit_bytes(4096), 128 * BYTES_PER_MIB);
    }

    #[test]
    #[serial_test::serial(env)]
    fn rate_limit_trusts_forwarded_headers_only_when_opted_in() {
        std::env::remove_var("MUXLAYER_TRUST_PROXY");
        std::env::remove_var("AGENTGATE_TRUST_PROXY");
        let from_env = || {
            rate_limit_key_mode(crate::compat::env_flag(
                "MUXLAYER_TRUST_PROXY",
                "AGENTGATE_TRUST_PROXY",
            ))
        };
        assert_eq!(from_env(), RateLimitKeyMode::PeerIp);

        std::env::set_var("AGENTGATE_TRUST_PROXY", "1");
        assert_eq!(from_env(), RateLimitKeyMode::ForwardedHeaders);
        std::env::set_var("MUXLAYER_TRUST_PROXY", "0");
        assert_eq!(
            from_env(),
            RateLimitKeyMode::PeerIp,
            "current env name wins over legacy"
        );
        std::env::remove_var("MUXLAYER_TRUST_PROXY");
        std::env::remove_var("AGENTGATE_TRUST_PROXY");
    }

    #[test]
    fn wake_management_only_tracks_generation_requests() {
        use axum::http::Method;

        for path in [
            "/v1/responses",
            "/responses",
            "/v1/responses/compact",
            "/v1/chat/completions",
            "/chat/completions",
            "/v1/messages",
            "/messages",
            "/v1beta/models/gemini-2.5-pro:generateContent",
            "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
        ] {
            assert!(
                is_wake_managed_request(&Method::POST, path),
                "expected generation path to be tracked: {path}"
            );
        }

        for path in [
            "/health",
            "/metrics",
            "/v1/models",
            "/v1/messages/count_tokens",
            "/messages/count_tokens",
            "/v1beta/models",
            "/v1beta/models/gemini-2.5-pro:countTokens",
        ] {
            assert!(
                !is_wake_managed_request(&Method::POST, path),
                "expected non-generation path to be ignored: {path}"
            );
        }
        assert!(!is_wake_managed_request(&Method::GET, "/v1/responses"));
    }

    struct TestWakeBackend;

    impl crate::wake::WakeBackend for TestWakeBackend {
        fn supported(&self) -> bool {
            true
        }

        fn platform(&self) -> &'static str {
            "test"
        }

        fn acquire(&self, _options: crate::wake::WakeOptions) -> Result<(), String> {
            Ok(())
        }

        fn release(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn dropping_response_body_finishes_the_wake_request() {
        let wake = crate::wake::WakeManager::with_backend(Arc::new(TestWakeBackend));
        wake.set_config(crate::wake::WakeConfig {
            request_control: true,
            cooldown_seconds: 15,
            ..crate::wake::WakeConfig::default()
        });
        wake.start();
        wake.request_started();

        let body = CountingBody {
            inner: Body::empty(),
            counter: Arc::new(AtomicU64::new(1)),
            wake: Some(wake.clone()),
            decremented: false,
        };
        drop(body);

        assert_eq!(wake.status().active_requests, 0);
        assert_eq!(wake.status().mode, crate::wake::WakeMode::Cooldown);
    }
}
