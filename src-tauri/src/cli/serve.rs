//! Headless MuxLayer server — runs without Tauri/GUI.
//!
//! Usage:
//!   agentgate-serve                          # defaults: 127.0.0.1:9090
//!   agentgate-serve --port 8080              # custom port
//!   agentgate-serve --host 0.0.0.0 --port 80 # bind all interfaces
//!   AGENTGATE_PORT=8080 agentgate-serve      # env var config
//!
//! Environment variables:
//!   AGENTGATE_HOST     — bind address (default: 127.0.0.1)
//!   AGENTGATE_PORT     — port (default: 9090)
//!   MUXLAYER_DB_PATH   — SQLite database directory
//!   AGENTGATE_DB_PATH  — legacy alias for MUXLAYER_DB_PATH

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

use agentgate_lib::storage::db::{DbConn, DbPool};

#[derive(Parser)]
#[command(
    name = "agentgate",
    about = "MuxLayer — The local model control layer for coding agents",
    version
)]
struct Cli {
    /// Database directory path
    #[arg(long, global = true)]
    db_path: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the gateway server
    Serve {
        /// Host to bind to
        #[arg(long, env = "AGENTGATE_HOST", default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on
        #[arg(long, short, env = "AGENTGATE_PORT", default_value = "9090")]
        port: u16,
        /// PEM-encoded TLS certificate file path. Provide both --tls-cert and --tls-key to enable HTTPS.
        #[arg(long, env = "AGENTGATE_TLS_CERT")]
        tls_cert: Option<PathBuf>,
        /// PEM-encoded TLS private key file path.
        #[arg(long, env = "AGENTGATE_TLS_KEY")]
        tls_key: Option<PathBuf>,
    },
    /// Add a provider
    #[command(name = "provider-add")]
    ProviderAdd {
        /// Provider type (deepseek, openai, anthropic, kimi, minimax, groq, etc.)
        #[arg(long, short = 't')]
        r#type: String,
        /// Display name
        #[arg(long, short)]
        name: Option<String>,
        /// API key. Use `-` to read it from stdin; when omitted it is read from
        /// MUXLAYER_PROVIDER_API_KEY (legacy: AGENTGATE_API_KEY). Passing the
        /// literal key here leaves it in shell history / process listings.
        #[arg(long, short = 'k')]
        api_key: Option<String>,
        /// Base URL (auto-filled from type if omitted)
        #[arg(long)]
        base_url: Option<String>,
        /// Default model (auto-filled from type if omitted)
        #[arg(long, short)]
        model: Option<String>,
        /// Set as active provider
        #[arg(long, default_value = "true")]
        active: bool,
    },
    /// List all providers
    #[command(name = "provider-list")]
    ProviderList,
    /// Remove a provider by name
    #[command(name = "provider-remove")]
    ProviderRemove {
        /// Provider name to remove
        name: String,
    },
    /// Show the gateway access token
    Token,
    /// Regenerate the gateway access token
    #[command(name = "token-regen")]
    TokenRegen,
    /// Show gateway status and config summary
    Status,
    /// Update an existing provider's fields
    #[command(name = "provider-update")]
    ProviderUpdate {
        /// Provider name to update
        name: String,
        /// New API key (`-` reads it from stdin, avoiding shell history)
        #[arg(long, short = 'k')]
        api_key: Option<String>,
        /// New base URL
        #[arg(long)]
        base_url: Option<String>,
        /// New default model
        #[arg(long, short)]
        model: Option<String>,
        /// New Anthropic Messages endpoint (for providers with native Anthropic support)
        #[arg(long)]
        anthropic_url: Option<String>,
        /// New Responses endpoint (for providers with native Responses support)
        #[arg(long)]
        responses_url: Option<String>,
        /// Enable / disable
        #[arg(long)]
        enabled: Option<bool>,
    },
    /// Set a provider as the active one
    #[command(name = "provider-set-active")]
    ProviderSetActive {
        /// Provider name
        name: String,
    },
    /// Add a model mapping: client-side model name → upstream model name
    #[command(name = "mapping-add")]
    MappingAdd {
        /// Provider name
        provider: String,
        /// Client model name (what your agent sends)
        #[arg(long)]
        from: String,
        /// Upstream model name (what MuxLayer forwards)
        #[arg(long)]
        to: String,
    },
    /// List model mappings
    #[command(name = "mapping-list")]
    MappingList {
        /// Filter by provider name
        #[arg(long)]
        provider: Option<String>,
    },
    /// Remove a model mapping
    #[command(name = "mapping-remove")]
    MappingRemove {
        /// Provider name
        provider: String,
        /// Client model name to unmap
        #[arg(long)]
        from: String,
    },
    /// Tail recent request logs (most-recent first)
    Logs {
        /// Max rows to show
        #[arg(long, short = 'n', default_value = "20")]
        limit: i64,
        /// Filter by client (codex / claude-code / gemini-cli / ...)
        #[arg(long)]
        client: Option<String>,
        /// Filter by provider name
        #[arg(long)]
        provider: Option<String>,
        /// Substring search in request_id / error_message / model / route
        #[arg(long, short = 'q')]
        keyword: Option<String>,
        /// Only show errors (status >= 400 or error_message set)
        #[arg(long)]
        errors_only: bool,
    },
    /// Show token / cost / success stats over recent days
    Stats {
        /// Days of history to roll up (default 7)
        #[arg(long, short, default_value = "7")]
        days: i64,
    },
    /// Ping each enabled provider once and report health. Suitable for cron.
    #[command(name = "health-check")]
    HealthCheck {
        /// Per-provider HTTP timeout seconds
        #[arg(long, default_value = "10")]
        timeout: u64,
        /// Mark unhealthy providers in DB (sets cooldown)
        #[arg(long)]
        mark: bool,
    },
}

