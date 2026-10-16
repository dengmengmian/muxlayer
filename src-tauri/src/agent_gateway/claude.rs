//! Claude Code Adapter（第二阶段新增）。
//!
//! 复用第一阶段 `AgentAdapter` 契约，通过官方 `claude --print --output-format
//! stream-json` 程序化入口执行只读任务，不新建第二套 Task Manager，也不改变统一结果格式。
//!
//! 只读边界不依赖提示词：
//! - `--tools Read,Grep,Glob` 只保留读取类内置工具，模型根本没有写文件或执行命令的工具
//!   （Bash / Edit / Write / NotebookEdit / WebFetch / Task 都不存在）；
//! - `--strict-mcp-config` 不加载任何外部 MCP server；
//! - `--setting-sources user` 只加载用户级 settings，项目 / 本地 settings 一律忽略，
//!   仓库无法通过 `.claude/settings.json` 注入 hooks、MCP 或插件（有真实验证）；
//! - `--disable-slash-commands` 关闭 skills；
//! - `--permission-prompts none` 让任何需要审批的动作直接被拒绝。
//! 另外每个 Turn 的 `init` 事件会被校验：真实可用工具必须落在白名单内、MCP server 必须为空，
//! 否则该 Turn 直接失败，绝不把"提示词只读"当成强制只读。
//!
//! 为什么不加 `--restricted`：它会连用户级 settings 一起忽略，而 DeepSeek 等第三方
//! Anthropic 兼容端点正是通过用户 settings 的 `env`（或进程环境变量）配置的；实测
//! `--restricted` 会让 model 回退到内置默认模型并认证失败，等于让 Claude Code 只能跑
//! 官方登录。`--setting-sources user` 保留了"排除项目配置"这一安全边界，同时让用户
//! 自己的模型配置生效。代价（明确边界）：读取类工具不再被限制在 cwd 内，且用户级
//! hooks / 插件会生效——那是机器主人自己的配置，与凭据同一信任级别。
//!
//! 认证：复用本机 Claude Code 自己的登录/配置（`claude auth status` 预检），
//! 不读取、不输出、不复制任何凭据，也不在失败时切换到其它模型或 API Key。
//!
//! 取消：`claude --print` 是单次进程，官方没有 interrupt RPC；真实取消就是终止该进程
//! （SIGKILL），随后按 `interrupted` + `cancel_requested` 记为 `cancelled`。
use super::protocol::*;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, Mutex},
    task::JoinHandle,
};

/// 允许模型使用的内置工具白名单（全部是只读类）。
pub const READ_ONLY_TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// 单条过程事件文本上限。最终回答不截断，这里只限制过程日志体积。
const PROGRESS_CHARS: usize = 4000;

/// 环境变量中可能存在认证信息时的探测键。只检查是否存在，不读取值。
const AUTH_ENV_KEYS: [&str; 5] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
];

fn adapter(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::Adapter, message)
}
fn protocol(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::Protocol, message)
}
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Default)]
pub struct ClaudeFactory {
    executable: Option<PathBuf>,
}
impl ClaudeFactory {
    /// Construction is lazy; use `discover` to validate the public CLI before registration.
    pub fn new(executable: Option<PathBuf>) -> Self {
        Self { executable }
    }
    pub async fn discover(executable: Option<PathBuf>) -> Result<Self, AgentError> {
        Ok(Self::new(Some(discover(executable.as_deref()).await?)))
    }
    async fn connect_adapter(&self) -> Result<Arc<ClaudeAdapter>, AgentError> {
        let executable = discover(self.executable.as_deref()).await?;
        Ok(Arc::new(
            ClaudeAdapter::connect(executable, auth_env_present()).await?,
        ))
    }
}
impl AdapterFactory for ClaudeFactory {
    fn descriptor(&self) -> AgentDescriptor {
        AgentDescriptor {
            id: "claude-code".into(),
            name: "Claude Code".into(),
            permissions: vec![PermissionMode::ReadOnly],
            supports_continuation: true,
            supports_cancel: true,
        }
    }
    fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>> {
        Box::pin(async move { Ok(self.connect_adapter().await? as Arc<dyn AgentAdapter>) })
    }
    /// Claude Code 没有官方只读终局查询入口（会话数据是 CLI 私有磁盘格式），
    /// 不做猜测式解析：无法查证时返回 `None`，服务保留 `interrupted`。
    fn reconcile<'a>(
        &'a self,
        _session: &'a SessionRecord,
        _task: &'a TaskRecord,
    ) -> BoxFuture<'a, Result<Option<RecoveredTurn>, AgentError>> {
        Box::pin(async { Ok(None) })
    }
}

