# macOS 最终发布验收记录

候选提交：`17af05c59ff026d3eb0c4d3185240e3cccca6ed3`

本轮只做 macOS Release Acceptance，不创建 Release、不签名、不公证、不 push、不打 Tag。

## 构建产物

| 产物 | 路径 / SHA |
| --- | --- |
| DMG | `src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/MuxLayer_2.0.8_aarch64.dmg` |
| DMG SHA256 | `e77dd50b88f378837bcc996c2b5c1fc28888d489af8ba4a48c1f4626611e7d56` |
| Desktop binary SHA256 | `16553d3d905b7012e200b993cdef8822f885fdb207016ff44e183a73d0205e4e` |
| Bundled CLI SHA256 | `aa86350e0db185886d30568a65ac6d65c65a7f22115755a61c0b6b726fd42f72` |

构建命令：

```bash
bash scripts/prepare-desktop-cli.sh aarch64-apple-darwin
vite build
tauri build --bundles dmg --target aarch64-apple-darwin \
  --config '{"build":{"beforeBuildCommand":""},"bundle":{"createUpdaterArtifacts":false}}'
```

这是本地 ad-hoc 构建，不是签名/公证产物。

## DMG 与安装

DMG 已验证：

- UDZO，CRC32 校验通过
- 包含 `MuxLayer.app` 和 `Applications -> /Applications` 链接
- `.app/Contents/MacOS/agentgate` 为 arm64
- `.app/Contents/MacOS/agentgate-serve` 为 arm64
- 两个可执行文件权限均为 `0755`
- 内置 CLI 与构建目录 release CLI SHA256 完全一致
- Info.plist：`com.mengmian.agentgate` / `2.0.8` / `LSMinimumSystemVersion=10.13`

在隔离 HOME、`PATH=/usr/bin:/bin` 下把 DMG 内容复制到用户级临时安装目录并启动：

- 首次启动自动把内置 CLI 安装到 `~/.muxlayer/bin/agentgate-serve`
- 生成托管 marker（版本 + 大小）
- Agent Service 自动启动并 ready
- 没有额外下载或安装 Gateway CLI
- 关闭 Desktop 后 Service 继续运行
- 从稳定 CLI 调用 MCP `agent_list` 成功
- 重开 Desktop 复用同一 Service PID

正式安装路径 `~/.muxlayer/bin/agentgate-serve` 不是临时构建目录，也不是 App Translocation 路径。

## 签名与公证

本轮没有 Apple Developer 凭据，未执行正式签名、公证、Gatekeeper 验证。

本地 ad-hoc 检查结果：

```text
codesign --verify --deep --strict MuxLayer.app
=> code has no resources but signature indicates they must be present (exit 1)

spctl --assess --type execute MuxLayer.app
=> exit 1
```

内置 `agentgate-serve` 的 ad-hoc 单文件签名可以验证，但整个 `.app` 没有 `_CodeSignature` 资源，属于预期的本地 unsigned 结果。

因此：

- SIGNING：BLOCKED_CREDENTIALS
- NOTARIZATION：BLOCKED_CREDENTIALS
- 不能把本轮 DMG 报告为已签名/已公证

正式发布必须在 CI 使用 Developer ID 签名并验证嵌套 sidecar 的签名/公证链。

## 发布流水线检查

静态检查 `.github/workflows/release.yml`：

- preflight 和 Desktop build 都会在 Tauri 构建前准备内置 CLI
- macOS matrix 包含 aarch64 和 x86_64，打包 `app,dmg`
- Desktop 与 CLI tarball 分别发行
- `publish` 依赖 `updater-json` 和 `cli`
- Homebrew cask 在发布后校验两个 DMG 的 SHA
- tauri-action 使用签名/公证 secrets

未实际运行 CI，未验证正式签名后的 sidecar Gatekeeper 行为。

## 验收 Gate

