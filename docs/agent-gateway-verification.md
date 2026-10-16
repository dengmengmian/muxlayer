# Agent Gateway 第二阶段验收记录

macOS（官方 Codex CLI `0.162.1`、官方 Claude Code CLI `2.1.287`）下：SQLite 持久化、Claude Code Adapter、本地 IPC、MCP stdio Server、服务独立生命周期全部实现并通过自动化验收。

**Codex 经 MCP 的真实调用 PASS。Claude Code 真实推理 BLOCKED**：本机 Claude Code 未登录（`claude auth status` → `loggedIn: false`, `authMethod: none`），真实任务按设计返回明确的 `adapter` 认证错误。Adapter 本身已通过真实进程级验证，真实 CLI 的只读工具边界也已用真实 CLI 探针取得证据（见下文）。

## 1. 真实 MCP 调用（Codex）

真实链路：`真实 MCP stdio 客户端 → agentgate-serve mcp-agent → 本地 IPC(UDS) → Agent Service → 官方 Codex App Server → 临时 Git 仓库`。

测试入口 `src-tauri/tests/agent_gateway_phase_two_live.rs::live_codex_via_mcp_stdio_full_lifecycle`，1 项通过、20.00 秒。长度直接取自 `final_answer`，无裁剪。

| Case | Task ID | Session ID | Thread ID | Turn ID | State | Answer chars | Events |
| --- | --- | --- | --- | --- | --- | --- | --- |
| run | cfea7770-8ef1-4dd7-8fee-27a53a7230f1 | 9bd51c1c-66a3-4b27-9017-b10e3eaac73e | 01a1430a-4454-7c93-8960-0c2da7f725b1 | 01a1430a-46a0-75f2-abf6-d033240b0b1d | completed | 36 | 46 |
| continue | a5cb6f07-d31b-417d-ae2c-abaf3e37a274 | 9bd51c1c-66a3-4b27-9017-b10e3eaac73e | 01a1430a-4454-7c93-8960-0c2da7f725b1 | 01a1430a-62cf-7c52-917b-53950a9a40c1 | completed | 36 | 25 |
| cancel | 6f61f405-d86b-4ca5-a4ec-726f2b5dca16 | 06d422c0-7890-4bd1-88fd-5ec169c94d72 | 01a1430a-6cdf-7512-9bad-c6b30f788d8c | 01a1430a-6db2-7db2-960f-ed916d767e8d | cancelled | 0 | 23 |

- `agent_list` 返回 `claude-code` 与 `codex-native`；`tools/list` 返回五个工具。
- 首次回答精确返回仓库里的随机 nonce；续会 Task / Turn 不同、Session / Thread 相同，回答同样包含该 nonce。
- 分页：以 64 字符为页循环读取 `agent_status`，拼接结果与原文逐字相同。
- 只读：`evidence.txt` 内容未变，仓库内没有新增文件（`.git` 与 `evidence.txt` 之外为空）。
- 取消：先观察到真实命令开始事件，再发取消；终端状态为 `cancelled`。

```json
{
  "kind": "item/started",
  "message": "{\"command\":\"/bin/zsh -lc 'sleep 120'\",\"exitCode\":null,\"status\":\"inProgress\",\"type\":\"commandExecution\"}"
}
```

- 客户端退出后服务仍在：杀掉 MCP 客户端进程，token 与 socket 仍在；新的 MCP 客户端连上同一个服务，用原 `task_id` 读回完全相同的完整回答，并用原 `session_id` 继续了会话（第三次任务 completed）。最后通过 IPC `service_shutdown` 优雅停止，socket / token / lock 均被清理。

## 2. Claude Code 真实 CLI 证据

### 只读工具边界（PASS，与登录状态无关）

测试 `live_claude_init_event_reports_only_read_only_tools`，1 项通过、1.11 秒。用生产函数 `read_only_cli_args` 生成的真实 argv 调用真实 CLI，读取它自己报告的 `init` 事件：

```text
exit=exit status: 1 stdout_lines=3
init: session="8709f336-f7d8-43c7-93af-3e1e5b64afeb" tools=["Glob","Grep","Read"] mcp=[]
```

即：`--tools Read,Grep,Glob --restricted --strict-mcp-config --disable-slash-commands --permission-prompts none` 下，真实可调用工具只有 `Glob/Grep/Read`，没有任何写文件、执行命令或外部 MCP 工具。`init` 在认证失败之前到达，所以这条证据不依赖登录。

### 真实推理（BLOCKED）

测试 `live_claude_via_mcp_stdio_full_lifecycle` 在本机的结果：

```json
{
  "state": "failed",
  "error": {
    "code": "adapter",
    "message": "Claude Code is not authenticated. Sign in with `claude auth login` (or configure the CLI's own credentials), then retry. MuxLayer does not read credentials, switch accounts or fall back to another model."
  },
  "final_answer": null
}
```

本机 `claude auth status` 实际输出（`exit 1`）：`{"loggedIn": false, "authMethod": "none", ...}`。Gateway 没有读取凭据、没有切换账号或模型，也没有伪造成功。完成 Claude 真实推理验收只需登录后重跑该测试。

## 3. Adapter 进程级验证（Claude Code）

`agent_gateway::claude::tests` 8 项通过，全部使用真实子进程（测试替身 CLI，记录 argv / cwd / 会话模式）：

