# Use Codex with DeepSeek through MuxLayer

中文：[让 Codex 使用 DeepSeek](./use-codex-with-deepseek-zh.md)

MuxLayer turns Codex's official Responses API entry into a local model entry you control. Codex keeps sending Responses API requests, while MuxLayer routes them to DeepSeek with protocol conversion or native pass-through, model mapping, failover, request logs, and cost tracking.

## When to use this

Use this guide if you want:

- Codex to call DeepSeek models while keeping a one-click path back to the official config.
- OpenAI Responses API requests from Codex converted to DeepSeek-compatible Chat Completions or Anthropic-compatible endpoints.
- One-click switching between official Codex config and MuxLayer config.
- A local gateway that can later switch Codex from DeepSeek to MiMo, OpenAI, Kimi, GLM, DashScope, or another provider.

## Quick Setup

1. Download MuxLayer from [Releases](../../releases) and open the app.
2. Go to **Quick Setup** or **Providers**.
3. Add a DeepSeek provider and paste your DeepSeek API key.
4. Start the gateway from **Overview** or **Gateway**. The default client endpoint is `http://127.0.0.1:9090`.
5. Open **Clients** and click **Apply Config** on the Codex card.
6. Send a test message in Codex.

MuxLayer keeps the official Codex configuration restorable, so you can switch back from the Codex card when needed.

## What MuxLayer configures

| Codex side | MuxLayer side | DeepSeek side |
|---|---|---|
| OpenAI Responses API | `/v1/responses` local gateway route | DeepSeek Chat Completions or Anthropic-compatible endpoint |
| Codex model names | Model Mapping or `muxlayer` virtual model (`agentgate` still works) | DeepSeek model IDs such as `deepseek-flash` or `deepseek-v4-pro` |
| Codex tools and streaming | Protocol conversion and request tracing | Provider-specific DeepSeek handling |

## Chinese Notes / 中文说明

如果你搜索的是“Codex 使用 DeepSeek”“Codex 接入 DeepSeek”“DeepSeek 作为 Codex 后端”，MuxLayer 的作用是把 Codex 原本发往官方的 Responses API 入口变成本地模型入口，再由本地决定转换或直连到 DeepSeek。

常见路径是：

```text
Codex -> http://127.0.0.1:9090/v1/responses -> MuxLayer -> DeepSeek
```

你不需要长期手改 Codex 配置文件，也不需要在 DeepSeek、MiMo、OpenAI 等 Provider 之间来回改模型名。MuxLayer 会通过 Provider、Route Profile、Model Mapping 和 `muxlayer` 虚拟模型（仍兼容 `agentgate`）处理这些差异。

## Why MuxLayer does not pass through to DeepSeek's Responses API

`deepseek-flash` officially supports the Responses API, including image input. In practice direct pass-through still produces worse agent behavior for the regular coding models, so MuxLayer keeps the default route on protocol conversion and only allows native pass-through for explicitly supported model/tool combinations.

Four reasons, all reproducible:

**1. Every tool is dropped, and the model starts fabricating tool calls.**
Codex gpt-5.6+ puts tool definitions in an `additional_tools` item inside the `input` array rather than in top-level `tools`. DeepSeek only reads top-level `tools`, so on a direct connection it sees no tools at all. The model knows it is supposed to call one, has no usable definition, and emits raw DSML markup into its answer instead:

```text
<|DSML|tool_calls>
<|DSML|invoke name="exec_command">
<|DSML|parameter name="cmd" string="true">cat README.md</|DSML|parameter>
```

Codex cannot parse that, so the model appears to be talking nonsense.

**2. Codex's `exec` tool is rejected outright.**
DeepSeek's Responses API accepts exactly one custom tool, `apply_patch`. Anything else returns 400:

```text
Unsupported custom tool: 'exec'. Only 'apply_patch' is supported.
```

Codex always sends `exec`, so even with the tools intact a direct connection fails.

**3. Reasoning continuity breaks silently across turns.**
Codex sends `include: ["reasoning.encrypted_content"]` on every turn to carry the previous reasoning chain forward. DeepSeek does not support `include` or `encrypted_content` and **ignores them silently** — no error, every turn simply starts from scratch. The conversion path compensates with DeepSeek V4 thinking-history backfill; pass-through has no such compensation.

**4. Pass-through skips all DeepSeek-specific handling.**
For the text-only `deepseek-v4-pro`, images are stripped with an explicit notice; schema cleaning, message reordering, and V4 thinking-history reasoning backfill all live on the conversion path. `deepseek-flash` preserves image input; native pass-through skips the conversion-layer fixes.

### If you still want to pass through

Fill in a Responses endpoint on the provider and the gateway will try. To keep the problems above from wasting requests, two gates are built in — hitting either one falls back to protocol conversion:

- The target model is not on the upstream Responses API's supported list (DeepSeek: `deepseek-flash` only).
- The request carries a custom tool the upstream does not accept (DeepSeek: `apply_patch` only), including Codex 0.152+ `exec` nested inside a `functions` namespace.

In other words, Codex falls back to conversion even with a pass-through endpoint configured. That is deliberate and no further handling is planned.

## Troubleshooting

| Symptom | Check |
|---|---|
| Codex still calls the official endpoint | Re-open **Clients** and apply the Codex config again. |
| DeepSeek returns a model error | Check the DeepSeek provider's default model and Model Mapping. |
| Gateway is unreachable | Make sure the MuxLayer gateway is running on `127.0.0.1:9090`; `1420` is only the development UI port. |
| You want to restore official Codex | Use the Codex card's switch-back action in **Clients**. |

## Related

- [Use Codex Desktop with third-party APIs and plugins](./use-codex-desktop-with-third-party-api-and-plugins.md)
- [Use Codex with Xiaomi MiMo](./use-codex-with-mimo.md)
- [Main README](../README.md)
- [中文 README](../README_ZH.md)
