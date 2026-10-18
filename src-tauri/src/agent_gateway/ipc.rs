//! 本地 IPC：让 Agent Service 拥有独立于 MCP 客户端的生命周期。
//!
//! ```text
//! MCP stdio 进程 ──(本地 IPC)──> Agent Service 进程 ──> Codex / Claude Code
//! ```
//!
//! - macOS / Linux：Unix Domain Socket（目录 0700 + socket 0600）
//! - Windows：Named Pipe（`\\.\pipe\...`，`first_pipe_instance` 保证单实例）
//! - 认证：每次连接先 `hello` 握手，token 由服务启动时生成、写入 0600 token 文件，
//!   客户端读取同一个文件；不通过网络传输任何凭据以外的秘密。
//! - IPC 只是传输层：它调用 `AgentService` 的现有语义，不重新定义业务状态机。
use super::protocol::*;
use super::service::AgentService;
use super::store::{SqliteStore, TaskStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;

/// 单条 IPC 消息上限（含长回答）。超过则明确报错，不做静默截断。
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
/// 服务端单次请求的处理预算：所有方法都必须立刻返回（任务在后台执行）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SERVICE_NAME: &str = "muxlayer-agent-service";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// 响应二选一：`result` 或 `error`，与第一阶段契约第 8 节一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentError>,
}

impl IpcResponse {
    fn ok(id: String, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }
    fn failed(id: String, error: AgentError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// 服务端文件布局。客户端与服务端用同一份逻辑推导路径，避免不一致。
#[derive(Debug, Clone)]
pub struct ServicePaths {
    pub dir: PathBuf,
    pub socket: PathBuf,
    pub token: PathBuf,
    pub lock: PathBuf,
    pub log: PathBuf,
}

fn short_hash(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl ServicePaths {
    /// True when something is actually accepting connections on the service socket.
    /// A leftover socket file after shutdown must not be reported as a service error.
    #[cfg(feature = "desktop")]
    pub(crate) fn is_live(&self) -> bool {
        transport::probe(&self.socket)
    }

    pub fn new(db_dir: &Path) -> Self {
        let dir = db_dir.join("agent-service");
        let identity = dir.to_string_lossy().to_string();
        #[cfg(windows)]
        let socket = PathBuf::from(format!(
            r"\\.\pipe\muxlayer-agent-{}",
            short_hash(&identity)
        ));
        #[cfg(unix)]
        let socket = {
            // macOS/Linux 的 sun_path 有长度上限：路径过长时退回用户私有临时目录，
            // 用目录哈希保证不同数据目录不会互相连接。
            let candidate = dir.join("agent-service.sock");
            if candidate.as_os_str().len() > 100 {
                std::env::temp_dir()
                    .join(format!("muxlayer-agent-{}", short_hash(&identity)))
                    .join("service.sock")
            } else {
                candidate
            }
        };
        Self {
            token: dir.join("token"),
            lock: dir.join("service.lock"),
            log: dir.join("service.log"),
            socket,
            dir,
        }
    }

    pub fn read_token(&self) -> Result<String, AgentError> {
        let raw = std::fs::read_to_string(&self.token).map_err(|e| {
            AgentError::new(
                ErrorCode::Adapter,
                format!(
                    "agent service token not found ({}): {e}. Start it with `agentgate-serve agent-service`.",
                    self.token.display()
                ),
            )
        })?;
        let token = raw.trim().to_string();
        if token.is_empty() {
            return Err(AgentError::new(
                ErrorCode::Unauthorized,
                "agent service token file is empty",
            ));
        }
        Ok(token)
    }
}

fn token_matches(expected: &str, provided: &str) -> bool {
    let (a, b) = (expected.as_bytes(), provided.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------- transport

#[cfg(unix)]
mod transport {
    use super::*;
    use tokio::net::{UnixListener, UnixStream};

    pub type Stream = UnixStream;

    pub fn bind(path: &Path) -> Result<UnixListener, AgentError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AgentError::new(
                    ErrorCode::Adapter,
                    format!("cannot create socket dir {}: {e}", parent.display()),
                )
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                let metadata = std::fs::symlink_metadata(parent).map_err(|e| {
                    AgentError::new(
                        ErrorCode::Adapter,
                        format!("cannot inspect socket directory: {e}"),
                    )
                })?;
                if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
                    return Err(AgentError::new(ErrorCode::Adapter, "socket directory must be owned by the current user and must not be a symlink"));
                }
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(
                    |e| {
                        AgentError::new(
                            ErrorCode::Adapter,
                            format!("cannot restrict socket directory: {e}"),
                        )
                    },
                )?;
            }
        }
        // 只有确认没有活着的服务时才会走到这里；残留文件直接清理。
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| {
                AgentError::new(
                    ErrorCode::Adapter,
                    format!("cannot remove stale socket {}: {e}", path.display()),
                )
            })?;
        }
        let listener = UnixListener::bind(path).map_err(|e| {
            AgentError::new(
                ErrorCode::Adapter,
                format!("cannot bind socket {}: {e}", path.display()),
            )
        })?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
            AgentError::new(
                ErrorCode::Adapter,
                format!("cannot restrict socket permissions: {e}"),
            )
        })?;
        Ok(listener)
    }

    pub async fn accept(listener: &mut UnixListener) -> Result<Stream, AgentError> {
        listener
            .accept()
            .await
            .map(|(stream, _)| stream)
            .map_err(|e| AgentError::new(ErrorCode::Adapter, format!("accept failed: {e}")))
    }

    pub async fn connect(path: &Path) -> Result<Stream, AgentError> {
        tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(path))
            .await
            .map_err(|_| AgentError::new(ErrorCode::Adapter, "connect timed out"))?
            .map_err(|e| AgentError::new(ErrorCode::Adapter, format!("connect failed: {e}")))
    }

    pub fn probe(path: &Path) -> bool {
        std::os::unix::net::UnixStream::connect(path).is_ok()
    }
}

