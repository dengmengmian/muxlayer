# Agent Gateway 第二阶段验收记录

macOS（官方 Codex CLI `0.162.1`、官方 Claude Code CLI `2.1.287`）下：SQLite 持久化、Claude Code Adapter、本地 IPC、MCP stdio Server、服务独立生命周期全部实现并通过自动化验收。

**Codex 经 MCP 的真实调用 PASS。Claude Code 经 MCP 的真实调用 PASS**：本机 Claude Code 按 DeepSeek 官方文档接入（`ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic` + `ANTHROPIC_AUTH_TOKEN`，写在用户级 `~/.claude/settings.json` 的 `env`），`init` 事件确认 `model=deepseek-flash[1m]`。

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

测试 `live_claude_init_event_reports_only_read_only_tools`，1 项通过。用生产函数 `read_only_cli_args` 生成的真实 argv（`--tools Read,Grep,Glob --strict-mcp-config --setting-sources user --disable-slash-commands --permission-prompts none`）调用真实 CLI，读取它自己报告的 `init` 事件：

```text
init: session="8709f336-f7d8-43c7-93af-3e1e5b64afeb" tools=["Glob","Grep","Read"] mcp=[]
```

即真实可调用工具只有 `Glob/Grep/Read`，没有任何写文件、执行命令或外部 MCP 工具；`session_id` 等于请求的 UUID。

### 真实推理（PASS）

链路：`真实 MCP 客户端 → mcp-agent → IPC → Agent Service → 真实 Claude Code CLI (2.1.287) → DeepSeek Anthropic 兼容端点`。1 项通过、7.32 秒。

| Case | Task ID | Session ID | Thread ID | Turn ID | State | Answer chars | Events |
| --- | --- | --- | --- | --- | --- | --- | --- |
| run | 3e6141f6-f9d1-482c-a7ac-a62dc3b51cc4 | 8679c9b9-680c-4242-8d97-513d4eb18088 | be96c03b-5f06-4514-b236-09d95cd4ad5e | 38917fa5-4a77-49aa-b9b0-f0bc2664c772 | completed | 49 | 30 |
| continue | 1d84cbce-4c7c-4fd9-a659-64c3b8055e2f | 8679c9b9-680c-4242-8d97-513d4eb18088 | be96c03b-5f06-4514-b236-09d95cd4ad5e | 116a3a4f-9aed-4a3c-ab70-3d02ea745cc4 | completed | 36 | 51 |
| cancel | 0b96bb77-a14f-4f47-ad46-698cd694874a | 14356ffc-a035-4c23-84fb-f6c6342c6c6d | c93c6a09-665e-466b-9b6a-bf737833e2e1 | 6d07ae2a-94ba-4dbb-9b41-9d6b23db47ee | cancelled | 0 | 1 |

- 首次回答带回仓库里的随机 nonce（`SOURCE_NONCE=36efd326-…`）；续会 Task / Turn 不同、Session / Thread 相同，仍返回同一 nonce。
- Turn 的 `init` 事件（来自真实 CLI，服务端记录为过程事件）：

```json
{"api_key_source":"none","mcp_servers":[],"model":"deepseek-flash[1m]","permission_mode":"default","tools":["Glob","Grep","Read"]}
```

- 取消：先等到 `state=running` 且已收到真实 CLI 事件，再发取消；终态 `cancelled`，错误写明「进程被取消请求终止」。这是对进行中 Turn 的真实进程终止，不是排队取消。

### 只读 argv 与 `--restricted` 的实测差异

按 DeepSeek 官方文档把凭据/端点写入用户级 settings 后，实测生产 argv 是否生效：

| argv | model | 结果 |
| --- | --- | --- |
| `--restricted`（原实现，含 `--tools` 白名单） | `claude-opus-5-5`（回退内置默认） | `is_error: true` / `Failed to authenticate` |
| 进程环境变量 + `--restricted` | `deepseek-flash[1m]` | 成功 |
| `--setting-sources user`（现实现） | `deepseek-flash[1m]` | 成功 |

`--restricted` 会连用户级 settings 一起忽略，使第三方端点无法配置；因此改为 `--setting-sources user`：只排除项目 / 本地 settings，用户自己的模型配置照常生效。

项目级 settings 的 hook 边界由自动化测试 `live_claude_excludes_project_settings_hooks`（1 项通过、7.27 秒）守住：先跑对照组证明仓库内 `.claude/settings.json` 的 `UserPromptSubmit` hook 确实会执行，再用生产 argv 验证同一 hook **不执行**。手动探针结果与之一致（`--restricted` 同样不执行）。