fn get_db_dir(cli: &Cli) -> PathBuf {
    if let Some(ref path) = cli.db_path {
        PathBuf::from(path)
    } else if let Some(path) =
        agentgate_lib::compat::env_value("MUXLAYER_DB_PATH", "AGENTGATE_DB_PATH")
    {
        PathBuf::from(path)
    } else {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        agentgate_lib::compat::default_cli_data_dir(Path::new(&home))
    }
}

fn open_db(cli: &Cli) -> DbPool {
    let db_dir = get_db_dir(cli);
    match agentgate_lib::storage::db::init_database(&db_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to initialize database: {}", e.message);
            std::process::exit(1);
        }
    }
}

/// 一次性子命令用:从池借一条连接。`PooledConnection` 自带池的 Arc 句柄,
/// 局部 pool 变量析构后连接依然有效。
fn open_conn(cli: &Cli) -> DbConn {
    match open_db(cli).get() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to acquire database connection: {e}");
            std::process::exit(1);
        }
    }
}

/// 声明式配置:设了 `AGENTGATE_CONFIG_FILE` 且库里还没有任何 provider 时,从该
/// JSON(导出的配置)导入一份。已有数据则跳过、绝不覆盖——避免重启清掉运行期改动。
/// 失败只告警不致命:配置文件坏不该挡住网关启动。
fn seed_config_if_empty(db: &DbPool, path: &str) {
    let mut conn = match db.get() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config seed: cannot acquire DB connection: {e}");
            return;
        }
    };
    match agentgate_lib::storage::providers::list_all(&conn) {
        Ok(existing) if !existing.is_empty() => {
            eprintln!(
                "Config seed: {} provider(s) already present, skipping {path}",
                existing.len()
            );
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("Config seed: cannot read providers: {}", e.message);
            return;
        }
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Config seed: cannot read {path}: {e}");
            return;
        }
    };
    let payload: agentgate_lib::storage::config_backups::ConfigExport =
        match serde_json::from_str(&raw) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Config seed: invalid config JSON in {path}: {e}");
                return;
            }
        };
    match agentgate_lib::storage::config_backups::import(&mut conn, &payload) {
        Ok(_) => eprintln!("Config seed: imported configuration from {path}"),
        Err(e) => eprintln!("Config seed: import failed: {}", e.message),
    }
}

/// Provider type presets: (base_url, default_model)
fn provider_presets() -> std::collections::HashMap<&'static str, (&'static str, &'static str)> {
    [
        (
            "deepseek",
            ("https://api.deepseek.com", "deepseek-v4-flash"),
        ),
        ("openai", ("https://api.openai.com", "gpt-5.5")),
        (
            "anthropic",
            ("https://api.anthropic.com", "claude-sonnet-4-7"),
        ),
        ("kimi", ("https://api.moonshot.cn", "kimi-k3")),
        ("minimax", ("https://api.minimax.chat", "MiniMax-M1")),
        (
            "groq",
            ("https://api.groq.com/openai", "llama-3.3-70b-versatile"),
        ),
        (
            "together",
            (
                "https://api.together.xyz",
                "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            ),
        ),
        (
            "google_gemini",
            (
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "gemini-2.5-flash",
            ),
        ),
        ("xai", ("https://api.x.ai", "grok-3-latest")),
        (
            "mistral",
            ("https://api.mistral.ai", "mistral-large-latest"),
        ),
    ]
    .into_iter()
    .collect()
}