async fn discover(explicit: Option<&Path>) -> Result<PathBuf, AgentError> {
    let mut candidates = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p.to_owned());
    } else {
        if let Some(p) = std::env::var_os("AGENTGATE_CLAUDE_BIN") {
            candidates.push(p.into());
        }
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                candidates.push(dir.join(if cfg!(windows) {
                    "claude.exe"
                } else {
                    "claude"
                }));
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
            if o.status.success() {
                let stdout = String::from_utf8_lossy(&o.stdout);
                if stdout.contains("Claude Code") {
                    return Ok(p);
                }
            }
        }
        if explicit.is_some() {
            break;
        }
    }
    Err(adapter(
        "Claude Code CLI is unavailable or cannot execute. Install the public CLI or configure AGENTGATE_CLAUDE_BIN.",
    ))
}

fn auth_env_present() -> bool {
    AUTH_ENV_KEYS
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}

/// 认证预检：只判断"本机已有可用认证"，不读取也不输出凭据内容。
/// 无法判定时返回明确错误，而不是静默尝试其它模型或 API Key。
async fn verify_authentication(executable: &Path, env_auth: bool) -> Result<(), AgentError> {
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new(executable)
            .arg("auth")
            .arg("status")
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let output = match output {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(adapter(format!("Could not run `claude auth status`: {e}"))),
        Err(_) => return Err(adapter("`claude auth status` timed out")),
    };
    // 未登录时这个子命令以非零退出码报告状态，所以先解析输出再判断退出码。
    let parsed = serde_json::from_slice::<Value>(&output.stdout).ok();
    if let Some(status) = parsed {
        let logged_in = status["loggedIn"].as_bool().unwrap_or(false);
        let method = status["authMethod"].as_str().unwrap_or("none");
        if logged_in || method != "none" || env_auth {
            return Ok(());
        }
        return Err(adapter(NOT_AUTHENTICATED));
    }
    if !output.status.success() {
        return Err(adapter(format!(
            "could not determine Claude Code authentication: `claude auth status` exited with {}",
            output.status
        )));
    }
    Err(adapter(
        "could not determine Claude Code authentication: `claude auth status` did not return the expected JSON",
    ))
}

const NOT_AUTHENTICATED: &str =
    "Claude Code is not authenticated. Sign in with `claude auth login` \
     (or configure the CLI's own credentials), then retry. \
     MuxLayer does not read credentials, switch accounts or fall back to another model.";

/// 只做可执行文件发现（验证探针用），不做认证预检。
pub async fn discover_for_probe() -> Result<PathBuf, AgentError> {
    discover(None).await
}

/// 生产只读 argv。抽成模块级函数，让真实验证探针能用完全相同的参数调用真实 CLI。
pub fn read_only_cli_args(thread_id: &str, resume: bool) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--print".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--tools".into(),
        READ_ONLY_TOOLS.join(","),
        "--strict-mcp-config".into(),
        // 项目 / 本地 settings（含其中的 hooks、MCP、插件）一律不加载。
        "--setting-sources".into(),
        "user".into(),
        "--disable-slash-commands".into(),
        "--permission-prompts".into(),
        "none".into(),
    ];
    if resume {
        args.push("--resume".into());
    } else {
        args.push("--session-id".into());
    }
    args.push(thread_id.into());
    args
}

