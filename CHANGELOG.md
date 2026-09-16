# Changelog / 更新日志

## [2.0.7] - 2026-09-16

### Changed / 变更

- **DeepSeek models are now `deepseek-flash` and `deepseek-v4-pro` / DeepSeek 只剩 `deepseek-flash` 和 `deepseek-v4-pro`** —— `deepseek-flash` is the default model and reads images directly, so images are no longer removed before they reach it. `deepseek-v4-pro` is still text-only. Setups that still use `deepseek-v4-flash` or `deepseek-v4-flash-vision-exp` switch to `deepseek-flash` automatically on startup. `deepseek-flash` 是默认模型，能直接看图，图片不再被去掉。`deepseek-v4-pro` 仍然只收文字。还在用 `deepseek-v4-flash` 或 `deepseek-v4-flash-vision-exp` 的配置，启动时自动换成 `deepseek-flash`。

## [2.0.6] - 2026-09-14

### Fixed / 修复

- **Failover really switches / 自动切换真的会切** —— A 429 or 5xx from a provider now moves to the backup on all four entry points, including Claude Code and Gemini CLI. Request errors like "prompt is too long" go back to you and don't bench the provider. A reply that has started is never retried. 上游 429、5xx 会切到备用，Claude Code 和 Gemini CLI 的入口也会切。「prompt 太长」这类请求错误原样返回，不会把 provider 踢出去。已经开始回内容的请求不重试。
- **Switch back keeps your changes / 切回官方保留你的改动** —— Only what MuxLayer wrote is removed; MCP servers, hooks, and trust added later stay. Unreadable config files are no longer overwritten. OpenCode keeps other providers and Gemini keeps the rest of `.env`. 只删 MuxLayer 写的部分，后来加的 MCP、hook、信任都在。读不了的配置不再被覆盖。OpenCode 保留其它 provider，Gemini 保留 `.env` 里其它变量。
- **Stop process and Restart Codex / 结束进程与重启 Codex** —— Claude Desktop, the Grok app, and ChatGPT's built-in Codex are no longer listed. Restart Codex no longer quits ChatGPT and only shows when Codex Desktop is installed. 不再把 Claude Desktop、Grok 桌面版、ChatGPT 里的 Codex 列进来。重启 Codex 不再关 ChatGPT，没装桌面端时不显示。
- **Protocol conversion / 协议转换** —— Chat clients can use `reasoning_effort` with Claude; long Chinese replies no longer cut off; Claude Code no longer double-counts cached tokens on OpenAI-style providers; Gemini CLI tool calls keep their arguments and ids; requests without `stream` routed to Claude work. Chat 客户端能给 Claude 开思考；长中文回复不再中断；Claude Code 接 OpenAI 形态上游不再把缓存算两遍；Gemini CLI 工具调用参数和 id 不再丢；不带 `stream` 的请求路由到 Claude 时能用。
- **Costs / 花费** —— Claude Code pass-through traffic and prompt cache are counted, and "today" follows your local day (set `TZ` in Docker). 直通 Anthropic 的流量和提示缓存计入花费，「今天」按本地日期算（Docker 里要设 `TZ`）。

### Improvements / 改进

- **Smoother under load / 高并发更顺** —— Logging and database work no longer block requests; hourly cleanup no longer rewrites the whole database; clearing logs, big stats, and client detection don't freeze the window. 写日志、查库不卡请求；清理不再每小时重写数据库；清日志、看统计、检测客户端不卡界面。
- **Page errors are recoverable / 页面出错可恢复** —— A message with a retry button instead of a blank window. 出错显示提示和重试按钮，不再白屏。
- **Easier to read / 更好读** —— Light and dark themes only (old themes migrate automatically), bigger small text, colors pass contrast checks. 只保留浅色和深色主题（旧主题自动迁移），小字调大，颜色过了对比度检查。

### Security / 安全

- **Safer defaults / 默认更安全** —— Database and token files are owner-only; exports hide keys without known prefixes; cross-host redirects aren't followed; the app window has a CSP; `docker-compose.yml` listens on `127.0.0.1`; `/metrics` needs the token; rate limiting trusts `X-Forwarded-For` only with `MUXLAYER_TRUST_PROXY=1`. 数据库和 token 文件只有自己能读；导出会遮住无前缀的 key；不跟随跨域重定向；窗口加了 CSP；compose 默认只监听本机；`/metrics` 要带 token；只有设了 `MUXLAYER_TRUST_PROXY=1` 才信 `X-Forwarded-For`。

## [2.0.5] - 2026-09-04

### Fixed / 修复

- **Codex 0.152+ cannot run shell / Codex 0.152+ 跑不了 shell** —— Codex CLI 0.152+ puts the code-mode `exec` tool inside a `functions` namespace. MuxLayer only inspected top-level custom tools, so DeepSeek and other third-party Responses endpoints dropped `exec`. Chat conversion restored it, but Claude and Gemini still wrote `function_call`, which Codex's `tools::router` rejected. Nested custom tools now force conversion on every non-OpenAI Responses path; Claude / Gemini / Chat all restore `custom_tool_call` with raw JavaScript. Codex 0.152+ 把 code-mode 的 `exec` 放进 `functions` 命名空间。MuxLayer 只扫顶层 custom 工具，DeepSeek 和其它第三方 Responses 会把 `exec` 丢掉。Chat 转换已经能还原，但 Claude / Gemini 仍回 `function_call`，Codex 的 router 会拒。现在所有非 OpenAI 的 Responses 路径都会转换，Claude / Gemini / Chat 都还原成带原始 JavaScript 的 `custom_tool_call`。

## [2.0.4] - 2026-09-01

### Added / 新增

- **Kimi CLI, Grok Build, DeepSeek Harness / 三个客户端一键接入** —— Apply from the Clients page. Each client has its own icon. 客户端页一键接入。图标换成各自的样子。
- **Stop a running client after apply / 应用后可结束进程** —— Click Stop process instead of copying `kill`. 点「结束进程」，不用自己复制 kill。
- **Route templates / 路由模板** —— Task split, local-first, cloud-first. 任务拆分、本地优先、云端优先。
- **OpenCode and Gemini MCP/Skills / OpenCode 与 Gemini 的 MCP 和 Skills** —— Same console as Codex and Claude Code. 和 Codex、Claude Code 同一套管理。

### Improvements / 改进

- **Virtual model is `muxlayer` / 虚拟模型叫 muxlayer** —— `agentgate` still works. 旧配置里的 `agentgate` 仍然认。
- **Kimi config path / Kimi 写对目录** —— Writes `~/.kimi-code` and includes `max_context_size` so a session can start. 写到 `~/.kimi-code`，并补上上下文长度。
- **Several keys / 多 Key** —— Shows "N keys · round-robin", no fake per-key quota. 显示「N 把 key · 轮转」，不编额度。
- **`MUXLAYER_*` env vars / 环境变量** —— Preferred names; `AGENTGATE_*` still works; Docker writes both. 优先新名字，旧名字兼容，Docker 双写。
- **Overview hit rate / 概览命中率** —— Cache hit rate is visible on the dashboard. 概览能看到缓存命中率。
- **Lighter UI / 界面轻一点** —— SVG sidebar logo, slower gateway polling, command palette loads on open. 小 logo、轮询没那么勤、命令面板用时才加载。

## [2.0.3] - 2026-08-28

### Added / 新增

- **OrcaRouter as a first-class provider / OrcaRouter 成为一级 Provider** —— OrcaRouter is an OpenAI-compatible aggregator fronting 190+ models behind one key. Base URL `https://api.orcarouter.ai/v1` is preset, `sk-orca-` keys are detected on paste, and the model list is fetched live instead of pinned in the catalog. The default model `orcarouter/auto` is the per-account router OrcaRouter creates on signup, which sizes the model to each request. No gateway, protocol, or adapter changes were needed. OrcaRouter 是 OpenAI 兼容的聚合网关，一个 key 覆盖 190+ 模型。Base URL `https://api.orcarouter.ai/v1` 已预置，粘贴 `sk-orca-` 开头的 key 会自动识别，模型列表实时拉取而不在目录里固化。默认模型 `orcarouter/auto` 是 OrcaRouter 在注册时为每个账号创建的路由，按请求难度挑选模型。网关、协议与适配层零改动。

### Documentation / 文档

- Added an OrcaRouter section to both READMEs and provider entries to the full reference, and synced the provider tables and preset count. 中英文 README 新增 OrcaRouter 小节，完整参考新增 Provider 条目，Provider 表格与预设数量同步更新。

## [2.0.2] - 2026-08-27

### Fixed / 修复

- **Windows infinite console popup after Apply / Windows 应用配置后黑窗无限弹出** —— On Windows, client process detection spawned `tasklist` without `CREATE_NO_WINDOW`, so a black console window popped up on every check. The popup stole window focus, and returning focus triggered another refresh-and-detect cycle, looping forever after clicking Apply. Detection now runs with a hidden console. Windows 下客户端进程探测调用 `tasklist` 时未设置 `CREATE_NO_WINDOW`，每次探测都会弹出黑色控制台窗口；弹窗抢走焦点、焦点回到主窗口又触发一次刷新探测，点「应用配置」后就此无限循环。现在探测在隐藏控制台中执行。
- **Restart Codex Desktop console flash on Windows / Windows 重启 Codex Desktop 闪黑窗** —— The `taskkill` call behind the restart button now also runs with a hidden console. 「重启 Codex Desktop」按钮背后的 `taskkill` 调用同样改为隐藏控制台执行。

## [2.0.1] - 2026-08-21

### Added / 新增

- **DeepSeek image understanding / DeepSeek 图像理解** —— Added `deepseek-v4-flash-vision-exp` to the built-in catalog with image-input support across Chat Completions, Responses, and Anthropic-compatible routes. 内置目录新增 `deepseek-v4-flash-vision-exp`，支持通过 Chat Completions、Responses 和 Anthropic 兼容入口传入图片。

### Fixed / 修复

- **Per-model vision boundaries / 按模型区分视觉能力** —— `deepseek-v4-flash` and `deepseek-v4-pro` remain text-only: image requests are never passed through to them and conversion paths keep the explicit image-degradation notice. `deepseek-v4-flash` 与 `deepseek-v4-pro` 仍按纯文本模型处理：带图请求不会直通给它们，协议转换路径继续剥图并注入明确提示。
- **Vision promotion survives model mapping / 视觉升级不再被模型映射覆盖** —— Image-aware routing now keeps the promoted vision model through native Chat, Responses, and Anthropic pass-through instead of mapping it back to a text-only model. 带图请求升级到视觉模型后，原生 Chat、Responses 与 Anthropic 直通不会再被 Model Mapping 覆盖回纯文本模型。

### Documentation / 文档

- Updated the English and Chinese DeepSeek guides, provider catalog, README tables, and full routing reference. 同步更新中英文 DeepSeek 指南、Provider 目录、README 表格与完整路由参考。

## [1.6.3] - 2026-08-13

### Added / 新增

- **Outbound HTTP proxy / 出站 HTTP 代理** —— Settings can send provider API calls through a local `http://` / `https://` proxy (Clash / V2Ray). Loopback stays direct. Off keeps using `HTTP(S)_PROXY` from the environment. An empty URL does not apply the UI proxy. 设置里可让访问上游的请求走本机 HTTP/HTTPS 代理；回环地址仍直连。关闭时继续尊重环境变量。地址为空时开关不会生效。

### Improvements / 改进

- **Routing conditions on every protocol / 路由条件覆盖全部协议入口** —— Chat Completions, Anthropic Messages, and Gemini now evaluate the same body conditions as Codex Responses (`min/max_input_chars`, `has_images`, `has_tools`, `system_keywords`, `model_name_match`). Existing route profiles are unchanged; conditions that were previously ignored on those routes now apply. Chat / Messages / Gemini 与 Codex Responses 使用同一套请求体条件；已有路由配置不用改，只是以前被忽略的条件现在会生效。
- **Daily budget on Gemini / Gemini 也走日预算闸** —— Gemini generateContent is gated by the same daily spend policy as other routes. Gemini 请求同样受日预算策略约束。

### Performance / 性能

- **Hot-path work cut / 热路径减负** —— Budget off skips the lifetime stats scan; daily spend uses a cheap SUM with a short cache. Request JSON is parsed once per route. SSE line splitting is shared. Session lookup has an in-process cache. Dashboard / footer poll less often. 预算关闭时不再扫全量统计；日花费用短缓存 SUM。请求 JSON 每条路由只解析一次。SSE 拆行共用。会话查找有进程内缓存。概览与页脚轮询更慢。

