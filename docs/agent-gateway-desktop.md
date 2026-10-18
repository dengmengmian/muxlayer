# Agent Gateway Desktop 使用说明与验收记录

> Agent Gateway 在 2.0.9 中仍是测试性功能（Beta），接口和行为可能调整，不建议用于关键生产流程。

实现提交：

- Phase 5：`43c4e888519563f2151e9a303c4493f98b817c5a`
- Phase 6 异常修复：`5259efa51bb9ed8e195c475fb6dab653af030271`
- Phase 6 统一启动与内置 CLI：`d6dab0f648000320525fce0f197ba322714ae58a`
- Phase 6 启动状态与错误：`5881b5e3c1a7bcf055cc7ef3d0b4ea32d1cdd285`
- 产品交互重构：`8e93eebca582a80b8a53eb7d852276fcaa15a27a`

本轮增加 Desktop 统一启动管理、内置 CLI 和稳定 MCP 路径；未改动 Model Gateway 业务、Codex / Claude Adapter、Agent Service 任务语义或 MCP 协议。

## 页面结构

侧边栏「工具 → Agent Gateway」页面按普通用户的实际操作顺序组织，不再以 Agents / Connections / Settings 三个独立 Tab 作为主入口：

| 区域 | 内容 |
| --- | --- |
| 页头 | 产品用途说明、Agent Service 健康摘要、重新检测 |
| ① 选择可用 Agent | Codex / Claude Code 的名称、模型来源、可用状态、测试连接、详细信息和路径编辑 |
| ② 连接到你的应用 | CodeLeveler / Pi 的配置状态、兼容性提醒、配置 / 移除 MCP、详细信息 |
| ③ 开始使用 | 根据可用 Agent 和已配置客户端生成可复制的真实指令 |
| 高级设置与诊断 | Agent Service 启停、自动启动、CLI 路径、数据目录、运行任务、历史错误、诊断信息、原始 MCP 配置 |

路径、CLI 版本、配置文件、Provider ID 等默认收进「详细信息」；主页面只保留名称、模型来源、状态和下一步操作。

## 产品交互与状态语义

用户目标被明确为三步：

1. 确认 Codex 或 Claude Code 能真实执行；
2. 把 MCP 配置写入 CodeLeveler 或 Pi；
3. 打开客户端，用自然语言指令调用 Agent。

Agent 状态：

- 未安装
- 未配置
- 需要验证（检测到 CLI，但认证/模型配置无法确认）
- 可用
- 测试未通过
- 当前不可用

真实测试失败会显示「测试未通过」，并把原始错误放进可展开的「技术详情」。401 的默认文案为认证未通过，同时明确提示「这不代表账号一定失效，可能是当前环境没有提供有效凭据」，不把隔离环境的错误误判为用户账号问题。

客户端状态：

- 未安装
- 未配置
- 已配置（配置文件存在不等于在线）
- 需要更新（旧 CLI 路径）
- 同名冲突
- 客户端不可用（例如 Pi 缺少 Node）

只有真实客户端调用成功才属于「验证成功」；当前主界面不会把「已配置」写成「已连接」。

首次使用：

- 没有 Agent：显示安装 Codex / Claude Code 的入口和重新检测；
- 有 Agent 但没有客户端：第③步引导先完成第②步；
- Agent 不可用或客户端配置过期：不显示可直接使用的成功态。

错误与诊断：

- 主界面使用中文、可操作的错误说明；
- 原始错误、日志、命令输出只在「技术详情 / 诊断信息」中展示；
- Token、Authorization、API Key 等在展示前脱敏；
- Service 健康时主页面显示「Agent 服务运行正常」，历史错误只出现在高级设置中，不把历史错误当成当前故障。
- 顶部全局状态现在明确写着「模型网关运行中 / 模型网关未启动」，不再用含糊的「运行中 / 已停止」让用户误以为 Agent Gateway 也停止。

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

## 统一 Gateway Manager

Desktop 启动后由 `src-tauri/src/app/gateway_manager.rs` 统一协调，但不合并两个 Gateway：

