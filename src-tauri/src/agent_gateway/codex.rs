//! Public Codex app-server transport; configuration overrides are process-local.
use super::protocol::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot, watch, Mutex},
};
fn protocol(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::Protocol, message)
}
fn validate_disabled_mcp(status: &Value) -> Result<(), AgentError> {
    let servers = status["data"]
        .as_array()
        .ok_or_else(|| protocol("Missing MCP runtime inventory"))?;
    if servers.iter().any(|server| {
        server["runtimeStatus"] != "disabled"
            || server["tools"]
                .as_object()
                .is_none_or(|tools| !tools.is_empty())
    }) {
        return Err(protocol("Codex read-only thread has an active MCP server"));
    }
    Ok(())
}
fn disabled_mcp(servers: &serde_json::Map<String, Value>) -> Value {
    Value::Object(
        servers
            .keys()
            .map(|name| (name.clone(), json!({"enabled":false})))
            .collect(),
    )
}
fn turn_has_full_items(turn: &Value) -> bool {
    turn["itemsView"].is_null() || turn["itemsView"] == "full"
}
fn progress_message(p: &Value) -> String {
    if p["item"]["type"] == "commandExecution" {
        let i = &p["item"];
        return json!({"type":"commandExecution","command":i["command"],"status":i["status"],"exitCode":i["exitCode"]}).to_string();
    }
    p["delta"]
        .as_str()
        .or_else(|| p["item"]["text"].as_str())
        .unwrap_or("")
        .into()
}
fn final_answer(items: &[Value]) -> Option<String> {
    let messages: Vec<_> = items
        .iter()
        .filter(|i| i["type"] == "agentMessage")
        .collect();
    let explicit = messages.iter().any(|i| i["phase"] == "final_answer");
    let text = messages
        .iter()
        .filter(|i| {
            if explicit {
                i["phase"] == "final_answer"
            } else {
                i["phase"].is_null()
            }
        })
        .filter_map(|i| i["text"].as_str())
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
fn validate_native(config: &Value, provider: &str) -> Result<(), AgentError> {
    let p = &config["model_providers"][provider];
    if config["model_provider"] != provider
        || !p["base_url"].is_null()
        || config["chatgpt_base_url"] != "https://chatgpt.com/backend-api/"
        || p["requires_openai_auth"] != true
        || p["wire_api"] != "responses"
    {
        return Err(protocol(
            "Codex native provider isolation could not be verified",
        ));
    }
    for key in [
        "env_key",
        "experimental_bearer_token",
        "auth",
        "http_headers",
        "env_http_headers",
        "gateway_oauth",
        "aws",
        "query_params",
    ] {
        if !p[key].is_null() {
            return Err(protocol(
                "Native provider contains an unexpected authentication/header override",
            ));
        }
    }
    if !config["model_catalog_json"].is_null() || !config["model_catalog_url"].is_null() {
        return Err(protocol(
            "Custom model catalog prevents verification of the official model source",
        ));
    }
    Ok(())
}
#[derive(Default)]
pub struct CodexFactory {
    executable: Option<PathBuf>,
}
impl CodexFactory {
    /// Construction is lazy; use `discover` to validate the public CLI before registration.
    pub fn new(executable: Option<PathBuf>) -> Self {
        Self { executable }
    }
    pub async fn discover(executable: Option<PathBuf>) -> Result<Self, AgentError> {
        Ok(Self::new(Some(discover(executable.as_deref()).await?)))
    }
}
async fn discover(explicit: Option<&Path>) -> Result<PathBuf, AgentError> {
    let mut candidates = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p.to_owned());
    } else {
        if let Some(p) = std::env::var_os("AGENTGATE_CODEX_BIN") {
            candidates.push(p.into());
        }
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                candidates.push(dir.join(if cfg!(windows) { "codex.exe" } else { "codex" }));
            }
        }
    }
    for p in candidates {
        if p.to_string_lossy().contains(".app/Contents/") {
            continue;
        }
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            Command::new(&p)
                .arg("--version")
                .kill_on_drop(true)
                .output(),
        )
        .await;
        if let Ok(Ok(o)) = output {
            if o.status.success() && String::from_utf8_lossy(&o.stdout).starts_with("codex-cli ") {
                return Ok(p);
            }
        }
        if explicit.is_some() {
            break;
        }
    }
    Err(AgentError::new(ErrorCode::Adapter,"Official Codex CLI is unavailable or cannot execute. Install the public Codex CLI or configure AGENTGATE_CODEX_BIN."))
}
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, AgentError>>>>>;
struct CodexAdapter {
    stdin: Arc<Mutex<ChildStdin>>,
    child: Mutex<Child>,
    pending: Pending,
    events: Mutex<mpsc::UnboundedReceiver<Value>>,
    ids: AtomicU64,
    provider: String,
    model: String,
    reader: tokio::task::JoinHandle<()>,
    active: Mutex<Option<(String, String)>>,
    completion: watch::Receiver<Option<(String, String)>>,
}
impl AdapterFactory for CodexFactory {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            id: "codex-native".into(),
            name: "Codex Native".into(),
            permissions: vec![PermissionMode::ReadOnly],
            supports_continuation: true,
            supports_cancel: true,
        }
    }
    fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>> {
        Box::pin(async move {
            Ok(
                Arc::new(CodexAdapter::connect(discover(self.executable.as_deref()).await?).await?)
                    as Arc<dyn AgentAdapter>,
            )
        })
    }
}
impl CodexAdapter {
    async fn connect(executable: PathBuf) -> Result<Self, AgentError> {
        let provider = format!("agentgate_native_{}", uuid::Uuid::new_v4().simple());
        let mut cmd = Command::new(&executable);
        let overrides=vec![format!("model_provider={provider:?}"),format!("model_providers.{provider}={{name=\"OpenAI\",wire_api=\"responses\",requires_openai_auth=true,supports_websockets=false}}"),"chatgpt_base_url=\"https://chatgpt.com/backend-api/\"".into(),"approval_policy=\"never\"".into(),"sandbox_mode=\"read-only\"".into(),"features.apps=false".into(),"features.plugins=false".into(),"features.hooks=false".into(),"web_search=\"disabled\"".into(),"notify=[]".into(),"otel.exporter=\"none\"".into()];
        for value in overrides {
            cmd.arg("-c").arg(value);
        }
        for key in [
            "OPENAI_BASE_URL",
            "CODEX_API_ENDPOINT",
            "CODEX_CHATGPT_BASE_URL",
            "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
            "CODEX_MODEL_PROVIDER",
        ] {
            cmd.env_remove(key);
        }
        cmd.arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .current_dir(std::env::temp_dir())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|_| {
            AgentError::new(ErrorCode::Adapter, "Could not launch Codex app-server")
        })?;
        let stdin = Arc::new(Mutex::new(
            child
                .stdin
                .take()
                .ok_or_else(|| protocol("Missing Codex stdin"))?,
        ));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| protocol("Missing Codex stdout"))?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, events) = mpsc::unbounded_channel();
        let requests = pending.clone();
        let writer = stdin.clone();
        let (completed, completion) = watch::channel(None);
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    let _ = tx.send(json!({"method":"_protocol_error"}));
                    break;
                };
                if msg.get("id").is_some() {
                    if msg.get("method").is_some() {
                        let id = &msg["id"];
                        // No client-mediated approval or dynamic tool is authorized in read-only mode.
                        let response = json!({"id":id,"error":{"code":-32601,"message":"Client-mediated tools are disabled in read-only mode"}});
                        if writer
                            .lock()
                            .await
                            .write_all(format!("{response}\n").as_bytes())
                            .await
                            .is_err()
                        {
                            break;
                        }
                    } else if let Some(waiter) = requests
                        .lock()
                        .await
                        .remove(&msg["id"].as_u64().unwrap_or(0))
                    {
                        let result = if msg.get("error").is_some() {
                            Err(protocol(format!(
                                "Codex RPC rejected request (code {})",
                                msg["error"]["code"]
                            )))
                        } else {
                            Ok(msg["result"].clone())
                        };
                        let _ = waiter.send(result);
                    }
                } else {
                    if msg["method"] == "turn/completed" {
                        if let (Some(thread), Some(turn)) = (
                            msg["params"]["threadId"].as_str(),
                            msg["params"]["turn"]["id"].as_str(),
                        ) {
                            let _ = completed.send(Some((thread.into(), turn.into())));
                        }
                    }
                    if tx.send(msg).is_err() {
                        break;
                    }
                }
            }
            for (_, waiter) in requests.lock().await.drain() {
                let _ = waiter.send(Err(AgentError::new(
                    ErrorCode::ProcessExited,
                    "Codex app-server closed its output",
                )));
            }
            let _ = tx.send(json!({"method":"_exit"}));
        });
        let mut adapter = Self {
            stdin,
            child: Mutex::new(child),
            pending,
            events: Mutex::new(events),
            ids: AtomicU64::new(1),
            provider,
            model: String::new(),
            reader,
            active: Mutex::new(None),
            completion,
        };
        let init=adapter.rpc("initialize",json!({"clientInfo":{"name":"agentgate","title":"AgentGate","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":false}})).await?;
        if init["userAgent"].as_str().is_none() {
            return Err(protocol("Unsupported Codex initialize response"));
        }
        adapter.notify("initialized", json!({})).await?;
        let config = adapter
            .rpc("config/read", json!({"includeLayers":false}))
            .await?;
        validate_native(&config["config"], &adapter.provider)?;
        let models = adapter
            .rpc("model/list", json!({"includeHidden":false}))
            .await?;
        adapter.model = models["data"]
            .as_array()
            .and_then(|a| a.iter().find(|m| m["isDefault"] == true))
            .and_then(|m| m["model"].as_str())
            .ok_or_else(|| protocol("Official Codex catalog has no default model"))?
            .to_owned();
        Ok(adapter)
    }
    async fn notify(&self, method: &str, params: Value) -> Result<(), AgentError> {
        self.stdin
            .lock()
            .await
            .write_all(format!("{}\n", json!({"method":method,"params":params})).as_bytes())
            .await
            .map_err(|_| AgentError::new(ErrorCode::ProcessExited, "Codex input closed"))
    }
    async fn rpc(&self, method: &str, params: Value) -> Result<Value, AgentError> {
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let message = format!("{}\n", json!({"id":id,"method":method,"params":params}));
        if self
            .stdin
            .lock()
            .await
            .write_all(message.as_bytes())
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return Err(AgentError::new(
                ErrorCode::ProcessExited,
                "Codex input closed",
            ));
        }
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => {
                result.map_err(|e| AgentError::new(e.code, format!("{method}: {}", e.message)))
            }
            _ => {
                self.pending.lock().await.remove(&id);
                Err(protocol(format!("Codex {method} did not respond")))
            }
        }
    }
    async fn verify_tools_disabled(&self, thread_id: &str) -> Result<(), AgentError> {
        let mut cursor: Option<String> = None;
        loop {
            let status = self
                .rpc(
                    "mcpServerStatus/list",
                    json!({"threadId":thread_id,"cursor":cursor}),
                )
                .await?;
            validate_disabled_mcp(&status)?;
            cursor = status["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                return Ok(());
            }
        }
    }
    async fn thread_config(&self, cwd: &Path) -> Result<Value, AgentError> {
        let response = self
            .rpc("config/read", json!({"cwd":cwd,"includeLayers":false}))
            .await?;
        let config = &response["config"];
        validate_native(config, &self.provider)?;
        let mut overrides = serde_json::Map::new();
        if let Some(servers) = config["mcp_servers"].as_object() {
            overrides.insert("mcp_servers".into(), disabled_mcp(servers));
        }
        for key in [
            "features.apps",
            "features.plugins",
            "features.hooks",
            "features.multi_agent",
            "features.computer_use",
            "features.browser_use",
            "features.in_app_browser",
            "features.remote_plugin",
            "features.image_generation",
            "features.memories",
            "features.goals",
            "features.plugin_hooks",
        ] {
            overrides.insert(key.into(), json!(false));
        }
        overrides.insert("web_search".into(), json!("disabled"));
        overrides.insert("notify".into(), json!([]));
        Ok(Value::Object(overrides))
    }
}
impl Drop for CodexAdapter {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
impl AgentAdapter for CodexAdapter {
    fn start_thread<'a>(&'a self, cwd: &'a Path) -> BoxFuture<'a, Result<String, AgentError>> {
        Box::pin(async move {
            let config = self.thread_config(cwd).await?;
            let r=self.rpc("thread/start",json!({"cwd":cwd,"model":self.model,"modelProvider":self.provider,"approvalPolicy":"never","sandbox":"read-only","config":config})).await?;
            if r["modelProvider"] != self.provider
                || r["model"] != self.model
                || r["sandbox"]["type"] != "readOnly"
            {
                return Err(protocol(
                    "Codex thread did not preserve native provider, model, and read-only sandbox",
                ));
            }
            let id = r["thread"]["id"]
                .as_str()
                .ok_or_else(|| protocol("Missing Codex thread ID"))?
                .to_owned();
            self.verify_tools_disabled(&id).await?;
            Ok(id)
        })
    }
    fn resume_thread<'a>(
        &'a self,
        thread_id: &'a str,
        cwd: &'a Path,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            let config = self.thread_config(cwd).await?;
            let r=self.rpc("thread/resume",json!({"threadId":thread_id,"cwd":cwd,"model":self.model,"modelProvider":self.provider,"approvalPolicy":"never","sandbox":"read-only","config":config})).await?;
            if r["modelProvider"] != self.provider || r["sandbox"]["type"] != "readOnly" {
                return Err(protocol(
                    "Resumed Codex thread has unexpected provider or permissions",
                ));
            }
            self.verify_tools_disabled(thread_id).await?;
            Ok(())
        })
    }
    fn start_turn<'a>(
        &'a self,
        thread_id: &'a str,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<String, AgentError>> {
        Box::pin(async move {
            let r=self.rpc("turn/start",json!({"threadId":thread_id,"input":[{"type":"text","text":prompt}],"model":self.model,"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly"}})).await?;
            let turn_id = r["turn"]["id"]
                .as_str()
                .ok_or_else(|| protocol("Missing Codex turn ID"))?
                .to_owned();
            *self.active.lock().await = Some((thread_id.into(), turn_id.clone()));
            Ok(turn_id)
        })
    }
    fn next_event(&self) -> BoxFuture<'_, Result<AdapterEvent, AgentError>> {
        Box::pin(async move {
            loop {
                let msg = self.events.lock().await.recv().await.ok_or_else(|| {
                    AgentError::new(ErrorCode::ProcessExited, "Codex event stream closed")
                })?;
                let method = msg["method"].as_str().unwrap_or("");
                let p = &msg["params"];
                if method == "_exit" || method == "_protocol_error" {
                    return Ok(AdapterEvent::ProcessExited {
                        message: if method == "_exit" {
                            "Codex app-server exited"
                        } else {
                            "Codex emitted malformed JSON"
                        }
                        .into(),
                    });
                }
                let thread_id = p["threadId"].as_str().unwrap_or("").to_owned();
                if method == "turn/completed" {
                    let turn = &p["turn"];
                    let turn_id = turn["id"]
                        .as_str()
                        .ok_or_else(|| protocol("Turn completion missing ID"))?
                        .to_owned();
                    let outcome = match turn["status"].as_str() {
                        Some("completed") => TurnOutcome::Completed,
                        Some("failed") => TurnOutcome::Failed,
                        Some("interrupted") => TurnOutcome::Interrupted,
                        _ => TurnOutcome::Unknown,
                    };
                    let mut answer = if turn_has_full_items(turn) {
                        turn["items"].as_array().and_then(|a| final_answer(a))
                    } else {
                        None
                    };
                    if answer.is_none() && outcome == TurnOutcome::Completed {
                        let r = self
                            .rpc(
                                "thread/read",
                                json!({"threadId":thread_id,"includeTurns":true}),
                            )
                            .await?;
                        answer = r["thread"]["turns"]
                            .as_array()
                            .and_then(|a| {
                                a.iter()
                                    .find(|t| t["id"] == turn_id && turn_has_full_items(t))
                            })
                            .and_then(|t| t["items"].as_array())
                            .and_then(|a| final_answer(a));
                    }
                    let error = if outcome == TurnOutcome::Failed || !turn["error"].is_null() {
                        Some(AgentError::new(
                            ErrorCode::Adapter,
                            turn["error"]["message"]
                                .as_str()
                                .unwrap_or("Codex turn failed"),
                        ))
                    } else {
                        None
                    };
                    *self.active.lock().await = None;
                    return Ok(AdapterEvent::TurnFinished {
                        thread_id,
                        turn_id,
                        outcome,
                        final_answer: answer,
                        error,
                    });
                }
                if method == "item/agentMessage/delta"
                    || method == "item/started"
                    || method == "item/completed"
                    || method == "turn/started"
                {
                    return Ok(AdapterEvent::Progress {
                        thread_id,
                        turn_id: p["turnId"]
                            .as_str()
                            .or_else(|| p["turn"]["id"].as_str())
                            .unwrap_or("")
                            .into(),
                        kind: method.into(),
                        message: progress_message(p),
                    });
                }
            }
        })
    }
    fn interrupt<'a>(
        &'a self,
        thread_id: &'a str,
        turn_id: &'a str,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            self.rpc(
                "turn/interrupt",
                json!({"threadId":thread_id,"turnId":turn_id}),
            )
            .await?;
            Ok(())
        })
    }
    fn shutdown(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let active = self.active.lock().await.clone();
            let running = self
                .child
                .lock()
                .await
                .try_wait()
                .map_err(|_| protocol("Could not inspect Codex child"))?
                .is_none();
            let interrupt_error = if running {
                if let Some((thread, turn)) = active {
                    match tokio::time::timeout(
                        Duration::from_secs(1),
                        self.rpc("turn/interrupt", json!({"threadId":thread,"turnId":turn})),
                    )
                    .await
                    {
                        Ok(Ok(_)) => {
                            let mut completed = self.completion.clone();
                            let target = (thread, turn);
                            match tokio::time::timeout(Duration::from_secs(2), async {
                                loop {
                                    if completed.borrow().as_ref() == Some(&target) {
                                        return Ok(());
                                    }
                                    completed.changed().await.map_err(|_| {
                                        protocol("Codex exited before confirming interruption")
                                    })?;
                                }
                            })
                            .await
                            {
                                Ok(result) => result.err(),
                                Err(_) => Some(protocol(
                                    "Codex did not confirm interruption before shutdown",
                                )),
                            }
                        }
                        Ok(Err(e)) => Some(e),
                        Err(_) => Some(protocol(
                            "Codex did not acknowledge interruption before shutdown",
                        )),
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let mut child = self.child.lock().await;
            if child
                .try_wait()
                .map_err(|_| protocol("Could not inspect Codex child"))?
                .is_none()
            {
                child.kill().await.map_err(|_| {
                    AgentError::new(ErrorCode::ProcessExited, "Could not terminate Codex child")
                })?;
            }
            child
                .wait()
                .await
                .map_err(|_| protocol("Could not reap Codex child"))?;
            self.reader.abort();
            for (_, waiter) in self.pending.lock().await.drain() {
                let _ = waiter.send(Err(AgentError::new(
                    ErrorCode::Closed,
                    "Codex adapter shut down",
                )));
            }
            if let Some(error) = interrupt_error {
                return Err(error);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_endpoint_is_selected_by_codex_auth() {
        assert!(validate_native(&json!({"model_provider":"native","chatgpt_base_url":"https://chatgpt.com/backend-api/","model_providers":{"native":{"base_url":null,"requires_openai_auth":true,"wire_api":"responses"}}}),"native").is_ok());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn native_reuses_auth_without_account_gate_or_home_override() {
        let (_dir, adapter) = fake_adapter().await;
        let context = adapter.rpc("test/context", json!({})).await.unwrap();
        assert_eq!(
            context["home"],
            std::env::var("CODEX_HOME")
                .ok()
                .map(Value::String)
                .unwrap_or(Value::Null)
        );
        assert_eq!(
            context["api_key_present"],
            std::env::var_os("OPENAI_API_KEY").is_some()
        );
        adapter.shutdown().await.unwrap();
    }

    #[test]
    fn enabled_mcp_runtime_is_rejected() {
        assert!(validate_disabled_mcp(
            &json!({"data":[{"name":"writer","runtimeStatus":"ready","tools":{"write":{}}}]})
        )
        .is_err());
    }
    #[test]
    fn mcp_disable_does_not_replay_null_or_secret_fields() {
        let mut servers = serde_json::Map::new();
        servers.insert(
            "a.b".into(),
            json!({"tool_timeout_sec":null,"env":{"TOKEN":"secret"}}),
        );
        assert_eq!(disabled_mcp(&servers), json!({"a.b":{"enabled":false}}));
    }
    #[test]
    fn summarized_items_are_not_complete_answers() {
        assert!(!turn_has_full_items(
            &json!({"itemsView":"summary","items":[{"type":"agentMessage","phase":"final_answer","text":"summary"}]})
        ));
    }
    #[test]
    fn command_progress_preserves_execution_evidence_without_output() {
        let p = json!({"item":{"type":"commandExecution","command":"touch blocked","exitCode":1,"status":"completed","aggregatedOutput":"secret"}});
        let text = progress_message(&p);
        let out: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(out["command"], "touch blocked");
        assert_eq!(out["exitCode"], 1);
        assert!(out.get("aggregatedOutput").is_none());
    }
    #[test]
    fn partial_answer_is_not_terminal() {
        assert_eq!(
            final_answer(&[
                json!({"type":"agentMessage","phase":"partial_answer","text":"not done"})
            ]),
            None
        );
    }
    #[test]
    fn query_parameters_are_not_native() {
        let mut c = json!({"model_provider":"native","chatgpt_base_url":"https://chatgpt.com/backend-api/","model_providers":{"native":{"base_url":null,"requires_openai_auth":true,"wire_api":"responses","query_params":{"redirect":"other"}}}});
        assert!(validate_native(&c, "native").is_err());
        c["model_providers"]["native"]
            .as_object_mut()
            .unwrap()
            .remove("query_params");
        assert!(validate_native(&c, "native").is_ok());
    }
    #[test]
    fn refuses_redirected_native_endpoint() {
        assert!(validate_native(&json!({"model_provider":"native","model_providers":{"native":{"base_url":"http://127.0.0.1:9090/v1","requires_openai_auth":true}}}),"native").is_err());
    }
    #[test]
    fn final_output_excludes_commentary_and_preserves_all_final_items() {
        let items = vec![
            json!({"type":"agentMessage","phase":"commentary","text":"working"}),
            json!({"type":"agentMessage","phase":"final_answer","text":"One"}),
            json!({"type":"agentMessage","phase":"final_answer","text":"Two"}),
        ];
        assert_eq!(final_answer(&items), Some("One\n\nTwo".into()));
    }
    /// Opt-in deterministic enforcement check using the same official sandbox policy
    /// as thread/turn execution; no production command API is exposed.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires an installed public Codex CLI and existing native authentication"]
    async fn live_read_only_commands_are_denied() {
        let executable = discover(None).await.unwrap();
        let adapter = CodexAdapter::connect(executable).await.unwrap();
        let repo = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(repo.path())
            .status()
            .unwrap()
            .success());
        let original = format!("read-only nonce {}\n", uuid::Uuid::new_v4());
        std::fs::write(repo.path().join("evidence.txt"), &original).unwrap();
        let read=adapter.rpc("command/exec",json!({"command":["/bin/cat","evidence.txt"],"cwd":repo.path(),"sandboxPolicy":{"type":"readOnly","networkAccess":false},"timeoutMs":10000})).await.unwrap();
        assert_eq!(read["exitCode"], 0);
        assert_eq!(read["stdout"], original);
        let overwrite=adapter.rpc("command/exec",json!({"command":["/bin/sh","-c","printf tampered > evidence.txt"],"cwd":repo.path(),"sandboxPolicy":{"type":"readOnly","networkAccess":false},"timeoutMs":10000})).await.unwrap();
        let create=adapter.rpc("command/exec",json!({"command":["/usr/bin/touch","forbidden.txt"],"cwd":repo.path(),"sandboxPolicy":{"type":"readOnly","networkAccess":false},"timeoutMs":10000})).await.unwrap();
        for result in [&overwrite, &create] {
            assert_ne!(result["exitCode"].as_i64().unwrap(), 0);
            let stderr = result["stderr"].as_str().unwrap().to_ascii_lowercase();
            assert!(
                stderr.contains("operation not permitted") || stderr.contains("permission denied"),
                "unexpected sandbox stderr: {stderr}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(repo.path().join("evidence.txt")).unwrap(),
            original
        );
        assert!(!repo.path().join("forbidden.txt").exists());
        if let Some(path) = std::env::var_os("AGENTGATE_READONLY_EVIDENCE") {
            std::fs::write(path,serde_json::to_vec_pretty(&json!({"sandbox":{"type":"readOnly","networkAccess":false},"read":read,"overwrite":overwrite,"create":create,"source_unchanged":true,"new_file_absent":true})).unwrap()).unwrap();
        }
        adapter.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    async fn fake_adapter() -> (tempfile::TempDir, CodexAdapter) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("codex");
        std::fs::write(&script,r##"#!/usr/bin/env python3
import sys,json,os
args=sys.argv[1:]; conf={}
for n,x in enumerate(args):
 if x=='-c':
  key,_,value=args[n+1].partition('=')
  if key=='model_provider':conf[key]=json.loads(value)
provider=conf['model_provider']; pending=None; mode='matching'
conf['model_providers']={provider:{'base_url':None,'requires_openai_auth':True,'wire_api':'responses'}}
for line in sys.stdin:
 m=json.loads(line); method=m['method']
 if 'id' not in m: continue
 r={}
 if method=='initialize':r={'userAgent':'agentgate/codex-cli 0.162.1'}
 elif method=='account/read':
  print(json.dumps({'id':m['id'],'error':{'code':-32601,'message':'account gating forbidden'}}),flush=True);continue
 elif method=='test/context':r={'home':os.environ.get('CODEX_HOME'),'api_key_present':'OPENAI_API_KEY' in os.environ}
 elif method=='config/read':r={'config':{'model_provider':provider,'model_providers':conf['model_providers'],'cli_auth_credentials_store':'keyring','chatgpt_base_url':'https://chatgpt.com/backend-api/'}}
 elif method=='model/list':r={'data':[{'isDefault':True,'model':'official-test-model'}]}
 elif method=='turn/start':
  mode=m['params']['input'][0]['text'];r={'turn':{'id':'active-turn'}}
 elif method=='turn/interrupt':
  print(json.dumps({'id':m['id'],'result':{}}),flush=True)
  print(json.dumps({'method':'turn/completed','params':{'threadId':'thread','turn':{'id':'unrelated-turn','status':'interrupted'}}}),flush=True)
  if mode=='matching':print(json.dumps({'method':'turn/completed','params':{'threadId':'thread','turn':{'id':'active-turn','status':'interrupted'}}}),flush=True)
  continue
 elif method=='test/first':pending=m['id'];continue
 elif method=='test/second':
  print(json.dumps({'id':m['id'],'result':{'order':2}}),flush=True)
  print(json.dumps({'id':pending,'result':{'order':1}}),flush=True);continue
 elif method=='test/exit':sys.exit(0)
 print(json.dumps({'id':m['id'],'result':r}),flush=True)
"##).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let adapter = CodexAdapter::connect(script).await.unwrap();
        (dir, adapter)
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_waits_for_the_matching_turn_and_reaps_child() {
        let (_dir, adapter) = fake_adapter().await;
        assert_eq!(
            adapter.start_turn("thread", "matching").await.unwrap(),
            "active-turn"
        );
        adapter.shutdown().await.unwrap();
        assert_eq!(
            adapter.completion.borrow().as_ref(),
            Some(&("thread".into(), "active-turn".into()))
        );
        assert!(adapter.child.lock().await.try_wait().unwrap().is_some());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_rejects_ack_without_matching_completion_and_still_reaps() {
        let (_dir, adapter) = fake_adapter().await;
        adapter.start_turn("thread", "wrong-only").await.unwrap();
        let error = adapter.shutdown().await.unwrap_err();
        assert!(error.message.contains("did not confirm interruption"));
        assert!(adapter.child.lock().await.try_wait().unwrap().is_some());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn transport_correlates_reversed_responses_and_reports_eof() {
        let (_dir, adapter) = fake_adapter().await;
        let (a, b) = tokio::join!(
            adapter.rpc("test/first", json!({})),
            adapter.rpc("test/second", json!({}))
        );
        assert_eq!(a.unwrap()["order"], 1);
        assert_eq!(b.unwrap()["order"], 2);
        assert_eq!(
            adapter.rpc("test/exit", json!({})).await.unwrap_err().code,
            ErrorCode::ProcessExited
        );
        assert!(matches!(
            adapter.next_event().await.unwrap(),
            AdapterEvent::ProcessExited { .. }
        ));
        adapter.shutdown().await.unwrap();
        adapter.shutdown().await.unwrap();
    }
}