## [1.6.2] - 2026-08-12

### Added / 新增

- **Stream failure timeline / 流式失败时间轴** —— Failed request details show start → route → upstream status → last SSE event → failover chain → end reason (from `trace_json` / `sse_events`). 失败请求详情展示：开始 → 选路 → 上游状态 → 末次 SSE → 故障转移 → 结束原因（来自 `trace_json` / `sse_events`）。
- **First-request launch commands / 首条请求启动命令** —— After successful Quick Setup (and when Dashboard is truly ready), show copyable commands only for AgentGate-wired clients. 快速配置成功后（及概览真正就绪时），仅对已接入 AgentGate 的客户端展示可复制启动命令。

### Improvements / 改进

- **Honest onboarding completion / 首装完成条件收紧** —— Complete only when provider + gateway running + ≥1 client applied + probe OK; one primary failure CTA. 仅当供应商、网关运行、至少一个客户端 apply、探针成功才算完成；失败只有一个主 CTA。
- **Dashboard readiness / 概览就绪判定** —— Requires usable API key and `has_agentgate`, not `config_exists` alone. 需要可用 API Key 且客户端已 `has_agentgate`，不再只看本机配置文件是否存在。
- **Sessions UI / 会话视图** —— Primary request/session switch, lighter session filters, card list with conversation as main action. 请求/会话主切换、会话筛选更轻、卡片列表主操作为打开对话。
- **Local models dialog / 本地模型弹窗** —— Scan Ollama / LM Studio from a toolbar dialog instead of an always-on panel. 从工具栏弹窗扫描，不再常驻供应商页。
- **MCP more menu / MCP 更多菜单** —— Header overflow no longer clips the export/import menu. 页头不再裁切导出/导入菜单。
- **Provider tips / 供应商提示** —— Refiner jargon removed from card face; plain-language tips under expanded details. 卡片正面去掉工程师术语，展开详情后用人话提示。

### Removed / 移除

- **Dead SetupWizard / 未挂载的 SetupWizard** —— Removed unused onboarding component; `/quick-setup` is the real entry. 删除未挂载的引导组件，真入口为 `/quick-setup`。

## [1.6.1] - 2026-07-31

### Fixed / 修复

- **Responses pass-through dropped every tool / 原生 Responses 直通丢工具** —— Codex gpt-5.6+ puts tool definitions inside an `additional_tools` item in `input` instead of top-level `tools`. Pass-through forwards the raw body, but the hoist only ran on the parsed struct, so upstreams received no tools at all and the model fabricated tool calls in plain text. The same hoist now runs on the forwarded body. Codex gpt-5.6+ 把工具定义放在 `input` 的 `additional_tools` 项里而非顶层 `tools`；直通转发的是 body 原文，而提升只作用于解析后的结构体，导致上游收不到任何工具、模型在正文里编造工具调用。现在 body 上也做同样提升。
- **Responses pass-through token accounting / 原生 Responses 直通用量统计** —— The pass-through usage sidecar only understood Chat Completions field names, so Responses-shaped usage (`response.usage` with `input_tokens` / `output_tokens`) produced empty token and cost records. Both shapes are now parsed. 直通的 usage 旁路解析只认 Chat Completions 字段名，Responses 形态（`response.usage` 下的 `input_tokens` / `output_tokens`）解析不出来，token 与成本记录为空。现已兼容两种形态。

### Improvements / 改进

- **Upstream capability gate for Responses pass-through / 直通前的上游能力判定** —— Pass-through now falls back to protocol conversion when the target model or a requested custom tool is outside what the upstream Responses API accepts, instead of sending a request that is bound to fail. 目标模型或请求里的自定义工具超出上游 Responses API 的接受范围时，直通会自动回落到协议转换，而不是把注定失败的请求发出去。

### Docs / 文档

- **Why AgentGate does not pass through to DeepSeek Responses / 为什么不直连 DeepSeek Responses** —— The Codex + DeepSeek guides now document, with measured evidence, why direct pass-through degrades output quality and why conversion stays the default. Codex + DeepSeek 教程补充实测依据，说明直连为何会让效果明显变差，以及为什么默认保持协议转换。

## [1.6.0] - 2026-07-30

### Added / 新增

- **Session affinity / 会话亲和** —— Same conversation sticks to the same upstream provider when routing would otherwise rotate (failover and cheapest-override still apply when required). 同一会话在正常选路下会粘在同一上游；失败转移与预算强制最便宜策略仍可覆盖。
- **Daily budget gate / 日预算闸** —— Optional daily spend limit with `notify_only` / `block` / `force_cheapest` policies; new requests are gated, in-flight streams are not cut mid-response. 可选日花费上限，支持仅提醒 / 拦截 / 强制最便宜选路；只卡新请求，不中途掐断流式响应。
- **Request Diff + redacted repro export / 请求 Diff 与脱敏复现包** —— Logs detail drawer compares raw vs converted bodies and can copy or download a redacted repro package for issue reports. 日志详情可对比原始与转换后 body，并可复制/下载脱敏复现包便于提 issue。
- **Local model discovery / 本地模型发现** —— Providers page can scan Ollama / LM Studio (and common local OpenAI-compatible ports) and one-click add them. 供应商页可扫描 Ollama / LM Studio 等常见本地 OpenAI 兼容端口并一键添加。
- **Client process detection / 客户端进程探测** —— Clients step and list surface whether configured tools appear to be running. 客户端配置步骤与列表展示对应工具是否在运行。
- **Gateway connection card + editor guides / 网关连接卡片与编辑器指南** —— Gateway page shows local Base URL / token usage; guides cover Cursor / Continue / Cline and local models. 网关页展示本机 Base URL 与 token 用法；新增 Cursor / Continue / Cline 与本地模型接入指南。
- **Onboarding completion hardening / 引导完成条件收紧** —— Setup wizard requires real provider success and clearer next actions on failure. 引导向导要求真实供应商连通成功，失败时给出明确主操作。

### Improvements / 改进

- **Session cost & cache savings / 会话成本与缓存节省** —— Session views aggregate tokens/cost; dashboard highlights cache savings and missing pricing more clearly. 会话视图汇总 token/成本；概览更清楚展示缓存节省与缺价状态。
- **Provider refiner hints / 供应商适配提示** —— Provider cards surface protocol/refiner tips for the selected provider type. 供应商卡片展示该类型的协议/适配提示。
- **Auto-compact settings / 自动压缩设置** —— Compact policy settings for long contexts are exposed in Settings / gateway config. 长上下文自动压缩策略可在设置/网关配置中调整。

## [1.5.1] - 2026-07-17

### Added / 新增

- **Kimi K3 / Kimi Code K3** —— Built-in Kimi catalog now includes Platform `kimi-k3` and Kimi Code `k3` (1M context), plus highspeed coding IDs `kimi-k2.7-code-highspeed` / `kimi-for-coding-highspeed`. Default model is `kimi-k3`. 内置 Kimi 目录补齐 Platform `kimi-k3` 与 Kimi Code `k3`（1M 上下文），以及高速编码模型 `kimi-k2.7-code-highspeed` / `kimi-for-coding-highspeed`；默认模型升级为 `kimi-k3`。
- **Kimi Anthropic endpoint / Kimi Anthropic 端点** —— Preset fills Platform `anthropic_base_url` so Claude Code can native-pass through. 预设补上 Platform Anthropic 端点，Claude Code 可原生透传。

### Improvements / 改进

- **K3 reasoning_effort / K3 思考强度** —— K3 requests send top-level `reasoning_effort: max` and no longer use the K2 `thinking` object; coding models keep on/off thinking and still disable thinking when `$web_search` is present. K3 请求发送顶层 `reasoning_effort: max`，不再使用 K2 的 `thinking` 对象；Coding 模型仍用 thinking 开关，并在带 `$web_search` 时关闭思考。
- **Kimi Code key auto-route / Kimi Code key 自动路由** —— Keys with prefix `sk-kimi-` are detected as Kimi and resolve to `https://api.kimi.com/coding/v1` with default model `k3` (Anthropic: `https://api.kimi.com/coding`). Classic Platform keys stay on `api.moonshot.cn` + `kimi-k3`. 识别 `sk-kimi-` 前缀并自动切到 Kimi Code 端点与模型 `k3`；经典 Platform key 仍走 `api.moonshot.cn` + `kimi-k3`。

## [1.5.0] - 2026-07-15

### Added / 新增

- **Keep Awake / 防休眠** —— AgentGate now prevents automatic system sleep by default on macOS and Windows while the app is running. The display may still turn off unless explicitly kept awake. AgentGate 现在默认在应用运行期间阻止 macOS 和 Windows 自动休眠；显示器默认仍可关闭，也可单独设为常亮。
- **Request-aware control / 请求智能控制** —— An optional request-aware mode keeps the system awake only while AI generation requests are active and during a configurable cooldown (15 minutes by default). Concurrent and streaming requests remain protected until their response bodies finish or disconnect. 可选的请求智能控制仅在 AI 生成请求进行中及可配置冷却期内保持唤醒（默认 15 分钟）；并发和流式请求会持续保护到响应结束或客户端断开。
- **Settings and tray controls / 设置页与托盘快捷操作** —— Keep-awake status, errors, request mode, cooldown, and display behavior are available in Settings; the system tray adds one-click toggles for keep-awake, request-aware control, and display wakefulness. 设置页可查看防休眠状态、错误、请求模式、冷却时间和显示器行为；系统托盘新增防休眠、请求智能控制、显示器常亮三个快捷开关。

### Improvements / 改进

- **Cross-platform native lifecycle / 跨平台原生生命周期** —— macOS uses the built-in `caffeinate` assertion and Windows uses native Power Request APIs. Assertions reconcile immediately when settings change and are released on shutdown. Unsupported Linux environments are reported explicitly instead of pretending success. macOS 使用系统内置 `caffeinate`，Windows 使用原生 Power Request API；设置变化会立即重建申请，退出时可靠释放；Linux 暂不支持时明确显示，不会假装成功。

## [1.4.13] - 2026-07-12

### Improvements / 改进

- **Denser provider layout / 更紧凑的供应商布局** —— The Providers page now uses the available desktop width with a responsive two-column grid, flatter information grouping, and a compact bottom-aligned action row. 供应商页改为桌面端双列响应式布局，减少嵌套容器并压缩底部操作区，充分利用横向空间。
- **Single first-run provider / 首次初始化仅保留一个供应商** —— New or empty databases now seed only DeepSeek instead of also adding an unused Custom OpenAI Compatible placeholder. Existing provider records are never removed or changed. 新安装或空数据库现在只初始化 DeepSeek，不再额外创建未配置的 Custom OpenAI Compatible；已有供应商记录不会被删除或修改。

## [1.4.12] - 2026-07-10

### Added / 新增

- **ChatGPT+Codex merged client support / 新增支持 ChatGPT+Codex 合并后的新版客户端** —— The merged ChatGPT+Codex desktop app (gpt-5.6 protocol) declares tools inside the `input` array as `additional_tools` items instead of the top-level `tools` field. The gateway now hoists them into `tools` before conversion, so tool calling keeps working on Chat Completions / Anthropic / Gemini upstreams; without this, third-party models received no tools and could only emit fake `<tool_call>` text. ChatGPT+Codex 合并后的新版桌面端(gpt-5.6 协议)把工具定义放在 `input` 数组的 `additional_tools` 项里,不再放顶层 `tools` 字段。网关现在会在转换前把它们提升合并进 `tools`,Chat Completions / Anthropic / Gemini 三条上游链路的工具调用全部恢复;此前第三方模型收不到任何工具,只能在正文里输出假 `<tool_call>` 文本。
- **gpt-5.6 model family / gpt-5.6 模型家族** —— Added `gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` to the built-in OpenAI catalog with pricing, bumped the default model to `gpt-5.6-terra`, and covered the family in Codex recommended mappings (`-luna` maps to the mini-tier model). 内置 OpenAI 目录补齐 `gpt-5.6` / `gpt-5.6-sol` / `gpt-5.6-terra` / `gpt-5.6-luna`(含价格),默认模型升级到 `gpt-5.6-terra`,Codex 推荐映射覆盖全家族(`-luna` 映射到 mini 档模型)。
- **Merged desktop app restart / 合并版桌面端重启适配** —— The Codex client restart tool now handles the merged desktop app (main process "ChatGPT") and still falls back to the legacy standalone Codex.app. Codex 客户端重启工具适配合并后的桌面端(主进程名 "ChatGPT"),同时兼容旧的独立 Codex.app。

