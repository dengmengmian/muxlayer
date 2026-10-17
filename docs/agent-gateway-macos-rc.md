# Agent Gateway macOS Release Candidate 验收

验收日期：2026-10-17。平台：macOS 26.6.2，arm64。起始 HEAD：`4a488f7`。

## 0. 最终判定

**macOS RELEASE QUALIFICATION：CONDITIONAL_PASS。**

Agent Gateway 的正式交付物（headless `agentgate-serve`）已经在非源码目录完成真实安装、真实 MCP 调用、真实 Codex / Claude Code 执行、连续会话、取消、重启恢复和 A–H 生命周期验收。本轮发现并修复了两个真实交付问题：

1. MCP 自动拉起的 Agent Service 与 MCP 客户端处于同一进程组，终端关闭的 `SIGHUP` 或 Ctrl+C 的 `SIGINT` 会连带杀死 Service，正在执行的任务随之丢失。
2. `agent_status` / `agent_cancel` 返回的 TaskRecord 自带 `error` 字段时，MCP 把一次成功的状态查询误标为 `isError: true`，Claude 取消后的状态轮询会被客户端当成失败。

仍有三项条件/限制，因此不是无条件 PASS：

1. 本机已安装的 CodeLeveler `1.0.16` 不包含 `isError` 传播修复；用当前 CodeLeveler 源码构建的 `1.0.18` 才通过。普通用户需要升级 CodeLeveler，AgentGate 本身的行为是正确的。
2. 在任务从 `queued` 进入 `running`、provider 刚返回 turn ID 的极窄窗口内，`agent_cancel` 可能收到 Codex `turn/interrupt` 的 `-32600`，需要重试一次；重试后真实取消成功，系统没有伪造终态。证据见 [cancel-race.json](macos-rc-evidence/cancel-race.json)。
3. Windows 未做原生编译与运行验收；macOS 本地 RC 产物未签名、未公证，不等于正式公开发布。

完整证据目录：[docs/macos-rc-evidence/](macos-rc-evidence/)。

## 1. 交付方式

MuxLayer 当前同时发布两种 macOS 产物：

| 产物 | 发布位置 | 用途 |
| --- | --- | --- |
| `MuxLayer_<version>_<arch>.dmg` | GitHub Release / Homebrew cask | 桌面应用（Model Gateway UI） |
| `agentgate-serve-<arch>.tar.gz` | GitHub Release 的独立 `cli` job | Headless CLI，Agent Gateway 的实际交付入口 |

`src-tauri/tauri.conf.json` 没有 `externalBin`；桌面 `.app` 不内置 `agentgate-serve`。`release.yml` 的 `cli` job 单独构建并上传四个平台的 CLI 包，这是现有发行契约，不是本轮遗漏。Agent Gateway 的普通使用方式是下载对应架构的 CLI tarball，而不是依赖 Desktop 安装包。若产品希望“安装 Desktop 即自带 Gateway”，需要单独的产品打包决策，本轮没有擅自改造安装体系。

本轮验收使用从当前源码构建的正式 release 二进制：

```text
src-tauri/target/release/agentgate-serve
Mach-O 64-bit executable arm64
agentgate 2.0.8
SHA256: e757121da74fc95a7382c2f6cc5c5b1bfb7e81c8e9800f916727d8a12b51f029
```

安装到隔离目录后从 `/tmp` 运行，不依赖 `cargo run`、项目源码目录或 `target` 路径：

```bash
mkdir -p /tmp/muxlayer-rc/install/bin
cp src-tauri/target/release/agentgate-serve /tmp/muxlayer-rc/install/bin/
/tmp/muxlayer-rc/install/bin/agentgate-serve --version
```

## 2. 启动与 MCP 注册

真实有效的命令：

```bash
agentgate-serve agent-service [--db-path DIR] [--idle-timeout N]
agentgate-serve mcp-agent   [--db-path DIR]
```

`mcp-agent` 在 Service 不在线时会用当前可执行文件自动拉起 `agent-service`，等待 socket 就绪后连接。自动拉起的 Service 现在使用独立进程组（`process_group(0)`），终端关闭或 Ctrl+C 只影响前台 MCP 客户端，不会杀死 Service 和正在执行的任务；本轮新增的 `hangup` 场景验证了这一点。

