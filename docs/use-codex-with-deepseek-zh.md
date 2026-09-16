# 用 MuxLayer 让 Codex 使用 DeepSeek

English: [Use Codex with DeepSeek through MuxLayer](./use-codex-with-deepseek.md)

MuxLayer 把 Codex 原本发往官方 Responses API 的入口，变成你本地可控的模型入口。Codex 继续发 Responses API 请求，MuxLayer 在本地决定转换协议或直连 DeepSeek，附带模型映射、故障转移、请求日志和成本统计。

## 什么时候用这个

如果你想：

- 让 Codex 调用 DeepSeek 模型，同时保留一键回退到官方配置的能力。
- 让 Codex 的 OpenAI Responses API 请求被转换成 DeepSeek 兼容的 Chat Completions，或 Anthropic 兼容 endpoint。
- 在官方 Codex 配置和 MuxLayer 配置之间一键切换。
- 用一个本地网关，以后随时把 Codex 从 DeepSeek 切到 MiMo、OpenAI、Kimi、GLM、DashScope 或其他 Provider。

## 快速配置

1. 从 [Releases](../../releases) 下载 MuxLayer 并打开应用。
2. 进入 **快速配置** 或 **供应商**。
3. 添加 DeepSeek Provider，粘贴你的 DeepSeek API Key。
4. 在 **概览** 或 **网关** 启动网关。默认客户端端点是 `http://127.0.0.1:9090`。
5. 打开 **客户端**，在 Codex 卡片上点 **应用配置**。
6. 在 Codex 里发一条测试消息。

MuxLayer 保留了原 Codex 配置的可恢复状态，你可以随时从 Codex 卡片切回官方。

## MuxLayer 配置了什么

| Codex 侧 | MuxLayer 侧 | DeepSeek 侧 |
|---|---|---|
| OpenAI Responses API | `/v1/responses` 本地网关路由 | DeepSeek Chat Completions 或 Anthropic 兼容 endpoint |
| Codex 模型名 | Model Mapping 或 `muxlayer` 虚拟模型（仍兼容 `agentgate`） | DeepSeek 模型 ID，如 `deepseek-flash` 或 `deepseek-v4-pro` |
| Codex 的工具和流式输出 | 协议转换和请求追踪 | DeepSeek 专属处理 |

## 工作原理

MuxLayer 的作用，是把 Codex 原本发往官方的 Responses API 入口变成本地模型入口，再由本地决定转换或直连到 DeepSeek。

常见路径是：

```text
Codex -> http://127.0.0.1:9090/v1/responses -> MuxLayer -> DeepSeek
```

你不需要长期手改 Codex 配置文件，也不需要在 DeepSeek、MiMo、OpenAI 等 Provider 之间来回改模型名。MuxLayer 会通过 Provider、Route Profile、Model Mapping 和 `muxlayer` 虚拟模型（仍兼容 `agentgate`）处理这些差异。

## 为什么默认不直连 DeepSeek 的 Responses API

`deepseek-flash` 已经正式支持 Responses API，也支持通过 Responses 传入图片。但对普通 Coding 模型来说，直连的 Agent 实际效果仍然更差，因此 MuxLayer 默认继续走协议转换，只对明确支持的模型和工具组合开放原生直连。

原因有四条，都可复现：

**1. 工具会整批丢失，模型转而"编造"工具调用。**
Codex gpt-5.6+ 把工具定义放在请求 `input` 数组的 `additional_tools` 项里，而不是顶层 `tools`。DeepSeek 只读顶层 `tools`，直连时它一个工具都看不到。模型知道自己应该调用工具，却拿不到可用的工具定义，于是在正文里输出裸的 DSML 标记当作调用：

```text
<|DSML|tool_calls>
<|DSML|invoke name="exec_command">
<|DSML|parameter name="cmd" string="true">cat README.md</|DSML|parameter>
```

Codex 解析不了这种正文内容，表现就是模型"开始胡说八道"。

**2. Codex 的 `exec` 工具会被上游直接拒绝。**
DeepSeek 的 Responses API 只接受一个自定义工具 `apply_patch`，其余一律返回 400：

```text
Unsupported custom tool: 'exec'. Only 'apply_patch' is supported.
```

Codex 必发 `exec`，因此即使工具没丢，直连也会被拒。

**3. 跨轮思维链断掉，而且不报错。**
Codex 每轮都会带 `include: ["reasoning.encrypted_content"]`，用于把上一轮的推理链带回下一轮。DeepSeek 明确不支持 `include` 与 `encrypted_content`，且是**静默忽略**——不报错，只是每轮推理都从零开始。转换路径里 MuxLayer 会做 DeepSeek V4 thinking 历史回填来补偿，直连没有这层补偿。

**4. 直连绕过全部 DeepSeek 专属处理。**
对纯文本的 `deepseek-v4-pro`，图片剥离并注入可解释提示；schema 清洗、消息重排、V4 thinking 历史 reasoning 回填都发生在转换路径上。`deepseek-flash` 会保留图片输入；原生直连则会跳过转换层修复。

### 如果你仍然想直连

在 Provider 里手动填 Responses 端点即可，网关会尝试直连。为避免上述问题把请求打废，网关内置两道判定，命中任意一条就自动回落到协议转换：

- 目标模型不在上游 Responses API 的支持列表内（DeepSeek 只支持 `deepseek-flash`）。
- 请求里带了上游不接受的自定义工具（DeepSeek 仅接受 `apply_patch`），包括 Codex 0.152+ 放在 `functions` 命名空间里的 `exec`。

也就是说，Codex 即使配了直连端点，实际仍会走转换路径。这是有意为之，不做进一步处理。

## 排查

| 现象 | 检查 |
|---|---|
| Codex 还在调用官方 endpoint | 回到 **客户端**，重新应用一次 Codex 配置。 |
| DeepSeek 返回模型错误 | 检查 DeepSeek Provider 的默认模型和 Model Mapping。 |
| 网关无法连接 | 确认 MuxLayer 网关在 `127.0.0.1:9090` 上运行；`1420` 只是开发用的 UI 端口。 |
| 想恢复官方 Codex | 在 **客户端** 页用 Codex 卡片上的切回官方动作。 |

## 相关教程

- [让 Codex Desktop 使用第三方 API 并保留插件能力](./use-codex-desktop-with-third-party-api-and-plugins-zh.md)
- [让 Codex 使用小米 MiMo](./use-codex-with-mimo-zh.md)
- [English README](../README.md)
- [中文 README](../README_ZH.md)