### Fixes / 修复

- **Fake channel tags in replies / 回复里出现假 channel 标签** —— Third-party models imitating Codex's channel system emit literal `<commentary>` / `<context_addition>` tags in text; the gateway now strips the tags (streaming-safe across chunk boundaries) while keeping the inner text visible. 第三方模型模仿 Codex channel 机制时会在正文输出字面 `<commentary>` / `<context_addition>` 标签;网关现在剥离标签、保留正文,流式下跨 chunk 半截标签也能正确处理。
- **Redaction panic on multibyte values / 日志脱敏遇多字节字符崩溃** —— Key redaction sliced values at fixed byte offsets and panicked when the boundary fell inside a CJK character; slicing is now char-boundary safe. 密钥脱敏按固定字节偏移切片,边界落在中文等多字节字符中间会 panic;现已收敛到字符边界。
- **Request log truncation / 请求日志截断放宽** —— `raw_request` was truncated at 50KB before storage, which cut off large tool definitions (gpt-5.6 requests) and misled debugging; the limit now aligns with the 1MB per-field cap. `raw_request` 落库前 50KB 截断会把大体积工具定义(gpt-5.6 请求)切掉、误导排查;上限放宽到与单字段 1MB 上限对齐。

## [1.4.11] - 2026-07-07

### Added / 新增

- **Pet chat page / 宠物聊天页** —— Added a full main-window Pet Chat page backed by the same persisted chat history, pet persona, memory, and gateway chat path as the desktop pet. 新增主窗口宠物聊天页,复用桌宠同一份持久化聊天记录、人格、记忆和网关聊天链路。
- **LLM-powered pet behavior / 桌宠 LLM 智能化** —— The pet's startup greeting, 30-minute usage report, error explanations, and periodic ambient check-ins are now generated by the gateway model, each with a local fallback when the gateway is unavailable. After each chat the pet also asks the model to extract stable preferences into its memory. 桌宠的开机问候、30 分钟用量播报、报错解释和低频主动搭话改由网关模型生成,每处都带网关不可用时的本地兜底;每次聊天后还会让模型把稳定偏好提取进记忆。
- **Editable pet memory / 宠物记忆可编辑** —— The Pet Chat page can view/add/edit/delete what the pet remembers; memory and chat history clear independently and sync across the pet window and page via events. 宠物聊天页可查看/新增/编辑/删除宠物记忆;记忆与聊天记录分开清空,并通过事件在宠物窗口与页面间同步。
- **Playful pet interactions / 桌宠趣味互动** —— Claude Code working badge + finish celebration, load-based bounce tiers with a heavy-load sweat drop, a temper on repeated pokes, a "stuffed" look over the daily spend threshold, seasonal / late-night / Friday accessories, edge-docking, and long-reply bubbles that scroll instead of clipping. 新增 Claude Code 工作徽章+完成庆祝、按负载分级的弹跳(高负载冒汗)、连戳生气、花费超阈值"吃撑"、节日/深夜/周五装饰、贴边挂靠,以及长回复气泡内部滚动不被裁切。

### Improvements / 改进

- **Cleaner app information architecture / 更清晰的应用信息架构** —— Refined the major pages into clearer console-style sections: Overview, Providers, Provider Detail, Routes, Gateway, Diagnostics, Logs, MCP, Skills, Global Instructions, Clients, Settings, and Pet Chat now use more consistent headers, status panels, and primary work areas. 将主要页面整理成更清晰的控制台式结构:概览、供应商、供应商详情、路由、网关、诊断、日志、MCP、技能、全局指令、客户端、设置和宠物聊天都统一了入口、状态面板和主工作区层级。
- **Route and provider readability / 路由与供应商更易理解** —— Route strategies now present the effective request flow and provider priority more directly, while provider cards and provider detail pages surface health, identity, and actions with less visual noise. 路由策略更直接展示实际请求流和供应商优先级;供应商卡片与详情页更清楚呈现健康状态、身份信息和主要操作。
- **Pet chat layout / 宠物聊天布局** —— The conversation stream is fixed to the viewport and scrolls internally; memory editing moved next to the send box so long sessions no longer stretch the whole page. 宠物聊天的会话流固定在视口内并内部滚动;记忆编辑移动到发送框旁边,长会话不再把整页撑长。

### Fixes / 修复

- **Disabled provider routing / 禁用供应商仍被路由选中** —— Route candidate selection now filters both route-profile provider enablement and the provider's own `enabled` flag, preventing disabled providers from being picked. 路由候选现在同时过滤路由档位供应商启用状态和供应商自身 `enabled` 标记,避免禁用供应商仍被选中。
- **Pet chat behind a local proxy / 本机代理下宠物聊天不通** —— The pet's loopback request to the gateway now uses a `no_proxy` HTTP client, fixing the empty `HTTP 502` when a system/HTTP proxy (e.g. Clash) hijacked the `127.0.0.1` request. 宠物发往网关的回环请求改用 `no_proxy` 客户端,修复系统/HTTP 代理(如 Clash)劫持 `127.0.0.1` 请求导致的空 `HTTP 502`。
- **Pet chat on strict providers / 宠物聊天被严格供应商拒绝** —— Removed the hardcoded `temperature` some models reject (e.g. `kimi-for-coding` only allows `1`), and empty model replies no longer poison the history and trigger a follow-up `assistant must not be empty` 400. 去掉部分模型拒绝的写死 `temperature`(如 `kimi-for-coding` 仅允许 `1`),空回复不再污染历史触发后续 `assistant must not be empty` 的 400。

## [1.4.10] - 2026-07-03

### Improvements / 改进

- **Dependency security cleanup / 依赖安全清理** —— Upgraded `tauri-winrt-notification` to 0.7.3 to drop the vulnerable `quick-xml` 0.37 chain flagged by `cargo audit`; the remaining `plist` advisory has no fixed upstream release yet and is exempted by ID with a recorded recall condition. 升级 `tauri-winrt-notification` 至 0.7.3,消除 `cargo audit` 标记的 `quick-xml` 0.37 依赖链;`plist` 告警上游暂无修复版本,按 ID 豁免并注明回收条件。
- **Release automation / 发版自动化** —— Publishing a release now automatically updates the Homebrew tap cask, so `brew install --cask agentgate` always gets the latest version. 发布 release 后自动更新 Homebrew tap 的 cask,`brew install --cask agentgate` 始终安装最新版。

## [1.4.9] - 2026-07-01

### Added / 新增

- **Latest flagship models / 补齐最新旗舰模型** —— Added `gemini-3.5-flash`, `grok-4.3`, and `glm-5.2` to the built-in provider catalog with pricing, and bumped OpenAI's default model to `gpt-5.5`. 内置供应商目录补齐 `gemini-3.5-flash`、`grok-4.3`、`glm-5.2`（含价格），OpenAI 默认模型升级到 `gpt-5.5`。

### Improvements / 改进

- **CI supply-chain & lint gates / CI 供应链与静态检查门禁** —— CI now enforces Rust `clippy` + `rustfmt` and adds dependency security audits (`cargo audit` for Rust, `pnpm audit --prod` for the frontend); `react-router-dom` was upgraded to clear a production dependency advisory. CI 新增 Rust `clippy` + `rustfmt` 门禁和依赖安全审计（Rust `cargo audit`、前端 `pnpm audit --prod`）；升级 `react-router-dom` 清除生产依赖安全告警。

## [1.4.8] - 2026-06-26

### Added / 新增

- **Configurable request body limit / 请求体上限可配置** —— The gateway request body limit now defaults to 32 MB and can be changed in Settings or via `AGENTGATE_REQUEST_BODY_LIMIT_MB`; both paths are capped at 128 MB to avoid accidental memory blow-ups. 网关单次请求体上限默认 32 MB，可在设置里调整，也可用 `AGENTGATE_REQUEST_BODY_LIMIT_MB` 覆盖；两条路径都硬限制最高 128 MB，避免误设超大值打爆内存。

### Fixes / 修复

- **Friendly 413 errors / 413 错误提示可理解** —— Oversized requests now return a structured AgentGate error with guidance to start a new session, reduce payload size, or raise the configured limit, instead of surfacing Axum's raw `Payload Too Large` message. 请求过大时返回结构化 AgentGate 错误，并提示新开会话、减少内容或调大上限，不再把 Axum 原始 `Payload Too Large` 文案直接丢给用户。
- **Existing database upgrade path / 存量数据库升级路径** —— Existing v6 databases now receive the `request_body_limit_mb` column through a guarded v7 migration, preventing settings reads from breaking after upgrade. 存量 v6 数据库会通过带守卫的 v7 迁移补上 `request_body_limit_mb` 字段，避免升级后读取设置失败。

### Performance / 性能

- **Bounded request-log storage / 请求日志存储有界** —— Large raw/converted request and response bodies, SSE event logs, and trace JSON are capped per field before writing to SQLite; retention cleanup and manual log clearing now checkpoint and vacuum the database so deleted log pages can be returned to disk. 写入 SQLite 前会限制大体积原始/转换请求、响应、SSE 事件和 trace JSON 的单字段大小；保留期清理和手动清空日志后会执行 checkpoint 与 vacuum，让删除后的日志页释放回磁盘。

## [1.4.7] - 2026-06-23

### Added / 新增

- **Daily cost alert / 今日花费预警** —— Set a daily spend threshold; when today's total cost exceeds it, AgentGate fires a system notification + pet bubble (at most once per day). Configurable in Settings. 在设置里设一个每日花费阈值,当今日累计花费超过它时,AgentGate 发系统通知 + 桌宠气泡提醒(每天最多一次)。

### Fixes / 修复

- **Existing-user upgrade crash on missing columns / 存量用户升级缺列崩溃** —— New `gateway_settings` columns had been added in a code path that only runs for brand-new databases, so already-installed users got `DATABASE_ERROR` on every settings read after upgrading (blank Overview). New columns now go through versioned migrations with idempotent guards, and a regression test covers the existing-DB upgrade path. 新增的 `gateway_settings` 列此前加在只对全新数据库执行的代码路径里,导致已安装用户升级后每次读设置都 `DATABASE_ERROR`(概览白屏)。现改为走带幂等守卫的版本化迁移,并补了存量数据库升级的回归测试。

## [1.4.6] - 2026-06-22

### Added / 新增

- **Scenario routing by model name / 按模型名做场景路由** —— A route profile's provider can match on the requested model name (`model_name_match`), routing background subtasks (e.g. Claude Code `haiku` calls) to a cheaper provider while the main conversation stays on the primary. Works on all three protocols (Claude Code / Codex / Gemini) since it only depends on the model name. 路由档位的供应商可按请求模型名匹配(`model_name_match`)，把后台子任务(如 Claude Code 的 `haiku` 调用)分流到便宜供应商，主对话仍走主力。只依赖模型名，三协议(Claude Code / Codex / Gemini)全部生效。

### Fixes / 修复

- **Config write verification / 配置写后校验** —— After applying a client config (Codex / Claude Code / Claude Desktop / atomcode / Gemini / OpenCode), AgentGate reads the file back and byte-compares it; a mis-written config now reports an error instead of a false success. 应用客户端配置(Codex / Claude Code / Claude Desktop / atomcode / Gemini / OpenCode)后读回文件逐字节比对，写歪时报错而非假成功。

### Improvements / 改进

- **Slimmer Routes page / 路由页更精简** —— Removed the duplicated "Conditions" summary block (already shown per-provider) and the fallback chain now only renders in failover mode. 去掉与供应商行重复的「条件」汇总区块，失败转移链路仅在故障转移模式显示。

## [1.4.5] - 2026-06-22

### Fixes / 修复

- **Codex long-conversation stall / Codex 长对话卡死** —— Applying a Codex config now enlarges the context window, so auto-compaction no longer triggers early and freezes long chats. 应用 Codex 配置时加大上下文窗口，避免自动压缩过早触发、把长对话卡住。
- **Image+text dropped when sent to Codex / 发图文给 Codex 内容被丢弃** —— Fixed the message body being dropped when sending mixed image/text content to Codex. 修复混合图文消息发给 Codex 时正文被丢弃的问题。

### Performance / 性能

- **Skip summary round-trip for tiny history / 历史过小跳过摘要调用** —— When the middle history to compact is below ~1k tokens, the gateway skips the summary call and passes the request through, saving an upstream call / cost / latency. 待压缩的中间历史低于约 1k token 时，网关跳过摘要调用、原样透传请求，省一次上游调用 / 成本 / 延迟。

## [1.4.3] - 2026-06-15

