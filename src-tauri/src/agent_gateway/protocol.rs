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
    /// 持久化失败：写入不可靠时不允许把任务报告为成功（第二阶段新增）。
    Storage,
    /// 本地 IPC 未通过认证（第二阶段新增）。
    Unauthorized,
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
/// 服务重启后对账时，由 Adapter 通过官方协议查到的真实终局。
/// `None` 表示无法查证（例如 provider 进程随服务一起退出且没有耐久查询入口）。
pub struct RecoveredTurn {
    pub outcome: TurnOutcome,
    pub final_answer: Option<String>,
    pub error: Option<AgentError>,
}

pub trait AdapterFactory: Send + Sync {
    fn descriptor(&self) -> AgentDescriptor;
    fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>>;
    /// 只读对账：查证一个未被服务观察到终局的 Turn 的真实状态。
    /// 默认实现返回 `None`（无法查证），调用方必须退化为 `interrupted`。
    /// 实现不得借对账重新执行任务。
    #[allow(unused_variables)]
    fn reconcile<'a>(
        &'a self,
        session: &'a SessionRecord,
        task: &'a TaskRecord,
    ) -> BoxFuture<'a, Result<Option<RecoveredTurn>, AgentError>> {
        Box::pin(async { Ok(None) })
    }
}

/// `agent_status` 的分页查询参数。单位是 Unicode 标量值（chars），
/// 分页只做切片，调用方按 offset 顺序拼接即可精确还原原文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusQuery {
    pub task_id: String,
    #[serde(default)]
    pub final_answer_offset: usize,
    /// 未提供时返回 `final_answer` 的全部剩余内容（不静默截断）。
    #[serde(default)]
    pub final_answer_limit: Option<usize>,
    #[serde(default)]
    pub events_offset: usize,
    /// 未提供时不返回过程事件，只报告 `events_total` 与 `events_next_offset`。
    #[serde(default)]
    pub events_limit: Option<usize>,
}

