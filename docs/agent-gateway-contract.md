# Agent Gateway 第一阶段接口契约

本文件对应 `src-tauri/src/agent_gateway/`。本阶段提供进程内 Rust 库接口；MCP、IPC、SQLite、Claude Code、桌面 UI 和跨服务重启恢复均未实现。下文标注“下一轮”的部分是接入约定，不表示已有网络服务。

## 1. 架构与职责

| 模块 | 职责 |
| --- | --- |
| `protocol.rs` | 可序列化请求、Task / Session 记录、状态与错误；Adapter trait |
| `service.rs` | Agent 注册、创建任务、Session 排他、状态转换、取消协调、完整结果保存、关闭 |
| `codex.rs` | 官方 Codex CLI App Server、JSON-RPC 请求关联、Thread / Turn、事件与结果、Native 配置校验、子进程清理 |

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

`AdapterFactory` 提供 `descriptor()` 和异步 `connect()`；后者返回 `Arc<dyn AgentAdapter>`。每条 Adapter 连接专属于一个 Service Session。

| `AgentAdapter` 方法 | 语义 |
| --- | --- |
| `start_thread(cwd)` | 创建真实 Thread，返回 Thread ID |
| `resume_thread(thread_id, cwd)` | 在原目录继续该真实 Thread，并重新验证目录配置和只读工具边界；不拼接历史替代恢复 |
| `start_turn(thread_id, prompt)` | 开始执行，返回 Turn ID |
| `next_event()` | 等待 `AdapterEvent` |
| `interrupt(thread_id, turn_id)` | 发送真实中断；RPC 确认不代表终态 |
| `shutdown()` | 关闭进程和通信资源 |

这些异步方法返回 `BoxFuture<..., Result<..., AgentError>>`。`next_event` 与 `interrupt` 必须可以并发，避免等待事件时阻塞取消。Adapter 的 Drop 负责子进程资源回收。

`AdapterEvent` 分三类：`Progress { thread_id, turn_id, kind, message }`、`TurnFinished { thread_id, turn_id, outcome, final_answer, error }`、`ProcessExited { message }`。Service 仅接受与当前 Thread / Turn 匹配的执行事件；`TurnOutcome::Unknown` 必须失败，不得转换为成功。

Codex Adapter 使用 stdio App Server，覆盖 `initialize`、`config/read`、`model/list`、`thread/start`、`thread/resume`、`turn/start`、`turn/interrupt`、`thread/read`。每个连接使用独立随机 provider 名，provider 不设置 `base_url`，由 Codex 原生实现按当前认证选择官方默认 endpoint。Adapter 验证有效配置和官方目录中的默认模型，拒绝 provider 的 base_url 覆盖、自定义模型目录、附加认证头和 query 参数；固定官方 `chatgpt_base_url` 并移除路由重定向环境变量，避免误用 Model Gateway。它禁用 MCP、插件、hooks、apps、浏览器等可绕过只读沙箱的外部工具，并检查真实 MCP runtime inventory；配置 MCP 时只传各 server 的 `enabled:false`，不回放 null 字段或凭据。

工厂入口固定为 `CodexFactory::new(Option<PathBuf>) -> Self`（延迟连接）、`CodexFactory::discover(Option<PathBuf>).await -> Result<Self, AgentError>`（验证公开 CLI 后构造）。`None` 按 `AGENTGATE_CODEX_BIN` 和 PATH 发现 CLI。上层优先调用 `discover` 成功后注册。

### Native 认证边界

Gateway 复用现有 Codex 认证，继承当前 `CODEX_HOME`、credential store 和原生认证环境。它不管理登录，不调用 `account/read` 设置登录门槛，不创建独立认证目录、不强制文件存储，也不要求先运行登录命令。Codex 能调用就执行；缺少有效认证、服务不可达或执行失败时，通过正常 Task 错误返回。