### Added / 新增

- **Claude Code desktop pet status alerts / Claude Code 桌宠状态提醒** —— The pet right-click menu adds "Receive CC alerts", which injects/removes hooks in Claude Code's `settings.json`; it receives `UserPromptSubmit` / `PreToolUse` / `Notification` / `Stop` status events through a local file mailbox and syncs them to the pet's bubble. 桌宠右键菜单新增"接收 CC 提醒"，可向 Claude Code `settings.json` 注入/移除 hooks；通过本地文件信箱接收 `UserPromptSubmit` / `PreToolUse` / `Notification` / `Stop` 状态事件，同步到桌宠气泡。
- **Claude Code system notifications / Claude Code 系统通知** —— System notifications fire when waiting for permission (`permission_prompt`) and when the current turn finishes (`idle_prompt` / `Stop`); working events do not send system notifications, to avoid spam. On macOS dev mode it uses `osascript display notification` as an observable path, avoiding the Tauri notification plugin's false success in dev mode. 等待授权(`permission_prompt`)和本轮完成(`idle_prompt` / `Stop`)时触发系统通知；工作中事件不发系统通知，避免刷屏。macOS dev 模式使用 `osascript display notification` 作为可观测路径，避免 Tauri notification 插件 dev 模式假成功。

### Improvements / 改进

- **Pet bubble priority / 桌宠气泡优先级** —— CC waiting/done bubbles are not immediately replaced by ordinary pet bubbles; high-frequency working events do not pop bubbles. CC 等待/完成气泡不会被普通桌宠气泡立即顶掉；高频 working 事件不弹气泡。
- **Community entry / 社区入口** —— A GitHub Discussions / community discussion entry is added to the top of the README. README 顶部增加 GitHub Discussions / 社区讨论入口。

## [1.4.2] - 2026-06-13

### Added / 新增

- **Standalone promotion entry (GitHub Pages site) / 独立传播入口（GitHub Pages 站点）** —— Launched https://dengmengmian.github.io/agentgate-ai/, a bilingual CLI-aesthetic landing page; the download section connects to the GitHub Releases API to auto-fetch the latest installer and recommends the matching platform by browser UA; includes complete SEO with sitemap / robots / OG / Twitter card / JSON-LD. 上线 https://dengmengmian.github.io/agentgate-ai/，中英双语，CLI 美学落地页；下载区接 GitHub Releases API 自动拉最新版安装包，按浏览器 UA 推荐对应平台；含 sitemap / robots / OG / Twitter card / JSON-LD 完整 SEO。
- **Bilingual docs / docs 双语化** —— Added 8 Chinese docs (`full-reference-zh.md` + 7 `use-*-zh.md` tutorials); the English versions get bidirectional switch links at the top, and all guide links on the Chinese site point to the Chinese versions. 新增 8 个中文版 docs（`full-reference-zh.md` + 7 篇 `use-*-zh.md` 教程）；英文版顶部加双向切换链接，中文站点 guide 链接全部指向中文版。
- **Dashboard onboarding hints / Dashboard 引导提示** —— The Dashboard shows different guidance cards and action entries based on current state (provider not configured / gateway not started / client not configured / no requests yet), so a newcomer can click straight through the prompts to run their first request. Dashboard 按当前状态（供应商没配 / 网关没启动 / 没配客户端 / 还没请求）显示不同的引导卡片和操作入口，新手按提示一路点过去就能跑通第一条请求。
- **Setup Wizard completion guidance / Setup Wizard 完成引导** —— After a successful configuration it jumps to "configure a client" or back to the overview; on failure it provides error-recovery entries (edit key and retry / skip to settings). 配置成功后跳"配客户端"或回概览，失败提供错误恢复入口（编辑 key 重试 / 跳设置）。

### Improvements / 改进

- **README and site slogan unified / README 与站点 slogan 统一** —— "Turn AI Agents' official APIs into your own local model entry" + three selling points (official experience unchanged / model entry under your control / request flow visible), aligned bidirectionally in Chinese and English. "让 AI Agent 的官方接口，变成你的本地模型入口" + 三卖点（官方体验不变 / 模型入口归你管 / 请求链路看得见）中英双向对齐。
- **README slimmed down / README 瘦身** —— The main README is compressed from 990 lines to 117 lines, with long content split out to `docs/full-reference.md`; a 30-second first-visit conversion, with deep docs carried separately. 主 README 从 990 行压到 117 行，长内容拆到 `docs/full-reference.md`；30 秒首访转化、深度文档单独承接。
- **New demo image / demo 图换新版** —— Replaced with a CLI-aesthetic trace flow diagram (client → AgentGate → providers + live trace log), echoing the three selling points. 替换为贴 CLI 美学的 trace 流图（客户端 → AgentGate → providers + 实时 trace 日志），呼应三卖点。

### Performance / 性能

- **LTO+strip enabled for release binary / release 二进制开 LTO+strip** —— The Cargo release profile adds `lto = "thin"` / `codegen-units = 1` / `strip`, compressing binary size by 30-50%. Cargo release profile 加 `lto = "thin"` / `codegen-units = 1` / `strip`，二进制体积压 30-50%。
- **Request-log writes moved off the response hot path / 请求日志写入移出响应热路径** —— The pass_through `log_to_db` is changed to `tokio::task::spawn_blocking`, so non-streaming / error responses no longer synchronously wait for the SQLite INSERT before returning, saving a few to tens of milliseconds of first-byte latency. pass_through 的 `log_to_db` 改 `tokio::task::spawn_blocking`，非流式 / 错误响应不再同步等 SQLite INSERT 才返回，首字节延迟省几毫秒到几十毫秒。

## [1.4.1] - 2026-06-13

### Fixes / 修复

- **Steady memory growth on long runs / 长期运行内存持续上涨** —— Session storage's in-memory cache was previously capped only by entry count (1000); long agent sessions can have single histories up to several MB, so resident memory could grow to 10GB+; it now evicts by a byte budget (64MB), with evicted sessions backed up by the disk layer and functionality unaffected. 会话存储的内存缓存原本只按条数(1000)封顶，长 agent 会话单条历史可达数 MB，常驻运行内存可涨到 10GB+；现按字节预算(64MB)淘汰，被淘汰会话由磁盘层兜底，功能不受影响。

### Performance / 性能

- **Polling paused when the window is hidden / 窗口隐藏时暂停轮询** —— After the main window is minimized / sent to the tray, each page's polling (previously the overview page fired up to 7 queries every 5 seconds) pauses automatically, and refreshes immediately on returning to the foreground. 主窗口最小化/收进托盘后，各页面轮询(此前概览页每 5 秒最多 7 个查询)自动暂停，回到前台立即刷新补上。
- **Covering index for stats aggregation / 统计聚合覆盖索引** —— Lifetime stats such as the bottom KPIs, previously a full-table scan every 5 seconds, now use a covering index; measured on a 306MB log database, a single query went 74ms→15ms, and cold start no longer reads the entire database file. 底部 KPI 等 lifetime 统计每 5 秒的全表扫描改走覆盖索引；实测 306MB 日志库单次查询 74ms→15ms，冷启动不再读整个库文件。
- **Faster first paint / 首屏更快** —— Pages are lazy-loaded by route, main bundle 850KB→416KB; heavy dependencies such as Markdown rendering load only when entering the corresponding page, and the desktop pet window no longer loads the main UI code. 页面按路由懒加载，主 bundle 850KB→416KB；Markdown 渲染等重依赖只在进入对应页面时加载，桌面宠物窗口不再加载主界面代码。
- **Log search debounce / 日志搜索防抖** —— Queries fire only 300ms after typing stops, no longer hitting the database twice per keystroke. 关键字停止输入 300ms 后才查询，不再每个键击打两次数据库。
- **Single polling source for gateway state / 网关状态单一轮询源** —— The topbar and overview page, which each polled the same state, are merged into one place, so start/stop operations sync to all interfaces instantly. 顶栏/概览页各自轮询同一状态合并为一处，启停操作即时同步所有界面。

### Documentation / 文档

- A "download by system" table is added to the top of the README, linking directly to the matching platform installer; the GitHub description provider count is updated to 26. / README 顶部新增"按系统下载"表，直达对应平台安装包；GitHub 简介 Provider 数更新为 26。

## [1.4.0] - 2026-06-11

### Added / 新增

- **GitHub Copilot integration / GitHub Copilot 接入** —— Added a `copilot` provider type: run Claude Code / Codex with a Copilot subscription (Pro / Business), no separate Anthropic API needed. GitHub OAuth tokens (`gho_`/`ghu_`) are automatically exchanged for and renew Copilot credentials (hash-cached, never stored in plaintext); tool follow-up / history-compaction requests are automatically tagged `x-initiator: agent` so they don't consume the premium-request quota; model-name dash→dot is auto-normalized; pasting a `gho_` token is auto-recognized as the Copilot type. ⚠️ Using a Copilot subscription outside official clients is a GitHub ToS gray area; the feature is entirely optional — see the README risk note. 新增 `copilot` Provider 类型：用 Copilot 订阅(Pro / Business)跑 Claude Code / Codex，无需单独的 Anthropic API。GitHub OAuth token(`gho_`/`ghu_`)自动换取并续期 Copilot 凭证(hash 缓存、不落明文)；工具续写 / 历史压缩请求自动标记 `x-initiator: agent`，不消耗 premium 请求额度；模型名 dash→dot 自动归一化；粘贴 `gho_` token 自动识别为 Copilot 类型。⚠️ 在官方客户端之外使用 Copilot 订阅属 GitHub ToS 灰色地带，功能完全可选，详见 README 风险声明。
- **Windows client integration / Windows 客户端集成** —— Running-client detection, automatic Codex restart after applying config, and Claude Desktop integration complete their Windows implementation (previously macOS-only). 运行中客户端检测、应用配置后自动重启 Codex、Claude Desktop 集成补齐 Windows 实现(此前仅 macOS)。

### Improvements / 改进

- **Thinking quality first / 思考质量优先** —— When converting to Claude-family models, thinking is "on if supported" (opus-4.6+/sonnet-4.6 use adaptive, the rest use budget); it turns off only on explicit `effort: none/off`; meanwhile it holds three constraints — haiku unsupported, forced tool-call mutual exclusion, and budget must be less than max_tokens — to avoid 400s. 转换到 Claude 系模型时"支持就开"思考(opus-4.6+/sonnet-4.6 用 adaptive、其余用 budget)，显式 `effort: none/off` 才关；同时守住 haiku 不支持、强制工具调用互斥、budget 须小于 max_tokens 三类约束，避免 400。
- **Prompt cache auto-injection extended to the pass-through path / Prompt cache 自动注入扩展到直通路径** —— When Claude Code connects directly to an Anthropic-compatible upstream, `cache_control` breakpoints are also auto-injected, budget-aware and not exceeding the 4-breakpoint cap; the money saved by cache hits in long conversations shows up directly in the cost dashboard. Claude Code 直连 Anthropic 兼容上游时也自动注入 `cache_control` 断点，预算感知不超 4 断点上限；长对话缓存命中省的钱直接体现在成本仪表盘。
- **Fastest-route cold-start fallback / 最快路由冷启动兜底** —— With active health probing enabled, the "fastest" routing strategy uses probe latency as a fallback for providers with no recent request records, no longer blindly ranking them last (probing consumes a small amount of tokens and requires explicit enabling via `AGENTGATE_LATENCY_PROBE_MINUTES`). 开启主动健康探测后，"最快"路由策略对无近期请求记录的 provider 用探测延迟兜底，不再盲排末尾(探测消耗少量 token，需 `AGENTGATE_LATENCY_PROBE_MINUTES` 显式开启)。
- **Configurable circuit-breaker threshold / 熔断阈值可配** —— `AGENTGATE_CB_FAILURE_THRESHOLD=N` changes it to trip only after N consecutive failures (default 1 keeps the original behavior), easing accidental harm to healthy providers from occasional jitter. `AGENTGATE_CB_FAILURE_THRESHOLD=N` 改为连续失败 N 次才跳闸(默认 1 保持原行为)，缓解偶发抖动误伤健康 provider。
- **Session-pollution error hint / 会话污染错误提示** —— When leftover truncated tool arguments in history cause an upstream JSON-parse 400, it is rewritten into an actionable "start a new session / upgrade" hint instead of dropping a raw error. 历史残留截断工具参数导致上游 JSON 解析 400 时，改写为"开新会话 / 升级"的可操作提示，不再丢裸错误。

### Fixes / 修复

