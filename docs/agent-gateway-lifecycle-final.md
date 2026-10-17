# Agent Gateway 生命周期最终验收

验收：2026-10-16；收口：2026-10-17。macOS 26.6.2，官方 `codex-cli 0.162.1`。

**Verdict：PARTIAL。原先已确认的 Codex SIGKILL 独立 Session 残留已修复，最终真实 A–H 8/8 PASS；macOS 上任意脱链后代的绝对回收仍无法保证。** 本报告只收口进程生命周期，保留平台保证边界。

## 1. Root Cause

1. Codex 的 shell 执行路径使用 `setsid()`。shell 与命令后代可以成为另一个 Session / PGID，原 guardian 的 `killpg` 覆盖不到它们；macOS 没有 Codex Linux 路径的 parent-death signal。源码已对照本机对应的 [rust-v0.162.1 pipe.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/utils/pty/src/pipe.rs) 与 [process_group.rs](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/utils/pty/src/process_group.rs)。
2. 真实 `turn/interrupt` / `interrupted` 终局也可能留下 shell。旧 Service 在 provider 中断后就发布 `cancelled`，没有证明执行范围已清理。
3. 本轮原生监管首轮验收还捕获到 Service SIGKILL 后监管器被 `SIGHUP` 终止，留下已经 STOP 的命令。监管器与 root 同 PGID，Service 死亡后成为孤儿进程组；冻结 root 的操作可能触发内核的孤儿停止组 HUP/CONT。现将 root 与监管器的 PGID 在出生 gate 放行前同步分开。
4. 监管器清理失败必须保持可见。补上了 Claude shutdown 内层 wait 错误、等待 future 被取消时丢句柄，以及首次清理失败被第二次空成功掩盖的错误路径。

Task `interrupted` / `cancelled` 仅是状态；本轮所有退出验收均独立检查真实进程身份。

## 2. 原始进程关系与证据

原链路：`Agent Service → shell guardian → Codex App Server → 独立 Session shell / sleep`。Codex code-mode-host 也可能处在独立 PGID，不能按进程名或原 PGID推断完整归属。

修复前真实 C 场景：Codex 被 SIGKILL 后，`sleep` PID / PGID / SID 均为 `53012`，内核 unique ID `134493870`，继续存活；两个无关真实 Codex / Shell 均存活。见 [修复前证据](lifecycle-evidence/before-codex-sigkill.json)。

本轮中间版本 E 场景：内核 kqueue 记录 guardian 退出 raw status `1`（SIGHUP），其已 STOP 的命令仍存在。见 [监管器 HUP 证据](lifecycle-evidence/before-owner-sighup.json)。这个失败也已纳入修复。

进程快照记录 PID、PPID、PGID、SID、unique ID、parent unique ID、pidversion。存活检查按 PID + unique ID，跨 `exec` 仍视为同一实例；不能把 pidversion 或名称变化当作退出。

## 3. 修复架构与控制权

```text
Agent Service
    ├─ stdio 协议 ──────────────── Codex App Server（root）
    └─ 私有寿命 FD ── native owner ─── fork / 出生 gate ── root
                          │                              └─ 任意 PGID / SID 后代
                          └─ root EXIT / 寿命 EOF
                              → 冻结已确认实例
                              → 有界补全可证明的身份链
                              → audit token 发信号
                              → 确认已登记成员退出
```

- 沿用每 Adapter 一个临时 owner，使用应用同一 executable 的内部入口；没有新增常驻 Daemon、端口、全局注册或运行时 Python / clang 依赖。C 在构建时静态链接，CLI与桌面入口在创建 runtime 前分流。
- owner fork root，先同步建立独立 PGID、读取 root 内核 unique ID、注册 kqueue 退出事件，完成这些步骤后才允许 root `exec`。
- root 与 owner 分组只用于保护监管器；归属判断不依赖 PGID、名称或 PID文本。
- Service 寿命 FD EOF 或 root 退出触发一次有界退出事务。运行期间没有周期扫描或周期性 kill。
- 从 [Apple 内核身份接口](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_info_private.h) 读取实例与原父实例关系，递归登记能够证明归属的成员。保存成员自身 unique ID，避免重挂父进程后错误换认实例。
- 每次发信号前取得真实 `TASK_AUDIT_TOKEN`，重新核对 unique ID，由 `proc_signal_with_audittoken` 原子核对 pidversion；PID复用不能替换已确认的实例。相关内核实现见 [proc_info.c](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/proc_info.c)。
- 先确认内核停止状态，再有界补全身份链；发送终止信号后确认已登记实例已消失或成为不可执行的 zombie。root由owner回收，真实 A–H 还要求全部采样 PID 实际消失。
- API / ABI / 身份 / 冻结 / 退出确认失败，owner非零退出，Rust返回明确错误；不退回猜PID或按名称清理。provider原始stderr仍屏蔽，owner只记录安全的进程身份和清理诊断。
- `cancelled` 发布前先关闭并确认整个 Adapter scope，随后移除运行时 Adapter 缓存；持久化 Session / Thread 映射保留，下一次续会创建新连接并调用官方 `thread/resume`。
- 首次取消清理失败就记录 `cleanup_error` 并阻止该Session续会；后续重试不能抹去清理未确认事实。Claude保持owner句柄直到wait成功，关闭future被取消后仍可继续等待。

