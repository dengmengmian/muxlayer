import { useState, useEffect } from "react";
import { X, Loader2, Sparkles, Eye, EyeOff } from "lucide-react";
import {
  PROVIDER_TYPES,
  PROTOCOLS,
  ALL_CAPABILITIES,
  CAPABILITY_LABELS,
} from "@/types/provider";
import {
  PROVIDER_PRESETS,
  isKnownMimoEndpointUrl,
  resolveKnownProviderEndpoints,
  resolveProviderPresetForKey,
} from "@/data/providerPresets";
import { GENERATED_PROVIDER_CATALOG } from "@/data/generatedProviderCatalog";
import { useI18n } from "@/lib/i18n";
import { toast } from "@/components/common/Toast";
import * as api from "@/lib/api";
import { detectProviderType } from "@/lib/keyDetection";
import {
  normalizeModelsForProvider,
  pickModelsForProvider,
} from "@/lib/modelHeuristics";
import type {
  ProviderView,
  CreateProviderInput,
  UpdateProviderInput,
} from "@/types/provider";

interface ProviderFormDialogProps {
  open: boolean;
  provider?: ProviderView | null;
  onSubmit: (data: CreateProviderInput | UpdateProviderInput) => void;
  onClose: () => void;
}

/** catalog 内置的该模型上下文窗口,用作输入框 placeholder(让用户看到默认值)。 */
function catalogContextWindow(
  providerType: string,
  model: string
): number | undefined {
  const entry = (
    GENERATED_PROVIDER_CATALOG as unknown as Record<
      string,
      { models?: Array<{ id: string; contextWindow?: number }> }
    >
  )[providerType];
  return entry?.models?.find((m) => m.id === model)?.contextWindow;
}