- **Slow overview first paint on large databases / 概览页大库首屏慢** —— The cost aggregation query is moved onto an index plus a new `(source, timestamp)` composite index; measured on a tens-of-thousands-of-rows database, first-paint DB time dropped from about 4 seconds to about 0.2 seconds. 成本聚合查询改走索引 + 新增 `(source, timestamp)` 复合索引；实测数万行级库首屏 DB 时间从约 4 秒降到约 0.2 秒。
- **Streaming Chinese/emoji corruption / 流式中文/emoji 乱码** —— Fixed the issue where SSE multi-byte characters cut by network packet boundaries became `�` (handled uniformly across 8 streaming paths). 修复 SSE 多字节字符被网络包边界切断时变 `�` 的问题(8 处流式路径统一处理)。
- **Windows usage stats and MCP config failure / Windows 用量统计与 MCP 配置失效** —— The absence of a `HOME` environment variable on Windows caused official-client usage sync and MCP config reads to fail silently; added a `USERPROFILE` fallback. Windows 无 `HOME` 环境变量导致官方客户端用量同步、MCP 配置读取静默失败，补 `USERPROFILE` 回退。
- **Windows desktop pet white background / Windows 桌面宠物白底** —— The transparent window adds `shadow(false)`, fully transparent consistent with macOS. 透明窗口补 `shadow(false)`，与 macOS 一致全透明。
- **Claude Code compaction request misjudgment / Claude Code 压缩请求误判** —— Compaction detection is narrowed from full-text matching to the system prefix + the last user message, avoiding the whole session being stripped of tools when file content containing the marker string is read. 压缩检测从全文匹配收窄到 system 前缀 + 最后一条用户消息，避免读到含标记串的文件内容时整个会话被剥工具。
- **Kimi reasoning_effort / Kimi reasoning_effort** —— No longer forwards the `reasoning_effort` that Kimi does not recognize, avoiding a potential 400. 不再向 Kimi 透传它不识别的 `reasoning_effort`，避免潜在 400。
- **Chinese long-conversation compaction not triggering / 中文长对话压缩不触发** —— Self-compaction's token estimate counts CJK as 1 char = 1 token, fixing the previous underestimate that caused Chinese-heavy conversation compaction not to trigger. 自压缩的 token 估算对 CJK 按 1 字 1 token，修正此前低估导致中文重对话压缩不触发。

### Security / 安全

- Redaction for request logs / diagnostic bundles is extended to `x-api-key`, the `api_key` field, and Gemini-style `?key=` query parameters. / 请求日志 / 诊断包脱敏扩展到 `x-api-key`、`api_key` 字段、Gemini 风格 `?key=` 查询参数。
- Headless mode adds Host / Origin checks to prevent DNS rebinding; domain access requires whitelisting via `AGENTGATE_ALLOWED_HOSTS`. / Headless 模式新增 Host / Origin 校验，防 DNS rebinding；域名访问需 `AGENTGATE_ALLOWED_HOSTS` 白名单放行。

## [1.3.7] - 2026-06-10

### Added / 新增

- **Config share code / 配置分享码** —— Client config can be exported to a single-line share code with one click; paste it on another machine to import, no need to retype ports, tokens, or model mappings. 客户端配置可一键导出为单行分享码，另一台机器粘贴即导入，免去手敲端口、token、模型映射。
- **Per-model context window config / 模型级上下文窗口配置** —— The provider editor's capability matrix gains a "context window" column to override the context window (token) for a single model; leave it blank to auto-display and use the built-in default. Provider 编辑页的能力矩阵新增"上下文窗口"列，可为单个模型覆盖上下文窗口(token)；留空自动显示并使用内置默认值。

### Improvements / 改进

- **Auto-compaction of long history on by default + adaptive threshold / 长历史自压缩默认开启 + 阈值自适应** —— Auto-compaction of overly long conversation history changes from off-by-default to on-by-default, with the trigger threshold adapting to 85% of the model's context window (built-in MiMo / DeepSeek 128K, uncataloged models fall back to 110K); the reserved segment budget narrows accordingly for small windows. `AGENTGATE_AUTO_COMPACT=off` turns it all off. 自动压缩超长对话历史从默认关改为默认开，触发阈值按模型上下文窗口的 85% 自适应(内置 MiMo / DeepSeek 128K，未收录的模型退回 110K)，小窗口下保留段预算同步收窄。`AGENTGATE_AUTO_COMPACT=off` 可全关。
- **Codex remote compaction (experimental, off by default) / Codex 远程压缩(实验性，默认关)** —— Handles Codex CLI's `remote_compaction_v2` protocol: when Codex has a long context it sends the summary request to the gateway, which generates the summary with the current main provider and returns it via SSE, avoiding the 503 caused by the hardcoded `gpt-5.5-openai-compact` model. Set `AGENTGATE_CODEX_COMPACT=1` to enable; SSE protocol-layer compatibility is covered by two test layers — mirrored and real `eventsource-stream` parsing. 接住 Codex CLI 的 `remote_compaction_v2` 协议：Codex 在长上下文时把摘要请求发到网关，网关用当前主供应商生成摘要再以 SSE 返回，避免硬编码 `gpt-5.5-openai-compact` 模型导致的 503。设 `AGENTGATE_CODEX_COMPACT=1` 开启；SSE 协议层兼容性已被镜像 + 真实 `eventsource-stream` 解析两层测试覆盖。
- **Dashboard "Today's Codex compactions" card / 首页"今日 Codex 压缩"卡片** —— The Dashboard adds a counter card showing how many Codex compactions completed through the gateway that day, making it easy to see whether the experimental feature is actually firing. Dashboard 加一张计数卡，展示当天通过网关完成的 Codex compaction 次数，便于看实验功能是否真的命中。
- **Large component splitting / 大组件拆分** —— The three overly long pages — Settings / Tools / Routes — are split into smaller subcomponents to reduce the blast radius of future maintenance changes; external behavior unchanged. Settings / Tools / Routes 三个超长页面拆成更小子组件，降低后续维护改动半径，对外行为不变。

## [1.3.6] - 2026-06-09

### Added / 新增

- **Config history is deletable / 配置历史可删除** —— Both client config history and global instruction (CLAUDE.md / AGENTS.md) history can be deleted one by one, no longer piling up forever. The "initial" snapshot is protected and cannot be deleted, guaranteeing you can always roll back to the original config from before adopting AgentGate. 客户端配置历史和全局指令(CLAUDE.md / AGENTS.md)历史都能逐条删除，不再一直堆叠。「初始」快照受保护、不可删，保证随时能回滚到接入 AgentGate 前的原始配置。

### Fixes / 修复

- **Fix agentgate-serve (CLI / Docker) build failure / 修复 agentgate-serve(CLI / Docker)无法构建** —— The connection pool rework missed the headless binary, which prevented the CLI / Docker image from building. 连接池改造漏改了 headless 二进制，导致 CLI / Docker 镜像编不出来。

### Improvements / 改进

- **Database switched to a connection pool / 数据库换连接池** —— Multiple concurrent gateway requests can now hold independent database connections simultaneously (SQLite WAL supports multiple readers), removing the old global Mutex serialization bottleneck. Imperceptible in daily use; responses are steadier when QPS is high or log volume is large. 网关多个并发请求现在可以同时持有独立数据库连接(SQLite WAL 支持多 reader)，旧的全局 Mutex 串行瓶颈解除。日常使用无感，QPS 高或日志量大时响应更稳。
- **Steadier database concurrency / 数据库并发更稳** —— Connections add busy_timeout so high-concurrency writes no longer error out directly; when the database version is higher than the current app, it clearly prompts to upgrade instead of running on. 连接加 busy_timeout，高并发写不再直接报错；数据库版本高于当前应用时明确提示升级而不是硬跑。

## [1.3.5] - 2026-06-08

### Fixes / 修复

- **Vision routing for image requests aligned across three entry points / 带图请求的视觉路由对齐三个入口** —— Previously only `/v1/responses` skipped vision-unsupported providers when a request carried an image; `/v1/chat/completions` and `/v1/messages` did not, so image requests could be routed to a provider doomed to fail. Now all three entry points are unified: vision-unsupported providers are skipped for image requests, and failover scenarios automatically pick a vision-capable candidate. Only the current turn's images are considered; historical images don't affect routing. 此前只有 `/v1/responses` 会在请求带图时跳过不支持视觉的供应商；`/v1/chat/completions` 和 `/v1/messages` 不会，导致带图请求可能被路由到必然失败的供应商。现在三个入口统一：带图时跳过显式不支持视觉的供应商，失败转移场景自动选到支持视觉的候选。只看当前轮次的图片，历史图片不影响路由。
- **usage stat boundary fix / usage 统计的边界修正** —— When the upstream returns `prompt_tokens` as `null`, token stats now correctly fall back to `input_tokens` and no longer record 0. 上游返回 `prompt_tokens` 为 `null` 时，token 统计现在会正确回退到 `input_tokens`，不再记成 0。
- **Auto-update endpoint fix / 自动更新 endpoint 修正** —— It previously pointed at the old repo name `AgentGate` and was only reachable via GitHub's rename redirect; it now points at the real repo `agentgate-ai`, avoiding silently missing updates if the redirect fails. 之前指向旧仓库名 `AgentGate`，仅靠 GitHub 改名重定向才能访问；现改为真实仓库 `agentgate-ai`，避免重定向失效时静默收不到更新。
- **Update failures no longer silent / 更新失败不再静默** —— If installation fails after the user clicks "Update", it now shows "Failed to install update" instead of silently clearing; background auto-check failures are also logged to the console for easier troubleshooting. 用户点击「更新」后若安装失败，现会显示「安装更新失败」而不是无声清空；后台自动检查失败也会记录到 console 便于排查。

### Improvements / 改进

- **Gateway request handling consolidation (internal refactor) / 网关请求处理收敛(内部重构)** —— Consolidates the usage extraction, failover candidate sorting, and log token params scattered across `routes.rs` into a single source (adds `gateway/usage.rs`, `gateway/failover.rs`), reducing "fix one spot, miss another". External behavior unchanged. 把散落在 `routes.rs` 里的 usage 提取、失败转移候选排序、日志 token 参数收敛为单一来源(新增 `gateway/usage.rs`、`gateway/failover.rs`)，减少"改一处漏一处"。对外行为不变。

## [1.3.4] - 2026-06-08

### Added / 新增

- **Desktop pet AI chat / 桌面宠物 AI 聊天** —— Double-click the pet to open a chat box; 9 characters each have their own tone and reactions, remember things you've said (like your name) across restarts, and show the real reason on failure (no main provider set / API key missing / call error) instead of an unrelated greeting. 双击宠物开聊天框，9 个角色各自有专属语气和反应，记住你说过的事(名字等)跨重启保留；失败显示真实原因(未设主供应商 / API key 缺失 / 调用错误)而不是无关问候。
- **Native right-click menu / 原生右键菜单** —— Switch character / open gateway · logs · settings / clear memory / hide pet, fully detached from the pet window, no longer blocking the app below when expanded. 切换角色 / 打开网关 · 日志 · 设置 / 清空记忆 / 隐藏宠物，完全脱离宠物窗口，不再因展开挡住下方应用。
- **Click-through mode / 鼠标穿透模式** —— Let clicks pass through the pet to the app below. The right-click menu / Settings / tray entry points keep the state in sync, so you won't get stuck in click-through. 让点击穿过宠物到下方应用。右键菜单 / Settings / tray 三处入口同步状态，不会卡在穿透里出不来。
- **Single-click poke / 单击戳一下** —— Drag threshold 4px; a true single click triggers a character-specific reaction bubble. 拖拽阈值 4px，真单击触发角色专属反应气泡。

### Improvements / 改进

- **Smaller window, less screen blocking / 窗口更小，挡屏更少** —— Default window 140×200 (was 220×240), grows on demand when a bubble / chat box appears and restores immediately when it disappears. 默认窗口 140×200(原 220×240)，气泡 / 聊天框出现时按需撑高，消失立刻还原。
- **More natural chat box exit / 聊天框退场更自然** —— Auto-collapses after sending a line; clicking outside the window or pressing Esc also closes it. 发完一句自动收起；点窗口外或按 Esc 也能关。
- **Nicer bubbles / 气泡更耐看** —— Hovering pauses the disappear timer; clicking closes it immediately. 悬停时暂停消失计时，点击立即关闭。
- **pet_chat path handling / pet_chat 路径处理** —— Uses `smart_append_path` to avoid the double-join 404 when base_url already contains `/v1`. 用 `smart_append_path` 避免 base_url 已含 `/v1` 时的双拼 404。
- **Gateway recognizes the Pet client / 网关识别 Pet 客户端** —— Logs can distinguish requests initiated by AgentGate Pet. 日志能区分 AgentGate Pet 发起的请求。