/// 初始化结构化日志（JSON to stdout）。AGENTGATE_LOG env-filter 控制级别，
/// 默认 info（含 reqwest=warn，避免每次请求一条 hyper trace）。
/// 输出 schema: {timestamp, level, target, fields, message}。
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("AGENTGATE_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,reqwest=warn,hyper=warn,h2=warn,rustls=warn"));
    fmt()
        .with_env_filter(filter)
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_target(true)
        .flatten_event(true)
        .init();
}

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Serve {
            host,
            port,
            tls_cert,
            tls_key,
        }) => cmd_serve(&cli, host, *port, tls_cert.clone(), tls_key.clone()).await,
        Some(Commands::ProviderAdd {
            r#type,
            name,
            api_key,
            base_url,
            model,
            active,
        }) => {
            let api_key = resolve_provider_api_key(api_key.as_deref(), true);
            cmd_provider_add(
                &cli,
                r#type,
                name.as_deref(),
                &api_key,
                base_url.as_deref(),
                model.as_deref(),
                *active,
            );
        }
        Some(Commands::ProviderList) => cmd_provider_list(&cli),
        Some(Commands::ProviderRemove { name }) => cmd_provider_remove(&cli, name),
        Some(Commands::Token) => cmd_token(),
        Some(Commands::TokenRegen) => cmd_token_regen(),
        Some(Commands::Status) => cmd_status(&cli),
        Some(Commands::ProviderUpdate {
            name,
            api_key,
            base_url,
            model,
            anthropic_url,
            responses_url,
            enabled,
        }) => {
            // update 不读环境变量:没传 --api-key 就表示不改 key。
            let api_key = api_key
                .as_deref()
                .map(|v| resolve_provider_api_key(Some(v), false));
            cmd_provider_update(
                &cli,
                name,
                api_key.as_deref(),
                base_url.as_deref(),
                model.as_deref(),
                anthropic_url.as_deref(),
                responses_url.as_deref(),
                *enabled,
            );
        }
        Some(Commands::ProviderSetActive { name }) => cmd_provider_set_active(&cli, name),
        Some(Commands::MappingAdd { provider, from, to }) => {
            cmd_mapping_add(&cli, provider, from, to)
        }
        Some(Commands::MappingList { provider }) => cmd_mapping_list(&cli, provider.as_deref()),
        Some(Commands::MappingRemove { provider, from }) => {
            cmd_mapping_remove(&cli, provider, from)
        }
        Some(Commands::Logs {
            limit,
            client,
            provider,
            keyword,
            errors_only,
        }) => {
            cmd_logs(
                &cli,
                *limit,
                client.as_deref(),
                provider.as_deref(),
                keyword.as_deref(),
                *errors_only,
            );
        }
        Some(Commands::Stats { days }) => cmd_stats(&cli, *days),
        Some(Commands::HealthCheck { timeout, mark }) => {
            cmd_health_check(&cli, *timeout, *mark).await
        }
        None => {
            // Default: serve. env-based TLS（用户不传子命令 + 仅靠 env 配置 TLS）。
            let host = agentgate_lib::compat::env_value("MUXLAYER_HOST", "AGENTGATE_HOST")
                .unwrap_or_else(|| "127.0.0.1".to_string());
            let port = agentgate_lib::compat::env_value("MUXLAYER_PORT", "AGENTGATE_PORT")
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(9090);
            let tls_cert =
                agentgate_lib::compat::env_value("MUXLAYER_TLS_CERT", "AGENTGATE_TLS_CERT")
                    .map(PathBuf::from);
            let tls_key = agentgate_lib::compat::env_value("MUXLAYER_TLS_KEY", "AGENTGATE_TLS_KEY")
                .map(PathBuf::from);
            cmd_serve(&cli, &host, port, tls_cert, tls_key).await;
        }
    }
}

