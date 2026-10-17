//! Orchestration over a write-through store. TaskRecord/SessionRecord are storage DTOs;
//! adapters, workers, locks, channels and JoinHandles must never be persisted.
//! SQLite (see `store.rs`) is the single source of truth for Task / Session state.
use super::protocol::*;
use super::store::{MemoryStore, TaskStore};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::task::JoinHandle;

pub struct AgentService {
    inner: Arc<Inner>,
}
struct Inner {
    factories: HashMap<String, Arc<dyn AdapterFactory>>,
    store: Arc<dyn TaskStore>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: bool,
    initializing: HashSet<String>,
    sessions: HashMap<String, Session>,
    tasks: HashMap<String, TaskRecord>,
    /// 无法提交的终态必须以错误暴露，不能让未落库的缓存冒充已完成。
    persistence_errors: HashMap<String, AgentError>,
    workers: Vec<JoinHandle<()>>,
    /// 重启时被判定为 `interrupted` 但可以继续用官方协议对账的任务。
    pending_reconcile: Vec<String>,
}
struct Session {
    record: SessionRecord,
    adapter: Option<Arc<dyn AgentAdapter>>,
    cleanup_error: Option<AgentError>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        // Aborting drops the worker's adapter Arc; adapter Drop owns process cleanup.
        if let Ok(state) = self.state.get_mut() {
            for worker in &state.workers {
                worker.abort();
            }
        }
    }
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn error(code: ErrorCode, message: &str) -> AgentError {
    AgentError::new(code, message)
}
fn validate_prompt(prompt: &str) -> Result<(), AgentError> {
    if prompt.trim().is_empty() {
        Err(error(ErrorCode::InvalidRequest, "prompt must not be empty"))
    } else {
        Ok(())
    }
}
const RESTART_REASON: &str = "service restarted before the task reached a terminal state; provider execution state is unknown until reconciled";

impl AgentService {
    /// Must be used inside a Tokio runtime. Register only factories whose executable
    /// discovery succeeded; authentication and source validation happen on connect.
    /// In-memory store: tests only. Production hosts must use [`AgentService::with_store`].
    pub fn new(factories: Vec<Arc<dyn AdapterFactory>>) -> Result<Self, AgentError> {
        Self::with_store(factories, MemoryStore::new())
    }

    /// Durable variant. Loads persisted Session / Task state and reconciles it before
    /// accepting new work: a task that was `queued` / `running` when the service died can
    /// never be reported as successful, because the provider process did not survive.
    pub fn with_store(
        factories: Vec<Arc<dyn AdapterFactory>>,
        store: Arc<dyn TaskStore>,
    ) -> Result<Self, AgentError> {
        let service = Self::build(factories, store)?;
        service.load_persisted_state()?;
        Ok(service)
    }

