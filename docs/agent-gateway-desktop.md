# Agent Gateway Desktop 使用说明与验收记录

实现提交：

- Phase 5：`43c4e888519563f2151e9a303c4493f98b817c5a`
- Phase 6：`5259efa51bb9ed8e195c475fb6dab653af030271`

本阶段只增加 Desktop 配置与可视化层，未改动 Model Gateway、Codex / Claude Adapter、Agent Service 任务语义或 MCP 协议。

## 页面结构

侧边栏「工具 → Agent Gateway」新增一个页面，复用现有 React / Tauri / 设计系统，不引入新框架。

| Tab | 内容 |
| --- | --- |
| Agents | Codex、Claude Code 的真实检测状态、版本、路径、认证、模型来源、模型、配置文件、发现方式、错误；配置路径；测试连接 |
| Connections | CodeLeveler、Pi Agent 的安装、配置、命令、兼容性；配置 / 移除 MCP；复制 TOML / JSON；自定义 MCP 客户端配置 |
| Settings | Agent Service 运行状态、版本、CLI、数据目录、运行任务、最近错误、启停重启、打开数据目录、复制诊断信息、Agent Gateway CLI 路径 |

页面标题与侧边栏走既有路由 / i18n，`/agent-gateway` 单独懒加载。

## Agent 检测

检测全部在 Rust 后端完成，普通状态检查只运行 `--version`、读取配置、检查认证状态，不调用模型。

CLI 候选顺序：

1. 用户在 UI 中保存的绝对路径（唯一候选，不再回退）
2. 环境变量：`AGENTGATE_CODEX_BIN` / `AGENTGATE_CLAUDE_BIN`
3. 进程 `PATH`
4. 常见安装目录：Homebrew、`/usr/local/bin`、`~/.local/bin`、`~/.cargo/bin`、pnpm、nvm、volta、asdf 等

`AGENTGATE_CODEX_BIN` / `AGENTGATE_CLAUDE_BIN` 仍然有效，但不是唯一入口；GUI 启动时 PATH 受限，因此增加常见目录回退。`.app/Contents/` 内的路径会被跳过。

Agent 状态不把「找到文件」等同于「可用」，分别展示：

- 未安装
- 已安装但 `--version` 失败 / 不可执行
- 已安装但未配置认证
- 配置有效、可用
- 认证无法确认（Codex 显示「需验证」，仍允许用户发起真实测试连接）
- 测试连接失败
- 未知状态

MCP 客户端同样区分「未安装」「已安装但不可执行（客户端不可用）」「未配置」「已配置」「需要更新」「同名冲突」。不可执行状态不会显示成「未安装」。

Codex 认证读取 `codex login status`；模型来源和模型读取现有 Codex 配置检测，不写死版本或模型。

Claude Code 读取 `claude auth status` 和 `~/.claude/settings.json` 中已有的 base URL / model / env 名称。`loggedIn:false` 不会直接判定不可用：如果存在第三方 Anthropic 兼容 token / base URL，状态显示为「第三方 token 已配置」，模型来源按配置显示为 DeepSeek 或其它第三方，不会标成 Anthropic 官方。

## Agent 配置与持久化

| 数据 | 存储位置 |
| --- | --- |
| Codex / Claude 可执行路径 | CLI 数据目录的 SQLite `app_settings`（默认 `~/.muxlayer`，兼容 `~/.agentgate`） |
| Agent Gateway CLI 路径 | Desktop 自己的 SQLite `app_settings` |
| Service 状态、任务 | 由 Agent Service 持有并通过 IPC 查询 |

Desktop 和 headless `agentgate-serve agent-service` 读取同一份 Codex / Claude 路径设置。`agent-service` 启动时会先加载该设置，再做自动发现；有集成测试覆盖「保存路径后 headless service 使用该路径」。

手动路径必须是绝对路径且通过版本校验；不存在或不可执行的路径会在保存时返回错误，不会覆盖旧值。清空路径恢复自动检测。

不会修改 `~/.codex/auth.json`、`~/.codex/config.toml` 或 Claude 认证文件。UI 只读展示配置，不显示 API Key / Token。

### Node / nvm 客户端

Pi 通过 nvm 安装时，`pi` 是指向 JS 文件的软链，shebang 为 `#!/usr/bin/env node`；Finder 启动的 Desktop 没有 nvm 的 PATH，直接执行会得到 `env: node: No such file or directory`。