/// 解析 provider API key(规则见 `security::cli_exposure::resolve_api_key`);
/// `use_env` 为 false 时不读环境变量。命令行明文传入时告警,解析失败直接退出。
fn resolve_provider_api_key(cli_value: Option<&str>, use_env: bool) -> String {
    use agentgate_lib::security::cli_exposure as exposure;
    let env_value = if use_env {
        agentgate_lib::compat::env_value(
            exposure::PROVIDER_API_KEY_ENV,
            exposure::LEGACY_PROVIDER_API_KEY_ENV,
        )
    } else {
        None
    };
    let read_stdin = || {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).map(|_| buf)
    };
    match exposure::resolve_api_key(cli_value, env_value, read_stdin) {
        Ok((key, source)) => {
            if source == exposure::ApiKeySource::CommandLine {
                eprintln!("{}", exposure::command_line_key_warning());
                tracing::warn!("provider API key passed on the command line");
            }
            key
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_serve(
    cli: &Cli,
    host: &str,
    port: u16,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
) {
    let db_dir = get_db_dir(cli);
    let db = open_db(cli);

    // 历史 NULL cost 回填放阻塞线程池后台跑,不挡启动;幂等,每次启动重试直到补完。
    {
        let db = db.clone();
        tokio::task::spawn_blocking(move || {
            let result = db.get().map_err(|e| e.to_string()).and_then(|conn| {
                agentgate_lib::storage::pricing::backfill_costs(&conn)
                    .map_err(|e| format!("{} {}", e.message, e.detail.unwrap_or_default()))
            });
            match result {
                Ok(n) if n > 0 => eprintln!("[cost-backfill] Filled cost for {n} request logs"),
                Ok(_) => {}
                Err(e) => eprintln!("[cost-backfill] backfill failed: {e}"),
            }
        });
    }

    // 声明式部署:AGENTGATE_CONFIG_FILE 指向一份导出的配置,库为空时种子化。
    if let Some(path) =
        agentgate_lib::compat::env_value("MUXLAYER_CONFIG_FILE", "AGENTGATE_CONFIG_FILE")
    {
        if !path.trim().is_empty() {
            seed_config_if_empty(&db, &path);
        }
    }

    let _ = agentgate_lib::security::local_token::ensure_token();
    let token = agentgate_lib::security::local_token::read_token().unwrap_or_default();

    let provider_count = match db.get() {
        Ok(c) => agentgate_lib::storage::providers::list_all(&c)
            .map(|p| p.len())
            .unwrap_or(0),
        Err(_) => 0,
    };

    // 只提供 cert 不提供 key（或反之）直接报错——避免用户以为 TLS 开了实际还是 HTTP。
    let tls = match (tls_cert, tls_key) {
        (Some(c), Some(k)) => Some(agentgate_lib::gateway::server::TlsConfig {
            cert_path: c,
            key_path: k,
        }),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            eprintln!("Error: --tls-cert and --tls-key must be provided together");
            std::process::exit(2);
        }
    };
    let scheme = if tls.is_some() { "https" } else { "http" };

    eprintln!("MuxLayer headless server");
    eprintln!("  Database:   {}", db_dir.display());
    eprintln!(
        "  Providers:  {}",
        if provider_count > 0 {
            format!("{provider_count} configured")
        } else {
            "none (use `agentgate provider-add` to configure)".to_string()
        }
    );
    eprintln!(
        "  Token:      {}...{}",
        &token[..8.min(token.len())],
        &token[token.len().saturating_sub(4)..]
    );
    if tls.is_some() {
        eprintln!("  TLS:        enabled (HTTPS)");
    }
    eprintln!();
    if let Some(warning) =
        agentgate_lib::security::cli_exposure::cleartext_bind_warning(host, tls.is_some())
    {
        tracing::warn!(host, "{warning}");
        eprintln!("!!! WARNING: {warning}");
        eprintln!();
    }

    // SIGHUP 监听：收到信号清空内存缓存（session_affinity + provider runtime
    // status），DB 里的 provider 配置每次请求都即时读，本来就热的。
    #[cfg(unix)]
    install_sighup_handler(db.clone());

    let wake = agentgate_lib::wake::WakeManager::new();
    if let Ok(conn) = db.get() {
        if let Ok(settings) = agentgate_lib::storage::gateway_settings::get(&conn) {
            wake.set_config(agentgate_lib::wake::WakeConfig {
                enabled: settings.wake_enabled,
                request_control: settings.wake_request_control,
                cooldown_seconds: settings.wake_cooldown_seconds.clamp(0, 86_400) as u64,
                keep_display_awake: settings.wake_keep_display_awake,
            });
        }
    }
    wake.start();
    {
        let wake = wake.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                wake.tick();
            }
        });
    }
    match agentgate_lib::gateway::server::start(host, port, db, tls, wake.clone()).await {
        Ok((shutdown_tx, handle, _active_requests, _port)) => {
            eprintln!("Gateway running on {scheme}://{host}:{port}");
            eprintln!("Send SIGINT (Ctrl+C) or SIGTERM to stop. SIGHUP to reload caches.");
            eprintln!();
            wait_shutdown_signal().await;
            eprintln!("\nShutting down (graceful, up to 30s)...");
            tracing::info!("shutdown signal received, draining in-flight requests");
            let _ = shutdown_tx.send(());
            let _ = handle.await;
            wake.shutdown();
            tracing::info!("server stopped");
        }
        Err(e) => {
            eprintln!("Failed to start: {}", e.message);
            if let Some(ref s) = e.suggestion {
                eprintln!("  {s}");
            }
            std::process::exit(1);
        }
    }
}