- 生产 argv 不含 `Bash/Edit/Write/NotebookEdit/WebFetch/Task`，含 `--restricted`、`--strict-mcp-config`、`--disable-slash-commands`、`--permission-prompts none`。
- `init` 报告白名单外工具或非空 `mcp_servers` → Turn 失败（`protocol`），不伪装成功。
- 真实进程执行 Turn：`--session-id <uuid>` 新建、`--resume <uuid>` 续会、cwd 为请求目录、最终回答取自 `result` 事件。
- 真实取消：终止子进程，报告 `interrupted`（服务层记 `cancelled`），耗时远小于命令本身的 60 秒。
- 未登录：连接阶段即拒绝（`adapter`）。
- CLI 以 `is_error: true` 报告错误（如认证失败）→ 任务 `failed`，错误文本来自 CLI。

## 4. 持久化与重启恢复

- 迁移：`agent_sessions` / `agent_tasks` 幂等创建，`user_version` 保持 **12**（新增表不能 bump：2.0.5 及更早版本看到更高版本会拒绝启动）。
- 外键拒绝孤儿 Task；`Provider Session` / `Turn` / 事件 / 取消意图 / 错误 / 完整回答均可往返读写；非法 `status` 直接报错而不是静默映射。
- 已完成结果在**新连接池 + 新服务实例**下可读（`completed_result_is_readable_after_a_new_service_instance`、`completed_results_and_sessions_survive_a_service_restart`）。
- 重启时非终态任务先降为 `interrupted` 并落库，清空 Session 占位；重复重启仍然稳定。
- 对账：provider 明确确认 `completed` + 完整回答才升级（`reconcile_only_completes_a_provider_confirmed_turn`）；`None` 保持 `interrupted`；确认 `failed` 记为 `failed`。
- 落库失败的完成结果绝不报成功：`a_result_that_cannot_be_persisted_is_never_reported_completed`（任务降级为 `failed` / `storage`）。

## 5. MCP / IPC 自动化

`src-tauri/tests/agent_gateway_mcp_ipc.rs` 8 项通过（真实 IPC + 真实 AgentService + 真实 SQLite + 测试替身 Adapter）：

- `initialize` / `notifications/initialized`（无响应）/ `tools/list` 五个工具 / `tools/call`。
- 长任务不阻塞：`agent_run` 提交 < 800ms；同一连接上可继续查询状态。
- 分页精确重组；事件默认不返回但报告 `events_total` 与 `events_next_offset`。
- 客户端断开重连后用原 `task_id` / `session_id` 查询与续会。
- 取消是真实 provider 中断；取消请求返回的 `cancel_requested: true` 不是终态。
- 业务错误（`not_found`）以 `isError: true` 的工具结果返回并保留 code；参数错误为 `-32602`。
- 未认证连接被拒绝（`unauthorized`）；第二个服务实例以 `busy` 拒绝启动。
- 真实二进制 `mcp-agent` 在 stdin 关闭时 **stdout 为空**、日志只在 stderr。

## 6. 回归

| 项目 | 结果 |
| --- | --- |
| 全库 headless lib | 1415 passed、1 ignored（第一阶段基线 1392 passed、1 ignored） |
| Agent Gateway 单元测试 | 50 passed、1 ignored |
| 第一阶段真实 Codex 集成 | 复跑 PASS，67.29 秒，无回归 |
| 其他 fixture | 均通过 |
| 既有 protocol fixture 失败 | 仍是两项：`metrics_requires_token_only_and_health_does_not`、`unauthenticated_request_is_rejected_before_body_is_read`（与第一阶段记录一致，非本轮引入） |
| `cargo fmt --check` | 通过 |
| headless `cargo clippy --lib -- -D warnings` | 通过 |
| `clippy --lib --tests` | 仅 `cli/serve.rs` 三项既有 lint（852/857/943 行），未扩大范围 |

## 7. 未验证边界

| 项目 | 状态 |
| --- | --- |
| Claude Code 真实推理 | BLOCKED：本机未登录，需 `claude auth login` 后重跑 |
| 强制崩溃（SIGKILL 服务）后的后代进程回收 | 未验证；只验证了正常取消、优雅关闭与客户端异常退出 |
| Windows | 仅实现 Named Pipe 代码路径，未编译、未验收（`first_pipe_instance` 单实例、路径映射、真实 CLI 发现均未验证） |
| 事件级耐久性 | 过程事件按 25 条一批刷盘（终态与状态转换立即写）；崩在批次之间会丢最后不足 25 条事件 |
| CodeLeveler / Pi 端到端 | 本轮不做（按要求留给下一轮）；本轮用独立 MCP 客户端完成验证 |

## 8. 复跑

```bash
# 从仓库根目录执行；真实测试会消耗官方 Codex 使用额度。
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib -- -D warnings
cargo test  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib
cargo test  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --test agent_gateway_mcp_ipc

AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --test agent_gateway_codex_live -- --ignored --nocapture

AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --test agent_gateway_phase_two_live -- --ignored --nocapture --test-threads=1
```

`claude auth login` 之后，`live_claude_via_mcp_stdio_full_lifecycle` 应该直接通过；`live_claude_init_event_reports_only_read_only_tools` 不需要登录。接口细节见 [接口契约](agent-gateway-contract.md)。