#[cfg(windows)]
mod transport {
    use super::*;
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    pub enum Stream {
        Client(NamedPipeClient),
        Server(NamedPipeServer),
    }

    /// Named Pipe 监听器：每接受一个连接后立刻创建下一个实例。
    pub struct Listener {
        name: String,
        pending: Option<NamedPipeServer>,
    }

    pub fn bind(path: &Path) -> Result<Listener, AgentError> {
        let name = pipe_name(path);
        let pending = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .map_err(|e| AgentError::new(ErrorCode::Adapter, format!("cannot create pipe: {e}")))?;
        Ok(Listener {
            name,
            pending: Some(pending),
        })
    }

    fn pipe_name(path: &Path) -> String {
        path.to_string_lossy().to_string()
    }

    pub async fn accept(listener: &mut Listener) -> Result<Stream, AgentError> {
        // pending 实例可能因为取消（服务关闭）被丢弃，这里按需重建，避免永久失败。
        if listener.pending.is_none() {
            listener.pending = Some(ServerOptions::new().create(&listener.name).map_err(|e| {
                AgentError::new(
                    ErrorCode::Adapter,
                    format!("cannot create pipe instance: {e}"),
                )
            })?);
        }
        let server = listener
            .pending
            .take()
            .expect("pending instance just created");
        server.connect().await.map_err(|e| {
            AgentError::new(ErrorCode::Adapter, format!("pipe connect failed: {e}"))
        })?;
        listener.pending = Some(ServerOptions::new().create(&listener.name).map_err(|e| {
            AgentError::new(
                ErrorCode::Adapter,
                format!("cannot create next pipe instance: {e}"),
            )
        })?);
        Ok(Stream::Server(server))
    }

