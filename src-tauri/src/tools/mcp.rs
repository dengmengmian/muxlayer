//! MCP 服务器配置读取(第一步:只读展示)。
//!
//! 以**客户端文件为真相源**,不建数据库表:
//! - Codex: `~/.codex/config.toml` 的 `[mcp_servers.<name>]`(TOML)
//! - Claude Code: `~/.claude.json` 的 `mcpServers.<name>`(JSON,驼峰)
//!
//! env 只返回 key 名,不外泄 value——MCP 的 env 常含 token 等敏感值。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::AppError;
use crate::tools::toml_merge;

const SENSITIVE_ENV_MARKERS: &[&str] =
    &["KEY", "TOKEN", "SECRET", "PASSWORD", "AUTH", "CREDENTIAL"];

/// 一条 MCP server 配置(跨客户端归一后的形态)。
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    /// 只暴露 env 元数据,value 不返回(常含敏感 token)。
    pub env: Vec<McpEnvVar>,
    pub enabled_clients: Vec<String>,
    pub sources: Vec<McpServerSource>,
    pub validation: McpValidationState,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct McpEnvVar {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub is_sensitive: bool,
    pub has_value: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct McpServerSource {
    pub client: String,
    pub source: String,
    pub config_path: String,
    pub raw_name: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct McpValidationState {
    pub status: String,
    pub issues: Vec<McpValidationIssue>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct McpValidationIssue {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
pub struct UpsertMcpServerInput {
    pub client: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<McpEnvInput>,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
pub struct McpEnvInput {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
pub struct SyncMcpServerInput {
    pub from_client: String,
    pub name: String,
    #[serde(default)]
    pub to_clients: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpExport {
    pub version: u32,
    pub servers: Vec<McpExportServer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpExportServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<McpExportEnv>,
    #[serde(default)]
    pub clients: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpExportEnv {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub has_value: bool,
    pub is_sensitive: bool,
}

fn home() -> Result<PathBuf, AppError> {
    crate::fsutil::home_dir()
}

/// 只读列表用:文件不存在 → None(没有配置);其它读取错误 → 作为 `__config__`
/// 校验条目上报,不静默当成"没有 MCP"。
fn read_for_listing(path: &Path, client: &str) -> Result<Option<String>, Box<McpServer>> {
    match fs::read_to_string(path) {
        Ok(c) => Ok(Some(c)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Box::new(config_error_server(
            client,
            path.to_string_lossy().as_ref(),
            format!("Cannot read MCP config: {e}"),
        ))),
    }
}

/// 写入前读取:只有文件不存在才视为空;权限 / 编码错误直接报错,绝不拿空内容覆盖。
fn read_for_write(path: &Path) -> Result<String, AppError> {
    crate::fsutil::read_config_or_empty(path).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot read {}: {e}", path.display()),
        )
    })
}

/// JSON 配置写入前读取;空文件 / 不存在按 `{}` 处理。
fn read_json_for_write(path: &Path) -> Result<String, AppError> {
    let content = read_for_write(path)?;
    Ok(if content.trim().is_empty() {
        "{}".to_string()
    } else {
        content
    })
}

fn read_codex_mcp_from_home(home: &Path) -> Vec<McpServer> {
    let path = home.join(".codex").join("config.toml");
    let content = match read_for_listing(&path, "codex") {
        Ok(Some(c)) => c,
        Ok(None) => return vec![],
        Err(entry) => return vec![*entry],
    };
    let doc = match content.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            return vec![config_error_server(
                "codex",
                path.to_string_lossy().as_ref(),
                format!("Cannot parse Codex MCP config: {e}"),
            )]
        }
    };
    let servers = match doc.get("mcp_servers").and_then(|v| v.as_table()) {
        Some(t) => t,
        None => return vec![],
    };
    let mut out = Vec::new();
    for (name, val) in servers.iter() {
        let Some(tbl) = val.as_table() else { continue };
        let command = tbl
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let args = tbl
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let env = tbl
            .get("env")
            .and_then(|v| v.as_table())
            .map(|e| e.iter().map(|(k, v)| env_var(k, v.as_str())).collect())
            .unwrap_or_default();
        out.push(server(
            "codex",
            path.to_string_lossy().as_ref(),
            name,
            command,
            args,
            env,
        ));
    }
    out
}

fn read_claude_mcp_from_home(home: &Path) -> Vec<McpServer> {
    read_json_mcp_servers(
        &home.join(".claude.json"),
        "claude_code",
        "mcpServers",
        "Cannot parse Claude Code MCP config",
    )
}

fn read_gemini_mcp_from_home(home: &Path) -> Vec<McpServer> {
    read_json_mcp_servers(
        &home.join(".gemini").join("settings.json"),
        "gemini",
        "mcpServers",
        "Cannot parse Gemini CLI MCP config",
    )
}

fn read_json_mcp_servers(path: &Path, client: &str, key: &str, parse_err: &str) -> Vec<McpServer> {
    let content = match read_for_listing(path, client) {
        Ok(Some(c)) => c,
        Ok(None) => return vec![],
        Err(entry) => return vec![*entry],
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return vec![config_error_server(
                client,
                path.to_string_lossy().as_ref(),
                format!("{parse_err}: {e}"),
            )]
        }
    };
    let servers = match json.get(key).and_then(|v| v.as_object()) {
        Some(o) => o,
        None => return vec![],
    };
    let mut out = Vec::new();
    for (name, val) in servers.iter() {
        let (command, args) = command_and_args(val);
        let env = json_env_vars(val);
        out.push(server(
            client,
            path.to_string_lossy().as_ref(),
            name,
            command,
            args,
            env,
        ));
    }
    out
}

fn read_opencode_mcp_from_home(home: &Path) -> Vec<McpServer> {
    let path = home.join(".config").join("opencode").join("opencode.json");
    let content = match read_for_listing(&path, "opencode") {
        Ok(Some(c)) => c,
        Ok(None) => return vec![],
        Err(entry) => return vec![*entry],
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return vec![config_error_server(
                "opencode",
                path.to_string_lossy().as_ref(),
                format!("Cannot parse OpenCode MCP config: {e}"),
            )]
        }
    };
    let servers = opencode_mcp_object(&json);
    let Some(servers) = servers else {
        return vec![];
    };
    let mut out = Vec::new();
    for (name, val) in servers {
        if name == "servers" {
            continue;
        }
        let (command, args) = command_and_args(&val);
        let env = json_env_vars(&val);
        out.push(server(
            "opencode",
            path.to_string_lossy().as_ref(),
            &name,
            command,
            args,
            env,
        ));
    }
    out
}

fn opencode_mcp_object(
    json: &serde_json::Value,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mcp = json.get("mcp")?;
    if let Some(servers) = mcp.get("servers").and_then(|v| v.as_object()) {
        return Some(servers.clone());
    }
    mcp.as_object().cloned()
}

fn command_and_args(val: &serde_json::Value) -> (String, Vec<String>) {
    if let Some(arr) = val.get("command").and_then(|v| v.as_array()) {
        let parts: Vec<String> = arr
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        if parts.is_empty() {
            return (String::new(), vec![]);
        }
        return (parts[0].clone(), parts[1..].to_vec());
    }
    let command = val
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let args = val
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    (command, args)
}

fn json_env_vars(val: &serde_json::Value) -> Vec<McpEnvVar> {
    let env = val
        .get("env")
        .or_else(|| val.get("environment"))
        .and_then(|v| v.as_object());
    env.map(|e| e.iter().map(|(k, v)| env_var(k, v.as_str())).collect())
        .unwrap_or_default()
}

/// 汇总所有客户端的 MCP server。
pub fn list_all() -> Result<Vec<McpServer>, AppError> {
    Ok(list_all_from_home(&home()?))
}

pub fn upsert(input: UpsertMcpServerInput) -> Result<McpServer, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    upsert_in_home(&home()?, input)
}

