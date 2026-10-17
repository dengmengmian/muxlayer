# Agent Gateway 接口契约（第二阶段接口，含第三阶段修正）

本文件对应 `src-tauri/src/agent_gateway/`。第二阶段在保留第一阶段语义的前提下补齐：SQLite 持久化、Claude Code Adapter、本地 IPC、MCP stdio Server 五个工具、Agent Service 独立生命周期与重启恢复。第三阶段在这些接口上修复可靠性并完成 CodeLeveler / Pi 联调；桌面管理 UI 与云端能力仍不在范围内。

> 第三阶段历史结论见 [原报告](agent-gateway-phase-three-verification.md)。后续生命周期验收已修复原始 Codex SIGKILL 残留，真实 A–H 8/8 通过；任意脱链后代仍无绝对回收保证，整体 **PARTIAL**。最新边界见 [生命周期最终报告](agent-gateway-lifecycle-final.md)。

## 1. 架构与职责

| 模块 | 职责 |
| --- | --- |
| `protocol.rs` | 可序列化请求、Task / Session 记录、状态与错误；Adapter trait；`agent_status` 分页投影 |
| `service.rs` | Agent 注册、创建任务、Session 排他、状态转换、取消协调、完整结果保存、写穿持久化、重启对账、关闭 |
| `store.rs` | Task / Session 持久化。SQLite 是唯一真相源；`MemoryStore` 仅供 `AgentService::new` 与单元测试 |
| `codex.rs` | 官方 Codex CLI App Server、JSON-RPC 请求关联、Thread / Turn、事件与结果、Native 配置校验、只读对账、子进程清理 |
| `claude.rs` | 官方 Claude Code CLI `--print --output-format stream-json`、工具白名单强制只读、会话续接、真实取消 |
| `ipc.rs` | 本地 IPC（Unix Domain Socket / Windows Named Pipe）、token 认证、单实例所有权、服务宿主与按需拉起 |
| `mcp.rs` | MCP stdio Server：`initialize` / `tools/list` / `tools/call`，五个工具，参数校验与错误映射 |

数据流：`MCP Client → MCP stdio → 本地 IPC → Agent Service → Codex / Claude Code`。

Agent Gateway 独立于 `gateway/` Model Gateway。它不经过 Provider 选路或模型协议转换，不使用 `gateway/session_store.rs` 的模型响应缓存。`tools/mcp.rs` 管理客户端配置，`session_sync/codex.rs` 解析历史日志，均不承担实时 Agent 执行。

## 2. 已有 Agent Service 接口

调用方须提供运行中的 Tokio runtime。导入入口为 `agentgate_lib::agent_gateway`。

| Rust 接口 | 返回值 / 语义 |
| --- | --- |
| `AgentService::new(Vec<Arc<dyn AdapterFactory>>)` | `Result<AgentService, AgentError>`；拒绝空或重复 Agent ID |
| `list()` | `Vec<AgentDescriptor>`，按 ID 排序；只描述已注册 Adapter 能力，不能据此认定已登录 |
| `run(RunRequest)` | `Result<Submission, AgentError>`；校验后立即返回新 Task / Session ID，连接和执行在后台进行 |
| `continue_session(ContinueRequest)` | `Result<Submission, AgentError>`；复用原 Session 和 Thread，创建新 Task；已有活动 Task 时返回 `busy` |
| `status(&task_id)` | `Result<TaskRecord, AgentError>`；返回当前完整快照 |
| `session(&session_id)` | `Result<SessionRecord, AgentError>`；返回当前完整会话快照 |
| `cancel(&task_id).await` | `Result<TaskRecord, AgentError>`；返回请求后的快照，可能仍在执行，必须继续查询终态 |
| `shutdown().await` | `Result<(), AgentError>`；停止接收任务，中断未完成任务并关闭 Adapter |
| `with_store(factories, store)` | `Result<AgentService, AgentError>`；持久化入口。装载历史记录，并把重启时仍非终态的任务降为 `interrupted` |
| `reconcile_interrupted().await` | `Result<usize, AgentError>`；用官方协议查证那些任务的真实终局，只有 provider 明确确认 `completed` 且带完整回答才升级 |
| `running_task_ids()` | `Result<Vec<String>, AgentError>`；非终态任务 ID，供服务生命周期决策 |