- Model Gateway 仍由 `AppState.gateway_runtime` 持有，是 Tauri 进程内的 HTTP Server
- Agent Gateway 仍是独立 `agentgate-serve agent-service` 进程，由 IPC / SQLite 锁保证单实例
- Manager 只做状态查询、按各自配置启动、复用已运行实例和记录启动错误
- 两个 Gateway 按顺序独立启动；一个失败不会阻止另一个
- 状态查询只读，不触发启动副作用
- Model Gateway 继续遵循原有 `gateway_settings.auto_start`
- Agent Gateway 使用 `agent_gateway.auto_start`（默认 true，可在页面关闭）
- 已运行的 Agent Service 直接复用，不会创建第二个实例
- 关闭 Desktop 不停止 Agent Service；运行中的任务继续，Model Gateway 随 Desktop 退出

统一状态入口位于 Agent Gateway 页面顶部：

- Model Gateway：运行中 / 已停止 / 错误、host:port、启动 / 停止
- Agent Gateway：运行中 / 已停止 / 错误、Service 版本、自动启动开关、启动 / 停止
- 内置 CLI 路径
- 状态每 5 秒刷新一次，用户操作后立即刷新

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
- `get_gateway_manager_status`
- `set_agent_gateway_auto_start`

前端只做展示、收集输入、复制文本和调用这些命令；没有前端直接改数据库、HTTP Agent API 或任意 shell 执行。

## Desktop 内置 CLI

Desktop 现在通过 Tauri `externalBin` 内置 `agentgate-serve`：

- `src-tauri/tauri.conf.json` 的 `bundle.externalBin` 指向 `binaries/agentgate-serve`
- `scripts/prepare-desktop-cli.sh` 按目标架构构建一份 headless CLI，并放到 Tauri 约定的 `binaries/agentgate-serve-<target>`；产物不入库
- `release.yml` 的 Desktop 构建、preflight 都在 Tauri 构建前执行该脚本
- 独立 headless CLI tarball 继续由 `cli` job 发行，未删除、未改协议
- 仍然只有一份 Agent Gateway 核心实现，没有 Desktop 专用 Agent Gateway

Desktop 首次启动时把内置 sidecar 复制到用户级稳定路径：

`~/.muxlayer/bin/agentgate-serve`（Windows 为 `agentgate-serve.exe`）

规则：

1. 目录不存在时创建，文件 `0755`，父目录 `0755`
2. 使用同目录临时文件 + `fsync` + 原子 rename
3. 写入后执行 `--version` 校验；失败保留旧文件
4. 写入 `<bin>/.agentgate-serve.managed.json` 记录版本和文件大小
5. 只有 marker 与目标文件匹配时才更新，视为本应用托管
6. 目标已存在但没有匹配 marker 时视为用户自行安装，绝不覆盖
7. 不修改用户 PATH，不覆盖 Homebrew / 用户独立安装
8. 不把 `.app/Contents` 或 App Translocation 临时路径写入 MCP

MCP 只使用稳定路径；Desktop 更新后路径不变，MCP 配置继续有效。正在运行的 Agent Service 持有旧 inode，替换磁盘文件不会终止运行中任务；新启动的 MCP/Service 使用新文件。

CLI 发现优先级：

1. 用户显式设置
2. `AGENTGATE_SERVE_BIN`
3. 进程 PATH / 常见安装目录
4. Desktop 托管的内置 CLI

显式路径失败时不会回退到内置路径，避免“我设置了一个坏路径，系统却悄悄用另一个”。普通用户不设置路径时，内置 CLI 自动兜底。

## 安装交付方案

最终采用“Desktop 内置 + 独立 CLI 继续发行”的混合方案：

| 维度 | 结果 |
| --- | --- |
| 一次安装 | `MuxLayer.dmg` 内含 Agent Gateway CLI |
| 独立 headless | `agentgate-serve-<arch>.tar.gz` 继续发行 |
| Service 所有权 | 仍是独立进程 + SQLite 单实例锁 |
| Desktop 更新 | 只更新稳定路径的托管副本，不删除 SQLite |
| 用户已有 CLI | 无托管 marker 时不覆盖；显式路径优先 |
| 签名/公证 | Desktop 与嵌套 sidecar 一起签名；本轮只做本地 ad-hoc 构建验证 |