/// 完整 `TaskRecord` + 明确的分页元数据。`final_answer` / `events` 的裁剪只发生在
/// 调用方显式给出 limit 时，并且总是同时报告总长度与下一页位置。
pub fn project_status(task: &TaskRecord, query: &StatusQuery) -> serde_json::Value {
    let mut value = serde_json::to_value(task).expect("TaskRecord is serializable");
    let answer = task.final_answer.clone().unwrap_or_default();
    let total = answer.chars().count();
    let offset = query.final_answer_offset.min(total);
    let limit = query.final_answer_limit.unwrap_or(total - offset);
    let slice: String = answer.chars().skip(offset).take(limit).collect();
    let end = offset + slice.chars().count();
    let next = (end < total).then_some(end);
    value["final_answer"] = if task.final_answer.is_some() {
        serde_json::Value::String(slice)
    } else {
        serde_json::Value::Null
    };
    value["final_answer_offset"] = offset.into();
    value["final_answer_total_chars"] = total.into();
    value["final_answer_next_offset"] = match next {
        Some(next) => next.into(),
        None => serde_json::Value::Null,
    };
    value["final_answer_complete"] = next.is_none().into();

    let events_total = task.events.len();
    let events_offset = query.events_offset.min(events_total);
    let events: &[TaskEvent] = match query.events_limit {
        Some(limit) => {
            let end = events_offset.saturating_add(limit).min(events_total);
            &task.events[events_offset..end]
        }
        None => &[],
    };
    let events_end = events_offset + events.len();
    value["events"] = serde_json::to_value(events).expect("events are serializable");
    value["events_offset"] = events_offset.into();
    value["events_total"] = events_total.into();
    value["events_included"] = query.events_limit.is_some().into();
    value["events_next_offset"] = match (query.events_limit.is_some(), events_end < events_total) {
        // 调用方没要事件，但仍有事件可读：从 0 开始就是下一页。
        (false, true) => 0usize.into(),
        (true, true) => events_end.into(),
        _ => serde_json::Value::Null,
    };
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(answer: Option<&str>, events: usize) -> TaskRecord {
        TaskRecord {
            task_id: "t1".into(),
            session_id: "s1".into(),
            prompt: "audit".into(),
            state: TaskState::Completed,
            thread_id: Some("thread".into()),
            turn_id: Some("turn".into()),
            final_answer: answer.map(str::to_owned),
            error: None,
            events: (0..events)
                .map(|i| TaskEvent {
                    kind: "assistant".into(),
                    message: format!("event {i}"),
                })
                .collect(),
            cancel_requested: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            finished_at: Some("2026-01-01T00:01:00Z".into()),
        }
    }

    fn query(task_id: &str) -> StatusQuery {
        StatusQuery {
            task_id: task_id.into(),
            final_answer_offset: 0,
            final_answer_limit: None,
            events_offset: 0,
            events_limit: None,
        }
    }

    #[test]
    fn default_status_returns_the_whole_answer_without_truncation() {
        let answer = "答案 ".repeat(3000) + "end";
        let projected = project_status(&task(Some(&answer), 4), &query("t1"));
        assert_eq!(projected["final_answer"], serde_json::json!(answer));
        assert_eq!(
            projected["final_answer_total_chars"],
            answer.chars().count()
        );
        assert_eq!(projected["final_answer_complete"], serde_json::json!(true));
        assert_eq!(
            projected["final_answer_next_offset"],
            serde_json::Value::Null
        );
        // 没有显式要事件时不返回过程事件，但必须报告总数与下一页位置。
        assert_eq!(projected["events"], serde_json::json!([]));
        assert_eq!(projected["events_total"], 4);
        assert_eq!(projected["events_included"], false);
        assert_eq!(projected["events_next_offset"], 0);
    }

    #[test]
    fn paginated_reads_reassemble_the_exact_original_text() {
        let answer: String = "审计结论：权限边界 abc\n".repeat(37) + "中文尾";
        let expected: Vec<char> = answer.chars().collect();
        let mut rebuilt = String::new();
        let mut offset = 0;
        loop {
            let mut q = query("t1");
            q.final_answer_offset = offset;
            q.final_answer_limit = Some(13);
            let projected = project_status(&task(Some(&answer), 0), &q);
            rebuilt.push_str(projected["final_answer"].as_str().unwrap());
            assert_eq!(projected["final_answer_offset"], offset);
            assert_eq!(projected["final_answer_total_chars"], expected.len());
            match projected["final_answer_next_offset"].as_u64() {
                Some(next) => {
                    assert_eq!(next as usize, offset + 13);
                    assert_eq!(projected["final_answer_complete"], false);
                    offset = next as usize;
                }
                None => {
                    assert_eq!(projected["final_answer_complete"], true);
                    break;
                }
            }
        }
        assert_eq!(rebuilt, answer);

        // 越界 offset 得到空串且标记完成，不 panic、不返回旧内容。
        let mut q = query("t1");
        q.final_answer_offset = expected.len() + 100;
        let projected = project_status(&task(Some(&answer), 0), &q);
        assert_eq!(projected["final_answer"], "");
        assert_eq!(projected["final_answer_complete"], true);
    }

    #[test]
    fn oversized_event_limit_does_not_overflow() {
        let mut q = query("t1");
        q.events_offset = 1;
        q.events_limit = Some(usize::MAX);
        assert_eq!(
            project_status(&task(Some("done"), 2), &q)["events"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn event_pages_are_explicit_about_what_is_omitted() {
        let mut q = query("t1");
        q.events_limit = Some(2);
        let projected = project_status(&task(Some("done"), 5), &q);
        assert_eq!(projected["events"].as_array().unwrap().len(), 2);
        assert_eq!(projected["events_total"], 5);
        assert_eq!(projected["events_included"], true);
        assert_eq!(projected["events_next_offset"], 2);

        q.events_offset = 4;
        let projected = project_status(&task(Some("done"), 5), &q);
        assert_eq!(projected["events"].as_array().unwrap().len(), 1);
        assert_eq!(projected["events_next_offset"], serde_json::Value::Null);
    }

    #[test]
    fn a_task_without_an_answer_reports_null_and_zero() {
        let projected = project_status(&task(None, 0), &query("t1"));
        assert_eq!(projected["final_answer"], serde_json::Value::Null);
        assert_eq!(projected["final_answer_total_chars"], 0);
        assert_eq!(projected["final_answer_complete"], true);
    }
}