MCP 客户端注册示例：

```json
{
  "mcpServers": {
    "muxlayer": {
      "command": "/absolute/path/to/agentgate-serve",
      "args": ["mcp-agent"]
    }
  }
}
```

隔离验收时可追加 `"--db-path", "/absolute/tmp/db"`。数据目录默认按 `MUXLAYER_DB_PATH` / `AGENTGATE_DB_PATH`，否则使用 `$HOME` 下的 CLI 数据目录；SQLite 文件为 `agentgate.db`，Service 的 socket/token/log 位于 `<db_dir>/agent-service/`。

Agent CLI 发现顺序：

| Agent | 显式覆盖 | 默认 |
| --- | --- | --- |
| Codex | `AGENTGATE_CODEX_BIN` | 遍历 `PATH`，跳过 `.app/Contents/`，执行 `codex --version` 且输出以 `codex-cli ` 开头 |
| Claude Code | `AGENTGATE_CLAUDE_BIN` | 遍历 `PATH`，跳过 `.app/Contents/`，执行 `claude --version` 且输出包含 `Claude Code` |

本机 PATH 中 `codex` 首先命中一个缺少 vendor 二进制的 npm wrapper；发现逻辑会因 `--version` 失败继续向后找到 `/opt/homebrew/bin/codex`，真实验收确认最终使用的是官方 `codex-cli 0.162.1`。如果 MCP 客户端由 GUI 启动、PATH 被限制为 `/usr/bin:/bin`，需要显式设置两个 `AGENTGATE_*_BIN`；本轮没有把 GUI 启动器纳入验收。

## 3. 真实 Codex 验收

本机官方 CLI：`/opt/homebrew/bin/codex`，`codex-cli 0.162.1`。直接读取官方 app-server 模型目录得到默认模型 `gpt-6.1-sol`；用户配置没有自定义 `model_provider` / `base_url`。Adapter 使用随机命名的原生 provider（`requires_openai_auth=true`、无 `base_url`、无附加认证头），不经过 MuxLayer Model Gateway。

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| 新 Session、真实读源码、完整回答 | PASS | task `fe1d7101-779d-4d4d-87ca-f724a102ef6e`，session `df484d29-ea69-45c3-ac7b-e5f3d251f75e`，thread `01a149b8-f73d-7962-bfbe-0811a418d04f`，36 字符，包含 `中文🙂` |
| 原 Session / Thread 续会 | PASS | task `180a4425-258e-43a0-8362-3cd04d9b4492`，thread 与首次相同 |
| 显式分页重组 | PASS | `final_answer_limit=3` 逐页重组，`final_answer_total_chars=36`，与 SQLite / 完整回答逐字相等 |
| 只读强制 | PASS | 要求执行 `printf HACKED > fixture.txt && touch created.txt`；返回 `zsh:1: operation not permitted: fixture.txt`，exit 1；原文件未变、`created.txt` 不存在、`git status --porcelain` 为空 |
| 真实长任务 / 取消 | PASS | 运行中取消，终态 `cancelled`；取消前捕获 4 个进程实例，25 秒后 survivors 为 0 |
| MCP 客户端断开 | PASS | 客户端退出后任务仍 `running`；新客户端按原 task_id 查询并取消 |
| Service 重启后读回 | PASS | `service_shutdown` 后重启，原 task 仍 `completed` 且 `final_answer` 完全一致；原 Session 续会返回 `PERSISTED`，thread 不变 |
| SQLite 唯一真相源 | PASS | `agent_tasks.status=completed`，`final_answer` 与 MCP 返回逐字一致，`completed_at` 非空；`agent_sessions.active_task_id` 为空 |
| 失败传播 | PASS | 不存在 cwd → `isError=true` / `invalid_request`；未知 agent → `unknown_agent`；`permission=execute` 在 MCP 参数层返回 JSON-RPC `-32602`（与契约“仅开放 read_only”一致） |

证据：[codex.json](macos-rc-evidence/codex.json)。

## 4. 真实 Claude Code 验收

CLI：`/Users/mengmian/.local/bin/claude`，`2.1.302 (Claude Code)`。`claude auth status` 显示 `loggedIn:true`、`authMethod:oauth_token`、`apiProvider:firstParty`；但本机用户级 settings 通过 `env` 将 Anthropic 端点指向 DeepSeek，实际模型是 **`deepseek-flash[1m]`**，不是 Anthropic 官方模型。本轮没有修改任何认证或模型配置，也没有读取/输出凭据值。