    impl tokio::io::AsyncRead for Stream {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.get_mut() {
                Stream::Client(inner) => std::pin::Pin::new(inner).poll_read(cx, buf),
                Stream::Server(inner) => std::pin::Pin::new(inner).poll_read(cx, buf),
            }
        }
    }

    impl tokio::io::AsyncWrite for Stream {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            match self.get_mut() {
                Stream::Client(inner) => std::pin::Pin::new(inner).poll_write(cx, buf),
                Stream::Server(inner) => std::pin::Pin::new(inner).poll_write(cx, buf),
            }
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.get_mut() {
                Stream::Client(inner) => std::pin::Pin::new(inner).poll_flush(cx),
                Stream::Server(inner) => std::pin::Pin::new(inner).poll_flush(cx),
            }
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.get_mut() {
                Stream::Client(inner) => std::pin::Pin::new(inner).poll_shutdown(cx),
                Stream::Server(inner) => std::pin::Pin::new(inner).poll_shutdown(cx),
            }
        }
    }

    pub async fn connect(path: &Path) -> Result<Stream, AgentError> {
        let name = pipe_name(path);
        // Windows 的命名管道 open 是同步调用，放进 blocking 线程里保留超时语义。
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio::task::spawn_blocking(move || ClientOptions::new().open(&name)),
        )
        .await
        .map_err(|_| AgentError::new(ErrorCode::Adapter, "connect timed out"))?
        .map_err(|e| AgentError::new(ErrorCode::Adapter, format!("connect task failed: {e}")))?
        .map(Stream::Client)
        .map_err(|e| AgentError::new(ErrorCode::Adapter, format!("connect failed: {e}")))
    }

    pub fn probe(path: &Path) -> bool {
        ClientOptions::new().open(pipe_name(path)).is_ok()
    }
}

// ---------------------------------------------------------------- framing

async fn write_message<W>(writer: &mut W, message: &impl Serialize) -> Result<(), AgentError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let mut line = serde_json::to_string(message)
        .map_err(|e| AgentError::new(ErrorCode::Protocol, format!("encode failed: {e}")))?;
    line.push('\n');
    if line.len() > MAX_MESSAGE_BYTES {
        return Err(AgentError::new(
            ErrorCode::InvalidRequest,
            format!("message exceeds {MAX_MESSAGE_BYTES} bytes; use agent_status pagination"),
        ));
    }
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| AgentError::new(ErrorCode::Adapter, format!("write failed: {e}")))?;
    writer
        .flush()
        .await
        .map_err(|e| AgentError::new(ErrorCode::Adapter, format!("flush failed: {e}")))
}

async fn read_message<R, T>(reader: &mut R) -> Result<Option<T>, AgentError>
where
    R: tokio::io::AsyncBufRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut line = String::new();
    loop {
        line.clear();
        let read = (&mut *reader)
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_line(&mut line)
            .await
            .map_err(|e| AgentError::new(ErrorCode::Adapter, format!("read failed: {e}")))?;
        if read == 0 {
            return Ok(None);
        }
        if line.len() > MAX_MESSAGE_BYTES {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                format!("message exceeds {MAX_MESSAGE_BYTES} bytes"),
            ));
        }
        if line.trim().is_empty() {
            continue;
        }
        return serde_json::from_str(&line)
            .map(Some)
            .map_err(|e| AgentError::new(ErrorCode::InvalidRequest, format!("invalid JSON: {e}")));
    }
}

// ---------------------------------------------------------------- dispatch