未新增 `agentgate-serve start`：现有 `serve`、`agent-service`、`mcp-agent` 已分别覆盖 Model Gateway、常驻 Agent Service 和 MCP 自动拉起；新增 `start` 会需要额外的前台/后台 Supervisor 语义，收益不足。Desktop 的统一入口由 Gateway Manager 承担，headless 保持原命令兼容。

## macOS GUI 验收

环境：本地真实 release `.app`（ad-hoc，未签名/未公证）从隔离 HOME 启动，`PATH=/usr/bin:/bin` 模拟 Finder；独立 Desktop DB / CLI DB；真实 Codex / Claude / CodeLeveler / Pi CLI。未使用生产数据，未修改真实 MCP 配置。

| 场景 | 结果 |
| --- | --- |
| 首次使用、CLI 未安装 | PASS：显示未找到 CLI、下载说明、手动设置路径；不崩溃 |
| 手动设置 Agent Gateway CLI | PASS：保存后显示版本与绝对路径 |
| Codex 状态 | PASS：`codex-cli 0.162.1`，`/opt/homebrew/bin/codex`，ChatGPT 登录，OpenAI Native，`gpt-6.1-sol` |
| Codex 认证无法确认 | PASS：显示「需验证」，不伪装成「可用」，测试连接仍可用 |
| 新首页用途与步骤 | PASS：真实 `.app` 首屏直接说明用途，并显示「① 选择可用 Agent / ② 连接到你的应用 / ③ 开始使用」 |
| 技术信息层级 | PASS：版本、路径、配置文件默认不显示，展开「详细信息」才出现 |
| Codex 认证未知 | PASS：显示「需要验证」并引导测试；测试成功后才提升为可用 |
| 真实 401 | PASS：显示「测试未通过」和中文认证说明，原始 401 只在「技术详情」 |
| Claude 模型来源 | PASS：显示 DeepSeek，不显示 Anthropic 官方 |
| CodeLeveler 旧版本 | PASS：主文案只提示升级，不再暴露 `e4510fa / UNTRUSTED / dirty` |
| 开始使用指令 | PASS：有可用 Agent + 已配置客户端时显示可复制指令；缺少条件时只显示下一步 |
| 高级设置 | PASS：默认折叠；服务、CLI、数据目录、历史错误、诊断和原始 MCP 配置在内部 |
| 窄窗口 | PASS：960×600 下状态、按钮和文案不横向溢出 |
| Model Gateway 停止 | PASS：顶部显示「模型网关未启动」，Agent 服务仍显示正常，不混淆 |
| Desktop 重启恢复 | PASS：重建并重开 `.app` 后页面恢复真实 Agent / 客户端 / Service 状态 |
| Claude 认证缺失 | PASS：显示「未配置」、来源 Unknown，并提示配置 Claude Code 或测试真实请求 |
| Claude 状态 | PASS：`2.1.302`，第三方 token 已配置，DeepSeek，`deepseek-flash[1m]`；未显示 Token |
| 测试 Codex 连接 | PASS：确认费用提示后真实执行，`AGENTGATE_OK`，约 6.6s |
| 测试 Claude 连接 | PASS：确认费用提示后真实执行，`AGENTGATE_OK`，约 5.0s |
| 无效 Agent 路径 | PASS：保存被拒绝并显示错误，旧路径不变 |
| 无效 CLI 路径 | PASS：保存被拒绝；启动失败显示 `Agent Service did not become ready`，不崩溃 |
| Service 启动 / 停止 / 重启 | PASS：状态、版本、任务数正确；残留 Socket 显示已停止 |
| 有运行任务时停止 | PASS：确认框显示 `当前有 1 个任务正在运行`，未静默杀死 |
| Desktop 重启后 Service 状态 | PASS：终止 Desktop 后 Service 继续运行，重开后显示「运行中」 |
| 一次安装、无独立 CLI | PASS：release `.app` 内含 sidecar，首次启动复制到 `~/.muxlayer/bin` 并自动启动 Agent Service |
| 双 Gateway 自动启动 | PASS：Model Gateway 按 auto_start 启动，Agent Gateway 默认启动；两个状态独立显示 |
| 已运行 Service 复用 | PASS：重开 Desktop 后 Service PID 不变，未创建第二个实例 |
| 两个 MCP 客户端并发 | PASS：两个客户端都连接同一个 Service PID，工具调用成功 |
| 关闭 Desktop 后任务持续 | PASS：Desktop 终止后 task 仍 running；新 MCP 客户端可查询并取消 |
| Model Gateway 启动失败 | PASS：端口被占用时 Agent Gateway 仍启动并 ready |
| Agent Gateway 启动失败 | PASS：CLI 数据目录不可用时 Model Gateway 仍启动并监听 |
| 用户已有 CLI | PASS：无托管 marker 的同名文件未被覆盖，校验和不变 |
| Desktop 更新后 MCP 配置 | PASS：重建 `.app` 后 CodeLeveler 配置仍指向同一稳定 CLI，Service 未重启 |
| Pi 受限 PATH + nvm | PASS：版本 1.3.0、`pi mcp list` connected / 5 tools、真实 `pi -p` 调用 `agent_list` |
| CodeLeveler 受限 PATH + 稳定 CLI | PASS：配置写入稳定路径，`leveler mcp list` 正常；真实调用成功 |
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
- 只验收 macOS；Windows / Linux GUI 与 sidecar 打包未测试。
- 未生成签名 DMG，未执行公开发布、Tag、推送；本轮 `.app` 是 ad-hoc 本地构建。正式发布必须验证嵌套 `agentgate-serve` 的 Developer ID 签名与公证，以及从 `~/.muxlayer/bin` 复制后的 Gatekeeper 行为。
- Desktop 更新替换稳定 CLI 时，不强制重启正在运行的 Agent Service；旧进程继续持有旧 inode，直到用户主动重启或服务退出。期间 MCP 客户端连接的是旧运行实例，可能出现新 CLI 文件与旧 Service 并存的版本窗口，这是为了不打断运行中任务的有意取舍。
- 用户自行放置在 `~/.muxlayer/bin/agentgate-serve` 且没有托管 marker 的文件不会被更新；如果版本过旧，页面会显示真实版本和错误，不会静默覆盖。
- Agent Service 启动失败最长等待 20 秒；已分别验证 Model 失败不影响 Agent、Agent 失败不影响 Model。
- 最近错误来自 Service 日志；只显示级别和 message，不展示内部模块路径。
- 外部并发修改检查是「写入前比较 + 原子替换」，覆盖实际竞态窗口；macOS 没有可移植的 compare-and-swap rename，极端毫秒级竞态仍只能通过写入前比较降低，不能数学上消除。