真实 `init` 事件（Task 过程事件原文）：

```json
{"api_key_source":"none","mcp_servers":[],"model":"deepseek-flash[1m]",
 "permission_mode":"default","tools":["Glob","Grep","Read"]}
```

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| 真实启动与工具白名单 | PASS | 只有 `Glob` / `Grep` / `Read`；`mcp_servers=[]`；无 Bash / Edit / Write / NotebookEdit / WebFetch / Task |
| 真实读源码、完整结果 | PASS | task `f6bc8e2e-b8f4-44ff-875a-a3a5cc00a971`，session `cbf18a81-2895-461f-aace-769bd8663442`，thread `a11b30a6-1351-4845-be38-0089ed592e01` |
| 原 Session / Thread 续会 | PASS | task `6e355181-60e3-4c8f-a16d-9219674d47b7`，thread 相同，nonce 一致 |
| 真实取消 | PASS | 运行中取消，终态 `cancelled`；捕获 2 个进程实例，survivors 为 0 |
| 写入尝试 | PASS | 模型明确报告 `Write is disabled for this session`；文件未创建，git 状态为空 |
| Service 重启后读回 | PASS | 完整回答逐字一致；原 Session 续会返回 `PERSISTED`，thread 不变 |
| cwd 外读取 | 观察到被 CLI 拒绝 | 对绝对路径的 cwd 外文件读取未返回内容（`outside_cwd_file_read=false`）。Adapter 本身不限制读取目录；这是 Claude Code 在 `--permission-prompts none` 下的 CLI 权限行为。**“禁止写入”和“限制读取目录”是两种能力，不能混为一谈。** |
| 项目级 hook 排除 | PASS | 生产 argv 下仓库 `.claude/settings.json` hook 不执行；对照组证明探针有效 |

证据：[claude.json](macos-rc-evidence/claude.json)；真实 CLI 探针输出见回归记录。

## 5. CodeLeveler 真实验收

本机安装版：`/opt/homebrew/bin/leveler`，`1.0.16 (693dcc1d6673-dirty) UNTRUSTED`。当前源码构建：`1.0.18 (e58c259a2382-dirty)`（包含 CodeLeveler `e4510fa` 的 MCP `isError` 修复）。

使用临时全局注册 `muxlayer-rc`，隔离数据库 `/tmp/.../leveler-db`；测试结束后移除注册，`~/.leveler/config.toml` 的 SHA256 与测试前完全一致（`a8c95199...`）。

headless `leveler run` 默认会对 MCP 网络动作发起审批；首次不带 `--auto-approve` 的真实运行被默认拒绝（说明安全默认未被关闭）。后续使用 `--auto-approve` 仅作为本次运行的显式批准，不修改 CodeLeveler 全局权限或 Kernel。

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| MCP 注册 / 发现五个工具 | PASS | `leveler mcp list` → connected, 5 tools |
| `agent_list` / `agent_run` | PASS | task `d851e9f8-80fd-41a3-8c75-483a3c222177`，session `7190d883-642d-43a4-9a54-9c13d04cf3a9`，thread `01a149ba-415b-7ac2-80c5-39d8fef44967`，546 字符 |
| `agent_continue` 原 Thread | PASS | task `fb3deb19-0169-4547-a379-9084bacd8391`，session/thread 不变，1056 字符 |
| 显式 `agent_cancel` | PASS | task `c36f7509-e9a0-4e67-beec-6924ff53225d`，`cancel_requested=1`，终态 `cancelled` |
| `agent_run` 快速返回 / 30 秒预算 | PASS | 提交立即返回 task_id；长任务通过短轮询完成，没有把推理塞进单次 MCP 调用 |
| MCP `isError`（安装版 1.0.16） | **FAIL** | 不存在的 task 返回 `{"error":{"code":"not_found"}}`，但 CodeLeveler `tool_result.is_error=false`；安装版二进制不含 `isError` 处理 |
| MCP `isError`（当前构建 1.0.18） | PASS | 同一调用 `tool_result.is_error=true`，preview 为 `MCP 调用失败:{"error":{"code":"not_found"...}}` |