`RunRequest = { agent, cwd, prompt, permission }`；`ContinueRequest = { session_id, prompt }`；`Submission = { task_id, session_id }`。`cwd` 必须是存在的目录，Service 将其规范化。空白 prompt 被拒绝。只有 `read_only` 开放；虽然枚举保留 `execute`，提交它始终返回 `unsupported_permission`。

`AgentDescriptor = { id, name, permissions, supports_continuation, supports_cancel }`。可执行文件发现成功后再注册工厂；模型来源检查在连接阶段进行，认证交给 Codex 原生处理。运行请求被接收不等于已执行成功。

## 3. Task / Session 数据契约

所有枚举序列化为 `snake_case`。ID 是不透明字符串，Task ID、Session ID、Codex Thread ID、Codex Turn ID 各有独立含义，不可互换。时间字段为 UTC RFC 3339 字符串。

| 记录 | 字段 |
| --- | --- |
| `TaskRecord` | `task_id`, `session_id`, `prompt`, `state`, `thread_id: string?`, `turn_id: string?`, `final_answer: string?`, `error: AgentError?`, `events: TaskEvent[]`, `cancel_requested: bool`, `created_at`, `finished_at: string?` |
| `SessionRecord` | `session_id`, `agent`, `cwd`, `permission`, `thread_id: string?`, `active_task_id: string?`, `task_ids: string[]`, `created_at` |
| `TaskEvent` | `kind: string`, `message: string` |
| `AgentError` | `code: ErrorCode`, `message: string` |

一个 Session 保存一个真实 Thread 映射，允许多个顺序 Task；每个 Task 保存自己的 Turn 映射。执行前 ID 字段可为空。`active_task_id` 是运行中的互斥占位，终态后清空。独立 Session 可以并行，Session 内不排队执行多个冲突 Turn。

## 4. 状态转换

| 状态 | 含义及转换 |
| --- | --- |
| `queued` | 已接收，可能正在连接或准备 Thread；可到 `running`、`cancelled`、`failed`、`interrupted` |
| `running` | 已取得真实 Turn ID；可到 `completed`、`failed`、`cancelled`、`interrupted` |
| `completed` | Adapter 确认成功、无错误且完整最终回答非空 |
| `failed` | RPC / 协议 / 执行失败，成功终态缺少回答，或未知 Turn 结果 |
| `cancelled` | 未启动的任务被取消，或观察到真实取消终态 |
| `interrupted` | 子进程退出、Service 关闭，或 Adapter 明确报告中断 |

后四项是终态，不再改写；记录 `finished_at` 并释放 Session。`cancel_requested=true` 只表示取消意图，不能据此把正在执行的任务标为 `cancelled`。完成与取消竞态以真实终态为准：若真实执行已成功且结果完整，可以是 `completed`。重复取消终态任务返回原快照。

## 5. Adapter 接口

`AdapterFactory` 提供 `descriptor()`、异步 `connect()`（返回 `Arc<dyn AgentAdapter>`）和带默认实现的 `reconcile(session, task)`（只读对账，默认返回 `None`）。每条 Adapter 连接专属于一个 Service Session。

| `AgentAdapter` 方法 | 语义 |
| --- | --- |
| `start_thread(cwd)` | 创建真实 Thread，返回 Thread ID |
| `resume_thread(thread_id, cwd)` | 在原目录继续该真实 Thread，并重新验证目录配置和只读工具边界；不拼接历史替代恢复 |
| `start_turn(thread_id, prompt)` | 开始执行，返回 Turn ID |
| `next_event()` | 等待 `AdapterEvent` |
| `interrupt(thread_id, turn_id)` | 发送真实中断；RPC 确认不代表终态 |
| `shutdown()` | 关闭进程和通信资源 |