/// SIGHUP 热重载：清 session_affinity 内存缓存 + DB 里 provider_runtime_status
/// 全部重置为 healthy。背景循环 task，永不退出（直到进程死）。
/// provider 配置（base_url/api_key/model_mapping 等）本来每次请求查 DB 已经
/// 即时生效，所以 SIGHUP 不需要触发 DB-side reload。
#[cfg(unix)]
fn install_sighup_handler(db: DbPool) {
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut hup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to install SIGHUP handler — hot reload disabled");
                return;
            }
        };
        loop {
            hup.recv().await;
            tracing::info!("SIGHUP received, clearing runtime caches");
            agentgate_lib::gateway::session_affinity::clear();
            if let Ok(c) = db.get() {
                if let Err(e) = agentgate_lib::storage::provider_runtime_status::reset_all(&c) {
                    tracing::warn!(error = %e.message, "SIGHUP: reset_all failed");
                }
            }
            tracing::info!("SIGHUP reload complete");
        }
    });
}

/// 监听 shutdown 信号：unix 同时收 SIGINT + SIGTERM，window 走 ctrl-c。
/// 任一信号到达就 return。
async fn wait_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to install SIGTERM handler: {e}");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to install SIGINT handler: {e}");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM received"),
            _ = int.recv() => tracing::info!("SIGINT received"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Ctrl-C received");
    }
}

fn cmd_provider_add(
    cli: &Cli,
    provider_type: &str,
    name: Option<&str>,
    api_key: &str,
    base_url: Option<&str>,
    model: Option<&str>,
    active: bool,
) {
    let conn = open_conn(cli);
    let presets = provider_presets();
    let (default_url, default_model) = presets.get(provider_type).copied().unwrap_or(("", ""));

    let base_url = base_url.unwrap_or(default_url);
    let model = model.unwrap_or(default_model);

    if base_url.is_empty() {
        eprintln!("Error: --base-url required for provider type '{provider_type}'");
        std::process::exit(1);
    }
    if model.is_empty() {
        eprintln!("Error: --model required for provider type '{provider_type}'");
        std::process::exit(1);
    }

    let label = if let Some(n) = name {
        n.to_string()
    } else {
        let mut s = provider_type[..1].to_uppercase();
        s.push_str(&provider_type[1..]);
        s
    };

    let mut input = agentgate_lib::models::provider::CreateProviderInput {
        name: label.clone(),
        provider_type: provider_type.to_string(),
        base_url: base_url.to_string(),
        api_key: Some(api_key.to_string()),
        default_model: model.to_string(),
        reasoning_model: None,
        supported_models: None,
        model_mapping: None,
        extra_headers: None,
        anthropic_base_url: None,
        responses_base_url: None,
        auto_cache_control: None,
        model_capabilities: None,
        provider_quirks: None,
        body_filter_enabled: None,
        thinking_rectifier_enabled: None,
        error_mapper_enabled: None,
        model_degradation_chain: None,
        model_context_windows: None,
        protocol: "openai_chat_completions".to_string(),
        timeout_seconds: Some(120),
        enabled: Some(true),
    };
    agentgate_lib::storage::recommended_mappings::apply_to_create_input(&mut input);

    match agentgate_lib::storage::providers::create(&conn, input) {
        Ok(p) => {
            if active {
                if let Err(e) = agentgate_lib::storage::providers::set_active(&conn, &p.id) {
                    eprintln!("Provider created, but activation failed: {}", e.message);
                    std::process::exit(1);
                }
            }
            println!(
                "✓ Provider '{}' created (type: {}, model: {}, active: {})",
                label, provider_type, model, active
            );
        }
        Err(e) => {
            eprintln!("Failed to create provider: {}", e.message);
            std::process::exit(1);
        }
    }
}

fn cmd_provider_list(cli: &Cli) {
    let conn = open_conn(cli);
    let providers = agentgate_lib::storage::providers::list_all(&conn).unwrap_or_default();

    if providers.is_empty() {
        println!("No providers configured. Use `agentgate provider-add` to add one.");
        return;
    }

    println!(
        "{:<4} {:<20} {:<15} {:<35} {:<25} {}",
        "#", "Name", "Type", "Base URL", "Model", "Status"
    );
    println!("{}", "-".repeat(110));
    for (i, p) in providers.iter().enumerate() {
        let active = if p.is_active { " *" } else { "" };
        let key_status = if p.api_key.as_ref().map_or(false, |k| !k.is_empty()) {
            "✓ key"
        } else {
            "✗ no key"
        };
        println!(
            "{:<4} {:<20} {:<15} {:<35} {:<25} {}{}",
            i + 1,
            &p.name[..p.name.len().min(18)],
            &p.provider_type[..p.provider_type.len().min(13)],
            &p.base_url[..p.base_url.len().min(33)],
            &p.default_model[..p.default_model.len().min(23)],
            key_status,
            active,
        );
    }
    println!("\n  * = active provider");
}