结论：AgentGate 的 MCP 结果正确；安装版 CodeLeveler 过旧是客户端交付条件。普通用户若使用 CodeLeveler 1.0.16，会把 MCP 业务错误当成功，必须在 CodeLeveler 发布包含 `e4510fa` 的版本后才满足该项。

取消语义：本轮验证的是显式 `agent_cancel`。用户取消当前 CodeLeveler 对话、或退出客户端，**不会**被自动映射为持久化任务取消；这与现有契约一致，本轮没有把两者混为一谈。

证据：[clients.json](macos-rc-evidence/clients.json)。

## 6. Pi Agent 真实验收

Pi：`1.3.0`，外层模型 `taotoken/deepseek-flash`。使用临时全局 MCP 注册，隔离数据库 `/tmp/.../pi-db`；测试结束后移除注册，`~/.pi/agent/mcp.json` 的 SHA256 与测试前完全一致（`d8e397af...`）。

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| MCP 工具发现 | PASS | `pi mcp list` → connected, 5 tools |
| Codex 真实调用 | PASS | task `3b819d5b-58d0-4d26-a841-e1bb5ae2d1c9`，session `b0d66f43-fdfd-4149-85f0-7f84cee87ac1`，thread `01a149c4-521a-7c13-b8cf-a52b0f0dfe79`，439 字符 |
| Codex 连续会话 | PASS | task `2feb4e67-cd2f-4cf6-9fd5-b91e74616683`，session/thread 不变，1127 字符 |
| Claude Code 真实调用 | PASS | task `5d589c99-09b0-4fa9-b516-46feb2261222`，session `313f145e-d7d7-4d4e-b118-4067bed80442`，thread `3017986a-0ca5-4775-8a63-fcafe9f9b302`，nonce 一致 |
| Claude Code 连续会话 | PASS | task `c6f15464-16fa-47af-a058-72d88926cde9`，session/thread 不变，nonce 一致 |
| Unicode 分页 | PASS | `final_answer_limit=100`，offset `0/100/200/300/400` 共 5 页，拼接与服务端 `final_answer_total_chars=439` 完全一致，emoji 保留 |
| 显式取消 | PASS | task `a74688c1-b8c0-49cf-9f6f-02a00ca4a976`，`cancel_requested=1`，终态 `cancelled` |
| 客户端退出 | PASS | Pi 进程被杀后 task `4f13a19a-283d-4e18-a9fc-277e0465b094` 仍 `running`；新 MCP 客户端查询并取消成功 |
| 重新连接 | PASS | 新 Pi 进程按原 task_id 读回 `cancelled` 及原 session/thread |
| MCP 错误传播 | PASS | `agent_status(does-not-exist)` 的 `tool_execution_end.isError=true`，`details.error=tool_error`，code/message 保留 |

Pi 在 headless 模式下的两次长时间无输出，定位为 Pi 客户端等待 stdin/审批，不是 Gateway 卡死：使用 `</dev/null` 关闭 stdin 后同一提示稳定完成。该行为属于 Pi 自身，本轮没有关闭 Pi 的安全设置。另有启动/模型延迟波动，已记录为环境噪声，不据此宣称 Gateway 失败。

证据：[clients.json](macos-rc-evidence/clients.json)。

## 7. Task / Session 持久化与完整结果

- SQLite 是唯一持久化真相源：`agent_sessions` / `agent_tasks` 写穿；完成结果、Thread/Turn 映射、取消意图、终态都在事务中落库。
- 服务重启后，已完成任务读回与 MCP 原始回答逐字一致；非终态任务先降级 `interrupted`，Codex 通过官方 `thread/read` 只读对账，只有真实完成且回答完整才升级；绝不重新执行。
- 分页按 Unicode scalar value 切片；服务端总长度、下一页 offset、完成标记齐全；不同分页重组与原文完全一致。
- 既有 33 MiB Unicode 大结果测试通过：完整结果持久化，超 IPC 上限时返回结构化 `invalid_request`，随后分页逐字重组。
- 同 Session 并发提交：活动任务存在时 `agent_continue` 返回 `busy`；重复启动 Service 被 OS 文件锁拒绝，且不会改写现任任务。
- mutation 响应丢失：不自动重放，明确报告 outcome unknown；本轮没有新增幂等提交协议。
- `agent_status` / `agent_cancel` 返回的 TaskRecord `error` 是任务数据，不是工具失败；本轮修复后，失败/取消/中断任务仍可被客户端正常读取。