`AdapterFactory::reconcile` 只在服务重启恢复时使用：它必须通过官方协议**只读**查证那个 Turn 的真实终局，禁止借对账重新执行任务。返回 `None` 表示无法查证，服务保留 `interrupted`。
这些异步方法返回 `BoxFuture<..., Result<..., AgentError>>`。`next_event` 与 `interrupt` 必须可以并发，避免等待事件时阻塞取消。Adapter 的 Drop 负责子进程资源回收。

`AdapterEvent` 分三类：`Progress { thread_id, turn_id, kind, message }`、`TurnFinished { thread_id, turn_id, outcome, final_answer, error }`、`ProcessExited { message }`。Service 仅接受与当前 Thread / Turn 匹配的执行事件；`TurnOutcome::Unknown` 必须失败，不得转换为成功。

Codex Adapter 使用 stdio App Server，覆盖 `initialize`、`config/read`、`model/list`、`thread/start`、`thread/resume`、`turn/start`、`turn/interrupt`、`thread/read`。每个连接使用独立随机 provider 名，provider 不设置 `base_url`，由 Codex 原生实现按当前认证选择官方默认 endpoint。Adapter 验证有效配置和官方目录中的默认模型，拒绝 provider 的 base_url 覆盖、自定义模型目录、附加认证头和 query 参数；固定官方 `chatgpt_base_url` 并移除路由重定向环境变量，避免误用 Model Gateway。它禁用 MCP、插件、hooks、apps、浏览器等可绕过只读沙箱的外部工具，并检查真实 MCP runtime inventory；配置 MCP 时只传各 server 的 `enabled:false`，不回放 null 字段或凭据。

工厂入口固定为 `CodexFactory::new(Option<PathBuf>) -> Self`（延迟连接）、`CodexFactory::discover(Option<PathBuf>).await -> Result<Self, AgentError>`（验证公开 CLI 后构造）。`None` 按 `AGENTGATE_CODEX_BIN` 和 PATH 发现 CLI。上层优先调用 `discover` 成功后注册。

### Claude Code Adapter

`ClaudeFactory` 与 `CodexFactory` 同形（`new` / `discover`，`None` 按 `AGENTGATE_CLAUDE_BIN` 和 PATH 发现）。Adapter ID 为 `claude-code`，能力：`read_only`、支持续会、支持取消。

程序化入口是官方 `claude --print --output-format stream-json --verbose`，每个 Turn 一个进程：

| 参数 | 作用 |
| --- | --- |
| `--tools Read,Grep,Glob` | **真实工具白名单**：Bash / Edit / Write / NotebookEdit / WebFetch / Task 都不存在，模型没有写文件或执行命令的工具 |
| `--setting-sources user` | 只加载用户级 settings；项目 / 本地 settings 一律忽略，仓库无法注入 hooks、MCP 或插件 |
| `--strict-mcp-config` | 不加载任何外部 MCP server |
| `--disable-slash-commands` | 关闭 skills |
| `--permission-prompts none` | 任何需要审批的动作直接被拒绝 |
| `--session-id <uuid>` / `--resume <uuid>` | 新建 / 继续会话 |

只读不是提示词约定：每个 Turn 的 `init` 事件会被校验，`tools` 必须落在白名单内且 `mcp_servers` 必须为空，否则该 Turn 直接失败（`protocol`）。

**为什么不用 `--restricted`**：它会把用户级 settings 一起忽略，而第三方 Anthropic 兼容端点（DeepSeek 等）正是通过用户 settings 的 `env`（或进程环境变量）配置的。实测 `--restricted` 下 model 回退到内置默认模型并认证失败，等于让 Claude Code 只能跑官方登录。`--setting-sources user` 保留了"排除项目 / 本地配置"这条安全边界（真实 CLI 探针确认仓库级 hook 不会执行），同时让用户自己的模型配置生效。

