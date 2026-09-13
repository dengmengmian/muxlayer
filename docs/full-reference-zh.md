<p align="center">
  <img src="logo.svg" width="128" height="128" alt="MuxLayer Logo">
</p>

<h1 align="center">MuxLayer</h1>

English: [Main Reference](./full-reference.md)

<p align="center">
  <b>一个本地入口，统一管理你的 AI 模型请求。</b><br>
  官方客户端请求先进入本地网关：协议转换、原生直连、Provider 路由、失败转移、成本统计和请求追踪。
</p>

<p align="center">
  <a href="https://github.com/dengmengmian/muxlayer/releases"><img src="https://img.shields.io/github/v/release/dengmengmian/muxlayer?style=flat-square&color=blue" alt="Release"></a>
  <a href="https://github.com/dengmengmian/muxlayer/stargazers"><img src="https://img.shields.io/github/stars/dengmengmian/muxlayer?style=flat-square&cacheSeconds=3600" alt="Stars"></a>
  <a href="https://github.com/dengmengmian/muxlayer/releases"><img src="https://img.shields.io/github/downloads/dengmengmian/muxlayer/total?style=flat-square&color=green&cacheSeconds=3600" alt="Downloads"></a>
  <a href="../LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
</p>

<p align="center">
  <a href="../README.md">English</a> · <a href="https://github.com/dengmengmian/muxlayer/releases">下载</a> · <a href="#5-分钟快速上手">5 分钟快速上手</a> · <a href="./use-codex-with-deepseek-zh.md">Codex + DeepSeek</a> · <a href="./use-claude-code-with-deepseek-zh.md">Claude Code + DeepSeek</a> · <a href="./use-gemini-cli-with-muxlayer-zh.md">Gemini CLI</a>
</p>

<p align="center">
  <img src="demo-header-v2.gif" width="800" alt="MuxLayer 把 AI Agent 模型请求接到本地网关：转换、直连、路由、追踪">
</p>

## 按系统下载