fn cmd_provider_remove(cli: &Cli, name: &str) {
    let conn = open_conn(cli);
    let providers = agentgate_lib::storage::providers::list_all(&conn).unwrap_or_default();
    let target = providers.iter().find(|p| p.name.eq_ignore_ascii_case(name));

    match target {
        Some(p) => match agentgate_lib::storage::providers::delete(&conn, &p.id) {
            Ok(_) => println!("✓ Provider '{}' removed", p.name),
            Err(e) => {
                eprintln!("Failed: {}", e.message);
                std::process::exit(1);
            }
        },
        None => {
            eprintln!(
                "Provider '{}' not found. Use `agentgate provider-list` to see all.",
                name
            );
            std::process::exit(1);
        }
    }
}

fn cmd_token() {
    let _ = agentgate_lib::security::local_token::ensure_token();
    match agentgate_lib::security::local_token::read_token() {
        Ok(token) => println!("{token}"),
        Err(e) => {
            eprintln!("Failed to read token: {}", e.message);
            std::process::exit(1);
        }
    }
}

fn cmd_token_regen() {
    match agentgate_lib::security::local_token::regenerate_token() {
        Ok(token) => println!("✓ New token: {token}"),
        Err(e) => {
            eprintln!("Failed: {}", e.message);
            std::process::exit(1);
        }
    }
}

fn cmd_status(cli: &Cli) {
    let db_dir = get_db_dir(cli);
    let conn = open_conn(cli);
    let providers = agentgate_lib::storage::providers::list_all(&conn).unwrap_or_default();
    let active = providers.iter().find(|p| p.is_active);
    let token = agentgate_lib::security::local_token::read_token().unwrap_or_default();

    println!("MuxLayer Status");
    println!("  Database:   {}", db_dir.display());
    println!("  Providers:  {} configured", providers.len());
    if let Some(a) = active {
        println!(
            "  Active:     {} ({} → {})",
            a.name, a.provider_type, a.default_model
        );
    }
    println!(
        "  Token:      {}...{}",
        &token[..8.min(token.len())],
        &token[token.len().saturating_sub(4)..]
    );
}

fn cmd_provider_update(
    cli: &Cli,
    name: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
    model: Option<&str>,
    anthropic_url: Option<&str>,
    responses_url: Option<&str>,
    enabled: Option<bool>,
) {
    let conn = open_conn(cli);
    let target = match find_provider_by_name(&conn, name) {
        Some(p) => p,
        None => {
            eprintln!("No provider named '{name}'");
            std::process::exit(1);
        }
    };
    let input = agentgate_lib::models::provider::UpdateProviderInput {
        name: None,
        provider_type: None,
        base_url: base_url.map(String::from),
        api_key: api_key.map(String::from),
        default_model: model.map(String::from),
        reasoning_model: None,
        supported_models: None,
        model_mapping: None,
        extra_headers: None,
        anthropic_base_url: anthropic_url.map(String::from),
        responses_base_url: responses_url.map(String::from),
        auto_cache_control: None,
        model_capabilities: None,
        provider_quirks: None,
        body_filter_enabled: None,
        thinking_rectifier_enabled: None,
        error_mapper_enabled: None,
        model_degradation_chain: None,
        model_context_windows: None,
        protocol: None,
        timeout_seconds: None,
        enabled,
    };
    match agentgate_lib::storage::providers::update(&conn, &target.id, input) {
        Ok(_) => println!("Updated provider '{name}'."),
        Err(e) => {
            eprintln!("Update failed: {}", e.message);
            std::process::exit(1);
        }
    }
}

fn cmd_provider_set_active(cli: &Cli, name: &str) {
    let conn = open_conn(cli);
    let target = match find_provider_by_name(&conn, name) {
        Some(p) => p,
        None => {
            eprintln!("No provider named '{name}'");
            std::process::exit(1);
        }
    };
    match agentgate_lib::storage::providers::set_active(&conn, &target.id) {
        Ok(_) => println!("Activated provider '{name}'."),
        Err(e) => {
            eprintln!("Activate failed: {}", e.message);
            std::process::exit(1);
        }
    }
}

fn cmd_mapping_add(cli: &Cli, provider_name: &str, from: &str, to: &str) {
    let conn = open_conn(cli);
    let target = match find_provider_by_name(&conn, provider_name) {
        Some(p) => p,
        None => {
            eprintln!("No provider named '{provider_name}'");
            std::process::exit(1);
        }
    };
    let mut map = parse_mapping(target.model_mapping.as_deref());
    map.insert(from.to_string(), to.to_string());
    let new_json = serde_json::to_string(&map).unwrap_or_else(|_| "{}".into());
    let input = update_with_mapping(new_json);
    match agentgate_lib::storage::providers::update(&conn, &target.id, input) {
        Ok(_) => println!("Mapped {from} → {to} on '{provider_name}'."),
        Err(e) => {
            eprintln!("Mapping update failed: {}", e.message);
            std::process::exit(1);
        }
    }
}

