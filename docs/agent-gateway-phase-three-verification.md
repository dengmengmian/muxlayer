# Agent Gateway 第三阶段：真实验收、审计与收口

日期：2026-10-16。Verdict：**PARTIAL**，不满足整体稳定交付标准。

> 本文保留第三阶段历史。后续已修复原始SIGKILL残留并完成真实A–H验收；最新保证范围与剩余风险见[生命周期最终报告](agent-gateway-lifecycle-final.md)。

真实外部 Agent、CodeLeveler 和 Pi 的常规链路、完整结果、连续会话、取消与结果恢复已通过。仍有一项已确认核心生命周期缺陷：**macOS 上 Codex App Server 自身被 SIGKILL 后，其独立 Session 中的 shell 后代可能残留**。Windows 原生编译与运行也未完成，不能给整体 PASS。

## 1. 起始状态与范围

- 仓库：`/Users/mengmian/Develop/app/dengmengmian/AgentGate`。
- 全程沿用 `dengmm/agent-gateway-phase-one`，未新建或切换分支，未 push / 发版。
- 起始 HEAD：`4aaa3ac`（已有 Claude 第三方端点支持）；前两阶段分别为 `61b8524`、`1207cb7`。
- 读取了 AGENTS、最新契约、前阶段验收、Service / Adapter / Storage / IPC / MCP 与 CLI 实现。
- 保留既有未跟踪 `.pi/`。CodeLeveler 中其他执行器 WIP 也未暂存、修改或提交。
- 使用隔离数据库与 MCP 配置；没有修改全局 Codex / Claude 认证，没有接管 Desktop 现有会话，没有切换模型伪装验收。

## 2. 架构审计结论

| 边界 | 结论 |
| --- | --- |
| Service / Adapter | Service 持有 Task / Session 状态、排他和持久化；Adapter 持有真实 Thread / Turn 与子进程协议；未重写 Adapter 或 Model Gateway |
| 唯一真相源 | SQLite 为耐久真相，内存为写穿缓存；修复了提交、取消、Thread、Running、终态、对账的写入顺序与原子性 |
| 终态 / 取消 | 取消意图不等于终态，真实 provider 事件裁决完成竞态；普通终态不覆盖；重启 interrupted 仅经官方只读查证升级 |
| 恢复 | 先 durable interrupted，再查原 Turn；无法查证不报成功、不重新执行；二次重启仍保留待对账身份 |
| IPC | OS 所有权锁先于数据库恢复；Unix socket 0600、私有目录0700、token认证；有界输入与响应超限错误 |
| MCP | 只提交任务，不占有执行生命周期；变更请求断线不重放，避免重复执行 |
| Codex | 使用官方公开 CLI App Server；有效 provider / 官方模型目录校验及子进程配置隔离，调用不经过 Model Gateway |
| Claude Code | 原生 CLI，真实 init 校验 Read/Grep/Glob 白名单及空 MCP inventory；用户既有配置被信任，读取范围不局限 cwd |
| 进程控制 | Unix guardian 只能清理专属进程组，无法保证回收 Codex 自行 setsid 的后代；Windows 仍缺全树所有权证明 |