pub fn delete(client: &str, name: &str) -> Result<bool, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    delete_in_home(&home()?, client, name)
}

pub fn sync(input: SyncMcpServerInput) -> Result<Vec<McpServer>, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    sync_in_home(&home()?, input)
}

pub fn export_config(include_secrets: bool) -> Result<String, AppError> {
    export_from_home(&home()?, include_secrets)
}

pub fn import_config(
    payload: &str,
    target_clients: Vec<String>,
) -> Result<Vec<McpServer>, AppError> {
    // 整个读 → 改 → 写期间持有客户端配置锁,防止并发命令互相覆盖(见 fsutil)。
    let _config_lock = crate::fsutil::lock_client_configs();
    import_in_home(&home()?, payload, target_clients)
}

fn list_all_from_home(home: &Path) -> Vec<McpServer> {
    let mut out = read_codex_mcp_from_home(home);
    out.extend(read_claude_mcp_from_home(home));
    out.extend(read_gemini_mcp_from_home(home));
    out.extend(read_opencode_mcp_from_home(home));
    out
}

fn upsert_in_home(home: &Path, mut input: UpsertMcpServerInput) -> Result<McpServer, AppError> {
    normalize_client(&mut input);
    validate_input(&input)?;
    write_validated(home, input)
}

fn normalize_client(input: &mut UpsertMcpServerInput) {
    if input.client == "gemini_cli" {
        input.client = "gemini".into();
    }
}

/// 批量写入(sync / import):先把全部条目校验完,任何一条不合法就一个都不写。
fn upsert_all_in_home(
    home: &Path,
    mut inputs: Vec<UpsertMcpServerInput>,
) -> Result<Vec<McpServer>, AppError> {
    for input in &mut inputs {
        normalize_client(input);
        validate_input(input)?;
    }
    inputs
        .into_iter()
        .map(|input| write_validated(home, input))
        .collect()
}

fn write_validated(home: &Path, input: UpsertMcpServerInput) -> Result<McpServer, AppError> {
    match input.client.as_str() {
        "codex" => upsert_codex(home, &input)?,
        "claude_code" => upsert_claude(home, &input)?,
        "gemini" | "gemini_cli" => upsert_gemini(home, &input)?,
        "opencode" => upsert_opencode(home, &input)?,
        _ => {
            return Err(AppError::validation(
                "Unsupported MCP client. Expected codex, claude_code, gemini, or opencode",
            ))
        }
    }
    let id = format!("{}:{}", input.client, input.name);
    list_all_from_home(home)
        .into_iter()
        .find(|server| server.id == id)
        .ok_or_else(|| AppError::internal("MCP server was written but could not be read back"))
}

fn delete_in_home(home: &Path, client: &str, name: &str) -> Result<bool, AppError> {
    if name.trim().is_empty() {
        return Err(AppError::validation("MCP server name is required"));
    }
    match client {
        "codex" => delete_codex(home, name),
        "claude_code" => delete_claude(home, name),
        "gemini" | "gemini_cli" => delete_gemini(home, name),
        "opencode" => delete_opencode(home, name),
        _ => Err(AppError::validation(
            "Unsupported MCP client. Expected codex, claude_code, gemini, or opencode",
        )),
    }
}

fn sync_in_home(home: &Path, input: SyncMcpServerInput) -> Result<Vec<McpServer>, AppError> {
    if input.name.trim().is_empty() {
        return Err(AppError::validation("MCP server name is required"));
    }
    if input.to_clients.is_empty() {
        return Err(AppError::validation(
            "Select at least one target MCP client",
        ));
    }
    let source = read_raw_server(home, &input.from_client, &input.name)?;
    let inputs = input
        .to_clients
        .into_iter()
        .filter(|client| *client != input.from_client)
        .map(|client| UpsertMcpServerInput {
            client,
            name: source.name.clone(),
            command: source.command.clone(),
            args: source.args.clone(),
            env: source.env.clone(),
        })
        .collect();
    upsert_all_in_home(home, inputs)
}

fn export_from_home(home: &Path, include_secrets: bool) -> Result<String, AppError> {
    let mut servers = Vec::new();
    for listed in list_all_from_home(home) {
        if listed.name == "__config__" {
            continue;
        }
        let Some(source) = listed.sources.first() else {
            continue;
        };
        let raw = read_raw_server(home, &source.client, &listed.name)?;
        servers.push(McpExportServer {
            name: raw.name,
            command: raw.command,
            args: raw.args,
            env: raw
                .env
                .into_iter()
                .map(|item| McpExportEnv {
                    is_sensitive: is_sensitive_env_key(&item.key),
                    has_value: !item.value.is_empty(),
                    value: include_secrets.then_some(item.value),
                    key: item.key,
                })
                .collect(),
            clients: listed.enabled_clients,
        });
    }
    serde_json::to_string_pretty(&McpExport {
        version: 1,
        servers,
    })
    .map_err(|e| AppError::internal(format!("serialize MCP export: {e}")))
}

fn import_in_home(
    home: &Path,
    payload: &str,
    target_clients: Vec<String>,
) -> Result<Vec<McpServer>, AppError> {
    let export: McpExport = serde_json::from_str(payload)
        .map_err(|e| AppError::validation(format!("Invalid MCP export JSON: {e}")))?;
    if export.version != 1 {
        return Err(AppError::validation("Unsupported MCP export version"));
    }

    let mut inputs = Vec::new();
    for server in export.servers {
        let clients = import_clients(&target_clients, &server.clients)?;
        let env = server
            .env
            .into_iter()
            .filter_map(|item| {
                item.value.map(|value| McpEnvInput {
                    key: item.key,
                    value,
                })
            })
            .collect::<Vec<_>>();
        for client in clients {
            inputs.push(UpsertMcpServerInput {
                client,
                name: server.name.clone(),
                command: server.command.clone(),
                args: server.args.clone(),
                env: env.clone(),
            });
        }
    }
    upsert_all_in_home(home, inputs)
}

fn is_supported_mcp_client(client: &str) -> bool {
    matches!(
        client,
        "codex" | "claude_code" | "gemini" | "gemini_cli" | "opencode"
    )
}