/// Turn 级的只读校验：用 CLI 自报的 init 事件确认真实可用工具与 MCP server。
fn validate_read_only_init(init: &Value) -> Result<(), AgentError> {
    let tools: Vec<String> = init["tools"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let unexpected: Vec<&String> = tools
        .iter()
        .filter(|t| !READ_ONLY_TOOLS.contains(&t.as_str()))
        .collect();
    if !unexpected.is_empty() {
        return Err(protocol(format!(
            "Claude Code reported tools outside the read-only allowlist: {unexpected:?}"
        )));
    }
    let servers = init["mcp_servers"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|s| s["name"].as_str().or_else(|| s.as_str()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !servers.is_empty() {
        return Err(protocol(format!(
            "Claude Code loaded MCP servers despite --strict-mcp-config: {servers:?}"
        )));
    }
    Ok(())
}

fn progress_text(message: &Value) -> String {
    let mut text = String::new();
    if let Some(blocks) = message["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
                Some("tool_use") => {
                    text.push_str(&format!("[tool {}]", block["name"].as_str().unwrap_or("")))
                }
                Some(other) => text.push_str(&format!("[{other}]")),
                None => {}
            }
            text.push('\n');
        }
    }
    let text = text.trim_end().to_owned();
    if text.chars().count() <= PROGRESS_CHARS {
        return text;
    }
    let head: String = text.chars().take(PROGRESS_CHARS).collect();
    format!("{head}…(progress truncated at {PROGRESS_CHARS} chars)")
}

struct ClaudeAdapter {
    executable: PathBuf,
    child: Mutex<Option<super::process::OwnedChild>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    events: Mutex<mpsc::UnboundedReceiver<Value>>,
    sender: mpsc::UnboundedSender<Value>,
    thread: Mutex<Option<String>>,
    cwd: Mutex<Option<PathBuf>>,
    turn: Mutex<Option<String>>,
    /// 被 cancel 终止的 turn：进程 EOF 时据此报告"真实取消"，而不是意外退出。
    interrupt_requested: Mutex<Option<String>>,
    resume_next: AtomicBool,
}

impl ClaudeAdapter {
    async fn connect(executable: PathBuf, env_auth: bool) -> Result<Self, AgentError> {
        verify_authentication(&executable, env_auth).await?;
        let (sender, events) = mpsc::unbounded_channel();
        Ok(Self {
            executable,
            child: Mutex::new(None),
            reader: Mutex::new(None),
            events: Mutex::new(events),
            sender,
            thread: Mutex::new(None),
            cwd: Mutex::new(None),
            turn: Mutex::new(None),
            interrupt_requested: Mutex::new(None),
            resume_next: AtomicBool::new(false),
        })
    }

    /// 只读任务参数。`--tools` 是真实工具白名单，不是提示词约束。
    fn read_only_arguments(&self, thread_id: &str, resume: bool) -> Vec<String> {
        read_only_cli_args(thread_id, resume)
    }

    async fn read_turn(
        &self,
        thread_id: &str,
        cwd: &Path,
        prompt: &str,
        resume: bool,
    ) -> Result<String, AgentError> {
        let mut cmd = Command::new(&self.executable);
        cmd.args(self.read_only_arguments(thread_id, resume))
            .arg(prompt)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // 与 Codex Adapter 一致：stderr 不进入错误消息，避免诊断文本泄露凭据。
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = super::process::spawn(&mut cmd)
            .map_err(|e| adapter(format!("could not launch Claude Code: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| protocol("missing Claude Code stdout"))?;
        let turn_id = uuid::Uuid::new_v4().to_string();
        let sender = self.sender.clone();
        let thread = thread_id.to_owned();
        let turn = turn_id.clone();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
                            let _ =
                                sender.send(tagged(&thread, &turn, json!({"_parse_error": true})));
                            break;
                        };
                        if sender.send(tagged(&thread, &turn, message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        let _ = sender.send(tagged(&thread, &turn, json!({"_parse_error": true})));
                        break;
                    }
                }
            }
            let _ = sender.send(tagged(&thread, &turn, json!({"_eof": true})));
        });
        *self.reader.lock().await = Some(reader);
        *self.child.lock().await = Some(child);
        *self.turn.lock().await = Some(turn_id.clone());
        Ok(turn_id)
    }
}

