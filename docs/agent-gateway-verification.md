# Agent Gateway 第一阶段验收记录

macOS、官方 Codex CLI `0.162.1` 下核心功能和真实验收通过。复用现有 Codex 登录，没有额外登录流程。Windows、MCP、IPC、SQLite 和跨服务重启恢复未验收或未实现。

## 实际调用与结果

Service 启动独立 stdio App Server。随机 Provider 不设置 endpoint，由 Codex 原生认证选择官方默认地址；Adapter 校验有效配置、默认模型及 Thread 的 Provider、模型和 read-only 策略。不经过 Model Gateway，不在认证失败后切换凭据。补充协议探测验证了官方来源与 `gpt-6.1-sol`；生产实现不硬编码模型。

真实 Service 测试通过，耗时 76.15 秒。下列长度直接计算自完整 `TaskRecord.final_answer`，没有裁剪内容。

| Case | Task ID | Session ID | Thread ID | Turn ID | State | Answer chars |
| --- | --- | --- | --- | --- | --- | --- |
| sandbox_fixture | c5619088-8077-423f-94c4-dc7d39960003 | 5c4302c1-4f5b-4b57-8ed5-bf74cddaf686 | 01a1403d-640a-74c0-a76d-861e004faf6e | 01a1403d-6464-7a01-88ef-dc3743e96128 | completed | 141 |
| continuation | 1f825b17-8433-487f-abff-71d06ecf462a | 5c4302c1-4f5b-4b57-8ed5-bf74cddaf686 | 01a1403d-640a-74c0-a76d-861e004faf6e | 01a1403d-8a96-72a0-8bba-a5416dfee492 | completed | 36 |
| source_audit | f973d3bd-3d48-401f-9742-0c8eec677a10 | 027ce59b-3024-44f1-9d89-dc27628b6c8c | 01a1403d-969b-7592-a9ec-29aff4bb3861 | 01a1403d-96f5-7ef3-abfb-af1b3410ce47 | completed | 1177 |
| source_audit_continuation | 5b513814-f3a5-45dd-ba63-9f4e5fb91130 | 027ce59b-3024-44f1-9d89-dc27628b6c8c | 01a1403d-969b-7592-a9ec-29aff4bb3861 | 01a1403e-30e8-7f91-bebb-741c383f2d85 | completed | 634 |
| cancelled | 4ab1f822-699e-4ade-94d2-6e9d0648fc4d | 5c4302c1-4f5b-4b57-8ed5-bf74cddaf686 | 01a1403d-640a-74c0-a76d-861e004faf6e | 01a1403e-768c-7e43-ae8f-1b9eff162ac5 | cancelled | 0 |

临时仓库第二次回答准确返回第一次读取的随机 nonce；Task / Turn 不同，Session / Thread 相同。源码审计及续审也使用同一 Thread。

取消测试先观察到真实 shell 开始，再调用 `turn/interrupt`；Service 观察真实终态后进入 `cancelled`，没有把中断 RPC 的确认当作取消完成。实际命令开始事件如下。

```json
{
  "command": "/bin/zsh -lc 'sleep 120'",
  "exitCode": null,
  "status": "inProgress",
  "type": "commandExecution"
}
```

## 强制只读权限

模型可能遵守只读指令，直接拒绝尝试写文件。因此强制权限由官方 `command/exec` 确定性验证，使用与生产 Thread / Turn 等价的 `readOnly`、`networkAccess:false` sandbox。这个执行入口仅在测试中存在，没有新增生产执行能力。

测试在临时 Git 仓库实际执行读取、覆盖原文件及创建新文件三条命令，1 项通过，耗时 0.11 秒。

```json
{
  "create": {
    "exitCode": 1,
    "stderr": "touch: forbidden.txt: Operation not permitted\n",
    "stdout": ""
  },
  "new_file_absent": true,
  "overwrite": {
    "exitCode": 1,
    "stderr": "/bin/sh: evidence.txt: Operation not permitted\n",
    "stdout": ""
  },
  "read": {
    "exitCode": 0,
    "stderr": "",
    "stdout": "read-only nonce 066a909c-eaa7-4466-abeb-671f6e7f9f97\n"
  },
  "sandbox": {
    "networkAccess": false,
    "type": "readOnly"
  },
  "source_unchanged": true
}
```

审计期间 692 个项目文件指纹和 Git 状态相同；本次原 Codex `config.toml` / `auth.json` 哈希也相同。用户允许官方 Codex 正常自动刷新认证，本次哈希结果不保证未来不会刷新。Service 关闭后未观察到本轮 Native App Server 或 `sleep 120` 残留。强制崩溃后的任意后代进程清理尚未原生验收。

## 完整源码审计回答

以下为 `source_audit.final_answer` 原文：

最值得关注的权限边界：**本地访问令牌被用于远程 HTTP 鉴权时，传输机密性仅靠告警保障。** 已确认代码允许这种配置；尚未确认实际部署存在可利用漏洞。