fn import_clients(
    target_clients: &[String],
    fallback_clients: &[String],
) -> Result<Vec<String>, AppError> {
    let source = if target_clients.is_empty() {
        fallback_clients
    } else {
        target_clients
    };
    let mut clients = Vec::new();
    for client in source {
        if !is_supported_mcp_client(client) {
            return Err(AppError::validation(
                "Unsupported MCP client. Expected codex, claude_code, gemini, or opencode",
            ));
        }
        if !clients.contains(client) {
            clients.push(client.clone());
        }
    }
    if clients.is_empty() {
        return Err(AppError::validation(
            "Select at least one target MCP client",
        ));
    }
    Ok(clients)
}

#[derive(Debug, Clone)]
struct RawMcpServer {
    name: String,
    command: String,
    args: Vec<String>,
    env: Vec<McpEnvInput>,
}

fn read_raw_server(home: &Path, client: &str, name: &str) -> Result<RawMcpServer, AppError> {
    match client {
        "codex" => read_raw_codex_server(home, name),
        "claude_code" => read_raw_claude_server(home, name),
        "gemini" | "gemini_cli" => read_raw_json_server(
            home,
            name,
            &home.join(".gemini").join("settings.json"),
            "gemini",
            "mcpServers",
        ),
        "opencode" => read_raw_opencode_server(home, name),
        _ => Err(AppError::validation(
            "Unsupported MCP client. Expected codex, claude_code, gemini, or opencode",
        )),
    }
}

fn read_raw_codex_server(home: &Path, name: &str) -> Result<RawMcpServer, AppError> {
    let path = home.join(".codex").join("config.toml");
    let content =
        fs::read_to_string(&path).map_err(|_| AppError::not_found("Codex MCP server", name))?;
    let doc = content.parse::<toml_edit::DocumentMut>().map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot parse Codex MCP config: {e}"),
        )
    })?;
    let server = doc
        .get("mcp_servers")
        .and_then(|v| v.as_table())
        .and_then(|servers| servers.get(name))
        .and_then(|v| v.as_table())
        .ok_or_else(|| AppError::not_found("Codex MCP server", name))?;
    let command = server
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let args = server
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let env = server
        .get("env")
        .and_then(|v| v.as_table())
        .map(raw_env_from_toml)
        .unwrap_or_default();
    Ok(RawMcpServer {
        name: name.to_string(),
        command,
        args,
        env,
    })
}

fn read_raw_claude_server(home: &Path, name: &str) -> Result<RawMcpServer, AppError> {
    let path = home.join(".claude.json");
    let content = fs::read_to_string(&path)
        .map_err(|_| AppError::not_found("Claude Code MCP server", name))?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot parse Claude Code MCP config: {e}"),
        )
    })?;
    let server = json
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .and_then(|servers| servers.get(name))
        .ok_or_else(|| AppError::not_found("Claude Code MCP server", name))?;
    let command = server
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let args = server
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let env = server
        .get("env")
        .and_then(|v| v.as_object())
        .map(raw_env_from_json)
        .unwrap_or_default();
    Ok(RawMcpServer {
        name: name.to_string(),
        command,
        args,
        env,
    })
}

fn validate_input(input: &UpsertMcpServerInput) -> Result<(), AppError> {
    if input.name.trim().is_empty() {
        return Err(AppError::validation("MCP server name is required"));
    }
    // Codex:名字会直接拼进 `[mcp_servers.<name>]` header(裸 key),只允许
    // TOML 裸 key 字符集;空格 / `#` / 引号 / `.` 会写出非法或错位的段。
    // JSON 客户端(claude / gemini / opencode)的 key 可以是任意字符串,只拒绝
    // 控制字符(换行等),让已有的 `my.server` / `@scope/pkg` 仍能编辑、同步。
    // 只校验新写入,已有配置里不合规的名字仍可读取 / 删除。
    if input.client == "codex" {
        if !is_valid_codex_server_name(&input.name) {
            return Err(AppError::validation(
                "Codex MCP server name must be 1-64 characters of letters, digits, '_' or '-'",
            ));
        }
    } else if input.name.chars().any(char::is_control) {
        return Err(AppError::validation(
            "MCP server name must not contain control characters",
        ));
    }
    if input.command.trim().is_empty() {
        return Err(AppError::validation("MCP server command is required"));
    }
    for item in &input.env {
        if item.key.trim().is_empty() || item.key.contains('=') {
            return Err(AppError::validation(
                "MCP env key must be non-empty and must not contain '='",
            ));
        }
    }
    Ok(())
}

/// `^[A-Za-z0-9_-]{1,64}$`
fn is_valid_codex_server_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn upsert_codex(home: &Path, input: &UpsertMcpServerInput) -> Result<(), AppError> {
    let path = home.join(".codex").join("config.toml");
    ensure_parent(&path)?;
    let content = read_for_write(&path)?;
    let env = merge_codex_env(&content, &input.name, &input.env);
    let mut next = toml_merge::upsert_section(
        &content,
        &format!("mcp_servers.{}", input.name),
        &codex_server_body(input),
    );
    next = if env.is_empty() {
        toml_merge::remove_section(&next, &format!("mcp_servers.{}.env", input.name))
    } else {
        toml_merge::upsert_section(
            &next,
            &format!("mcp_servers.{}.env", input.name),
            &codex_env_body(&env),
        )
    };
    write_config(&path, next.as_bytes(), "Codex")
}

fn delete_codex(home: &Path, name: &str) -> Result<bool, AppError> {
    let path = home.join(".codex").join("config.toml");
    let content = read_for_write(&path)?;
    if content.is_empty() {
        return Ok(false);
    }
    let without_env = toml_merge::remove_section(&content, &format!("mcp_servers.{name}.env"));
    let next = toml_merge::remove_section(&without_env, &format!("mcp_servers.{name}"));
    if next == content {
        return Ok(false);
    }
    write_config(&path, next.as_bytes(), "Codex")?;
    Ok(true)
}

fn upsert_claude(home: &Path, input: &UpsertMcpServerInput) -> Result<(), AppError> {
    let path = home.join(".claude.json");
    ensure_parent(&path)?;
    let content = read_json_for_write(&path)?;
    let mut json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot parse Claude Code MCP config: {e}"),
        )
    })?;
    if !json.is_object() {
        return Err(AppError::validation(
            "Claude Code config root must be an object",
        ));
    }
    if json.get("mcpServers").and_then(|v| v.as_object()).is_none() {
        json["mcpServers"] = serde_json::json!({});
    }
    let env = merge_claude_env(&json, &input.name, &input.env);
    let servers = json["mcpServers"]
        .as_object_mut()
        .ok_or_else(|| AppError::validation("Claude Code mcpServers field must be an object"))?;
    servers.insert(input.name.clone(), claude_server_value(input, &env));
    write_pretty_json(&path, &json)
}

fn delete_claude(home: &Path, name: &str) -> Result<bool, AppError> {
    let path = home.join(".claude.json");
    let content = read_for_write(&path)?;
    if content.trim().is_empty() {
        return Ok(false);
    }
    let mut json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot parse Claude Code MCP config: {e}"),
        )
    })?;
    let Some(servers) = json.get_mut("mcpServers").and_then(|v| v.as_object_mut()) else {
        return Ok(false);
    };
    let removed = servers.remove(name).is_some();
    if removed {
        write_pretty_json(&path, &json)?;
    }
    Ok(removed)
}