### Performance / 性能

- **Polling from 3s to 10s + event-driven / 轮询从 3s 改 10s + 事件驱动** —— The Rust-side gateway actively emits on start/stop and the frontend listens to refresh immediately; when `document.hidden`, all polling and event listening stop, near-zero cost in the background. Rust 端 gateway 启停时主动 emit，前端 listen 立即刷新；`document.hidden` 时全停轮询和事件监听，后台几乎零消耗。
- **mousemove merge + rAF throttle / mousemove 合并 + rAF 节流** —— Drag-threshold and eye-follow share one listener; eye-follow writes the DOM transform directly via ref, no longer triggering a React render on every mouse move. drag-threshold 和 eye-follow 合一个 listener，eye-follow 用 ref 直写 DOM transform，不再每次鼠标动都触发 React render。
- **onMoved window-drag debounce 300ms / onMoved 拖窗 debounce 300ms** —— Previously wrote to the DB once per frame (60Hz); now writes once only after you stop. 之前每帧写一次 DB(60Hz)，现在停手才写一次。
- **`get_pet_gateway_state` split into lite + stats / `get_pet_gateway_state` 拆 lite + stats** —— The 10s poll only does an indexed query of state + last_error; the full-table SUM aggregation runs only before the 30-minute stats bubble triggers. 10s 轮询只走索引查 state + last_error；全表 SUM 聚合放到 30 分钟 stats 气泡触发前才跑。
- **9 SVG packages React.memo'd / 9 个 SVG 包 React.memo** —— SVG nodes no longer reconcile on unrelated state changes. 无关状态变化时不再 reconcile SVG 节点。

## [1.3.3] - 2026-06-05

### Added / 新增

- **Full Markdown preview / 完整 Markdown 预览** —— Session conversations and global-instruction previews now use GFM rendering, supporting tables, task lists, code blocks and other common Markdown content. 会话对话和全局指令预览接入 GFM 渲染，支持表格、任务列表、代码块等常见 Markdown 内容。
- **Request detail trail and cost / 请求详情链路与成本** —— Show route, provider, model, status, tokens, cost, fallback and the error-trail summary together in a single view. 在同一视图集中展示路由、供应商、模型、状态、tokens、成本、fallback 和错误链路摘要。

### Improvements / 改进

- **Logs page easier to scan / 日志页更易扫** —— Added a filter summary, layered common / advanced filters, collapsed the sync entry, and stopped showing 0ms for unrecorded latency. 新增筛选摘要，常用 / 高级筛选分层，同步入口折叠，未记录延迟不再显示 0ms。
- **Clearer session conversations / 会话对话更清晰** —— Tool calls / tool results are shown in layers, with tool results capped in height and keeping their raw output format. 工具调用 / 工具结果分层展示，工具结果限制高度并保留原始输出格式。

### Fixes / 修复

- **Routing-profile stats no longer stuck at 0 / 路由策略统计不再一直为 0** —— When older gateway logs lack `route_decision.profile_id`, attribute the route to the current default profile for stats. 旧网关日志缺少 `route_decision.profile_id` 时，按 route 归到当前默认策略统计。

## [1.3.2] - 2026-06-05

### Added / 新增

- **Claude Desktop integration / Claude Desktop 接入**（macOS） —— Point Claude Desktop's third-party inference gateway at AgentGate, applied in one click with history rollback. 把 Claude Desktop 的第三方推理网关指向 AgentGate，一键应用、可历史回滚。
- **View full conversation in session / 会话查看完整对话** —— The logs "session" view opens the full Claude Code / Codex chat record, with a one-click-copy resume command. 日志「会话」视图点开就能看 Claude Code / Codex 的完整聊天记录，并附一键复制的恢复命令。
- **Smart routing model selection / 智能路由选模** —— Failover can use a "cheapest" / "fastest" strategy, automatically picking a provider by unit price or latency. 失败转移可选「最便宜」/「最快」策略，自动按单价或延迟挑供应商。
- **Active health probing / 主动健康探测**（off by default / 默认关） —— Periodically probe providers in the background and show the result on the card, without affecting actual routing. 后台定期探活供应商，结果显示在卡片上，不影响实际路由。

### Improvements / 改进

- **More accurate cost stats / 成本统计更准** —— Requests from passthrough clients are also counted; models without a price are marked "no price" to distinguish "truly free" from "uncomputable"; the cost breakdown adds a "by strategy" dimension and filters out noise entries with no tokens. 直通客户端的请求也能算成本；缺价模型标「无价格」，区分「真免费」和「算不出」；成本分解新增「按策略」维度，并过滤掉无 token 的噪音条目。
- **Enhanced log filtering / 日志筛选增强** —— Model filter changed to a dropdown, error types add "network" and "protocol conversion", and the "session" view now follows the filters too. 模型筛选改成下拉，错误类型补上「网络」「协议转换」，「会话」视图也跟随筛选了。
- **MiniMax compatibility / MiniMax 兼容** —— Completed field handling for the strict API, so integration no longer easily returns 400. 补齐对严格 API 的字段处理，接入不再容易报 400。

### Fixes / 修复

- Fixed cost always computing as $0 — switched to matching prices by model across providers, with a built-in price table added. / 修复成本一直算成 $0 —— 改为按模型跨供应商匹配价格，并补充内置价格表。
- Added a connect timeout to the gateway to avoid requests hanging when the upstream is unreachable; fixed the stats bias where Gemini passthrough was wrongly counted as success on a stream interruption. / 网关新增建连超时，避免上游不可达时请求挂死；修正 Gemini 直通在流式中断时被误记为成功的统计偏差。

## [1.3.1] - 2026-06-02

### Added / 新增

- **Global instruction file management / 全局指令文件管理** —— Edit `~/.claude/CLAUDE.md` / `~/.codex/AGENTS.md` directly inside AgentGate, with 4 built-in templates (minimal Chinese rules / TDD / code review / security audit) that can overwrite or append. Auto-snapshot before writing to disk, with one-click rollback. 在 AgentGate 内直接编辑 `~/.claude/CLAUDE.md` / `~/.codex/AGENTS.md`，4 个内置模板（极简中文规范 / TDD / 代码评审 / 安全审计）可覆盖或追加。写盘前自动 snapshot，可一键回滚。
- **Client config version history and one-click rollback / 客户端配置版本史与一键回滚** —— For all 5 clients, auto-snapshot the on-disk config before every "apply / disable / switch", keeping the initial version + the latest 10 rolling. A new "history" button on the card allows one-click rollback after a second confirmation. 5 个客户端每次「应用 / 关闭 / 切换」前自动 snapshot 盘上配置，保留初始版本 + 最近 10 条滚动。卡片上新增「历史」按钮，二次确认后一键回滚。
- **One-click restart of the Codex desktop app / 一键重启 Codex 桌面应用**（macOS） —— After Codex is configured, the dialog adds a restart button that matches by basename to kill only the desktop App, not the CLI. Manual trigger by default; not shown on Windows / Linux. Codex 应用配置后弹窗多一个重启按钮，按 basename 精确匹配只杀桌面 App，不动 CLI。默认手动触发，Windows / Linux 不显示。
- **Node speed test / 节点测速** —— The Providers page sends a 1-token probe to all enabled providers in parallel, sorts by latency, and color-codes connect / TTFB / total in three segments. Manual trigger. Providers 页对所有启用 provider 并行发 1-token 探测，按延迟排序，连接 / TTFB / 总耗时三段染色。手动触发。
- **Gateway refinement layer / 网关精炼层** —— A global switch under "Settings → General", with three items off by default: request field filtering (stripping unsupported fields), reasoning-param correction (capping `thinking.budget_tokens` / `reasoning.effort` to the provider range), and error-response normalization (recognizing Chinese/English context-overlength hints and uniformly tagging `context_length_exceeded`). Built-in DeepSeek / MiMo / Anthropic / OpenAI / Kimi rules. 「设置 → 通用」全局开关，三项默认关：请求字段过滤（剥不支持字段）、推理参数校正（`thinking.budget_tokens` / `reasoning.effort` 收口到 provider 范围）、错误响应归一（识别中英文上下文超长提示统一标 `context_length_exceeded`）。内置 DeepSeek / MiMo / Anthropic / OpenAI / Kimi 规则。

### Improvements / 改进

- **Clients page switched to master-detail layout / 客户端页改主从布局** —— The 5 thick-card accordion becomes a 260px list on the left + detail on the right. The list permanently shows a tri-state status dot (integrated / detected / undetected), the detail area is no longer chopped up by cards, and the selected item is remembered in sessionStorage. 5 张厚卡手风琴改成左侧 260px 列表 + 右侧详情。列表常驻显示三态状态点（已接入 / 已检测 / 未检测），详情区不再被卡片切碎，选中项 sessionStorage 记忆。

## [1.3.0] - 2026-06-01

### Added / 新增

- **Structured diagnostics for test-connection failures / 测试连接失败结构化诊断** —— Failures are no longer a bare `HTTP 401`. 13 typical failure types each get a one-line cause + suggestion, and 11 major providers get one-click buttons like "regenerate key", "check account balance" or "enable plugin" that jump to the corresponding console. 失败不再裸 `HTTP 401`。13 种典型失败各给一句话原因 + 建议，11 家主流 provider 直接给「去重建 key」「查看账户余额」「去开通插件」等一键按钮，跳到对应控制台。
- **Quick setup auto-detects the key in the clipboard / 快速配置自动识别剪贴板里的 key** —— On entering quick setup, read the clipboard once quietly; if recognized as a known key, pop a banner above the input, and clicking "fill in" brings it straight into the form + auto-selects the provider type. If it can't read it / the content isn't a key, it doesn't disturb you at all. 进入快速配置时悄悄读一次剪贴板，识别为已知 key 就在输入框上方弹 banner，点「填入」直接带进表单 + 自动选好 provider type。读不到 / 内容不是 key 就完全不打扰。
- **Detect the process after applying a client config / 应用客户端配置后检测进程** —— After Codex / Claude Code / OpenCode / Gemini CLI / AtomCode config is applied successfully, the dialog shows the config path + process status; if it's running it gives the PID + one-click-copy `kill <pid>`. It never auto-kills. Codex / Claude Code / OpenCode / Gemini CLI / AtomCode 应用配置成功后，弹窗显示配置 path + 进程状态，正在跑就给 PID + 一键复制 `kill <pid>`。从不自动 kill。

### Fixes / 修复

- Test connection wasn't forwarding `extra_headers`, so Kimi always returned 401 under UA validation, looking like a wrong key. / 测试连接没转发 `extra_headers`，Kimi 在 UA 校验下一律 401，看起来像 key 错了。
- Test connection wasn't parsing multi-key JSON arrays; `["sk-a","sk-b"]` was concatenated into Bearer literally, guaranteeing a 401. / 测试连接没解析多 key JSON 数组，`["sk-a","sk-b"]` 当字面量拼 Bearer 必 401。
- When editing a provider, the form always showed 1 empty slot, so you couldn't see how many existing keys there were or which one you were editing; now editing backfills all keys. / 编辑 provider 时表单总显示 1 个空槽，看不到现有几把 key、改的是哪一把；现在编辑时回填全部 key。

## [1.2.4] - 2026-05-30

### Fixes / 修复

- **AtomCode / OpenCode one-click setup now uses the `agentgate` virtual model / AtomCode / OpenCode 一键配置改用 `agentgate` 虚拟模型** —— It no longer bakes the current provider's real model name into the client, so switching providers won't carry an old model name and trigger a 400. Native real model names still pass through per Model Mapping rules. 不再把当前 provider 的真实模型名固化进客户端，切换 provider 后不会带着旧模型名 400。原生真实模型名仍按 Model Mapping 规则透传。

## [1.2.3] - 2026-05-30

### Fixes / 修复

