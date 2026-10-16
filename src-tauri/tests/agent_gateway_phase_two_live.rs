//! 第二阶段真实验收（显式 opt-in）。用真实 MCP stdio 客户端驱动真实二进制：
//! `agentgate-serve mcp-agent` → 本地 IPC → 真实 Agent Service → 真实 Codex / Claude Code CLI。
//!
//! ```bash
//! AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
//!   --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
//!   --test agent_gateway_phase_two_live -- --ignored --nocapture --test-threads=1
//! ```
//! 真实推理会消耗官方额度。测试不读取任何凭据。
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    seq: u64,
    log: PathBuf,
}

impl McpClient {
    async fn spawn(db_dir: &Path) -> Self {
        std::fs::create_dir_all(db_dir).unwrap();
        let log = db_dir.join("mcp-client.log");
        let file = std::fs::File::create(&log).unwrap();
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentgate-serve"));
        command
            .arg("mcp-agent")
            .arg("--db-path")
            .arg(db_dir)
            .env("AGENTGATE_LOG", "info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(file));
        if let Some(codex) = std::env::var_os("AGENTGATE_CODEX_BIN") {
            command.env("AGENTGATE_CODEX_BIN", codex);
        }
        let mut child = command.spawn().expect("spawn mcp-agent");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut client = Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            seq: 0,
            log,
        };
        let init = client
            .call(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
            )
            .await;
        assert_eq!(
            init["result"]["serverInfo"]["name"],
            "muxlayer-agent-gateway"
        );
        client.notify("notifications/initialized", json!({})).await;
        client
    }

    async fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_string(message).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await;
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.seq += 1;
        let id = self.seq;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let mut line = String::new();
            let read =
                tokio::time::timeout(Duration::from_secs(60), self.reader.read_line(&mut line))
                    .await
                    .unwrap_or_else(|_| {
                        panic!(
                            "MCP client got no reply for {method}; log: {}",
                            self.log.display()
                        )
                    })
                    .unwrap();
            assert!(
                read > 0,
                "MCP server closed stdout during {method}; log: {}",
                self.log.display()
            );
            let value: Value = serde_json::from_str(line.trim()).unwrap_or_else(|e| {
                panic!("invalid MCP line ({e}) — stdout must be protocol-only: {line}")
            });
            if value["id"] == json!(id) {
                return value;
            }
        }
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self
            .call("tools/call", json!({"name": name, "arguments": arguments}))
            .await;
        assert!(
            response["error"].is_null(),
            "tools/call {name} failed at JSON-RPC level: {response}"
        );
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool {name} returned no text: {response}"))
            .to_owned();
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("tool {name} payload not JSON ({e}): {text}"))
    }

    fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn terminal_state(state: &str) -> bool {
    ["completed", "failed", "cancelled", "interrupted"].contains(&state)
}

async fn wait_terminal(client: &mut McpClient, task_id: &str, budget: Duration) -> Value {
    let deadline = Instant::now() + budget;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = client
            .tool("agent_status", json!({"task_id": task_id}))
            .await;
        if last["state"].as_str().is_some_and(terminal_state) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("task {task_id} did not reach a terminal state in time: {last}");
}

fn git_init(path: &Path) {
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(path)
        .status()
        .unwrap()
        .success());
}