修复方式只作用于 Desktop 启动的子进程：执行客户端 CLI 时，把该 CLI 所在目录临时加到子进程 `PATH` 的最前面。nvm 的 `pi` 与 `node` 在同一 `bin` 目录，因此无需修改用户全局 PATH、nvm 配置或 Shell 文件。该处理覆盖 `pi --version`、`pi mcp add/remove`，不改变 MCP 注册的命令或参数。

## MCP 接入

### CodeLeveler

- 配置文件：`$LEVELER_HOME/config.toml`，默认 `~/.leveler/config.toml`
- 格式：`[[mcp_servers]]`，含 `name`、`command`、`args`
- 配置 / 移除通过官方 `leveler mcp add/remove` 执行
- `leveler 1.0.16 / 1.0.17` 缺少 `e4510fa` 的 MCP `isError` 修复，页面明确提示升级；Gateway 不修改 MCP 协议来掩盖旧客户端缺陷

### Pi Agent

- 配置文件：`~/.pi/agent/mcp.json`
- 格式：`mcpServers.<name>.{command,args}`
- 配置 / 移除通过官方 `pi mcp add/remove` 执行
- 保留 Pi 自身的信任 / 权限机制，不修改 Pi 源码

### 自定义 MCP 客户端

页面提供只读 JSON 和命令复制，内容只有 `muxlayer`、`mcp-agent`、CLI 绝对路径，不包含任何 Token。

## 一键配置与撤销安全

1. 用户显式点击配置，并二次确认
2. 检查客户端是否安装、MCP 条目是否已存在、是否为托管条目
3. 同名条目若不是 `agentgate-serve mcp-agent`：拒绝覆盖，提示用户先在客户端处理
4. 如果已经是当前 CLI 的配置：幂等返回「已配置」，不重复写
5. 在 `0700` 临时目录创建 staging 配置，复制原配置；通过官方客户端 CLI 修改 staging 副本：
   - CodeLeveler：`LEVELER_HOME` 指向 staging
   - Pi：`PI_CODING_AGENT_DIR` 指向 staging
6. 重新读取 staging 配置，校验命令与参数；失败时真实配置完全未动
7. 写入真实配置前再次读取，若被外部进程修改则拒绝覆盖并提示重试
8. 使用同目录临时文件 + `fsync` + 原子 rename 安装；已有文件保留原权限，新文件使用 `0600`
9. 写入后再次读取校验；失败时恢复原内容
10. 移除只删除托管条目；冲突条目拒绝移除

真实客户端配置只在最后一步被原子替换，客户端 CLI 自身即使非原子写入，也只影响 staging 副本，不会留下半写入的真实配置。

如果已有托管条目指向旧的 `agentgate-serve` 路径，页面显示「需要更新」，允许直接重新配置到当前 CLI，或移除该托管条目；这不会把旧路径误判成他人的 MCP 条目。

「已配置」只表示配置文件存在本应用条目，页面明确说明它不是在线状态。

## Agent Service 状态

- 通过现有 IPC Unix socket / named pipe 查询，不新建第二套生命周期控制
- 展示：运行中 / 已停止 / 错误、Service 版本、CLI、数据目录、运行任务、最近错误
- 启动：复用 `spawn_detached_service`，等待 service ready
- 停止 / 重启：先刷新最新任务状态；有运行任务时在确认框中显示任务数，用户确认后才中断
- 不删除数据库、Socket 或 Lock 文件
- 残留 Socket 文件但无进程监听时显示「已停止」，不再显示连接错误
- 最近一次错误只展示日志级别和 message，不展示 r2d2 / 文件路径等内部元数据

## Tauri commands

新增命令均位于 `src-tauri/src/app/commands/agent_gateway.rs`：

- `get_agent_gateway_overview`
- `get_agent_gateway_service_status`
- `set_agent_gateway_agent_path`
- `set_agent_gateway_serve_path`
- `test_agent_gateway_agent`
- `start_agent_gateway_service`
- `stop_agent_gateway_service`
- `restart_agent_gateway_service`
- `configure_agent_gateway_mcp`
- `remove_agent_gateway_mcp`
- `get_agent_gateway_mcp_copy`
- `get_agent_gateway_diagnostics`
- `open_agent_gateway_data_dir`

