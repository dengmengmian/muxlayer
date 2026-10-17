//! CLI ownership must be acquired before opening/recovering persisted tasks.
#![cfg(unix)]
use agentgate_lib::agent_gateway::{
    ipc::{self, IpcClient, ServicePaths},
    PermissionMode, SessionRecord, TaskRecord, TaskState,
};
use serde_json::json;
use std::{process::Stdio, time::Duration};

#[tokio::test]
async fn rejected_second_service_does_not_recover_the_owners_running_tasks() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    // Discovery only: this regression must not depend on installed/authenticated agents.
    let codex = dir.path().join("codex-discovery-fixture");
    std::fs::write(
        &codex,
        "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\necho 'codex-cli 9.9.9'\n",
    )
    .unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o700)).unwrap();
    let paths = ServicePaths::new(dir.path());
    let mut owner = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentgate-serve"))
        .args(["agent-service", "--db-path"])
        .arg(dir.path())
        .env("AGENTGATE_CODEX_BIN", &codex)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut client = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(client) = IpcClient::connect(&paths).await {
                break client;
            }
            assert!(
                owner.try_wait().unwrap().is_none(),
                "owner exited before listening"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    // Seed an owner's durable in-flight record without invoking any model.
    let store = ipc::open_store(dir.path()).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    store
        .upsert_session(&SessionRecord {
            session_id: "owner-session".into(),
            agent: "codex-native".into(),
            cwd: dir.path().into(),
            permission: PermissionMode::ReadOnly,
            thread_id: None,
            active_task_id: Some("owner-task".into()),
            task_ids: vec!["owner-task".into()],
            created_at: now.clone(),
        })
        .unwrap();
    store
        .insert_task(&TaskRecord {
            task_id: "owner-task".into(),
            session_id: "owner-session".into(),
            prompt: "ownership regression".into(),
            state: TaskState::Running,
            thread_id: None,
            turn_id: None,
            final_answer: None,
            error: None,
            events: vec![],
            cancel_requested: false,
            created_at: now,
            finished_at: None,
        })
        .unwrap();
    let second = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agentgate-serve"))
            .args(["agent-service", "--db-path"])
            .arg(dir.path())
            .env("AGENTGATE_CODEX_BIN", &codex)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!second.status.success(), "second owner must be rejected");
    assert_eq!(
        store.task("owner-task").unwrap().unwrap().state,
        TaskState::Running,
        "rejected startup must not reconcile an existing owner's tasks"
    );
    client
        .call("service_shutdown", json!({}))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), owner.wait())
        .await
        .unwrap()
        .unwrap();
}

/// The Desktop writes CLI overrides into the CLI database. The headless
/// service must read that same database before discovery. An invalid explicit
/// Codex path must suppress the otherwise-available real Codex CLI; this proves
/// the setting is actually applied rather than silently ignored.
#[tokio::test]
async fn agent_service_uses_saved_cli_path_overrides() {
    use agentgate_lib::agent_gateway::settings::{set_bin, CLAUDE_BIN_KEY, CODEX_BIN_KEY};
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude-discovery-fixture");
    std::fs::write(
        &claude,
        "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\necho '2.1.999 (Claude Code)'\n",
    )
    .unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o700)).unwrap();
    let missing_codex = dir.path().join("missing-codex");
    set_bin(dir.path(), CODEX_BIN_KEY, Some(&missing_codex)).unwrap();
    set_bin(dir.path(), CLAUDE_BIN_KEY, Some(&claude)).unwrap();

    let paths = ServicePaths::new(dir.path());
    let mut service = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentgate-serve"))
        .args(["agent-service", "--db-path"])
        .arg(dir.path())
        .env_remove("AGENTGATE_CODEX_BIN")
        .env_remove("AGENTGATE_CLAUDE_BIN")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut client = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(client) = IpcClient::connect(&paths).await {
                break client;
            }
            assert!(
                service.try_wait().unwrap().is_none(),
                "service exited before listening"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let status = client
        .call("service_status", json!({}))
        .await
        .unwrap()
        .unwrap();
    let agents: Vec<&str> = status["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|agent| agent["id"].as_str())
        .collect();
    assert!(agents.contains(&"claude-code"), "{status}");
    assert!(
        !agents.contains(&"codex-native"),
        "invalid saved Codex path must override auto-discovery: {status}"
    );
    let _ = client.call("service_shutdown", json!({})).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), service.wait()).await;
}