明确边界：读取类工具不再被限制在 cwd 内（仍只读，无写入能力）；用户级 hooks / 插件会生效——那是机器主人自己的配置，与凭据同一信任级别。`resume_thread` 校验 cwd 与 UUID 形式；会话是否真实存在由 CLI 的 `--resume` 裁决，不猜测私有磁盘格式。`interrupt` 是终止该次 `--print` 进程（官方没有 interrupt RPC），终止后由 EOF 报告 `interrupted`，服务据 `cancel_requested` 记为 `cancelled`。认证复用本机 Claude Code 登录：连接前用官方 `claude auth status` 预检，未登录（或存在 `ANTHROPIC_API_KEY` 等本机认证环境）时明确返回 `adapter` 错误，**不**读取、输出、复制凭据，也不切换账号或模型。

### 六个 CLI 入口

| 命令 | 作用 |
| --- | --- |
| `agentgate-serve agent-service [--idle-timeout N]` | 启动 Agent Service（独立进程；默认永不因空闲退出） |
| `agentgate-serve mcp-agent` | 启动 MCP stdio Server；服务不在线时自动拉起并等待就绪 |

两者都不需要 GUI。CodeLeveler / Pi 注册 `agentgate-serve mcp-agent` 即可接入。

### Native 认证边界

Gateway 复用现有 Codex 认证，继承当前 `CODEX_HOME`、credential store 和原生认证环境。它不管理登录，不调用 `account/read` 设置登录门槛，不创建独立认证目录、不强制文件存储，也不要求先运行登录命令。Codex 能调用就执行；缺少有效认证、服务不可达或执行失败时，通过正常 Task 错误返回。