async fn dispatch(
    service: &AgentService,
    authenticated: bool,
    expected_token: &str,
    request: &IpcRequest,
) -> Result<Value, AgentError> {
    if request.method == "hello" {
        let provided = request.params["token"].as_str().unwrap_or("");
        if !token_matches(expected_token, provided) {
            return Err(AgentError::new(
                ErrorCode::Unauthorized,
                "invalid service token",
            ));
        }
        return Ok(json!({
            "service": SERVICE_NAME,
            "version": SERVICE_VERSION,
            "agents": service.list(),
        }));
    }
    if !authenticated {
        return Err(AgentError::new(
            ErrorCode::Unauthorized,
            "authentication required: call `hello` with the service token first",
        ));
    }
    match request.method.as_str() {
        "agent_list" => Ok(json!({ "agents": service.list() })),
        "agent_run" => {
            let run: RunRequest = serde_json::from_value(request.params.clone()).map_err(|e| {
                AgentError::new(ErrorCode::InvalidRequest, format!("agent_run: {e}"))
            })?;
            Ok(serde_json::to_value(service.run(run)?).expect("Submission is serializable"))
        }
        "agent_continue" => {
            let request: ContinueRequest =
                serde_json::from_value(request.params.clone()).map_err(|e| {
                    AgentError::new(ErrorCode::InvalidRequest, format!("agent_continue: {e}"))
                })?;
            Ok(serde_json::to_value(service.continue_session(request)?)
                .expect("Submission is serializable"))
        }
        "agent_status" => {
            let query: StatusQuery =
                serde_json::from_value(request.params.clone()).map_err(|e| {
                    AgentError::new(ErrorCode::InvalidRequest, format!("agent_status: {e}"))
                })?;
            let task = service.status(&query.task_id)?;
            Ok(project_status(&task, &query))
        }
        "agent_session" => {
            let session_id = request.params["session_id"].as_str().ok_or_else(|| {
                AgentError::new(ErrorCode::InvalidRequest, "session_id is required")
            })?;
            Ok(serde_json::to_value(service.session(session_id)?)
                .expect("SessionRecord is serializable"))
        }
        "agent_cancel" => {
            let task_id = request.params["task_id"]
                .as_str()
                .ok_or_else(|| AgentError::new(ErrorCode::InvalidRequest, "task_id is required"))?;
            let task = service.cancel(task_id).await?;
            Ok(serde_json::to_value(task).expect("TaskRecord is serializable"))
        }
        "service_shutdown" => Ok(json!({ "stopping": true })),
        "service_status" => {
            let tasks = service.running_task_ids()?;
            Ok(json!({
                "service": SERVICE_NAME,
                "version": SERVICE_VERSION,
                "running_task_ids": tasks,
                "agents": service.list(),
            }))
        }
        other => Err(AgentError::new(
            ErrorCode::InvalidRequest,
            format!("unknown method {other}"),
        )),
    }
}

// ---------------------------------------------------------------- server

pub struct ServiceHostConfig {
    pub paths: ServicePaths,
    /// 0 = 永不因空闲退出（默认）。任务执行期间不计空闲。
    pub idle_timeout: Option<Duration>,
}

/// 启动 IPC 服务：先确认没有其它实例，再绑定 socket、写 token，然后循环服务请求。
pub async fn serve(
    service: Arc<AgentService>,
    config: ServiceHostConfig,
) -> Result<(), AgentError> {
    let ownership = ServiceOwnership::acquire(&config.paths)?;
    serve_owned(service, config, ownership).await
}

/// 持有 OS 文件锁才拥有恢复数据库、绑定端点和清理文件的权限。
/// 锁文件永久保留：删除会让等待者锁住旧 inode，造成两个 owner。
pub struct ServiceOwnership {
    _file: std::fs::File,
    dir: PathBuf,
}

impl ServiceOwnership {
    pub fn acquire(paths: &ServicePaths) -> Result<Self, AgentError> {
        std::fs::create_dir_all(&paths.dir).map_err(|e| {
            AgentError::new(
                ErrorCode::Adapter,
                format!("cannot create service directory: {e}"),
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = std::fs::symlink_metadata(&paths.dir).map_err(|e| {
                AgentError::new(
                    ErrorCode::Adapter,
                    format!("cannot inspect service directory: {e}"),
                )
            })?;
            if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
                return Err(AgentError::new(
                    ErrorCode::Adapter,
                    "service directory must not be a symlink",
                ));
            }
            std::fs::set_permissions(&paths.dir, std::fs::Permissions::from_mode(0o700)).map_err(
                |e| {
                    AgentError::new(
                        ErrorCode::Adapter,
                        format!("cannot restrict service directory: {e}"),
                    )
                },
            )?;
        }
        reject_symlink(&paths.lock)?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&paths.lock).map_err(|e| {
            AgentError::new(
                ErrorCode::Adapter,
                format!("cannot open service ownership lock: {e}"),
            )
        })?;
        file.try_lock().map_err(|e| {
            AgentError::new(
                ErrorCode::Busy,
                format!("another service owns the database: {e}"),
            )
        })?;
        if transport::probe(&paths.socket) {
            return Err(AgentError::new(
                ErrorCode::Busy,
                "another agent service is already listening",
            ));
        }
        Ok(Self {
            _file: file,
            dir: paths.dir.clone(),
        })
    }
}