前端只做展示、收集输入、复制文本和调用这些命令；没有前端直接改数据库、HTTP Agent API 或任意 shell 执行。

## CLI 安装边界

- Desktop 只负责检测、配置路径、启动已有 CLI，不捆绑侧载 CLI
- 未找到 CLI 时显示安装说明、Releases 下载入口、重新检测、手动设置路径
- 不显示无效的「启用」按钮，不新增 Tauri sidecar，不修改发布流程
- 正式形态仍是 Desktop DMG + 独立 headless CLI tarball

## 安装交付方案评估

现有 `release.yml` 已经分别构建 Desktop（macOS `.app` / DMG）和 `agentgate-serve` 的独立 tarball；`tauri.conf.json` 没有 `externalBin`。本轮不修改发行体系，只比较两种方案。

| 维度 | A. 独立 CLI（现状） | B. Desktop 内置 CLI（sidecar） |
| --- | --- | --- |
| 用户安装 | 需安装 Desktop 并额外下载 CLI；页面有下载、手动路径、重新检测引导 | 安装 Desktop 即得到 CLI，路径由应用控制 |
| Tauri 打包 | 不改 | 需要 `externalBin`、按 arch 放二进制、签名/公证一起处理 |
| 现有 release.yml | 已支持，无改动 | 需要改 Desktop 构建矩阵和资产结构 |
| CLI 独立升级 | 可以单独升级，适合 headless / 终端用户 | 与 Desktop 版本绑定；独立升级会与内置副本冲突 |
| 可执行文件发现 | 需要 PATH / 常见目录 / 手动路径 | 路径固定，但需要处理 sidecar 版本与安装位置 |
| Service 生命周期 | 一个独立 CLI，一套 service | 可能出现 Desktop 内置 CLI 与用户 PATH CLI 两个版本，需保证只有一个 Service |
| 维护成本 | 低，职责清晰 | 高，签名、升级、双版本、调试成本都增加 |

建议：继续方案 A。现有发行方式没有造成不可接受的交付阻塞，CLI 缺失已有明确引导；本轮不引入 sidecar，也不引入第二套 Agent Service。若未来要求「安装 Desktop 即用」，应单独提案，统一 CLI 版本、签名、升级和 Service 所有权后再改。

## macOS GUI 验收

环境：isolated Tauri dev build，独立数据目录 `/tmp/agentgate-desktop-accept/db`，临时 `LEVELER_HOME`，未使用生产数据库。真实 Codex / Claude / CodeLeveler / Pi CLI。