- **DeepSeek default models converge on v4 / DeepSeek 默认模型收敛到 v4** —— It no longer targets the soon-to-be-retired `deepseek-chat` / `deepseek-reasoner` as auto-config targets. 不再把即将下线的 `deepseek-chat` / `deepseek-reasoner` 作为自动配置目标。
- **MiMo Token Plan keeps a consistent regional domain / MiMo Token Plan 区域域名保持一致** —— `sk-*` uses the open API domain; `tp-*` uses `token-plan-{cn|sgp|ams}` and keeps the same regional host. `sk-*` 用开放 API 域名，`tp-*` 用 `token-plan-{cn|sgp|ams}` 并保持同一区域 host。
- **MiMo `web_search` auto-degrades / MiMo `web_search` 自动降级** —— Token Plan strips it up front; pay-as-you-go strips and retries once when the Plugin isn't enabled. Token Plan 预剥离；按量付费遇 Plugin 未开通时剥离后重试一次。
- **Claude Code direct-connect models no longer write the `[1m]` suffix / Claude Code 直连模型不再写 `[1m]` 后缀** —— The default recommended mapping uses plain model IDs. 默认推荐映射用普通模型 ID。
- **Capability degradations are diagnosable / 能力降级可诊断** —— Image stripping, `web_search` downgrades, MCP advisories and other events are written to the request log's `degradation_events`. 图片剥离、`web_search` 降级、MCP advisory 等事件写入请求日志的 `degradation_events`。

## [1.2.2] - 2026-05-29

### Fixes / 修复

- **Windows desktop pet white background / Windows 桌面宠物白色背景** —— Windows' WebView2 control defaults to an opaque background, so the pet window gets an explicit dark background and is no longer a white card on Windows. macOS behavior is unchanged. Windows 的 WebView2 控件默认底色不透明，给 pet 窗口加显式深色背景，宠物在 Windows 上不再是个白卡片。macOS 行为不变。

## [1.2.1] - 2026-05-29

### Fixes / 修复

- **MiMo / DeepSeek no longer auto-configure `[1m]` long-context models / MiMo / DeepSeek 不再自动配置 `[1m]` 长上下文模型** —— Plain model IDs are enough and avoid the upstream `Not supported model` response. 普通模型 ID 已经够用，避免上游返回 `Not supported model`。
- **Quick setup tested the wrong provider / 快速配置测试连接打错 provider** —— It's set active immediately after creation, so the test no longer follows the old route. 创建后立即设为 active，不再沿用旧路由测试。
- **Quick setup fills in models + detects capabilities / 快速配置补齐模型 + 能力识别** —— Consistent with the "Add provider → fetch and detect capabilities" flow. 与「添加供应商 → 拉取并识别能力」流程一致。
- **Failed connection tests show the specific reason / 连接测试失败显示具体原因** —— No more vague errors. 不再笼统报错。

## [1.2.0] - 2026-05-28

### Added / 新增

- **MiMo auto-selects the domain by key type and region / MiMo 按 key 类型和区域自动选域名** —— `sk-*` uses the open API, `tp-*` uses token-plan-{cn|sgp|ams}, avoiding 401s from mismatched key, Chat host and Anthropic host. `sk-*` 走开放 API，`tp-*` 走 token-plan-{cn|sgp|ams}，避免 key、Chat host、Anthropic host 不匹配 401。
- **MiMo / DeepSeek recommended model mappings auto-complete / MiMo / DeepSeek 推荐模型映射自动补齐** —— Creating a provider, fetching models, and applying config auto-complete the `gpt-*` → upstream model / `claude-*` → upstream model mappings. 创建 provider、拉模型、应用配置时自动补齐 `gpt-*` → 上游模型 / `claude-*` → 上游模型 的映射。
- **Provider form restructured into three sections / Provider 表单三段重构** —— Basics → Models & capabilities → Advanced (collapsed by default). Newcomers are no longer overwhelmed by 8+ fields laid out flat. 基础 → 模型与能力 → 高级（默认折叠）。新手不再被 8+ 字段平铺吓住。
- **New providers auto-pick the latest model / 新建 provider 后自动挑最新模型** —— It auto-fetches upstream models in the background → detects capabilities → chooses the default + reasoning model by version/tier, without hardcoding model names. 后台自动拉上游模型 → 识别能力 → 按版本/层级选 default + reasoning model，不 hardcode 模型名。
- **Provider card direct-connect protocol chip / Provider 卡片直连协议 chip** —— "Direct Chat" and "Direct Anthropic" show at a glance which clients connect directly without protocol conversion. 「直连 Chat」「直连 Anthropic」一眼看出哪些客户端走直连不需要协议转换。

### Fixes / 修复

- **Native direct-connect model resolution rules converge / 原生直连模型解析规则收敛** —— When no Mapping matches, the client model is kept as-is instead of auto-falling back to default_model (the protocol-conversion path still has a fallback). 未命中 Mapping 时保持客户端 model 原样，不再自动回退到 default_model（协议转换路径继续兜底）。
- **Connection test "cache support detection" hang / 测试连接「缓存支持检测」卡死** —— A misconfigured anthropic_base_url used to take 240s+ to fail; there's now a 15s hard limit, and the frontend polling no longer restarts the check every 10s. 误配 anthropic_base_url 时要等满 240s+ 才失败；现在 15s 硬上限。前端轮询不再每 10s 重启检测。
- **Codex config self-check no longer falsely reports "not configured" / Codex 配置自检不再误报「未配置」** —— It adapts to the 1.1.0 "hijack OpenAI provider" approach, so a clean `auth.json` no longer warns. 适配 1.1.0 的「劫持 OpenAI provider」写法，干净的 `auth.json` 不再 warn。

### Renamed / 重命名

- **Service provider → Provider / 服务商 → 供应商** —— Aligned with CC-Switch's common terminology; existing users searching "服务商" can still use it as an alias. 对齐 CC-Switch 通用语，老用户搜索关键词仍可用「服务商」别名。

## [1.1.2] - 2026-05-28

- **Auto-restart after an update installs / 更新安装完成后自动重启** —— No more manual quit + reopen. 不再需要手动 quit + 重开。

## [1.1.1] - 2026-05-27

- **Themes expanded to 8 / 主题扩展到 8 套** —— Warm Amber, Sunny Day, Steel Blue, Pine Forest, Purple Night, Rice Beige, Misty Blue, Sakura Pink. The settings page switches to a 2×4 swatch preview. 暖琥珀、晴日、钢蓝、松林、紫夜、米麻、雾蓝、樱粉。设置页换成 2×4 色板预览。
- **App icon redrawn / 应用图标重画** —— A beige rounded-square base + tri-color atomic orbit, so the shape is legible in the macOS dark dock. 米黄圆角矩形底 + 三色原子轨道，macOS 暗色 dock 里能看出形状了。
- Fixed the `agentgate-serve` CLI binary build failure. / 修复 `agentgate-serve` CLI 二进制编译失败。

## [1.1.0] - 2026-05-27

A major round of work around Codex.app IDE plugin compatibility + Xiaomi MiMo integration + cache hit-rate visualization. 围绕 Codex.app IDE 插件兼容 + 小米 MiMo 集成 + 缓存命中率可视化做了一轮大改。

### Added / 新增

- **Codex.app IDE plugin compatibility / Codex.app IDE 插件兼容** —— Switched to a "hijack the OpenAI provider + `requires_openai_auth = true`" config, so IDE plugin / Browser / Mobile / quota lookup all work while conversation requests actually go through AgentGate; ChatGPT OAuth tokens are fully preserved. 改用「劫持 OpenAI provider + `requires_openai_auth = true`」配置，IDE 插件 / Browser / Mobile / 配额查询全部可用，对话请求实际走 AgentGate；ChatGPT OAuth tokens 完全保留。
- **Compressed request body support / 压缩请求体支持** —— Codex.app's zstd / ChatGPT desktop client's gzip no longer 500 with "invalid utf-8". Codex.app 的 zstd / ChatGPT 桌面客户端的 gzip 不再 500 报「invalid utf-8」。
- **Session Affinity / 会话亲和** —— When the upstream returns `cached_tokens > 0`, the session → provider binding is recorded for 1 hour, so later requests in the same session prefer the same provider to maximize prompt cache hits. 上游回 `cached_tokens > 0` 时记录 session → provider 1 小时绑定，同会话后续请求优先复用同 provider，最大化 prompt cache 命中。
- **SSE first-frame error protection / SSE 首段错误保护** —— When the upstream returns HTTP 200 but stuffs an error event into the first frame (quota / ban / rate-limit), it auto-switches to the next provider with zero client impact. 上游 HTTP 200 但首段塞错误事件（quota / ban / rate-limit）时自动切换下个 provider，客户端零感知。
- **Cache token write/read split / 缓存 Token 写/读拆分** —— The Dashboard shows a "cache" inline footer with an auto-computed hit rate. Dashboard 显示「缓存」inline footer，命中率自动计算。
- **Dashboard time-range switcher / Dashboard 时段切换** —— Today / 7 days / 14 days / 30 days tabs. 今天 / 7 天 / 14 天 / 30 天 tab。
- **Live KPI footer / 实时 KPI 页脚** —— 6 metrics at the bottom of the Dashboard (active connections / uptime / total requests / Tokens / cost / success rate), refreshing every 5 seconds. Dashboard 底部 6 项指标（活跃连接 / 运行时间 / 累计请求 / Tokens / 费用 / 成功率），5 秒刷新。
- **Tray menu live status / 托盘菜单实时状态** —— Current active provider / today's request count / gateway port, with a one-click "switch provider" submenu. 当前 active provider / 今日请求数 / 网关端口，「切换服务商」子菜单一键切换。
- **Log pagination / 日志分页** —— 100 rows per page, switching filters auto-returns to the first page. 每页 100 条，过滤切换自动回首页。
- **Xiaomi MiMo first-class support / 小米 MiMo 一等公民支持** —— All 5 chat models, multi-turn reasoning_content backfill, Token Plan area domain matching, Web Search Plugin auto-degradation, built-in pricing. 完整 5 个聊天模型，多轮 reasoning_content 回填，Token Plan 区域域名匹配，Web Search Plugin 自动降级，内置定价。
- **Per-model capability matrix / 每模型能力矩阵** —— 8 capabilities (text / vision / audio_in / tts / video_in / reasoning / tools / web_search) checkable independently per model. Image requests auto-swap to a vision-capable model; unchecking web_search for a model stops it being sent. 8 种能力（text / vision / audio_in / tts / video_in / reasoning / tools / web_search）每个模型独立勾选。带图请求自动 swap 到支持 vision 的模型；用户取消勾选某 model 的 web_search 即停止下发。
- **Codex `web_search_preview` → MiMo `web_search` translation / Codex `web_search_preview` → MiMo `web_search` 翻译** —— Codex's web search capability passes through to MiMo. Codex 的联网搜索能力穿透到 MiMo。
- **Precise client identification in logs / 日志精确客户端识别** —— Distinguishes Codex / Claude Code / OpenCode / Cursor / Cherry Studio / Continue / Cline / Roo Code etc. by User-Agent. 按 User-Agent 区分 Codex / Claude Code / OpenCode / Cursor / Cherry Studio / Continue / Cline / Roo Code 等。
- **Dashboard rework / Dashboard 重做** —— 9 redundant cards → 5 core metrics for today at the top + a 7-day bar chart. 9 张冗余卡 → 顶部今日 5 个核心指标 + 7 天柱状图。

### Fixes / 修复

- **MiMo multi-turn history image pass-through / MiMo 多轮历史图片穿透** —— When a Codex session carried an image early on, later text-only requests no longer 404. Codex 会话早期带过图，后续纯文本请求不再 404。
- **DeepSeek API key recognition / DeepSeek API key 识别** —— `sk-` + 32 hex digits is no longer mistaken for OpenAI. `sk-` + 32 位 hex 不再被错认 OpenAI。
- **Vision probe false-positive on MiMo / Vision 探针对 MiMo 误报** —— All 4xx are treated as unsupported (previously only 400 was recognized). 4xx 都视为不支持（之前只识别 400）。
- **Auto-retry on network jitter / 网络抖动自动重试** —— Pool silent dead connections / connect failures retry with backoff. 池静默死连接 / connect 失败带 backoff 重试。

## [1.0.0] - 2026-05-20

Official release. / 正式发布。

## [0.8.x] - 2026-05-18 ~ 2026-05-20

- Pull desktop pets back into the visible area when their restored coordinates are off-screen. / 桌面宠物在屏幕外坐标恢复时拉回可见区域。
- macOS / Docker release flow fixes (`latest.json` missing platforms, CLI sub-program signing, Dockerfile dependencies and context). / macOS / Docker release 流程修复（`latest.json` 缺平台、CLI 子程序签名、Dockerfile 依赖与上下文）。
- First-install empty log table no longer throws a database error. / 首次安装空日志表不再报数据库错误。
- The desktop package no longer includes the headless CLI (the CLI is now a separate artifact). / 桌面包不再包含 headless CLI（CLI 改为独立产物）。

## [0.5.x] - 2026-05-18