#[tokio::test]
#[ignore = "requires the public Codex CLI, existing login and real model requests"]
async fn live_codex_via_mcp_stdio_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_init(&repo);
    let nonce = uuid::Uuid::new_v4().to_string();
    let source = format!("SOURCE_NONCE={nonce}\nRead-only fixture.\n");
    std::fs::write(repo.join("evidence.txt"), &source).unwrap();

    let db_dir = dir.path().join("db");
    let mut client = McpClient::spawn(&db_dir).await;

    let agents = client.tool("agent_list", json!({})).await;
    println!("agents: {agents}");
    assert!(agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|agent| agent["id"] == "codex-native"));

    let listed = client.call("tools/list", json!({})).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
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

    // 1) 提交 + 完整结果
    let submission = client
        .tool(
            "agent_run",
            json!({
                "agent": "codex-native",
                "cwd": repo,
                "prompt": "Read evidence.txt with a shell command and return its exact SOURCE_NONCE. \
                           Do not modify any file.",
            }),
        )
        .await;
    let task_id = submission["task_id"].as_str().unwrap().to_owned();
    let session_id = submission["session_id"].as_str().unwrap().to_owned();
    assert_ne!(task_id, session_id, "Task ID 与 Session ID 不能混用");
    println!("task1={task_id} session={session_id}");

    let first = wait_terminal(&mut client, &task_id, Duration::from_secs(420)).await;
    println!("first task: {first}");
    assert_eq!(first["state"], "completed", "{first}");
    let answer = first["final_answer"].as_str().unwrap().to_owned();
    assert!(
        answer.contains(&nonce),
        "final answer must contain the nonce: {answer}"
    );
    let total = first["final_answer_total_chars"].as_u64().unwrap() as usize;
    assert_eq!(answer.chars().count(), total);
    assert_eq!(first["final_answer_complete"], true);
    let thread = first["thread_id"].as_str().unwrap().to_owned();
    let turn = first["turn_id"].as_str().unwrap().to_owned();

    // 分页精确重组
    let mut rebuilt = String::new();
    let mut offset = 0usize;
    loop {
        let page = client
            .tool(
                "agent_status",
                json!({"task_id": task_id, "final_answer_offset": offset, "final_answer_limit": 64}),
            )
            .await;
        rebuilt.push_str(page["final_answer"].as_str().unwrap());
        match page["final_answer_next_offset"].as_u64() {
            Some(next) => offset = next as usize,
            None => break,
        }
    }
    assert_eq!(rebuilt, answer, "分页必须能精确重组原文");

    // 2) 继续原 Session：同一 Provider Thread，新 Turn
    let continued = client
        .tool(
            "agent_continue",
            json!({
                "session_id": session_id,
                "prompt": "Without reading files again, return the exact SOURCE_NONCE from the previous turn.",
            }),
        )
        .await;
    let second = wait_terminal(
        &mut client,
        continued["task_id"].as_str().unwrap(),
        Duration::from_secs(420),
    )
    .await;
    println!("continuation: {second}");
    assert_eq!(second["state"], "completed", "{second}");
    assert_eq!(second["thread_id"].as_str().unwrap(), thread);
    assert_ne!(second["turn_id"].as_str().unwrap(), turn);
    assert!(second["final_answer"].as_str().unwrap().contains(&nonce));

    // 3) 真实取消：先观察到真实命令开始，再取消
    let hanging = client
        .tool(
            "agent_run",
            json!({
                "agent": "codex-native",
                "cwd": repo,
                "prompt": "Run the shell command `sleep 120` and then report the exit code.",
            }),
        )
        .await;
    let hang_task = hanging["task_id"].as_str().unwrap().to_owned();
    // 必须先观察到真实 shell 命令开始，再取消；不能凭 RPC 确认就当取消完成。
    let observe = Instant::now();
    let mut saw_sleep = false;
    while !saw_sleep {
        let status = client
            .tool(
                "agent_status",
                json!({"task_id": hang_task, "events_offset": 0, "events_limit": 100}),
            )
            .await;
        // 只认"真实 commandExecution 事件"：不接受模型提前说出 sleep 之类的话术。
        let observed = status["events"].as_array().and_then(|events| {
            events.iter().find(|event| {
                event["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("commandExecution") && m.contains("sleep"))
            })
        });
        if let Some(event) = observed {
            println!("observed provider command event: {event}");
            saw_sleep = true;
            break;
        }
        assert!(
            status["state"].as_str().is_some_and(|s| !terminal_state(s)),
            "task ended before the real command started: {status}"
        );
        assert!(
            observe.elapsed() < Duration::from_secs(120),
            "no real provider command observed before cancel: {status}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    println!("observed real shell start for task {hang_task}");
    let requested = client
        .tool("agent_cancel", json!({"task_id": hang_task}))
        .await;
    println!("cancel request: {requested}");
    assert_eq!(requested["cancel_requested"], true);
    let cancelled = wait_terminal(&mut client, &hang_task, Duration::from_secs(120)).await;
    println!("cancel terminal: {cancelled}");
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");

    // 只读：文件未被修改，也没有新文件
    assert_eq!(
        std::fs::read_to_string(repo.join("evidence.txt")).unwrap(),
        source
    );
    let created: Vec<_> = std::fs::read_dir(&repo)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name != "evidence.txt" && name != ".git")
        .collect();
    assert!(
        created.is_empty(),
        "read-only run created files: {created:?}"
    );

    // 4) MCP 客户端退出后，服务继续运行：杀掉客户端进程，用新客户端查询原 task
    let socket = agentgate_lib::agent_gateway::ipc::ServicePaths::new(&db_dir)
        .socket
        .clone();
    client.kill();
    drop(client);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        agentgate_lib::agent_gateway::ipc::ServicePaths::new(&db_dir)
            .read_token()
            .is_ok(),
        "agent service must keep running after the MCP client exits"
    );
    let mut reconnected = McpClient::spawn(&db_dir).await;
    let recovered = reconnected
        .tool("agent_status", json!({"task_id": task_id}))
        .await;
    assert_eq!(recovered["state"], "completed");
    assert_eq!(recovered["final_answer"].as_str().unwrap(), answer);
    println!("reconnected to the same service at {}", socket.display());

    // 仍然可以用原 session_id 继续会话
    let third = reconnected
        .tool(
            "agent_continue",
            json!({"session_id": session_id, "prompt": "Reply with the single word: ok"}),
        )
        .await;
    let third = wait_terminal(
        &mut reconnected,
        third["task_id"].as_str().unwrap(),
        Duration::from_secs(420),
    )
    .await;
    assert_eq!(third["state"], "completed", "{third}");

    // 收尾：通过 IPC 优雅关闭服务并等待它退出。
    let paths = agentgate_lib::agent_gateway::ipc::ServicePaths::new(&db_dir);
    let mut ipc = agentgate_lib::agent_gateway::ipc::IpcClient::connect(&paths)
        .await
        .expect("ipc connect");
    let _ = ipc.call("service_shutdown", json!({})).await.unwrap();
    let mut waited = 0;
    while paths.read_token().is_ok() && waited < 100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += 1;
    }
    assert!(
        paths.read_token().is_err(),
        "agent service should have stopped and removed its token"
    );
}