| 你的机器 | 下载 |
|---|---|
| 🍎 macOS — Apple Silicon (M1–M4) | [MuxLayer 2.0.6](https://github.com/dengmengmian/muxlayer/releases/download/v2.0.6/MuxLayer_2.0.6_aarch64.dmg) |
| 🍎 macOS — Intel | [MuxLayer 2.0.6](https://github.com/dengmengmian/muxlayer/releases/download/v2.0.6/MuxLayer_2.0.6_x64.dmg) |
| 🪟 Windows 10 / 11 | [MuxLayer 2.0.6](https://github.com/dengmengmian/muxlayer/releases/download/v2.0.6/MuxLayer_2.0.6_x64-setup.exe) |
| 🐧 Linux — Debian / Ubuntu | [MuxLayer 2.0.6](https://github.com/dengmengmian/muxlayer/releases/download/v2.0.6/MuxLayer_2.0.6_amd64.deb) |
| 🐧 Linux — 其他发行版 | [MuxLayer 2.0.6](https://github.com/dengmengmian/muxlayer/releases/download/v2.0.6/MuxLayer_2.0.6_amd64.AppImage) |

macOS 也可以用 Homebrew 安装：

```bash
brew install --cask dengmengmian/tap/muxlayer
```

> 无界面 CLI（`agentgate-serve`）的 tarball 包和所有历史版本：[Releases](https://github.com/dengmengmian/muxlayer/releases)

**Windows 安装提示：** Edge/Chrome 可能提示「通常不会下载」，SmartScreen 可能提示「Windows 已保护你的电脑」。这是预期行为：安装包未做 Authenticode 代码签名。没有「免费且被 Windows 默认信任」的代码签名证书（Let’s Encrypt 只签网站 HTTPS，不能签 `.exe`），开源项目常先发未签名包——不是病毒结论。请只从官方 Releases 下载；浏览器选 **保留**，运行时点 **更多信息** → **仍要运行**。

## 为什么用 MuxLayer

| 不破坏官方体验 | 模型路由归你 | 每一条请求都看得到 |
|:---|:---|:---|
| AI agent 客户端继续按它习惯的方式工作，一键还原到官方配置 | 让官方客户端请求先进 MuxLayer，再决定转换协议或 pass-through 到你选的上游 | 路由决策、转换后的 payload、上游错误、token、成本、延迟、故障转移 尝试一条都不漏 |

## 5 分钟快速上手

1. 在上面那张表里下载 MuxLayer，装好。
2. 进入 **快速配置** 或 **供应商**，粘贴你的 Provider API Key。
3. 在 **概览** 或 **网关** 点 **启动网关**。默认客户端端点是 `127.0.0.1:9090`。
4. 在 **客户端** 页，对 Codex / Claude Code / OpenCode / Gemini CLI / AtomCode 点 **应用配置**。
5. 在客户端里发一条测试消息。任何时候想恢复原客户端配置，点 **切换到官方** 就行。

MuxLayer 会从 Provider 预设里自动填好常见的 base URL、协议、默认模型和能力矩阵（每个模型能干什么的清单：文本、图像、工具调用、推理等）。大多数人一开始都不需要碰 Model Mapping 或高级 endpoint 字段。

---

MuxLayer 是一个 **给 AI 应用和客户端模型请求用的本地网关**。它把原本要直接发给官方 endpoint 的模型请求先接进你的桌面，再决定是做协议转换，还是原生直连到 25+ 家模型供应商里的某一个——包括小米 MiMo、DeepSeek、OpenAI、Anthropic、GitHub Copilot、Kimi、GLM、DashScope、SiliconFlow、Volcengine 等等。

> **一个本地入口，统一管理你的 AI 模型请求。** Codex、Claude Code、Gemini CLI、OpenCode、AtomCode，以及兼容 OpenAI / Anthropic / Gemini 协议的应用继续按它们熟悉的方式跑，MuxLayer 在本地处理上游选择、协议差异、故障转移、成本和可追溯性。

![成本看板](screenshots/dashboard.png)

它是冲着真实集成痛点来的：

- Codex 说的是 Responses API，但很多 Provider 只暴露 Chat Completions 或 Anthropic 兼容的 Messages。
- Codex Desktop 的插件和账号能力依赖官方 OpenAI 认证 Provider 路径，第三方 API 代理常把这条路打断。
- Claude Code 能直接用 DeepSeek / MiMo 的 Anthropic 兼容 endpoint，但 base URL、模型名、映射规则一不留神就配错。
- 同一个 Provider 下的不同模型能力差别很大；把图片、工具或 `web_search` 发给错的模型，经常 400。
- 多 Provider + 多 API Key 应该能自动 故障转移，并且有请求日志、token 统计、成本追踪。
- 切模型不该意味着手改 `~/.codex/config.toml` 或 `~/.claude/settings.json`。

MuxLayer 的事就一句话：**让官方客户端的入口在本地可控**——客户端一键应用 / 还原、需要时做协议转换、能 pass-through 就 pass-through、Route Profile、故障转移、请求日志、成本追踪、诊断。

## 横向对比

业界有不少 LLM 代理工具。MuxLayer 的位置是 **桌面上的本地 AI 模型入口**——它专注于保持客户端原本的行为，同时把模型入口搬进一个你本地可控的 GUI，而不是去运营一个共享 API 服务。

| 工具 | 它最擅长的事 | MuxLayer 的差异 |
|---|---|---|
| **普通代理** | 换一个 base URL | 保留客户端专属行为，需要时转换协议，支持原生 pass-through，并能追踪每次请求的完整路径 |
| **claude-code-router** | 把 Claude Code（CLI）路由到其他模型 | 还覆盖 Codex 的 Responses API、Gemini CLI、OpenCode——并有 GUI 和成本看板 |
| **one-api / new-api** | 服务端多用户 API 转售和计费 | 本地优先、单用户、没账号体系；客户端一键配置内建 |
| **LiteLLM** | 一个 Python SDK / proxy，在自己 app 里接 100+ LLM | 一个给 AI 应用和客户端用的桌面网关，不是库——零代码、GUI 驱动 |

> 这只是大致定位，工具都在快速演进——按你的工作流挑。如果你在运营一个共享 API 服务，one-api / LiteLLM 可能更合适；如果你日常住在 Codex / Claude Code 里，那这工具就是为你做的。

## 常见用途

教程：[Codex Desktop 插件](./use-codex-desktop-with-third-party-api-and-plugins-zh.md) · [Codex + DeepSeek](./use-codex-with-deepseek-zh.md) · [Codex + 小米 MiMo](./use-codex-with-mimo-zh.md) · [Claude Code + DeepSeek](./use-claude-code-with-deepseek-zh.md) · [Claude Code + GitHub Copilot](./use-claude-code-with-github-copilot-zh.md) · [Gemini CLI](./use-gemini-cli-with-muxlayer-zh.md) · [OpenCode](./use-opencode-with-muxlayer-zh.md)

| 目标 | MuxLayer 做的事 |
|---|---|
| Codex 用 DeepSeek | 把 Codex 的 OpenAI Responses API 请求转成 DeepSeek 兼容的 Chat Completions 或 Anthropic 兼容 endpoint。 |
| Codex 用小米 MiMo | 把 Codex 的 Responses 入口变成本地的 MuxLayer 入口，再路由到 MiMo 模型，带 Model Mapping、推理能力和能力检查。 |
| 用 GitHub Copilot 订阅跑 Claude Code | 自动用你的 GitHub token 换 Copilot 凭据，并把工具续聊 / 历史压缩打上 agent 流量标签，不消耗 premium request 配额。看 [专门那一节](#可选用-github-copilot-订阅跑-claude-code--codex)。 |
| 在小上下文模型上跑长会话 | 历史超过模型上下文窗口时，网关会自动对话的中间段做总结（系统消息和最近几轮原样保留）——一个 128K 窗口的模型也能撑过 300K+ token 的会话。 |
| Codex Desktop 插件 + 第三方 API | 让 Codex Desktop 保持在它的官方 OpenAI 认证 Provider 路径上，插件和账号能力不受影响，模型请求照样经过 MuxLayer。 |
| Claude Code 用 DeepSeek / MiMo | 走 Anthropic 兼容 pass-through，加上针对 DeepSeek 和 MiMo endpoint 的 Model Mapping。 |
| Codex 在多个 Provider 之间切换 | 一个本地 endpoint，让 Codex 在 DeepSeek、MiMo、OpenAI、Kimi、GLM、DashScope 等 Provider 之间切换，不需要手改配置文件。 |

<a id="可选用-github-copilot-订阅跑-claude-code--codex"></a>

## 可选：用 GitHub Copilot 订阅跑 Claude Code / Codex

如果你有 Copilot 订阅（Pro / Business），Claude Code 可以跑在它包含的 Claude 模型上——**不需要单独的 Anthropic API 计费**。MuxLayer 干三件事：

1. **凭据交换**：你提供一个 GitHub OAuth token（`gho_...`）；网关自动换出并续期 Copilot API 凭据（按哈希缓存，绝不存明文）。
2. **Premium request 优化**：大多数 agent 工作流请求其实是工具结果续聊和历史压缩——MuxLayer 给这些请求打上 `x-initiator: agent` 标签，所以 **只有你真正发出去的消息才消耗 premium request**。一条指令加 10 轮工具往返，只算 1 次。
3. **模型名归一化**：Claude Code 里的 `claude-sonnet-4-6` 会被改写成 Copilot endpoint 接受的 `claude-sonnet-4.6`——不用配映射。

**步骤：**

1. 拿一个 GitHub token：如果你已经登录 VS Code Copilot，从 `~/.config/github-copilot/apps.json` 读 `oauth_token`。
2. MuxLayer → **供应商** → 添加，类型选 **GitHub Copilot**，把 `gho_` token 当 API Key 粘贴（base URL 和模型会自动填）。
3. 在 **客户端** 页应用 Claude Code 配置，开始聊。**日志** 页会显示每条请求的 `x-initiator` 分类。

> ⚠️ **风险告知**：在官方客户端之外使用 Copilot 订阅，在 GitHub 服务条款里属于灰色地带。类似的社区工具存在很久了也没看到大规模封禁，但 **账号风险不能完全排除——自己评估**，重要的公司账号别上。这个能力完全是 opt-in，你不添加 copilot 类型的 Provider，就跟你没关系。

## 三种模式

| 模式 | 触发时机 | 模型处理 | 典型场景 |
|---|---|---|---|
| **协议转换** | 客户端协议和上游协议不一样 | Model Mapping 优先；没匹配上就用 Provider 默认模型做兼容兜底 | Codex Responses → DeepSeek / MiMo Chat |
| **原生 Pass-through** | 客户端协议和某个上游原生 endpoint 一致 | 请求的 `model` 原样保留，除非命中 Model Mapping；虚拟模型 `muxlayer`（仍兼容 `agentgate`）会解析成路由选中的那个模型 | OpenCode / curl → Chat Completions |
| **Pass-through + Model Mapping** | 协议一致，但客户端模型名和上游不一样 | Model Mapping 改写 `model` | Claude Code 的 `claude-*` → DeepSeek / MiMo 模型 |

经验法则：**协议匹不匹决定走 pass-through 还是转换；Model Mapping 只负责改名。** 对一键客户端集成来说，`muxlayer` 是个虚拟模型名，意思是"这一条请求让 MuxLayer 自己挑 Provider 的模型"。旧配置里的 `agentgate` 仍然有效。

## 功能

**协议转换 — 4 种格式，双向**
- OpenAI Responses API（`/v1/responses`）→ Chat Completions / Claude Messages / Gemini API，给 Codex 用
- Anthropic Messages API（`/v1/messages`）→ Chat Completions 转换 / Anthropic pass-through，给 Claude Code 用
- Google Gemini API（`/v1beta/models/:model:generateContent`）→ Chat Completions 转换，给 Gemini CLI 用
- Chat Completions（`/v1/chat/completions`）pass-through 转发
- 原生 Anthropic Claude API：`tool_use`/`tool_result`、`input_schema`、`thinking.budget_tokens`
- 原生 Gemini API：`contents`/`functionCall`/`functionResponse`、`generationConfig`
- 完整支持 DeepSeek 的 reasoning_content（思考模式），不降级
- 自动请求重试（429/5xx、指数退避、Retry-After）
- **长历史自动压缩**：历史超过模型上下文窗口时（自适应阈值在编目窗口的 85% 处，可按模型覆盖），网关会总结对话中段，系统消息和最近几轮原样保留——小窗口模型也能撑过很长的会话，不再 400
- **质量优先的 thinking**：转换到 Claude 模型时，只要模型支持就开 thinking（新模型走 adaptive，老模型走 budget），同时守住 Anthropic 的限制（budget 上下界、采样参数、强制 tool choice），免得 400
- **自动注入 prompt cache**：发往 Anthropic 的请求（无论转换还是 pass-through）会在 tools / system / history 上加 `cache_control` 断点——按预算分配，绝不超 4 个断点上限；缓存省下来的钱直接体现在成本看板上

**成本追踪 & 多 Key 池化**
- 内建 22+ 模型价格，每条请求自动算钱
- 看板：总成本 / 今日 / 平均成本卡片，加上按 **模型、客户端、路由** 的成本拆分，限定时间窗（7/30 天）
- **设置** 里支持就地改价、自定义价格覆盖
- 每个 Provider 多 API Key：轮询，429 时自动切换
- **计价规则：** 默认来自 catalog 的 `models[].pricing`（美元 / 百万 token）。设置里的自定义价覆盖同名 catalog。查找顺序：精确自定义 → 精确 catalog → 去掉 `[1m]` 后缀 → 该供应商 `*` 通配 → 跨供应商按模型名（含去掉 `vendor/` 前缀）。**看板上的 `$0` 不是免费**，除非该模型有明确的 `0/0` 标价；缺价显示「无价格 / 去补价」，补价后下次启动会重算历史行（`backfill_costs`）。本地 Ollama / LM Studio 是未标价，不当成 $0。

**智能路由**
- 任务级路由条件：按输入大小、是否有图、是否有工具、系统关键词、模型名匹配来路由。同一套条件对 Codex Responses、Chat Completions、Anthropic Messages、Gemini 都生效。
- 可选日预算闸（仅提醒 / 拦截 / 强制最便宜）覆盖上述全部入口，包括 Gemini `generateContent`
- 每条路由的 故障转移 选择策略：**优先级**（默认）/ **最便宜**（按模型单价）/ **最快**（按最近网关延迟）
- 预置场景：图片请求 / 推理 / 后台 / 长文 / 工具密集
- Anthropic 自动注入 prompt cache（`cache_control` 自动加，约 90% 输入成本省下来）
- Provider 测试时自动检测是否支持 cache

**网关 Refiner 层（可选，默认关）**
- 默认情况下，网关按字节原样转发请求 / 响应；除非你显式开，否则不会改写任何内容
- 三个独立的 refiner，每个都有一个全局总开关在 **设置** 里，每个 Provider 还可以覆盖成强制关（不能强制开）：
  - **请求字段过滤**：根据每个 Provider 的怪癖，剥掉它会拒收的请求字段，避免 400
  - **推理参数纠正**：把 `thinking.budget_tokens` / `reasoning.effort` 归一化成该 Provider 接受的格式和范围
  - **错误响应归一化**：把上游的错误体改写成客户端（Codex / Claude Code / Gemini CLI）期望的形状
- Provider 怪癖有内建默认值，可以按 Provider 覆盖；每次 refiner 的动作都记录在请求日志的 `trace_json` 里

**多模态支持 & 按模型的能力矩阵**
- 协议转换过程中图片完整保留（`input_image`/`image_url` → Chat Completions 的 `image_url`、Anthropic 的 `image source` 格式）
- 按模型的能力矩阵，每个模型 8 个维度：`text` / `vision` / `audio_in` / `tts` / `video_in` / `reasoning` / `tools` / `web_search`
- 能力感知的升级：请求带图时，网关自动换到同 Provider 下支持 vision 的同系列模型（比如把图片请求路由到 `mimo-v2.5` 而不是 `mimo-v2.5-pro`）
- 升级时按"保留原模型其他能力最多"来排序候选，`supported_models` 的顺序作为决胜局
- 能力矩阵还管工具下发：把某模型的 `web_search` 取消勾选，网关就不会再给它发这个内建工具
- 矩阵根据模型名模式自动初始化：MiMo / DeepSeek / Kimi / Moonshot 有内建规则，其他走通用兜底
- Provider 测试按钮现在合并了连通性检查 + 非破坏性矩阵自动填充（手工编辑会保留）
- 故障转移 模式下，带图请求会跳过那些矩阵里没有 vision 模型的 Provider
- 所选模型不支持图片的 Provider，会在 Provider 专属层剥掉图片内容，避免上游 400/404

**多 Provider 管理**
- **27 个内建预设**（自动填 base URL / 协议 / Anthropic endpoint / 默认模型）：
  - **国内**：小米 MiMo、DeepSeek、Kimi/Moonshot、MiniMax、GLM（智谱 BigModel）、DashScope（阿里通义）、SiliconFlow、Volcengine（豆包）、百川、StepFun、商汤 SenseNova、ModelScope、Yi（零一万物）
  - **海外**：OpenAI、Anthropic（Claude）、GitHub Copilot、Google Gemini、xAI（Grok）、Mistral、Groq、Together、Fireworks、Cerebras、Perplexity、Cohere
  - **聚合**：OpenRouter、OrcaRouter
  - **自定义**：任何 OpenAI 兼容的 endpoint（vLLM / Ollama / LiteLLM / 本地代理）
- MiMo 一等公民支持：5 个 chat 模型（`mimo-v2.5-pro` / `mimo-v2-pro` / `mimo-v2.5` / `mimo-v2-omni` / `mimo-v2-flash`）、多轮 `reasoning_content` 往返、`sk-*` / `tp-*` Key 自动路由到 Open API 或 Token Plan host、按区域的 Token Plan URL（`cn` / `sgp` / `ams`）、付费插件不可用时自动降级 `web_search`
- Claude Code 经 MiMo / DeepSeek 的 pass-through 默认用普通 Provider 模型 ID；MuxLayer 不再自动配置 `[1m]` 后缀的模型。
- Route Profile 支持多 Provider 优先级链，按协议自动匹配
- 手动切换或自动 故障转移
- Provider 冷却和运行状态追踪
- 单请求级别 故障转移：Provider A 失败 → 自动尝试 Provider B
- 能力感知路由：带图 / 音频 / 等的请求，在一个 Provider 内自动路由到能跑的模型，整条链里再回退到有能力的 Provider
- 新加的 Provider 自动加入所有路由链
- 自动从 Provider 拉模型列表
- 连接稳定性：HTTP 客户端调优过 `pool_idle_timeout` 和 `tcp_keepalive`，应用层对瞬时连接 / 超时错误有重试（避免一段时间不用之后陈旧的 keep-alive 失败）

**客户端配置**
- Codex：一键配置 + 在官方和 MuxLayer 之间切换（会话保留）
- Codex Desktop 兼容：把模型请求路由到第三方 API，同时保留官方 OpenAI Provider 路径、登录账号状态、插件和账号能力
- Claude Code：一键配置 + 在官方和 MuxLayer 之间切换
- OpenCode：一键配置
- Claude Desktop（macOS / Windows）：把它的第三方推理网关指向 MuxLayer；一键应用，带历史回滚
- 全局指令文件：在 MuxLayer 内编辑 `~/.claude/CLAUDE.md` / `~/.codex/AGENTS.md`，按用途分组的 6 个内建模板（general / coding / review / debug / security / docs）；覆盖或追加，自动快照，一键回滚，JSON 备份 / 恢复
- MCP 服务器：一个面板搞定 Codex 和 Claude Code 的 MCP 服务器配置的读、加、改、删、同步；env 值在列表里不显示；JSON 导入 / 导出默认不带 Key
- 本地 **技能**：列出、启用 / 禁用、删除 `~/.claude/skills` 和 `~/.codex/skills` 下的技能；从本地 `.zip` 安装（防 zip-slip，不联网下载），JSON 备份 / 恢复
- 本地网关 access token（`ag_local_*`）认证

**出站 HTTP 代理**
- 设置 → **出站 HTTP 代理**：让访问上游供应商的请求走本机 `http://` 或 `https://` 代理（Clash / V2Ray）。不支持 SOCKS。
- 回环地址（`127.0.0.1` / `localhost` / `::1`）始终直连，本机探针不会绕出去。
- 开关关闭：仍尊重环境变量 `HTTP(S)_PROXY`。
- 地址为空：开关不会生效。填了 URL 才会走设置里的代理。改这项会重启正在运行的网关。

**桌面体验**
- 窗口关闭后系统托盘后台运行
- 开机自启、应用内更新、中英双语 UI、8 个内建主题
- 可选桌面宠物显示网关状态、请求统计、错误气泡；可以在 **设置** 里关掉

**快速配置 & 诊断**
- 首次启动引导：粘贴 API Key → 自动识别 Provider → 选工具 → 一键配置
- 快速加 Provider：粘贴 API Key，已知前缀自动识别，模糊前缀可手选
- 连接测试：**客户端** 页 3 步状态栏（配置 → 网关 → Provider）
- 侧栏的 **快速配置** 页（配置完 Provider 后自动隐藏，在 **设置** 里可重新打开）

**诊断 & 可观测性**
- 请求日志、token 统计、成本预估、Provider 运行状态
- Provider 失败状态在卡片上可见：冷却 / 连续失败 / 配额耗尽，一键重置
- 主动健康探测（可选，默认关，因为探测会消耗几个 token）：每个 Provider 周期性发最小探针，结果显示在卡片上；开启后会顺便给"最快"路由策略提供冷启动延迟基线（最近没流量的 Provider 不再傻乎乎排在最后）
- **自检** 和脱敏诊断包导出
- 能力降级事件：剥图、`web_search` 降级、MCP 提示、忽略工具输出图片

## 截图

| 概览 | 供应商 |
|:---:|:---:|
| ![概览](screenshots/dashboard.png) | ![供应商](screenshots/providers.png) |

| 路由 | 网关 |
|:---:|:---:|
| ![路由](screenshots/routes.png) | ![网关](screenshots/gateway.png) |

| 客户端 | 日志 |
|:---:|:---:|
| ![客户端](screenshots/tools.png) | ![日志](screenshots/logs.png) |

| 诊断 | 设置 |
|:---:|:---:|
| ![诊断](screenshots/diagnostics.png) | ![设置](screenshots/settings.png) |

| 全局指令 | MCP 服务器 |
|:---:|:---:|
| ![全局指令](screenshots/instructions.png) | ![MCP 服务器](screenshots/mcp.png) |

| 技能 | 快速配置 |
|:---:|:---:|
| ![技能](screenshots/skills.png) | ![快速配置](screenshots/quick-setup.png) |

| 宠物设置 | 桌面宠物 |
|:---:|:---:|
| ![宠物设置](screenshots/pet-settings.png) | ![桌面宠物](screenshots/pet.png) |

## 技术栈

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri v2 |
| 前端 | React 19 + TypeScript + Tailwind CSS v4 |
| 后端 | Rust + Tokio + Axum |
| 数据库 | SQLite（rusqlite，WAL 模式） |
| HTTP 客户端 | reqwest |

## 安装与构建

### 下载

从 [Releases](../../releases) 页拿你平台的安装包。

| 平台 | 格式 |
|---|---|
| macOS（Apple Silicon） | `.dmg` (aarch64) |
| macOS（Intel） | `.dmg` (x86_64) |
| Windows | `.exe` |
| Linux | `.AppImage` / `.deb` |

> **平台支持**：核心网关能力（协议转换、路由、故障转移、成本看板、客户端配置应用 / 还原）三个平台都可以。便利功能（Codex 应用配置后自动重启、Claude Desktop 集成、运行中客户端检测）支持 macOS 和 Windows（Windows 实现刚上不久——遇到怪现象请提 issue）；Linux 上应用完配置请手动重启客户端。欢迎贡献代码。

<details>
<summary><b>macOS: 提示"无法验证开发者"?</b>（点击展开）</summary>

如果被 macOS Gatekeeper 拦了，三种方法任选其一：

**方法 1：系统设置（推荐）**
1. 双击 MuxLayer，在提示框上点 **取消**
2. 打开 **系统设置 → 隐私与安全性**
3. 往下滚，找到 `"MuxLayer" 已被阻止` → 点 **仍要打开**
4. 再打开 MuxLayer，点 **打开**

**方法 2：右键打开**
1. 在访达里找到 MuxLayer.app
2. 按住 **Control** 点击（或右键）→ 选 **打开**
3. 在提示框上点 **打开**

**方法 3：终端**
```bash
xattr -d com.apple.quarantine /Applications/MuxLayer.app
```

> 只需要做一次。

</details>

<details>
<summary><b>Windows SmartScreen / 浏览器「通常不会下载」?</b>（点击展开）</summary>

安装包未做 Authenticode 代码签名，首次下载和运行常会遇到两层提示：

1. **浏览器（Edge/Chrome）：**「通常不会下载」→ 点 ⋯ → **保留** / **仍要保留**。
2. **SmartScreen：**「Windows 已保护你的电脑」→ **更多信息** → **仍要运行**。

**为什么未签名？** 商用 Windows 代码签名证书需要付费；不存在像 Let’s Encrypt 那样免费且被系统默认信任的代码签名（Let’s Encrypt 只覆盖网站 HTTPS，不能签安装包）。MuxLayer 目前选择未签名发布，并在文档中说明如何通过上述提示。

请只从 [官方 GitHub Releases](https://github.com/dengmengmian/muxlayer/releases) 下载。这是签名/信誉拦截，不是已确认的病毒结论。同一台机器通常只需确认一次。

</details>

### 从源码构建

**前置依赖**

- Node.js >= 20
- pnpm >= 10
- Rust >= 1.75
- macOS / Windows / Linux

**安装依赖**

```bash
pnpm install
```

**开发模式**

```bash
pnpm tauri dev
```

**打包**

```bash
pnpm tauri build
```

## 无界面 / 服务端模式

无 GUI 运行 MuxLayer——服务器、CI、Docker、团队部署都能用。

```bash
# 添加 Provider
agentgate-serve provider-add -t deepseek -k sk-xxx

# 启动网关
agentgate-serve serve --host 0.0.0.0 --port 9090

# 其他命令
agentgate-serve provider-list          # 列出所有 Provider
agentgate-serve provider-remove NAME   # 删除 Provider
agentgate-serve token                  # 显示 access token
agentgate-serve status                 # 显示配置摘要
```

Provider 预设会自动填好 base URL 和模型，覆盖：`deepseek`、`openai`、`anthropic`、`kimi`、`minimax`、`groq`、`together`、`google_gemini`、`xai`、`mistral`。

**Docker：**

```bash
docker compose up
# 或者
docker build -t agentgate . && docker run -p 9090:9090 \
  -e AGENTGATE_PROVIDER=deepseek -e AGENTGATE_API_KEY=sk-xxx agentgate
```

**环境变量：**优先使用 `MUXLAYER_DB_PATH`、`MUXLAYER_TOKEN`。`AGENTGATE_DB_PATH`、`AGENTGATE_TOKEN` 仍是兼容别名；迁移周期内其余已有 `AGENTGATE_*` 服务变量继续有效。新的无界面安装使用 `~/.muxlayer`；如果已有 `~/.agentgate` 数据库或 token，会自动接管。

## 使用指南

大多数人看完上面的 [5 分钟快速上手](#5-分钟快速上手) 就够了。下面是完整说明，按需展开。

<details>
<summary><b>完整使用指南——Provider、客户端、API 调用、故障转移、能力路由、诊断</b></summary>

### 1. 添加 Provider

打开 MuxLayer → **供应商** → **添加 Provider**

**快通道（推荐）—— 粘贴 API Key：**

1. 把 Provider 的 API Key 粘进最上面的输入框
2. MuxLayer 识别已知的 Key 前缀（`sk-ant-` / `deepseek-` / `gsk_` / ……）。前缀模糊的话，手动选一下 Provider 类型
3. 点 **创建**——名字、base URL、协议、默认 / 推理模型、能力都自动填好。结束。

**手动模式 —— 3 段，只有第一段必填：**

| 段 | 字段 | 说明 |
|---|---|---|
| **基础** | 类型 · 名字 · API Key（仅 `custom` 类型还要填 Base URL） | 选完类型，其余字段都从预设里自动填 |
| **模型 & 能力** | 默认模型 · 推理模型 · `拉取并探测` 按钮 · 能力矩阵开关 | 新建 Provider 时这一段 **会在后台自动跑**——不点任何按钮就能拿到最新的非 mini 模型当默认 + 最新推理模型 + 按模型能力矩阵 |
| **高级**（折叠，"一般不用碰"） | 协议和它们的 endpoint（Chat / Responses / Anthropic）· 额外 Header · 超时 · 自动 cache control · Model Mapping | 勾哪个协议就显示哪个 URL——一眼能看出"这个上游原生支持哪几个 endpoint" |

**Model Mapping** 摆在 **高级** 最下面是有原因的：一般不需要。MuxLayer 在你创建 Provider、拉模型、测试 Provider、应用 Codex / Claude Code 配置时，会自动填上推荐的 MiMo / DeepSeek 映射，已有的映射会保留。原生 pass-through 默认不改 `model`，除非命中映射或客户端发的是虚拟模型 `muxlayer`（仍兼容 `agentgate`）；协议转换会优先用映射，再退回到 `default_model` 做兼容，覆盖 Codex、Claude Code、Gemini CLI 这些客户端。

**Provider 配置示例：**

<details>
<summary>DeepSeek</summary>

| 字段 | 值 |
|---|---|
| 名字 | `DeepSeek` |
| 类型 | `deepseek` |
| Base URL | `https://api.deepseek.com` |
| 默认模型 | `deepseek-v4-flash` |
| 推理模型 | `deepseek-v4-pro` |
| 视觉模型 | `deepseek-v4-flash-vision-exp` |
| Model Mapping | `gpt-5.5` → `deepseek-v4-flash`，`o3` → `deepseek-v4-pro` |
| Anthropic Endpoint | `https://api.deepseek.com/anthropic`（支持 Claude Code pass-through） |

</details>

<details>
<summary>KimiCoding（Moonshot）</summary>

| 字段 | 值 |
|---|---|
| 名字 | `KimiCoding` |
| 类型 | `kimi` |
| Base URL | `https://api.moonshot.cn` |
| Anthropic Base URL | `https://api.moonshot.cn/anthropic` |
| 默认模型 | `kimi-k3` |
| 额外 Header | `{"User-Agent":"KimiCLI/1.40.0"}` |
| Model Mapping | `gpt-5.5` → `kimi-k3` |

> KimiCoding 支持 Vision，可以作为图片请求的 故障转移 目标。

</details>

<details>
<summary>OpenAI</summary>

| 字段 | 值 |
|---|---|
| 名字 | `OpenAI` |
| 类型 | `openai` |
| Base URL | `https://api.openai.com` |
| 默认模型 | `gpt-4o` |
| Responses API Endpoint | `https://api.openai.com`（OpenAI 原生支持 Responses API，走 pass-through） |
| Model Mapping | 一般不需要（直接用客户端的模型名） |

</details>

<details>
<summary>Anthropic（Claude）</summary>

| 字段 | 值 |
|---|---|
| 名字 | `Anthropic` |
| 类型 | `anthropic` |
| Base URL | `https://api.anthropic.com` |
| 默认模型 | `claude-sonnet-4-6` |
| Model Mapping | `gpt-5.5` → `claude-sonnet-4-6` |

> 类型设为 `Anthropic (Claude)` 时，Codex 的请求会用 Claude Messages API 原生格式（`tool_use`/`tool_result`/`input_schema`）转换，而不是转成 Chat Completions。

</details>

<details>
<summary>MiniMax</summary>

| 字段 | 值 |
|---|---|
| 名字 | `MiniMax` |
| 类型 | `minimax` |
| Base URL | `https://api.minimax.chat` |
| 默认模型 | `MiniMax-M1` |
| Model Mapping | `gpt-5.5` → `MiniMax-M1` |

</details>

<details>
<summary>OpenRouter</summary>

| 字段 | 值 |
|---|---|
| 名字 | `OpenRouter` |
| 类型 | `openrouter` |
| Base URL | `https://openrouter.ai/api` |
| 默认模型 | `deepseek/deepseek-v4-flash` |
| Model Mapping | `gpt-5.5` → `deepseek/deepseek-v4-flash` |

</details>

<details>
<summary>OrcaRouter</summary>

| 字段 | 值 |
|---|---|
| 名字 | `OrcaRouter` |
| 类型 | `orcarouter` |
| Base URL | `https://api.orcarouter.ai/v1` |
| 默认模型 | `orcarouter/auto` |
| Key 前缀 | `sk-orca-`（自动识别） |

</details>

<details>
<summary>Custom OpenAI Compatible</summary>

| 字段 | 值 |
|---|---|
| 名字 | 自定义 |
| 类型 | `custom_openai_compatible` |
| Base URL | 你自己的服务地址，例如 `http://localhost:8000` |
| 默认模型 | 你的模型名 |

> 任何 OpenAI Chat Completions API 兼容服务都行（vLLM、Ollama、LiteLLM 等）。

</details>

**保存之后：**

- 拉取 + 能力探测会在后台自动跑——不用手点
- 点 **测试连接** 验证配置——会弹出一个对话框，3 步实时进度：**连通性 & 鉴权** → **能力自动填充** → **Prompt cache 检测**（非 Anthropic 的 Provider 会自动跳过）

### 2. 启动网关

**概览** 或 **网关** 页 → **启动网关**

默认监听 `127.0.0.1:9090`。

### 3. 配置 Codex

**客户端** → **Codex** → **应用配置**

MuxLayer 会自动：

- 保存原来的 `~/.codex/config.toml` 和 `auth.json`
- 写入 MuxLayer 的 Provider 配置和本地 token

任何时候点 **切换到官方** 都能恢复原配置——会话不会丢。

### 4. 配置 Claude Code

**客户端** → **Claude Code** → **应用配置**

MuxLayer 会写 `~/.claude/settings.json`，把 `ANTHROPIC_BASE_URL` 指向本地网关，`ANTHROPIC_API_KEY` 设成 MuxLayer 的本地 token。

点 **切换到官方** 恢复原 settings.json。

### 5. 配置 OpenCode

**客户端** → **OpenCode** → **应用配置**

MuxLayer 写 `~/.config/opencode/opencode.json`，配一个指向本地网关的 OpenAI 兼容 Provider。模型用 `openai/muxlayer` 这个虚拟名，这样以后在 MuxLayer 里换 Provider，不需要再改 OpenCode。旧配置里的 `openai/agentgate` 仍然有效。

### 6. 配置 Gemini CLI

**客户端** → **Gemini CLI** → **应用配置**

MuxLayer 把 Gemini CLI 的配置写成指向本地网关的 `/v1beta/...` 路由（Gemini 兼容）。一键切回官方。

### 7. 配置 AtomCode

**客户端** → **AtomCode** → **应用配置**

AtomCode 集成写它的配置文件，把 MuxLayer 当成上游——切换模式跟其他客户端一样。模型用 `muxlayer` 这个虚拟名，让网关在请求时再解析成 DeepSeek / MiMo / 其他 Provider 的模型。旧配置里的 `agentgate` 仍然有效。

### 8. 直接调 API

所有 endpoint（除了 `/health`）都需要本地 access token。

**怎么拿 token：**

- **从 UI 复制**：MuxLayer → **设置** → **网关认证** → 点 token 旁边的复制按钮
- **终端读**：
  ```bash
  TOKEN=$(cat ~/.muxlayer/token 2>/dev/null || cat ~/.agentgate/token)
  ```
- **重新生成**：**设置** → **重新生成 Token**（旧 token 立刻失效）

Token 格式是 `ag_local_*`。它只用于本地网关认证，绝不转发给上游 Provider。

**Chat Completions（Pass-through）**

```bash
curl -X POST http://127.0.0.1:9090/v1/chat/completions \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"model":"deepseek-v4-flash","messages":[{"role":"user","content":"Hello"}]}'
```

**Responses API（Codex 协议）**

```bash
curl -X POST http://127.0.0.1:9090/v1/responses \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5.5","input":"Hello","stream":true}'
```

**Messages API（Claude Code 协议）**

```bash
curl -X POST http://127.0.0.1:9090/v1/messages \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-sonnet-4-6","max_tokens":1024,"messages":[{"role":"user","content":"Hello"}]}'
```

**模型列表**

```bash
curl http://127.0.0.1:9090/v1/models -H "Authorization: Bearer $TOKEN"
```

**健康检查（不需要鉴权）**

```bash
curl http://127.0.0.1:9090/health
```

**客户端报"网络连接失败"？**

先确认网关端口确实在运行：

```bash
curl http://127.0.0.1:9090/health
```

- 连不上：回 MuxLayer **概览 / 网关 / 客户端**，点 **启动网关**。
- health 通的话：用 **客户端** 页的连接测试做收敛：配置 → 网关 → Provider。
- `http://localhost:1420` 只是开发用的 UI；Codex / Claude Code / OpenCode / Gemini CLI / AtomCode 调的是 `http://127.0.0.1:9090`。

### 9. 多 Provider & Failover

在 **路由** 页配置 Route Profile：

1. 默认会按协议（Codex / Claude Code / OpenCode）自动建路由
2. 给 Provider 链加多个 Provider，调优先级
3. 切换模式：手动 / 故障转移
4. 故障转移 模式下，429/402/5xx/超时错误会自动尝试下一个 Provider

### 10. 能力感知路由（多模态 & 推理）

MuxLayer **按模型** 追踪 8 个维度的能力，并用它做路由——请求里有图 / 音频 / 工具时，网关挑一个真正支持的模型。

**按模型的能力矩阵：**

`text` · `vision` · `audio_in` · `tts` · `video_in` · `reasoning` · `tools` · `web_search`

**设置：**

1. 添加 Provider——模型列表、能力、最佳的默认 / 推理模型，在创建之后自动探测
2. **供应商** 页的卡片上会显示能力图标 + 原生 pass-through 徽标（`Native Chat`、`Native Anthropic`、……）
3. 想手动重新探测：打开编辑 → 点 **拉取并探测能力**
4. **路由** 页把模式切到 **故障转移**

**怎么工作的：**

- Provider 创建后：网关调上游 `/models`，按名字模式给能力做初始化，挑最新的非 mini 模型当默认，挑最新的推理风格模型当 reasoning_model
- 请求带图时，网关会：
  - 先试着在同一个 Provider 里换到支持 vision 的同系列模型（比如图片请求 `mimo-v2.5-pro` → `mimo-v2.5`）
  - 当前 Provider 没有同系列的，就 故障转移 到链上下一个矩阵里有 vision 模型的 Provider
- 能力矩阵也管工具下发——把某模型的 `web_search` 取消勾选，网关就不会再给它发 `web_search` 内建工具
- 升级时挑候选者优先看"保留原模型其他能力最多"那个；`supported_models` 顺序作为决胜局
- 完全没有 vision 模型的 Provider，会在 Provider 转换层剥掉图片内容，避免在纯文本兜底时上游 400/404

**示例场景：**

```
Codex 发了一条带图请求
  → 网关看到请求有图
  → MiMo 矩阵：mimo-v2.5-pro = text，mimo-v2.5 = text + vision
  → 升级 mimo-v2.5-pro → mimo-v2.5（同 Provider，能力匹配）
  → 请求穿过去，图片 + 文字完整保留
```

### 11. 诊断

在 **诊断** 页：

- **运行自检** —— 检查网关、Provider、配置、数据库状态
- **导出诊断包** —— 生成脱敏后的诊断报告，用于排查
- 请求日志的 `trace_json` 里会带 `degradation_events`，当 MuxLayer 剥掉某些不支持的能力时，比如图片、原生 web search、MCP 连接、工具输出图片

</details>

## 支持的 Provider

标了 **Provider 专属处理** 的 Provider，在 `src-tauri/src/transform/providers/` 里有专门的转换代码。其余的走通用的 Chat Completions / Anthropic pass-through 路径，开箱即用。

<!-- PROVIDER_CATALOG_TABLE:START -->
| Provider | 类型 | 原生协议 | Provider 专属处理 |
|---|---|---|---|
| 小米 MiMo | `mimo` | Chat + Anthropic | 多轮 `reasoning_content` 往返、区域感知 `tp-*` host 自动路由、thinking 模式剥 temperature、tool_choice 非 auto 时剥、omni `web_search` 剥、`web_search` 内建按矩阵门控、Web Search 插件自动降级 / 重试 |
| DeepSeek | `deepseek` | Chat + Anthropic | `deepseek-v4-flash-vision-exp` 保留图片输入；`deepseek-v4-flash` 和 `deepseek-v4-pro` 剥图并注入显式通知；DeepSeek V4 thinking 历史 reasoning 回填、schema 清洗、消息重排 |
| Anthropic (Claude) | `anthropic` | Anthropic | `tool_use`/`tool_result`、`input_schema`、thinking budget、原生 cache_control |
| GitHub Copilot | `copilot` | Chat + Anthropic | GitHub token → Copilot bearer 交换、`x-initiator` 计费分类、Claude 模型短横线→点 归一化 |
| OpenAI | `openai` | Chat + Responses | 无（Responses pass-through 或 Chat 转换） |
| Google Gemini | `google_gemini` | Chat | 无 |
| Kimi / Moonshot | `kimi` | Chat | `web_search` → `builtin_function`/`$web_search`、thinking 控制 |
| MiniMax | `minimax` | Chat | 剥 reasoning_effort / response_format、`<think>` 抽取 |
| GLM（智谱） | `glm` | Chat | 通用 |
| DashScope（通义） | `dashscope` | Chat | 通用 |
| SiliconFlow | `siliconflow` | Chat | 通用 |
| Volcengine（豆包） | `volcengine` | Chat | 通用 |
| 百川 | `baichuan` | Chat | 通用 |
| StepFun | `stepfun` | Chat | 通用 |
| SenseNova | `sensenova` | Chat | 剥掉 null strict / response_format / 非 function 工具，合并 system 消息 |
| Yi（零一万物） | `yi` | Chat | 通用 |
| ModelScope | `modelscope` | Chat | 通用 |
| xAI（Grok） | `xai` | Chat | 通用 |
| Mistral | `mistral` | Chat | 通用 |
| Groq | `groq` | Chat | 通用 |
| Together | `together` | Chat | 通用 |
| Fireworks | `fireworks` | Chat | 通用 |
| Cerebras | `cerebras` | Chat | 通用 |
| Perplexity | `perplexity` | Chat | 通用 |
| Cohere | `cohere` | Chat | 通用 |
| OpenRouter | `openrouter` | Chat | 无 |
| OrcaRouter | `orcarouter` | Chat | 无 |
| Custom | `custom_openai_compatible` | Chat | 无（Base URL 自己填） |
<!-- PROVIDER_CATALOG_TABLE:END -->

> Vision / 推理 / 工具 / `web_search` 能力是 **按模型** 追踪在能力矩阵里的，不是按 Provider。看下面的 *能力感知路由*。

## 架构与内部实现

<details>
<summary><b>数据流、请求模式、网关路由</b></summary>

### 数据流

MuxLayer 把协议处理和模型命名分开。常见请求有三种模式：

> **怎么判断？** 先看客户端协议跟某个 Provider 的原生 endpoint 是不是匹配。匹配的话，请求不做协议转换直接转发。模型名是另一码事：哪怕协议匹配，命中 Model Mapping 也能改写 `model`。

| 客户端 | 发送 | 下游 Provider | MuxLayer 模式 | 触发条件 |
|---|---|---|---|---|
| Codex | Responses API | Chat Completions | 协议转换 | 默认（没特殊 URL） |
| Codex | Responses API | Claude Messages API | 协议转换 | `provider_type` 是 `anthropic` |
| Codex | Responses API | Responses API | 原生 Pass-through | 配了 `responses_base_url` |
| Claude Code | Messages API | Chat Completions | 协议转换 | 没配 `anthropic_base_url` |
| Claude Code | Messages API | Anthropic 兼容 endpoint | 原生 Pass-through + Model Mapping | 配了 `anthropic_base_url`，并且 `claude-*` 映射到 Provider 模型 |
| OpenCode | Chat Completions | Chat Completions | 原生 Pass-through | 协议和上游模型名都一致 |
| curl / New API 等 | Chat Completions | Chat Completions | 原生 Pass-through | 协议和上游模型名都一致 |

### 协议转换

客户端协议跟下游 Provider 不一样时，MuxLayer 会转换格式。这是最复杂的那条路径，包含 vision 感知路由和 Provider 专属处理。

```
┌──────────────────┐    ┌──────────────────┐
│      Codex       │    │    Claude Code    │
│  (Responses API) │    │  (Messages API)   │
└────────┬─────────┘    └────────┬─────────┘
         │                       │
         ▼                       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                    MuxLayer (127.0.0.1:9090)                           │
│                                                                         │
│  ① 鉴权：校验本地 token（ag_local_*）                                   │
│                         ▼                                               │
│  ② 路由匹配：按协议匹配 Route Profile                                   │
│     /v1/responses → Codex Default                                       │
│     /v1/messages  → Claude Code Default                                 │
│                         ▼                                               │
│  ③ 协议转换（共享层）                                                   │
│     Responses API → Chat Completions（input_image → image_url）         │
│     Messages API  → Chat Completions（image → image_url）               │
│                         ▼                                               │
│  ④ 能力感知路由（故障转移 模式）                                        │
│     带图 → 升级到同 Provider 下支持 vision 的同系列，                   │
│            或 故障转移 到链上下一个有 vision 模型的 Provider            │
│     不带图 → 按优先级正常选                                             │
│                         ▼                                               │
│  ⑤ Provider 专属转换                                                    │
│     DeepSeek   → 视觉模型保留图片；纯文本模型剥图                       │
│                   + reasoning_content + schema 修复                     │
│     KimiCoding → web_search 转换 + thinking 控制                        │
│     Anthropic  → 转 Claude Messages（image→source.base64）              │
│     其他       → 直接发                                                 │
│                         ▼                                               │
│  ⑥ Failover：429/402/5xx/超时 → 冷却 → 尝试下一个 Provider             │
│                         ▼                                               │
│  ⑦ 日志 → SQLite                                                       │
│                         ▼                                               │
│  ⑧ 响应反向转换：回到原协议返回给客户端                                 │
└─────────┬───────────────────────────────┬───────────────────────────────┘
          │                               │
          ▼                               ▼
   ┌──────────────┐               ┌──────────────┐
   │   DeepSeek   │               │  KimiCoding  │  ...
   │  （纯文本）  │               │（文本+图片） │
   └──────────────┘               └──────────────┘
```

### 原生 Pass-through

客户端协议跟下游 Provider 一致时，MuxLayer 不转换请求格式，只替换 URL 和凭据。模型处理只有一条规则：Model Mapping 优先，否则保留请求里的模型。如果请求里的模型是 `muxlayer`、`openai/muxlayer`，或旧别名 `agentgate` / `openai/agentgate`，MuxLayer 解析成这次路由选中的模型。客户端没传 `model`，就用 Provider 默认值。

```
┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
│    Claude Code   │  │     OpenCode     │  │  curl / New API  │
│  (Messages API)  │  │(Chat Completions)│  │(Chat Completions)│
└────────┬─────────┘  └────────┬─────────┘  └────────┬─────────┘
         │                     │                      │
         ▼                     ▼                      ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                    MuxLayer (127.0.0.1:9090)                           │
│                                                                         │
│  ① 鉴权：校验本地 token（ag_local_*）                                   │
│                         ▼                                               │
│  ② 路由匹配：按协议匹配 Route Profile                                   │
│     /v1/messages          → Claude Code Default                         │
│     /v1/chat/completions  → OpenCode Default                            │
│                         ▼                                               │
│  ③ 原生 Pass-through                                                    │
│     替换目标 URL（base_url 或 anthropic_base_url）                       │
│     注入 Provider API Key                                               │
│     可选的 model mapping（claude-* → mimo/deepseek 模型）               │
│     请求体原样转发 ──→ 响应流原样回                                    │
│                         ▼                                               │
│  ④ 日志 → SQLite                                                       │
└─────────┬───────────────┬───────────────────┬───────────────────────────┘
          │               │                   │
          ▼               ▼                   ▼
  ┌──────────────┐ ┌──────────────┐   ┌──────────────┐
  │  DeepSeek    │ │   OpenAI     │   │  New API     │  ...
  │  /anthropic  │ │              │   │  / 聚合代理  │
  └──────────────┘ └──────────────┘   └──────────────┘

触发条件：
  • /v1/messages    + Provider 有 anthropic_base_url → Messages API 原生 pass-through
  • /v1/chat/completions → Chat Completions 原生 pass-through（所有 Provider）
```

### 流程示例

**Codex 带图请求（协议转换 + vision 感知路由）：**

```
Codex 发 input_image
  → /v1/responses（Responses API）
  → ① 鉴权通过
  → ② 匹配 Codex Default Route Profile
  → ③ 协议转换：input_image → image_url（图片保留）
  → ④ Vision 路由：检测到图片 → 跳过 DeepSeek 纯文本模型；配置了 `deepseek-v4-flash-vision-exp` 就使用它，否则选 KimiCoding（有 Vision）
  → ⑤ KimiCoding 转换：不剥图，直接发
  → ⑥ KimiCoding 返回成功 → 标记健康
  → ⑦ 记日志
  → ⑧ 反向转换成 Responses API 格式 → 返回给 Codex
```

**Claude Code → DeepSeek（原生 pass-through + Model Mapping）：**

```
Claude Code 发 Messages API 请求
  → /v1/messages
  → ① 鉴权通过
  → ② 匹配 Claude Code Default Route Profile
  → ③ DeepSeek 配了 anthropic_base_url → 原生 pass-through
  → Model Mapping：claude-sonnet-4-6 → deepseek-v4-pro
  → URL 换成 api.deepseek.com/anthropic + 注入 API Key
  → 请求体原样转发 → SSE 响应流原样回
  → ④ 记日志
```

**OpenCode / curl / New API（原生 pass-through）：**

```
客户端发 Chat Completions 请求
  → /v1/chat/completions
  → ① 鉴权通过
  → ② 匹配 Route Profile
  → ③ 原生 pass-through：换 URL + API Key；模型保持原样，除非命中映射
  → 请求体原样转发 → SSE 响应流原样回
  → ④ 记日志
```

### 网关路由

| 方法 | 路径 | 模式 | 说明 |
|---|---|---|---|
| GET | `/health` | 内部 | 健康检查（无需鉴权） |
| GET | `/v1/models` | 内部 | 模型列表 |
| POST | `/v1/responses` | 自动 | 配了 `responses_base_url` → pass-through；Anthropic 类型 → Claude 转换；否则 → Chat Completions 转换 |
| POST | `/v1/chat/completions` | pass-through | Chat Completions 直转 |
| POST | `/v1/messages` | 自动 | 配了 `anthropic_base_url` → pass-through；否则 → Chat Completions 转换 |

</details>

## 项目结构

```
MuxLayer/
├── provider-catalog/             # Provider/模型的源信息，用于生成默认值
├── src/                          # 前端（React）
│   ├── app/App.tsx               # App 入口
│   ├── pages/                    # 页面（概览/快速配置/供应商/路由/网关/客户端/日志/诊断/指令/MCP/技能/设置）
│   ├── components/               # UI 组件（layout/common/dashboard/providers/logs/tools/onboarding）
│   ├── pet/                      # 桌面宠物系统（PetApp/Bubble/greetings/9 个宠物 SVG 组件）
│   ├── lib/                      # API 封装、i18n、工具
│   └── types/                    # TypeScript 类型定义
├── src-tauri/                    # 后端（Rust）
│   └── src/
│       ├── gateway/              # HTTP 网关（server/routes/SSE/SSE Anthropic/pass-through/故障转移）
│       ├── protocol/             # 协议类型（Responses/ChatCompletions/Messages/SSE 事件）
│       ├── transform/            # 协议转换（responses→chat/responses→anthropic/schema 清洗/tool_calls/reasoning store/providers）
│       ├── providers/            # Provider 适配器
│       ├── storage/              # SQLite 存储层
│       ├── models/               # 数据模型
│       ├── tools/                # 客户端配置 + MCP / 技能 / 全局指令管理
│       ├── security/             # 鉴权 & 脱敏
│       ├── diagnostics/          # 诊断 & 自检
│       ├── app/                  # Tauri 命令 & app 状态
│       └── errors/               # 统一错误类型
├── scripts/                      # 测试和生成脚本
└── package.json
```

## 安全

- 网关默认走本地 token 鉴权
- Provider API Key 只存在本地 SQLite 里，绝不发给客户端
- 客户端的 token 绝不会转发给上游 Provider
- 请求日志和诊断包对密钥全面脱敏：`sk-` 开头的 Key、Bearer token、`x-api-key`、`api_key` 字段、Gemini 风格的 `?key=` query 参数
- 桌面 app 只绑 `127.0.0.1`；无界面模式（`agentgate-serve`）只有显式传 `--host` 才会暴露网关，并带了针对 DNS 重绑定的 Host / Origin 校验（用域名访问需要 `AGENTGATE_ALLOWED_HOSTS` 白名单）
- Token 文件权限 `0600`（Unix）

## 常见问题

**我的 API Key 安全吗？MuxLayer 会回传数据吗？**
Key 只存在你机器上的本地 SQLite 文件里，绝不发给客户端，也不发给任何 MuxLayer 服务器——MuxLayer 根本没有后端。你的 Key 只会发给你自己配的上游 Provider。桌面 app 绑在 `127.0.0.1`；只有无界面模式带 `--host 0.0.0.0` 才会暴露，并自带 Host / Origin 校验。

**用 Copilot 订阅跑 Claude Code 会不会被封号？**
在 GitHub 服务条款里是灰色地带：类似的社区工具存在很久了也没看到大规模封禁，但风险不能完全排除——看 [风险告知](#可选用-github-copilot-订阅跑-claude-code--codex)。你不添加 copilot 类型的 Provider，就跟你没关系。

**会不会搞坏我的 Codex / ChatGPT 登录或 Codex Desktop 插件？**
不会。MuxLayer 让 Codex 留在官方的 OpenAI 认证 Provider 路径上，你的登录账号、插件、Browser / Computer-Use / Mobile 和配额查询都正常，模型请求路由到第三方。**切换到官方** 任何时候都能还原原配置——会话不会丢。

**能在离线 / 服务器上无 GUI 运行吗？**
能。无界面模式（`agentgate-serve`）和 Docker 都能跑——看 [无界面 / 服务端模式](#无界面--服务端模式)。

**必须手改 `config.toml` / `settings.json` 吗？**
不用。一键 **应用配置** 帮你写好，原配置会备份；**切换到官方** 一键回滚。

**客户端报"网络连接失败"怎么办？**
先确认网关在跑：`curl http://127.0.0.1:9090/health`。失败就在 app 里启动网关。注意 `localhost:1420` 只是开发用 UI——客户端调的是 `127.0.0.1:9090`。

**这条请求到底打的哪个模型？**
打开 **日志**——每条请求都显示客户端、路由、最终选中的 Provider、模型、状态、成本。

**为什么今日费用是 $0？**
要么还没有计过价的流量，要么价格表里没有这个模型。缺价不是免费——拆分里会显示 **去补价**。补价后重启，`cost IS NULL` 的历史行会重算。

## 开发

### Provider Catalog

内建的 Provider/模型数据维护在 `provider-catalog/providers/*.json` 里。
改完 catalog 文件之后跑这几条：

```bash
pnpm provider:catalog:generate
pnpm provider:catalog:check
pnpm provider:catalog:sync:check
```

`provider:catalog:sync:check` 在匹配的 API Key 环境变量存在时会调 Provider 的 `/models` endpoint，没凭据的 Provider 会跳过。当某个目录模型在上游已经查不到时，它会失败；传 `--strict` 还会在发现上游有新模型时也失败。要手动刷新某个 Provider，跑：

```bash
pnpm provider:catalog:sync -- --provider deepseek --update
pnpm provider:catalog:generate
```

生成的 TS/Rust 文件喂给 Provider 预设、endpoint 默认值、能力初始化、价格默认值、`supported_models` 门控、推荐映射策略。Provider 专属的运行时行为还是在代码里，不在 catalog 里。

上游模型同步是有意做成本地维护步骤的，不放在 GitHub release 工作流里。Release CI 只检查生成的 catalog 产物是不是最新。

### 测试脚本

```bash
# 健康检查
./scripts/test-gateway-health.sh

# 鉴权测试
./scripts/check-gateway-auth.sh

# Responses API 测试
./scripts/test-responses-stream.sh

# Chat Completions 测试
./scripts/test-chat-completions-pass-through.sh
```

## 社区 & 支持

- 🐛 **发现 bug 或想要新功能？** 开 [Issue](https://github.com/dengmengmian/muxlayer/issues)——报网关问题时附上脱敏的诊断包（**诊断** → 导出）。
- 💡 **问题和想法**：[Discussions](https://github.com/dengmengmian/muxlayer/discussions) 用来聊配置帮助、Provider 请求、工作流点子、成功故事。
- 🧩 **想贡献代码？** 从 [CONTRIBUTING.md](../CONTRIBUTING.md)、[PR 模板](../.github/pull_request_template.md) 和打 `good first issue` 标签的 issue 开始。
- ⭐ **如果 MuxLayer 帮你省了钱或时间，给个 star 能帮别人发现它** ——也能帮我们排接下来要做什么。

三个支柱——智能路由、自愈、成本看板——已经上了。下一步做什么，由 Issue 和 Discussion 推动，所以告诉我们你需要什么。

## Star 历史

<a href="https://star-history.com/#dengmengmian/muxlayer&Date">
  <img src="https://api.star-history.com/svg?repos=dengmengmian/muxlayer&type=Date" alt="Star History Chart" width="600">
</a>

## 许可证

MIT