fn cmd_mapping_list(cli: &Cli, provider_filter: Option<&str>) {
    let conn = open_conn(cli);
    let providers = agentgate_lib::storage::providers::list_all(&conn).unwrap_or_default();
    let mut printed = 0usize;
    for p in &providers {
        if let Some(f) = provider_filter {
            if p.name != f {
                continue;
            }
        }
        let map = parse_mapping(p.model_mapping.as_deref());
        if map.is_empty() {
            continue;
        }
        println!("[{}]", p.name);
        // 排序输出，确定性
        let mut entries: Vec<_> = map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (from, to) in entries {
            println!("  {from} → {to}");
        }
        printed += 1;
    }
    if printed == 0 {
        println!("(no mappings)");
    }
}

fn cmd_mapping_remove(cli: &Cli, provider_name: &str, from: &str) {
    let conn = open_conn(cli);
    let target = match find_provider_by_name(&conn, provider_name) {
        Some(p) => p,
        None => {
            eprintln!("No provider named '{provider_name}'");
            std::process::exit(1);
        }
    };
    let mut map = parse_mapping(target.model_mapping.as_deref());
    if map.remove(from).is_none() {
        eprintln!("No mapping for '{from}' on '{provider_name}'");
        std::process::exit(1);
    }
    let new_json = serde_json::to_string(&map).unwrap_or_else(|_| "{}".into());
    let input = update_with_mapping(new_json);
    match agentgate_lib::storage::providers::update(&conn, &target.id, input) {
        Ok(_) => println!("Removed mapping '{from}' from '{provider_name}'."),
        Err(e) => {
            eprintln!("Mapping update failed: {}", e.message);
            std::process::exit(1);
        }
    }
}

fn find_provider_by_name(
    conn: &rusqlite::Connection,
    name: &str,
) -> Option<agentgate_lib::models::provider::Provider> {
    agentgate_lib::storage::providers::list_all(conn)
        .ok()?
        .into_iter()
        .find(|p| p.name == name)
}

fn parse_mapping(json: Option<&str>) -> std::collections::BTreeMap<String, String> {
    json.and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

fn update_with_mapping(json: String) -> agentgate_lib::models::provider::UpdateProviderInput {
    agentgate_lib::models::provider::UpdateProviderInput {
        name: None,
        provider_type: None,
        base_url: None,
        api_key: None,
        default_model: None,
        reasoning_model: None,
        supported_models: None,
        model_mapping: Some(json),
        extra_headers: None,
        anthropic_base_url: None,
        responses_base_url: None,
        auto_cache_control: None,
        model_capabilities: None,
        provider_quirks: None,
        body_filter_enabled: None,
        thinking_rectifier_enabled: None,
        error_mapper_enabled: None,
        model_degradation_chain: None,
        model_context_windows: None,
        protocol: None,
        timeout_seconds: None,
        enabled: None,
    }
}

fn cmd_logs(
    cli: &Cli,
    limit: i64,
    client: Option<&str>,
    provider: Option<&str>,
    keyword: Option<&str>,
    errors_only: bool,
) {
    let conn = open_conn(cli);
    let filter = agentgate_lib::models::request_log::RequestLogFilter {
        client: client.map(String::from),
        provider: provider.map(String::from),
        model: None,
        route_profile_id: None,
        status: if errors_only {
            Some("error".into())
        } else {
            None
        },
        error_type: None,
        keyword: keyword.map(String::from),
        source: None,
        session_id: None,
        limit: Some(limit),
        offset: None,
    };
    let logs = match agentgate_lib::storage::request_logs::list(&conn, filter) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to list logs: {}", e.message);
            std::process::exit(1);
        }
    };

    if logs.is_empty() {
        println!("(no logs match)");
        return;
    }

    // 列：time / client / provider / model / route / status / latency
    println!(
        "{:<25} {:<14} {:<14} {:<22} {:<25} {:<7} {:>8}",
        "TIMESTAMP", "CLIENT", "PROVIDER", "MODEL", "ROUTE", "STATUS", "LATENCY"
    );
    for log in &logs {
        let ts = log.timestamp.chars().take(19).collect::<String>();
        let client = log.client.as_deref().unwrap_or("-");
        let provider = log.provider.as_deref().unwrap_or("-");
        let model = log.model.as_deref().unwrap_or("-");
        let route = log.route.as_deref().unwrap_or("-");
        let status = log
            .status_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".into());
        let latency = log
            .latency_ms
            .map(|l| format!("{l}ms"))
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<25} {:<14} {:<14} {:<22} {:<25} {:<7} {:>8}",
            truncate(&ts, 25),
            truncate(client, 14),
            truncate(provider, 14),
            truncate(model, 22),
            truncate(route, 25),
            status,
            latency
        );
        if let Some(ref err) = log.error_message {
            println!("  ⚠️  {}", truncate(err, 100));
        }
    }
    println!("\n({} rows)", logs.len());
}