Gateway 不手动修改或复制 config.toml、auth.json 或 OAuth 凭据；配置隔离全部是子进程级覆盖。用户允许官方 Codex 在正常使用中自动刷新及保存认证。官方原生使用现有 ChatGPT / API 认证不属于 Gateway 的重试切换；Gateway 不在失败后更换认证方式或偷偷切到自定义 Provider。参见 [Codex 官方认证文档](https://learn.chatgpt.com/docs/auth#credential-storage)、[官方配置参考](https://learn.chatgpt.com/docs/config-file/config-reference) 及官方源码快照的 [认证刷新管理](https://github.com/openai/codex/blob/86ee40c3801a8c4fbb35b1159c32da1447cf09d9/codex-rs/login/src/auth/manager.rs) 与 [凭据存储](https://github.com/openai/codex/blob/86ee40c3801a8c4fbb35b1159c32da1447cf09d9/codex-rs/login/src/auth/storage.rs)。源码快照是补充审计依据，不代表本机 CLI 构建的确切 commit。

## 6. 下一轮 SQLite 存储边界

当前全部记录只在 Service 内存中，进程结束即丢失。`TaskRecord` / `SessionRecord` 是存储 DTO，不是已经接入数据库的 Repository。下一轮须增加实际事务存储与恢复入口，不能只在 MCP 层复制一套状态。

至少实现：创建 Session 与初始 Task；按 ID 读取记录；原子检查 Session 空闲并附加 Task；保存 Thread / Turn 映射；保存事件、取消意图、完整结果和错误；原子完成 Task 并清空 Session 占位；查询非终态 Task 并执行重启对账。数据库外键为 Task → Session，`task_ids` 可由有序任务关系派生，但对外字段保持一致。

Child、锁、channel、JoinHandle、Adapter 实例均为运行时资源，不能序列化。不要持久化 OAuth 凭据。重启后不能把旧 `queued/running` 直接标为成功；须通过官方协议查证，无法查证时明确记录 `interrupted` 及原因。跨 Service 重启恢复尚无实现；引入存储钩子和恢复入口前，先按这些字段与核心维护者核对。

## 7. 下一轮 MCP 五个工具

以下是建议采用的 JSON 业务 payload，字段直接对应上述 Rust 类型；当前没有 MCP Server 或工具注册。

| 工具 | 请求 | 成功结果 |
| --- | --- | --- |
| `agent_list` | `{}` | `{ "agents": [AgentDescriptor] }` |
| `agent_run` | `{ "agent": "codex-native", "cwd": "/absolute/repo", "prompt": "审计权限架构", "permission": "read_only" }` | `Submission` |
| `agent_continue` | `{ "session_id": "...", "prompt": "继续调查第一个问题" }` | `Submission` |
| `agent_status` | `{ "task_id": "..." }` | 完整 `TaskRecord` |
| `agent_cancel` | `{ "task_id": "..." }` | 请求取消后的完整 `TaskRecord` |

下一轮将 `AgentError` 转为 MCP tool error，保留原 code 和 message。`agent_status` 返回 `failed/interrupted/cancelled` 也是一次成功查询，不能把这些业务终态包装成 MCP 传输失败。调用方通过查询终态获取最终结果。

## 8. 下一轮 IPC 契约

当前没有监听器、socket、stdio IPC 或桌面命令。本阶段不选择传输机制。下一轮建议复用相同 payload，使用以下 envelope，并在接入前补齐本地认证、请求大小和连接生命周期规则：

```json
{"id":"caller-request-id","method":"agent_run","params":{"agent":"codex-native","cwd":"/absolute/repo","prompt":"审计","permission":"read_only"}}
```

响应仅二选一：`{"id":"caller-request-id","result":Submission}` 或 `{"id":"caller-request-id","error":{"code":"busy","message":"..."}}`。其余方法与 MCP 表一致；关联 ID 由调用方提供，与 Task / Session ID 无关。该 envelope 是未来 IPC 约定，不是 Codex App Server 的线协议。

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
| `process_exited` | App Server 退出；相关任务为 `interrupted` |

错误 message 是诊断文本，不作为机器分支依据，不保证语言或逐字稳定。不能包含 token、auth.json 内容或完整敏感配置。失败正常返回，不由 Gateway 偷偷改用模型代理或另一种认证。

## 10. 完整结果读取

Service 内部通过 `status(task_id)` 返回完整 `final_answer`，不分页、不截断。过程事件不能代替最终回答。成功必须同时满足 Turn 成功、回答存在、内容非空、提取无误；仅收到 `turn/completed` 不够。Adapter 仅提取最终 agentMessage，排除 commentary 和 partial_answer，保留全部最终消息；若完成通知未包含完整 items，就读取原 Thread 的匹配 Turn，拒绝 summary 视图，不依赖固定 sleep。失败或取消也可能带部分内容，应保留原真实状态。

MCP 分页尚未实现。下一轮若增加分页，应单独提供稳定的结果版本及明确的 offset / limit 单位、总长度、下一页位置和完成标记，读取不可变终态结果。不能在现有 `final_answer` 字段静默裁剪。当前内存保存没有数据库耐久性，长时间运行的容量管理也不属于本阶段承诺。

## 11. 已验证与待验证

| 验证项 | 当前证据及边界 |
| --- | --- |
| 独立真实协议 probe | 官方 CLI `0.162.1`、官方来源隔离及 ChatGPT 认证、`gpt-6.1-sol`、只读文件读取与写入阻断、原 Thread 续接、真实 interrupt 通过；该 probe 不能代替最终 Service 验收 |
| Service 单元测试 | 14 组通过，覆盖状态、长结果、连续会话、并发、取消竞态、连接失败、子进程退出及资源清理 |
| Adapter 测试 | 最终原生认证继承、Native 配置拒绝、最终消息提取、MCP 禁用及传输关联测试通过 |
| 全库 headless lib | 最终 1392 项通过、1 项真实权限测试默认忽略；该权限测试已显式执行通过。独立 verifier 此前验证包含 Model Gateway 与 Agent Gateway 的 504 项相关回归通过 |
| 格式与静态检查 | `cargo fmt --manifest-path src-tauri/Cargo.toml --check` 和 headless `cargo clippy --lib -- -D warnings` 通过。扩大到 `--lib --tests` 会报告 `cli/serve.rs` 的既有 lint：754 行 `print_literal`、759 行 `unnecessary_map_or`、845 行 `too_many_arguments`；未扩大本轮修改范围 |
| 最终真实 Service 集成 | PASS，macOS 官方 CLI `0.162.1`，1 项通过、76.15 秒：源码审计完整回答 1177 字符，原 Thread 续审 634 字符；观察实际 `sleep 120` 开始后取消，真实终态 `cancelled` |
| 真实只读执行 | PASS，官方 `exec` 路径 1 项通过、0.11 秒：读取退出码 0，覆盖与新建均退出码 1 / `Operation not permitted`；原文件未变、新文件不存在 |
| 审计无意外写入 | 审计前后 692 个文件指纹和 Git 状态相同；本次 config.toml / auth.json 哈希相同。官方未来自动刷新认证仍属允许行为 |
| Windows | 设计使用跨平台 Rust / 官方 CLI 接口，未做 Windows 原生验收 |
| 进程资源 | shutdown 对活动 Turn 先发送 interrupt，最多等待 2 秒观察对应真实终态，再 kill / wait App Server；未确认时返回显式错误。执行中关闭后是否遗留后代命令进程，尚需真实验收 |
| 现有 protocol fixture | 当前 38 通过、2 失败；在原 HEAD `7e88df1` 的归档副本复现同样失败：`metrics_requires_token_only_and_health_does_not` 的 Host 限制断言收到 200，`unauthenticated_request_is_rejected_before_body_is_read` 超过 5 秒超时。属于既有失败，未在本轮扩大修改范围；其他 fixture 16 通过 |

本阶段核心及 macOS 真实验收 PASS，完整证据见 [验收记录](agent-gateway-verification.md)。真实集成测试入口是 `src-tauri/tests/agent_gateway_codex_live.rs`，直接使用已有 Codex 环境调用。仅安装 Desktop 不保证 PATH 存在 CLI；不能依赖 Desktop 私有可执行路径。无可用官方 CLI 时返回可执行文件错误；认证或调用失败交由正常执行错误上报。Windows、SQLite、MCP、IPC 均不在已完成能力中；强制崩溃后的后代进程回收尚未真实验收。

```bash
# 从仓库根目录执行；真实测试会消耗官方 Codex 使用额度。
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib agent_gateway
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --test agent_gateway_codex_live -- --ignored --nocapture
```

## 12. 下一轮范围与接入顺序

1. 基于当前 DTO 与排他规则实现 SQLite migration、事务存储和崩溃对账；先补测试，再接核心状态变更。
2. 实现 Claude Code `AdapterFactory` / `AgentAdapter`，真实声明能力，不改变 Task 与 Session 语义。
3. 实现 MCP stdio Server 和五个工具，直接调用 Service，不另建任务状态机。
4. 实现本地 IPC，并补认证、并发、错误及完整结果传输测试。

当前没有实现上述四项、Pi / CodeLeveler 接入、桌面管理 UI、安装包集成或 Windows 验收。若需要改变核心接口，先提交具体问题、复现证据和兼容方案，避免由上层重写本轮生命周期。