| 场景 | 结果 |
| --- | --- |
| 首次使用、CLI 未安装 | PASS：显示未找到 CLI、下载说明、手动设置路径；不崩溃 |
| 手动设置 Agent Gateway CLI | PASS：保存后显示版本与绝对路径 |
| Codex 状态 | PASS：`codex-cli 0.162.1`，`/opt/homebrew/bin/codex`，ChatGPT 登录，OpenAI Native，`gpt-6.1-sol` |
| Codex 认证无法确认 | PASS：显示「需验证」，不伪装成「可用」，测试连接仍可用 |
| Claude 认证缺失 | PASS：显示「未配置」、来源 Unknown，并提示配置 Claude Code 或测试真实请求 |
| Claude 状态 | PASS：`2.1.302`，第三方 token 已配置，DeepSeek，`deepseek-flash[1m]`；未显示 Token |
| 测试 Codex 连接 | PASS：确认费用提示后真实执行，`AGENTGATE_OK`，约 6.6s |
| 测试 Claude 连接 | PASS：确认费用提示后真实执行，`AGENTGATE_OK`，约 5.0s |
| 无效 Agent 路径 | PASS：保存被拒绝并显示错误，旧路径不变 |
| 无效 CLI 路径 | PASS：保存被拒绝；启动失败显示 `Agent Service did not become ready`，不崩溃 |
| Service 启动 / 停止 / 重启 | PASS：状态、版本、任务数正确；残留 Socket 显示已停止 |
| 有运行任务时停止 | PASS：确认框显示 `当前有 1 个任务正在运行`，未静默杀死 |
| Desktop 重启后 Service 状态 | PASS：终止 Desktop 后 Service 继续运行，重开后显示「运行中」 |
| CodeLeveler MCP 配置 | PASS：`leveler mcp list` 显示 `muxlayer`；旧版兼容性警告存在 |
| CodeLeveler MCP 移除 | PASS：只删除托管条目，状态回到未配置 |
| Phase 6 staging 写入 | PASS：CodeLeveler 配置 / 移除均通过 staging + 原子安装；`leveler mcp list` 结果与 UI 一致 |
| CodeLeveler MCP 端到端 | PASS（Phase 6 复验）：真实 `leveler run` 调用 `mcp__muxlayer__agent_list` 并返回两个 Agent |
| 托管条目指向旧 CLI | PASS：显示「需要更新」，可直接重新配置或移除，不再误判为冲突 |
| MCP 同名冲突 | PASS：`/bin/echo` 同名条目显示「同名冲突」，原配置未覆盖 |
| 配置不可写 | PASS：staging 成功但原子替换被拒绝，真实配置未改动，UI 显示 Permission denied |
| Pi 通过 nvm + 受限 PATH | PASS：Desktop 子进程补齐同目录 Node 后，检测、配置和真实 MCP 调用均成功 |
| Pi 未安装 | PASS：显示「客户端未安装」，配置按钮不可用 |
| Pi 已安装但不可执行 | PASS：显示「客户端不可用」和 Permission denied，不误报为未安装 |
| Pi MCP 配置 | PASS：`pi mcp list` 显示 connected、5 tools |
| Pi MCP 端到端 | PASS（Phase 6 复验）：真实 `pi -p` 调用 `agent_list` 并返回两个 Agent |
| Pi MCP 移除 | PASS：移除托管条目；验收后已恢复原 `mcp.json` |
| GUI 受限 PATH | PASS：Codex / Claude 通过 common 目录发现；版本、模型、模型来源正常 |
| MCP 客户端未安装 | NOT_TESTED（GUI）：受限 PATH 下 Pi 显示「客户端不可用」及真实 `env: node` 错误；没有通过移动系统客户端来制造「完全未安装」 |

认证失效场景没有做破坏性测试：本阶段禁止修改认证文件，因此没有删除或伪造 Codex / Claude 凭据。

## 未完成与风险

- CodeLeveler 当前安装版 1.0.16 不包含 `e4510fa`；1.0.18 源码通过但尚未发布。页面保留升级警告，MCP 协议未做兼容性改写。最低兼容版本应为「第一个包含修复的正式发布」，本阶段没有把尚未发布的 1.0.18 写成正式最低版本。
- 仅 CodeLeveler 缺失的 GUI 场景未单独测试：本机 CodeLeveler 固定安装在 `/opt/homebrew/bin`，没有在不移动系统安装的前提下可靠隐藏；Pi 缺失场景已通过隔离 HOME 验证。
- 只验收 macOS；Windows / Linux GUI 未测试。
- 未生成签名 DMG，未执行公开发布、Tag、推送。
- Agent Service 启动失败最长等待 20 秒；该路径沿用 Phase 5 的真实 GUI 验收，本轮未改动启动逻辑。
- 最近错误来自 Service 日志；只显示级别和 message，不展示内部模块路径。
- 外部并发修改检查是「写入前比较 + 原子替换」，覆盖实际竞态窗口；macOS 没有可移植的 compare-and-swap rename，极端毫秒级竞态仍只能通过写入前比较降低，不能数学上消除。

## 测试结果

| 项目 | 结果 |
| --- | --- |
| `cargo test --features desktop --lib` | 1808 passed / 0 failed / 3 ignored |
| `cargo test --features desktop --test agent_gateway_phase_three_cli` | 2 passed |
| `cargo check --features desktop` | PASS |
| `cargo clippy --features desktop --lib` | 无 warning |
| `tsc --noEmit` | PASS |
| `vitest run` | 62 files / 489 tests passed |
| `vite build` | PASS |

Phase 6 新增覆盖：Node 客户端子进程 PATH、staging 布局、外部并发修改拒绝、原子安装、新配置文件 `0600`、旧托管条目迁移 / 移除、未托管冲突、非可执行客户端、Codex 认证未知状态、前端「需验证」「需要更新」「客户端不可用」交互。

完整测试首轮曾出现两个与并发/环境相关的既有测试波动（`health_check_fails_when_db_lock_unavailable`、`interrupt_after_process_exit_reports_process_exited`），单独复跑和第二次全量跑均通过；本轮没有修改这两个测试。