fn cmd_stats(cli: &Cli, days: i64) {
    let conn = open_conn(cli);
    let stats = match agentgate_lib::storage::request_logs::get_stats_for_range(&conn, days) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to compute stats: {}", e.message);
            std::process::exit(1);
        }
    };

    println!(
        "MuxLayer Stats (last {days} day{}, plus all-time)\n",
        if days == 1 { "" } else { "s" }
    );

    println!("All-time:");
    println!("  Requests:        {}", stats.total);
    println!(
        "  Success:         {} ({:.1}%)",
        stats.success,
        stats.success_rate * 100.0
    );
    println!("  Errors:          {}", stats.errors);
    println!("  Avg latency:     {}ms", stats.avg_latency_ms);
    println!("  Input tokens:    {}", stats.total_input_tokens);
    println!("  Output tokens:   {}", stats.total_output_tokens);
    println!(
        "  Cache R/W tokens:{} / {}",
        stats.total_cache_read_tokens, stats.total_cache_write_tokens
    );
    println!("  Cost:            ${:.4}", stats.total_cost);

    println!("\nToday:");
    println!("  Requests:        {}", stats.today_total);
    println!("  Errors:          {}", stats.today_errors);
    println!("  Input tokens:    {}", stats.today_input_tokens);
    println!("  Output tokens:   {}", stats.today_output_tokens);
    println!("  Cost:            ${:.4}", stats.today_cost);

    if !stats.daily.is_empty() {
        println!("\nDaily breakdown:");
        println!(
            "  {:<12} {:>8} {:>8} {:>10} {:>10} {:>10}",
            "DATE", "TOTAL", "ERRORS", "IN_TOK", "OUT_TOK", "COST"
        );
        for d in &stats.daily {
            println!(
                "  {:<12} {:>8} {:>8} {:>10} {:>10} ${:>9.4}",
                d.date, d.total, d.errors, d.input_tokens, d.output_tokens, d.cost
            );
        }
    }

    if !stats.providers.is_empty() {
        println!("\nBy provider:");
        for p in &stats.providers {
            println!("  {:<25} {:>8}", truncate(&p.name, 25), p.count);
        }
    }
}

async fn cmd_health_check(cli: &Cli, timeout_secs: u64, mark: bool) {
    let conn = open_conn(cli);
    let providers = agentgate_lib::storage::providers::list_all(&conn).unwrap_or_default();
    let enabled: Vec<_> = providers.into_iter().filter(|p| p.enabled).collect();

    if enabled.is_empty() {
        println!("(no enabled providers)");
        return;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .expect("build http client");

    println!(
        "{:<25} {:<14} {:<8} {:>10}  STATUS",
        "PROVIDER", "TYPE", "REACH", "LATENCY"
    );
    let mut unhealthy_count = 0;
    for p in &enabled {
        let start = std::time::Instant::now();
        let res = client.get(p.base_url.trim_end_matches('/')).send().await;
        let latency_ms = start.elapsed().as_millis();
        let (reach, status, healthy) = match res {
            Ok(r) => {
                let code = r.status().as_u16();
                // 200-499 算 reachable（4xx 常见，比如未带 auth header 的 401）。
                // 5xx / 网络错算 unhealthy。
                let h = code < 500;
                (
                    if h { "ok" } else { "down" }.to_string(),
                    format!("HTTP {code}"),
                    h,
                )
            }
            Err(e) => {
                let kind = if e.is_timeout() {
                    "timeout"
                } else if e.is_connect() {
                    "connect-fail"
                } else {
                    "net-err"
                };
                ("down".to_string(), kind.to_string(), false)
            }
        };
        println!(
            "  {:<23} {:<14} {:<8} {:>8}ms  {}",
            truncate(&p.name, 23),
            truncate(&p.provider_type, 14),
            reach,
            latency_ms,
            status
        );

        if mark {
            let result = if healthy {
                agentgate_lib::storage::provider_runtime_status::mark_success(&conn, &p.id)
            } else {
                agentgate_lib::storage::provider_runtime_status::mark_failure(
                    &conn,
                    &p.id,
                    "HEALTH_CHECK_FAILED",
                    &status,
                    300,
                )
            };
            if let Err(e) = result {
                eprintln!("    (DB update failed: {})", e.message);
            }
        }
        if !healthy {
            unhealthy_count += 1;
        }
    }

    println!(
        "\n{} provider(s), {} unhealthy.",
        enabled.len(),
        unhealthy_count
    );
    if unhealthy_count > 0 {
        std::process::exit(1);
    }
}

/// 字符串截断到 max（按字符，不按字节），尾部补 …
fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let mut out: String = chars[..max.saturating_sub(1)].iter().collect();
        out.push('…');
        out
    }
}
