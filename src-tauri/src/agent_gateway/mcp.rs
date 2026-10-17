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
                // 请求可能已被服务执行。只有只读查询可以安全重放。
                if !matches!(method, "agent_list" | "agent_status" | "ping") {
                    return Err(super::protocol::AgentError::new(
                        transport.code,
                        format!("{method} outcome is unknown after transport failure; query task status before submitting again: {}", transport.message),
                    ));
                }
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
            // `agent_*` 业务错误由服务端以 {"error":{"code","message"}} 明确回答。
            // TaskRecord（status / cancel）自身的 error 是任务数据，不能把一次成功的
            // 状态查询标成工具失败——取消或失败的任务仍必须可被客户端读取。
            let is_task_record = value.get("task_id").is_some() && value.get("state").is_some();
            let is_error = !is_task_record
                && value
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

const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

async fn read_stdio_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut String,
) -> Result<usize, super::protocol::AgentError> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let read = reader
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_line(line)
        .await
        .map_err(|e| super::protocol::AgentError::new(ErrorCode::Adapter, e.to_string()))?;
    if line.len() > MAX_MESSAGE_BYTES {
        return Err(super::protocol::AgentError::new(
            ErrorCode::InvalidRequest,
            format!("MCP message exceeds {MAX_MESSAGE_BYTES} bytes"),
        ));
    }
    Ok(read)
}

/// MCP stdio 主循环：stdout 只写协议消息，日志一律 stderr。
pub async fn serve_stdio(mut gateway: Gateway) -> Result<(), super::protocol::AgentError> {
    use tokio::io::AsyncWriteExt;
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        let read = read_stdio_line(&mut reader, &mut line).await?;
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

#[cfg(all(test, unix))]
mod phase_three_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn oversized_unterminated_stdio_input_is_rejected_without_waiting_for_eof() {
        let (mut writer, reader) = tokio::io::duplex(8192);
        let producer = tokio::spawn(async move {
            let bytes = vec![b'x'; MAX_MESSAGE_BYTES + 1];
            writer.write_all(&bytes).await.unwrap();
            // 不结束输入，读取器应在达到预算时立即返回。
            std::future::pending::<()>().await;
        });
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            read_stdio_line(&mut reader, &mut line),
        )
        .await;
        producer.abort();
        assert_eq!(
            result
                .expect("oversized input must not wait for EOF")
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
        assert!(line.len() <= MAX_MESSAGE_BYTES + 1);
    }

    #[tokio::test]
    async fn task_record_error_field_does_not_mark_status_or_cancel_as_tool_error() {
        use super::super::ipc::{IpcClient, ServicePaths};
        let dir = tempfile::tempdir().unwrap();
        let mut paths = ServicePaths::new(dir.path());
        paths.socket = dir.path().join("s");
        std::fs::create_dir_all(&paths.dir).unwrap();
        std::fs::write(&paths.token, "test-token").unwrap();
        let listener = tokio::net::UnixListener::bind(&paths.socket).unwrap();
        let record = json!({
            "task_id": "t1", "session_id": "s1", "prompt": "p",
            "state": "cancelled", "thread_id": "th", "turn_id": "tu",
            "final_answer": null,
            "error": {"code": "adapter", "message": "cancelled by request"},
            "events": [], "cancel_requested": true,
            "created_at": "2026-10-17T00:00:00Z", "finished_at": "2026-10-17T00:00:01Z"
        });
        let worker = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let hello: Value = serde_json::from_str(&line).unwrap();
            let response = json!({"id": hello["id"], "result": {}}).to_string() + "\n";
            stream
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
            for _ in 0..4 {
                line.clear();
                stream.read_line(&mut line).await.unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "agent_status" | "agent_cancel" => record.clone(),
                    _ => json!({"error": {"code": "invalid_request", "message": "bad"}}),
                };
                let response = json!({"id": request["id"], "result": result}).to_string() + "\n";
                stream
                    .get_mut()
                    .write_all(response.as_bytes())
                    .await
                    .unwrap();
            }
        });
        let client = IpcClient::connect(&paths).await.unwrap();
        let mut gateway = Gateway {
            db_dir: dir.path().to_owned(),
            executable: dir.path().join("missing-executable"),
            client: Some(client),
        };
        for (sequence, tool) in [(1, "agent_status"), (2, "agent_cancel")] {
            let response = handle_call(
                &mut gateway,
                Some(json!(sequence)),
                &json!({"name": tool, "arguments": {"task_id": "t1"}}),
            )
            .await;
            assert_eq!(response["result"]["isError"], false, "{tool}: {response}");
        }
        let failed_run = handle_call(
            &mut gateway,
            Some(json!(3)),
            &json!({"name": "agent_run", "arguments": {
                "agent": "codex-native", "cwd": "/tmp", "prompt": "x",
                "permission": "read_only"}}),
        )
        .await;
        assert_eq!(failed_run["result"]["isError"], true, "{failed_run}");
        let failed_continue = handle_call(
            &mut gateway,
            Some(json!(4)),
            &json!({"name": "agent_continue", "arguments": {
                "session_id": "s1", "prompt": "x"}}),
        )
        .await;
        assert_eq!(
            failed_continue["result"]["isError"], true,
            "{failed_continue}"
        );
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn lost_mutation_response_is_reported_without_reconnecting_or_resubmitting() {
        for method in ["agent_run", "agent_continue", "agent_cancel"] {
            let dir = tempfile::tempdir().unwrap();
            let mut paths = super::super::ipc::ServicePaths::new(dir.path());
            // 保证 macOS 的 Unix socket 路径不超过 sockaddr_un 上限。
            paths.socket = dir.path().join("s");
            std::fs::create_dir_all(&paths.dir).unwrap();
            std::fs::write(&paths.token, "test-token").unwrap();
            let listener = tokio::net::UnixListener::bind(&paths.socket).unwrap();
            let worker = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                let hello: Value = serde_json::from_str(&line).unwrap();
                let response = json!({"id": hello["id"], "result": {}}).to_string() + "\n";
                stream
                    .get_mut()
                    .write_all(response.as_bytes())
                    .await
                    .unwrap();
                line.clear();
                stream.read_line(&mut line).await.unwrap();
                let accepted: Value = serde_json::from_str(&line).unwrap();
                // 模拟服务已接收操作，但在回复前丢失连接。
                accepted["method"].as_str().unwrap().to_owned()
            });
            let client = super::super::ipc::IpcClient::connect(&paths).await.unwrap();
            let mut gateway = Gateway {
                db_dir: dir.path().to_owned(),
                executable: dir.path().join("missing-executable"),
                client: Some(client),
            };
            let error = gateway.call(method, json!({})).await.unwrap_err();
            assert_eq!(worker.await.unwrap(), method);
            assert_eq!(error.code, ErrorCode::ProcessExited);
            assert!(error.message.contains("outcome is unknown"), "{error}");
        }
    }
}