协议依据核对了 [官方 App Server 文档](https://learn.chatgpt.com/docs/app-server)。Native 认证配置保持现状，未引入文档中其他集成模式的凭据交换方案。

## 3. 问题、根因与实际修复

| 问题 | 根因 | 修复 / 证据 |
| --- | --- | --- |
| 第二实例先污染运行任务再被拒绝 | 恢复数据库发生在 socket / PID 判断前，且判断不提供独占所有权 | 恢复前获取 OS 文件锁；真实第二 CLI 测试红：Running→Interrupted，绿：仍 Running |
| 陈旧 PID、PID 复用及启动竞态 | PID 文本 / socket probe 非所有权机制 | File::try_lock；锁文件保留防 inode 分裂；陈旧活 PID 文本不阻止新 owner |
| Task 插入失败留下永久 Session busy | Session 与 Task 分步提交 | 新增原子 submit_task；SQLite事务 / MemoryStore同次更新；trigger 注入失败回归 |
| 取消及 Thread / Running 假内存状态 | 先改缓存再落库 | 克隆候选快照，写入成功后才更新缓存 |
| 对账落库失败仍显示 completed | 先更新内存再写终态 | 完成事务成功后才替换快照 |
| 终态两次写入均失败仍可能伪装终态 | 失败缓存与耐久状态不一致 | status 明确 storage 错误，保留耐久快照及 busy占位 |
| 第二次重启丢失待对账队列 | 队列仅在第一次恢复内存中 | 由已持久化重启错误重建原 Turn 对账队列 |
| 历史 Agent 已不可用时 panic | continue 直接索引注册表 | 返回 unknown_agent |
| 分页整数溢出 | offset + limit 未防溢出 | saturating_add；Unicode scalar 分页保持原文 |
| MCP 响应丢失后重复执行 | 所有方法传输失败都自动重试 | 仅重试 list/status，变更请求明确 outcome unknown |
| MCP / IPC 超长无换行输入耗尽内存 | read_line 无界等待换行/EOF | 读取期间限制32MiB+1；超限立即拒绝 |
| 超大响应未遵守传输限制 | 写路径无大小检查 | 结构化 invalid_request 提示分页，保留连接，无截断 |
| fallback socket 改动共享临时目录权限 | socket直接放temp_dir、bind chmod父目录 | 使用专属哈希子目录；拒绝符号链接及非属主目录 |
| token / lock 跟随符号链接 | 直接写文件 | 拒绝非普通文件；Unix创建0600；目录权限失败明确报错 |
| Service硬杀时Drop不执行 | kill_on_drop依赖Rust析构 | 私有寿命管道驱动Unix guardian清理所属进程组；不宣称覆盖独立Session |
| 关闭错误被报告为成功 | shutdown错误只日志记录 | 清理资源后返回真实shutdown错误；红→绿 |
| CodeLeveler把MCP业务错误当成功 | 通用MCP适配器忽略isError | 仅修复通用isError传播；独立仓库、提交与测试，无Codex特例 |

所有缺陷修复都先有失败回归再做实现。大结果测试属于已有能力验收，原本即绿，未借此修改生产实现。未为覆盖率增加测试，未修改两项既有 fixture 失败或三项既有 CLI lint。

## 4. 真实 Codex MCP 验收

官方 CLI：`/opt/homebrew/bin/codex`，`codex-cli 0.162.1`。显式使用有效公开 CLI，PATH 中另一包装器的 vendor 文件缺失不作为可用 CLI。真实请求由生产 CodexFactory 的官方 provider / 模型验证路径执行，未调用 Model Gateway。

| 场景 | 结果 |
| --- | --- |
| MCP run / status / 完整最终回答 | PASS，含随机源码 nonce与实际项目审计 |
| 原 Session / Thread 续会 | PASS，新Task/Turn，原Session/Thread |
| 真实长命令取消 | PASS，先观察commandExecution/sleep，再取消并达到cancelled |
| MCP 客户端退出 | PASS，运行任务继续，新客户端可读原任务 |
| 只读强制执行 | PASS，官方command/exec覆盖文件及touch均拒绝，原文件未变 |
| Service SIGKILL / 重启 | PASS（采样进程范围），已捕获后代消失，任务恢复interrupted，原Thread可续会 |
| Codex App Server SIGKILL | **FAIL：独立Session的sleep残留**；Task正确记录interrupted，没有假completed |

证据：[MCP真实结果](phase3-evidence/mcp-live.json)、[强制只读](phase3-evidence/readonly.json)、[服务SIGKILL](phase3-evidence/service-sigkill.json)、[Codex SIGKILL失败](phase3-evidence/codex-sigkill-failure.json)。

## 5. 真实 Claude Code MCP 验收

`claude auth status` 返回 `loggedIn:true`，没有认证阻塞。CLI为2.1.287；真实init模型是**既有用户配置的 DeepSeek `deepseek-flash[1m]`**，不能标为官方 Anthropic 模型推理。

- MCP run / status / 全文、Session ID、原Session续会：PASS。
- 真实运行中取消、CLI被终止后cancelled：PASS。
- 真实init只有Glob/Grep/Read、MCP为空；项目级hook对照探针：PASS。
- 完成回答后杀Service、重启读回完全相同结果，并用原Thread续会重复nonce：PASS。
- 未单独强杀 Claude CLI 验证；CLI意外退出路径由真实子进程测试替身覆盖，不等同于真实Claude崩溃验收。

证据：[MCP真实结果](phase3-evidence/mcp-live.json)、[Claude重启全文与续会](phase3-evidence/claude-restart.json)。

## 6. CodeLeveler 端到端

使用 `/opt/homebrew/bin/leveler` 的实际 `mcp add` / `run` 命令，注册同一 `agentgate-serve mcp-agent`。只用独立配置与数据库，不改Kernel。

- 真实 agent_run → task_id → agent_status → 完整源码审计展示：PASS。
- agent_continue复用Session / Thread：PASS。
- 真实运行中的取消、轮询到cancelled：PASS。
- unknown task错误：修复前tool_result.is_error=false；修复后true且保留not_found/message。
- 30秒MCP调用预算内提交与查询；长审计由轮询完成，没有把整个推理放进一次工具调用。
- CodeLeveler原生用户“取消当前聊天”是否自动取消底层Gateway任务：未验证；本轮证明的是显式agent_cancel。MCP客户端退出不会默认取消Service任务。
- 未单独复现CodeLeveler面对MCP server异常退出的完整交互；Gateway变更请求断线不重放已有回归。

## 7. Pi 端到端

Pi 1.3.0，原生MCP发现、调用同一Server：Codex run/status/continue、完整展示、Unicode显式分页均PASS。Pi → Claude Code run/continue亦PASS，模型来源为上述DeepSeek配置。

初次尝试的 `--no-extensions` 会移除Pi的MCP扩展，已如实纠正；项目server trust未通过时没有关闭安全设置，改为唯一临时全局注册。验收结束移除临时注册、关闭独立Service，保留原配置。

Pi展示Claude首答省去一个空行，实质文本无缺失；严格字节展示比较为false，忽略空行比较为true。没有把排版转换当字节一致。

CodeLeveler与Pi的完整记录、工具调用数、SHA256、全文、关联ID及最终新二进制复验均见 [clients.json](phase3-evidence/clients.json)。

## 8. Session 与结果完整性

| 客户端 | 操作 | Task | Session | Thread | 字符数 |
| --- | --- | --- | --- | --- | --- |
| CodeLeveler | leveler-run | 58b7e9a2-c691-44fa-aa33-a1c2c06ca61c | 4cd5fdf0-ac5c-4360-a833-acc2ab53bcb9 | 01a14337-33aa-7cd1-8fc0-01389f442aaa | 281 |
| CodeLeveler | leveler-continue | 6d05e223-a0f6-487f-8244-672351fc06cc | 4cd5fdf0-ac5c-4360-a833-acc2ab53bcb9 | 01a14337-33aa-7cd1-8fc0-01389f442aaa | 242 |
| Pi | pi-global | b3561fb3-318b-400f-a2c5-8eafb86ec9cd | f512a0b7-ed98-4783-a29a-367852dc3a33 | 01a14338-1dc9-7252-86a6-f93b78be8be4 | 495 |
| Pi | pi-continue | 791ad041-e3ca-4acf-a35e-f3947f849204 | f512a0b7-ed98-4783-a29a-367852dc3a33 | 01a14338-1dc9-7252-86a6-f93b78be8be4 | 199 |
| Pi | pi-claude | b82a279a-5b27-468a-a0ff-4f4d96689384 | cb5f49d9-7d7f-4d58-a573-711a0cbe9f13 | d2101a9e-b06f-4a00-b167-3efaca847031 | 301 |
| Pi | pi-claude | d574e29b-a1fb-4ddb-81b3-18a1c75f72aa | cb5f49d9-7d7f-4d58-a573-711a0cbe9f13 | d2101a9e-b06f-4a00-b167-3efaca847031 | 83 |
| CodeLeveler | leveler-final | ca4d5bb4-f137-4ea1-aede-96befd813904 | 3a895b60-6a66-46ec-8072-3068ab7890e9 | 01a1433d-18a6-7643-aa80-64d003562cde | 20 |
| CodeLeveler | leveler-final | bdeb1148-878e-46d6-809d-dea66240d87f | 3a895b60-6a66-46ec-8072-3068ab7890e9 | 01a1433d-18a6-7643-aa80-64d003562cde | 4 |
| Pi | pi-final | 71e29e3d-60c0-4599-9721-3f6b1ced4cd7 | 49d72186-095a-44ff-81d4-401bc2961996 | 01a1433d-601a-79b2-bb69-30f2e644f0e8 | 20 |
| Pi | pi-final | b14b040b-1bf6-40f3-a205-1fc69a0be2b6 | 49d72186-095a-44ff-81d4-401bc2961996 | 01a1433d-601a-79b2-bb69-30f2e644f0e8 | 4 |

- Service内部保存完整结果；成功必须有真实成功终局与非空最终回答，过程事件不代替最终回答。
- Pi五页重组495字符原文一致，Unicode包含emoji /组合字符；MCP测试也覆盖稳定字符offset。
- 超过IPC上限：真实SQLite保存33,554,439字节Unicode结果，同连接默认status与cancel均返回结构化invalid_request，之后6页（每页2,000,000字符）逐字重组相等。生产无Mock，测试仅替换provider。
- 空回答、失败、落库失败、重启、取消/完成竞态均有回归。持续落库失败保留占位并返回storage，需要恢复存储后重启，不宣称自动自愈。
- mutation响应丢失后无法保证调用方取得task_id；当前明确报结果未知而不自动重放，尚未新增幂等提交协议。

## 9. 取消、崩溃与恢复的证据范围

服务SIGKILL验收先观察真实sleep启动，再确认MCP断开不会取消任务，杀独立Service，检查已采样后代退出，重启查询interrupted并在原Thread续会完成RECOVERED。它证明的是**该次采样列表**，不证明任意后续fork均受管理。

Codex SIGKILL失败记录：Task `937ee6c7-9575-4ea0-936a-38f27531b61a`；准确ID以 [failure JSON](phase3-evidence/codex-sigkill-failure.json) 为准。捕获到`sleep`成为独立PGID / Session leader（状态Ss），Task durable interrupted；测试teardown只清理捕获的私有后代，没有清理Desktop或其他用户进程。

源码旁证：[Codex spawn.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/spawn.rs) 的RedirectForShellTool调用detach_from_tty；[process_group.rs](https://github.com/openai/codex/blob/main/codex-rs/utils/pty/src/process_group.rs) 使用setsid，只有Linux配置parent-death信号。这解释了观察到的Session逃逸；这些main源码不是本机0.162.1精确构建commit，实测进程证据才是本轮FAIL依据。

未采用猜PID或定时kill的生产兜底，未静默fallback。**该缺陷未解决，整体稳定交付不通过。**

## 10. macOS 与 Windows

| 平台 | 结果 / 未覆盖 |
| --- | --- |
| macOS原生 | headless CLI编译、真实MCP / 两客户端、只读、取消、结果恢复通过；Codex自身SIGKILL独立Session后代回收失败 |
| Windows静态审计 | 有Named Pipe / first_pipe_instance、.exe发现、路径哈希与OS文件锁代码路径；token文件ACL继承和Named Pipe默认DACL未完成原生验证，全树进程回收未实现Job Object所有权 |
| Windows交叉编译尝试 | 已安装x86_64-pc-windows-msvc Rust目标；实际cargo check因zstd-sys C依赖缺Windows SDK的stdlib.h/string.h失败，未到达完整项目Rust检查，不是Windows编译通过 |
| Windows原生E2E | 没有可用Windows运行环境，BLOCKED_ENV；Named Pipe权限、并发、取消、路径发现、CLI打包均未实测 |

没有为绕过SDK缺失改动第三方依赖或破坏macOS行为。

## 11. 测试与回归

| 检查 | 结果 |
| --- | --- |
| 全库headless lib | 1432 passed / 0 failed / 2 ignored（官方真实只读探针默认忽略；进程测试内部helper默认忽略） |
| Agent Gateway lib子集 | 67 passed / 0 failed / 2 ignored |
| MCP / IPC集成 | 9 passed，含33MiB大结果 |
| CLI二次启动回归 | 1 passed；不依赖真实安装/认证，临时CLI只用于version发现 |
| 真实MCP测试 | 4 passed，45.52秒：Codex、Claude、Claude真实工具白名单、项目hook排除 |
| 官方只读command/exec | 显式运行ignored测试，1 passed；拒绝覆盖与新建 |
| Service SIGKILL脚本 | 采样范围PASS，interrupted及原Thread续会PASS |
| Codex自身SIGKILL脚本 | FAIL，已确认且未解决；任务状态未假成功 |
| Claude重启脚本 | PASS，全文一致及原Session续会 |
| CodeLeveler目标回归 | mcp_tool_error 1/1，MCP lib 6/6；独立reviewer复跑目标1/1 |
| cargo fmt / git diff --check | PASS |
| cargo clippy --lib -D warnings | PASS |
| cargo clippy --lib --tests -D warnings | 仅既有CLI三项：print_literal / unnecessary_map_or / too_many_arguments（因本轮插行现约861 /866 /952） |
| protocol_fixture | 38 passed / 2 failed：metrics_requires_token_only_and_health_does_not、unauthenticated_request_is_rejected_before_body_is_read；与前阶段记录一致，相关Model Gateway实现未改动 |

没有把已知失败改成ignored或放宽断言。没有宣称全仓库所有集成target全部复跑；本次完整跑lib、针对性Gateway集成及已知失败基线。

## 12. 提交与独立审查

| 仓库 | 实际Commit | 内容 |
| --- | --- | --- |
| AgentGate | `06450c8cc4cca5b8af49967896c26d93adf26f62` | 任务耐久性、OS服务所有权、IPC/MCP边界、Unix进程组guardian、必要回归及真实崩溃验收脚本 |
| CodeLeveler | `e4510fa981802fe3b791d70baddb14b775dc7a6a` | 仅通用MCP isError错误传播与回归 |

后续验收报告提交独立记录于Git历史，未push。

author与reviewer分开：state实现由lifecycle lane独立审查；IPC/process及shutdown修复由state lane独立审查并复跑；CLI与崩溃脚本由client lane独立审查；CodeLeveler由state lane独立审查和复跑。已修复审查发现的shutdown假成功与测试PID身份核对问题。审批仅针对实际修改，不意味着未解决生命周期缺陷获得放行。

## 13. 复跑

从仓库根目录运行；真实推理会消耗本机已有服务额度。

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --lib --test agent_gateway_mcp_ipc --test agent_gateway_phase_three_cli
cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib -- -D warnings
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --test agent_gateway_phase_two_live -- --ignored --nocapture --test-threads=1
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex python3 scripts/verify-agent-gateway-crash.py \
  --binary src-tauri/target/debug/agentgate-serve \
  --output /tmp/agentgate-phase3-service-recheck --scenario service_sigkill
# 已知失败，应返回非零并生成failure.json：
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex python3 scripts/verify-agent-gateway-crash.py \
  --binary src-tauri/target/debug/agentgate-serve \
  --output /tmp/agentgate-phase3-codex-recheck --scenario codex_sigkill
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --target x86_64-pc-windows-msvc --bin agentgate-serve
```

输出目录使用新的空目录，脚本不读取凭据。诊断原始日志留在本机`/tmp/agentgate-phase3-evidence`，仓库中仅保留可核验的fixture结果和客户端证据，没有token、密钥或用户认证内容。

## 14. 未解决风险与交付判断

**必改**：需要可靠的原生进程所有权方案或上游Codex生命周期支持，覆盖App Server强杀后的独立Session命令；在此之前不能标为稳定交付PASS。不得使用猜测PID扫描kill掩盖。

Windows原生编译、权限与端到端需在可用Windows SDK及运行环境下补验。Claude的独立CLI强杀、两客户端的全部异常退出/原生取消语义未穷尽；当前真实用户级Claude hook/插件信任边界与读取可越cwd的既有约定保持。进程guardian只保证其进程组，事件最后不足25条仍可能在硬崩溃中丢失，持续存储故障需运维恢复。

本轮已停止在验收与必要修复范围内，没有继续开发UI、其他Agent、云端能力或发布版本。
