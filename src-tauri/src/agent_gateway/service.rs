//! In-memory orchestration only. TaskRecord/SessionRecord are storage DTOs;
//! adapters, workers, and cancellation coordination must never be persisted.
use super::protocol::*;
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
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: bool,
    initializing: HashSet<String>,
    sessions: HashMap<String, Session>,
    tasks: HashMap<String, TaskRecord>,
    workers: Vec<JoinHandle<()>>,
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
impl AgentService {
    /// Must be used inside a Tokio runtime. Register only factories whose executable
    /// discovery succeeded; authentication and source validation happen on connect.
    pub fn new(factories: Vec<Arc<dyn AdapterFactory>>) -> Result<Self, AgentError> {
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
                state: Mutex::new(State::default()),
            }),
        })
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
        let mut state = self.inner.state.lock().unwrap();
        if state.closed {
            return Err(error(ErrorCode::Closed, "service is shut down"));
        }
        state.sessions.insert(
            session_id.clone(),
            Session {
                record: SessionRecord {
                    session_id: session_id.clone(),
                    agent: request.agent,
                    cwd,
                    permission: request.permission,
                    thread_id: None,
                    active_task_id: None,
                    task_ids: vec![],
                    created_at: now(),
                },
                adapter: None,
                cleanup_error: None,
            },
        );
        Ok(self.submit(&mut state, session_id, request.prompt))
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
        let descriptor = self.inner.factories[&session.record.agent].descriptor();
        if !descriptor.supports_continuation {
            return Err(error(
                ErrorCode::InvalidRequest,
                "agent does not support continuation",
            ));
        }
        Ok(self.submit(&mut state, request.session_id, request.prompt))
    }
    fn submit(&self, state: &mut State, session_id: String, prompt: String) -> Submission {
        state.workers.retain(|worker| !worker.is_finished());
        let task_id = id();
        let session = state.sessions.get_mut(&session_id).unwrap();
        session.record.active_task_id = Some(task_id.clone());
        session.record.task_ids.push(task_id.clone());
        state.tasks.insert(
            task_id.clone(),
            TaskRecord {
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
            },
        );
        let weak = Arc::downgrade(&self.inner);
        let worker_id = task_id.clone();
        state.workers.push(tokio::spawn(async move {
            worker(weak, worker_id).await;
        }));
        Submission {
            task_id,
            session_id,
        }
    }
    pub fn status(&self, task_id: &str) -> Result<TaskRecord, AgentError> {
        self.inner
            .state
            .lock()
            .unwrap()
            .tasks
            .get(task_id)
            .cloned()
            .ok_or_else(|| error(ErrorCode::NotFound, "task not found"))
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
            task.cancel_requested = true;
            let not_started = !state.initializing.contains(task_id);
            let task = &state.tasks[task_id];
            if task.state == TaskState::Queued && not_started {
                // The worker checks terminal state before creating a process or turn.
                let session_id = task.session_id.clone();
                finish(&mut state, task_id, TaskState::Cancelled, None, None);
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
        let (adapters, workers) = {
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
            for task in tasks {
                finish(
                    &mut state,
                    &task,
                    TaskState::Interrupted,
                    None,
                    Some(error(ErrorCode::Closed, "service shut down")),
                );
            }
            let adapters: Vec<_> = state
                .sessions
                .values_mut()
                .filter_map(|s| s.adapter.take())
                .collect();
            (adapters, std::mem::take(&mut state.workers))
        };
        for worker in &workers {
            worker.abort();
        }
        for worker in workers {
            let _ = worker.await;
        }
        let mut first_error = None;
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
fn finish(
    state: &mut State,
    task_id: &str,
    status: TaskState,
    answer: Option<String>,
    err: Option<AgentError>,
) {
    let task = state.tasks.get_mut(task_id).unwrap();
    if task.state.is_terminal() {
        return;
    }
    task.state = status;
    task.final_answer = answer;
    task.error = err;
    task.finished_at = Some(now());
    state.initializing.remove(task_id);
    let session = state.sessions.get_mut(&task.session_id).unwrap();
    if session.record.active_task_id.as_deref() == Some(task_id) {
        session.record.active_task_id = None;
    }
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
        with_state(&weak, |_, state| {
            let status = if err.code == ErrorCode::ProcessExited {
                TaskState::Interrupted
            } else {
                TaskState::Failed
            };
            finish(state, &task_id, status, None, Some(err));
        });
    }
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
    let proceed = with_state(weak, |_, state| {
        let task = state.tasks.get_mut(task_id).unwrap();
        if state.closed || task.state.is_terminal() {
            return false;
        }
        task.thread_id = Some(thread.clone());
        state
            .sessions
            .get_mut(&task.session_id)
            .unwrap()
            .record
            .thread_id = Some(thread.clone());
        if task.cancel_requested {
            finish(state, task_id, TaskState::Cancelled, None, None);
            return false;
        }
        true
    })
    .unwrap_or(false);
    if !proceed {
        return Ok(());
    }
    let turn = tokio::time::timeout(
        Duration::from_secs(30),
        adapter.start_turn(&thread, &prompt),
    )
    .await
    .map_err(|_| error(ErrorCode::Adapter, "turn start timed out"))??;
    let cancelled = with_state(weak, |_, state| {
        let task = state.tasks.get_mut(task_id).unwrap();
        task.turn_id = Some(turn.clone());
        if !task.state.is_terminal() {
            task.state = TaskState::Running;
        }
        task.cancel_requested
    })
    .unwrap_or(true);
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
                with_state(weak, |_, state| {
                    let task = state.tasks.get_mut(task_id).unwrap();
                    if !task.state.is_terminal() {
                        task.events.push(TaskEvent { kind, message });
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
                with_state(weak, |_, state| {
                    finish(state, task_id, status, final_answer, err)
                });
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
        next_error: Mutex<Option<AgentError>>,
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
                next_error: Mutex::new(None),
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
                if self.fail_shutdown.load(std::sync::atomic::Ordering::SeqCst) {
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
}