## 4. 修改文件

| 文件 | 用途 |
| --- | --- |
| `src-tauri/native/process_owner.c` | macOS出生gate、kqueue、实例谱系、audit信号及有界退出确认 |
| `src-tauri/src/agent_gateway/process.rs` | 选择native owner、ready握手、错误传播与真实进程回归 |
| `src-tauri/src/agent_gateway/service.rs` | 取消终态前清理scope、重连原Thread、保留首次清理错误 |
| `src-tauri/src/agent_gateway/claude.rs` | 关闭错误传播、等待取消后保留owner；认证模式未改 |
| `src-tauri/src/agent_gateway/mod.rs` | 导出内部入口分流函数 |
| `src-tauri/src/cli/serve.rs`、`src-tauri/src/main.rs` | runtime / GUI启动前进入内部owner模式 |
| `src-tauri/build.rs`、`Cargo.toml`、`Cargo.lock` | 仅macOS编译链接native owner，新增构建依赖cc |
| `src-tauri/tests/agent_gateway_codex_live.rs` | library集成harness显式指定支持owner入口的CLI executable |
| `scripts/verify-agent-gateway-crash.py` | 补真实SID采集 |
| `scripts/verify-agent-gateway-lifecycle.py` | A–H真实验收、实例绑定信号、并发与无关进程证据 |
| 本报告、契约、验收索引及 `docs/lifecycle-evidence/` | 最终证据与保证范围 |

继续 `dengmm/agent-gateway-phase-one`；起始HEAD `dfae8bd`。既有未跟踪 `.pi/` 保留。没有修改 Model Gateway、Claude认证、UI、Windows实现或发版流程。

macOS library嵌入方若自己的main没有调用内部入口分流，须将 `AGENTGATE_PROCESS_OWNER_BIN` 指向配套 `agentgate-serve`。本应用CLI/桌面自身无需额外设置；集成测试明确传入Cargo构建的CLI路径，不猜测试产物目录。

## 5. 最终真实 macOS A–H

使用真实官方Codex、隔离数据库、临时Git仓库。每项都在观察到实际命令进程后执行退出动作；命令全部按只读策略运行，不读取或修改用户认证。

| 场景 | 最终结果 | 退出证据 |
| --- | --- | --- |
| A 正常取消Task | PASS | 旧scope全部实例消失；Task取消后用原Session恢复原Thread，新AppServer实例完成回答，随后新scope也退出 |
| B Codex正常退出 | PASS | 对官方command/exec启动的sleep关闭stdin，kqueue确认AppServer真实退出raw status 0，owner退出0，命令消失 |
| C Codex SIGKILL | PASS | 独立Session命令及code-mode-host、owner均消失 |
| D Service正常退出 | PASS | IPC service_shutdown，全部捕获实例消失 |
| E Service SIGKILL | PASS | 全部捕获实例消失；kqueue确认owner退出raw status 0，没有STOP残留 |
| F MCP Client断开 | PASS | 任务与原进程保留；重连显式取消后scope全部消失 |
| G 深层独立Session Shell | PASS | 真实Codex执行fork→setsid→shell→sleep，深层SID与外层执行进程、Codex根均不同；杀AppServer后全部清理 |
| H 多并发Session | PASS | 杀第一scope不影响第二scope及running任务；取消第二scope后全部退出 |

每项同时保留一个无关独立Session Shell及一个无关真实Codex App Server，始终检查其内核实例仍存活；验收结束只清理自己创建的这些sentinel。

最终完整PID / PGID / SID / unique ID、Task关联、取消续会及退出证据：[macos-matrix.json](lifecycle-evidence/macos-matrix.json)。本次矩阵中所有 `survivors` 为空，未发生误杀。

## 6. TDD、独立审查与回归

先确认真实Codex原始SIGKILL残留，并先写失败测试，再实现：

- `root_sigkill_reaps_detached_session_without_harming_unrelated_process`：原setsid后代存活跑红，修复后通过；覆盖并发owner与无关进程。
- `provider_group_shutdown_cannot_kill_its_owner`：旧共PGID布局会同时杀owner并留下独立Session；隔离后通过。
- 取消等待cleanup与重连原Thread、首次cleanup失败而第二次成功、Claude重复shutdown错误保留，均有目标行为红→绿。
- 并发16次启动立即关闭寿命FD、Claude关闭future被取消保留句柄，是对同一修复新增的验收，未虚称它们单独抢调度稳定跑红。