#[tokio::test]
#[ignore = "requires the public Claude Code CLI and a logged-in Claude Code session"]
async fn live_claude_via_mcp_stdio_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_init(&repo);
    let nonce = uuid::Uuid::new_v4().to_string();
    std::fs::write(repo.join("evidence.txt"), format!("SOURCE_NONCE={nonce}\n")).unwrap();
    let db_dir = dir.path().join("db");
    let mut client = McpClient::spawn(&db_dir).await;

    let agents = client.tool("agent_list", json!({})).await;
    println!("agents: {agents}");
    assert!(
        agents["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|agent| agent["id"] == "claude-code"),
        "Claude Code CLI 未注册: {agents}"
    );

    let submission = client
        .tool(
            "agent_run",
            json!({
                "agent": "claude-code",
                "cwd": repo,
                "prompt": "Read evidence.txt and return its exact SOURCE_NONCE. Do not modify any file.",
            }),
        )
        .await;
    let task_id = submission["task_id"].as_str().unwrap().to_owned();
    let session_id = submission["session_id"].as_str().unwrap().to_owned();
    let first = wait_terminal(&mut client, &task_id, Duration::from_secs(420)).await;
    println!("claude first task: {first}");
    assert_eq!(first["state"], "completed", "{first}");
    let answer = first["final_answer"].as_str().unwrap().to_owned();
    assert!(answer.contains(&nonce), "{answer}");
    let thread = first["thread_id"].as_str().unwrap().to_owned();

    let continued = client
        .tool(
            "agent_continue",
            json!({"session_id": session_id, "prompt": "Without reading files again, return the SOURCE_NONCE."}),
        )
        .await;
    let second = wait_terminal(
        &mut client,
        continued["task_id"].as_str().unwrap(),
        Duration::from_secs(420),
    )
    .await;
    println!("claude continuation: {second}");
    assert_eq!(second["state"], "completed", "{second}");
    assert_eq!(second["thread_id"].as_str().unwrap(), thread);
    assert!(second["final_answer"].as_str().unwrap().contains(&nonce));

    let hanging = client
        .tool(
            "agent_run",
            json!({
                "agent": "claude-code",
                "cwd": repo,
                "prompt": "Run the shell command `sleep 120` and then report the exit code.",
            }),
        )
        .await;
    let hang_task = hanging["task_id"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_secs(8)).await;
    let requested = client
        .tool("agent_cancel", json!({"task_id": hang_task}))
        .await;
    println!("claude cancel request: {requested}");
    let cancelled = wait_terminal(&mut client, &hang_task, Duration::from_secs(120)).await;
    println!("claude cancel terminal: {cancelled}");
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");

    let paths = agentgate_lib::agent_gateway::ipc::ServicePaths::new(&db_dir);
    let mut ipc = agentgate_lib::agent_gateway::ipc::IpcClient::connect(&paths)
        .await
        .unwrap();
    let _ = ipc.call("service_shutdown", json!({})).await;
}