| Gate | 状态 | 证据 |
| --- | --- | --- |
| BUILD | PASS | 真实 DMG 构建成功，arm64 |
| DMG_INSTALL | PASS | DMG 挂载、复制、启动成功 |
| ONE_INSTALL | PASS | 干净 HOME 下仅安装 Desktop 即可启动 Agent Gateway |
| BUNDLED_CLI | PASS | `.app` 内 sidecar 与稳定副本 SHA 一致 |
| UNIFIED_STARTUP | PASS | Model + Agent 按设置启动；Service 复用 |
| MODEL_GATEWAY | PASS | `/health` 200；未认证 `/v1/models` 401；认证后 200 |
| AGENT_GATEWAY | PASS | 当前候选真实 Codex / Claude run / continue / cancel |
| CODEX_NATIVE | PASS | `AGENTGATE_RC_OK`、`CONTINUE_OK`、真实取消 |
| CLAUDE_CODE | PASS | `CLAUDE_RC_OK`、`CLAUDE_CONTINUE_OK`、DeepSeek 来源、真实取消 |
| AGENT_STATUS | PASS | 配置就绪 / 需要验证 / 测试未通过 / 真实通过分离 |
| MCP_CONFIGURATION | PASS | Pi `connected, 5 tools`；CodeLeveler 配置识别 |
| CODELEVELER | PASS | 真实 `agent_list` + `agent_run/status`；旧版 isError 条件仍保留 |
| PI | PASS | 真实 `agent_run/status`，nvm Node 链正常 |
| SESSION | PASS | Codex / Claude 连续会话成功 |
| PERSISTENCE | PASS | Service 重启后已完成任务与最终回答可读 |
| RECOVERY | PASS | 服务停止后 MCP 自动重新拉起并恢复查询 |
| CANCEL | PASS | Codex / Claude 均到终态 `cancelled` |
| PROCESS_LIFECYCLE | PASS | Desktop 退出后 Service 存活；测试只清理精确 PID |
| READ_ONLY | PASS | 真实任务使用 read_only；测试工作区无文件改动 |
| SECURITY | PASS | 未认证 partial body 立即 401；无 token 401；evil Host 未放行 |
| DESKTOP_UX | PASS | 三步流程、详情折叠、中文错误、960×600 实际验收 |
| UPDATE_COMPATIBILITY | NOT_TESTED | 没有可用的旧版 DMG；稳定路径/重开已验证，正式升级路径未执行 |
| REGRESSION | PASS | Rust 1816、frontend 495、tsc、vite、Tauri build |
| SIGNING | BLOCKED_CREDENTIALS | 无 Apple 凭据；ad-hoc 整包 verify 失败 |
| NOTARIZATION | BLOCKED_CREDENTIALS | 未提交公证请求 |
| RELEASE_PIPELINE | NOT_TESTED | 静态检查通过，未实际运行 CI |

## 已知基线问题

- `protocol_fixture` 仍有 2 个已知失败：
  - `unauthenticated_request_is_rejected_before_body_is_read`
  - `metrics_requires_token_only_and_health_does_not`
  真实 socket 探针显示未认证 partial body 会在 1ms 内返回 401，evil Host + 正确 token 返回 500 而不是成功，未观察到对应绕过；这两个失败是已知测试基线问题，不属于本轮新增回归。
- CLI clippy 仍有 3 个已知 lint（`print_literal`、`unnecessary_map_or`、`too_many_arguments`），未修改。
- CodeLeveler 1.0.16 / 1.0.17 仍不包含 `e4510fa`；UI 保留升级提醒。最低兼容版本应为第一个包含该修复的正式发布。
- 没有旧版 DMG，真实升级路径未验收。

## 结论

功能实现和本地 macOS DMG 安装/运行满足发布候选条件；正式发布仍被签名、公证和真实 CI 执行阻塞。不能把本地 ad-hoc DMG 报告为正式公证安装包。
