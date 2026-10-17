//! Explicit opt-in acceptance. Uses the public CLI and the user's existing ChatGPT login.
//! Run with AGENTGATE_CODEX_BIN=/path/to/codex cargo test ... --test
//! agent_gateway_codex_live -- --ignored --nocapture. No credentials are read by this test.
use agentgate_lib::agent_gateway::{
    codex::CodexFactory, AdapterEvent, AdapterFactory, AgentService, ContinueRequest,
    PermissionMode, RunRequest, TaskRecord, TaskState, TurnOutcome,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};

async fn terminal(service: &AgentService, task_id: &str) -> TaskRecord {
    tokio::time::timeout(Duration::from_secs(300), async {
        let mut ticks = tokio::time::interval(Duration::from_millis(25));
        loop {
            ticks.tick().await;
            let task = service.status(task_id).unwrap();
            if task.state.is_terminal() {
                return task;
            }
        }
    })
    .await
    .expect("real Codex task did not reach a terminal state")
}

fn completed(task: &TaskRecord) -> &str {
    assert_eq!(task.state, TaskState::Completed, "{task:?}");
    assert!(task.thread_id.is_some());
    assert!(task.turn_id.is_some());
    let answer = task.final_answer.as_deref().expect("missing final answer");
    assert!(!answer.trim().is_empty());
    answer
}