## 8. 取消与恢复

| 场景 | 结果 |
| --- | --- |
| Codex 运行中取消 | PASS，真实 `turn/interrupt`，终态 `cancelled`，无残留 |
| Claude 运行中取消 | PASS，终止真实 CLI 进程，终态 `cancelled`，无残留 |
| Pi 显式取消 | PASS，终态 `cancelled` |
| CodeLeveler 显式取消 | PASS，终态 `cancelled` |
| MCP 客户端断开 | PASS，任务继续；重连可查 |
| Service 正常退出 / SIGKILL | PASS，A–H；任务 `interrupted`，原 Thread 可续会 |
| 取消后清理失败 | 既有回归覆盖：不复用未确认清理的 Session，错误保留 |
| turn-start 窗口取消竞态 | **已知限制**：task 刚进入 running 时首次 `agent_cancel` 可能收到 `turn/interrupt -32600`；不伪造终态，重试一次后真实取消成功。见 [cancel-race.json](macos-rc-evidence/cancel-race.json) |

## 9. A–H 生命周期回归

使用 release 二进制、真实官方 Codex、隔离数据库与临时 Git 仓库。每一项都独立检查真实 PID / PGID / SID / kernel unique ID；每次同时保留无关的独立 Session Shell 与真实 Codex App Server 作为 sentinel。

| 场景 | 结果 | Task 终态 | Survivors | Sentinels |
| --- | --- | --- | --- | --- |
| A 正常取消 | PASS | cancelled | 0 | alive |
| B App Server 正常退出 | PASS | — | 0 | alive |
| C App Server SIGKILL | PASS | interrupted | 0 | alive |
| D Service 正常退出 | PASS | interrupted | 0 | alive |
| E Service SIGKILL | PASS | interrupted | 0 | alive |
| F MCP Client 断开 | PASS | cancelled（重连后显式取消） | 0 | alive |
| G 深层独立 Session Shell | PASS | interrupted | 0 | alive |
| H 多并发 Session | PASS | interrupted | 0 | alive |

矩阵没有按进程名批杀、没有猜 PID、没有误杀 sentinel。证据：[lifecycle.json](macos-rc-evidence/lifecycle.json)。

## 10. 出生 Gate 专项

本轮没有重写既有机制，只做验证：

- Owner 在 root `exec` 前建立独立 PGID、读取 root 内核 unique ID、注册 kqueue 退出事件，然后放行；身份登记早于执行。
- 归属判断使用内核实例身份与 audit token，不使用进程名 / PID 文本 / PGID。
- A–H 中实际杀 App Server、杀 Service、杀 owner 生命周期 FD、取消任务，均未观察到未登记后代逃逸；每次退出后 `survivors` 为空。
- 新增的 Service 独立进程组只保护 Service 不受终端组信号影响，不改变 owner 的谱系判断；`hangup` 场景验证 SIGHUP 后 Service、任务和后续取消均正常。
- 保证范围仍是 **PARTIAL**：首次发现前中间祖先已退出、孤儿已再次 `exec`、owner 自身被 SIGKILL、macOS 私有 ABI/权限变化，都可能丢失可证明的身份链。本轮没有把有限测试包装成“任意进程树绝对可回收”。

## 11. 安全与只读边界

| 项 | 结果 |
| --- | --- |
| Codex 只读 | PASS：真实 shell 写入被沙箱拒绝，文件与 git 状态不变 |
| Claude Code 只读 | PASS：真实 init 只有 `Glob/Grep/Read`，无写入/命令工具，MCP inventory 为空 |
| 项目级配置注入 | PASS：生产 argv `--setting-sources user`，仓库 hook 不执行 |
| 用户级配置 | 信任边界保持：用户 settings 中的模型端点/hook 属于机器主人配置，与凭据同信任级 |
| cwd 外读取 | 观察到被 CLI 拒绝；Adapter 本身不限制；与“禁止写入”分开记录 |
| 凭据 | 未读取、未输出、未复制；未记录 token/API key；测试配置已恢复原字节 |
| MCP stdout | 空输入时 stdout 为 0 字节；日志只写 stderr |
| 文件权限 | SQLite 0600；Service socket 0600、目录 0700；token 0600 |
| 取消清理 | 取消发布前确认整个 Adapter scope 关闭；清理失败保留错误并阻止续会 |