export function ProviderFormDialog({
  open,
  provider,
  onSubmit,
  onClose,
}: ProviderFormDialogProps) {
  const { t } = useI18n();
  const isEdit = !!provider;

  const [name, setName] = useState("");
  const [providerType, setProviderType] = useState("deepseek");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKeys, setApiKeys] = useState<string[]>([""]);
  const [visibleApiKeys, setVisibleApiKeys] = useState<boolean[]>([false]);
  const [defaultModel, setDefaultModel] = useState("");
  const [reasoningModel, setReasoningModel] = useState("");
  const [supportedModels, setSupportedModels] = useState("");
  const [modelCapabilities, setModelCapabilities] = useState<
    Record<string, string[]>
  >({});
  const [modelContextWindows, setModelContextWindows] = useState<
    Record<string, number>
  >({});
  const [seedingCaps, setSeedingCaps] = useState(false);
  const [modelMapping, setModelMapping] = useState<Record<string, string>>({});
  const [extraHeaders, setExtraHeaders] = useState("");
  const [anthropicBaseUrl, setAnthropicBaseUrl] = useState("");
  const [responsesBaseUrl, setResponsesBaseUrl] = useState("");
  const [autoCacheControl, setAutoCacheControl] = useState(true);
  const [protocols, setProtocols] = useState<string[]>([
    "openai_chat_completions",
  ]);
  const [timeoutSeconds, setTimeoutSeconds] = useState("120");
  const [enabled, setEnabled] = useState(true);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [fetchingModels, setFetchingModels] = useState(false);
  const [newMappingClient, setNewMappingClient] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [showCapMatrix, setShowCapMatrix] = useState(false);
  const [quickMode, setQuickMode] = useState(true);
  const [quickKey, setQuickKey] = useState("");
  const [detectedType, setDetectedType] = useState<string | null>(null);

  const applyPreset = (type: string) => {
    const preset = PROVIDER_PRESETS[type];
    if (!preset || isEdit) return;
    setBaseUrl(preset.baseUrl);
    setProtocols(preset.protocols);
    setDefaultModel(preset.defaultModel);
    setReasoningModel(preset.reasoningModel ?? "");
    setAnthropicBaseUrl(preset.anthropicBaseUrl ?? "");
    setResponsesBaseUrl(preset.responsesBaseUrl ?? "");
    setExtraHeaders(preset.extraHeaders ?? "");
    // 创建场景下永远覆盖 name——用户每选一次 type 都视作"我想要这个 provider 的默认名"
    const typeLabel = PROVIDER_TYPES.find((t) => t.value === type)?.label;
    if (typeLabel) setName(typeLabel);
    // auto_cache_control：anthropic 类型或带 anthropic 端点自动 ON，其他默认 OFF
    // 用户基本不需要关心这个，藏到高级里只显示状态
    setAutoCacheControl(
      type === "anthropic" || type === "claude" || !!preset.anthropicBaseUrl
    );
  };

  // MiMo-specific: when user enters a tp-* key after picking MiMo from the
  // dropdown, swap to the token-plan host. Only swap if the current URL is
  // one of the two known MiMo URLs (don't clobber custom edits).
  useEffect(() => {
    if (providerType !== "mimo") return;
    const key = (apiKeys[0] || "").trim();
    if (!key || (!key.startsWith("tp-") && !key.startsWith("sk-"))) return;
    const target = resolveKnownProviderEndpoints(
      providerType,
      key,
      baseUrl,
      anthropicBaseUrl
    );
    if (!target) return;
    if (isKnownMimoEndpointUrl(baseUrl) && baseUrl !== target.baseUrl)
      setBaseUrl(target.baseUrl);
    if (
      (!anthropicBaseUrl || isKnownMimoEndpointUrl(anthropicBaseUrl)) &&
      anthropicBaseUrl !== target.anthropicBaseUrl
    ) {
      setAnthropicBaseUrl(target.anthropicBaseUrl ?? "");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerType, apiKeys]);

  const handleQuickKeyChange = (key: string) => {
    setQuickKey(key);
    setDetectedType(detectProviderType(key));
  };

  const quickProviderTypes = PROVIDER_TYPES.filter(
    (tp) => PROVIDER_PRESETS[tp.value]
  );

  const handleQuickCreate = () => {
    const type = detectedType;
    if (!type || !quickKey.trim()) return;
    // Apply preset and fill API key
    setProviderType(type);
    applyPreset(type);
    setApiKeys([quickKey.trim()]);
    setVisibleApiKeys([false]);
    // Submit directly
    const preset = resolveProviderPresetForKey(type, quickKey.trim());
    if (!preset) return;
    const typeLabel =
      PROVIDER_TYPES.find((t) => t.value === type)?.label ?? type;
    onSubmit({
      name: typeLabel,
      provider_type: type,
      base_url: preset.baseUrl,
      api_key: quickKey.trim(),
      default_model: preset.defaultModel,
      reasoning_model: preset.reasoningModel ?? null,
      protocol: JSON.stringify(preset.protocols),
      timeout_seconds: 120,
      enabled: true,
      anthropic_base_url: preset.anthropicBaseUrl ?? null,
      responses_base_url: preset.responsesBaseUrl ?? null,
      extra_headers: preset.extraHeaders ?? null,
      auto_cache_control: true,
    } as CreateProviderInput);
  };

  useEffect(() => {
    if (provider) {
      setName(provider.name);
      setProviderType(provider.provider_type);
      setBaseUrl(provider.base_url);
      // 编辑模式：先用 [""] 占位避免一帧空，再异步拉真实 keys 回填。
      // 这样多 key provider 也能看到全部槽，知道改的是哪一个；保存逻辑
      // 不变（apiKeys 数组 → 单串 or JSON 数组）。
      setApiKeys([""]);
      setVisibleApiKeys([false]);
      api
        .getProviderKeys(provider.id)
        .then((keys) => {
          const nextKeys = keys.length > 0 ? keys : [""];
          setApiKeys(nextKeys);
          setVisibleApiKeys(nextKeys.map(() => false));
        })
        .catch(() => {
          setApiKeys([""]);
          setVisibleApiKeys([false]);
        });
      setDefaultModel(provider.default_model);
      setReasoningModel(provider.reasoning_model ?? "");
      setSupportedModels(provider.supported_models ?? "");
      try {
        setModelCapabilities(
          provider.model_capabilities
            ? JSON.parse(provider.model_capabilities)
            : {}
        );
      } catch {
        setModelCapabilities({});
      }
      try {
        setModelContextWindows(
          provider.model_context_windows
            ? JSON.parse(provider.model_context_windows)
            : {}
        );
      } catch {
        setModelContextWindows({});
      }
      try {
        setModelMapping(
          provider.model_mapping ? JSON.parse(provider.model_mapping) : {}
        );
      } catch {
        setModelMapping({});
      }
      setExtraHeaders(provider.extra_headers ?? "");
      setAnthropicBaseUrl(provider.anthropic_base_url ?? "");
      setResponsesBaseUrl(provider.responses_base_url ?? "");
      setAutoCacheControl(provider.auto_cache_control ?? true);
      try {
        setProtocols(JSON.parse(provider.protocol));
      } catch {
        setProtocols([provider.protocol]);
      }
      setTimeoutSeconds(String(provider.timeout_seconds));
      setEnabled(provider.enabled);
    } else {
      const defaultType = "deepseek";
      const preset = PROVIDER_PRESETS[defaultType];
      setName(PROVIDER_TYPES.find((t) => t.value === defaultType)?.label ?? "");
      setProviderType(defaultType);
      setBaseUrl(preset?.baseUrl ?? "");
      setApiKeys([""]);
      setVisibleApiKeys([false]);
      setDefaultModel(preset?.defaultModel ?? "");
      setReasoningModel(preset?.reasoningModel ?? "");
      setSupportedModels("");
      setModelCapabilities({});
      setModelContextWindows({});
      setModelMapping({});
      setExtraHeaders(preset?.extraHeaders ?? "");
      setAnthropicBaseUrl(preset?.anthropicBaseUrl ?? "");
      setResponsesBaseUrl(preset?.responsesBaseUrl ?? "");
      setAutoCacheControl(!!preset?.anthropicBaseUrl);
      setProtocols(preset?.protocols ?? ["openai_chat_completions"]);
      setTimeoutSeconds("120");
      setEnabled(true);
    }
    setErrors({});
    setQuickMode(!provider);
    setQuickKey("");
    setDetectedType(null);
  }, [provider, open]);

  const validate = (): boolean => {
    const errs: Record<string, string> = {};
    if (!name.trim()) errs.name = t("providers.name") + " required";
    if (!baseUrl.trim()) errs.baseUrl = t("providers.base_url") + " required";
    if (!defaultModel.trim())
      errs.defaultModel = t("providers.default_model") + " required";
    const tv = parseInt(timeoutSeconds, 10);
    if (isNaN(tv) || tv <= 0) errs.timeoutSeconds = "> 0";
    setErrors(errs);
    return Object.keys(errs).length === 0;
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!validate()) return;

    const mmStr =
      Object.keys(modelMapping).length > 0
        ? JSON.stringify(modelMapping)
        : undefined;
    const smStr = supportedModels || undefined;
    const ehStr = extraHeaders || undefined;
    const abuStr = anthropicBaseUrl || undefined;
    const rbuStr = responsesBaseUrl || undefined;
    const mcStr =
      Object.keys(modelCapabilities).length > 0
        ? JSON.stringify(modelCapabilities)
        : undefined;
    const mcwStr =
      Object.keys(modelContextWindows).length > 0
        ? JSON.stringify(modelContextWindows)
        : undefined;
    const validKeys = apiKeys.filter((k) => k.trim());
    const keyEndpoints = resolveKnownProviderEndpoints(
      providerType,
      validKeys[0]
    );
    const submitBaseUrl =
      keyEndpoints && isKnownMimoEndpointUrl(baseUrl)
        ? keyEndpoints.baseUrl
        : baseUrl;
    const submitAnthropicBaseUrl =
      keyEndpoints &&
      (!anthropicBaseUrl || isKnownMimoEndpointUrl(anthropicBaseUrl))
        ? keyEndpoints.anthropicBaseUrl
        : abuStr;

    if (isEdit) {
      const input: UpdateProviderInput = {
        name,
        provider_type: providerType,
        base_url: submitBaseUrl,
        default_model: defaultModel,
        reasoning_model: reasoningModel || undefined,
        supported_models: smStr,
        model_mapping: mmStr,
        extra_headers: ehStr,
        anthropic_base_url: submitAnthropicBaseUrl,
        responses_base_url: rbuStr,
        auto_cache_control: autoCacheControl,
        model_capabilities: mcStr,
        model_context_windows: mcwStr,
        protocol: JSON.stringify(protocols),
        timeout_seconds: parseInt(timeoutSeconds, 10),
        enabled,
      };
      if (validKeys.length === 1) input.api_key = validKeys[0];
      else if (validKeys.length > 1) input.api_key = JSON.stringify(validKeys);
      onSubmit(input);
    } else {
      const input: CreateProviderInput = {
        name,
        provider_type: providerType,
        base_url: submitBaseUrl,
        default_model: defaultModel,
        reasoning_model: reasoningModel || undefined,
        supported_models: smStr,
        model_mapping: mmStr,
        extra_headers: ehStr,
        anthropic_base_url: submitAnthropicBaseUrl,
        responses_base_url: rbuStr,
        auto_cache_control: autoCacheControl,
        model_capabilities: mcStr,
        model_context_windows: mcwStr,
        protocol: JSON.stringify(protocols),
        timeout_seconds: parseInt(timeoutSeconds, 10),
        enabled,
      };
      if (validKeys.length === 1) input.api_key = validKeys[0];
      else if (validKeys.length > 1) input.api_key = JSON.stringify(validKeys);
      onSubmit(input);
    }
  };

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center">
      <div className="fixed inset-0 bg-black/50" onClick={onClose} />
      <div className="relative z-10 w-full max-w-lg max-h-[90vh] overflow-y-auto rounded-lg border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-6 py-4">
          <h2 className="text-sm font-semibold text-text-primary">
            {isEdit ? t("providers.edit") : t("providers.add")}
          </h2>
          <button
            onClick={onClose}
            className="rounded-md p-1.5 text-text-muted transition-colors hover:bg-card-secondary hover:text-text-primary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Quick mode — paste API key to auto-create */}
        {quickMode && !isEdit && (
          <div className="p-6 space-y-4">
            <div>
              <p className="text-xs text-text-muted mb-3">
                {t("providers.quick_add_hint")}
              </p>
              <input
                value={quickKey}
                onChange={(e) => handleQuickKeyChange(e.target.value)}
                placeholder="sk-xxx / tp-xxx / deepseek-xxx / sk-ant-xxx ..."
                className="form-input text-sm"
                autoFocus
              />
            </div>
            {quickKey.trim() && (
              <div className="rounded-xl border border-accent/30 bg-accent-soft p-4">
                <div className="mb-3 space-y-1.5">
                  <label className="text-xs font-medium text-text-secondary">
                    {t("onboarding.provider_label")}
                  </label>
                  <select
                    value={detectedType ?? ""}
                    onChange={(e) => setDetectedType(e.target.value || null)}
                    className="form-input text-sm"
                  >
                    <option value="">
                      {t("onboarding.provider_select_placeholder")}
                    </option>
                    {quickProviderTypes.map((tp) => (
                      <option key={tp.value} value={tp.value}>
                        {tp.label}
                      </option>
                    ))}
                  </select>
                  {detectedType ? (
                    <p className="text-xs text-text-muted">
                      {PROVIDER_PRESETS[detectedType]?.defaultModel}
                    </p>
                  ) : (
                    <p className="text-xs text-text-muted">
                      {t("providers.quick_add_unknown")}
                    </p>
                  )}
                </div>
                <button
                  type="button"
                  onClick={handleQuickCreate}
                  disabled={!detectedType}
                  className="btn-primary disabled:opacity-40"
                >
                  {t("providers.create")}
                </button>
              </div>
            )}
            <button
              type="button"
              onClick={() => setQuickMode(false)}
              className="text-xs text-accent hover:underline"
            >
              {t("providers.manual_setup")}
            </button>
          </div>
        )}

        <form
          onSubmit={handleSubmit}
          className={`space-y-4 p-6 ${quickMode && !isEdit ? "hidden" : ""}`}
        >
          {/* ────── Section A · 基础 ──────
              Type 选完自动套全部 preset（URL/协议/cache 都不让用户操心）。
              Base URL 只在 custom 类型时显式露出，其他类型藏到高级里。 */}
          <div className="grid grid-cols-2 gap-4">
            <Field label={t("providers.type")}>
              <select
                value={providerType}
                onChange={(e) => {
                  setProviderType(e.target.value);
                  applyPreset(e.target.value);
                }}
                className="form-input"
              >
                {PROVIDER_TYPES.map((tp) => (
                  <option key={tp.value} value={tp.value}>
                    {tp.label}
                  </option>
                ))}
              </select>
            </Field>
            <Field label={t("providers.name")} error={errors.name}>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="My Provider"
                className="form-input"
              />
            </Field>
          </div>

          {providerType === "custom_openai_compatible" && (
            <Field
              label={t("providers.base_url")}
              error={errors.baseUrl}
              hint={t("providers.base_url_custom_hint")}
            >
              <input
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                placeholder="https://api.example.com"
                className="form-input"
              />
            </Field>
          )}

          <div>
            <label className="mb-1 block text-xs font-medium text-text-secondary">
              {t("providers.api_key")}{" "}
              {apiKeys.filter((k) => k).length > 1 && (
                <span className="text-text-muted">
                  ({apiKeys.filter((k) => k).length} keys)
                </span>
              )}
            </label>
            <div className="space-y-1.5">
              {apiKeys.map((key, i) => (
                <div key={i} className="flex items-center gap-1.5">
                  <div className="relative flex-1">
                    <input
                      type={visibleApiKeys[i] ? "text" : "password"}
                      value={key}
                      onChange={(e) => {
                        const next = [...apiKeys];
                        next[i] = e.target.value;
                        setApiKeys(next);
                      }}
                      placeholder={i === 0 ? "sk-... / tp-..." : `Key ${i + 1}`}
                      className="form-input pr-10"
                    />
                    <button
                      type="button"
                      onClick={() => {
                        const next = [...visibleApiKeys];
                        next[i] = !next[i];
                        setVisibleApiKeys(next);
                      }}
                      aria-label={
                        visibleApiKeys[i]
                          ? t("providers.hide_key")
                          : t("providers.show_key")
                      }
                      title={
                        visibleApiKeys[i]
                          ? t("providers.hide_key")
                          : t("providers.show_key")
                      }
                      className="absolute right-2 top-1/2 grid h-7 w-7 -translate-y-1/2 place-items-center rounded text-text-muted transition-colors hover:bg-card-secondary hover:text-text-primary"
                    >
                      {visibleApiKeys[i] ? (
                        <EyeOff className="h-4 w-4" />
                      ) : (
                        <Eye className="h-4 w-4" />
                      )}
                    </button>
                  </div>
                  {apiKeys.length > 1 && (
                    <button
                      type="button"
                      onClick={() => {
                        setApiKeys(apiKeys.filter((_, j) => j !== i));
                        setVisibleApiKeys(
                          visibleApiKeys.filter((_, j) => j !== i)
                        );
                      }}
                      className="text-text-muted hover:text-error text-xs"
                    >
                      &times;
                    </button>
                  )}
                </div>
              ))}
            </div>
            <button
              type="button"
              onClick={() => {
                setApiKeys([...apiKeys, ""]);
                setVisibleApiKeys([...visibleApiKeys, false]);
              }}
              className="mt-1.5 text-[11px] text-accent hover:text-accent/80"
            >
              + {t("providers.add_key")}
            </button>
          </div>

          {/* ────── Section B · 模型与能力 ──────
              一个按钮把"拉取上游模型列表"和"按模型名识别能力"两步合一，
              新手不用懂"模型矩阵"这个概念。能力矩阵默认折叠，需要时再展开微调。 */}
          <div>
            <div className="mb-1 flex items-center justify-between">
              <label className="text-xs font-medium text-text-secondary">
                {t("providers.models_and_caps")}
              </label>
              {isEdit && provider && (
                <button
                  type="button"
                  disabled={fetchingModels || seedingCaps}
                  onClick={async () => {
                    setFetchingModels(true);
                    try {
                      // 先把当前 form 里的 API key / base_url 保存到后端，否则 fetchProviderModels 用的是旧值
                      const saveInput: UpdateProviderInput = {};
                      const vk = apiKeys.filter((k) => k.trim());
                      if (vk.length === 1) saveInput.api_key = vk[0];
                      else if (vk.length > 1)
                        saveInput.api_key = JSON.stringify(vk);
                      const keyEndpoints = resolveKnownProviderEndpoints(
                        providerType,
                        vk[0]
                      );
                      const nextBaseUrl =
                        keyEndpoints && isKnownMimoEndpointUrl(baseUrl)
                          ? keyEndpoints.baseUrl
                          : baseUrl;
                      const nextAnthropicBaseUrl =
                        keyEndpoints &&
                        (!anthropicBaseUrl ||
                          isKnownMimoEndpointUrl(anthropicBaseUrl))
                          ? keyEndpoints.anthropicBaseUrl
                          : anthropicBaseUrl;
                      if (nextBaseUrl && nextBaseUrl !== provider.base_url)
                        saveInput.base_url = nextBaseUrl;
                      if (
                        nextAnthropicBaseUrl &&
                        nextAnthropicBaseUrl !== provider.anthropic_base_url
                      ) {
                        saveInput.anthropic_base_url = nextAnthropicBaseUrl;
                      }
                      if (Object.keys(saveInput).length > 0) {
                        await api.updateProvider(provider.id, saveInput);
                      }
                      const fetchedModels = await api.fetchProviderModels(
                        provider.id
                      );
                      const models = normalizeModelsForProvider(
                        providerType,
                        fetchedModels
                      );
                      setSupportedModels(JSON.stringify(models));
                      // 拉完接着自动按模型名识别能力——一步到位
                      setSeedingCaps(true);
                      try {
                        const seeded = await api.seedModelCapabilities(
                          providerType,
                          models
                        );
                        setModelCapabilities(
                          seeded as Record<string, string[]>
                        );
                        toast(
                          "success",
                          `${models.length} ${t("providers.toast_models_and_caps")}`
                        );
                      } catch {
                        toast("success", `${models.length} models`);
                      } finally {
                        setSeedingCaps(false);
                      }
                      // 当前 default/reasoning 不在新拉到的列表里时，按 heuristic 自动选最新——避免
                      // preset 写死的 default（如 mimo-v2.5-pro）在上游上线新版本后还指向老版本
                      const picked = pickModelsForProvider(
                        providerType,
                        models
                      );
                      if (!defaultModel || !models.includes(defaultModel))
                        setDefaultModel(picked.default);
                      if (!reasoningModel || !models.includes(reasoningModel))
                        setReasoningModel(picked.reasoning);
                    } catch (err) {
                      toast("error", (err as api.AppError).message);
                    } finally {
                      setFetchingModels(false);
                    }
                  }}
                  className="flex items-center gap-1 text-[11px] text-accent hover:text-accent/80 disabled:opacity-50"
                >
                  {fetchingModels || seedingCaps ? (
                    <Loader2 className="h-3 w-3 animate-spin" />
                  ) : (
                    <Sparkles className="h-3 w-3" />
                  )}
                  {t("providers.fetch_and_detect")}
                </button>
              )}
            </div>
            {(() => {
              let models: string[] = [];
              try {
                models = supportedModels ? JSON.parse(supportedModels) : [];
              } catch {
                /* */
              }
              return models.length > 0 ? (
                <div className="flex flex-wrap gap-1.5 rounded-md border border-border bg-card-secondary p-2">
                  {models.map((m) => (
                    <span
                      key={m}
                      className="flex items-center gap-1 rounded bg-bg px-2 py-0.5 font-mono text-[11px] text-text-secondary"
                    >
                      {m}
                      <button
                        type="button"
                        onClick={() => {
                          const next = models.filter((x) => x !== m);
                          setSupportedModels(
                            next.length > 0 ? JSON.stringify(next) : ""
                          );
                        }}
                        className="text-text-muted hover:text-error"
                      >
                        &times;
                      </button>
                    </span>
                  ))}
                </div>
              ) : (
                <p className="text-[11px] text-text-muted">
                  {isEdit
                    ? t("providers.fetch_models_hint")
                    : t("providers.supported_models_hint")}
                </p>
              );
            })()}
          </div>

          {/* 能力矩阵——默认折叠，只在用户主动展开时才显示完整 checkbox 表 */}
          {(() => {
            let models: string[] = [];
            try {
              models = supportedModels ? JSON.parse(supportedModels) : [];
            } catch {
              /* */
            }
            const merged = new Set(models);
            if (defaultModel) merged.add(defaultModel);
            if (reasoningModel) merged.add(reasoningModel);
            models = Array.from(merged);
            if (models.length < 1) return null;
            return (
              <div>
                <button
                  type="button"
                  onClick={() => setShowCapMatrix(!showCapMatrix)}
                  className="flex items-center gap-1 text-[11px] text-accent hover:text-accent/80"
                >
                  <span
                    className={`transition-transform ${showCapMatrix ? "rotate-90" : ""}`}
                  >
                    &#9654;
                  </span>
                  {t("providers.cap_matrix_toggle")}
                  <span className="text-text-muted">({models.length})</span>
                </button>
                {showCapMatrix && (
                  <div className="mt-2">
                    <p className="mb-2 text-[11px] text-text-muted">
                      {t("providers.cap_matrix_hint")}
                    </p>
                    <div className="overflow-x-auto rounded-md border border-border bg-card-secondary">
                      <table className="w-full text-[11px]">
                        <thead>
                          <tr className="border-b border-border bg-bg/50">
                            <th className="sticky left-0 z-10 bg-bg/50 px-2 py-1.5 text-left font-mono text-text-secondary">
                              model
                            </th>
                            {ALL_CAPABILITIES.map((cap) => (
                              <th
                                key={cap}
                                className="px-2 py-1.5 text-center font-medium text-text-secondary"
                                title={cap}
                              >
                                {CAPABILITY_LABELS[cap]}
                              </th>
                            ))}
                            <th
                              className="px-2 py-1.5 text-center font-medium text-text-secondary"
                              title={t("providers.ctx_window_hint")}
                            >
                              {t("providers.ctx_window_col")}
                            </th>
                          </tr>
                        </thead>
                        <tbody>
                          {models.map((m) => {
                            const caps = modelCapabilities[m] ?? [];
                            return (
                              <tr
                                key={m}
                                className="border-b border-border last:border-0"
                              >
                                <td className="sticky left-0 z-10 bg-card-secondary px-2 py-1.5 font-mono text-text-primary">
                                  {m}
                                </td>
                                {ALL_CAPABILITIES.map((cap) => (
                                  <td
                                    key={cap}
                                    className="px-2 py-1.5 text-center"
                                  >
                                    <input
                                      type="checkbox"
                                      checked={caps.includes(cap)}
                                      onChange={(e) => {
                                        setModelCapabilities((prev) => {
                                          const next = { ...prev };
                                          const cur = new Set(next[m] ?? []);
                                          if (e.target.checked) cur.add(cap);
                                          else cur.delete(cap);
                                          next[m] = Array.from(cur).sort();
                                          return next;
                                        });
                                      }}
                                      className="h-3.5 w-3.5 cursor-pointer accent-accent"
                                    />
                                  </td>
                                ))}
                                <td className="px-2 py-1.5 text-center">
                                  <input
                                    type="number"
                                    min="1"
                                    value={modelContextWindows[m] ?? ""}
                                    placeholder={
                                      catalogContextWindow(
                                        providerType,
                                        m
                                      )?.toString() ?? "auto"
                                    }
                                    onChange={(e) => {
                                      const raw = e.target.value.trim();
                                      setModelContextWindows((prev) => {
                                        const next = { ...prev };
                                        const n = parseInt(raw, 10);
                                        if (raw === "" || isNaN(n) || n <= 0)
                                          delete next[m];
                                        else next[m] = n;
                                        return next;
                                      });
                                    }}
                                    className="w-20 rounded border border-border bg-bg px-1 py-0.5 text-center text-[11px]"
                                  />
                                </td>
                              </tr>
                            );
                          })}
                        </tbody>
                      </table>
                    </div>
                  </div>
                )}
              </div>
            );
          })()}

          {/* Default / Reasoning Model — dropdown if models available */}
          <div className="grid grid-cols-2 gap-4">
            <Field
              label={t("providers.default_model")}
              error={errors.defaultModel}
            >
              {(() => {
                let models: string[] = [];
                try {
                  models = supportedModels ? JSON.parse(supportedModels) : [];
                } catch {
                  /* */
                }
                return models.length > 0 ? (
                  <select
                    value={defaultModel}
                    onChange={(e) => setDefaultModel(e.target.value)}
                    className="form-input"
                  >
                    <option value="">--</option>
                    {models.map((m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    value={defaultModel}
                    onChange={(e) => setDefaultModel(e.target.value)}
                    placeholder="model-name"
                    className="form-input"
                  />
                );
              })()}
            </Field>
            <Field label={t("providers.reasoning_model")}>
              {(() => {
                let models: string[] = [];
                try {
                  models = supportedModels ? JSON.parse(supportedModels) : [];
                } catch {
                  /* */
                }
                return models.length > 0 ? (
                  <select
                    value={reasoningModel}
                    onChange={(e) => setReasoningModel(e.target.value)}
                    className="form-input"
                  >
                    <option value="">--</option>
                    {models.map((m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    value={reasoningModel}
                    onChange={(e) => setReasoningModel(e.target.value)}
                    placeholder="Optional"
                    className="form-input"
                  />
                );
              })()}
            </Field>
          </div>

          {/* ────── Section C · 高级（默认折叠） ──────
              99% 用户不需要打开。打开后:
              1) 协议 + 各协议对应 URL 合并成一个 list，一眼看清"这家上游同时支持哪些原生入口"
              2) base_url 在这里露（custom 类型在 Section A 已经露过则此处隐藏）
              3) timeout / enabled / auto_cache 都有合理默认，仅 isEdit 显示给"懂的用户"
              4) Model mapping 在最底部 + 文案"通常无需配置" */}
          <div>
            <button
              type="button"
              onClick={() => setShowAdvanced(!showAdvanced)}
              className="flex items-center gap-1 text-[11px] text-accent hover:text-accent/80"
            >
              <span
                className={`transition-transform ${showAdvanced ? "rotate-90" : ""}`}
              >
                &#9654;
              </span>
              {t("providers.advanced_settings")}
              <span className="text-text-muted">
                · {t("providers.advanced_hint")}
              </span>
            </button>
            {showAdvanced && (
              <div className="mt-3 space-y-4 rounded-md border border-border/50 bg-card-secondary p-4">
                {/* 协议 + URL 合并视图：勾选+对应入口一行一行展示，传达"哪些协议网关能原生送给上游" */}
                <Field
                  label={t("providers.endpoints")}
                  hint={t("providers.endpoints_hint")}
                >
                  <div className="space-y-2">
                    {PROTOCOLS.map((p) => {
                      const checked = protocols.includes(p.value);
                      // 每个协议对应的 URL 字段：chat → base_url, anthropic → anthropic_base_url, responses → responses_base_url
                      const urlValue =
                        p.value === "anthropic_messages"
                          ? anthropicBaseUrl
                          : p.value === "openai_responses"
                            ? responsesBaseUrl
                            : baseUrl;
                      const setUrl =
                        p.value === "anthropic_messages"
                          ? setAnthropicBaseUrl
                          : p.value === "openai_responses"
                            ? setResponsesBaseUrl
                            : setBaseUrl;
                      const placeholder =
                        p.value === "anthropic_messages"
                          ? "https://api.deepseek.com/anthropic"
                          : p.value === "openai_responses"
                            ? "https://api.openai.com"
                            : "https://api.example.com";
                      return (
                        <div
                          key={p.value}
                          className="rounded-md border border-border bg-bg/40 p-2.5"
                        >
                          <label className="flex cursor-pointer items-center gap-2">
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={(e) => {
                                if (e.target.checked)
                                  setProtocols([...protocols, p.value]);
                                else {
                                  const next = protocols.filter(
                                    (x) => x !== p.value
                                  );
                                  if (next.length > 0) setProtocols(next);
                                }
                              }}
                              className="accent-accent"
                            />
                            <span className="text-xs font-medium text-text-secondary">
                              {p.label}
                            </span>
                          </label>
                          {checked && (
                            <input
                              value={urlValue}
                              onChange={(e) => setUrl(e.target.value)}
                              placeholder={placeholder}
                              className="form-input mt-1.5 text-[11px]"
                            />
                          )}
                        </div>
                      );
                    })}
                  </div>
                </Field>

                <Field
                  label={t("providers.extra_headers")}
                  hint={t("providers.extra_headers_hint")}
                >
                  <input
                    value={extraHeaders}
                    onChange={(e) => setExtraHeaders(e.target.value)}
                    placeholder='{"User-Agent":"KimiCLI/1.40.0"}'
                    className="form-input"
                  />
                </Field>

                {/* auto_cache_control：只在 anthropic-capable 时露，且默认按 type 设好——一般人不动 */}
                {(providerType === "anthropic" ||
                  providerType === "claude" ||
                  anthropicBaseUrl) && (
                  <Field
                    label={t("providers.auto_cache")}
                    hint={
                      providerType === "anthropic" || providerType === "claude"
                        ? t("providers.auto_cache_hint_native")
                        : t("providers.auto_cache_hint_compat")
                    }
                  >
                    <label className="mt-1 flex cursor-pointer items-center gap-2">
                      <input
                        type="checkbox"
                        checked={autoCacheControl}
                        onChange={(e) => setAutoCacheControl(e.target.checked)}
                        className="accent-accent"
                      />
                      <span className="text-xs text-text-secondary">
                        {autoCacheControl
                          ? t("providers.enabled")
                          : t("providers.disabled")}
                      </span>
                    </label>
                  </Field>
                )}

                <div className="grid grid-cols-2 gap-4">
                  <Field
                    label={t("providers.timeout")}
                    error={errors.timeoutSeconds}
                  >
                    <input
                      type="number"
                      value={timeoutSeconds}
                      onChange={(e) => setTimeoutSeconds(e.target.value)}
                      min={1}
                      className="form-input"
                    />
                  </Field>
                  {isEdit && (
                    <Field label={t("providers.enabled")}>
                      <label className="mt-1.5 flex cursor-pointer items-center gap-2">
                        <input
                          type="checkbox"
                          checked={enabled}
                          onChange={(e) => setEnabled(e.target.checked)}
                          className="accent-accent"
                        />
                        <span className="text-xs text-text-secondary">
                          {enabled
                            ? t("providers.enabled")
                            : t("providers.disabled")}
                        </span>
                      </label>
                    </Field>
                  )}
                </div>

                {/* Model Mapping——最底部，加"通常无需配置"文案 */}
                <div className="border-t border-border/50 pt-4">
                  <label className="mb-1 block text-xs font-medium text-text-secondary">
                    {t("providers.model_mapping")}
                  </label>
                  <p className="mb-2 text-[11px] text-text-muted">
                    {t("providers.model_mapping_hint_v2")}
                  </p>
                  <div className="space-y-1.5">
                    {Object.entries(modelMapping).map(
                      ([clientModel, providerModel]) => (
                        <div
                          key={clientModel}
                          className="flex items-center gap-2"
                        >
                          <input
                            value={clientModel}
                            onChange={(e) => {
                              const newKey = e.target.value;
                              if (newKey && newKey !== clientModel) {
                                const next: Record<string, string> = {};
                                for (const [k, v] of Object.entries(
                                  modelMapping
                                )) {
                                  next[k === clientModel ? newKey : k] = v;
                                }
                                setModelMapping(next);
                              }
                            }}
                            className="form-input flex-1"
                          />
                          <span className="text-text-muted">→</span>
                          <ModelCombo
                            value={providerModel}
                            onChange={(v) =>
                              setModelMapping({
                                ...modelMapping,
                                [clientModel]: v,
                              })
                            }
                            models={(() => {
                              try {
                                return supportedModels
                                  ? JSON.parse(supportedModels)
                                  : [];
                              } catch {
                                return [];
                              }
                            })()}
                          />
                          <button
                            type="button"
                            onClick={() => {
                              const next = { ...modelMapping };
                              delete next[clientModel];
                              setModelMapping(next);
                            }}
                            className="text-text-muted hover:text-error text-xs"
                          >
                            ✕
                          </button>
                        </div>
                      )
                    )}
                  </div>
                  <div className="mt-2 flex gap-2">
                    <ModelCombo
                      value={newMappingClient}
                      onChange={(v) => setNewMappingClient(v)}
                      models={[
                        "gpt-5.5",
                        "gpt-5.4",
                        "gpt-5.4-mini",
                        "gpt-5.3-codex",
                        "gpt-5.2",
                        "claude-sonnet-4-7",
                        "claude-sonnet-4-6",
                        "claude-opus-4-8",
                        "claude-opus-4-7",
                        "claude-opus-4-6",
                        "claude-haiku-4-5-20251001",
                        "o3",
                        "o4-mini",
                        "gemini-2.5-flash",
                        "gemini-2.5-pro",
                        "gemini-3-pro-preview",
                      ].filter((m) => !(m in modelMapping))}
                      placeholder={t("providers.select_client_model")}
                    />
                    <button
                      type="button"
                      onClick={() => {
                        if (
                          newMappingClient.trim() &&
                          !(newMappingClient.trim() in modelMapping)
                        ) {
                          let models: string[] = [];
                          try {
                            models = supportedModels
                              ? JSON.parse(supportedModels)
                              : [];
                          } catch {
                            /* */
                          }
                          setModelMapping({
                            ...modelMapping,
                            [newMappingClient.trim()]:
                              models[0] || defaultModel || "",
                          });
                          setNewMappingClient("");
                        }
                      }}
                      className="btn-secondary"
                    >
                      {t("routes.add")}
                    </button>
                  </div>
                </div>
              </div>
            )}
          </div>

          <div className="flex justify-end gap-2 pt-2">
            <button
              type="button"
              onClick={onClose}
              className="rounded-md bg-card-secondary px-4 py-2 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary"
            >
              {t("common.cancel")}
            </button>
            <button
              type="submit"
              className="rounded-md bg-accent px-4 py-2 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90"
            >
              {isEdit ? t("providers.save") : t("providers.create")}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

function Field({
  label,
  error,
  hint,
  children,
}: {
  label: string;
  error?: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <label className="mb-1 block text-xs font-medium text-text-secondary">
        {label}
      </label>
      {children}
      {hint && <p className="mt-1 text-[11px] text-text-muted">{hint}</p>}
      {error && <p className="mt-1 text-[11px] text-error">{error}</p>}
    </div>
  );
}

function ModelCombo({
  value,
  onChange,
  models,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  models: string[];
  placeholder?: string;
}) {
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const filtered = filter
    ? models.filter((m) => m.toLowerCase().includes(filter.toLowerCase()))
    : models;

  return (
    <div className="relative flex-1">
      <input
        value={value}
        onChange={(e) => {
          onChange(e.target.value);
          setFilter(e.target.value);
          setOpen(true);
        }}
        onFocus={() => {
          if (models.length > 0) setOpen(true);
        }}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        placeholder={placeholder || "model-name"}
        className="form-input w-full"
      />
      {open && filtered.length > 0 && (
        <ul className="absolute z-50 mt-1 max-h-40 w-full overflow-y-auto rounded-md border border-accent/30 bg-bg shadow-xl ring-1 ring-white/5">
          {filtered.map((m) => (
            <li
              key={m}
              onMouseDown={(e) => {
                e.preventDefault();
                onChange(m);
                setFilter("");
                setOpen(false);
              }}
              className={`cursor-pointer px-3 py-1.5 text-xs transition-colors hover:bg-accent/20 hover:text-accent ${m === value ? "bg-accent/15 text-accent font-medium" : "text-text-secondary"}`}
            >
              {m}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