fn reject_symlink(path: &Path) -> Result<(), AgentError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => Err(AgentError::new(
            ErrorCode::Adapter,
            format!("service file must be a regular file: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AgentError::new(
            ErrorCode::Adapter,
            format!("cannot inspect service file: {e}"),
        )),
    }
}

pub async fn serve_owned(
    service: Arc<AgentService>,
    config: ServiceHostConfig,
    ownership: ServiceOwnership,
) -> Result<(), AgentError> {
    let paths = config.paths.clone();
    if ownership.dir != paths.dir {
        return Err(AgentError::new(
            ErrorCode::InvalidRequest,
            "ownership does not match service directory",
        ));
    }
    let mut listener = transport::bind(&paths.socket)?;
    let token = new_token();
    write_token(&paths.token, &token)?;
    tracing::info!(socket = %paths.socket.display(), "agent service listening");

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let idle_timeout = config.idle_timeout.filter(|d| !d.is_zero());

    let result = loop {
        // 每个连接独立处理：MCP 客户端会长期持有一条连接，
        // 不能阻塞其它客户端（否则状态查询 / 关闭请求会排队超时）。
        let accept = transport::accept(&mut listener);
        tokio::pin!(accept);
        let accepted = tokio::select! {
            _ = shutdown_rx.changed() => break Ok(()),
            accepted = &mut accept => accepted,
            _ = async {
                match idle_timeout {
                    Some(idle) => tokio::time::sleep(idle).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                // 空闲退出只是可选项：有任务在跑就继续等待。
                if service.running_task_ids()?.is_empty() {
                    tracing::info!("agent service idle timeout reached; exiting");
                    break Ok(());
                }
                continue;
            }
        };
        let stream = match accepted {
            Ok(stream) => stream,
            Err(e) => {
                tracing::error!(error = %e, "agent service accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let (reader, writer) = tokio::io::split(stream);
        let service = service.clone();
        let token = token.clone();
        let shutdown = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                service,
                tokio::io::BufReader::new(reader),
                writer,
                token,
                shutdown,
            )
            .await
            {
                tracing::warn!(error = %e, "agent service connection ended with error");
            }
        });
    };

    // 关闭：先停止接收并中断任务，再清理 socket / token；保留 lock 文件并释放 OS 锁。
    let shutdown_result = service.shutdown().await;
    if let Err(e) = &shutdown_result {
        tracing::warn!(error = %e, "agent service shutdown reported an error");
    }
    #[cfg(unix)]
    let _ = std::fs::remove_file(&paths.socket);
    let _ = std::fs::remove_file(&paths.token);
    drop(listener);
    drop(ownership);
    result.and(shutdown_result)
}

async fn handle_connection<R, W>(
    service: Arc<AgentService>,
    mut reader: R,
    mut writer: W,
    token: String,
    shutdown: watch::Sender<bool>,
) -> Result<(), AgentError>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut authenticated = false;
    loop {
        let request: IpcRequest = match read_message(&mut reader).await? {
            Some(request) => request,
            // 客户端断开：任务属于服务端，继续执行，不受影响。
            None => return Ok(()),
        };
        let id = request.id.clone();
        let outcome = match tokio::time::timeout(
            REQUEST_TIMEOUT,
            dispatch(&service, authenticated, &token, &request),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(AgentError::new(
                ErrorCode::Adapter,
                "agent service did not answer in time",
            )),
        };
        if request.method == "hello" && outcome.is_ok() {
            authenticated = true;
        }
        let wants_shutdown = request.method == "service_shutdown" && outcome.is_ok();
        let response = match outcome {
            Ok(result) => IpcResponse::ok(id, result),
            Err(error) => IpcResponse::failed(id, error),
        };
        if let Err(error) = write_message(&mut writer, &response).await {
            if error.code != ErrorCode::InvalidRequest {
                return Err(error);
            }
            write_message(&mut writer, &IpcResponse::failed(response.id, error)).await?;
        }
        if wants_shutdown {
            let _ = shutdown.send(true);
            return Ok(());
        }
    }
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    // uuid v4 提供两个 128bit 随机数，拼成 256bit token；不引入新的随机数依赖。
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    bytes[..16].copy_from_slice(a.as_bytes());
    bytes[16..].copy_from_slice(b.as_bytes());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn write_token(path: &Path, token: &str) -> Result<(), AgentError> {
    reject_symlink(path)?;
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(format!("{token}\n").as_bytes()))
        .map_err(|e| {
            AgentError::new(ErrorCode::Adapter, format!("cannot write token file: {e}"))
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
            AgentError::new(
                ErrorCode::Adapter,
                format!("cannot restrict token file: {e}"),
            )
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------- client

pub struct IpcClient {
    writer: tokio::io::WriteHalf<transport::Stream>,
    reader: tokio::io::BufReader<tokio::io::ReadHalf<transport::Stream>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl IpcClient {
    /// 连接服务并完成 token 握手。
    pub async fn connect(paths: &ServicePaths) -> Result<Self, AgentError> {
        let token = paths.read_token()?;
        let stream = transport::connect(&paths.socket).await?;
        let (reader, writer) = tokio::io::split(stream);
        let mut client = Self {
            reader: tokio::io::BufReader::new(reader),
            writer,
            next_id: 1.into(),
        };
        match client
            .call("hello", json!({ "token": token, "client": "mcp" }))
            .await
        {
            Ok(Ok(_)) => Ok(client),
            // 认证失败是服务端明确回答，原样上报，不去尝试其它账号或路径。
            Ok(Err(error)) => Err(error),
            Err(error) => Err(AgentError::new(
                error.code,
                format!(
                    "agent service handshake failed ({}): {}",
                    paths.socket.display(),
                    error.message
                ),
            )),
        }
    }

    /// 外层错误表示传输/连接问题，内层错误表示服务端明确回答的业务错误。
    /// 调用方据此只在传输失败时重连，不会把业务错误当成断线重试。
    pub async fn call(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Result<Value, AgentError>, AgentError> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();
        let request = IpcRequest {
            id: id.clone(),
            method: method.to_owned(),
            params,
        };
        write_message(&mut self.writer, &request).await?;
        let response: IpcResponse =
            tokio::time::timeout(REQUEST_TIMEOUT, read_message(&mut self.reader))
                .await
                .map_err(|_| AgentError::new(ErrorCode::Adapter, "agent service timed out"))??
                .ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::ProcessExited,
                        "agent service closed the connection",
                    )
                })?;
        if response.id != id {
            return Err(AgentError::new(
                ErrorCode::Protocol,
                format!(
                    "agent service answered request {} instead of {id}",
                    response.id
                ),
            ));
        }
        match (response.result, response.error) {
            (Some(result), None) => Ok(Ok(result)),
            (None, Some(error)) => Ok(Err(error)),
            _ => Err(AgentError::new(
                ErrorCode::Protocol,
                "agent service response must carry exactly one of result/error",
            )),
        }
    }
}

/// 服务不可用时按需拉起（MCP 客户端退出后服务继续运行）。
pub fn spawn_detached_service(db_dir: &Path, executable: &Path) -> Result<(), AgentError> {
    let paths = ServicePaths::new(db_dir);
    std::fs::create_dir_all(&paths.dir).map_err(|e| {
        AgentError::new(
            ErrorCode::Adapter,
            format!("cannot create {}: {e}", paths.dir.display()),
        )
    })?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .map_err(|e| {
            AgentError::new(
                ErrorCode::Adapter,
                format!("cannot open service log {}: {e}", paths.log.display()),
            )
        })?;
    let log2 = log.try_clone().map_err(|e| {
        AgentError::new(
            ErrorCode::Adapter,
            format!("cannot duplicate log handle: {e}"),
        )
    })?;
    let mut command = std::process::Command::new(executable);
    command
        .arg("agent-service")
        .arg("--db-path")
        .arg(db_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(log2))
        .current_dir(std::env::temp_dir());
    // 独立进程组：终端关闭的 SIGHUP 和 Ctrl+C 的 SIGINT 只发给前台组，
    // 不能连带结束后台任务。这里不做新的 daemon，只让既有服务真正脱离客户端组。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().map_err(|e| {
        AgentError::new(
            ErrorCode::Adapter,
            format!("cannot start agent service: {e}"),
        )
    })?;
    // 子进程已脱离：MCP 客户端退出只会让它变成孤儿进程，不会结束服务。
    tracing::info!(pid = child.id(), "spawned agent service");
    Ok(())
}