fn upsert_gemini(home: &Path, input: &UpsertMcpServerInput) -> Result<(), AppError> {
    upsert_json_mcp_servers(
        &home.join(".gemini").join("settings.json"),
        input,
        "Cannot parse Gemini CLI MCP config",
    )
}

fn delete_gemini(home: &Path, name: &str) -> Result<bool, AppError> {
    delete_json_mcp_servers(
        &home.join(".gemini").join("settings.json"),
        name,
        "Cannot parse Gemini CLI MCP config",
    )
}

fn upsert_json_mcp_servers(
    path: &Path,
    input: &UpsertMcpServerInput,
    parse_err: &str,
) -> Result<(), AppError> {
    ensure_parent(path)?;
    let content = read_json_for_write(path)?;
    let mut json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("{parse_err}: {e}"),
        )
    })?;
    if !json.is_object() {
        return Err(AppError::validation("MCP config root must be an object"));
    }
    if json.get("mcpServers").and_then(|v| v.as_object()).is_none() {
        json["mcpServers"] = serde_json::json!({});
    }
    let env = merge_claude_env(&json, &input.name, &input.env);
    let servers = json["mcpServers"]
        .as_object_mut()
        .ok_or_else(|| AppError::validation("mcpServers field must be an object"))?;
    servers.insert(input.name.clone(), claude_server_value(input, &env));
    write_pretty_json(path, &json)
}

fn delete_json_mcp_servers(path: &Path, name: &str, parse_err: &str) -> Result<bool, AppError> {
    let content = read_for_write(path)?;
    if content.trim().is_empty() {
        return Ok(false);
    }
    let mut json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("{parse_err}: {e}"),
        )
    })?;
    let Some(servers) = json.get_mut("mcpServers").and_then(|v| v.as_object_mut()) else {
        return Ok(false);
    };
    let removed = servers.remove(name).is_some();
    if removed {
        write_pretty_json(path, &json)?;
    }
    Ok(removed)
}

fn upsert_opencode(home: &Path, input: &UpsertMcpServerInput) -> Result<(), AppError> {
    let path = home.join(".config").join("opencode").join("opencode.json");
    ensure_parent(&path)?;
    let content = read_json_for_write(&path)?;
    let mut json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot parse OpenCode MCP config: {e}"),
        )
    })?;
    if !json.is_object() {
        return Err(AppError::validation(
            "OpenCode config root must be an object",
        ));
    }
    if json.get("mcp").and_then(|v| v.as_object()).is_none() {
        json["mcp"] = serde_json::json!({});
    }
    let env = merge_opencode_env(&json, &input.name, &input.env);
    let mut cmd = vec![input.command.clone()];
    cmd.extend(input.args.iter().cloned());
    let mut value = serde_json::json!({
        "type": "local",
        "command": cmd,
    });
    if !env.is_empty() {
        let environment = env
            .iter()
            .map(|item| (item.key.clone(), serde_json::json!(item.value)))
            .collect::<serde_json::Map<String, serde_json::Value>>();
        value["environment"] = serde_json::Value::Object(environment);
    }
    let use_servers = json
        .get("mcp")
        .and_then(|v| v.get("servers"))
        .and_then(|v| v.as_object())
        .is_some();
    if use_servers {
        json["mcp"]["servers"][&input.name] = value;
    } else {
        json["mcp"][&input.name] = value;
    }
    write_pretty_json(&path, &json)
}

fn delete_opencode(home: &Path, name: &str) -> Result<bool, AppError> {
    let path = home.join(".config").join("opencode").join("opencode.json");
    let content = read_for_write(&path)?;
    if content.trim().is_empty() {
        return Ok(false);
    }
    let mut json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot parse OpenCode MCP config: {e}"),
        )
    })?;
    let removed = if let Some(servers) = json
        .get_mut("mcp")
        .and_then(|v| v.get_mut("servers"))
        .and_then(|v| v.as_object_mut())
    {
        servers.remove(name).is_some()
    } else if let Some(mcp) = json.get_mut("mcp").and_then(|v| v.as_object_mut()) {
        mcp.remove(name).is_some()
    } else {
        false
    };
    if removed {
        write_pretty_json(&path, &json)?;
    }
    Ok(removed)
}

fn merge_opencode_env(
    json: &serde_json::Value,
    name: &str,
    input: &[McpEnvInput],
) -> Vec<McpEnvInput> {
    if input.is_empty() {
        return Vec::new();
    }
    let entry = opencode_mcp_object(json).and_then(|m| m.get(name).cloned());
    let existing = entry
        .as_ref()
        .and_then(|v| v.get("environment").or_else(|| v.get("env")))
        .and_then(|v| v.as_object())
        .map(|env| {
            env.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<std::collections::HashMap<_, _>>()
        })
        .unwrap_or_default();
    merge_env(input, &existing)
}

fn read_raw_json_server(
    _home: &Path,
    name: &str,
    path: &Path,
    client: &str,
    key: &str,
) -> Result<RawMcpServer, AppError> {
    let content = fs::read_to_string(path)
        .map_err(|_| AppError::not_found(&format!("{client} MCP server"), name))?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot parse {client} MCP config: {e}"),
        )
    })?;
    let server = json
        .get(key)
        .and_then(|v| v.as_object())
        .and_then(|servers| servers.get(name))
        .ok_or_else(|| AppError::not_found(&format!("{client} MCP server"), name))?;
    let (command, args) = command_and_args(server);
    let env = server
        .get("env")
        .or_else(|| server.get("environment"))
        .and_then(|v| v.as_object())
        .map(raw_env_from_json)
        .unwrap_or_default();
    Ok(RawMcpServer {
        name: name.to_string(),
        command,
        args,
        env,
    })
}

fn read_raw_opencode_server(home: &Path, name: &str) -> Result<RawMcpServer, AppError> {
    let path = home.join(".config").join("opencode").join("opencode.json");
    let content =
        fs::read_to_string(&path).map_err(|_| AppError::not_found("OpenCode MCP server", name))?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_PARSE_ERROR,
            format!("Cannot parse OpenCode MCP config: {e}"),
        )
    })?;
    let servers = opencode_mcp_object(&json)
        .ok_or_else(|| AppError::not_found("OpenCode MCP server", name))?;
    let server = servers
        .get(name)
        .ok_or_else(|| AppError::not_found("OpenCode MCP server", name))?;
    let (command, args) = command_and_args(server);
    let env = server
        .get("environment")
        .or_else(|| server.get("env"))
        .and_then(|v| v.as_object())
        .map(raw_env_from_json)
        .unwrap_or_default();
    Ok(RawMcpServer {
        name: name.to_string(),
        command,
        args,
        env,
    })
}

fn ensure_parent(path: &Path) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AppError::new(
                crate::errors::codes::MCP_CONFIG_WRITE_FAILED,
                format!("Cannot create MCP config directory: {e}"),
            )
        })?;
    }
    Ok(())
}

fn codex_server_body(input: &UpsertMcpServerInput) -> String {
    format!(
        "command = {}\nargs = {}\n",
        toml_string(&input.command),
        toml_string_array(&input.args)
    )
}