fn tagged(thread: &str, turn: &str, message: Value) -> Value {
    json!({"_thread": thread, "_turn": turn, "_message": message})
}

impl Drop for ClaudeAdapter {
    fn drop(&mut self) {
        // kill_on_drop 负责回收子进程；reader 任务由 shutdown 或被丢弃时结束。
        if let Ok(mut reader) = self.reader.try_lock() {
            if let Some(reader) = reader.take() {
                reader.abort();
            }
        }
    }
}

impl AgentAdapter for ClaudeAdapter {
    fn start_thread<'a>(&'a self, cwd: &'a Path) -> BoxFuture<'a, Result<String, AgentError>> {
        Box::pin(async move {
            let cwd = std::fs::canonicalize(cwd).map_err(|e| {
                AgentError::new(ErrorCode::InvalidRequest, format!("invalid cwd: {e}"))
            })?;
            if !cwd.is_dir() {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "cwd must be a directory",
                ));
            }
            // Claude Code 的会话即"Thread"。用 CLI 官方 `--session-id` 采用我们生成的 UUID，
            // 真实创建由首个 Turn 的 init 事件回显确认（见 next_event 校验）。
            let thread_id = uuid::Uuid::new_v4().to_string();
            *self.thread.lock().await = Some(thread_id.clone());
            *self.cwd.lock().await = Some(cwd);
            self.resume_next.store(false, Ordering::SeqCst);
            Ok(thread_id)
        })
    }

    fn resume_thread<'a>(
        &'a self,
        thread_id: &'a str,
        cwd: &'a Path,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            uuid::Uuid::parse_str(thread_id).map_err(|_| {
                AgentError::new(
                    ErrorCode::InvalidRequest,
                    "Claude Code session id must be a UUID",
                )
            })?;
            let cwd = std::fs::canonicalize(cwd).map_err(|e| {
                AgentError::new(ErrorCode::InvalidRequest, format!("invalid cwd: {e}"))
            })?;
            if !cwd.is_dir() {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "cwd must be a directory",
                ));
            }
            // 会话是否真实存在由 CLI 的 --resume 在 start_turn 里裁决：不存在会明确报错，
            // 不用私有磁盘格式猜测。
            *self.thread.lock().await = Some(thread_id.to_owned());
            *self.cwd.lock().await = Some(cwd);
            self.resume_next.store(true, Ordering::SeqCst);
            Ok(())
        })
    }

    fn start_turn<'a>(
        &'a self,
        thread_id: &'a str,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<String, AgentError>> {
        Box::pin(async move {
            if self.thread.lock().await.as_deref() != Some(thread_id) {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "turn does not belong to the current Claude Code session",
                ));
            }
            let cwd = self
                .cwd
                .lock()
                .await
                .clone()
                .ok_or_else(|| AgentError::new(ErrorCode::InvalidRequest, "cwd is not set"))?;
            let resume = self.resume_next.swap(false, Ordering::SeqCst);
            self.read_turn(thread_id, &cwd, prompt, resume).await
        })
    }

    fn next_event(&self) -> BoxFuture<'_, Result<AdapterEvent, AgentError>> {
        Box::pin(async move {
            loop {
                let tagged_message = self.events.lock().await.recv().await.ok_or_else(|| {
                    AgentError::new(ErrorCode::ProcessExited, "Claude Code event stream closed")
                })?;
                let thread_id = tagged_message["_thread"].as_str().unwrap_or("").to_owned();
                let turn_id = tagged_message["_turn"].as_str().unwrap_or("").to_owned();
                let message = &tagged_message["_message"];
                {
                    let current_turn = self.turn.lock().await;
                    if current_turn.as_deref() != Some(turn_id.as_str()) {
                        // 已被取消的旧 Turn 的迟到事件，直接丢弃。
                        continue;
                    }
                }
                if !message["_parse_error"].is_null() {
                    return Err(protocol("Claude Code emitted malformed JSON"));
                }
                if !message["_eof"].is_null() {
                    let cancelled = {
                        let mut requested = self.interrupt_requested.lock().await;
                        if requested.as_deref() == Some(turn_id.as_str()) {
                            *requested = None;
                            true
                        } else {
                            false
                        }
                    };
                    if cancelled {
                        return Ok(AdapterEvent::TurnFinished {
                            thread_id,
                            turn_id,
                            outcome: TurnOutcome::Interrupted,
                            final_answer: None,
                            error: Some(adapter(
                                "Claude Code process was terminated by the cancel request",
                            )),
                        });
                    }
                    let status = self
                        .child
                        .lock()
                        .await
                        .as_mut()
                        .and_then(|child| child.try_wait().ok().flatten())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "unknown".into());
                    return Ok(AdapterEvent::ProcessExited {
                        message: format!("Claude Code exited with status {status}"),
                    });
                }
                match message["type"].as_str().unwrap_or("") {
                    "system" if message["subtype"] == "init" => {
                        let reported = message["session_id"].as_str().unwrap_or("");
                        if reported != thread_id {
                            return Err(protocol(format!(
                                "Claude Code started session {reported} but {thread_id} was requested"
                            )));
                        }
                        validate_read_only_init(message)?;
                        return Ok(AdapterEvent::Progress {
                            thread_id,
                            turn_id,
                            kind: "init".into(),
                            message: json!({
                                "model": message["model"],
                                "permission_mode": message["permissionMode"],
                                "tools": message["tools"],
                                "mcp_servers": message["mcp_servers"],
                                "api_key_source": message["apiKeySource"],
                                "started_at": now_secs(),
                            })
                            .to_string(),
                        });
                    }
                    "result" => {
                        let reported = message["session_id"].as_str().unwrap_or("");
                        if reported != thread_id {
                            return Err(protocol(format!(
                                "Claude Code result belongs to session {reported}, not {thread_id}"
                            )));
                        }
                        let is_error = message["is_error"].as_bool().unwrap_or(false);
                        let text = message["result"].as_str().unwrap_or("").to_owned();
                        if is_error {
                            let reason = if text.trim().is_empty() {
                                message["subtype"]
                                    .as_str()
                                    .unwrap_or("unknown error")
                                    .to_owned()
                            } else {
                                text
                            };
                            return Ok(AdapterEvent::TurnFinished {
                                thread_id,
                                turn_id,
                                outcome: TurnOutcome::Failed,
                                final_answer: None,
                                error: Some(AgentError::new(ErrorCode::Adapter, reason)),
                            });
                        }
                        if text.trim().is_empty() {
                            return Ok(AdapterEvent::TurnFinished {
                                thread_id,
                                turn_id,
                                outcome: TurnOutcome::Completed,
                                final_answer: None,
                                error: None,
                            });
                        }
                        return Ok(AdapterEvent::TurnFinished {
                            thread_id,
                            turn_id,
                            outcome: TurnOutcome::Completed,
                            final_answer: Some(text),
                            error: None,
                        });
                    }
                    "assistant" => {
                        return Ok(AdapterEvent::Progress {
                            thread_id,
                            turn_id,
                            kind: "assistant".into(),
                            message: progress_text(&message["message"]),
                        });
                    }
                    other => {
                        return Ok(AdapterEvent::Progress {
                            thread_id,
                            turn_id,
                            kind: other.to_owned(),
                            message: progress_text(message),
                        });
                    }
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
            if self.thread.lock().await.as_deref() != Some(thread_id) {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "interrupt does not belong to the current Claude Code session",
                ));
            }
            if self.turn.lock().await.as_deref() != Some(turn_id) {
                return Err(AgentError::new(
                    ErrorCode::InvalidRequest,
                    "interrupt does not belong to the current Claude Code turn",
                ));
            }
            // 先登记取消意图再终止进程，避免 EOF 与 kill 竞态被当成意外退出。
            *self.interrupt_requested.lock().await = Some(turn_id.to_owned());
            if let Some(child) = self.child.lock().await.as_mut() {
                // 进程可能刚好自己退出：真实终局由随后的 EOF 事件决定，
                // 这里不把"已经退出"当成取消失败。
                let _ = child.start_kill();
            }
            Ok(())
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            if let Some(turn) = self.turn.lock().await.clone() {
                *self.interrupt_requested.lock().await = Some(turn);
            }
            if let Some(child) = self.child.lock().await.as_mut() {
                let _ = child.start_kill();
            }
            let child = self.child.lock().await.take();
            if let Some(mut child) = child {
                if tokio::time::timeout(Duration::from_secs(5), child.wait())
                    .await
                    .is_err()
                {
                    return Err(protocol("Claude Code did not exit after termination"));
                }
            }
            if let Some(reader) = self.reader.lock().await.take() {
                reader.abort();
            }
            Ok(())
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;

    /// 用真实进程验证 Adapter：stub CLI 记录 argv/cwd，并按脚本输出 stream-json。
    fn stub_script() -> String {
        r#"#!/bin/sh
LOG="${AGENTGATE_STUB_LOG:-/dev/null}"
printf '%s\n' "ARGV:$*" >> "$LOG"
printf '%s\n' "CWD:$(pwd)" >> "$LOG"
case "$1" in
  --version) echo "9.9.9 (Claude Code)"; exit 0;;
  auth) if [ "$2" = "status" ]; then
          printf '{"loggedIn":%s,"authMethod":"%s"}\n' "${AGENTGATE_STUB_LOGGED_IN:-true}" "${AGENTGATE_STUB_AUTH_METHOD:-claudeai}"
          exit 0
        fi;;