/// 连接服务；不在线则拉起一个并等待其就绪。
pub async fn ensure_service(db_dir: &Path, executable: &Path) -> Result<IpcClient, AgentError> {
    let paths = ServicePaths::new(db_dir);
    if let Ok(client) = IpcClient::connect(&paths).await {
        return Ok(client);
    }
    spawn_detached_service(db_dir, executable)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut last = None;
    while std::time::Instant::now() < deadline {
        match IpcClient::connect(&paths).await {
            Ok(client) => return Ok(client),
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(AgentError::new(
        ErrorCode::Adapter,
        format!(
            "agent service did not become ready at {}: {}",
            paths.socket.display(),
            last.map(|e| e.message).unwrap_or_else(|| "unknown".into())
        ),
    ))
}

/// 打开 Agent Service 使用的 SQLite（与 Model Gateway 共用库文件，独立连接池）。
pub fn open_store(db_dir: &Path) -> Result<Arc<dyn TaskStore>, AgentError> {
    let pool = crate::storage::db::init_database(&db_dir.to_path_buf()).map_err(|e| {
        AgentError::new(
            ErrorCode::Storage,
            format!(
                "cannot open agent database: {}{}",
                e.message,
                e.detail.map(|d| format!(" ({d})")).unwrap_or_default()
            ),
        )
    })?;
    Ok(SqliteStore::new(pool))
}

/// 服务端组装：注册可用 Adapter、装载并恢复持久化状态、尝试 provider 对账。
pub async fn build_service(
    codex: Option<Arc<dyn AdapterFactory>>,
    claude: Option<Arc<dyn AdapterFactory>>,
    store: Arc<dyn TaskStore>,
) -> Result<Arc<AgentService>, AgentError> {
    let mut factories: Vec<Arc<dyn AdapterFactory>> = Vec::new();
    if let Some(codex) = codex {
        factories.push(codex);
    }
    if let Some(claude) = claude {
        factories.push(claude);
    }
    let service = Arc::new(AgentService::with_store(factories, store)?);
    match service.reconcile_interrupted().await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "reconciled interrupted agent tasks"),
        // 对账失败不改变已写下的 interrupted 终态，只记录。
        Err(e) => tracing::warn!(error = %e, "agent task reconciliation failed"),
    }
    Ok(service)
}

