//! MCP stdio Server：把第一阶段 Agent Service 暴露成标准 MCP 工具。
//!
//! 本层只做四件事：请求解析、参数校验、调用 Agent Service（经本地 IPC）、结果序列化/错误映射。
//! 它不持有任务状态机，不重试业务错误，不新增 Agent 语义。
//!
//! stdout 只允许协议消息，所有日志走 stderr（CLI 启动时把 tracing 指向 stderr）。
use super::protocol::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

/// 支持的 MCP 协议版本；客户端请求不在其中时回落到最新支持版本。
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
const PREFERRED_PROTOCOL_VERSION: &str = "2025-06-18";

const JSONRPC_INVALID_PARAMS: i64 = -32602;
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
const JSONRPC_PARSE_ERROR: i64 = -32700;

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

/// 维持一条到 Agent Service 的 IPC 连接；断线只重连一次，不做业务重试。
pub struct Gateway {
    db_dir: PathBuf,
    executable: PathBuf,
    client: Option<super::ipc::IpcClient>,
}

impl Gateway {
    pub fn new(db_dir: PathBuf, executable: PathBuf) -> Self {
        Self {
            db_dir,
            executable,
            client: None,
        }
    }

    async fn connect(&mut self) -> Result<(), super::protocol::AgentError> {
        let client = super::ipc::ensure_service(&self.db_dir, &self.executable).await?;
        self.client = Some(client);
        Ok(())
    }

    /// 返回 `(业务结果, 是否传输层失败)`。
    pub async fn call(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, super::protocol::AgentError> {
        if self.client.is_none() {
            self.connect().await?;
        }
        let first = self
            .client
            .as_mut()
            .expect("client connected")
            .call(method, params.clone())
            .await;
        match first {
            Ok(Ok(value)) => Ok(value),
            // 服务端明确回答的业务错误：原样返回，不重连、不掩盖。
            Ok(Err(error)) => Ok(error_json(error)),
            Err(transport) => {
                // 传输断开：重连一次（服务本身可以比 MCP 客户端活得更久）。
                self.client = None;
                self.connect().await.map_err(|e| {
                    super::protocol::AgentError::new(
                        e.code,
                        format!("agent service unavailable after reconnect: {}", e.message),
                    )
                })?;
                match self
                    .client
                    .as_mut()
                    .expect("client connected")
                    .call(method, params)
                    .await
                {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(error)) => Ok(error_json(error)),
                    Err(e) => Err(super::protocol::AgentError::new(
                        e.code,
                        format!("{} (transport error: {})", e.message, transport.message),
                    )),
                }
            }
        }
    }
}

fn error_json(error: super::protocol::AgentError) -> Value {
    json!({ "error": { "code": error.code, "message": error.message } })
}

// ------------------------------------------------------------- tool registry

struct Tool {
    name: &'static str,
    description: &'static str,
    input_schema: Value,
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "agent_list",
            description: "List the coding agents (Codex / Claude Code) available through MuxLayer.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        Tool {
            name: "agent_run",
            description: "Submit a read-only audit task to an agent. Returns task_id/session_id immediately; \
                          the task keeps running even if this MCP client exits. Poll with agent_status.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "agent": {"type": "string", "description": "Agent id from agent_list, e.g. codex-native or claude-code"},
                    "cwd": {"type": "string", "description": "Absolute path of the repository/directory to inspect"},
                    "prompt": {"type": "string", "description": "Task prompt"},
                    "permission": {"type": "string", "enum": ["read_only"], "default": "read_only"}
                },
                "required": ["agent", "cwd", "prompt"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "agent_continue",
            description: "Continue an existing session (same provider thread) with a new prompt.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": {"type": "string"},
                    "prompt": {"type": "string"}
                },
                "required": ["session_id", "prompt"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "agent_status",
            description: "Query a task. Returns the complete TaskRecord plus explicit pagination metadata. \
                          Long final answers are never truncated silently: pass final_answer_offset/final_answer_limit \
                          (in Unicode characters) and concatenate the slices. Events are omitted unless events_limit is given.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "final_answer_offset": {"type": "integer", "minimum": 0},
                    "final_answer_limit": {"type": "integer", "minimum": 0},
                    "events_offset": {"type": "integer", "minimum": 0},
                    "events_limit": {"type": "integer", "minimum": 0}
                },
                "required": ["task_id"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "agent_cancel",
            description: "Request cancellation of a running task (real provider interrupt). \
                          The response is not the final state: keep polling agent_status until it is terminal.",
            input_schema: json!({
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"],
                "additionalProperties": false
            }),
        },
    ]
}

fn require_string(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("missing required string argument `{key}`"))
}

fn require_usize(arguments: &Value, key: &str) -> Result<Option<usize>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|value| Some(value as usize))
            .ok_or_else(|| format!("argument `{key}` must be a non-negative integer")),
    }
}

