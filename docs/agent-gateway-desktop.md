# Agent Gateway Desktop 使用说明

> Agent Gateway 在 2.0.9 中仍是测试性功能（Beta），接口和行为可能调整，不建议用于关键生产流程。

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
| 签名/公证 | Desktop 与嵌套 sidecar 一起签名；2.0.9 由 CI 完成签名、公证和发布 |

未新增 `agentgate-serve start`：现有 `serve`、`agent-service`、`mcp-agent` 已分别覆盖 Model Gateway、常驻 Agent Service 和 MCP 自动拉起；新增 `start` 会需要额外的前台/后台 Supervisor 语义，收益不足。Desktop 的统一入口由 Gateway Manager 承担，headless 保持原命令兼容。