#[cfg(test)]
mod phase_three_tests {
    use super::*;

    struct FailingShutdown;
    impl AdapterFactory for FailingShutdown {
        fn descriptor(&self) -> AgentDescriptor {
            AgentDescriptor {
                id: "failure".into(),
                name: "failure".into(),
                permissions: vec![PermissionMode::ReadOnly],
                supports_continuation: false,
                supports_cancel: true,
            }
        }
        fn connect(&self) -> BoxFuture<'_, Result<Arc<dyn AgentAdapter>, AgentError>> {
            Box::pin(async { Ok(Arc::new(FailingShutdown) as Arc<dyn AgentAdapter>) })
        }
    }
    impl AgentAdapter for FailingShutdown {
        fn start_thread<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, Result<String, AgentError>> {
            Box::pin(async { Ok("thread".into()) })
        }
        fn resume_thread<'a>(
            &'a self,
            _: &'a str,
            _: &'a Path,
        ) -> BoxFuture<'a, Result<(), AgentError>> {
            Box::pin(async { Ok(()) })
        }
        fn start_turn<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> BoxFuture<'a, Result<String, AgentError>> {
            Box::pin(async { Ok("turn".into()) })
        }
        fn next_event(&self) -> BoxFuture<'_, Result<AdapterEvent, AgentError>> {
            Box::pin(std::future::pending())
        }
        fn interrupt<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> BoxFuture<'a, Result<(), AgentError>> {
            Box::pin(async { Ok(()) })
        }
        fn shutdown(&self) -> BoxFuture<'_, Result<(), AgentError>> {
            Box::pin(async { Err(AgentError::new(ErrorCode::Adapter, "cleanup failed")) })
        }
    }
    #[tokio::test]
    async fn host_reports_adapter_shutdown_failure() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ServicePaths::new(dir.path());
        let service = Arc::new(AgentService::new(vec![Arc::new(FailingShutdown)]).unwrap());
        let task = service
            .run(RunRequest {
                agent: "failure".into(),
                cwd: dir.path().to_owned(),
                prompt: "hold".into(),
                permission: PermissionMode::ReadOnly,
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while service.status(&task.task_id).unwrap().state != TaskState::Running {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let host = tokio::spawn(serve(
            service,
            ServiceHostConfig {
                paths: paths.clone(),
                idle_timeout: None,
            },
        ));
        let mut client = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(client) = IpcClient::connect(&paths).await {
                    break client;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        client
            .call("service_shutdown", json!({}))
            .await
            .unwrap()
            .unwrap();
        assert!(
            host.await.unwrap().is_err(),
            "host must not report successful shutdown after cleanup failed"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn socket_parent_symlink_cannot_change_another_directory() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::create_dir(&victim).unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let alias = dir.path().join("alias");
        symlink(&victim, &alias).unwrap();
        assert!(transport::bind(&alias.join("socket")).is_err());
        assert_eq!(
            std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[tokio::test]
    async fn oversized_unterminated_message_is_rejected_before_eof() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(8192);
        let sender = tokio::spawn(async move {
            writer
                .write_all(&vec![b'x'; MAX_MESSAGE_BYTES + 1])
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            read_message::<_, Value>(&mut tokio::io::BufReader::new(reader)),
        )
        .await;
        sender.abort();
        assert!(
            matches!(
                result,
                Ok(Err(AgentError {
                    code: ErrorCode::InvalidRequest,
                    ..
                }))
            ),
            "oversized message waited for EOF: {result:?}"
        );
    }

    #[tokio::test]
    async fn oversized_outgoing_message_is_explicitly_rejected() {
        let result = write_message(&mut tokio::io::sink(), &"x".repeat(MAX_MESSAGE_BYTES)).await;
        assert!(matches!(
            result,
            Err(AgentError {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn stale_pid_file_does_not_prevent_new_owner() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ServicePaths::new(dir.path());
        std::fs::create_dir_all(&paths.dir).unwrap();
        std::fs::write(&paths.lock, format!("{}\n", std::process::id())).unwrap();
        let service = Arc::new(AgentService::new(vec![]).unwrap());
        let config = ServiceHostConfig {
            paths: paths.clone(),
            idle_timeout: Some(Duration::from_millis(100)),
        };
        assert!(serve(service, config).await.is_ok());
    }
}