fn codex_env_body(env: &[McpEnvInput]) -> String {
    env.iter()
        .map(|item| format!("{} = {}", item.key, toml_string(&item.value)))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn claude_server_value(
    input: &UpsertMcpServerInput,
    env_input: &[McpEnvInput],
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "command": input.command,
        "args": input.args,
    });
    if !env_input.is_empty() {
        let env = env_input
            .iter()
            .map(|item| (item.key.clone(), serde_json::json!(item.value)))
            .collect::<serde_json::Map<String, serde_json::Value>>();
        value["env"] = serde_json::Value::Object(env);
    }
    value
}

fn merge_codex_env(content: &str, name: &str, input: &[McpEnvInput]) -> Vec<McpEnvInput> {
    if input.is_empty() {
        return Vec::new();
    }
    let existing = content
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|doc| {
            let server = doc
                .get("mcp_servers")
                .and_then(|v| v.as_table())
                .and_then(|servers| servers.get(name))
                .and_then(|v| v.as_table())?;
            let env = server.get("env").and_then(|v| v.as_table())?;
            Some(
                env.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
                    .collect::<std::collections::HashMap<_, _>>(),
            )
        })
        .unwrap_or_default();
    merge_env(input, &existing)
}

fn merge_claude_env(
    json: &serde_json::Value,
    name: &str,
    input: &[McpEnvInput],
) -> Vec<McpEnvInput> {
    if input.is_empty() {
        return Vec::new();
    }
    let existing = json
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .and_then(|servers| servers.get(name))
        .and_then(|server| server.get("env"))
        .and_then(|v| v.as_object())
        .map(|env| {
            env.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<std::collections::HashMap<_, _>>()
        })
        .unwrap_or_default();
    merge_env(input, &existing)
}

fn merge_env(
    input: &[McpEnvInput],
    existing: &std::collections::HashMap<String, String>,
) -> Vec<McpEnvInput> {
    input
        .iter()
        .map(|item| McpEnvInput {
            key: item.key.clone(),
            value: if item.value.is_empty() {
                existing.get(&item.key).cloned().unwrap_or_default()
            } else {
                item.value.clone()
            },
        })
        .collect()
}

fn raw_env_from_toml(env: &toml_edit::Table) -> Vec<McpEnvInput> {
    env.iter()
        .filter_map(|(key, value)| {
            value.as_str().map(|v| McpEnvInput {
                key: key.to_string(),
                value: v.to_string(),
            })
        })
        .collect()
}

fn raw_env_from_json(env: &serde_json::Map<String, serde_json::Value>) -> Vec<McpEnvInput> {
    env.iter()
        .filter_map(|(key, value)| {
            value.as_str().map(|v| McpEnvInput {
                key: key.clone(),
                value: v.to_string(),
            })
        })
        .collect()
}

fn write_pretty_json(path: &Path, value: &serde_json::Value) -> Result<(), AppError> {
    let content = serde_json::to_string_pretty(value)
        .map_err(|e| AppError::internal(format!("serialize MCP JSON: {e}")))?;
    write_config(path, format!("{content}\n").as_bytes(), "MCP")
}

/// 原子写 MCP 配置(保留原文件权限)。
fn write_config(path: &Path, bytes: &[u8], label: &str) -> Result<(), AppError> {
    crate::fsutil::atomic_write(path, bytes, None).map_err(|e| {
        AppError::new(
            crate::errors::codes::MCP_CONFIG_WRITE_FAILED,
            format!("Cannot write {label} MCP config {}: {e}", path.display()),
        )
    })
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn toml_string_array(values: &[String]) -> String {
    let items = values.iter().map(|v| toml_string(v)).collect::<Vec<_>>();
    format!("[{}]", items.join(", "))
}

fn server(
    client: &str,
    config_path: &str,
    name: &str,
    command: String,
    args: Vec<String>,
    env: Vec<McpEnvVar>,
) -> McpServer {
    McpServer {
        id: format!("{client}:{name}"),
        name: name.to_string(),
        transport: "stdio".to_string(),
        validation: validate_server(name, &command, &args, &env),
        command,
        args,
        env,
        enabled_clients: vec![client.to_string()],
        sources: vec![McpServerSource {
            client: client.to_string(),
            source: "client_config".to_string(),
            config_path: config_path.to_string(),
            raw_name: name.to_string(),
        }],
    }
}

fn config_error_server(client: &str, config_path: &str, message: String) -> McpServer {
    McpServer {
        id: format!("{client}:__config__"),
        name: "__config__".to_string(),
        transport: "stdio".to_string(),
        command: String::new(),
        args: Vec::new(),
        env: Vec::new(),
        enabled_clients: vec![client.to_string()],
        sources: vec![McpServerSource {
            client: client.to_string(),
            source: "client_config".to_string(),
            config_path: config_path.to_string(),
            raw_name: "__config__".to_string(),
        }],
        validation: McpValidationState {
            status: "invalid".to_string(),
            issues: vec![issue("client_config_invalid", &message, None)],
        },
    }
}

fn env_var(key: &str, value: Option<&str>) -> McpEnvVar {
    McpEnvVar {
        key: key.to_string(),
        value: None,
        is_sensitive: is_sensitive_env_key(key),
        has_value: value.map(|v| !v.is_empty()).unwrap_or(false),
    }
}

fn is_sensitive_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    SENSITIVE_ENV_MARKERS
        .iter()
        .any(|marker| upper.contains(marker))
}

fn validate_server(
    name: &str,
    command: &str,
    _args: &[String],
    env: &[McpEnvVar],
) -> McpValidationState {
    let mut issues = Vec::new();
    let mut has_invalid_issue = false;
    if name.trim().is_empty() {
        issues.push(issue(
            "name_required",
            "MCP server name is required",
            Some("name"),
        ));
        has_invalid_issue = true;
    }
    if command.trim().is_empty() {
        issues.push(issue(
            "command_required",
            "MCP server command is required",
            Some("command"),
        ));
        has_invalid_issue = true;
    } else if command.contains('/') {
        let path = PathBuf::from(command);
        if path.is_absolute() && !path.exists() {
            issues.push(issue(
                "command_not_found",
                "MCP server command path does not exist",
                Some("command"),
            ));
            has_invalid_issue = true;
        }
    } else if !command_in_path(command) {
        issues.push(issue(
            "command_not_found",
            "MCP server command was not found in PATH",
            Some("command"),
        ));
    }

    for item in env {
        if item.key.trim().is_empty() || item.key.contains('=') {
            issues.push(issue(
                "env_key_invalid",
                "MCP env key must be non-empty and must not contain '='",
                Some("env"),
            ));
            has_invalid_issue = true;
        }
        if !item.has_value {
            issues.push(issue(
                "env_value_missing",
                "MCP env value is missing",
                Some("env"),
            ));
        }
    }

    let status = if has_invalid_issue {
        "invalid"
    } else if issues.is_empty() {
        "valid"
    } else {
        "warning"
    };

    McpValidationState {
        status: status.to_string(),
        issues,
    }
}

fn issue(code: &str, message: &str, field: Option<&str>) -> McpValidationIssue {
    McpValidationIssue {
        code: code.to_string(),
        message: message.to_string(),
        field: field.map(String::from),
    }
}