## 12. 原有回归

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` / `git diff --check` | PASS |
| 版本一致性 | PASS（2.0.8） |
| Agent Gateway 单元测试 | 75 passed / 0 failed / 3 ignored |
| 全库 headless lib | 1440 passed / 0 failed / 3 ignored |
| MCP / IPC 集成 | 9 passed / 0 failed（含 33 MiB 分页） |
| CLI 所有权回归 | 1 passed |
| 真实 Codex + Claude live | 4 passed（50.68s） |
| 真实 Codex library live | 1 passed（64.27s） |
| `protocol_fixture` | 38 passed / 2 failed，与既有基线完全一致 |
| clippy `--lib -D warnings` | PASS |
| clippy `--lib --tests -D warnings` | 仍只有三项既有 CLI lint：`print_literal`、`unnecessary_map_or`、`too_many_arguments` |
| A–H 生命周期 | 8/8 PASS |

两项 protocol fixture 失败和三项 CLI lint 是 Known Baseline Issues，本轮没有修改、没有改成 ignored、没有扩大范围修复。完整命令与结果：[regression.json](macos-rc-evidence/regression.json)。

## 13. 已知限制与发布条件

1. **CodeLeveler 版本条件**：安装版 `1.0.16` 不识别 MCP `isError`；必须升级到包含 CodeLeveler `e4510fa` 的版本。当前源码构建 `1.0.18` 已通过。
2. **取消竞态**：turn-start 窗口首次取消可能返回 `turn/interrupt -32600`；重试后成功。影响窗口极窄，不伪造成功，但属于未收口的可靠性边界。
3. **Windows**：未做原生编译、Named Pipe 权限、进程所有权与端到端验收；不得宣称跨平台全面发布就绪。
4. **签名与公证**：本轮是本地 release 构建，未签名、未公证、未发版；正式公开发布仍需 Apple Developer 签名/公证与 Release 流水线。
5. **进程回收保证**：任意脱链后代仍是 PARTIAL；不能把 A–H 的 PASS 解读为绝对全树保证。
6. **桌面安装包不含 CLI**：Agent Gateway 当前通过独立 `agentgate-serve` tarball 交付；这是现有发行契约，不是本轮新增缺陷。
7. **GUI 启动 PATH**：GUI MCP 客户端可能只继承受限 PATH；需要 `AGENTGATE_CODEX_BIN` / `AGENTGATE_CLAUDE_BIN`。本轮只在终端环境验证。
8. **Pi headless stdin**：Pi 在无 TTY 时可能等待 stdin/审批；关闭 stdin 后可稳定运行。该行为不由 Gateway 控制。
9. **测试事件（必须披露）**：本轮在清理 Pi 测试进程时，使用过宽的 `ps` 过滤误向一个不属于本轮测试树的长期 Pi 进程（PID 95639，已运行约 16 天）发送了 SIGTERM。该进程被终止，未修改任何文件或配置。之后所有清理改为只使用本轮 Popen 返回的精确 PID。该事件不改变 Gateway 验收结果，但属于测试操作事故，必须记录。

## 14. 发布门禁

| Gate | 状态 | 执行方式 | 证据 | 阻塞发布 |
| --- | --- | --- | --- | --- |
| BUILD | PASS | release 构建 `agentgate-serve` | Mach-O arm64，`agentgate 2.0.8` | 否 |
| INSTALL | PASS | 隔离目录 + 非源码 cwd + 真实二进制 | [install.json](macos-rc-evidence/install.json) | 否 |
| MCP | PASS | 真实 stdio initialize / tools/list / 五工具 | [install.json](macos-rc-evidence/install.json) | 否 |
| CODEX_NATIVE | PASS | 官方 CLI 0.162.1，真实推理 | [codex.json](macos-rc-evidence/codex.json) | 否 |
| CLAUDE_CODE | PASS | 官方 CLI 2.1.302，真实推理；模型为 DeepSeek | [claude.json](macos-rc-evidence/claude.json) | 否 |
| SESSION | PASS | Codex / Claude / CodeLeveler / Pi 连续会话 | 各证据文件 | 否 |
| PERSISTENCE | PASS | SQLite 终态、完整回答、重启读回 | [codex.json](macos-rc-evidence/codex.json)、[claude.json](macos-rc-evidence/claude.json) | 否 |
| RECOVERY | PASS | Service 重启、interrupted 对账、原 Thread 续会 | A–H、codex/claude | 否 |
| CANCEL | PASS（附已知竞态） | 四种客户端 + A–H 真实取消 | A–H、clients | 否 |
| PROCESS_LIFECYCLE | PASS | A–H，内核身份与 sentinel | [lifecycle.json](macos-rc-evidence/lifecycle.json) | 否 |
| READ_ONLY | PASS | Codex 沙箱拒绝写；Claude init 白名单 | codex/claude | 否 |
| SECURITY | PASS（边界已注明） | 配置注入、凭据、stdout、权限 | 见第 11 节 | 否 |
| REGRESSION | PASS | lib / 集成 / live / fixture / clippy | [regression.json](macos-rc-evidence/regression.json) | 否 |
| CODEXLEVELER_ISERROR（当前构建 1.0.18） | PASS | 含 `e4510fa` 的 CodeLeveler 构建 | [clients.json](macos-rc-evidence/clients.json) | 否 |
| CODEXLEVELER_ISERROR（安装版 1.0.16） | FAIL | 本机已安装的真实客户端版本 | [clients.json](macos-rc-evidence/clients.json) | 是（需升级客户端） |
| PI | PASS | 真实 Pi 1.3.0 全链路 | [clients.json](macos-rc-evidence/clients.json) | 否 |
| WINDOWS | NOT_TESTED | 无原生环境 | — | 否（不属本轮 macOS RC） |

## 15. 复跑

```bash
# 构建与安装
cargo build --manifest-path src-tauri/Cargo.toml --release \
  --no-default-features --features cli --bin agentgate-serve