/// MCP 工具名 → IPC 方法与参数。只做校验与搬运。
fn tool_call_params(name: &str, arguments: &Value) -> Result<(&'static str, Value), String> {
    if !arguments.is_null() && !arguments.is_object() {
        return Err("arguments must be a JSON object".into());
    }
    match name {
        "agent_list" => Ok(("agent_list", json!({}))),
        "agent_run" => {
            let permission = arguments
                .get("permission")
                .and_then(Value::as_str)
                .unwrap_or("read_only");
            if permission != "read_only" {
                return Err(format!("unsupported permission `{permission}`"));
            }
            Ok((
                "agent_run",
                json!({
                    "agent": require_string(arguments, "agent")?,
                    "cwd": require_string(arguments, "cwd")?,
                    "prompt": require_string(arguments, "prompt")?,
                    "permission": "read_only",
                }),
            ))
        }
        "agent_continue" => Ok((
            "agent_continue",
            json!({
                "session_id": require_string(arguments, "session_id")?,
                "prompt": require_string(arguments, "prompt")?,
            }),
        )),
        "agent_status" => {
            let mut params = json!({ "task_id": require_string(arguments, "task_id")? });
            for (argument, field) in [
                ("final_answer_offset", "final_answer_offset"),
                ("final_answer_limit", "final_answer_limit"),
                ("events_offset", "events_offset"),
                ("events_limit", "events_limit"),
            ] {
                if let Some(value) = require_usize(arguments, argument)? {
                    params[field] = value.into();
                }
            }
            Ok(("agent_status", params))
        }
        "agent_cancel" => Ok((
            "agent_cancel",
            json!({ "task_id": require_string(arguments, "task_id")? }),
        )),
        other => Err(format!("unknown tool `{other}`")),
    }
}

fn tool_text(content: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&content).unwrap_or_else(|_| "{}".into());
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

async fn handle_call(gateway: &mut Gateway, id: Option<Value>, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let (method, forwarded) = match tool_call_params(name, &arguments) {
        Ok(forwarded) => forwarded,
        Err(message) => {
            return rpc_error(id, JSONRPC_INVALID_PARAMS, message);
        }
    };
    match gateway.call(method, forwarded).await {
        Ok(value) => {
            // `agent_*` 业务错误由服务端以 {"error":{"code","message"}} 明确回答，
            // 这仍然是一次成功的工具调用；注意 TaskRecord.error 为 null 时不算错误。
            let is_error = value
                .get("error")
                .and_then(|error| error.get("code"))
                .is_some();
            rpc_result(id, tool_text(value, is_error))
        }
        Err(transport) => {
            tracing::error!(method, error = %transport.message, "agent service transport failure");
            rpc_result(
                id,
                tool_text(
                    json!({
                        "error": {
                            "code": transport.code,
                            "message": format!("agent service unavailable: {}", transport.message),
                        }
                    }),
                    true,
                ),
            )
        }
    }
}

fn rpc_result(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
}

fn rpc_error(id: Option<Value>, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": RpcError { code, message: message.into(), data: None },
    })
}

async fn handle_request(gateway: &mut Gateway, request: RpcRequest) -> Option<Value> {
    let id = request.id.clone();
    match request.method.as_str() {
        "initialize" => {
            let requested = request
                .params
                .as_ref()
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str)
                .unwrap_or(PREFERRED_PROTOCOL_VERSION);
            let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
                requested
            } else {
                PREFERRED_PROTOCOL_VERSION
            };
            Some(rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "muxlayer-agent-gateway", "version": super::ipc::SERVICE_VERSION },
                    "instructions": "Read-only coding agent gateway. agent_run returns immediately; poll agent_status until the task is terminal and read final_answer (paginated when needed).",
                }),
            ))
        }
        // 通知类消息没有响应。
        "notifications/initialized" | "notifications/cancelled" | "notifications/progress" => None,
        "ping" => Some(rpc_result(id, json!({}))),
        "logging/setLevel" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(
            id,
            json!({
                "tools": tools()
                    .into_iter()
                    .map(|tool| json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    }))
                    .collect::<Vec<_>>()
            }),
        )),
        "tools/call" => {
            let params = request.params.unwrap_or(json!({}));
            if params.get("name").and_then(Value::as_str).is_none() {
                return Some(rpc_error(id, JSONRPC_INVALID_PARAMS, "missing tool name"));
            }
            Some(handle_call(gateway, id, &params).await)
        }
        other => Some(rpc_error(
            id,
            JSONRPC_METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        )),
    }
}

/// 处理一条 MCP 消息（JSON-RPC）。返回 `None` 表示这是通知，不需要响应。
/// 独立出来便于用真实 stdio 之外的测试客户端直接驱动同一套逻辑。
pub async fn handle_message(gateway: &mut Gateway, message: &str) -> Option<Value> {
    match serde_json::from_str::<RpcRequest>(message) {
        Ok(request) => handle_request(gateway, request).await,
        Err(e) => Some(rpc_error(
            None,
            JSONRPC_PARSE_ERROR,
            format!("invalid JSON-RPC message: {e}"),
        )),
    }
}

/// MCP stdio 主循环：stdout 只写协议消息，日志一律 stderr。
pub async fn serve_stdio(mut gateway: Gateway) -> Result<(), super::protocol::AgentError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|e| super::protocol::AgentError::new(ErrorCode::Adapter, e.to_string()))?;
        if read == 0 {
            tracing::info!("MCP client closed stdin; exiting");
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = handle_message(&mut gateway, trimmed).await;
        if let Some(response) = response {
            let mut encoded = serde_json::to_string(&response).map_err(|e| {
                super::protocol::AgentError::new(ErrorCode::Protocol, e.to_string())
            })?;
            encoded.push('\n');
            stdout
                .write_all(encoded.as_bytes())
                .await
                .map_err(|e| super::protocol::AgentError::new(ErrorCode::Adapter, e.to_string()))?;
            stdout
                .flush()
                .await
                .map_err(|e| super::protocol::AgentError::new(ErrorCode::Adapter, e.to_string()))?;
        }
    }
}