| 证据 | 含义 |
|---|---|
| [cli_exposure.rs:23](/Users/mengmian/Develop/app/dengmengmian/AgentGate/src-tauri/src/security/cli_exposure.rs:23)，23–31 行 | 非回环地址且未启用 TLS 时，只返回告警文本。 |
| [serve.rs:544](/Users/mengmian/Develop/app/dengmengmian/AgentGate/src-tauri/src/cli/serve.rs:544)，544–550、578 行 | 打印告警后仍启动服务，没有阻断。 |
| [server.rs:405](/Users/mengmian/Develop/app/dengmengmian/AgentGate/src-tauri/src/gateway/server.rs:405)，405–410 行 | 无 TLS 配置时实际启动明文 HTTP。 |
| [local_token.rs:196](/Users/mengmian/Develop/app/dengmengmian/AgentGate/src-tauri/src/security/local_token.rs:196)，196–209 行 | 持有与存储值一致的令牌即可通过令牌校验，没有客户端绑定。 |
| [shared.rs:285](/Users/mengmian/Develop/app/dengmengmian/AgentGate/src-tauri/src/gateway/routes/shared.rs:285)，285–325 行 | 令牌直接来自 Authorization 或 X-Api-Key 请求头。 |

**影响条件：** 若客户端通过不可信网络直接访问该 HTTP 监听，能监听链路的攻击者可获取令牌并重放，通过令牌这一层鉴权；请求内容也会暴露。其他 Host/Origin 检查不提供传输加密。

**修复建议：** 非回环监听且未配置 TLS 时默认拒绝启动；确需明文反代后端时，要求显式开启允许选项，并通过回环绑定或网络访问控制限制后端可达性。

**疑问与漏洞区分：** 默认监听为 `127.0.0.1`（serve.rs:40）。未检查部署网络和 TLS 终止位置，因此这是已确认的危险配置路径，不能直接认定当前部署已泄露令牌或存在未授权访问。

本次仅读取项目代码；未读取凭据或用户家目录其他内容，未运行测试、修改文件或委派。

## 完整续审回答

以下为同一 Thread 的 `source_audit_continuation.final_answer` 原文：

**确认存在明文暴露路径；是否构成实际漏洞，取决于部署链路。**

触发条件：以 `--host 0.0.0.0` 等非回环地址启动，不提供 TLS 证书和密钥，且客户端携带有效令牌通过可被监听的网络访问。

源码证据（路径均相对项目根目录）：

- `src-tauri/src/cli/serve.rs:512–517、544–550、578`：未提供证书时选择无 TLS；告警后继续启动。
- `src-tauri/src/security/cli_exposure.rs:23–31`：非回环且无 TLS 只返回告警。
- `src-tauri/src/gateway/server.rs:405–410`：实际启动 HTTP。
- `src-tauri/src/security/local_token.rs:196–207`：令牌匹配即通过，无请求绑定或防重放。
- `src-tauri/src/gateway/routes/shared.rs:225–226、260–262`：IP Host、无 Origin 可放行，因此这些检查不能阻止非浏览器客户端重放窃取的令牌。

**最小修复：** 在 `cmd_serve` 确定 TLS 后、启动监听前，对“非回环且无 TLS”直接报错退出，可复用现有回环判断。确需明文反代后端，再单独设计显式放行。

目前确认的是配置风险，未确认部署可被监听或令牌已泄露。全程只读源码，未读取用户凭据、修改文件或委派。

## 自动测试与既有问题

- 全库 headless lib：1392 passed、1 ignored；默认忽略的真实权限测试已另行显式执行通过。
- Agent Gateway：27 项普通测试通过。Service 与 Adapter 均经历行为红→绿，覆盖 RPC 逆序响应 / EOF、完整结果、取消竞态、并发、认证继承及 shutdown 等待真实终态。
- 真实 Service 集成：1 passed；真实只读权限：1 passed。
- `cargo fmt --check` 与 headless `cargo clippy --lib -- -D warnings` 通过。
- 相关 fixture：54 passed、2 failed。两项 `protocol_fixture` 失败在原始 HEAD `7e88df1` 的归档副本同样复现：`metrics_requires_token_only_and_health_does_not` 的 Host 断言收到 200；`unauthenticated_request_is_rejected_before_body_is_read` 超过 5 秒。本轮未修改既有 Model Gateway。
- 更宽的 `clippy --lib --tests -- -D warnings` 报告 `cli/serve.rs` 既有三项 lint：754 行 `print_literal`、759 行 `unnecessary_map_or`、845 行 `too_many_arguments`。

## 复跑

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --test agent_gateway_codex_live -- --ignored --nocapture
AGENTGATE_CODEX_BIN=/opt/homebrew/bin/codex cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features cli --lib live_read_only_commands_are_denied -- --ignored --nocapture
```

`AGENTGATE_CODEX_BIN` 可替换成公开安装的官方 CLI 路径，不依赖 Desktop 私有路径。真实推理使用现有官方认证与额度；未登录或调用失败时正常失败，Gateway 不接管登录。下一轮接口见 [接口契约](agent-gateway-contract.md)。