作者与审查分开。独立审查者严格编译C，并用两个独立owner / setsid后代及无关进程复验；另独立复跑进程、取消与错误传播测试。审查发现的停止/退出确认、stderr隔离、句柄丢失、首次失败掩盖及出生PGID竞态都已修复。

最终核心场景示例：

| 场景 | Codex PID | PGID | SID | 命令PID | PGID | SID | 残留数 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| C | 63143 | 63143 | 61675 | 63778 | 63778 | 63778 | 0 |
| E | 64578 | 64578 | 61675 | 65330 | 65330 | 65330 | 0 |
| G | 66182 | 66182 | 61675 | 66971 | 66971 | 66971 | 0 |

| 检查 | 最终结果 |
| --- | --- |
| Agent Gateway单元测试 | 74 passed / 0 failed / 3 ignored（真实权限探针及两个内部helper） |
| 全库headless lib | 1439 passed / 0 failed / 3 ignored |
| MCP / IPC集成 | 9 passed，含超大完整结果分页 |
| CLI所有权回归 | 1 passed |
| 真实Codex集成 | 1 passed，93.54秒：源码审计、只读、完整结果、原Thread续会、取消 |
| 真实MCP集成 | 4 passed，47.63秒：Codex、Claude及既有只读配置边界 |
| 最终真实生命周期矩阵 | 8 / 8 PASS；B额外确认AppServer原生退出状态0 |
| cargo fmt / git diff --check | PASS |
| cargo clippy --lib -D warnings | PASS |
| cargo clippy --lib --tests -D warnings | 仍仅三项既有CLI lint：print_literal、unnecessary_map_or、too_many_arguments；没有修改它们 |

测试命令和结果摘要：[regression.json](lifecycle-evidence/regression.json)。两个已知无关protocol fixture失败未修改，本轮没有重新运行这些Model Gateway fixture。没有处理Windows原生验收。

## 7. 剩余风险与明确保证范围

已验证范围：本机版本上，监管器存活且能够从内核读取完整归属身份链时，已登记及有界冻结补全的同权限后代会在owner报告成功前被确认停止执行；A–H中的全部采样PID最终实际消失。没有按进程名批杀、仅按PID认归属或在运行期周期扫描。

**不提供任意进程树的绝对保证：**

- 首次发现前，中间祖先已经退出，或者孤儿已经再次exec，可能丢失可证明的父实例链。`parent_unique`不是永久不可变；本机探针观察到孤儿exec后父unique变为1。监管器不会根据残缺链猜归属或杀同名进程，因此这种脱链后代可能残留。
- macOS不支持kqueue的递归NOTE_TRACK；无特权用户无法直接建立等同cgroup / Job Object的全树内核所有权边界。launchd的同组清理也不能涵盖setsid后代。没有因而再加一层无依据Daemon。
- `PROC_PIDUNIQIDENTIFIERINFO`是XNU私有flavor。实现核对结构尺寸并在身份API不符合预期时拒绝启动/报告错误；未来macOS私有ABI或权限变化需要重新验收。
- 监管器自身被SIGKILL、进程改变权限后无法查询身份、或内核无法及时终止进程，超出成功保证。可检测到的失败会明确返回错误；发现之前已丢失的身份链无法被可靠报告成“全树已清理”。
- 清理错误会阻止当前Session继续；其错误锁是运行时状态，恢复/重启前需要先确认异常scope实际状态。本轮没有新增持久化故障隔离协议。

因此本轮原复现及A–H达到PASS，整体任意进程树生命周期保证仍标为 **PARTIAL**。生产代码不把状态终态或“已发出SIGKILL”当作退出证明，也不声称零残留的绝对保证。

## 8. 复跑与提交

```bash
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib agent_gateway::
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --lib --test agent_gateway_mcp_ipc --test agent_gateway_phase_three_cli
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --test agent_gateway_codex_live -- --ignored --nocapture
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test \
  --manifest-path src-tauri/Cargo.toml --no-default-features --features cli \
  --test agent_gateway_phase_two_live -- --ignored --nocapture --test-threads=1
python3 scripts/verify-agent-gateway-lifecycle.py \
  --binary src-tauri/target/debug/agentgate-serve --codex /opt/homebrew/bin/codex \
  --output /tmp/agentgate-lifecycle-new-run
```

使用新的空输出目录。真实请求使用已有认证及官方模型额度。本轮完整Commit SHA见最终回复与Git历史；没有新建分支、push或发版。
