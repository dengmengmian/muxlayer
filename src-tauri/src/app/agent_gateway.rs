//! Desktop management layer for the existing Agent Gateway.
//!
//! This module only discovers CLIs, reads/writes the existing Agent Gateway
//! settings, talks to the existing local IPC service, and manages the official
//! CodeLeveler / Pi MCP configuration entries. It does not implement a second
//! task manager or execute coding agents itself.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::agent_gateway::{
    discovery, ipc, settings,
    settings::{CLAUDE_BIN_KEY, CODEX_BIN_KEY},
};
use crate::errors::{codes, AppError};

pub const MCP_SERVER_NAME: &str = "muxlayer";
pub const SERVE_BIN_SETTING: &str = "agent_gateway.serve_bin";
const VERSION_TIMEOUT: Duration = Duration::from_secs(6);
const CLI_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const SERVICE_START_TIMEOUT: Duration = Duration::from_secs(20);
const TEST_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayCliStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub detection: String,
    pub error: Option<String>,
    pub install_hint: String,
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayAgentStatus {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub executable: Option<String>,
    pub version: Option<String>,
    pub state: String,
    pub available: bool,
    pub auth_state: String,
    pub auth_detail: String,
    pub source: String,
    pub model: Option<String>,
    pub config_path: Option<String>,
    pub configured_path: Option<String>,
    pub detection: String,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayServiceAgent {
    pub id: String,
    pub name: String,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayServiceStatus {
    pub installed: bool,
    pub cli_path: Option<String>,
    pub cli_version: Option<String>,
    pub data_dir: String,
    pub running: bool,
    pub service_version: Option<String>,
    pub running_tasks: Vec<String>,
    pub agents: Vec<AgentGatewayServiceAgent>,
    pub last_error: Option<String>,
    pub error: Option<String>,
    pub install_hint: String,
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayOverview {
    pub cli: AgentGatewayCliStatus,
    pub data_dir: String,
    pub agents: Vec<AgentGatewayAgentStatus>,
    pub service: AgentGatewayServiceStatus,
    pub clients: Vec<AgentGatewayClientStatus>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayClientStatus {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub executable: Option<String>,
    pub config_path: String,
    pub config_exists: bool,
    pub mcp_name: String,
    pub config_status: String,
    pub configured: bool,
    pub matching: bool,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub copy_command: String,
    pub copy_args: Vec<String>,
    pub copy_json: String,
    pub copy_toml: String,
    pub compatibility: String,
    pub requires_upgrade: bool,
    pub warning: Option<String>,
    pub error: Option<String>,
    pub live_status_available: bool,
    pub live_status_note: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayPathResult {
    pub status: AgentGatewayAgentStatus,
    pub requires_service_restart: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayMcpCopy {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub json: String,
    pub toml: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayMcpResult {
    pub status: AgentGatewayClientStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct AgentGatewayTestResult {
    pub success: bool,
    pub agent: String,
    pub state: String,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub final_answer: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub started_service: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CliProbe {
    installed: bool,
    path: Option<PathBuf>,
    version: Option<String>,
    detection: String,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct McpEntry {
    command: String,
    args: Vec<String>,
}

fn internal(message: impl Into<String>) -> AppError {
    AppError::internal(message)
}

fn agent_error(error: crate::agent_gateway::AgentError) -> AppError {
    AppError::new(format!("{:?}", error.code), error.message)
}

pub fn data_dir() -> Result<PathBuf, AppError> {
    if let Some(path) = crate::compat::env_value("MUXLAYER_DB_PATH", "AGENTGATE_DB_PATH") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    Ok(crate::compat::default_cli_data_dir(
        &crate::fsutil::home_dir()?,
    ))
}

pub fn install_hint() -> String {
    "Agent Gateway CLI is not installed. Download the agentgate-serve tarball for this Mac from the official Releases page, extract it, and place agentgate-serve on PATH or set its path in Settings.".to_string()
}

pub const DOWNLOAD_URL: &str = "https://github.com/dengmengmian/muxlayer/releases";

async fn run_capture(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!("{} timed out", program.display())),
    }
}

fn text(output: &std::process::Output) -> String {
    let mut value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if value.is_empty() {
        value = stderr;
    }
    value
}

fn detection_source(configured: bool, env_key: &str, path: &Path, binary: &str) -> String {
    if configured {
        return "manual".into();
    }
    if std::env::var_os(env_key).is_some_and(|value| !value.is_empty()) {
        return "env".into();
    }
    if let Some(path_env) = std::env::var_os("PATH") {
        if std::env::split_paths(&path_env).any(|dir| dir.join(binary) == path) {
            return "path".into();
        }
    }
    "common".into()
}

async fn probe_cli(
    binary: &str,
    explicit: Option<&Path>,
    env_key: &str,
    version_prefix: &str,
) -> CliProbe {
    let configured = explicit.is_some();
    let candidates = discovery::candidate_paths(explicit, Some(env_key), binary);
    let mut first_existing = None;
    let mut last_error = None;
    for path in candidates {
        if discovery::is_app_bundle(&path) {
            continue;
        }
        if !path.is_file() {
            if explicit.is_some() {
                last_error = Some(format!(
                    "configured path does not exist: {}",
                    path.display()
                ));
            }
            continue;
        }
        first_existing.get_or_insert_with(|| path.clone());
        match run_capture(&path, &["--version"], VERSION_TIMEOUT).await {
            Ok(output) if output.status.success() => {
                let version = text(&output);
                if version.starts_with(version_prefix) || version.contains(version_prefix) {
                    return CliProbe {
                        installed: true,
                        path: Some(path.clone()),
                        version: Some(version),
                        detection: detection_source(configured, env_key, &path, binary),
                        error: None,
                    };
                }
                last_error = Some(format!(
                    "{} did not report the expected version",
                    path.display()
                ));
            }
            Ok(output) => {
                last_error = Some(format!(
                    "{} --version failed: {}",
                    path.display(),
                    text(&output)
                ));
            }
            Err(error) => last_error = Some(format!("{}: {error}", path.display())),
        }
        if explicit.is_some() {
            break;
        }
    }
    CliProbe {
        installed: first_existing.is_some(),
        path: first_existing,
        version: None,
        detection: if configured { "manual" } else { "auto" }.into(),
        error: Some(last_error.unwrap_or_else(|| format!("{binary} was not found on PATH"))),
    }
}

pub(crate) async fn probe_serve(explicit: Option<&Path>) -> CliProbe {
    probe_cli(
        "agentgate-serve",
        explicit,
        "AGENTGATE_SERVE_BIN",
        "agentgate ",
    )
    .await
}

pub(crate) fn load_serve_setting(
    db: &crate::storage::db::DbPool,
) -> Result<Option<PathBuf>, AppError> {
    let conn = db
        .get()
        .map_err(|error| internal(format!("DB connection failed: {error}")))?;
    Ok(crate::storage::app_settings::get(&conn, SERVE_BIN_SETTING)?
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from))
}

pub(crate) async fn save_serve_setting(
    db: &crate::storage::db::DbPool,
    path: Option<PathBuf>,
) -> Result<AgentGatewayCliStatus, AppError> {
    if let Some(ref path) = path {
        if !path.is_absolute() {
            return Err(AppError::new(
                codes::AGENT_GATEWAY_PATH_INVALID,
                "Agent Gateway CLI path must be absolute",
            ));
        }
        let probe = probe_serve(Some(path)).await;
        if probe.version.is_none() {
            return Err(AppError::new(
                codes::AGENT_GATEWAY_PATH_INVALID,
                probe
                    .error
                    .unwrap_or_else(|| "Agent Gateway CLI path is not executable".into()),
            ));
        }
    }
    let conn = db
        .get()
        .map_err(|error| internal(format!("DB connection failed: {error}")))?;
    let value = path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    crate::storage::app_settings::set(&conn, SERVE_BIN_SETTING, &value)?;
    Ok(cli_status(&probe_serve(path.as_deref()).await))
}

fn cli_status(probe: &CliProbe) -> AgentGatewayCliStatus {
    AgentGatewayCliStatus {
        installed: probe.installed && probe.version.is_some(),
        path: probe.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        version: probe.version.clone(),
        detection: probe.detection.clone(),
        error: probe.error.clone(),
        install_hint: install_hint(),
        download_url: DOWNLOAD_URL.into(),
    }
}

async fn codex_agent(
    configured: Option<PathBuf>,
    configured_path: Option<String>,
) -> AgentGatewayAgentStatus {
    let probe = probe_cli(
        "codex",
        configured.as_deref(),
        "AGENTGATE_CODEX_BIN",
        "codex-cli ",
    )
    .await;
    let mut status = AgentGatewayAgentStatus {
        id: "codex-native".into(),
        name: "Codex".into(),
        installed: probe.installed,
        executable: probe.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        version: probe.version.clone(),
        state: if probe.version.is_some() {
            "available".into()
        } else if probe.installed {
            "not_executable".into()
        } else {
            "not_installed".into()
        },
        available: false,
        auth_state: "unknown".into(),
        auth_detail: String::new(),
        source: "Unknown".into(),
        model: None,
        config_path: None,
        configured_path,
        detection: probe.detection.clone(),
        warnings: Vec::new(),
        error: probe.error.clone(),
    };
    if probe.version.is_none() {
        return status;
    }
    let path = probe.path.as_ref().expect("version implies path");
    if let Some(output) = run_capture(path, &["login", "status"], VERSION_TIMEOUT)
        .await
        .ok()
        .filter(|output| output.status.success())
    {
        let value = text(&output);
        if value.to_ascii_lowercase().contains("logged in") {
            status.auth_state = "authenticated".into();
            status.auth_detail = value.lines().next().unwrap_or("Logged in").to_string();
        } else if value.to_ascii_lowercase().contains("not logged in") {
            status.auth_state = "not_configured".into();
            status.auth_detail = value.lines().next().unwrap_or("Not logged in").to_string();
        } else {
            status.auth_state = "unknown".into();
            status.auth_detail = value
                .lines()
                .next()
                .unwrap_or("Unknown login state")
                .to_string();
        }
    } else {
        status.warnings.push(
            "Could not read `codex login status`; authentication state is unverified.".into(),
        );
    }
    if status.auth_state == "not_configured"
        && (std::env::var_os("OPENAI_API_KEY").is_some()
            || std::env::var_os("CODEX_API_KEY").is_some())
    {
        status.auth_state = "api_key".into();
        status.auth_detail = "API key is present in the environment".into();
    }
    let cfg = crate::tools::codex::detect();
    status.config_path = Some(cfg.config_path.clone());
    status.model = cfg.current_model.clone();
    let content = std::fs::read_to_string(&cfg.config_path).unwrap_or_default();
    let provider = cfg.current_provider.clone();
    let base_url = provider.as_deref().and_then(|name| {
        crate::tools::toml_merge::lookup_str(&content, &["model_providers", name, "base_url"])
    });
    if cfg.is_agentgate_active || cfg.has_agentgate {
        status.source = "MuxLayer Model Gateway".into();
        status.warnings.push(
            "Codex config currently points at MuxLayer Model Gateway; Agent Gateway uses its own native provider and is not affected."
                .into(),
        );
    } else if base_url
        .as_deref()
        .is_some_and(|url| url.contains("127.0.0.1") || url.contains("localhost"))
    {
        status.source = format!("Local provider ({})", base_url.unwrap_or_default());
    } else if let Some(provider) = provider {
        status.source = if provider.eq_ignore_ascii_case("openai") {
            "OpenAI Native".into()
        } else {
            format!("Codex provider: {provider}")
        };
    } else {
        status.source = "OpenAI Native".into();
    }
    if status.auth_state == "unknown" {
        status.warnings.push(
            "Authentication could not be verified; use Test connection to check a real request."
                .into(),
        );
    }
    status.available = status.auth_state != "not_configured";
    if !status.available {
        status.state = "not_configured".into();
    }
    status
}

fn claude_settings() -> (Option<String>, Option<String>, bool, Option<String>) {
    let path = crate::tools::claude_code::settings_path().ok();
    let content = path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let doc: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
    let env = doc.get("env").and_then(|v| v.as_object());
    let setting_model = doc
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let env_model = env
        .and_then(|env| env.get("ANTHROPIC_MODEL"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let base_url = env
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok());
    let token_present = env.is_some_and(|env| {
        env.get("ANTHROPIC_AUTH_TOKEN")
            .and_then(|v| v.as_str())
            .is_some_and(|v| !v.is_empty())
            || env
                .get("ANTHROPIC_API_KEY")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.is_empty())
    }) || std::env::var_os("ANTHROPIC_AUTH_TOKEN").is_some()
        || std::env::var_os("ANTHROPIC_API_KEY").is_some();
    let model = env_model
        .or(setting_model)
        .or_else(|| std::env::var("ANTHROPIC_MODEL").ok());
    (
        model,
        base_url,
        token_present,
        path.map(|p| p.to_string_lossy().to_string()),
    )
}

async fn claude_agent(
    configured: Option<PathBuf>,
    configured_path: Option<String>,
) -> AgentGatewayAgentStatus {
    let probe = probe_cli(
        "claude",
        configured.as_deref(),
        "AGENTGATE_CLAUDE_BIN",
        "Claude Code",
    )
    .await;
    let (model, base_url, token_present, config_path) = claude_settings();
    let login_state = if let Some(path) = probe.path.as_ref() {
        let output = run_capture(path, &["auth", "status"], VERSION_TIMEOUT)
            .await
            .ok();
        let logged_in = output
            .as_ref()
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .and_then(|v| v.get("loggedIn").and_then(|v| v.as_bool()))
            .unwrap_or(false);
        let method = output
            .as_ref()
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .and_then(|v| {
                v.get("authMethod")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        (logged_in, method)
    } else {
        (false, None)
    };
    let mut status = AgentGatewayAgentStatus {
        id: "claude-code".into(),
        name: "Claude Code".into(),
        installed: probe.installed,
        executable: probe.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        version: probe.version.clone(),
        state: if probe.version.is_some() {
            "available".into()
        } else if probe.installed {
            "not_executable".into()
        } else {
            "not_installed".into()
        },
        available: false,
        auth_state: "unknown".into(),
        auth_detail: String::new(),
        source: "Unknown".into(),
        model,
        config_path,
        configured_path,
        detection: probe.detection.clone(),
        warnings: Vec::new(),
        error: probe.error.clone(),
    };
    if probe.version.is_none() {
        return status;
    }
    let (logged_in, method) = login_state;
    if token_present {
        status.auth_state = "third_party_token".into();
        status.auth_detail = "Third-party Anthropic-compatible token/API key is configured".into();
    } else if logged_in {
        status.auth_state = "authenticated".into();
        status.auth_detail = method
            .map(|method| format!("Logged in ({method})"))
            .unwrap_or_else(|| "Logged in".into());
    } else {
        status.auth_state = "not_configured".into();
        status.auth_detail = "No Claude Code login or third-party token was found".into();
    }
    status.source = match base_url.as_deref() {
        Some(url) if url.to_ascii_lowercase().contains("deepseek") => "DeepSeek".into(),
        Some(url) if url.contains("127.0.0.1") || url.contains("localhost") => {
            format!("MuxLayer Model Gateway / local ({url})")
        }
        Some(url) => format!("Third-party Anthropic endpoint ({url})"),
        None if logged_in => "Anthropic official".into(),
        None => "Unknown".into(),
    };
    if !logged_in && token_present {
        status.warnings.push(
            "No Anthropic subscription login; the configured third-party Claude Code model is used instead."
                .into(),
        );
    }
    if status.source == "Unknown" {
        status.warnings.push(
            "No model endpoint or login was found. Configure Claude Code or use Test connection."
                .into(),
        );
    }
    status.available = status.auth_state != "not_configured";
    if !status.available {
        status.state = "not_configured".into();
    }
    status
}

pub async fn detect_agents(paths: &settings::AgentPaths) -> Vec<AgentGatewayAgentStatus> {
    vec![
        codex_agent(
            paths.codex_bin.clone(),
            paths
                .codex_bin
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        )
        .await,
        claude_agent(
            paths.claude_bin.clone(),
            paths
                .claude_bin
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        )
        .await,
    ]
}

fn format_service_log_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let level = value["level"].as_str().unwrap_or("LOG");
        let message = value["message"].as_str().unwrap_or(trimmed);
        return Some(format!("{level}: {message}").chars().take(300).collect());
    }
    Some(trimmed.chars().take(300).collect())
}

fn last_service_error(log: &Path) -> Option<String> {
    let content = std::fs::read_to_string(log).ok()?;
    let line = content
        .lines()
        .rev()
        .find(|line| line.contains("\"level\":\"ERROR\"") || line.contains("\"level\":\"WARN\""))?;
    format_service_log_line(line)
}

fn service_agents(value: &Value) -> Vec<AgentGatewayServiceAgent> {
    value["agents"]
        .as_array()
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| {
                    Some(AgentGatewayServiceAgent {
                        id: agent["id"].as_str()?.to_string(),
                        name: agent["name"].as_str()?.to_string(),
                        permissions: agent["permissions"]
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| item.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn connect_service(
    db_dir: &Path,
) -> Result<ipc::IpcClient, crate::agent_gateway::AgentError> {
    ipc::IpcClient::connect(&ipc::ServicePaths::new(db_dir)).await
}

pub(crate) async fn service_status(db_dir: &Path, cli: &CliProbe) -> AgentGatewayServiceStatus {
    let paths = ipc::ServicePaths::new(db_dir);
    let mut status = AgentGatewayServiceStatus {
        installed: cli.version.is_some(),
        cli_path: cli.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        cli_version: cli.version.clone(),
        data_dir: db_dir.to_string_lossy().to_string(),
        running: false,
        service_version: None,
        running_tasks: Vec::new(),
        agents: Vec::new(),
        last_error: last_service_error(&paths.log),
        error: None,
        install_hint: install_hint(),
        download_url: DOWNLOAD_URL.into(),
    };
    if !paths.socket.exists() || !paths.token.is_file() || !paths.is_live() {
        return status;
    }
    match tokio::time::timeout(Duration::from_secs(5), connect_service(db_dir)).await {
        Ok(Ok(mut client)) => match client.call("service_status", json!({})).await {
            Ok(Ok(value)) => {
                status.running = true;
                status.service_version = value["version"].as_str().map(str::to_string);
                status.running_tasks = value["running_task_ids"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                status.agents = service_agents(&value);
            }
            Ok(Err(error)) => status.error = Some(error.message),
            Err(error) => status.error = Some(error.message),
        },
        Ok(Err(error)) => {
            status.error = Some(format!("Agent Service is not reachable: {}", error.message))
        }
        Err(_) => status.error = Some("Agent Service connection timed out".into()),
    }
    status
}

pub(crate) async fn start_service(
    db_dir: &Path,
    cli: &CliProbe,
) -> Result<AgentGatewayServiceStatus, AppError> {
    let initial = service_status(db_dir, cli).await;
    if initial.running {
        return Ok(initial);
    }
    let Some(executable) = cli.path.as_ref().filter(|_| cli.version.is_some()) else {
        return Err(AppError::new(
            codes::AGENT_GATEWAY_CLI_NOT_FOUND,
            "Agent Gateway CLI is not installed",
        )
        .with_suggestion(install_hint()));
    };
    ipc::spawn_detached_service(db_dir, executable)
        .map_err(|error| AppError::new(codes::AGENT_GATEWAY_SERVICE_START_FAILED, error.message))?;
    let deadline = Instant::now() + SERVICE_START_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(Ok(mut client)) =
            tokio::time::timeout(Duration::from_secs(2), connect_service(db_dir)).await
        {
            if let Ok(Ok(_)) = client.call("service_status", json!({})).await {
                return Ok(service_status(db_dir, cli).await);
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let status = service_status(db_dir, cli).await;
    Err(AppError::new(
        codes::AGENT_GATEWAY_SERVICE_START_FAILED,
        "Agent Service did not become ready",
    )
    .with_detail(status.last_error.unwrap_or_default())
    .with_suggestion("Check the Agent Service log and the installed agentgate-serve CLI."))
}

pub(crate) async fn stop_service(
    db_dir: &Path,
    cli: &CliProbe,
    force: bool,
) -> Result<AgentGatewayServiceStatus, AppError> {
    let status = service_status(db_dir, cli).await;
    if !status.running {
        return Ok(status);
    }
    if !status.running_tasks.is_empty() && !force {
        return Err(AppError::new(
            codes::AGENT_GATEWAY_SERVICE_BUSY,
            format!(
                "Agent Service has {} running task(s); confirm before stopping.",
                status.running_tasks.len()
            ),
        )
        .with_detail(status.running_tasks.join(", ")));
    }
    let mut client = connect_service(db_dir).await.map_err(agent_error)?;
    match client.call("service_shutdown", json!({})).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return Err(agent_error(error)),
        Err(error) => return Err(agent_error(error)),
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if !ipc::ServicePaths::new(db_dir).socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(service_status(db_dir, cli).await)
}

pub(crate) async fn restart_service(
    db_dir: &Path,
    cli: &CliProbe,
    force: bool,
) -> Result<AgentGatewayServiceStatus, AppError> {
    stop_service(db_dir, cli, force).await?;
    start_service(db_dir, cli).await
}

fn normalize_path(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn args_match(args: &[String], expected: &[String]) -> bool {
    args == expected
}

fn entry_matches(entry: &McpEntry, serve: Option<&Path>) -> bool {
    let command = Path::new(&entry.command);
    let expected = serve.map(normalize_path);
    let actual = normalize_path(command);
    let command_matches = match expected {
        Some(expected) => actual == expected,
        None => command
            .file_name()
            .is_some_and(|name| name == "agentgate-serve"),
    };
    command_matches && args_match(&entry.args, &["mcp-agent".to_string()])
}

fn codeleveler_entry(config: &Path, name: &str) -> Result<Option<McpEntry>, AppError> {
    if !config.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(config).map_err(|error| {
        AppError::new(
            codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
            format!("Cannot read CodeLeveler config: {error}"),
        )
    })?;
    let doc = content.parse::<toml_edit::DocumentMut>().map_err(|error| {
        AppError::new(
            codes::AGENT_GATEWAY_MCP_CONFLICT,
            format!("Cannot parse CodeLeveler config: {error}"),
        )
    })?;
    let Some(entries) = doc
        .get("mcp_servers")
        .and_then(|item| item.as_array_of_tables())
    else {
        return Ok(None);
    };
    for entry in entries {
        if entry.get("name").and_then(|item| item.as_str()) != Some(name) {
            continue;
        }
        let command = entry
            .get("command")
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_string();
        let args = entry
            .get("args")
            .and_then(|item| item.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Some(McpEntry { command, args }));
    }
    Ok(None)
}

fn pi_entry(config: &Path, name: &str) -> Result<Option<McpEntry>, AppError> {
    if !config.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(config).map_err(|error| {
        AppError::new(
            codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
            format!("Cannot read Pi MCP config: {error}"),
        )
    })?;
    let value: Value = serde_json::from_str(&content).map_err(|error| {
        AppError::new(
            codes::AGENT_GATEWAY_MCP_CONFLICT,
            format!("Cannot parse Pi MCP config: {error}"),
        )
    })?;
    let Some(entry) = value
        .get("mcpServers")
        .and_then(|servers| servers.get(name))
    else {
        return Ok(None);
    };
    let command = entry
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let args = entry
        .get("args")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(McpEntry { command, args }))
}

fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let token = version
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('v');
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

fn codeleveler_compatibility(version: Option<&str>) -> (bool, String) {
    let Some(version) = version else {
        return (
            false,
            "Version could not be read; verify that the installed CodeLeveler contains the MCP isError fix (e4510fa)."
                .into(),
        );
    };
    match parse_semver(version) {
        Some((1, 0, patch)) if patch >= 18 => (
            false,
            "This source build is new enough to include the MCP isError fix. Confirm the formal release contains e4510fa."
                .into(),
        ),
        Some((1, 0, patch)) if patch <= 17 => (
            true,
            "This version predates the MCP isError fix (e4510fa). MCP errors may be shown as success; upgrade to a release containing the fix."
                .into(),
        ),
        _ => (
            false,
            "Version is not in the known 1.0.x range; verify that the installed release contains e4510fa before relying on MCP error propagation."
                .into(),
        ),
    }
}

fn client_config_path(id: &str) -> Result<PathBuf, AppError> {
    match id {
        "codeleveler" => crate::tools::codeleveler::config_path(),
        "pi" => Ok(crate::fsutil::home_dir()?.join(".pi/agent/mcp.json")),
        _ => Err(AppError::new(
            codes::AGENT_GATEWAY_AGENT_NOT_FOUND,
            format!("unknown MCP client: {id}"),
        )),
    }
}

fn copy_values(serve: Option<&Path>) -> (String, Vec<String>) {
    let command = serve
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "agentgate-serve".into());
    (command, vec!["mcp-agent".into()])
}

fn copy_payloads(id: &str, serve: Option<&Path>) -> (String, Vec<String>, String, Vec<String>) {
    let (command, args) = copy_values(serve);
    let json = serde_json::json!({
        "mcpServers": {
            MCP_SERVER_NAME: {
                "command": command,
                "args": args
            }
        }
    })
    .to_string();
    let toml = format!(
        "[[mcp_servers]]\nname = \"{}\"\ncommand = \"{}\"\nargs = [\"mcp-agent\"]\n",
        MCP_SERVER_NAME,
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let _ = id;
    (command, args, json, vec![toml])
}

fn client_status_from_parts(
    id: &str,
    name: &str,
    probe: &CliProbe,
    config_path: &Path,
    entry: Option<&McpEntry>,
    serve: Option<&Path>,
) -> AgentGatewayClientStatus {
    let (command, args, json, toml_values) = copy_payloads(id, serve);
    let matching = entry.is_some_and(|entry| entry_matches(entry, serve));
    let configured = entry.is_some();
    let (requires_upgrade, compatibility) = if id == "codeleveler" {
        codeleveler_compatibility(probe.version.as_deref())
    } else {
        (
            false,
            "No known MCP compatibility issue for this client.".into(),
        )
    };
    let warning = if !probe.installed {
        Some(
            "Client CLI is not installed; MCP configuration cannot be changed from this page."
                .into(),
        )
    } else if entry.is_some() {
        if !matching {
            Some(format!(
                "An MCP server named `{}` already exists with a different command. Remove it in the client before configuring MuxLayer.",
                MCP_SERVER_NAME
            ))
        } else if requires_upgrade {
            Some(
                "Upgrade CodeLeveler to a release containing the MCP isError fix (e4510fa).".into(),
            )
        } else {
            None
        }
    } else if requires_upgrade {
        Some("Upgrade CodeLeveler to a release containing the MCP isError fix (e4510fa).".into())
    } else {
        None
    };
    let config_status = if !probe.installed {
        "not_installed"
    } else if entry.is_none() {
        "not_configured"
    } else if matching {
        "configured"
    } else {
        "conflict"
    };
    AgentGatewayClientStatus {
        id: id.into(),
        name: name.into(),
        installed: probe.installed && probe.version.is_some(),
        version: probe.version.clone(),
        executable: probe.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        config_path: config_path.to_string_lossy().to_string(),
        config_exists: config_path.exists(),
        mcp_name: MCP_SERVER_NAME.into(),
        config_status: config_status.into(),
        configured,
        matching,
        command: entry.map(|e| e.command.clone()),
        args: entry.map(|e| e.args.clone()).unwrap_or_default(),
        copy_command: command,
        copy_args: args,
        copy_json: json,
        copy_toml: toml_values[0].clone(),
        compatibility,
        requires_upgrade,
        warning,
        error: probe.error.clone(),
        live_status_available: false,
        live_status_note:
            "Configuration presence is not an online status. Confirm the client connection inside the client."
                .into(),
    }
}

pub(crate) async fn detect_client(
    id: &str,
    serve: Option<&Path>,
) -> Result<AgentGatewayClientStatus, AppError> {
    match id {
        "codeleveler" => {
            let probe = probe_cli("leveler", None, "AGENTGATE_LEVELER_BIN", "1.").await;
            let config = crate::tools::codeleveler::config_path()?;
            let entry = codeleveler_entry(&config, MCP_SERVER_NAME)?;
            Ok(client_status_from_parts(
                "codeleveler",
                "CodeLeveler",
                &probe,
                &config,
                entry.as_ref(),
                serve,
            ))
        }
        "pi" => {
            let probe = probe_cli("pi", None, "AGENTGATE_PI_BIN", "1.").await;
            let config = client_config_path("pi")?;
            let entry = pi_entry(&config, MCP_SERVER_NAME)?;
            Ok(client_status_from_parts(
                "pi",
                "Pi Agent",
                &probe,
                &config,
                entry.as_ref(),
                serve,
            ))
        }
        _ => Err(AppError::new(
            codes::AGENT_GATEWAY_AGENT_NOT_FOUND,
            format!("unknown MCP client: {id}"),
        )),
    }
}

pub(crate) async fn detect_clients(serve: Option<&Path>) -> Vec<AgentGatewayClientStatus> {
    let mut clients = Vec::new();
    for id in ["codeleveler", "pi"] {
        match detect_client(id, serve).await {
            Ok(client) => clients.push(client),
            Err(error) => clients.push(AgentGatewayClientStatus {
                id: id.into(),
                name: if id == "pi" {
                    "Pi Agent"
                } else {
                    "CodeLeveler"
                }
                .into(),
                installed: false,
                version: None,
                executable: None,
                config_path: String::new(),
                config_exists: false,
                mcp_name: MCP_SERVER_NAME.into(),
                config_status: "error".into(),
                configured: false,
                matching: false,
                command: None,
                args: Vec::new(),
                copy_command: "agentgate-serve".into(),
                copy_args: vec!["mcp-agent".into()],
                copy_json: String::new(),
                copy_toml: String::new(),
                compatibility: String::new(),
                requires_upgrade: false,
                warning: None,
                error: Some(error.message),
                live_status_available: false,
                live_status_note: String::new(),
            }),
        }
    }
    clients
}

fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::new(
            codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
            format!("Cannot read {}: {error}", path.display()),
        )),
    }
}

fn restore_config(path: &Path, before: Option<&[u8]>) -> Result<(), AppError> {
    match before {
        Some(bytes) => crate::fsutil::atomic_write(path, bytes, None).map_err(|error| {
            AppError::new(
                codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
                format!("Failed to restore {}: {error}", path.display()),
            )
        }),
        None => {
            if path.exists() {
                std::fs::remove_file(path).map_err(|error| {
                    AppError::new(
                        codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
                        format!("Failed to remove {}: {error}", path.display()),
                    )
                })?;
            }
            Ok(())
        }
    }
}

async fn run_client_cli(executable: &Path, args: &[String]) -> Result<String, AppError> {
    let mut command = tokio::process::Command::new(executable);
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(CLI_WRITE_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => Ok(text(&output)),
        Ok(Ok(output)) => Err(AppError::new(
            codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
            format!("{} failed: {}", executable.display(), text(&output)),
        )),
        Ok(Err(error)) => Err(AppError::new(
            codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
            format!("Cannot run {}: {error}", executable.display()),
        )),
        Err(_) => Err(AppError::new(
            codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
            format!("{} timed out", executable.display()),
        )),
    }
}

enum McpMutation {
    Proceed,
    Noop(&'static str),
}

fn mcp_mutation(status: &AgentGatewayClientStatus, remove: bool) -> Result<McpMutation, AppError> {
    if remove {
        if !status.configured {
            return Ok(McpMutation::Noop("MuxLayer MCP is already absent."));
        }
        if !status.matching {
            return Err(AppError::new(
                codes::AGENT_GATEWAY_MCP_CONFLICT,
                "The existing `muxlayer` MCP entry is not managed by this app; refusing to remove it.",
            ));
        }
    } else if status.configured && !status.matching {
        return Err(AppError::new(
            codes::AGENT_GATEWAY_MCP_CONFLICT,
            "An MCP server named `muxlayer` already exists with a different command.",
        ));
    } else if status.matching {
        return Ok(McpMutation::Noop("MuxLayer MCP is already configured."));
    }
    Ok(McpMutation::Proceed)
}

async fn mutate_mcp(
    id: &str,
    serve: Option<&Path>,
    remove: bool,
    confirmed: bool,
) -> Result<AgentGatewayMcpResult, AppError> {
    if !confirmed {
        return Err(AppError::validation("User confirmation is required"));
    }
    let status = detect_client(id, serve).await?;
    let Some(_serve) = serve else {
        return Err(AppError::new(
            codes::AGENT_GATEWAY_CLI_NOT_FOUND,
            "Agent Gateway CLI is not installed",
        )
        .with_suggestion(install_hint()));
    };
    if !status.installed {
        return Err(AppError::new(
            codes::AGENT_GATEWAY_CLIENT_NOT_INSTALLED,
            format!("{} CLI is not installed", status.name),
        )
        .with_suggestion("Install the client CLI first, or copy the MCP configuration manually."));
    }
    let executable = status
        .executable
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| {
            AppError::new(
                codes::AGENT_GATEWAY_CLIENT_NOT_INSTALLED,
                "client path unavailable",
            )
        })?;
    match mcp_mutation(&status, remove)? {
        McpMutation::Noop(message) => {
            return Ok(AgentGatewayMcpResult {
                status,
                message: message.into(),
            });
        }
        McpMutation::Proceed => {}
    }
    let serve_path = serve.expect("checked above");
    let config = PathBuf::from(&status.config_path);
    let before = read_bytes(&config)?;
    let mut args: Vec<String> = vec!["mcp".into()];
    if remove {
        args.extend(["remove".into(), MCP_SERVER_NAME.into()]);
    } else {
        args.extend([
            "add".into(),
            MCP_SERVER_NAME.into(),
            "--".into(),
            serve_path.to_string_lossy().to_string(),
            "mcp-agent".into(),
        ]);
    }
    let run = run_client_cli(&executable, &args).await;
    let after = match &run {
        Ok(_) => detect_client(id, serve).await,
        Err(error) => Err(error.clone()),
    };
    let verified = after.as_ref().ok().is_some_and(|next| {
        if remove {
            !next.configured
        } else {
            next.matching
        }
    });
    if run.is_err() || !verified {
        let _ = restore_config(&config, before.as_deref());
        return Err(match run {
            Err(error) => error,
            Ok(_) => AppError::new(
                codes::AGENT_GATEWAY_MCP_WRITE_FAILED,
                "The client CLI did not produce the expected MCP configuration; the original file was restored.",
            ),
        });
    }
    Ok(AgentGatewayMcpResult {
        status: after.expect("verified"),
        message: if remove {
            "MuxLayer MCP configuration removed.".into()
        } else {
            "MuxLayer MCP configuration saved.".into()
        },
    })
}

pub(crate) async fn configure_client(
    id: &str,
    serve: Option<&Path>,
    confirmed: bool,
) -> Result<AgentGatewayMcpResult, AppError> {
    mutate_mcp(id, serve, false, confirmed).await
}

pub(crate) async fn remove_client(
    id: &str,
    serve: Option<&Path>,
    confirmed: bool,
) -> Result<AgentGatewayMcpResult, AppError> {
    mutate_mcp(id, serve, true, confirmed).await
}

pub(crate) fn mcp_copy(id: &str, serve: Option<&Path>) -> Result<AgentGatewayMcpCopy, AppError> {
    let _ = id;
    let (command, args, json, toml_values) = copy_payloads(id, serve);
    Ok(AgentGatewayMcpCopy {
        name: MCP_SERVER_NAME.into(),
        command,
        args,
        json,
        toml: toml_values[0].clone(),
        note: "No tokens are included. Use the format supported by your MCP client.".into(),
    })
}

pub(crate) async fn test_agent(
    agent_id: &str,
    db_dir: &Path,
    cli: &CliProbe,
    confirmed: bool,
) -> Result<AgentGatewayTestResult, AppError> {
    if !confirmed {
        return Err(AppError::validation(
            "Confirm that the connection test may consume model usage",
        ));
    }
    if agent_id != "codex-native" && agent_id != "claude-code" {
        return Err(AppError::new(
            codes::AGENT_GATEWAY_AGENT_NOT_FOUND,
            format!("unknown Agent: {agent_id}"),
        ));
    }
    let started_service = !service_status(db_dir, cli).await.running;
    if started_service {
        start_service(db_dir, cli).await?;
    }
    let started = Instant::now();
    let mut client = connect_service(db_dir).await.map_err(agent_error)?;
    let repo = std::env::temp_dir().join(format!(
        "muxlayer-agent-test-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&repo)
        .map_err(|error| internal(format!("Cannot create test directory: {error}")))?;
    let submission = match client
        .call(
            "agent_run",
            json!({
                "agent": agent_id,
                "cwd": repo,
                "prompt": "Reply with exactly AGENTGATE_OK. Do not use tools and do not read or modify files.",
                "permission": "read_only"
            }),
        )
        .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            let _ = std::fs::remove_dir_all(&repo);
            return Err(agent_error(error));
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&repo);
            return Err(agent_error(error));
        }
    };
    let task_id = submission["task_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let session_id = submission["session_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut last_state = "queued".to_string();
    let mut final_answer = None;
    let mut error = None;
    while Instant::now() < deadline {
        let status = match client
            .call("agent_status", json!({ "task_id": task_id }))
            .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(agent_error_value)) => {
                error = Some(agent_error_value.message);
                break;
            }
            Err(transport_error) => {
                error = Some(transport_error.message);
                break;
            }
        };
        last_state = status["state"].as_str().unwrap_or("unknown").to_string();
        final_answer = status["final_answer"].as_str().map(str::to_string);
        if status["error"]["message"].as_str().is_some() {
            error = status["error"]["message"].as_str().map(str::to_string);
        }
        if matches!(
            last_state.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted"
        ) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if !matches!(
        last_state.as_str(),
        "completed" | "failed" | "cancelled" | "interrupted"
    ) {
        let _ = client
            .call("agent_cancel", json!({ "task_id": task_id }))
            .await;
        error = Some("Agent test timed out and was cancelled".into());
    }
    let _ = std::fs::remove_dir_all(&repo);
    let success = last_state == "completed"
        && final_answer
            .as_deref()
            .is_some_and(|answer| answer.contains("AGENTGATE_OK"));
    Ok(AgentGatewayTestResult {
        success,
        agent: agent_id.into(),
        state: last_state,
        task_id: Some(task_id),
        session_id: Some(session_id),
        final_answer,
        error: if success {
            None
        } else {
            Some(error.unwrap_or_else(|| "Agent did not return AGENTGATE_OK".into()))
        },
        duration_ms: started.elapsed().as_millis() as u64,
        started_service,
    })
}

pub(crate) async fn overview(
    db_dir: &Path,
    serve_setting: Option<&Path>,
) -> Result<AgentGatewayOverview, AppError> {
    let paths = settings::load(db_dir).map_err(agent_error)?;
    let cli_probe = probe_serve(serve_setting).await;
    let agents = detect_agents(&paths).await;
    let service = service_status(db_dir, &cli_probe).await;
    let clients = detect_clients(cli_probe.path.as_deref()).await;
    Ok(AgentGatewayOverview {
        cli: cli_status(&cli_probe),
        data_dir: db_dir.to_string_lossy().to_string(),
        agents,
        service,
        clients,
    })
}

pub(crate) async fn save_agent_path(
    db_dir: &Path,
    agent_id: &str,
    path: Option<PathBuf>,
    serve: Option<&Path>,
) -> Result<AgentGatewayPathResult, AppError> {
    let key = match agent_id {
        "codex-native" => CODEX_BIN_KEY,
        "claude-code" => CLAUDE_BIN_KEY,
        _ => {
            return Err(AppError::new(
                codes::AGENT_GATEWAY_AGENT_NOT_FOUND,
                format!("unknown Agent: {agent_id}"),
            ))
        }
    };
    if let Some(ref path) = path {
        if !path.is_absolute() {
            return Err(AppError::new(
                codes::AGENT_GATEWAY_PATH_INVALID,
                "Agent CLI path must be absolute",
            ));
        }
        let probe = probe_cli(
            if agent_id == "codex-native" {
                "codex"
            } else {
                "claude"
            },
            Some(path),
            if agent_id == "codex-native" {
                "AGENTGATE_CODEX_BIN"
            } else {
                "AGENTGATE_CLAUDE_BIN"
            },
            if agent_id == "codex-native" {
                "codex-cli "
            } else {
                "Claude Code"
            },
        )
        .await;
        if probe.version.is_none() {
            return Err(AppError::new(
                codes::AGENT_GATEWAY_PATH_INVALID,
                probe
                    .error
                    .unwrap_or_else(|| "Configured CLI path could not be executed".into()),
            ));
        }
    }
    settings::set_bin(db_dir, key, path.as_deref()).map_err(agent_error)?;
    let paths = settings::load(db_dir).map_err(agent_error)?;
    let status = if agent_id == "codex-native" {
        codex_agent(
            paths.codex_bin.clone(),
            paths
                .codex_bin
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        )
        .await
    } else {
        claude_agent(
            paths.claude_bin.clone(),
            paths
                .claude_bin
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        )
        .await
    };
    let requires_service_restart = service_status(db_dir, &probe_serve(serve).await)
        .await
        .running;
    Ok(AgentGatewayPathResult {
        status,
        requires_service_restart,
    })
}

pub(crate) async fn diagnostics(
    db_dir: &Path,
    serve_setting: Option<&Path>,
) -> Result<String, AppError> {
    let overview = overview(db_dir, serve_setting).await?;
    let mut lines = vec![
        format!("MuxLayer Agent Gateway diagnostics"),
        format!("data_dir: {}", overview.data_dir),
        format!(
            "cli: {} ({})",
            overview.cli.path.as_deref().unwrap_or("not installed"),
            overview.cli.version.as_deref().unwrap_or("unknown")
        ),
        format!(
            "service: {} running={} tasks={}",
            overview
                .service
                .service_version
                .as_deref()
                .unwrap_or("not running"),
            overview.service.running,
            overview.service.running_tasks.len()
        ),
        format!(
            "service_last_error: {}",
            overview.service.last_error.as_deref().unwrap_or("none")
        ),
    ];
    for agent in &overview.agents {
        lines.push(format!(
            "agent {}: installed={} version={} state={} auth={} source={} model={} path={}",
            agent.id,
            agent.installed,
            agent.version.as_deref().unwrap_or("unknown"),
            agent.state,
            agent.auth_state,
            agent.source,
            agent.model.as_deref().unwrap_or("default"),
            agent.executable.as_deref().unwrap_or("not found")
        ));
    }
    for client in &overview.clients {
        lines.push(format!(
            "client {}: installed={} version={} config={} status={} upgrade={}",
            client.id,
            client.installed,
            client.version.as_deref().unwrap_or("unknown"),
            client.config_path,
            client.config_status,
            client.requires_upgrade
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parsing_handles_codeleveler_version_strings() {
        assert_eq!(parse_semver("1.0.16 (abc)"), Some((1, 0, 16)));
        assert_eq!(parse_semver("1.0.18 (e58c-dirty)"), Some((1, 0, 18)));
        assert_eq!(parse_semver("not a version"), None);
    }

    #[test]
    fn old_codeleveler_is_flagged_for_upgrade() {
        let (upgrade, _) = codeleveler_compatibility(Some("1.0.17 (abc)"));
        assert!(upgrade);
        let (upgrade, _) = codeleveler_compatibility(Some("1.0.18 (abc)"));
        assert!(!upgrade);
    }

    #[test]
    fn mcp_entry_matching_requires_command_and_args() {
        let serve = Path::new("/tmp/agentgate-serve");
        let entry = McpEntry {
            command: "/tmp/agentgate-serve".into(),
            args: vec!["mcp-agent".into()],
        };
        assert!(entry_matches(&entry, Some(serve)));
        let wrong = McpEntry {
            command: "/tmp/other".into(),
            args: vec!["mcp-agent".into()],
        };
        assert!(!entry_matches(&wrong, Some(serve)));
    }

    #[test]
    fn codeleveler_entry_round_trips_through_toml_parser() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[mcp_servers]]\nname = \"muxlayer\"\ncommand = \"/tmp/agentgate-serve\"\nargs = [\"mcp-agent\"]\n",
        )
        .unwrap();
        let entry = codeleveler_entry(&path, "muxlayer").unwrap().unwrap();
        assert_eq!(entry.command, "/tmp/agentgate-serve");
        assert_eq!(entry.args, vec!["mcp-agent"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_serve_accepts_a_valid_cli_and_rejects_a_bad_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("agentgate-serve");
        std::fs::write(
            &script,
            "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\necho 'agentgate 9.9.9'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let probe = probe_serve(Some(&script)).await;
        assert_eq!(probe.version.as_deref(), Some("agentgate 9.9.9"));
        assert_eq!(probe.detection, "manual");
        let missing = probe_serve(Some(&dir.path().join("missing"))).await;
        assert!(missing.version.is_none());
        assert!(missing.error.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn serve_path_setting_is_persisted_in_the_desktop_db() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("agentgate-serve");
        std::fs::write(
            &script,
            "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\necho 'agentgate 9.9.9'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let db = crate::storage::db::init_database(&dir.path().to_path_buf()).unwrap();
        let status = save_serve_setting(&db, Some(script.clone())).await.unwrap();
        assert_eq!(status.version.as_deref(), Some("agentgate 9.9.9"));
        assert_eq!(load_serve_setting(&db).unwrap(), Some(script));
        save_serve_setting(&db, None).await.unwrap();
        assert_eq!(load_serve_setting(&db).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn mcp_copy_contains_no_secret_and_uses_the_expected_command() {
        let copy = mcp_copy("pi", Some(Path::new("/tmp/agentgate-serve"))).unwrap();
        assert_eq!(copy.command, "/tmp/agentgate-serve");
        assert_eq!(copy.args, vec!["mcp-agent"]);
        assert!(!copy.json.to_ascii_lowercase().contains("token"));
        assert!(copy.toml.contains("mcp_servers"));
    }

    #[test]
    fn pi_entry_is_parsed_from_mcp_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"muxlayer":{"command":"/tmp/agentgate-serve","args":["mcp-agent"]}}}"#,
        )
        .unwrap();
        let entry = pi_entry(&path, "muxlayer").unwrap().unwrap();
        assert_eq!(entry.command, "/tmp/agentgate-serve");
        assert_eq!(entry.args, vec!["mcp-agent"]);
    }

    fn status_with_entry(
        entry: Option<McpEntry>,
        serve: Option<&Path>,
    ) -> AgentGatewayClientStatus {
        let probe = CliProbe {
            installed: true,
            path: Some(PathBuf::from("/tmp/pi")),
            version: Some("1.3.0".into()),
            detection: "path".into(),
            error: None,
        };
        client_status_from_parts(
            "pi",
            "Pi Agent",
            &probe,
            Path::new("/tmp/mcp.json"),
            entry.as_ref(),
            serve,
        )
    }

    #[test]
    fn mcp_mutation_refuses_conflicts_and_skips_unchanged_entries() {
        let serve = Path::new("/tmp/agentgate-serve");
        let matching = McpEntry {
            command: "/tmp/agentgate-serve".into(),
            args: vec!["mcp-agent".into()],
        };
        let conflicting = McpEntry {
            command: "/bin/echo".into(),
            args: vec!["mcp-agent".into()],
        };

        assert!(matches!(
            mcp_mutation(&status_with_entry(None, Some(serve)), false),
            Ok(McpMutation::Proceed)
        ));
        assert!(matches!(
            mcp_mutation(&status_with_entry(None, Some(serve)), true),
            Ok(McpMutation::Noop(_))
        ));
        assert!(matches!(
            mcp_mutation(
                &status_with_entry(Some(matching.clone()), Some(serve)),
                true
            ),
            Ok(McpMutation::Proceed)
        ));
        assert!(matches!(
            mcp_mutation(&status_with_entry(Some(matching), Some(serve)), false),
            Ok(McpMutation::Noop(_))
        ));
        assert!(mcp_mutation(
            &status_with_entry(Some(conflicting.clone()), Some(serve)),
            false
        )
        .is_err());
        assert!(mcp_mutation(&status_with_entry(Some(conflicting), Some(serve)), true).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_is_reported_as_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let service = dir.path().join("agent-service");
        std::fs::create_dir_all(&service).unwrap();
        std::fs::write(service.join("token"), "token").unwrap();
        std::fs::write(service.join("agent-service.sock"), "").unwrap();
        let cli = CliProbe {
            installed: true,
            path: Some(PathBuf::from("/tmp/agentgate-serve")),
            version: Some("agentgate 9.9.9".into()),
            detection: "manual".into(),
            error: None,
        };

        let status = service_status(dir.path(), &cli).await;

        assert!(!status.running);
        assert!(status.error.is_none(), "{:?}", status.error);
    }

    #[test]
    fn service_log_errors_hide_internal_metadata() {
        let line = r#"{"timestamp":"2026-10-17T16:54:15.693127Z","level":"ERROR","message":"database is locked","log.target":"r2d2","log.module_path":"r2d2","log.file":"/Users/me/.cargo/registry/src/r2d2-0.8.10/src/lib.rs","log.line":121}"#;
        assert_eq!(
            format_service_log_line(line).as_deref(),
            Some("ERROR: database is locked")
        );
    }

    #[test]
    fn service_log_without_errors_has_no_last_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service.log");
        std::fs::write(
            &path,
            "{\"level\":\"INFO\",\"message\":\"agent service listening\"}\n",
        )
        .unwrap();
        assert_eq!(last_service_error(&path), None);
    }

    #[test]
    fn plain_service_log_lines_stay_readable() {
        assert_eq!(
            format_service_log_line("service exited unexpectedly").as_deref(),
            Some("service exited unexpectedly")
        );
        assert_eq!(format_service_log_line("   "), None);
    }
}