fn command_in_path(command: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    let dirs: Vec<PathBuf> = std::env::split_paths(&paths).collect();
    command_in_dirs(command, &dirs, &path_extensions())
}

/// Windows 上 `npx` 实际是 `npx.cmd`,需要按 PATHEXT 逐个补扩展名查找;
/// 其它平台只按原名查找。
fn path_extensions() -> Vec<String> {
    if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    }
}

fn command_in_dirs(command: &str, dirs: &[PathBuf], extensions: &[String]) -> bool {
    dirs.iter().any(|dir| {
        dir.join(command).is_file()
            || extensions.iter().any(|ext| {
                dir.join(format!("{command}{ext}")).is_file()
                    || dir
                        .join(format!("{command}{}", ext.to_ascii_lowercase()))
                        .is_file()
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lookup_honours_path_extensions() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("npx.cmd"), "@echo off").unwrap();
        let dirs = vec![temp.path().to_path_buf()];
        assert!(!command_in_dirs("npx", &dirs, &[]));
        assert!(command_in_dirs(
            "npx",
            &dirs,
            &[".EXE".to_string(), ".CMD".to_string()]
        ));
        assert!(command_in_dirs("npx.cmd", &dirs, &[]));
        assert!(!command_in_dirs("uvx", &dirs, &[".CMD".to_string()]));
    }

    #[test]
    fn home_falls_back_to_userprofile_on_windows_style_env() {
        // Windows 没有 HOME,home() 返回空路径 → MCP 配置(~/.codex/config.toml
        // 等)读取全部失败。
        crate::test_utils::with_windows_style_home(|fake| {
            assert!(home().unwrap().starts_with(fake));
        });
    }

    /// 回归:~/.claude.json 读不出来(权限 / 非 UTF-8)时,旧代码把它当 "{}" 写回,
    /// 用户的 oauthAccount / projects 全丢。现在必须报错且文件原样不动。
    #[test]
    fn upsert_claude_refuses_to_overwrite_unreadable_config() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".claude.json");
        let original: &[u8] = b"{\"oauthAccount\":{\"id\":1},\"projects\":{}} \xff";
        fs::write(&path, original).unwrap();
        let err = upsert_in_home(
            temp.path(),
            UpsertMcpServerInput {
                client: "claude_code".to_string(),
                name: "pencil".to_string(),
                command: "/bin/echo".to_string(),
                args: vec![],
                env: vec![],
            },
        );
        assert!(err.is_err(), "unreadable config must abort the write");
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "file must stay untouched"
        );
    }

    #[test]
    fn upsert_json_and_opencode_refuse_to_overwrite_unreadable_config() {
        let temp = tempfile::tempdir().unwrap();
        let gemini = temp.path().join(".gemini").join("settings.json");
        let opencode = temp
            .path()
            .join(".config")
            .join("opencode")
            .join("opencode.json");
        let codex = temp.path().join(".codex").join("config.toml");
        for p in [&gemini, &opencode, &codex] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"\xff\xfe keep me").unwrap();
        }
        for client in ["gemini", "opencode", "codex"] {
            let res = upsert_in_home(
                temp.path(),
                UpsertMcpServerInput {
                    client: client.to_string(),
                    name: "fs".to_string(),
                    command: "npx".to_string(),
                    args: vec![],
                    env: vec![],
                },
            );
            assert!(res.is_err(), "{client}: unreadable config must abort");
        }
        for p in [&gemini, &opencode, &codex] {
            assert_eq!(fs::read(p).unwrap(), b"\xff\xfe keep me");
        }
    }

    fn input_named(client: &str, name: &str) -> UpsertMcpServerInput {
        UpsertMcpServerInput {
            client: client.to_string(),
            name: name.to_string(),
            command: "/bin/echo".to_string(),
            args: vec![],
            env: vec![],
        }
    }

    /// 回归:名字里的空格 / `#` / 引号 / 换行会写出非法的 `[mcp_servers.<name>]`
    /// header,`#` 还会让 header_matches 截断后匹配到别的段造成重复段。
    /// 裸 key 限制只针对 Codex TOML。
    #[test]
    fn upsert_rejects_names_that_break_toml_headers() {
        let temp = tempfile::tempdir().unwrap();
        let too_long = "x".repeat(65);
        for bad in [
            "my server",
            "a#b",
            "a\"b",
            "a\nb",
            "a/b",
            "a.b",
            "@scope/pkg",
            too_long.as_str(),
        ] {
            let err = upsert_in_home(temp.path(), input_named("codex", bad)).unwrap_err();
            assert_eq!(err.code, "VALIDATION_ERROR", "codex accepted {bad:?}");
        }
        assert!(!temp.path().join(".codex").join("config.toml").exists());

        let max = "a".repeat(64);
        for good in ["node_repl", "fs-1", "A9", max.as_str()] {
            upsert_in_home(temp.path(), input_named("codex", good)).unwrap();
        }
    }

    /// JSON 客户端的 key 可以是任意字符串:已有的 `my.server` / `@scope/pkg` /
    /// 含空格的名字必须能编辑;只拒绝空名与控制字符(换行等)。
    #[test]
    fn json_clients_accept_nonbare_names_but_reject_empty_and_control_chars() {
        let temp = tempfile::tempdir().unwrap();
        for client in ["claude_code", "gemini", "opencode"] {
            for good in ["my.server", "my server", "@scope/pkg"] {
                let written = upsert_in_home(temp.path(), input_named(client, good))
                    .unwrap_or_else(|e| panic!("{client} rejected {good:?}: {}", e.message));
                assert_eq!(written.name, good);
            }
            for bad in ["", "  ", "a\nb", "a\tb", "a\u{7f}b"] {
                let err = upsert_in_home(temp.path(), input_named(client, bad)).unwrap_err();
                assert_eq!(err.code, "VALIDATION_ERROR", "{client} accepted {bad:?}");
            }
        }
    }

    #[test]
    fn sync_json_server_with_dotted_name_between_json_clients() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join(".claude.json"),
            r#"{"mcpServers":{"my.server":{"command":"/bin/echo"}}}"#,
        )
        .unwrap();
        let out = sync_in_home(
            temp.path(),
            SyncMcpServerInput {
                from_client: "claude_code".into(),
                name: "my.server".into(),
                to_clients: vec!["gemini".into()],
            },
        )
        .unwrap();
        assert_eq!(out[0].id, "gemini:my.server");
    }

    /// 同步到多个客户端时,任何一个目标校验不过就一个都不写。
    #[test]
    fn sync_validates_all_targets_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join(".claude.json"),
            r#"{"mcpServers":{"my.server":{"command":"/bin/echo"}}}"#,
        )
        .unwrap();
        let err = sync_in_home(
            temp.path(),
            SyncMcpServerInput {
                from_client: "claude_code".into(),
                name: "my.server".into(),
                to_clients: vec!["gemini".into(), "codex".into()],
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
        assert!(!temp.path().join(".gemini").join("settings.json").exists());
        assert!(!temp.path().join(".codex").join("config.toml").exists());
    }

    /// import 先校验全部条目,任何一条不合法就什么都不写(不能写一半)。
    #[test]
    fn import_validates_all_entries_before_writing_anything() {
        let temp = tempfile::tempdir().unwrap();
        let entry = |name: &str| McpExportServer {
            name: name.to_string(),
            command: "/bin/echo".to_string(),
            args: vec![],
            env: vec![],
            clients: vec!["claude_code".to_string(), "codex".to_string()],
        };
        let payload = serde_json::to_string(&McpExport {
            version: 1,
            servers: vec![entry("good_one"), entry("bad.name")],
        })
        .unwrap();
        let err = import_in_home(temp.path(), &payload, vec![]).unwrap_err();
        assert_eq!(err.code, "VALIDATION_ERROR");
        assert!(!temp.path().join(".claude.json").exists());
        assert!(!temp.path().join(".codex").join("config.toml").exists());

        let payload = serde_json::to_string(&McpExport {
            version: 1,
            servers: vec![entry("good_one"), entry("")],
        })
        .unwrap();
        assert!(import_in_home(temp.path(), &payload, vec!["claude_code".into()]).is_err());
        assert!(!temp.path().join(".claude.json").exists());
    }

    #[test]
    fn listing_still_reads_existing_nonconforming_names() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join(".claude.json"),
            r#"{"mcpServers":{"my server":{"command":"/bin/echo"}}}"#,
        )
        .unwrap();
        let listed = list_all_from_home(temp.path());
        assert!(listed.iter().any(|s| s.name == "my server"));
        assert!(delete_in_home(temp.path(), "claude_code", "my server").unwrap());
    }

    #[test]
    fn parses_codex_toml_mcp_block() {
        let toml = r#"
model_provider = "OpenAI"

[mcp_servers.node_repl]
command = "/path/node_repl"
args = ["--port", "1234"]

[mcp_servers.node_repl.env]
TOKEN = "secret"
PATH_HINT = "/x"
"#;
        let doc = toml.parse::<toml_edit::DocumentMut>().unwrap();
        let servers = doc.get("mcp_servers").and_then(|v| v.as_table()).unwrap();
        let (name, val) = servers.iter().next().unwrap();
        let tbl = val.as_table().unwrap();
        assert_eq!(name, "node_repl");
        assert_eq!(
            tbl.get("command").and_then(|v| v.as_str()),
            Some("/path/node_repl")
        );
        let env: Vec<String> = tbl
            .get("env")
            .and_then(|v| v.as_table())
            .map(|e| e.iter().map(|(k, _)| k.to_string()).collect())
            .unwrap_or_default();
        // env 只取 key,不含 "secret" 这个 value
        assert!(env.contains(&"TOKEN".to_string()));
        assert!(!env.iter().any(|k| k == "secret"));
    }

    #[test]
    fn parses_claude_json_mcp_block() {
        let json = serde_json::json!({
            "mcpServers": {
                "pencil": { "command": "/p/mcp", "args": ["--app", "cursor"] }
            }
        });
        let servers = json.get("mcpServers").and_then(|v| v.as_object()).unwrap();
        let (name, val) = servers.iter().next().unwrap();
        assert_eq!(name, "pencil");
        assert_eq!(val.get("command").and_then(|v| v.as_str()), Some("/p/mcp"));
        let args: Vec<String> = val
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(args, vec!["--app", "cursor"]);
    }

    #[test]
    fn list_all_returns_unified_model_without_env_values() {
        let temp = tempfile::tempdir().unwrap();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("config.toml"),
            r#"
[mcp_servers.node_repl]
command = "/bin/echo"
args = ["hello"]

[mcp_servers.node_repl.env]
TOKEN = "secret-token"
PLAIN = "visible-value"
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join(".claude.json"),
            r#"{
  "mcpServers": {
    "pencil": {
      "command": "/bin/echo",
      "args": ["world"],
      "env": { "API_KEY": "secret-key" }
    }
  }
}"#,
        )
        .unwrap();

        let servers = list_all_from_home(temp.path());

        let json = serde_json::to_value(&servers).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);

        let codex = arr.iter().find(|s| s["name"] == "node_repl").unwrap();
        assert_eq!(codex["id"], "codex:node_repl");
        assert_eq!(codex["transport"], "stdio");
        assert_eq!(codex["enabled_clients"], serde_json::json!(["codex"]));
        assert_eq!(codex["sources"][0]["client"], "codex");
        assert_eq!(codex["validation"]["status"], "valid");
        assert_eq!(codex["env"][0]["has_value"], true);
        assert!(codex["env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["key"] == "TOKEN" && e["is_sensitive"] == true));
        assert!(!json.to_string().contains("secret-token"));
        assert!(!json.to_string().contains("visible-value"));

        let claude = arr.iter().find(|s| s["name"] == "pencil").unwrap();
        assert_eq!(claude["id"], "claude_code:pencil");
        assert_eq!(
            claude["enabled_clients"],
            serde_json::json!(["claude_code"])
        );
        assert!(claude["env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["key"] == "API_KEY" && e["is_sensitive"] == true));
        assert!(!json.to_string().contains("secret-key"));
    }

    #[test]
    fn upsert_and_delete_codex_mcp_server_in_temp_home() {
        let temp = tempfile::tempdir().unwrap();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("config.toml"),
            "model = \"gpt-5\"\n\n[mcp_servers.old]\ncommand = \"old\"\n",
        )
        .unwrap();

        upsert_in_home(
            temp.path(),
            UpsertMcpServerInput {
                client: "codex".to_string(),
                name: "node_repl".to_string(),
                command: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                env: vec![McpEnvInput {
                    key: "TOKEN".to_string(),
                    value: "secret-token".to_string(),
                }],
            },
        )
        .unwrap();

        let content = fs::read_to_string(codex_dir.join("config.toml")).unwrap();
        assert!(content.contains("model = \"gpt-5\""));
        assert!(content.contains("[mcp_servers.old]"));
        assert!(content.contains("[mcp_servers.node_repl]"));
        assert!(content.contains("command = \"/bin/echo\""));
        assert!(content.contains("args = [\"hello\"]"));
        assert!(content.contains("[mcp_servers.node_repl.env]"));
        assert!(content.contains("TOKEN = \"secret-token\""));

        let listed = serde_json::to_string(&list_all_from_home(temp.path())).unwrap();
        assert!(listed.contains("node_repl"));
        assert!(!listed.contains("secret-token"));

        delete_in_home(temp.path(), "codex", "node_repl").unwrap();
        let after = fs::read_to_string(codex_dir.join("config.toml")).unwrap();
        assert!(!after.contains("[mcp_servers.node_repl]"));
        assert!(!after.contains("[mcp_servers.node_repl.env]"));
        assert!(after.contains("[mcp_servers.old]"));
    }

    #[test]
    fn upsert_and_delete_claude_mcp_server_in_temp_home() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join(".claude.json"),
            r#"{"theme":"dark","mcpServers":{"old":{"command":"old"}}}"#,
        )
        .unwrap();

        upsert_in_home(
            temp.path(),
            UpsertMcpServerInput {
                client: "claude_code".to_string(),
                name: "pencil".to_string(),
                command: "/bin/echo".to_string(),
                args: vec!["world".to_string()],
                env: vec![McpEnvInput {
                    key: "API_KEY".to_string(),
                    value: "secret-key".to_string(),
                }],
            },
        )
        .unwrap();

        let content = fs::read_to_string(temp.path().join(".claude.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["theme"], "dark");
        assert_eq!(json["mcpServers"]["old"]["command"], "old");
        assert_eq!(json["mcpServers"]["pencil"]["command"], "/bin/echo");
        assert_eq!(
            json["mcpServers"]["pencil"]["args"],
            serde_json::json!(["world"])
        );
        assert_eq!(json["mcpServers"]["pencil"]["env"]["API_KEY"], "secret-key");

        let listed = serde_json::to_string(&list_all_from_home(temp.path())).unwrap();
        assert!(listed.contains("pencil"));
        assert!(!listed.contains("secret-key"));

        delete_in_home(temp.path(), "claude_code", "pencil").unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(temp.path().join(".claude.json")).unwrap())
                .unwrap();
        assert!(after["mcpServers"].get("pencil").is_none());
        assert_eq!(after["mcpServers"]["old"]["command"], "old");
    }

    #[test]
    fn upsert_preserves_existing_env_value_when_input_value_is_blank() {
        let temp = tempfile::tempdir().unwrap();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("config.toml"),
            r#"
[mcp_servers.node_repl]
command = "/bin/echo"
args = ["old"]

[mcp_servers.node_repl.env]
TOKEN = "secret-token"
"#,
        )
        .unwrap();

        upsert_in_home(
            temp.path(),
            UpsertMcpServerInput {
                client: "codex".to_string(),
                name: "node_repl".to_string(),
                command: "/bin/echo".to_string(),
                args: vec!["new".to_string()],
                env: vec![McpEnvInput {
                    key: "TOKEN".to_string(),
                    value: "".to_string(),
                }],
            },
        )
        .unwrap();

        let content = fs::read_to_string(codex_dir.join("config.toml")).unwrap();
        assert!(content.contains("args = [\"new\"]"));
        assert!(content.contains("TOKEN = \"secret-token\""));
    }

    #[test]
    fn sync_copies_codex_server_to_claude_with_env_value_hidden_from_list() {
        let temp = tempfile::tempdir().unwrap();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("config.toml"),
            r#"
[mcp_servers.node_repl]
command = "/bin/echo"
args = ["hello"]

[mcp_servers.node_repl.env]
TOKEN = "secret-token"
"#,
        )
        .unwrap();

        let synced = sync_in_home(
            temp.path(),
            SyncMcpServerInput {
                from_client: "codex".to_string(),
                name: "node_repl".to_string(),
                to_clients: vec!["claude_code".to_string()],
            },
        )
        .unwrap();

        assert_eq!(synced.len(), 1);
        assert_eq!(synced[0].id, "claude_code:node_repl");

        let claude_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(temp.path().join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(
            claude_json["mcpServers"]["node_repl"]["command"],
            "/bin/echo"
        );
        assert_eq!(
            claude_json["mcpServers"]["node_repl"]["args"],
            serde_json::json!(["hello"])
        );
        assert_eq!(
            claude_json["mcpServers"]["node_repl"]["env"]["TOKEN"],
            "secret-token"
        );

        let listed = serde_json::to_string(&list_all_from_home(temp.path())).unwrap();
        assert!(listed.contains("claude_code:node_repl"));
        assert!(!listed.contains("secret-token"));
    }

    #[test]
    fn invalid_client_configs_are_reported_as_validation_entries() {
        let temp = tempfile::tempdir().unwrap();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(codex_dir.join("config.toml"), "[mcp_servers.bad\n").unwrap();
        fs::write(temp.path().join(".claude.json"), "{ bad json").unwrap();

        let servers = list_all_from_home(temp.path());
        assert_eq!(servers.len(), 2);
        assert!(servers.iter().any(|s| {
            s.id == "codex:__config__"
                && s.validation.status == "invalid"
                && s.validation
                    .issues
                    .iter()
                    .any(|i| i.code == "client_config_invalid")
        }));
        assert!(servers.iter().any(|s| {
            s.id == "claude_code:__config__"
                && s.validation.status == "invalid"
                && s.validation
                    .issues
                    .iter()
                    .any(|i| i.code == "client_config_invalid")
        }));
    }

    #[test]
    fn export_hides_secret_values_by_default_and_can_include_them() {
        let temp = tempfile::tempdir().unwrap();
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("config.toml"),
            r#"
[mcp_servers.node_repl]
command = "/bin/echo"
args = ["hello"]

[mcp_servers.node_repl.env]
TOKEN = "secret-token"
"#,
        )
        .unwrap();

        let safe = export_from_home(temp.path(), false).unwrap();
        assert!(safe.contains("node_repl"));
        assert!(!safe.contains("secret-token"));
        let safe_json: McpExport = serde_json::from_str(&safe).unwrap();
        assert_eq!(safe_json.servers[0].env[0].value, None);
        assert!(safe_json.servers[0].env[0].has_value);

        let with_secrets = export_from_home(temp.path(), true).unwrap();
        assert!(with_secrets.contains("secret-token"));
        let secret_json: McpExport = serde_json::from_str(&with_secrets).unwrap();
        assert_eq!(
            secret_json.servers[0].env[0].value.as_deref(),
            Some("secret-token")
        );
    }

    #[test]
    fn import_writes_payload_to_selected_clients() {
        let temp = tempfile::tempdir().unwrap();
        let payload = serde_json::to_string(&McpExport {
            version: 1,
            servers: vec![McpExportServer {
                name: "node_repl".to_string(),
                command: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                env: vec![McpExportEnv {
                    key: "TOKEN".to_string(),
                    value: Some("secret-token".to_string()),
                    has_value: true,
                    is_sensitive: true,
                }],
                clients: vec!["codex".to_string()],
            }],
        })
        .unwrap();

        let imported =
            import_in_home(temp.path(), &payload, vec!["claude_code".to_string()]).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].id, "claude_code:node_repl");
        let claude_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(temp.path().join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(
            claude_json["mcpServers"]["node_repl"]["env"]["TOKEN"],
            "secret-token"
        );
    }

    #[test]
    fn upsert_and_list_gemini_and_opencode_mcp() {
        let temp = tempfile::tempdir().unwrap();
        upsert_in_home(
            temp.path(),
            UpsertMcpServerInput {
                client: "gemini".to_string(),
                name: "github".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                env: vec![],
            },
        )
        .unwrap();
        upsert_in_home(
            temp.path(),
            UpsertMcpServerInput {
                client: "opencode".to_string(),
                name: "fs".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".into(), "mcp-filesystem".into()],
                env: vec![],
            },
        )
        .unwrap();
        let listed = list_all_from_home(temp.path());
        assert!(listed.iter().any(|s| s.id == "gemini:github"));
        assert!(listed.iter().any(|s| s.id == "opencode:fs"));
        let oc: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(
                temp.path()
                    .join(".config")
                    .join("opencode")
                    .join("opencode.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(oc["mcp"]["fs"]["type"], "local");
        assert_eq!(
            oc["mcp"]["fs"]["command"],
            serde_json::json!(["npx", "-y", "mcp-filesystem"])
        );
        let gemini: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(temp.path().join(".gemini").join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(gemini["mcpServers"]["github"]["command"], "npx");
    }
}