esac
SESSION=""
MODE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --session-id) SESSION="$2"; MODE="session-id"; shift 2;;
    --resume) SESSION="$2"; MODE="resume"; shift 2;;
    *) shift;;
  esac
done
printf '%s\n' "MODE:$MODE:$SESSION" >> "$LOG"
case "${AGENTGATE_STUB_CASE:-ok}" in
  tools_violation) TOOLS='["Read","Bash"]';;
  *) TOOLS='["Glob","Grep","Read"]';;
esac
if [ "${AGENTGATE_STUB_CASE:-ok}" = "hang" ]; then
  sleep 60
  exit 0
fi
printf '{"type":"system","subtype":"init","session_id":"%s","tools":%s,"mcp_servers":[],"model":"stub-model","permissionMode":"default","apiKeySource":"none"}\n' "$SESSION" "$TOOLS"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"looking at files"}]},"session_id":"%s"}\n' "$SESSION"
if [ "${AGENTGATE_STUB_CASE:-ok}" = "error" ]; then
  printf '{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate","session_id":"%s"}\n' "$SESSION"
  exit 1
fi
printf '{"type":"result","subtype":"success","is_error":false,"result":"FINAL ANSWER %s:%s","session_id":"%s"}\n' "$MODE" "$SESSION" "$SESSION"
"#
            .to_string()
    }

    struct Stub {
        _dir: tempfile::TempDir,
        executable: PathBuf,
        log: PathBuf,
        cwd: PathBuf,
    }
    impl Stub {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let executable = dir.path().join("claude-stub.sh");
            let mut file = std::fs::File::create(&executable).unwrap();
            file.write_all(stub_script().as_bytes()).unwrap();
            drop(file);
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            let cwd = dir.path().join("repo");
            std::fs::create_dir(&cwd).unwrap();
            Self {
                log: dir.path().join("log"),
                _dir: dir,
                executable,
                cwd,
            }
        }
        fn entries(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }
        /// 环境变量是进程全局的，测试串行执行，避免互相覆盖。
        async fn adapter(&self, case: &str) -> ClaudeAdapter {
            std::env::set_var("AGENTGATE_STUB_LOG", &self.log);
            std::env::set_var("AGENTGATE_STUB_CASE", case);
            std::env::set_var("AGENTGATE_STUB_LOGGED_IN", "true");
            std::env::set_var("AGENTGATE_STUB_AUTH_METHOD", "claudeai");
            ClaudeAdapter::connect(self.executable.clone(), false)
                .await
                .unwrap()
        }
    }

    async fn collect(adapter: &ClaudeAdapter) -> Vec<AdapterEvent> {
        let mut events = vec![];
        for _ in 0..40 {
            match tokio::time::timeout(Duration::from_secs(10), adapter.next_event()).await {
                Ok(Ok(event)) => {
                    let terminal = matches!(
                        event,
                        AdapterEvent::TurnFinished { .. } | AdapterEvent::ProcessExited { .. }
                    );
                    events.push(event);
                    if terminal {
                        break;
                    }
                }
                Ok(Err(e)) => panic!("adapter error: {e}"),
                Err(_) => panic!("adapter produced no event in time"),
            }
        }
        events
    }

    fn final_answer(events: &[AdapterEvent]) -> Option<String> {
        events.iter().find_map(|e| match e {
            AdapterEvent::TurnFinished {
                final_answer: Some(answer),
                ..
            } => Some(answer.clone()),
            _ => None,
        })
    }

    fn fake_adapter() -> ClaudeAdapter {
        let (sender, events) = mpsc::unbounded_channel();
        ClaudeAdapter {
            executable: PathBuf::from("/nonexistent"),
            child: Mutex::new(None),
            reader: Mutex::new(None),
            events: Mutex::new(events),
            sender,
            thread: Mutex::new(None),
            cwd: Mutex::new(None),
            turn: Mutex::new(None),
            interrupt_requested: Mutex::new(None),
            resume_next: AtomicBool::new(false),
        }
    }

    #[test]
    fn read_only_arguments_never_include_write_or_shell_tools() {
        let adapter = fake_adapter();
        let args = adapter.read_only_arguments("11111111-1111-4111-8111-111111111111", true);
        let tools = args
            .iter()
            .position(|a| a == "--tools")
            .map(|i| args[i + 1].clone())
            .unwrap();
        for forbidden in ["Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "Task"] {
            assert!(
                !tools.contains(forbidden),
                "{tools} must not allow {forbidden}"
            );
        }
        for allowed in READ_ONLY_TOOLS {
            assert!(tools.contains(allowed));
        }
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        let sources = args
            .iter()
            .position(|a| a == "--setting-sources")
            .map(|i| args[i + 1].clone())
            .expect("--setting-sources is required");
        assert_eq!(sources, "user", "只能加载用户级 settings");
        assert!(
            !args.contains(&"--restricted".to_string()),
            "--restricted 会连用户级模型配置一起忽略，第三方端点将无法使用"
        );
        assert!(args.contains(&"--disable-slash-commands".to_string()));
        assert!(args.contains(&"--permission-prompts".to_string()));
        assert!(args
            .windows(2)
            .any(|w| w == ["--resume", "11111111-1111-4111-8111-111111111111"]));
    }

    #[test]
    fn init_outside_the_read_only_allowlist_is_rejected() {
        assert!(
            validate_read_only_init(&json!({"tools":["Read","Grep","Glob"],"mcp_servers":[]}))
                .is_ok()
        );
        let err = validate_read_only_init(&json!({"tools":["Read","Bash"],"mcp_servers":[]}))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Protocol);
        let err = validate_read_only_init(&json!({"tools":["Read"],"mcp_servers":[{"name":"fs"}]}))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Protocol);
    }

    #[tokio::test]
    #[serial(env)]
    async fn real_process_executes_a_turn_and_uses_resume_for_continuation() {
        let stub = Stub::new();
        let adapter = stub.adapter("ok").await;
        let thread = adapter.start_thread(&stub.cwd).await.unwrap();
        let turn = adapter.start_turn(&thread, "audit the repo").await.unwrap();
        assert_ne!(turn, thread);
        let events = collect(&adapter).await;
        assert_eq!(
            final_answer(&events).unwrap(),
            format!("FINAL ANSWER session-id:{thread}")
        );

        adapter.resume_thread(&thread, &stub.cwd).await.unwrap();
        adapter.start_turn(&thread, "continue").await.unwrap();
        let events = collect(&adapter).await;
        assert_eq!(
            final_answer(&events).unwrap(),
            format!("FINAL ANSWER resume:{thread}")
        );

        let entries = stub.entries();
        assert!(entries
            .iter()
            .any(|e| e.contains(&format!("MODE:session-id:{thread}"))));
        assert!(entries
            .iter()
            .any(|e| e.contains(&format!("MODE:resume:{thread}"))));
        let argv = entries
            .iter()
            .find(|e| e.starts_with("ARGV:") && e.contains("--tools"))
            .unwrap();
        assert!(argv.contains("--tools Read,Grep,Glob"), "{argv}");
        assert!(argv.contains("--setting-sources user"), "{argv}");
        assert!(argv.contains("--strict-mcp-config"));
        assert!(!argv.contains("--dangerously-skip-permissions"));
        // Turn 必须在请求的 cwd 里执行（真实目录，不是随机临时目录）。
        let canonical = stub.cwd.canonicalize().unwrap();
        assert!(
            entries
                .iter()
                .any(|e| *e == format!("CWD:{}", canonical.display())),
            "{entries:?}"
        );
        adapter.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[serial(env)]
    async fn a_turn_that_reports_extra_tools_fails_instead_of_pretending() {
        let stub = Stub::new();
        let adapter = stub.adapter("tools_violation").await;
        let thread = adapter.start_thread(&stub.cwd).await.unwrap();
        adapter.start_turn(&thread, "audit").await.unwrap();
        let err = tokio::time::timeout(Duration::from_secs(10), adapter.next_event())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Protocol);
        assert!(err.message.contains("Bash"), "{}", err.message);
        adapter.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[serial(env)]
    async fn authentication_failure_is_reported_without_falling_back() {
        let stub = Stub::new();
        std::env::set_var("AGENTGATE_STUB_LOGGED_IN", "false");
        std::env::set_var("AGENTGATE_STUB_AUTH_METHOD", "none");
        let err = ClaudeAdapter::connect(stub.executable.clone(), false)
            .await
            .err()
            .expect("unauthenticated Claude Code must not connect");
        assert_eq!(err.code, ErrorCode::Adapter);
        assert!(err.message.contains("not authenticated"), "{}", err.message);
    }

    #[tokio::test]
    #[serial(env)]
    async fn a_real_error_result_is_reported_as_failed_with_the_cli_message() {
        let stub = Stub::new();
        let adapter = stub.adapter("error").await;
        let thread = adapter.start_thread(&stub.cwd).await.unwrap();
        adapter.start_turn(&thread, "audit").await.unwrap();
        let events = collect(&adapter).await;
        let (outcome, error) = events
            .iter()
            .find_map(|e| match e {
                AdapterEvent::TurnFinished { outcome, error, .. } => {
                    Some((*outcome, error.clone()))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(outcome, TurnOutcome::Failed);
        assert_eq!(error.unwrap().message, "Failed to authenticate");
        assert!(final_answer(&events).is_none());
        adapter.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[serial(env)]
    async fn cancel_terminates_the_real_process_and_reports_interruption() {
        let stub = Stub::new();
        let adapter = stub.adapter("hang").await;
        let thread = adapter.start_thread(&stub.cwd).await.unwrap();
        let turn = adapter.start_turn(&thread, "audit").await.unwrap();
        let started = std::time::Instant::now();
        adapter.interrupt(&thread, &turn).await.unwrap();
        let events = collect(&adapter).await;
        assert!(
            events.iter().any(|e| matches!(
                e,
                AdapterEvent::TurnFinished {
                    outcome: TurnOutcome::Interrupted,
                    ..
                }
            )),
            "{events:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(30));
        adapter.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[serial(env)]
    async fn resuming_an_unknown_session_fails_loudly_at_the_cli() {
        let stub = Stub::new();
        let adapter = stub.adapter("ok").await;
        let err = adapter
            .resume_thread("not-a-uuid", &stub.cwd)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }
}