Gateway 不手动修改或复制 config.toml、auth.json 或 OAuth 凭据；配置隔离全部是子进程级覆盖。用户允许官方 Codex 在正常使用中自动刷新及保存认证。官方原生使用现有 ChatGPT / API 认证不属于 Gateway 的重试切换；Gateway 不在失败后更换认证方式或偷偷切到自定义 Provider。参见 [Codex 官方认证文档](https://learn.chatgpt.com/docs/auth#credential-storage)、[官方配置参考](https://learn.chatgpt.com/docs/config-file/config-reference) 及官方源码快照的 [认证刷新管理](https://github.com/openai/codex/blob/86ee40c3801a8c4fbb35b1159c32da1447cf09d9/codex-rs/login/src/auth/manager.rs) 与 [凭据存储](https://github.com/openai/codex/blob/86ee40c3801a8c4fbb35b1159c32da1447cf09d9/codex-rs/login/src/auth/storage.rs)。源码快照是补充审计依据，不代表本机 CLI 构建的确切 commit。

## 6. SQLite 存储边界（已实现）

复用 MuxLayer 现有 SQLite 基础设施（`storage::db::DbPool` + `storage/migrations.rs`），**不修改** Model Gateway 的 `gateway/session_store.rs`。

### 表结构

`user_version` 保持 **12**：2.0.5 及更早版本看到更高版本会拒绝启动，而空的新表对旧版本完全无害。因此新增表走 `migrations::ensure_additive_tables`，每次启动 `CREATE TABLE IF NOT EXISTS` 幂等补齐，不 bump 版本。

| `agent_sessions` | 说明 |
| --- | --- |
| `session_id` TEXT PK | 不透明 ID |
| `agent` TEXT | Adapter ID（`codex-native` / `claude-code`） |
| `provider_session_id` TEXT | 真实 Provider Thread / Session 映射 |
| `cwd` TEXT | 规范化后的目录 |
| `mode` TEXT | `read_only` / `execute` |
| `active_task_id` TEXT | 运行中的互斥占位，终态清空 |
| `created_at` / `updated_at` TEXT | UTC RFC 3339 |

| `agent_tasks` | 说明 |
| --- | --- |
| `task_id` TEXT PK | 不透明 ID，与 Session ID 不混用 |
| `session_id` TEXT NOT NULL → `agent_sessions(session_id)` ON DELETE CASCADE | 一个 Task 只属于一个 Session |
| `provider_turn_id` TEXT | 真实 Turn 映射 |
| `status` TEXT | `queued/running/completed/failed/cancelled/interrupted` |
| `prompt` TEXT | 原始请求 |
| `final_answer` TEXT | 完整最终回答（不裁剪） |
| `error_json` TEXT | `AgentError` JSON |
| `events_json` TEXT NOT NULL DEFAULT '[]' | 过程事件 |
| `cancel_requested` INTEGER | 取消意图 |
| `created_at` / `updated_at` / `completed_at` TEXT | 时间字段 |

索引：`idx_agent_tasks_session(session_id, created_at)`、`idx_agent_tasks_status(status)`。

### 写入规则

- 写穿：`run` / `submit` 在一个事务里写入 Task 和 Session 占位，再更新缓存；取消意图、Thread / Running 映射同样先落库。持久化失败直接返回 `storage` 错误，不留下孤立占位或内存假状态。
- 原子完成：`finish_task` 在同一个事务里写 Task 终态 + 清空 Session 占位。
- 终态结果不可丢：若 `completed` 的落库失败，任务降级为 `failed`（`storage` 错误）并重试一次；**绝不允许「结果没落库但对外报成功」**。
- 终态持续写盘失败时，`status` 明确返回 `storage`，保留最后耐久快照与 Session 占位，阻止不可靠续会；恢复存储后需重启对账，未承诺自动自愈。
- 过程事件按 25 条为一批刷盘，终态与状态转换（Thread / Turn / cancel 意图）立即写入。
- `task_ids` 由 `agent_tasks` 有序关系派生，不单独存储。
- Child / 锁 / channel / JoinHandle / Adapter 实例都是运行时资源，不持久化；不持久化任何凭据。

### 重启恢复规则

- 启动时装载全部记录；仍处于 `queued` / `running` 的任务**先**降级为 `interrupted`，错误码 `process_exited`，原因写明「服务在任务终态前重启，provider 执行状态未知」，并清空对应 Session 占位。
- 随后由 `reconcile_interrupted()` 逐个对账：Codex 用新的官方 App Server 调 `thread/read` 查原 Turn；只有真实 `completed` 且拿到完整最终回答才升级为 `completed`；`inProgress` / 未知状态保持 `interrupted`；查证失败只记录，不伪装完成。
- Claude Code 的会话数据是 CLI 私有磁盘格式，没有官方只读终局查询入口，因此不做猜测式解析，一律保持 `interrupted`。
- 对账前再次重启时，从持久化的重启错误标识重建待对账队列，不丢失原 Turn。
- 绝不重新执行任务来「恢复」。

## 7. MCP 五个工具（已实现）

MCP stdio Server 只做请求解析、参数校验、调用 Agent Service（经 IPC）、结果序列化与错误映射。它不持有任务状态机，不重试业务错误。

| 工具 | 请求 | 结果 |
| --- | --- | --- |
| `agent_list` | `{}` | `{ "agents": [AgentDescriptor] }` |
| `agent_run` | `{ "agent": "codex-native", "cwd": "/absolute/repo", "prompt": "审计权限架构", "permission": "read_only" }` | `Submission`（立即返回，不等执行完成） |
| `agent_continue` | `{ "session_id": "...", "prompt": "继续调查第一个问题" }` | `Submission` |
| `agent_status` | `{ "task_id": "...", "final_answer_offset"?, "final_answer_limit"?, "events_offset"?, "events_limit"? }` | 完整 `TaskRecord` + 分页元数据 |
| `agent_cancel` | `{ "task_id": "..." }` | 请求取消后的 `TaskRecord`（**不是**终态） |

- `agent_run` / `agent_continue` 只提交任务，必须快速返回；执行在 Agent Service 后台进行。
- 顶层 `AgentError` 映射为 `isError: true` 的工具结果，文本是 `{"error":{"code","message"}}`，保留原 code 与 message。
- `agent_status` / `agent_cancel` 返回的 `TaskRecord.error` 是任务数据，不是工具失败；`failed/interrupted/cancelled` 也是一次成功查询，必须保持 `isError: false`，否则客户端无法读取取消或失败后的结果。
- 未知工具或参数缺失 → JSON-RPC `-32602`；未知方法 → `-32601`；解析失败 → `-32700`。
- 协议方法：`initialize`、`notifications/initialized`、`ping`、`logging/setLevel`、`tools/list`、`tools/call`。`initialize` 在 `2024-11-05` / `2025-03-26` / `2025-06-18` 中原样回显，否则回落到 `2025-06-18`。
- stdout 只允许协议消息；tracing 在 `mcp-agent` 模式写入 stderr（有自动化测试断言 stdout 为空）。

### 分页（不静默截断）

`final_answer` 默认返回全部内容。显式给出 `final_answer_limit` 时按 Unicode 字符（scalar value）切片，并同时返回：

| 字段 | 含义 |
| --- | --- |
| `final_answer_offset` | 本次切片起点（被 clamp 到总长度内） |
| `final_answer_total_chars` | 完整回答的字符总数 |
| `final_answer_next_offset` | 下一页起点；`null` 表示已经取完 |
| `final_answer_complete` | 是否已取完 |

按 `offset` 递增顺序拼接各页 `final_answer` 即可精确还原原文（有自动化测试覆盖中英文混排的重组）。过程事件默认不返回，只报告 `events_total` 与 `events_next_offset`（非 0 表示仍可读）；给出 `events_limit` 时才返回 `events` 并置 `events_included: true`。

## 8. 本地 IPC 契约（已实现）

```json
{"id":"caller-request-id","method":"agent_run","params":{"agent":"codex-native","cwd":"/absolute/repo","prompt":"审计","permission":"read_only"}}
```

响应仅二选一：`{"id":"caller-request-id","result":{...}}` 或 `{"id":"caller-request-id","error":{"code":"busy","message":"..."}}`。`id` 由调用方提供，与 Task / Session ID 无关。

| 传输 | 路径 |
| --- | --- |
| macOS / Linux | Unix Domain Socket：`<db_dir>/agent-service/agent-service.sock`，权限 0600，目录 0700；路径超过 `sun_path` 上限时回落到用户私有临时目录（目录哈希命名） |
| Windows | Named Pipe：`\\.\pipe\muxlayer-agent-<db_dir 哈希>`，OS 文件锁 + `first_pipe_instance`；Windows 原生权限与运行尚未验收 |

| 方法 | 说明 |
| --- | --- |
| `hello` | `{"token": "...", "client": "mcp"}`；token 由服务启动时生成并写入 0600 文件，客户端读同一文件。失败返回 `unauthorized` |
| `agent_list` / `agent_run` / `agent_continue` / `agent_status` / `agent_cancel` | 语义与 MCP 工具一一对应，参数相同（`agent_status` 直接接受分页参数） |
| `agent_session` | `{"session_id": "..."}` → `SessionRecord`，用于校验续会映射 |
| `service_status` | `{"service","version","running_task_ids","agents"}` |
| `service_shutdown` | 请求优雅停止服务（中断任务，清理 socket / token，释放 OS 锁；lock 文件保留） |

规则：

- IPC 与 MCP 输入均在读取期间限制 32 MiB（含换行），不等 EOF 才检查。IPC 响应超限返回结构化 `invalid_request`，提示显式分页，同一连接仍可查询；不静默截断。`agent_cancel` 的完整快照也受此限制，取消动作可能已执行，应使用 `agent_status` 显式分页确认。
- 客户端每次请求 30 秒超时；服务端所有方法都必须快速返回（任务在后台执行）。
- 连接断开只影响该连接：任务属于服务端，继续执行；重连后可用原 `task_id` / `session_id` 继续。
- MCP 自动拉起的 Service 在 Unix 上使用独立进程组：终端关闭的 `SIGHUP`、Ctrl+C 的 `SIGINT` 只发给前台 MCP 客户端，不会连带终止 Service 和正在执行的任务。这不是新的 daemon 体系，只是让既有 Service 真正脱离客户端进程组。
- 连接之间互不阻塞（每个连接独立任务）。
- 单实例所有权：CLI 在打开数据库、恢复任务前获取 OS 文件锁，失败返回 `busy`；PID 文本不参与裁决。锁文件永久保留，退出只释放句柄，避免 unlink 造成 inode 分裂。socket probe 仅用于兼容发现已有旧服务。
- MCP 传输断开只重试 `agent_list` / `agent_status`；变更请求不自动重放，明确报告结果未知，避免重复执行。
- macOS Adapter 通过同一应用内部 native owner 管理寿命FD与root退出事件：出生gate前同步隔离root PGID并捕获内核uniqueid，退出时有界证明父实例链、audit token原子信号，并确认已登记进程退出。不使用名称/PID文本/PGID判断归属；不保证首次发现前已丢失身份链的后代。其他Unix保留原进程组guardian，Windows当前仍为单Child控制。
- 取消真实终局之后，Service先确认整个Adapter scope关闭再发布`cancelled`；释放运行时Adapter缓存，续会通过新连接官方resume原Thread。清理失败保存Session错误并阻止继续。
- Codex 在 `turn/start` 已返回 turn ID、但 provider 尚未进入 active/interruptible 状态时，会先有界等待真实 `turn/started`；若仍未启动，使用官方 `turnId=""` startup interrupt。取消 RPC 成功不等于终态，仍以 `turn/completed` 裁决；已完成或已退出不再发无效 interrupt，不重新提交 `turn/start`，不无限重试。
- macOS library嵌入方须让main调用内部owner分流，或通过`AGENTGATE_PROCESS_OWNER_BIN`指定配套CLI；本应用CLI/桌面自身已处理入口。
- 优雅关闭失败会使宿主返回真实错误，不输出假成功。
- IPC 层不重新定义 Task 语义，也不复制状态机。

## 9. 错误语义

| code | 含义 |
| --- | --- |
| `invalid_request` | 参数无效，如空 prompt、无效 cwd、重复注册 |
| `unsupported_permission` | 请求的权限未开放或 Adapter 不支持 |
| `unknown_agent` | Agent 未注册 |
| `not_found` | Task / Session 不存在 |
| `busy` | Session 已有活动任务 |
| `closed` | Service 已关闭 |
| `adapter` | Adapter 连接、认证、配置或执行失败；根据 message 排查 |
| `protocol` | 响应 / 事件异常、未知结果、成功但缺失最终回答 |
| `process_exited` | App Server / 子进程退出，或服务重启；相关任务为 `interrupted` |
| `storage` | 持久化失败。终态结果未能落库时不允许报告成功 |
| `unauthorized` | 本地 IPC 未通过 token 认证 |

错误 message 是诊断文本，不作为机器分支依据，不保证语言或逐字稳定。不能包含 token、auth.json 内容或完整敏感配置。失败正常返回，不由 Gateway 偷偷改用模型代理或另一种认证。

## 10. 完整结果读取

Service 内部通过 `status(task_id)` 返回完整 `final_answer`，不分页、不截断。过程事件不能代替最终回答。成功必须同时满足 Turn 成功、回答存在、内容非空、提取无误；仅收到 `turn/completed` 不够。Adapter 仅提取最终 agentMessage，排除 commentary 和 partial_answer，保留全部最终消息；若完成通知未包含完整 items，就读取原 Thread 的匹配 Turn，拒绝 summary 视图，不依赖固定 sleep。失败或取消也可能带部分内容，应保留原真实状态。

MCP 分页见第 7 节：默认返回完整回答，显式分页时给出稳定的字符单位、总长度、下一页位置与完成标记，只读取已持久化的终态结果。`final_answer` 字段不会静默裁剪。长时间运行的容量管理（事件清理/归档）仍不在本阶段承诺内。

## 11. 第二阶段历史验证（第三阶段见最新报告）

| 验证项 | 当前证据及边界 |
| --- | --- |
| 第一阶段真实 Codex Service 集成 | PASS，macOS 官方 CLI `0.162.1`；本轮复跑仍通过（67.29 秒），无回归 |
| 持久化 | `agent_sessions` / `agent_tasks` 通过 SQLite 落库；`user_version` 保持 12；完成结果在新连接池/新服务实例下可读；外键拒绝孤儿 Task；`storage` 失败时任务降级为 `failed`，绝不报成功 |
| 重启恢复 | 非终态任务先降为 `interrupted` 并落库；Codex 经 `thread/read` 只读对账，只有真实完成且带完整回答才升级；Claude 无官方只读终局入口，保持 `interrupted` |
| Claude Adapter（进程级） | 8 项通过：真实 argv 无写/命令工具、init 白名单校验、真实进程执行 Turn、`--resume` 续会、真实取消、认证预检拒绝、CLI 错误如实上报 |
| Claude 真实 CLI 只读边界 | PASS：真实 CLI `2.1.287` 对生产 argv 的 `init` 事件只报告 `["Glob","Grep","Read"]`、`mcp_servers: []`、`session_id` 等于请求值；仓库级 settings hook 不执行 |
| Claude 真实推理 | PASS：本机 Claude Code 已按官方方式接到 DeepSeek（用户 settings `env`），经 MCP 完整跑通 run / continue / cancel，`init` 报告 `model=deepseek-flash[1m]` |
| MCP 端到端 | 8 项通过：`initialize` / `tools/list` / 五个工具、长任务不阻塞（提交 <800ms）、分页精确重组、客户端退出后任务继续与重连查询、服务重启后结果可读、取消为真实 provider 中断、业务错误与参数错误区分 |
| stdout 纯净 | PASS：真实二进制 `mcp-agent` 在 stdin 关闭时 stdout 为空、stderr 有日志 |
| 全库 headless lib | 1415 项通过、1 项真实权限测试默认忽略 |
| 格式与静态检查 | `cargo fmt --check` 通过；headless `cargo clippy --lib -- -D warnings` 通过；Agent Gateway 相关 target 的 `clippy -D warnings` 通过。`--lib --tests` 仍只报告 `cli/serve.rs` 的三项既有 lint（852/857/943 行），未扩大范围 |
| 现有 protocol fixture | 38 通过、2 失败，与第一阶段记录的既有失败完全一致，本轮未新增失败 |
| Windows | 仅实现 Named Pipe 代码路径，未编译、未验收 |

完整证据见 [验收记录](agent-gateway-verification.md)。

```bash
# 从仓库根目录执行；真实测试会消耗官方 Codex 使用额度。
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib agent_gateway
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --test agent_gateway_mcp_ipc
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --test agent_gateway_codex_live -- --ignored --nocapture
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --test agent_gateway_phase_two_live -- --ignored --nocapture --test-threads=1
```

## 12. 第三阶段后尚未收口的边界

1. 原始macOS Codex App Server SIGKILL独立Session后代残留已通过新native owner修复，真实A–H全通过。首次发现前祖先退出/孤儿exec脱链仍无法绝对回收；最新生命周期结论PARTIAL，见[最终报告](agent-gateway-lifecycle-final.md)。
2. Windows 原生编译、Named Pipe 权限、进程树所有权及端到端验收；交叉检查被缺失 Windows SDK 阻塞。
3. CodeLeveler / Pi 常规 run / status / continue、完整结果已真实通过；CodeLeveler 显式取消、Pi Unicode 分页、Pi → Claude Code 亦已通过；全部异常退出及原生聊天取消联动语义未穷尽。
4. Claude Code 实际推理来源为用户既有 DeepSeek 配置；未宣称官方 Anthropic 模型验收。
5. 长期容量管理与 UI 均未扩展。
6. macOS RC 为 **CONDITIONAL_PASS**：headless CLI 真实安装、MCP、Codex、Claude Code、CodeLeveler、Pi 与 A–H 已验收；Service 终端组信号、MCP `isError` 和 turn-start 取消竞态均已修复。唯一集成条件是 CodeLeveler 发布包含 `e4510fa` 的版本；Windows 未验收，正式发布未执行，见 [macOS RC 报告](agent-gateway-macos-rc.md)。

完整证据、Commit SHA 与未解决风险见 [第三阶段验收报告](agent-gateway-phase-three-verification.md) 和 [macOS RC 报告](agent-gateway-macos-rc.md)。改变核心接口前仍需具体复现与兼容方案。