- Isolate per-platform CLI build outputs in the release. / 隔离 release 各平台 CLI 构建输出。
- Clean up example request logs (`req-seed-*`) on startup. / 清理启动时的示例请求日志（`req-seed-*`）。
- Tray menu correctly recognizes Chinese language when launched from macOS Dock/Finder. / macOS Dock/Finder 启动时托盘菜单正确识别中文语言。
- The standalone `agentgate-serve` CLI release artifact now ships with each version. / 独立 `agentgate-serve` CLI 发布产物开始随版本一起发。

## [0.5.0] - 2026-05-18

### Added / 新增

- **Native Gemini format support / Gemini 原生格式支持** —— Two-way Codex ↔ Gemini conversion; Gemini CLI can reach any provider like DeepSeek / Kimi through AgentGate. Codex → Gemini 双向转换，Gemini CLI 可通过 AgentGate 接 DeepSeek / Kimi 等任意 provider。
- **Task-level smart routing / 任务级智能路由** —— Auto-selects provider and model by input length / images / tools / system keywords. 按输入长度 / 图片 / 工具 / 系统关键词自动选 provider 和模型。
- **Headless serving mode / Headless 服务模式** —— `agentgate-serve` CLI binary + Docker. `agentgate-serve` CLI 二进制 + Docker。
- Logs show dates (MM-DD HH:MM:SS). / 日志时间显示日期（MM-DD HH:MM:SS）。

## [0.4.0] - 2026-05-18

### Added / 新增

- **Cost tracking / 费用追踪** —— Built-in pricing for 22 models, every request's cost auto-computed, dashboard shows total / today / average, and the pricing table in settings can be edited inline or overridden. 22 个内置模型价格，每条请求自动计算费用，仪表盘展示总费用 / 今日 / 平均，设置页价格表可内联编辑或自定义覆盖。
- **Multi-account rotation / 多账号轮转** —— One provider supports multiple API Keys (JSON array), auto round-robin. 同 provider 支持多 API Key（JSON 数组），自动 round-robin。
- **Prompt Cache auto-injection / Prompt Cache 自动注入** —— The Codex → Anthropic conversion path auto-adds `cache_control` to system / tools / last assistant. Codex → Anthropic 转换路径自动给 system / tools / 最后 assistant 打 `cache_control`。
- **Cache capability auto-detection / 缓存能力自动探测** —— On connection test, sends the same request twice and checks `cache_read_input_tokens > 0`. 测试连接时发两次相同请求，看 `cache_read_input_tokens > 0`。
- **Provider health panel / Provider 健康面板** —— Cards embed 1h / 24h success rate, average latency, P95 latency, request count. 卡片内嵌 1h / 24h 成功率、平均延迟、P95 延迟、请求数。
- **Request retry / 请求重试** —— 429 / 500 / 502 / 503 auto backoff retry, respecting `Retry-After`, switching Key on each retry. 429 / 500 / 502 / 503 自动退避重试，尊重 `Retry-After`，每次重试换 Key。

### Fixes / 修复

- Cost not computed (provider name case-insensitive matching + backfill history on startup). / 费用不计算（provider 名大小写不敏感匹配 + 启动时回填历史）。
- kimi-for-coding price missing. / kimi-for-coding 价格缺失。
- Gemini CLI / AtomCode hardcoded copy changed to i18n. / Gemini CLI / AtomCode 硬编码文案改 i18n。

## [0.3.0] - 2026-05-18

### Added / 新增

- **23 provider presets / 23 个 provider 预设** —— Up from the original 7, adding Google Gemini / xAI / Mistral / Groq / Together / Fireworks / Cerebras / Perplexity / Cohere / Zhipu GLM / Qwen / SiliconFlow / Volcengine / Baichuan / StepFun / 01.AI. 原 7 个，新增 Google Gemini / xAI / Mistral / Groq / Together / Fireworks / Cerebras / Perplexity / Cohere / 智谱 GLM / 通义千问 / 硅基流动 / 火山引擎 / 百川 / 阶跃星辰 / 零一万物。
- **One-click setup for Gemini CLI / AtomCode clients / Gemini CLI / AtomCode 客户端一键配置** —— Writes the matching config file and supports switching between official and AgentGate. 写入对应配置文件，支持切换官方 / AgentGate。

### Fixes / 修复

- Send `response.failed` to the client when an Anthropic SSE stream errors or ends with empty content. / Anthropic SSE 流错误 / 空内容结束时给客户端发 `response.failed`。
- Handle `\r\n\r\n` SSE framing and the `event:X` format without a space. / 兼容 `\r\n\r\n` SSE 分帧和 `event:X` 无空格格式。
- Requests after image recognition no longer switch back to the original provider (vision routing only looks at the last message). / 识图后请求不切回原 provider（vision 路由只看最后一条）。
- Gemini CLI config not taking effect (env vars written to `.env`, auth type set to `gemini-api-key`). / Gemini CLI 配置不生效（环境变量写到 `.env`、auth type 设为 `gemini-api-key`）。
- AtomCode reporting "no active Provider configured" (added the top-level `default_provider` field). / AtomCode 报「未配置活跃 Provider」（补 `default_provider` 顶层字段）。

## [0.2.x] - 2026-05-15 ~ 2026-05-18

- Model mapping dropdown supports list selection plus manual input, and is visible in the dark theme. / 模型映射下拉组件支持下拉选择 + 手动输入，深色主题可见。
- macOS ad-hoc signing + auto-update support (`.app.tar.gz`). / macOS ad-hoc 签名 + 自动更新支持（`.app.tar.gz`）。

## [0.1.9] - 2026-05-15

### Added / 新增

- **Native Claude Messages API support / Claude Messages API 原生支持** —— When `provider_type=anthropic`, Codex requests are converted to Claude's native Messages format, with full support for `tool_use` / `tool_result` / `input_schema` / `thinking.budget_tokens`. `provider_type=anthropic` 时 Codex 请求转 Claude 原生 Messages 格式，完整支持 `tool_use` / `tool_result` / `input_schema` / `thinking.budget_tokens`。
- **URL-driven routing / URL 驱动的路由机制** —— Adds `responses_base_url`; when set, requests pass through to the upstream Responses API endpoint, with passthrough / Claude conversion / Chat conversion chosen by field. 新增 `responses_base_url`，有值就透传到上游 Responses API 端点；按字段判断走透传 / Claude 转换 / Chat 转换。
- **Smart URL joining / 智能 URL 拼接** —— Recognizes both a full URL and a base URL. 填完整 URL 或 base URL 都识别。
- **`local_shell` tool conversion / `local_shell` 工具转换** —— Converts the Codex built-in tool into a standard `shell` function. Codex 内置工具转标准 `shell` function。
- **Anthropic / MiniMax added as provider types / Provider 类型增加 Anthropic / MiniMax** —— Protocols show a friendly label on the card. 协议在卡片显示友好标签。

### Fixes / 修复

- DeepSeek no longer receives a meaningless `thinking` field. / DeepSeek 不再收到无意义的 `thinking` 字段。
- Non-streaming empty `choices` no longer hangs Codex. / 非流式空 `choices` 不再让 Codex 挂起。
- Tool call IDs are truncated to 64 characters (Responses API limit). / Tool call ID 截断至 64 字符（Responses API 限制）。
- `anthropic_base_url` no longer wrongly triggers Claude conversion. / `anthropic_base_url` 不再误触发 Claude 转换。

## [0.1.5] - 2025-05-14

### Fixes / 修复

- **Chinese content causing a gateway panic / 中文内容导致网关 panic** —— All string truncation now uses `is_char_boundary` for safe truncation. 所有字符串截断改用 `is_char_boundary` 安全截断。
- **Tool output truncation crashing Codex / Tool output 截断导致 Codex 崩溃** —— Removed the 4000-byte truncation limit; tool output is passed through as-is. 移除 4000 字节截断限制，tool output 原样透传。
- **Error response format / 错误响应格式** —— Added a `type` field so clients no longer show "Unknown error". 添加 `type` 字段，客户端不再显示「Unknown error」。
- **Auth 5xx errors / 认证 5xx 错** —— `GATEWAY_AUTH_*` now correctly returns 401. `GATEWAY_AUTH_*` 正确返回 401。
- **SSE event log overflow / SSE 事件日志溢出** —— Strictly enforce the 1MB cap. 严格守住 1MB 上限。
- **Deleting a provider not cascading / 删除 provider 不级联** —— Cleans up route_profile_providers in sync. 同步清理 route_profile_providers。
- **API key display mask / API Key 显示遮罩** —— Fixed-length `sk-1****cdef`. 固定长度 `sk-1****cdef`。

### Performance / 性能

- `get_stats` query merged from 14 SQL statements into 3. / `get_stats` 查询从 14 条 SQL 合并为 3 条。
- Added an index on `request_logs.timestamp`. / 添加 `request_logs.timestamp` 索引。

## [0.1.4] - 2025-05-14

### Added / 新增

- **One-click switch for Claude Code / Claude Code 一键切换** —— The same save / restore mechanism as Codex. 与 Codex 同样的 save / restore 机制。
- **One-click setup for OpenCode / OpenCode 一键配置** —— Writes `~/.config/opencode/opencode.json`. 写入 `~/.config/opencode/opencode.json`。
- **Routing system / 路由系统** —— Automatically creates 3 default routes per protocol; new providers are added to every route chain automatically; the UI supports creating routes and inline renaming. 按协议自动建 3 个默认路由，新增 provider 自动加入所有路由链；UI 支持创建、inline 重命名。
- **Navigation rework / 导航重构** —— Overview → Providers → Routes → Gateway → Clients → Logs → Diagnostics → Settings. 概览 → 服务商 → 路由 → 网关 → 客户端 → 日志 → 诊断 → 设置。

## [0.1.3] - 2025-05-14

### Added / 新增

- **One-click Codex config switching / Codex 配置一键切换** —— "Switch to official / Switch to AgentGate" saves and restores `config.toml` + `auth.json` as a whole. 「切换到官方 / 切换到 AgentGate」整体保存恢复 `config.toml` + `auth.json`。
- **Preserve official sessions / 保留官方会话** —— Keeps the original OAuth tokens when applying AgentGate, so switching back to official doesn't lose chat history. 应用 AgentGate 时保留原始 OAuth tokens，切回官方对话记录不丢。
- **Pollution detection warning / 污染检测警告** —— Shows a yellow warning when it detects `OPENAI_API_KEY` was overwritten by an old version. 检测到 `OPENAI_API_KEY` 被旧版覆盖时弹黄色警告。

## [0.1.2] - 2025-05-13

- Version number is read from the Tauri API at runtime, no longer hardcoded. / 版本号从 Tauri API 运行时读取，不再硬编码。
- Update-check failures are handled silently. / 检查更新失败静默处理。
- Codex default model updated to `gpt-5.5`. / Codex 默认模型更新到 `gpt-5.5`。
- Added backup history + one-click restore on the tools page. / 工具页加备份历史 + 一键恢复。

## [0.1.1] - 2025-05-13

- App icon, in-app auto-update, a check-for-updates button in Settings, multi-platform automated CI/CD releases, and a bilingual Chinese/English README. / 应用图标、应用内自动更新、设置页检查更新按钮、CI/CD 多平台自动发版、中英双语 README。

## [0.1.0] - 2025-05-13

Open-sourced from a personal tool into a public release. / 自用开源成公开版本。

- Protocol-conversion gateway (OpenAI Responses / Anthropic Messages / Chat Completions interconversion). / 协议转换网关（OpenAI Responses / Anthropic Messages / Chat Completions 互转）。
- Multi-provider management (DeepSeek / OpenAI / OpenRouter / Kimi / custom). / 多 provider 管理（DeepSeek / OpenAI / OpenRouter / Kimi / 自定义）。
- Route Profile routing configuration (multi-provider priority + failover). / Route Profile 路由配置（多 provider 优先级 + failover）。
- One-click tool setup (Codex / Claude Code / OpenCode). / 工具一键配置（Codex / Claude Code / OpenCode）。
- Model mapping, custom headers, reasoning effort passthrough. / 模型映射、自定义请求头、reasoning effort 透传。
- Token usage, cost statistics, auto-refreshing dashboard. / Token 用量、费用统计、Dashboard 自动刷新。
- Request logs + diagnostic self-check + exportable diagnostic bundle. / 请求日志 + 诊断自检 + 导出诊断包。
- System tray, launch at startup, bilingual Chinese/English, automatic config backups. / 系统托盘、开机自启、中英双语、配置自动备份。
