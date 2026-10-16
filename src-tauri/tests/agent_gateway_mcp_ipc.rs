//! Agent Gateway 第二阶段端到端测试：MCP(stdio 协议层) → 本地 IPC → AgentService →
//! Adapter(测试替身) → SQLite。只替换最外层的 provider CLI，其余全部走真实实现。
use agentgate_lib::agent_gateway::ipc::{self, IpcClient, ServiceHostConfig, ServicePaths};
use agentgate_lib::agent_gateway::mcp::Gateway;
use agentgate_lib::agent_gateway::{
    AdapterEvent, AgentAdapter, AgentError, AgentService, BoxFuture, ErrorCode, TurnOutcome,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

const LONG_ANSWER: &str = "审计结论：\n- 权限边界集中在 security/\n- 明文传输风险仍需确认\n";

/// Behavior driven by the prompt, so one fake covers success / failure / long-running / hang.
struct Fake {
    rx: AsyncMutex<mpsc::UnboundedReceiver<AdapterEvent>>,
    tx: mpsc::UnboundedSender<AdapterEvent>,
    turns: Mutex<Vec<String>>,
    shutdowns: AtomicUsize,
}

impl Fake {
    fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            rx: AsyncMutex::new(rx),
            tx,
            turns: Mutex::new(vec![]),
            shutdowns: AtomicUsize::new(0),
        })
    }
    fn send(&self, event: AdapterEvent) {
        let _ = self.tx.send(event);
    }
}