#[tokio::test]
#[ignore = "requires public Codex CLI, ChatGPT login and real model requests"]
async fn real_interrupt_immediately_after_turn_start_is_cancelable() {
    #[cfg(target_os = "macos")]
    std::env::set_var(
        "AGENTGATE_PROCESS_OWNER_BIN",
        env!("CARGO_BIN_EXE_agentgate-serve"),
    );
    let executable = std::env::var_os("AGENTGATE_CODEX_BIN").map(PathBuf::from);
    let factory = CodexFactory::discover(executable).await.unwrap();
    let adapter = factory.connect().await.unwrap();
    let repo = tempfile::tempdir().unwrap();
    let thread = adapter.start_thread(repo.path()).await.unwrap();
    let turn = adapter
        .start_turn(
            &thread,
            "Run exactly this command with exec_command and wait for it: sleep 120. \
             This is a startup interrupt acceptance test. Do not modify files.",
        )
        .await
        .unwrap();
    // This is the turn-start window: interrupt before any turn/started event
    // has been processed by this test.
    adapter
        .interrupt(&thread, &turn)
        .await
        .expect("startup interrupt must not race turn/start");
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match adapter.next_event().await.unwrap() {
                AdapterEvent::TurnFinished {
                    thread_id,
                    turn_id,
                    outcome,
                    ..
                } if thread_id == thread && turn_id == turn => return outcome,
                AdapterEvent::ProcessExited { message } => {
                    panic!("app-server exited before interrupt terminal: {message}")
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("interrupted turn did not reach a terminal state");
    assert_eq!(outcome, TurnOutcome::Interrupted);
    adapter.shutdown().await.unwrap();
    println!("Startup interrupt acceptance passed: thread={thread} turn={turn}");
}

#[tokio::test]
#[ignore = "requires public Codex CLI, ChatGPT login and real model requests"]
async fn real_codex_read_only_audit_continue_cancel() {
    // This integration harness is not the application executable hosting the
    // private macOS owner entry point. Use Cargo's actual CLI artifact explicitly.
    #[cfg(target_os = "macos")]
    std::env::set_var(
        "AGENTGATE_PROCESS_OWNER_BIN",
        env!("CARGO_BIN_EXE_agentgate-serve"),
    );
    let executable = std::env::var_os("AGENTGATE_CODEX_BIN").map(PathBuf::from);
    let factory = CodexFactory::discover(executable).await.unwrap();
    let service = AgentService::new(vec![Arc::new(factory)]).unwrap();
    let repo = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(repo.path())
        .status()
        .unwrap()
        .success());
    let nonce = uuid::Uuid::new_v4().to_string();
    let source = format!("SOURCE_NONCE={nonce}\nRead-only sandbox verification fixture.\n");
    std::fs::write(repo.path().join("evidence.txt"), &source).unwrap();
    let first = service
        .run(RunRequest {
            agent: "codex-native".into(),
            cwd: repo.path().into(),
            permission: PermissionMode::ReadOnly,
            prompt: "Read evidence.txt with a shell command and return its exact SOURCE_NONCE. Explain what the fixture says. Do not modify files.".into(),
        })
        .unwrap();
    let first_task = terminal(&service, &first.task_id).await;
    assert!(completed(&first_task).contains(&nonce));
    // Forced write rejection is verified through the official command/exec
    // sandbox in codex::tests::live_read_only_commands_are_denied. A model may
    // correctly refuse to attempt a write before invoking a tool.
    assert_eq!(
        std::fs::read_to_string(repo.path().join("evidence.txt")).unwrap(),
        source
    );
    assert!(!repo.path().join("forbidden.txt").exists());

    let second = service
        .continue_session(ContinueRequest {
            session_id: first.session_id.clone(),
            prompt: "Continue the previous analysis. Without reading files again, return the exact SOURCE_NONCE you read in the previous turn.".into(),
        })
        .unwrap();
    let second_task = terminal(&service, &second.task_id).await;
    assert!(completed(&second_task).contains(&nonce));
    assert_eq!(first.session_id, second.session_id);
    assert_eq!(first_task.thread_id, second_task.thread_id);
    assert_ne!(first_task.turn_id, second_task.turn_id);

    let audit = service
        .run(RunRequest {
            agent: "codex-native".into(),
            cwd: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."),
            permission: PermissionMode::ReadOnly,
            prompt: "只读审计当前项目权限架构，实际读取 src-tauri/src/security/cli_exposure.rs、src-tauri/src/security/local_token.rs、src-tauri/src/cli/serve.rs 的相关代码。找出一个最值得关注的权限边界，给出文件路径、代码行号证据和修复建议；若没有确认漏洞，明确区分疑问和漏洞。回答控制在1000字内。不要读取用户家目录、凭据或运行测试，不要委派子代理。".into(),
        })
        .unwrap();
    let audit_task = terminal(&service, &audit.task_id).await;
    let audit_answer = completed(&audit_task);
    assert!(audit_answer.contains("src-tauri/src/"));
    let followup = service
        .continue_session(ContinueRequest {
            session_id: audit.session_id.clone(),
            prompt: "继续刚才的分析，深入调查第一个问题。用源码证据说明触发条件和最小修复方向，500字内；不要修改文件，不要读取用户凭据，不要委派子代理。".into(),
        })
        .unwrap();
    let followup_task = terminal(&service, &followup.task_id).await;
    completed(&followup_task);
    assert_eq!(audit_task.thread_id, followup_task.thread_id);

    let cancellation = service
        .continue_session(ContinueRequest {
            session_id: first.session_id.clone(),
            prompt: "Run shell sleep 120 before replying. This is a cancellation verification. Do not return early.".into(),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(45), async {
        let mut ticks = tokio::time::interval(Duration::from_millis(10));
        loop {
            ticks.tick().await;
            let task = service.status(&cancellation.task_id).unwrap();
            assert!(
                !task.state.is_terminal(),
                "task finished before cancellation: {task:?}"
            );
            let sleep_started = task.events.iter().any(|event| {
                event.kind == "item/started"
                    && serde_json::from_str::<serde_json::Value>(&event.message)
                        .ok()
                        .is_some_and(|item| {
                            item["type"] == "commandExecution"
                                && item["command"]
                                    .as_str()
                                    .is_some_and(|command| command.contains("sleep 120"))
                        })
            });
            if task.state == TaskState::Running && task.turn_id.is_some() && sleep_started {
                break;
            }
        }
    })
    .await
    .unwrap();
    service.cancel(&cancellation.task_id).await.unwrap();
    let cancelled_task = terminal(&service, &cancellation.task_id).await;
    assert_eq!(
        cancelled_task.state,
        TaskState::Cancelled,
        "{cancelled_task:?}"
    );
    assert!(cancelled_task.cancel_requested);
    assert!(cancelled_task.turn_id.is_some());
    service.shutdown().await.unwrap();

    let evidence = json!({
        "platform": std::env::consts::OS,
        "sandbox_fixture": first_task,
        "continuation": second_task,
        "source_audit": audit_task,
        "source_audit_continuation": followup_task,
        "cancelled": cancelled_task,
    });
    if let Some(path) = std::env::var_os("AGENTGATE_LIVE_EVIDENCE") {
        std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    }
    println!("Real Codex acceptance passed: read-only, full final answer, original-thread continuation, source audit, cancellation.");
}