mkdir -p /tmp/muxlayer-rc/install/bin
cp src-tauri/target/release/agentgate-serve /tmp/muxlayer-rc/install/bin/

# 安装 / 终端组 / Codex / Claude 真实验收
python3 scripts/verify-agent-gateway-rc.py \
  --binary /tmp/muxlayer-rc/install/bin/agentgate-serve \
  --output /tmp/muxlayer-rc/evidence-install --scenario install
python3 scripts/verify-agent-gateway-rc.py \
  --binary /tmp/muxlayer-rc/install/bin/agentgate-serve \
  --output /tmp/muxlayer-rc/evidence-hangup --scenario hangup
python3 scripts/verify-agent-gateway-rc.py \
  --binary /tmp/muxlayer-rc/install/bin/agentgate-serve \
  --output /tmp/muxlayer-rc/evidence-codex --scenario codex
python3 scripts/verify-agent-gateway-rc.py \
  --binary /tmp/muxlayer-rc/install/bin/agentgate-serve \
  --output /tmp/muxlayer-rc/evidence-claude --scenario claude

# A–H 生命周期
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex \
python3 scripts/verify-agent-gateway-lifecycle.py \
  --binary /tmp/muxlayer-rc/install/bin/agentgate-serve \
  --codex /opt/homebrew/bin/codex --output /tmp/muxlayer-rc/lifecycle
```

真实测试会消耗本机 Codex / Claude Code 额度。所有测试使用隔离数据库与临时目录；测试结束后 CodeLeveler / Pi 的临时 MCP 注册已移除，原配置文件 SHA256 与测试前一致。

## 16. 最终提交

- 起始 HEAD：`4a488f7`
- 最终提交：本文档随本轮最终提交落库，具体 SHA 见最终回复与 `git log -1`。
- 本轮代码修复：
  - `src-tauri/src/agent_gateway/ipc.rs`：自动拉起的 Service 使用独立进程组。
  - `src-tauri/src/agent_gateway/mcp.rs`：TaskRecord `error` 不再把 `agent_status` / `agent_cancel` 误标为工具失败；新增回归测试。
- 本轮验收资产：`scripts/verify-agent-gateway-rc.py`、`docs/agent-gateway-macos-rc.md`、`docs/macos-rc-evidence/`。
- 未 push、未发版、未创建 tag、未切换分支、未修改 Windows 实现。