| 运行方式 | 仓库内 `.claude/settings.json` 的 hook |
| --- | --- |
| 不限制 setting-sources（对照） | 执行（证明探测有效） |
| 生产 argv（`--setting-sources user`） | **不执行** |
| `--restricted`（对照） | 不执行 |


## 3. Adapter 进程级验证（Claude Code）

`agent_gateway::claude::tests` 8 项通过，全部使用真实子进程（测试替身 CLI，记录 argv / cwd / 会话模式）：

- 生产 argv 不含 `Bash/Edit/Write/NotebookEdit/WebFetch/Task` 与 `--restricted`，含 `--tools Read,Grep,Glob`、`--setting-sources user`、`--strict-mcp-config`、`--disable-slash-commands`、`--permission-prompts none`。
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

## 6. 本机 Claude Code → DeepSeek 配置

按 [DeepSeek 官方 Claude Code 接入文档](https://api-docs.deepseek.com/zh-cn/quick_start/agent_integrations/claude_code) 写入用户级 `~/.claude/settings.json` 的 `env`（文件权限已收紧为 0600，原文件备份为 `settings.json.bak-*`）：

```json
{
  "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
  "ANTHROPIC_AUTH_TOKEN": "<DeepSeek API Key>",
  "ANTHROPIC_MODEL": "deepseek-flash[1m]",
  "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-flash[1m]",
  "ANTHROPIC_DEFAULT_SONNET_MODEL": "deepseek-flash[1m]",
  "ANTHROPIC_DEFAULT_HAIKU_MODEL": "deepseek-flash",
  "CLAUDE_CODE_SUBAGENT_MODEL": "deepseek-flash",
  "CLAUDE_CODE_EFFORT_LEVEL": "max",
  "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "786432"
}
```

- 只放在这一个文件里：终端、GUI 与 Gateway 拉起的 CLI 都读同一份配置，不重复写密钥到 shell rc。
- 密钥不属于仓库内容，没有进入任何提交或文档。
- 实测普通 `claude -p` 正常，`init` 报告 `deepseek-flash[1m]`；`claude auth status` 因此返回 `loggedIn: true`（`authMethod: oauth_token`），Adapter 的认证预检通过。
- 若把 Gateway 拉起在完全没有用户级 settings 的环境里，也可以用 DeepSeek 文档的进程环境变量方式提供同样这几个变量；两种方式 Adapter 都支持（不读取、不输出凭据）。

## 7. 回归

| 项目 | 结果 |
| --- | --- |
| 全库 headless lib | 1415 passed、1 ignored（第一阶段基线 1392 passed、1 ignored） |
| Claude 进程级 Adapter | 8 passed（含 argv 断言已更新为 `--setting-sources user`） |
| Agent Gateway 单元测试 | 50 passed、1 ignored |
| 第一阶段真实 Codex 集成 | 复跑 PASS，67.29 秒，无回归 |
| 其他 fixture | 均通过 |
| 既有 protocol fixture 失败 | 仍是两项：`metrics_requires_token_only_and_health_does_not`、`unauthenticated_request_is_rejected_before_body_is_read`（与第一阶段记录一致，非本轮引入） |
| `cargo fmt --check` | 通过 |
| headless `cargo clippy --lib -- -D warnings` | 通过 |
| `clippy --lib --tests` | 仅 `cli/serve.rs` 三项既有 lint（852/857/943 行），未扩大范围 |

## 8. 未验证边界

| 项目 | 状态 |
| --- | --- |
| Claude 读取范围 | 去掉 `--restricted` 后，读取类工具不再限制在 cwd 内（仍无写入/执行能力） |
| Claude 用户级 hooks / 插件 | 会生效（用户自己的配置；项目级配置仍被排除） |
| 强制崩溃（SIGKILL 服务）后的后代进程回收 | 未验证；只验证了正常取消、优雅关闭与客户端异常退出 |
| Windows | 仅实现 Named Pipe 代码路径，未编译、未验收（`first_pipe_instance` 单实例、路径映射、真实 CLI 发现均未验证） |
| 事件级耐久性 | 过程事件按 25 条一批刷盘（终态与状态转换立即写）；崩在批次之间会丢最后不足 25 条事件 |
| CodeLeveler / Pi 端到端 | 本轮不做（按要求留给下一轮）；本轮用独立 MCP 客户端完成验证 |

## 9. 复跑

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

`live_claude_via_mcp_stdio_full_lifecycle` 需要本机 Claude Code 有可用认证：官方登录，或按第 6 节配好的第三方端点。

接口细节见 [接口契约](agent-gateway-contract.md)。