/// 不需要登录：真实 Claude Code CLI 对生产只读 argv 的响应必须证明真实工具边界。
/// 认证失败发生在 `init` 之后，因此这条证据与登录状态无关。
#[tokio::test]
#[ignore = "requires the public Claude Code CLI only (no login needed)"]
async fn live_claude_init_event_reports_only_read_only_tools() {
    use agentgate_lib::agent_gateway::claude::{
        discover_for_probe, read_only_cli_args, READ_ONLY_TOOLS,
    };

    let executable = discover_for_probe().await.expect("public Claude Code CLI");
    let repo = tempfile::tempdir().unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    let args = read_only_cli_args(&session, false);
    let output = tokio::process::Command::new(&executable)
        .args(&args)
        .arg("Reply with the single word: ok")
        .current_dir(repo.path())
        .stdin(Stdio::null())
        .output()
        .await
        .expect("run real claude CLI");
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!(
        "exit={} stdout_lines={}",
        output.status,
        stdout.lines().count()
    );
    let init = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["type"] == "system" && value["subtype"] == "init")
        .unwrap_or_else(|| {
            panic!(
                "real Claude Code emitted no init event (exit {}); stdout: {stdout}",
                output.status
            )
        });
    let tools: Vec<String> = init["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool.as_str().unwrap().to_owned())
        .collect();
    println!(
        "init: session={} tools={tools:?} mcp={}",
        init["session_id"], init["mcp_servers"]
    );
    assert_eq!(init["session_id"].as_str().unwrap(), session);
    for tool in &tools {
        assert!(
            READ_ONLY_TOOLS.contains(&tool.as_str()),
            "real CLI exposes a non read-only tool: {tools:?}"
        );
    }
    assert!(!tools.is_empty());
    assert_eq!(init["mcp_servers"], json!([]));
    for forbidden in ["Bash", "Edit", "Write", "Task", "WebFetch"] {
        assert!(!tools.contains(&forbidden.to_string()));
    }
}
