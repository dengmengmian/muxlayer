//! Serializable records form the future storage boundary; this module performs no persistence.
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedPermission,
    UnknownAgent,
    NotFound,
    Busy,
    Closed,
    Adapter,
    Protocol,
    ProcessExited,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentError {
    pub code: ErrorCode,
    pub message: String,
}
impl AgentError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for AgentError {}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    ReadOnly,
    Execute,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl TaskState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDescriptor {
    pub id: String,
    pub name: String,
    pub permissions: Vec<PermissionMode>,
    pub supports_continuation: bool,
    pub supports_cancel: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRequest {
    pub agent: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub permission: PermissionMode,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinueRequest {
    pub session_id: String,
    pub prompt: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    pub task_id: String,
    pub session_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub kind: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task_id: String,
    pub session_id: String,
    pub prompt: String,
    pub state: TaskState,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub final_answer: Option<String>,
    pub error: Option<AgentError>,
    pub events: Vec<TaskEvent>,
    pub cancel_requested: bool,
    pub created_at: String,
    pub finished_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub agent: String,
    pub cwd: PathBuf,
    pub permission: PermissionMode,
    pub thread_id: Option<String>,
    pub active_task_id: Option<String>,
    pub task_ids: Vec<String>,
    pub created_at: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Unknown,
}
#[derive(Debug, Clone)]
pub enum AdapterEvent {
    TurnFinished {
        thread_id: String,
        turn_id: String,
        outcome: TurnOutcome,
        final_answer: Option<String>,
        error: Option<AgentError>,
    },
    Progress {
        thread_id: String,
        turn_id: String,
        kind: String,
        message: String,
    },
    ProcessExited {
        message: String,
    },
}
/// Each connection belongs exclusively to one service session. Methods must allow
/// `next_event` and `interrupt` to execute concurrently. Drop must terminate child resources.
pub trait AgentAdapter: Send + Sync {
    fn start_thread<'a>(&'a self, cwd: &'a Path) -> BoxFuture<'a, Result<String, AgentError>>;
    fn resume_thread<'a>(
        &'a self,
        thread_id: &'a str,
        cwd: &'a Path,
    ) -> BoxFuture<'a, Result<(), AgentError>>;
    fn start_turn<'a>(
        &'a self,
        thread_id: &'a str,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<String, AgentError>>;
    fn next_event(&self) -> BoxFuture<'_, Result<AdapterEvent, AgentError>>;
    fn interrupt<'a>(
        &'a self,
        thread_id: &'a str,
        turn_id: &'a str,
    ) -> BoxFuture<'a, Result<(), AgentError>>;
    fn shutdown(&self) -> BoxFuture<'_, Result<(), AgentError>>;
}
pub trait AdapterFactory: Send + Sync {
    fn descriptor(&self) -> AgentDescriptor;
    fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>>;
}