impl AgentAdapter for Fake {
    fn start_thread<'a>(&'a self, _cwd: &'a Path) -> BoxFuture<'a, Result<String, AgentError>> {
        Box::pin(async { Ok("thread-1".into()) })
    }
    fn resume_thread<'a>(
        &'a self,
        _thread_id: &'a str,
        _cwd: &'a Path,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async { Ok(()) })
    }
    fn start_turn<'a>(
        &'a self,
        _thread_id: &'a str,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<String, AgentError>> {
        Box::pin(async move {
            self.turns.lock().unwrap().push(prompt.to_owned());
            // 全局递增，保证服务重启后新建的 Turn ID 不会与旧 Turn 冲突。
            static TURN_SEQ: AtomicUsize = AtomicUsize::new(0);
            let turn = format!("turn-{}", TURN_SEQ.fetch_add(1, Ordering::SeqCst) + 1);
            let fake = self.tx.clone();
            let prompt = prompt.to_owned();
            let turn_id = turn.clone();
            tokio::spawn(async move {
                let _ = fake.send(AdapterEvent::Progress {
                    thread_id: "thread-1".into(),
                    turn_id: turn_id.clone(),
                    kind: "assistant".into(),
                    message: "reading files".into(),
                });
                if prompt.contains("FAIL") {
                    let _ = fake.send(AdapterEvent::TurnFinished {
                        thread_id: "thread-1".into(),
                        turn_id,
                        outcome: TurnOutcome::Failed,
                        final_answer: None,
                        error: Some(AgentError::new(ErrorCode::Adapter, "fixture failure")),
                    });
                } else if prompt.contains("HANG") {
                    // 只有真实 interrupt 才会产生终局。
                } else {
                    if prompt.contains("SLOW") {
                        tokio::time::sleep(Duration::from_millis(1200)).await;
                    }
                    let _ = fake.send(AdapterEvent::TurnFinished {
                        thread_id: "thread-1".into(),
                        turn_id,
                        outcome: TurnOutcome::Completed,
                        final_answer: Some(format!("{LONG_ANSWER}{prompt}")),
                        error: None,
                    });
                }
            });
            Ok(turn)
        })
    }
    fn next_event(&self) -> BoxFuture<'_, Result<AdapterEvent, AgentError>> {
        Box::pin(async move {
            self.rx
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| AgentError::new(ErrorCode::ProcessExited, "fake stream closed"))
        })
    }
    fn interrupt<'a>(
        &'a self,
        thread_id: &'a str,
        turn_id: &'a str,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            self.send(AdapterEvent::TurnFinished {
                thread_id: thread_id.into(),
                turn_id: turn_id.into(),
                outcome: TurnOutcome::Interrupted,
                final_answer: None,
                error: Some(AgentError::new(ErrorCode::Adapter, "interrupted by cancel")),
            });
            Ok(())
        })
    }
    fn shutdown(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

struct FakeFactory {
    adapters: Mutex<Vec<Arc<Fake>>>,
}

impl agentgate_lib::agent_gateway::AdapterFactory for FakeFactory {
    fn descriptor(&self) -> agentgate_lib::agent_gateway::AgentDescriptor {
        agentgate_lib::agent_gateway::AgentDescriptor {
            id: "fake-native".into(),
            name: "Fake Native".into(),
            permissions: vec![agentgate_lib::agent_gateway::PermissionMode::ReadOnly],
            supports_continuation: true,
            supports_cancel: true,
        }
    }
    fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>> {
        Box::pin(async move {
            let fake = Fake::new();
            self.adapters.lock().unwrap().push(fake.clone());
            Ok(fake as Arc<dyn AgentAdapter>)
        })
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    db_dir: PathBuf,
    service: Arc<AgentService>,
    handle: tokio::task::JoinHandle<Result<(), AgentError>>,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with(None).await
    }

    /// `dir` 为 `Some` 时复用已有数据目录（服务重启测试）。
    async fn start_with(dir: Option<tempfile::TempDir>) -> Self {
        let dir = dir.unwrap_or_else(|| tempfile::tempdir().unwrap());
        let db_dir = dir.path().to_path_buf();
        let store = ipc::open_store(&db_dir).unwrap();
        let factory = Arc::new(FakeFactory {
            adapters: Mutex::new(vec![]),
        });
        let service = Arc::new(AgentService::with_store(vec![factory], store).unwrap());
        let paths = ServicePaths::new(&db_dir);
        let handle = tokio::spawn(ipc::serve(
            service.clone(),
            ServiceHostConfig {
                paths,
                idle_timeout: None,
            },
        ));
        for _ in 0..100 {
            if ServicePaths::new(&db_dir).read_token().is_ok()
                && IpcClient::connect(&ServicePaths::new(&db_dir))
                    .await
                    .is_ok()
            {
                return Self {
                    db_dir,
                    service,
                    handle,
                    _dir: dir,
                };
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("agent service did not start");
    }

    fn paths(&self) -> ServicePaths {
        ServicePaths::new(&self.db_dir)
    }

    fn gateway(&self) -> Gateway {
        Gateway::new(self.db_dir.clone(), PathBuf::from("/nonexistent-agentgate"))
    }

    async fn call(gateway: &mut Gateway, method: &str, params: Value) -> Value {
        let response = agentgate_lib::agent_gateway::mcp::handle_message(
            gateway,
            &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
        )
        .await
        .expect("request must produce a response");
        response
    }

    async fn tool(gateway: &mut Gateway, name: &str, arguments: Value) -> Value {
        Self::call(
            gateway,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
        .await
    }

    /// 停掉当前服务实例，再用同一个数据目录启动一个新实例。
    async fn restart(self) -> Harness {
        self.stop().await;
        let dir = self._dir;
        drop(self.service);
        drop(self.handle);
        Self::start_with(Some(dir)).await
    }

    async fn stop(&self) {
        let mut client = IpcClient::connect(&self.paths()).await.unwrap();
        let _ = client.call("service_shutdown", json!({})).await;
        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            while !self.handle.is_finished() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
    }
}

fn tool_payload(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content: {response}"));
    serde_json::from_str(text).unwrap()
}

async fn wait_terminal(gateway: &mut Gateway, task_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = tool_payload(
            &Harness::tool(gateway, "agent_status", json!({"task_id": task_id})).await,
        );
        if status["state"].as_str().is_some_and(|state| {
            ["completed", "failed", "cancelled", "interrupted"].contains(&state)
        }) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "task never became terminal: {status}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_initialize_list_and_run_status_round_trip() {
    let harness = Harness::start().await;
    let mut gateway = harness.gateway();

    let init = Harness::call(
        &mut gateway,
        "initialize",
        json!({"protocolVersion":"2025-06-18","capabilities":{}}),
    )
    .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        init["result"]["serverInfo"]["name"],
        "muxlayer-agent-gateway"
    );
    // 通知不能有响应（否则客户端会认为协议被污染）。
    assert!(agentgate_lib::agent_gateway::mcp::handle_message(
        &mut gateway,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
    )
    .await
    .is_none());

    let listed = Harness::call(&mut gateway, "tools/list", json!({})).await;
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "agent_list",
            "agent_run",
            "agent_continue",
            "agent_status",
            "agent_cancel"
        ]
    );
    assert!(listed["result"]["tools"][0]["inputSchema"]["type"] == "object");

    let agents = tool_payload(&Harness::tool(&mut gateway, "agent_list", json!({})).await);
    assert_eq!(agents["agents"][0]["id"], "fake-native");

    let submission = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_run",
            json!({"agent":"fake-native","cwd":std::env::current_dir().unwrap(),"prompt":"audit A"}),
        )
        .await,
    );
    let task_id = submission["task_id"].as_str().unwrap().to_owned();
    let session_id = submission["session_id"].as_str().unwrap().to_owned();
    assert_ne!(task_id, session_id);

    let completed = wait_terminal(&mut gateway, &task_id).await;
    assert_eq!(completed["state"], "completed");
    assert_eq!(completed["thread_id"], "thread-1");
    assert!(completed["turn_id"].as_str().unwrap().starts_with("turn-"));
    assert!(completed["final_answer"]
        .as_str()
        .unwrap()
        .contains("audit A"));
    assert_eq!(completed["final_answer_complete"], true);

    // 继续原 Session：同一 Thread，新 Turn。
    let second = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_continue",
            json!({"session_id": session_id, "prompt": "audit B"}),
        )
        .await,
    );
    let second_task = wait_terminal(&mut gateway, second["task_id"].as_str().unwrap()).await;
    assert_eq!(second_task["thread_id"], completed["thread_id"]);
    assert_ne!(second_task["turn_id"], completed["turn_id"]);
    let session = agentgate_lib::agent_gateway::mcp::handle_message(
        &mut gateway,
        &json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"agent_status","arguments":{"task_id":task_id}}}).to_string(),
    )
    .await
    .unwrap();
    assert_eq!(session["result"]["isError"], false, "{session}");

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_running_task_does_not_block_mcp_and_is_paginated() {
    let harness = Harness::start().await;
    let mut gateway = harness.gateway();

    let started = Instant::now();
    let submission = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_run",
            json!({"agent":"fake-native","cwd":std::env::current_dir().unwrap(),"prompt":"SLOW audit"}),
        )
        .await,
    );
    // 提交必须快速返回：不能等整个任务执行完。
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "agent_run blocked for {:?}",
        started.elapsed()
    );
    let task_id = submission["task_id"].as_str().unwrap().to_owned();
    let running = tool_payload(
        &Harness::tool(&mut gateway, "agent_status", json!({"task_id": task_id})).await,
    );
    assert!(
        ["queued", "running"].contains(&running["state"].as_str().unwrap()),
        "{running}"
    );

    let completed = wait_terminal(&mut gateway, &task_id).await;
    let answer = completed["final_answer"].as_str().unwrap().to_owned();
    let total = completed["final_answer_total_chars"].as_u64().unwrap() as usize;
    assert_eq!(answer.chars().count(), total);

    // 分页必须能精确重组原文，且不静默截断。
    let mut rebuilt = String::new();
    let mut offset = 0usize;
    loop {
        let page = tool_payload(
            &Harness::tool(
                &mut gateway,
                "agent_status",
                json!({"task_id": task_id, "final_answer_offset": offset, "final_answer_limit": 7}),
            )
            .await,
        );
        rebuilt.push_str(page["final_answer"].as_str().unwrap());
        match page["final_answer_next_offset"].as_u64() {
            Some(next) => {
                assert_eq!(page["final_answer_complete"], false);
                offset = next as usize;
            }
            None => {
                assert_eq!(page["final_answer_complete"], true);
                break;
            }
        }
    }
    assert_eq!(rebuilt, answer);

    // 事件默认不返回，但必须报告总量与下一页位置。
    let no_events = tool_payload(
        &Harness::tool(&mut gateway, "agent_status", json!({"task_id": task_id})).await,
    );
    assert_eq!(no_events["events"], json!([]));
    assert_eq!(no_events["events_included"], false);
    assert!(no_events["events_total"].as_u64().unwrap() >= 1);
    assert_eq!(no_events["events_next_offset"], 0);
    let with_events = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_status",
            json!({"task_id": task_id, "events_limit": 5}),
        )
        .await,
    );
    assert_eq!(with_events["events_included"], true);
    assert!(!with_events["events"].as_array().unwrap().is_empty());

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tasks_keep_running_after_the_calling_client_disconnects() {
    let harness = Harness::start().await;
    let mut gateway = harness.gateway();
    let submission = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_run",
            json!({"agent":"fake-native","cwd":std::env::current_dir().unwrap(),"prompt":"SLOW survives"}),
        )
        .await,
    );
    let task_id = submission["task_id"].as_str().unwrap().to_owned();

    // 模拟 MCP 客户端退出：丢掉这条连接，服务继续持有任务。
    drop(gateway);
    tokio::time::sleep(Duration::from_millis(150)).await;

    // 重新连接（真实 IPC 重连），仍能用原 task_id 查询并拿到完整结果。
    let mut reconnected = harness.gateway();
    let completed = wait_terminal(&mut reconnected, &task_id).await;
    assert_eq!(completed["state"], "completed");
    assert!(completed["final_answer"]
        .as_str()
        .unwrap()
        .contains("SLOW survives"));

    // 用原 session_id 继续会话，Session → Provider Thread 映射必须还在。
    let session_id = submission["session_id"].as_str().unwrap();
    let continued = tool_payload(
        &Harness::tool(
            &mut reconnected,
            "agent_continue",
            json!({"session_id": session_id, "prompt": "after reconnect"}),
        )
        .await,
    );
    let continued_task =
        wait_terminal(&mut reconnected, continued["task_id"].as_str().unwrap()).await;
    assert_eq!(continued_task["thread_id"], "thread-1");

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_is_a_real_provider_interrupt_and_business_failures_are_reported() {
    let harness = Harness::start().await;
    let mut gateway = harness.gateway();

    let hanging = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_run",
            json!({"agent":"fake-native","cwd":std::env::current_dir().unwrap(),"prompt":"HANG here"}),
        )
        .await,
    );
    let task_id = hanging["task_id"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let cancelled = tool_payload(
        &Harness::tool(&mut gateway, "agent_cancel", json!({"task_id": task_id})).await,
    );
    // 取消请求被接受 ≠ 取消完成：必须继续查询到真实终态。
    assert_eq!(cancelled["cancel_requested"], true);
    let terminal = wait_terminal(&mut gateway, &task_id).await;
    assert_eq!(terminal["state"], "cancelled");

    // 业务失败也是一次成功的查询，不是 MCP 传输失败。
    let failing = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_run",
            json!({"agent":"fake-native","cwd":std::env::current_dir().unwrap(),"prompt":"FAIL now"}),
        )
        .await,
    );
    let failed = wait_terminal(&mut gateway, failing["task_id"].as_str().unwrap()).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error"]["message"], "fixture failure");

    // 未知任务：业务错误以 isError 结果返回，保留原 code。
    let unknown = Harness::tool(&mut gateway, "agent_status", json!({"task_id":"nope"})).await;
    assert_eq!(unknown["result"]["isError"], true);
    assert_eq!(tool_payload(&unknown)["error"]["code"], "not_found");

    // 参数校验失败是 -32602。
    let invalid = Harness::tool(&mut gateway, "agent_run", json!({"agent":"fake-native"})).await;
    assert_eq!(invalid["error"]["code"], -32602);
    let unsupported = Harness::tool(
        &mut gateway,
        "agent_run",
        json!({"agent":"fake-native","cwd":"/tmp","prompt":"x","permission":"execute"}),
    )
    .await;
    assert_eq!(unsupported["error"]["code"], -32602);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ipc_rejects_an_unauthenticated_client_and_duplicate_service_instances() {
    let harness = Harness::start().await;
    let paths = harness.paths();

    // 正确 token 才能握手；错误 token 被明确拒绝。
    assert!(IpcClient::connect(&paths).await.is_ok());
    let raw = tokio::net::UnixStream::connect(&paths.socket)
        .await
        .unwrap();
    let (reader, writer) = tokio::io::split(raw);
    let mut writer = tokio::io::BufWriter::new(writer);
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"1\",\"method\":\"hello\",\"params\":{\"token\":\"wrong\"}}\n")
        .await
        .unwrap();
    writer.flush().await.unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(reader)
        .read_line(&mut line)
        .await
        .unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["error"]["code"], "unauthorized");

    // 第二个实例不能接管同一批任务。
    let duplicate = ipc::serve(
        harness.service.clone(),
        ServiceHostConfig {
            paths: paths.clone(),
            idle_timeout: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(duplicate.code, ErrorCode::Busy);

    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn service_status_and_agent_session_are_available_over_ipc() {
    let harness = Harness::start().await;
    let mut client = IpcClient::connect(&harness.paths()).await.unwrap();
    let status = client
        .call("service_status", json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status["service"], "muxlayer-agent-service");
    assert_eq!(status["running_task_ids"], json!([]));

    let run = client
        .call(
            "agent_run",
            json!({"agent":"fake-native","cwd":std::env::current_dir().unwrap(),"prompt":"ipc","permission":"read_only"}),
        )
        .await
        .unwrap()
        .unwrap();
    let session = client
        .call("agent_session", json!({"session_id": run["session_id"]}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session["agent"], "fake-native");
    assert_eq!(session["task_ids"][0], run["task_id"]);

    // 业务错误在 IPC 层也是"明确回答"，不是传输失败。
    let missing = client
        .call("agent_status", json!({"task_id":"nope"}))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
    harness.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_results_and_sessions_survive_a_service_restart() {
    let harness = Harness::start().await;
    let mut gateway = harness.gateway();
    let submission = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_run",
            json!({"agent":"fake-native","cwd":std::env::current_dir().unwrap(),"prompt":"durable audit"}),
        )
        .await,
    );
    let task_id = submission["task_id"].as_str().unwrap().to_owned();
    let session_id = submission["session_id"].as_str().unwrap().to_owned();
    let completed = wait_terminal(&mut gateway, &task_id).await;
    let answer = completed["final_answer"].as_str().unwrap().to_owned();
    drop(gateway);

    // 服务进程整体重启：MCP 客户端只能通过 task_id / session_id 找回结果。
    let harness = harness.restart().await;
    let mut gateway = harness.gateway();
    let recovered = tool_payload(
        &Harness::tool(&mut gateway, "agent_status", json!({"task_id": task_id})).await,
    );
    assert_eq!(recovered["state"], "completed");
    assert_eq!(recovered["final_answer"].as_str().unwrap(), answer);
    assert_eq!(recovered["final_answer_complete"], true);

    // 原 Session 仍可继续（Provider Thread 映射已持久化）。
    let continued = tool_payload(
        &Harness::tool(
            &mut gateway,
            "agent_continue",
            json!({"session_id": session_id, "prompt": "after restart"}),
        )
        .await,
    );
    let continued_task = wait_terminal(&mut gateway, continued["task_id"].as_str().unwrap()).await;
    assert_eq!(continued_task["thread_id"], completed["thread_id"]);
    assert_ne!(continued_task["turn_id"], completed["turn_id"]);

    harness.stop().await;
}

#[test]
fn mcp_agent_binary_never_writes_logs_to_stdout() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agentgate-serve"))
        .arg("mcp-agent")
        .arg("--db-path")
        .arg(dir.path())
        .stdin(Stdio::null())
        .output()
        .expect("run agentgate-serve mcp-agent");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "stdout must stay protocol-clean, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    // 日志确实产生了，只是走了 stderr。
    assert!(!output.stderr.is_empty());
}