    fn build(
        factories: Vec<Arc<dyn AdapterFactory>>,
        store: Arc<dyn TaskStore>,
    ) -> Result<Self, AgentError> {
        let mut registered = HashMap::new();
        for factory in factories {
            let descriptor = factory.descriptor();
            if descriptor.id.trim().is_empty()
                || registered.insert(descriptor.id, factory).is_some()
            {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "empty or duplicate agent id",
                ));
            }
        }
        Ok(Self {
            inner: Arc::new(Inner {
                factories: registered,
                store,
                state: Mutex::new(State::default()),
            }),
        })
    }

    /// 启动恢复：装载持久化状态，并把重启时仍未终态的任务降为 `interrupted`。
    /// 不确定的旧状态不允许保留为 `running`，否则会留下僵死任务。
    fn load_persisted_state(&self) -> Result<(), AgentError> {
        let (sessions, tasks) = self.inner.store.load()?;
        let mut state = self.inner.state.lock().unwrap();
        for session in sessions {
            state.sessions.insert(
                session.session_id.clone(),
                Session {
                    record: session,
                    adapter: None,
                    cleanup_error: None,
                },
            );
        }
        for mut task in tasks {
            if !task.state.is_terminal() {
                task.state = TaskState::Interrupted;
                task.finished_at = Some(now());
                task.error = Some(error(ErrorCode::ProcessExited, RESTART_REASON));
                let session_id = task.session_id.clone();
                if let Some(session) = state.sessions.get_mut(&session_id) {
                    if session.record.active_task_id.as_deref() == Some(&task.task_id) {
                        session.record.active_task_id = None;
                    }
                }
                let session = state
                    .sessions
                    .get(&session_id)
                    .ok_or_else(|| error(ErrorCode::Storage, "task without persisted session"))?
                    .record
                    .clone();
                self.inner.store.finish_task(&task, &session)?;
            }
            // 对账前再次退出也不能丢失待查证身份；恢复错误本身已持久化。
            if task.state == TaskState::Interrupted
                && task.error.as_ref().is_some_and(|e| {
                    e.code == ErrorCode::ProcessExited && e.message == RESTART_REASON
                })
            {
                state.pending_reconcile.push(task.task_id.clone());
            }
            state.tasks.insert(task.task_id.clone(), task);
        }
        Ok(())
    }

    /// 用官方协议查证重启前未终态的任务。只有 provider 明确确认 `completed` 且带完整
    /// 最终回答时才升级为 `completed`；否则保留 `interrupted`。绝不重新执行任务。
    pub async fn reconcile_interrupted(&self) -> Result<usize, AgentError> {
        let pending: Vec<String> = {
            let mut state = self.inner.state.lock().unwrap();
            std::mem::take(&mut state.pending_reconcile)
        };
        let mut upgraded = 0;
        let mut first_error = None;
        for task_id in pending {
            let (session, task, factory) = {
                let state = self.inner.state.lock().unwrap();
                let Some(task) = state.tasks.get(&task_id).cloned() else {
                    continue;
                };
                let Some(session) = state
                    .sessions
                    .get(&task.session_id)
                    .map(|s| s.record.clone())
                else {
                    continue;
                };
                let agent = state.sessions[&task.session_id].record.agent.clone();
                let Some(factory) = self.inner.factories.get(&agent).cloned() else {
                    continue;
                };
                (session, task, factory)
            };
            let recovered = match factory.reconcile(&session, &task).await {
                Ok(value) => value,
                Err(e) => {
                    // 查证失败不改变终态，只记录，避免把无法确认的任务伪装成完成。
                    first_error.get_or_insert(e);
                    continue;
                }
            };
            let Some(recovered) = recovered else {
                continue;
            };
            let (state_now, err) = match recovered.outcome {
                TurnOutcome::Completed
                    if recovered.error.is_none()
                        && recovered
                            .final_answer
                            .as_ref()
                            .is_some_and(|a| !a.trim().is_empty()) =>
                {
                    (TaskState::Completed, None)
                }
                TurnOutcome::Completed => (
                    TaskState::Failed,
                    Some(error(
                        ErrorCode::Protocol,
                        "reconciled turn reports success but has no final answer",
                    )),
                ),
                TurnOutcome::Failed => (
                    TaskState::Failed,
                    Some(
                        recovered
                            .error
                            .unwrap_or_else(|| error(ErrorCode::Adapter, "turn failed")),
                    ),
                ),
                TurnOutcome::Cancelled => (TaskState::Cancelled, recovered.error),
                TurnOutcome::Interrupted => (TaskState::Interrupted, recovered.error),
                TurnOutcome::Unknown => (
                    TaskState::Interrupted,
                    Some(error(
                        ErrorCode::Protocol,
                        "reconciled turn outcome is unknown",
                    )),
                ),
            };
            let applied = {
                let mut state = self.inner.state.lock().unwrap();
                let Some(mut task) = state.tasks.get(&task_id).cloned() else {
                    continue;
                };
                task.state = state_now;
                task.final_answer = recovered.final_answer;
                task.error = err;
                task.finished_at = Some(now());
                let session = state
                    .sessions
                    .get(&session.session_id)
                    .map(|s| s.record.clone());
                session.map(|session| {
                    self.inner.store.finish_task(&task, &session)?;
                    state.tasks.insert(task_id.clone(), task);
                    Ok(())
                })
            };
            match applied {
                Some(Ok(())) => upgraded += 1,
                Some(Err(e)) => {
                    first_error.get_or_insert(e);
                }
                None => {}
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(upgraded),
        }
    }

    pub fn list(&self) -> Vec<AgentDescriptor> {
        let mut agents: Vec<_> = self
            .inner
            .factories
            .values()
            .map(|f| f.descriptor())
            .collect();
        agents.sort_by(|a, b| a.id.cmp(&b.id));
        agents
    }
    pub fn run(&self, request: RunRequest) -> Result<Submission, AgentError> {
        validate_prompt(&request.prompt)?;
        if request.permission != PermissionMode::ReadOnly {
            return Err(error(
                ErrorCode::UnsupportedPermission,
                "execute is not enabled",
            ));
        }
        let factory = self
            .inner
            .factories
            .get(&request.agent)
            .ok_or_else(|| error(ErrorCode::UnknownAgent, "unknown agent"))?;
        if !factory
            .descriptor()
            .permissions
            .contains(&request.permission)
        {
            return Err(error(
                ErrorCode::UnsupportedPermission,
                "agent does not support read_only",
            ));
        }
        let cwd = std::fs::canonicalize(&request.cwd)
            .map_err(|e| AgentError::new(ErrorCode::InvalidRequest, format!("invalid cwd: {e}")))?;
        if !cwd.is_dir() {
            return Err(error(ErrorCode::InvalidRequest, "cwd must be a directory"));
        }
        let session_id = id();
        if self.inner.state.lock().unwrap().closed {
            return Err(error(ErrorCode::Closed, "service is shut down"));
        }
        // 先落库再改内存：持久化失败时不能返回一个只在内存里存在的 Session。
        let session_record = SessionRecord {
            session_id: session_id.clone(),
            agent: request.agent,
            cwd,
            permission: request.permission,
            thread_id: None,
            active_task_id: None,
            task_ids: vec![],
            created_at: now(),
        };
        self.inner.store.upsert_session(&session_record)?;
        let mut state = self.inner.state.lock().unwrap();
        if state.closed {
            return Err(error(ErrorCode::Closed, "service is shut down"));
        }
        state.sessions.insert(
            session_id.clone(),
            Session {
                record: session_record,
                adapter: None,
                cleanup_error: None,
            },
        );
        self.submit(&mut state, session_id, request.prompt)
    }
    pub fn continue_session(&self, request: ContinueRequest) -> Result<Submission, AgentError> {
        validate_prompt(&request.prompt)?;
        let mut state = self.inner.state.lock().unwrap();
        if state.closed {
            return Err(error(ErrorCode::Closed, "service is shut down"));
        }
        let session = state
            .sessions
            .get(&request.session_id)
            .ok_or_else(|| error(ErrorCode::NotFound, "session not found"))?;
        if let Some(err) = &session.cleanup_error {
            return Err(err.clone());
        }
        if session.record.active_task_id.is_some() {
            return Err(error(ErrorCode::Busy, "session already has an active task"));
        }
        let descriptor = self
            .inner
            .factories
            .get(&session.record.agent)
            .ok_or_else(|| error(ErrorCode::UnknownAgent, "session agent is not registered"))?
            .descriptor();
        if !descriptor.supports_continuation {
            return Err(error(
                ErrorCode::InvalidRequest,
                "agent does not support continuation",
            ));
        }
        self.submit(&mut state, request.session_id, request.prompt)
    }
    fn submit(
        &self,
        state: &mut State,
        session_id: String,
        prompt: String,
    ) -> Result<Submission, AgentError> {
        state.workers.retain(|worker| !worker.is_finished());
        let task_id = id();
        let session = state.sessions.get(&session_id).unwrap();
        let record = TaskRecord {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            prompt,
            state: TaskState::Queued,
            thread_id: session.record.thread_id.clone(),
            turn_id: None,
            final_answer: None,
            error: None,
            events: vec![],
            cancel_requested: false,
            created_at: now(),
            finished_at: None,
        };
        // 任务行有 Session 外键，且 Session 占位必须同时落库，否则重启后
        // 可能出现「有 Task 没有占位」或反之的不一致。任一步失败都不改内存。
        let mut updated_session = session.record.clone();
        updated_session.active_task_id = Some(task_id.clone());
        updated_session.task_ids.push(task_id.clone());
        self.inner.store.submit_task(&record, &updated_session)?;
        let session = state.sessions.get_mut(&session_id).unwrap();
        session.record.active_task_id = Some(task_id.clone());
        session.record.task_ids.push(task_id.clone());
        state.tasks.insert(task_id.clone(), record);
        let weak = Arc::downgrade(&self.inner);
        let worker_id = task_id.clone();
        state.workers.push(tokio::spawn(async move {
            worker(weak, worker_id).await;
        }));
        Ok(Submission {
            task_id,
            session_id,
        })
    }
    pub fn status(&self, task_id: &str) -> Result<TaskRecord, AgentError> {
        let state = self.inner.state.lock().unwrap();
        if let Some(error) = state.persistence_errors.get(task_id) {
            return Err(error.clone());
        }
        state
            .tasks
            .get(task_id)
            .cloned()
            .ok_or_else(|| error(ErrorCode::NotFound, "task not found"))
    }
    /// 当前未终态的任务 ID（服务生命周期决策用，不对外定义新状态）。
    pub fn running_task_ids(&self) -> Result<Vec<String>, AgentError> {
        let mut ids: Vec<String> = self
            .inner
            .state
            .lock()
            .unwrap()
            .tasks
            .values()
            .filter(|t| !t.state.is_terminal())
            .map(|t| t.task_id.clone())
            .collect();
        ids.sort();
        Ok(ids)
    }
    pub fn session(&self, session_id: &str) -> Result<SessionRecord, AgentError> {
        self.inner
            .state
            .lock()
            .unwrap()
            .sessions
            .get(session_id)
            .map(|s| s.record.clone())
            .ok_or_else(|| error(ErrorCode::NotFound, "session not found"))
    }
    /// Sending an interrupt only sets cancel_requested. The adapter's terminal
    /// event remains authoritative, including completion racing with cancellation.
    pub async fn cancel(&self, task_id: &str) -> Result<TaskRecord, AgentError> {
        let interrupt = {
            let mut state = self.inner.state.lock().unwrap();
            let task = state
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| error(ErrorCode::NotFound, "task not found"))?;
            if task.state.is_terminal() {
                return Ok(task.clone());
            }
            let mut task = task.clone();
            task.cancel_requested = true;
            // 只有耐久写入成功才应用取消意图。
            self.inner.store.update_task(&task)?;
            state.tasks.insert(task_id.to_owned(), task);
            let not_started = !state.initializing.contains(task_id);
            let task = &state.tasks[task_id];
            if task.state == TaskState::Queued && not_started {
                // The worker checks terminal state before creating a process or turn.
                let session_id = task.session_id.clone();
                let store = self.inner.store.clone();
                finish(
                    store.as_ref(),
                    &mut state,
                    task_id,
                    TaskState::Cancelled,
                    None,
                    None,
                )?;
                debug_assert!(state.sessions[&session_id].record.active_task_id.is_none());
                None
            } else {
                let task = &state.tasks[task_id];
                match (
                    &task.thread_id,
                    &task.turn_id,
                    &state.sessions[&task.session_id].adapter,
                ) {
                    (Some(thread), Some(turn), Some(adapter)) => {
                        Some((adapter.clone(), thread.clone(), turn.clone()))
                    }
                    _ => None,
                }
            }
        };
        if let Some((adapter, thread, turn)) = interrupt {
            if let Err(e) = adapter.interrupt(&thread, &turn).await {
                // Preserve a terminal event observed during the failed RPC.
                if !self.status(task_id)?.state.is_terminal() {
                    return Err(e);
                }
            }
        }
        self.status(task_id)
    }
    pub async fn shutdown(&self) -> Result<(), AgentError> {
        let (adapters, workers, storage_errors) = {
            let mut state = self.inner.state.lock().unwrap();
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            let tasks: Vec<_> = state
                .tasks
                .values()
                .filter(|t| !t.state.is_terminal())
                .map(|t| t.task_id.clone())
                .collect();
            let mut storage_errors = Vec::new();
            for task in tasks {
                if let Err(e) = finish(
                    self.inner.store.as_ref(),
                    &mut state,
                    &task,
                    TaskState::Interrupted,
                    None,
                    Some(error(ErrorCode::Closed, "service shut down")),
                ) {
                    storage_errors.push(e);
                }
            }
            let adapters: Vec<_> = state
                .sessions
                .values_mut()
                .filter_map(|s| s.adapter.take())
                .collect();
            (adapters, std::mem::take(&mut state.workers), storage_errors)
        };
        for worker in &workers {
            worker.abort();
        }
        for worker in workers {
            let _ = worker.await;
        }
        let mut first_error: Option<AgentError> = storage_errors.into_iter().next();
        for adapter in adapters {
            let result = tokio::time::timeout(Duration::from_secs(5), adapter.shutdown())
                .await
                .unwrap_or_else(|_| Err(error(ErrorCode::Adapter, "adapter shutdown timed out")));
            if let Err(e) = result {
                first_error.get_or_insert(e);
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
/// 终态写入。持久化是完成的一部分：只有结果真正落库才允许对外报告 `completed`。
fn finish(
    store: &dyn TaskStore,
    state: &mut State,
    task_id: &str,
    status: TaskState,
    answer: Option<String>,
    err: Option<AgentError>,
) -> Result<(), AgentError> {
    let mut record = state.tasks[task_id].clone();
    if record.state.is_terminal() {
        return Ok(());
    }
    record.state = status;
    record.final_answer = answer;
    record.error = err;
    record.finished_at = Some(now());
    let session_id = record.session_id.clone();
    let mut session = state.sessions[&session_id].record.clone();
    if session.active_task_id.as_deref() == Some(task_id) {
        session.active_task_id = None;
    }
    let persisted = match store.finish_task(&record, &session) {
        Ok(()) => Ok(()),
        Err(e) if record.state == TaskState::Completed => {
            // 禁止「结果没落库但对外成功」：降级为 failed 并重试一次。
            record.state = TaskState::Failed;
            record.final_answer = None;
            record.error = Some(AgentError::new(
                ErrorCode::Storage,
                format!("final answer could not be persisted: {}", e.message),
            ));
            store.finish_task(&record, &session).map_err(|retry| {
                AgentError::new(
                    ErrorCode::Storage,
                    format!(
                        "terminal state could not be persisted: {}; retry: {}",
                        e.message, retry.message
                    ),
                )
            })
        }
        Err(e) => Err(e),
    };
    if let Err(e) = persisted {
        // 保留数据库已确认的快照和占位，status 明确报 storage，阻止续会越过未耐久的终态。
        state
            .persistence_errors
            .insert(task_id.to_owned(), e.clone());
        return Err(e);
    }
    state.tasks.insert(task_id.to_owned(), record);
    state.sessions.get_mut(&session_id).unwrap().record = session;
    state.initializing.remove(task_id);
    state.persistence_errors.remove(task_id);
    Ok(())
}

fn with_state<T>(weak: &Weak<Inner>, f: impl FnOnce(&Inner, &mut State) -> T) -> Option<T> {
    let inner = weak.upgrade()?;
    let mut state = inner.state.lock().unwrap();
    Some(f(&inner, &mut state))
}
async fn worker(weak: Weak<Inner>, task_id: String) {
    let Some(Some((factory, existing, cwd, thread, prompt))) = with_state(&weak, |inner, state| {
        let task = &state.tasks[&task_id];
        if state.closed || task.state.is_terminal() {
            return None;
        }
        state.initializing.insert(task_id.clone());
        let task = &state.tasks[&task_id];
        let session = &state.sessions[&task.session_id];
        Some((
            inner.factories[&session.record.agent].clone(),
            session.adapter.clone(),
            session.record.cwd.clone(),
            session.record.thread_id.clone(),
            task.prompt.clone(),
        ))
    }) else {
        return;
    };
    let result = execute(&weak, &task_id, factory, existing, cwd, thread, prompt).await;
    if let Err(mut err) = result {
        let adapter = with_state(&weak, |_, state| {
            let session_id = state.tasks[&task_id].session_id.clone();
            state.sessions.get_mut(&session_id).unwrap().adapter.take()
        })
        .flatten();
        if let Some(adapter) = adapter {
            let cleanup = tokio::time::timeout(Duration::from_secs(5), adapter.shutdown())
                .await
                .unwrap_or_else(|_| Err(error(ErrorCode::Adapter, "adapter cleanup timed out")));
            if let Err(cleanup_error) = cleanup {
                with_state(&weak, |_, state| {
                    let session_id = state.tasks[&task_id].session_id.clone();
                    state.sessions.get_mut(&session_id).unwrap().cleanup_error =
                        Some(cleanup_error.clone());
                });
                err.message
                    .push_str(&format!("; cleanup failed: {}", cleanup_error));
            }
        }
        with_state(&weak, |inner, state| {
            let status = if err.code == ErrorCode::ProcessExited {
                TaskState::Interrupted
            } else {
                TaskState::Failed
            };
            if let Err(e) = finish(
                inner.store.as_ref(),
                state,
                &task_id,
                status,
                None,
                Some(err),
            ) {
                tracing::error!(task = %task_id, error = %e, "agent task terminal state could not be persisted");
            }
        });
    }
    // 防止 worker 异常返回时留下永久 running 的僵死任务。
    with_state(&weak, |inner, state| {
        if state
            .tasks
            .get(&task_id)
            .is_some_and(|t| !t.state.is_terminal())
        {
            if let Err(e) = finish(
                inner.store.as_ref(),
                state,
                &task_id,
                TaskState::Interrupted,
                None,
                Some(error(
                    ErrorCode::ProcessExited,
                    "task worker ended without a terminal state",
                )),
            ) {
                tracing::error!(task = %task_id, error = %e, "zombie task could not be persisted");
            }
        }
    });
}
async fn execute(
    weak: &Weak<Inner>,
    task_id: &str,
    factory: Arc<dyn AdapterFactory>,
    existing: Option<Arc<dyn AgentAdapter>>,
    cwd: std::path::PathBuf,
    thread: Option<String>,
    prompt: String,
) -> Result<(), AgentError> {
    let adapter = match existing {
        Some(adapter) => adapter,
        None => tokio::time::timeout(Duration::from_secs(30), factory.connect())
            .await
            .map_err(|_| error(ErrorCode::Adapter, "adapter connection timed out"))??,
    };
    let proceed = with_state(weak, |_, state| {
        let task = &state.tasks[task_id];
        if state.closed || task.state.is_terminal() {
            return false;
        }
        state
            .sessions
            .get_mut(&task.session_id.clone())
            .unwrap()
            .adapter = Some(adapter.clone());
        true
    })
    .unwrap_or(false);
    if !proceed {
        adapter.shutdown().await?;
        return Ok(());
    }
    let thread = tokio::time::timeout(Duration::from_secs(30), async {
        if let Some(thread) = thread {
            adapter.resume_thread(&thread, &cwd).await?;
            Ok(thread)
        } else {
            adapter.start_thread(&cwd).await
        }
    })
    .await
    .map_err(|_| error(ErrorCode::Adapter, "thread initialization timed out"))??;
    let proceed = with_state(weak, |inner, state| -> Result<bool, AgentError> {
        let task = &state.tasks[task_id];
        if state.closed || task.state.is_terminal() {
            return Ok(false);
        }
        let session_id = task.session_id.clone();
        let mut session = state.sessions[&session_id].record.clone();
        session.thread_id = Some(thread.clone());
        // Task 的持久化 Thread 来自 Session 关系，先提交唯一映射再刷新缓存。
        inner.store.upsert_session(&session)?;
        state.sessions.get_mut(&session_id).unwrap().record = session;
        state.tasks.get_mut(task_id).unwrap().thread_id = Some(thread.clone());
        if state.tasks[task_id].cancel_requested {
            finish(
                inner.store.as_ref(),
                state,
                task_id,
                TaskState::Cancelled,
                None,
                None,
            )?;
            return Ok(false);
        }
        Ok(true)
    })
    .unwrap_or(Ok(false))?;
    if !proceed {
        return Ok(());
    }
    let turn = tokio::time::timeout(
        Duration::from_secs(30),
        adapter.start_turn(&thread, &prompt),
    )
    .await
    .map_err(|_| error(ErrorCode::Adapter, "turn start timed out"))??;
    let cancelled = with_state(weak, |inner, state| -> Result<bool, AgentError> {
        let mut task = state.tasks[task_id].clone();
        if state.closed || task.state.is_terminal() {
            return Ok(true);
        }
        task.turn_id = Some(turn.clone());
        task.state = TaskState::Running;
        let cancel_requested = task.cancel_requested;
        inner.store.update_task(&task)?;
        state.tasks.insert(task_id.to_owned(), task);
        Ok(cancel_requested)
    })
    .unwrap_or(Ok(true))?;
    if cancelled {
        adapter.interrupt(&thread, &turn).await?;
    }
    loop {
        match adapter.next_event().await? {
            AdapterEvent::Progress {
                thread_id,
                turn_id,
                kind,
                message,
            } => {
                if thread_id != thread || turn_id != turn {
                    continue;
                }
                with_state(weak, |inner, state| {
                    let task = state.tasks.get_mut(task_id).unwrap();
                    if !task.state.is_terminal() {
                        task.events.push(TaskEvent { kind, message });
                        // 过程事件定期刷盘（每次写全量 JSON，不必逐条写）。
                        if task.events.len() % 25 == 0 {
                            if let Err(e) = inner.store.update_task(&state.tasks[task_id].clone()) {
                                tracing::error!(task = %task_id, error = %e, "agent task events could not be persisted");
                            }
                        }
                    }
                });
            }
            AdapterEvent::ProcessExited { message } => {
                return Err(AgentError::new(ErrorCode::ProcessExited, message))
            }
            AdapterEvent::TurnFinished {
                thread_id,
                turn_id,
                outcome,
                final_answer,
                error: adapter_error,
            } => {
                if thread_id != thread || turn_id != turn {
                    continue;
                }
                let (status, err) = match outcome {
                    TurnOutcome::Completed
                        if adapter_error.is_none()
                            && final_answer
                                .as_ref()
                                .is_some_and(|answer| !answer.trim().is_empty()) =>
                    {
                        (TaskState::Completed, None)
                    }
                    TurnOutcome::Completed => (
                        TaskState::Failed,
                        Some(adapter_error.unwrap_or_else(|| {
                            error(ErrorCode::Protocol, "successful turn has no final answer")
                        })),
                    ),
                    TurnOutcome::Failed => (
                        TaskState::Failed,
                        Some(
                            adapter_error
                                .unwrap_or_else(|| error(ErrorCode::Adapter, "turn failed")),
                        ),
                    ),
                    TurnOutcome::Cancelled => (TaskState::Cancelled, adapter_error),
                    TurnOutcome::Interrupted => {
                        let requested =
                            with_state(weak, |_, state| state.tasks[task_id].cancel_requested)
                                .unwrap_or(false);
                        (
                            if requested {
                                TaskState::Cancelled
                            } else {
                                TaskState::Interrupted
                            },
                            adapter_error,
                        )
                    }
                    TurnOutcome::Unknown => (
                        TaskState::Failed,
                        Some(error(ErrorCode::Protocol, "unknown turn outcome")),
                    ),
                };
                if status == TaskState::Cancelled {
                    // Provider interruption can leave detached commands alive. Close
                    // the owned execution scope before publishing cancellation. The
                    // durable Thread mapping survives; continuation reconnects it.
                    let cleanup = tokio::time::timeout(Duration::from_secs(5), adapter.shutdown())
                        .await
                        .unwrap_or_else(|_| {
                            Err(error(ErrorCode::Adapter, "cancel scope cleanup timed out"))
                        });
                    if let Err(error) = cleanup {
                        with_state(weak, |_, state| {
                            let session_id = state.tasks[task_id].session_id.clone();
                            state.sessions.get_mut(&session_id).unwrap().cleanup_error =
                                Some(error.clone());
                        });
                        return Err(error);
                    }
                    with_state(weak, |_, state| {
                        let session_id = state.tasks[task_id].session_id.clone();
                        let session = state.sessions.get_mut(&session_id).unwrap();
                        if session
                            .adapter
                            .as_ref()
                            .is_some_and(|cached| Arc::ptr_eq(cached, &adapter))
                        {
                            session.adapter = None;
                        }
                    });
                }
                with_state(weak, |inner, state| {
                    finish(
                        inner.store.as_ref(),
                        state,
                        task_id,
                        status,
                        final_answer,
                        err,
                    )
                })
                .transpose()?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tokio::sync::{mpsc, Mutex as AsyncMutex};
    struct Fake {
        rx: AsyncMutex<mpsc::UnboundedReceiver<AdapterEvent>>,
        tx: mpsc::UnboundedSender<AdapterEvent>,
        calls: Mutex<Vec<String>>,
        shutdowns: std::sync::atomic::AtomicUsize,
        fail_interrupt: std::sync::atomic::AtomicBool,
        fail_shutdown: std::sync::atomic::AtomicBool,
        fail_shutdown_once: std::sync::atomic::AtomicBool,
        next_error: Mutex<Option<AgentError>>,
        shutdown_wait: Mutex<Option<Arc<tokio::sync::Notify>>>,
    }
    impl Fake {
        fn new() -> Arc<Self> {
            let (tx, rx) = mpsc::unbounded_channel();
            Arc::new(Self {
                rx: AsyncMutex::new(rx),
                tx,
                calls: Mutex::new(vec![]),
                shutdowns: 0.into(),
                fail_interrupt: false.into(),
                fail_shutdown: false.into(),
                fail_shutdown_once: false.into(),
                next_error: Mutex::new(None),
                shutdown_wait: Mutex::new(None),
            })
        }
        fn finish(&self, outcome: TurnOutcome, answer: Option<&str>) {
            self.tx
                .send(AdapterEvent::TurnFinished {
                    thread_id: "thread".into(),
                    turn_id: "turn".into(),
                    outcome,
                    final_answer: answer.map(str::to_owned),
                    error: None,
                })
                .unwrap();
        }
    }
    impl AgentAdapter for Fake {
        fn start_thread<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, Result<String, AgentError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push("thread".into());
                Ok("thread".into())
            })
        }
        fn resume_thread<'a>(
            &'a self,
            _: &'a str,
            _: &'a Path,
        ) -> BoxFuture<'a, Result<(), AgentError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push("resume".into());
                Ok(())
            })
        }
        fn start_turn<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> BoxFuture<'a, Result<String, AgentError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push("turn".into());
                Ok("turn".into())
            })
        }
        fn next_event(&self) -> BoxFuture<'_, Result<AdapterEvent, AgentError>> {
            Box::pin(async move {
                if let Some(err) = self.next_error.lock().unwrap().take() {
                    return Err(err);
                }
                self.rx
                    .lock()
                    .await
                    .recv()
                    .await
                    .ok_or_else(|| AgentError::new(ErrorCode::ProcessExited, "closed"))
            })
        }
        fn interrupt<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> BoxFuture<'a, Result<(), AgentError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push("interrupt".into());
                if self
                    .fail_interrupt
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    Err(error(ErrorCode::Adapter, "interrupt failed"))
                } else {
                    Ok(())
                }
            })
        }
        fn shutdown(&self) -> BoxFuture<'_, Result<(), AgentError>> {
            Box::pin(async move {
                self.shutdowns
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let gate = self.shutdown_wait.lock().unwrap().clone();
                if let Some(gate) = gate {
                    gate.notified().await;
                }
                if self.fail_shutdown.load(std::sync::atomic::Ordering::SeqCst)
                    || self
                        .fail_shutdown_once
                        .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    Err(error(ErrorCode::Adapter, "cleanup failed"))
                } else {
                    Ok(())
                }
            })
        }
    }
    struct Factory {
        adapters: Mutex<Vec<Arc<Fake>>>,
    }
    impl AdapterFactory for Factory {
        fn descriptor(&self) -> AgentDescriptor {
            AgentDescriptor {
                id: "test".into(),
                name: "test".into(),
                permissions: vec![PermissionMode::ReadOnly],
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
    fn setup() -> (AgentService, Arc<Factory>) {
        let factory = Arc::new(Factory {
            adapters: Mutex::new(vec![]),
        });
        (AgentService::new(vec![factory.clone()]).unwrap(), factory)
    }
    fn request() -> RunRequest {
        RunRequest {
            agent: "test".into(),
            cwd: std::env::current_dir().unwrap(),
            prompt: "audit".into(),
            permission: PermissionMode::ReadOnly,
        }
    }
    async fn running(service: &AgentService, id: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if service.status(id).unwrap().state == TaskState::Running {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn terminal(service: &AgentService, id: &str) -> TaskRecord {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let r = service.status(id).unwrap();
                if r.state.is_terminal() {
                    break r;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn creates_distinct_records_retains_full_answer_and_continues_thread() {
        let (s, f) = setup();
        assert_eq!(s.list().len(), 1);
        let a = s.run(request()).unwrap();
        assert_ne!(a.task_id, a.session_id);
        running(&s, &a.task_id).await;
        let full = "evidence ".repeat(20000);
        f.adapters.lock().unwrap()[0].finish(TurnOutcome::Completed, Some(&full));
        let task = terminal(&s, &a.task_id).await;
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(task.final_answer.as_deref(), Some(full.as_str()));
        let b = s
            .continue_session(ContinueRequest {
                session_id: a.session_id.clone(),
                prompt: "continue".into(),
            })
            .unwrap();
        running(&s, &b.task_id).await;
        assert_eq!(b.session_id, a.session_id);
        assert_ne!(b.task_id, a.task_id);
        assert_eq!(f.adapters.lock().unwrap().len(), 1);
        assert!(f.adapters.lock().unwrap()[0]
            .calls
            .lock()
            .unwrap()
            .contains(&"resume".into()));
        assert_eq!(
            s.session(&a.session_id).unwrap().thread_id.as_deref(),
            Some("thread")
        );
        s.shutdown().await.unwrap();
        assert_eq!(s.status(&b.task_id).unwrap().state, TaskState::Interrupted);
    }
    #[tokio::test]
    async fn rejects_execute_empty_prompt_unknown_agent_and_unknown_records() {
        let (s, _) = setup();
        let mut r = request();
        r.permission = PermissionMode::Execute;
        assert_eq!(s.run(r).unwrap_err().code, ErrorCode::UnsupportedPermission);
        let mut r = request();
        r.prompt = " ".into();
        assert_eq!(s.run(r).unwrap_err().code, ErrorCode::InvalidRequest);
        let mut r = request();
        r.agent = "unknown".into();
        assert_eq!(s.run(r).unwrap_err().code, ErrorCode::UnknownAgent);
        assert_eq!(s.status("none").unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(
            s.cancel("none").await.unwrap_err().code,
            ErrorCode::NotFound
        );
    }
    #[tokio::test]
    async fn rejects_same_session_overlap_but_other_sessions_run_concurrently() {
        let (s, f) = setup();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        assert_eq!(
            s.continue_session(ContinueRequest {
                session_id: a.session_id,
                prompt: "overlap".into()
            })
            .unwrap_err()
            .code,
            ErrorCode::Busy
        );
        let b = s.run(request()).unwrap();
        running(&s, &b.task_id).await;
        assert_eq!(f.adapters.lock().unwrap().len(), 2);
        s.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn cancellation_waits_for_scope_cleanup_then_resumes_the_same_thread_on_a_new_connection()
    {
        use std::sync::atomic::Ordering;
        let (s, factory) = setup();
        let submitted = s.run(request()).unwrap();
        running(&s, &submitted.task_id).await;
        let fake = factory.adapters.lock().unwrap()[0].clone();
        let gate = Arc::new(tokio::sync::Notify::new());
        *fake.shutdown_wait.lock().unwrap() = Some(gate.clone());
        s.cancel(&submitted.task_id).await.unwrap();
        fake.finish(TurnOutcome::Interrupted, None);
        tokio::time::timeout(Duration::from_secs(1), async {
            while fake.shutdowns.load(Ordering::SeqCst) == 0
                && !s.status(&submitted.task_id).unwrap().state.is_terminal()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !s.status(&submitted.task_id).unwrap().state.is_terminal(),
            "cancelled must wait for actual scope cleanup"
        );
        assert_eq!(
            s.session(&submitted.session_id)
                .unwrap()
                .active_task_id
                .as_deref(),
            Some(submitted.task_id.as_str())
        );
        assert_eq!(fake.shutdowns.load(Ordering::SeqCst), 1);
        *fake.shutdown_wait.lock().unwrap() = None;
        gate.notify_one();
        assert_eq!(
            terminal(&s, &submitted.task_id).await.state,
            TaskState::Cancelled
        );
        let continued = s
            .continue_session(ContinueRequest {
                session_id: submitted.session_id.clone(),
                prompt: "continue after cancellation".into(),
            })
            .unwrap();
        running(&s, &continued.task_id).await;
        let second = factory.adapters.lock().unwrap()[1].clone();
        assert_eq!(second.calls.lock().unwrap()[0], "resume");
        assert_eq!(
            s.status(&continued.task_id).unwrap().thread_id,
            Some("thread".into())
        );
        second.finish(TurnOutcome::Completed, Some("resumed"));
        assert_eq!(
            terminal(&s, &continued.task_id).await.state,
            TaskState::Completed
        );
        s.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_cleanup_failure_does_not_claim_cancelled_or_allow_continuation() {
        let (s, factory) = setup();
        let submitted = s.run(request()).unwrap();
        running(&s, &submitted.task_id).await;
        let fake = factory.adapters.lock().unwrap()[0].clone();
        fake.fail_shutdown_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
        s.cancel(&submitted.task_id).await.unwrap();
        fake.finish(TurnOutcome::Interrupted, None);
        let task = terminal(&s, &submitted.task_id).await;
        assert_eq!(task.state, TaskState::Failed);
        assert!(task.error.unwrap().message.contains("cleanup"));
        assert!(s
            .continue_session(ContinueRequest {
                session_id: submitted.session_id,
                prompt: "unsafe continuation".into()
            })
            .is_err());
    }

    #[tokio::test]
    async fn cancellation_acknowledgement_is_not_terminal_and_completion_wins_race() {
        let (s, f) = setup();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        assert_eq!(
            s.cancel(&a.task_id).await.unwrap().state,
            TaskState::Running
        );
        f.adapters.lock().unwrap()[0].finish(TurnOutcome::Completed, Some("complete"));
        assert_eq!(terminal(&s, &a.task_id).await.state, TaskState::Completed);
        assert_eq!(
            s.cancel(&a.task_id).await.unwrap().state,
            TaskState::Completed
        );
        s.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn queued_cancel_does_not_start_process() {
        let (s, f) = setup();
        let a = s.run(request()).unwrap();
        assert_eq!(
            s.cancel(&a.task_id).await.unwrap().state,
            TaskState::Cancelled
        );
        tokio::task::yield_now().await;
        assert!(f.adapters.lock().unwrap().is_empty());
        s.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn success_requires_answer_and_unknown_or_process_exit_are_not_success() {
        for (outcome, answer, expected) in [
            (TurnOutcome::Completed, None, TaskState::Failed),
            (TurnOutcome::Completed, Some(" "), TaskState::Failed),
            (TurnOutcome::Unknown, Some("text"), TaskState::Failed),
            (TurnOutcome::Failed, None, TaskState::Failed),
            (TurnOutcome::Cancelled, None, TaskState::Cancelled),
            (TurnOutcome::Interrupted, None, TaskState::Interrupted),
        ] {
            let (s, f) = setup();
            let a = s.run(request()).unwrap();
            running(&s, &a.task_id).await;
            f.adapters.lock().unwrap()[0].finish(outcome, answer);
            assert_eq!(terminal(&s, &a.task_id).await.state, expected);
            s.shutdown().await.unwrap();
        }
        let (s, f) = setup();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        f.adapters.lock().unwrap()[0]
            .tx
            .send(AdapterEvent::ProcessExited {
                message: "crash".into(),
            })
            .unwrap();
        let task = terminal(&s, &a.task_id).await;
        assert_eq!(task.state, TaskState::Interrupted);
        assert_eq!(task.error.unwrap().code, ErrorCode::ProcessExited);
        s.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn shutdown_closes_connections_and_rejects_new_work() {
        let (s, f) = setup();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        s.shutdown().await.unwrap();
        assert_eq!(s.status(&a.task_id).unwrap().state, TaskState::Interrupted);
        assert_eq!(
            f.adapters.lock().unwrap()[0]
                .shutdowns
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(s.run(request()).unwrap_err().code, ErrorCode::Closed);
    }
    struct SlowFactory {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        fake: Arc<Fake>,
    }
    impl AdapterFactory for SlowFactory {
        fn descriptor(&self) -> AgentDescriptor {
            Factory {
                adapters: Mutex::new(vec![]),
            }
            .descriptor()
        }
        fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>> {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(self.fake.clone() as Arc<dyn AgentAdapter>)
            })
        }
    }
    #[tokio::test]
    async fn cancellation_during_initialization_keeps_session_busy_until_cleanup() {
        let f = Arc::new(SlowFactory {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            fake: Fake::new(),
        });
        let s = AgentService::new(vec![f.clone()]).unwrap();
        let a = s.run(request()).unwrap();
        f.entered.notified().await;
        let cancelled = s.cancel(&a.task_id).await.unwrap();
        assert!(
            !cancelled.state.is_terminal(),
            "initializing worker still owns session"
        );
        assert_eq!(
            s.continue_session(ContinueRequest {
                session_id: a.session_id,
                prompt: "next".into()
            })
            .unwrap_err()
            .code,
            ErrorCode::Busy
        );
        f.release.notify_one();
        assert_eq!(terminal(&s, &a.task_id).await.state, TaskState::Cancelled);
        assert!(!f.fake.calls.lock().unwrap().contains(&"turn".into()));
        s.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn process_exit_cleans_connection_and_continuation_reconnects_original_thread() {
        let (s, f) = setup();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        f.adapters.lock().unwrap()[0]
            .tx
            .send(AdapterEvent::ProcessExited {
                message: "crash".into(),
            })
            .unwrap();
        assert_eq!(terminal(&s, &a.task_id).await.state, TaskState::Interrupted);
        assert_eq!(
            f.adapters.lock().unwrap()[0]
                .shutdowns
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let b = s
            .continue_session(ContinueRequest {
                session_id: a.session_id,
                prompt: "resume".into(),
            })
            .unwrap();
        running(&s, &b.task_id).await;
        assert_eq!(f.adapters.lock().unwrap().len(), 2);
        assert!(f.adapters.lock().unwrap()[1]
            .calls
            .lock()
            .unwrap()
            .contains(&"resume".into()));
        s.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn failed_interrupt_can_be_retried_without_claiming_cancellation() {
        let (s, f) = setup();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        f.adapters.lock().unwrap()[0]
            .fail_interrupt
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            s.cancel(&a.task_id).await.unwrap_err().code,
            ErrorCode::Adapter
        );
        assert_eq!(
            s.cancel(&a.task_id).await.unwrap().state,
            TaskState::Running
        );
        assert_eq!(
            f.adapters.lock().unwrap()[0]
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.as_str() == "interrupt")
                .count(),
            2
        );
        f.adapters.lock().unwrap()[0].finish(TurnOutcome::Interrupted, None);
        assert_eq!(terminal(&s, &a.task_id).await.state, TaskState::Cancelled);
        s.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn transport_failure_cleans_before_releasing_session_and_failed_cleanup_blocks_resume() {
        for cleanup_fails in [false, true] {
            let fake = Fake::new();
            *fake.next_error.lock().unwrap() = Some(error(ErrorCode::Adapter, "broken reader"));
            fake.fail_shutdown
                .store(cleanup_fails, std::sync::atomic::Ordering::SeqCst);
            let f = Arc::new(SlowFactory {
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
                fake: fake.clone(),
            });
            let s = AgentService::new(vec![f.clone()]).unwrap();
            let a = s.run(request()).unwrap();
            f.entered.notified().await;
            f.release.notify_one();
            let task = terminal(&s, &a.task_id).await;
            assert_eq!(task.state, TaskState::Failed);
            assert!(task.error.unwrap().message.contains("broken reader"));
            assert_eq!(fake.shutdowns.load(std::sync::atomic::Ordering::SeqCst), 1);
            if cleanup_fails {
                assert_eq!(
                    s.continue_session(ContinueRequest {
                        session_id: a.session_id,
                        prompt: "resume".into()
                    })
                    .unwrap_err()
                    .message,
                    "cleanup failed"
                );
            }
            s.shutdown().await.unwrap();
        }
    }
    #[tokio::test]
    async fn ignores_other_turn_events_and_retains_progress_without_truncation() {
        let (s, f) = setup();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        let fake = f.adapters.lock().unwrap()[0].clone();
        fake.tx
            .send(AdapterEvent::TurnFinished {
                thread_id: "other".into(),
                turn_id: "turn".into(),
                outcome: TurnOutcome::Completed,
                final_answer: Some("wrong".into()),
                error: None,
            })
            .unwrap();
        let message = "source evidence ".repeat(10000);
        fake.tx
            .send(AdapterEvent::Progress {
                thread_id: "thread".into(),
                turn_id: "turn".into(),
                kind: "analysis".into(),
                message: message.clone(),
            })
            .unwrap();
        fake.finish(TurnOutcome::Completed, Some("right"));
        let task = terminal(&s, &a.task_id).await;
        assert_eq!(task.final_answer.as_deref(), Some("right"));
        assert_eq!(task.events[0].message, message);
        s.shutdown().await.unwrap();
    }
    struct EphemeralFactory {
        last: Mutex<std::sync::Weak<Fake>>,
    }
    impl AdapterFactory for EphemeralFactory {
        fn descriptor(&self) -> AgentDescriptor {
            Factory {
                adapters: Mutex::new(vec![]),
            }
            .descriptor()
        }
        fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>> {
            Box::pin(async move {
                let fake = Fake::new();
                *self.last.lock().unwrap() = Arc::downgrade(&fake);
                Ok(fake as Arc<dyn AgentAdapter>)
            })
        }
    }
    #[tokio::test]
    async fn dropping_service_aborts_worker_and_releases_its_adapter() {
        let f = Arc::new(EphemeralFactory {
            last: Mutex::new(std::sync::Weak::new()),
        });
        let s = AgentService::new(vec![f.clone()]).unwrap();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        let weak = f.last.lock().unwrap().clone();
        assert!(weak.upgrade().is_some());
        drop(s);
        tokio::time::timeout(Duration::from_secs(3), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    struct ErrorFactory;
    impl AdapterFactory for ErrorFactory {
        fn descriptor(&self) -> AgentDescriptor {
            Factory {
                adapters: Mutex::new(vec![]),
            }
            .descriptor()
        }
        fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>> {
            Box::pin(async { Err(error(ErrorCode::Adapter, "not authenticated")) })
        }
    }
    #[tokio::test]
    async fn connection_failure_is_visible_and_does_not_leave_busy_session() {
        let s = AgentService::new(vec![Arc::new(ErrorFactory)]).unwrap();
        let a = s.run(request()).unwrap();
        let task = terminal(&s, &a.task_id).await;
        assert_eq!(task.state, TaskState::Failed);
        assert_eq!(task.error.unwrap().message, "not authenticated");
        assert!(s.session(&a.session_id).unwrap().active_task_id.is_none());
        let b = s
            .continue_session(ContinueRequest {
                session_id: a.session_id,
                prompt: "retry".into(),
            })
            .unwrap();
        assert_eq!(terminal(&s, &b.task_id).await.state, TaskState::Failed);
        s.shutdown().await.unwrap();
    }

    // ---- 第二阶段：持久化 / 重启恢复 / 对账 ----

    /// 等待 worker 建立 Adapter 连接；不得在持有锁时 await，否则单线程 runtime 会死锁。
    async fn wait_for_adapter(factory: &Arc<Factory>) -> Arc<Fake> {
        loop {
            if let Some(adapter) = factory.adapters.lock().unwrap().first().cloned() {
                return adapter;
            }
            tokio::task::yield_now().await;
        }
    }

    fn sqlite_store(dir: &std::path::Path) -> Arc<dyn TaskStore> {
        let pool = crate::storage::db::init_database(&dir.to_path_buf()).unwrap();
        super::super::store::SqliteStore::new(pool)
    }

    fn persisted_session(id: &str) -> SessionRecord {
        SessionRecord {
            session_id: id.into(),
            agent: "test".into(),
            cwd: std::env::current_dir().unwrap(),
            permission: PermissionMode::ReadOnly,
            thread_id: Some("thread".into()),
            active_task_id: None,
            task_ids: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn persisted_task(id: &str, session_id: &str, state: TaskState) -> TaskRecord {
        TaskRecord {
            task_id: id.into(),
            session_id: session_id.into(),
            prompt: "audit".into(),
            state,
            thread_id: Some("thread".into()),
            turn_id: Some("turn".into()),
            final_answer: None,
            error: None,
            events: vec![],
            cancel_requested: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            finished_at: None,
        }
    }

    #[tokio::test]
    async fn completed_result_is_readable_after_a_new_service_instance() {
        let dir = tempfile::tempdir().unwrap();
        let answer = "full audit answer ".repeat(5000);
        let task_id;
        {
            let factory = Arc::new(Factory {
                adapters: Mutex::new(vec![]),
            });
            let s =
                AgentService::with_store(vec![factory.clone()], sqlite_store(dir.path())).unwrap();
            let a = s.run(request()).unwrap();
            task_id = a.task_id.clone();
            let adapter = wait_for_adapter(&factory).await;
            // 先发出 terminal 事件再确认状态，确保写入路径真的走过。
            adapter.finish(TurnOutcome::Completed, Some(&answer));
            let task = terminal(&s, &task_id).await;
            assert_eq!(task.state, TaskState::Completed);
            s.shutdown().await.unwrap();
        }
        // 新进程/新连接池：只能从库里读到结果。
        let factory = Arc::new(Factory {
            adapters: Mutex::new(vec![]),
        });
        let reopened = AgentService::with_store(vec![factory], sqlite_store(dir.path())).unwrap();
        let task = reopened.status(&task_id).unwrap();
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(task.final_answer.as_deref(), Some(answer.as_str()));
        assert!(!task.thread_id.unwrap().is_empty());
        assert!(reopened.status(&task_id).unwrap().finished_at.is_some());
    }

    #[tokio::test]
    async fn restart_never_keeps_an_unfinished_task_running() {
        let dir = tempfile::tempdir().unwrap();
        let store = sqlite_store(dir.path());
        let mut session = persisted_session("s1");
        session.active_task_id = Some("t1".into());
        store.upsert_session(&session).unwrap();
        store
            .insert_task(&persisted_task("t1", "s1", TaskState::Running))
            .unwrap();

        let factory = Arc::new(Factory {
            adapters: Mutex::new(vec![]),
        });
        let s = AgentService::with_store(vec![factory], store.clone()).unwrap();
        let task = s.status("t1").unwrap();
        assert_eq!(task.state, TaskState::Interrupted);
        let error = task.error.unwrap();
        assert_eq!(error.code, ErrorCode::ProcessExited);
        assert!(error.message.contains("restarted"));
        assert!(s.session("s1").unwrap().active_task_id.is_none());
        // 降级必须落库，不能只改内存。
        assert_eq!(
            store.task("t1").unwrap().unwrap().state,
            TaskState::Interrupted
        );
        let again = AgentService::with_store(
            vec![Arc::new(Factory {
                adapters: Mutex::new(vec![]),
            })],
            store,
        )
        .unwrap();
        assert_eq!(again.status("t1").unwrap().state, TaskState::Interrupted);
    }

    #[tokio::test]
    async fn a_second_restart_can_still_reconcile_the_original_interrupted_turn() {
        let store = MemoryStore::new();
        store.upsert_session(&persisted_session("s1")).unwrap();
        store
            .insert_task(&persisted_task("t1", "s1", TaskState::Running))
            .unwrap();
        let factory = || {
            Arc::new(ReconcileFactory(Reconcile::Completed("confirmed"))) as Arc<dyn AdapterFactory>
        };
        drop(AgentService::with_store(vec![factory()], store.clone()).unwrap());
        let restarted_again = AgentService::with_store(vec![factory()], store).unwrap();
        assert_eq!(restarted_again.reconcile_interrupted().await.unwrap(), 1);
        assert_eq!(
            restarted_again.status("t1").unwrap().state,
            TaskState::Completed
        );
    }

    enum Reconcile {
        Completed(&'static str),
        Failed,
        Unavailable,
    }

    struct ReconcileFactory(Reconcile);
    impl AdapterFactory for ReconcileFactory {
        fn descriptor(&self) -> AgentDescriptor {
            AgentDescriptor {
                id: "test".into(),
                name: "test".into(),
                permissions: vec![PermissionMode::ReadOnly],
                supports_continuation: true,
                supports_cancel: true,
            }
        }
        fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>> {
            Box::pin(async { Err(error(ErrorCode::Adapter, "not used")) })
        }
        fn reconcile<'a>(
            &'a self,
            _session: &'a SessionRecord,
            _task: &'a TaskRecord,
        ) -> BoxFuture<'a, Result<Option<RecoveredTurn>, AgentError>> {
            Box::pin(async move {
                Ok(match self.0 {
                    Reconcile::Completed(answer) => Some(RecoveredTurn {
                        outcome: TurnOutcome::Completed,
                        final_answer: Some(answer.into()),
                        error: None,
                    }),
                    Reconcile::Failed => Some(RecoveredTurn {
                        outcome: TurnOutcome::Failed,
                        final_answer: None,
                        error: Some(error(ErrorCode::Adapter, "provider says failed")),
                    }),
                    Reconcile::Unavailable => None,
                })
            })
        }
    }

    #[tokio::test]
    async fn reconcile_only_completes_a_provider_confirmed_turn() {
        let dir = tempfile::tempdir().unwrap();
        let store = sqlite_store(dir.path());
        let session = persisted_session("s1");
        store.upsert_session(&session).unwrap();
        for (id, state) in [("t1", TaskState::Running), ("t2", TaskState::Queued)] {
            store.insert_task(&persisted_task(id, "s1", state)).unwrap();
        }
        let s = AgentService::with_store(
            vec![Arc::new(ReconcileFactory(Reconcile::Completed(
                "confirmed answer",
            )))],
            store.clone(),
        )
        .unwrap();
        // 恢复时先降级，避免僵死 running。
        assert_eq!(s.status("t1").unwrap().state, TaskState::Interrupted);
        assert_eq!(s.reconcile_interrupted().await.unwrap(), 2);
        let t1 = s.status("t1").unwrap();
        assert_eq!(t1.state, TaskState::Completed);
        assert_eq!(t1.final_answer.as_deref(), Some("confirmed answer"));
        assert_eq!(
            store.task("t1").unwrap().unwrap().state,
            TaskState::Completed
        );

        // provider 无法查证时保持 interrupted，不得伪装完成。
        let dir2 = tempfile::tempdir().unwrap();
        let store2 = sqlite_store(dir2.path());
        store2.upsert_session(&persisted_session("s2")).unwrap();
        store2
            .insert_task(&persisted_task("t3", "s2", TaskState::Running))
            .unwrap();
        let s2 = AgentService::with_store(
            vec![Arc::new(ReconcileFactory(Reconcile::Unavailable))],
            store2,
        )
        .unwrap();
        assert_eq!(s2.reconcile_interrupted().await.unwrap(), 0);
        assert_eq!(s2.status("t3").unwrap().state, TaskState::Interrupted);

        // provider 确认失败：按真实终局记为 failed，并保留错误。
        let dir3 = tempfile::tempdir().unwrap();
        let store3 = sqlite_store(dir3.path());
        store3.upsert_session(&persisted_session("s3")).unwrap();
        store3
            .insert_task(&persisted_task("t4", "s3", TaskState::Running))
            .unwrap();
        let s3 =
            AgentService::with_store(vec![Arc::new(ReconcileFactory(Reconcile::Failed))], store3)
                .unwrap();
        assert_eq!(s3.reconcile_interrupted().await.unwrap(), 1);
        let t4 = s3.status("t4").unwrap();
        assert_eq!(t4.state, TaskState::Failed);
        assert_eq!(t4.error.unwrap().message, "provider says failed");
    }

    #[tokio::test]
    async fn rejected_task_insert_does_not_leave_a_persisted_session_busy() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::storage::db::init_database(&dir.path().to_path_buf()).unwrap();
        pool.get().unwrap().execute_batch(
            "CREATE TRIGGER reject_task BEFORE INSERT ON agent_tasks BEGIN SELECT RAISE(ABORT, 'disk full'); END;"
        ).unwrap();
        let store = super::super::store::SqliteStore::new(pool);
        let s = AgentService::with_store(
            vec![Arc::new(Factory {
                adapters: Mutex::new(vec![]),
            })],
            store.clone(),
        )
        .unwrap();
        assert_eq!(s.run(request()).unwrap_err().code, ErrorCode::Storage);
        assert!(store
            .load()
            .unwrap()
            .0
            .iter()
            .all(|s| s.active_task_id.is_none()));
    }

    #[tokio::test]
    async fn failed_cancel_write_does_not_apply_cancellation_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::storage::db::init_database(&dir.path().to_path_buf()).unwrap();
        let factory = Arc::new(Factory {
            adapters: Mutex::new(vec![]),
        });
        let store = super::super::store::SqliteStore::new(pool.clone());
        let s = AgentService::with_store(vec![factory], store).unwrap();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        pool.get().unwrap().execute_batch(
            "CREATE TRIGGER reject_cancel BEFORE UPDATE ON agent_tasks WHEN NEW.cancel_requested=1 BEGIN SELECT RAISE(ABORT, 'disk full'); END;"
        ).unwrap();
        assert_eq!(
            s.cancel(&a.task_id).await.unwrap_err().code,
            ErrorCode::Storage
        );
        assert!(!s.status(&a.task_id).unwrap().cancel_requested);
        s.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn continuing_a_persisted_unregistered_agent_returns_unknown_agent() {
        let store = MemoryStore::new();
        store.upsert_session(&persisted_session("old")).unwrap();
        let s = AgentService::with_store(vec![], store).unwrap();
        assert_eq!(
            s.continue_session(ContinueRequest {
                session_id: "old".into(),
                prompt: "next".into()
            })
            .unwrap_err()
            .code,
            ErrorCode::UnknownAgent
        );
    }

    #[tokio::test]
    async fn reconciliation_write_failure_does_not_report_completed() {
        let store = Arc::new(FailingStore {
            inner: MemoryStore::default(),
        });
        store.upsert_session(&persisted_session("s1")).unwrap();
        store
            .insert_task(&persisted_task("t1", "s1", TaskState::Running))
            .unwrap();
        let s = AgentService::with_store(
            vec![Arc::new(ReconcileFactory(Reconcile::Completed("answer")))],
            store.clone(),
        )
        .unwrap();
        assert_eq!(
            s.reconcile_interrupted().await.unwrap_err().code,
            ErrorCode::Storage
        );
        assert_eq!(s.status("t1").unwrap().state, TaskState::Interrupted);
        assert_eq!(
            store.task("t1").unwrap().unwrap().state,
            TaskState::Interrupted
        );
    }

    #[tokio::test]
    async fn failed_thread_mapping_write_keeps_the_persisted_mapping_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::storage::db::init_database(&dir.path().to_path_buf()).unwrap();
        pool.get().unwrap().execute_batch(
            "CREATE TRIGGER reject_thread BEFORE UPDATE ON agent_sessions WHEN NEW.provider_session_id IS NOT NULL BEGIN SELECT RAISE(ABORT, 'disk full'); END;"
        ).unwrap();
        let s = AgentService::with_store(
            vec![Arc::new(Factory {
                adapters: Mutex::new(vec![]),
            })],
            super::super::store::SqliteStore::new(pool),
        )
        .unwrap();
        let a = s.run(request()).unwrap();
        let task = terminal(&s, &a.task_id).await;
        assert_eq!(task.state, TaskState::Failed);
        assert_eq!(task.error.unwrap().code, ErrorCode::Storage);
        assert!(task.thread_id.is_none());
        assert!(s.session(&a.session_id).unwrap().thread_id.is_none());
        s.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn failed_running_write_does_not_publish_an_undurable_running_state() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::storage::db::init_database(&dir.path().to_path_buf()).unwrap();
        pool.get().unwrap().execute_batch(
            "CREATE TRIGGER reject_running BEFORE UPDATE ON agent_tasks WHEN NEW.status='running' BEGIN SELECT RAISE(ABORT, 'disk full'); END;"
        ).unwrap();
        let fake = Fake::new();
        let gate = Arc::new(tokio::sync::Notify::new());
        *fake.shutdown_wait.lock().unwrap() = Some(gate.clone());
        let factory = Arc::new(SlowFactory {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            fake: fake.clone(),
        });
        let s = AgentService::with_store(
            vec![factory.clone()],
            super::super::store::SqliteStore::new(pool),
        )
        .unwrap();
        let a = s.run(request()).unwrap();
        factory.entered.notified().await;
        factory.release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while fake.shutdowns.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(s.status(&a.task_id).unwrap().state, TaskState::Queued);
        gate.notify_one();
        assert_eq!(terminal(&s, &a.task_id).await.state, TaskState::Failed);
        s.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn terminal_storage_failure_is_visible_and_does_not_release_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::storage::db::init_database(&dir.path().to_path_buf()).unwrap();
        let factory = Arc::new(Factory {
            adapters: Mutex::new(vec![]),
        });
        let store = super::super::store::SqliteStore::new(pool.clone());
        let s = AgentService::with_store(vec![factory.clone()], store.clone()).unwrap();
        let a = s.run(request()).unwrap();
        running(&s, &a.task_id).await;
        pool.get().unwrap().execute_batch(
            "CREATE TRIGGER reject_terminal BEFORE UPDATE ON agent_tasks WHEN NEW.status IN ('completed', 'failed', 'interrupted', 'cancelled') BEGIN SELECT RAISE(ABORT, 'disk full'); END;"
        ).unwrap();
        factory.adapters.lock().unwrap()[0].finish(TurnOutcome::Completed, Some("answer"));
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match s.status(&a.task_id) {
                    Err(error) => break error,
                    Ok(task) if task.state.is_terminal() => {
                        panic!("undurable terminal state reported: {:?}", task.state)
                    }
                    _ => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(result.code, ErrorCode::Storage);
        assert_eq!(
            store.task(&a.task_id).unwrap().unwrap().state,
            TaskState::Running
        );
        assert_eq!(
            s.session(&a.session_id).unwrap().active_task_id.as_deref(),
            Some(a.task_id.as_str())
        );
        assert!(s.shutdown().await.is_err());
    }

    struct FailingStore {
        inner: MemoryStore,
    }
    impl TaskStore for FailingStore {
        fn upsert_session(&self, s: &SessionRecord) -> Result<(), AgentError> {
            self.inner.upsert_session(s)
        }
        fn insert_task(&self, t: &TaskRecord) -> Result<(), AgentError> {
            self.inner.insert_task(t)
        }
        fn submit_task(&self, t: &TaskRecord, s: &SessionRecord) -> Result<(), AgentError> {
            self.inner.submit_task(t, s)
        }
        fn update_task(&self, t: &TaskRecord) -> Result<(), AgentError> {
            self.inner.update_task(t)
        }
        fn finish_task(&self, t: &TaskRecord, s: &SessionRecord) -> Result<(), AgentError> {
            if t.state == TaskState::Completed {
                return Err(error(ErrorCode::Storage, "disk full"));
            }
            self.inner.finish_task(t, s)
        }
        fn task(&self, id: &str) -> Result<Option<TaskRecord>, AgentError> {
            self.inner.task(id)
        }
        fn session(&self, id: &str) -> Result<Option<SessionRecord>, AgentError> {
            self.inner.session(id)
        }
        fn load(&self) -> Result<(Vec<SessionRecord>, Vec<TaskRecord>), AgentError> {
            self.inner.load()
        }
    }

    #[tokio::test]
    async fn a_result_that_cannot_be_persisted_is_never_reported_completed() {
        let factory = Arc::new(Factory {
            adapters: Mutex::new(vec![]),
        });
        let store: Arc<dyn TaskStore> = Arc::new(FailingStore {
            inner: MemoryStore::default(),
        });
        let s = AgentService::with_store(vec![factory.clone()], store.clone()).unwrap();
        let a = s.run(request()).unwrap();
        let adapter = wait_for_adapter(&factory).await;
        adapter.finish(TurnOutcome::Completed, Some("valuable answer"));
        let task = terminal(&s, &a.task_id).await;
        assert_eq!(task.state, TaskState::Failed);
        assert!(task.final_answer.is_none());
        assert_eq!(task.error.as_ref().unwrap().code, ErrorCode::Storage);
        assert_eq!(
            store.task(&a.task_id).unwrap().unwrap().state,
            TaskState::Failed
        );
    }
}