## 测试结果

| 项目 | 结果 |
| --- | --- |
| `cargo test --features desktop --lib` | 1816 passed / 0 failed / 3 ignored |
| `cargo test --features desktop --test agent_gateway_phase_three_cli` | 2 passed |
| `cargo check --features desktop` | PASS |
| `cargo clippy --features desktop --lib` | 无 warning |
| `tsc --noEmit` | PASS |
| `vitest run` | 62 files / 495 tests passed |
| `tauri build --bundles app --target aarch64-apple-darwin` | PASS，`.app` 内含 `Contents/MacOS/agentgate-serve` |
| `vite build` | PASS |

Phase 6 新增覆盖：Node 客户端子进程 PATH、staging 布局、外部并发修改拒绝、原子安装、新配置文件 `0600`、旧托管条目迁移 / 移除、未托管冲突、非可执行客户端、Codex 认证未知状态、前端「需验证」「需要更新」「客户端不可用」交互；统一启动新增内置 CLI 安装 / 用户安装不覆盖 / marker 更新、显式路径不回退内置 CLI、Agent auto-start 默认值与读写、双 Gateway 状态卡片。

完整测试首轮曾出现两个与并发/环境相关的既有测试波动（`health_check_fails_when_db_lock_unavailable`、`interrupt_after_process_exit_reports_process_exited`），单独复跑和第二次全量跑均通过；本轮没有修改这两个测试。
