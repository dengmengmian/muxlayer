import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  Key,
  Monitor,
  Rocket,
  CheckCircle,
  XCircle,
  Loader2,
  ArrowRight,
  Clipboard,
  X,
  Eye,
  EyeOff,
  Terminal,
} from "lucide-react";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { useI18n } from "@/lib/i18n";
import * as api from "@/lib/api";
import { detectProvider } from "@/lib/keyDetection";
import { fetchDetectAndPersistProviderModels } from "@/lib/providerAutoSetup";
import {
  PROVIDER_PRESETS,
  resolveProviderPresetForKey,
} from "@/data/providerPresets";
import { PROVIDER_TYPES } from "@/types/provider";
import {
  firstRequestCommandsFor,
  type FirstRequestCommand,
} from "@/lib/firstRequestCommands";
import { CopyButton } from "@/components/common/CopyButton";

type Step = "key" | "tools" | "setup" | "done";

interface ToolDetection {
  id: string;
  name: string;
  detected: boolean;
  checked: boolean;
}

interface SetupLogEntry {
  label: string;
  status: "pending" | "running" | "ok" | "error";
  detail?: string;
}

type SetupCriteria = {
  provider: boolean;
  gateway: boolean;
  client: boolean;
  probeOk: boolean;
  hadClientTarget: boolean;
};

export function QuickSetup() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [step, setStep] = useState<Step>("key");
  const [apiKey, setApiKey] = useState("");
  const [detectedProvider, setDetectedProvider] = useState<{
    type: string;
    name: string;
  } | null>(null);
  const [tools, setTools] = useState<ToolDetection[]>([]);
  const [setupLog, setSetupLog] = useState<SetupLogEntry[]>([]);
  const [appliedClientIds, setAppliedClientIds] = useState<string[]>([]);
  const [criteria, setCriteria] = useState<SetupCriteria>({
    provider: false,
    gateway: false,
    client: false,
    probeOk: true,
    hadClientTarget: false,
  });
  const [showKey, setShowKey] = useState(false);
  /// 剪贴板里有可识别的 key 时显示「填入」banner。null = 没建议
  /// （包括读不到剪贴板、识别失败、被用户关掉这三种情况）。
  const [clipboardHint, setClipboardHint] = useState<{
    key: string;
    type: string;
    name: string;
  } | null>(null);
  /// 本次 mount 只检测一次，避免用户切回 step="key" 又弹一遍。
  const clipboardChecked = useRef(false);

  useEffect(() => {
    if (clipboardChecked.current) return;
    clipboardChecked.current = true;
    // 静默尝试读剪贴板：插件没装 / 权限被拒 / 系统不支持 / 内容超长
    // 全部吞掉，不影响正常表单流程。这是用户明确要求的设计约束。
    (async () => {
      try {
        const text = (await readText()) ?? "";
        const candidate = text.trim();
        // 太短不像 key，太长八成是粘了一段代码 / 文本
        if (candidate.length < 10 || candidate.length > 200) return;
        const detected = detectProvider(candidate);
        if (detected) {
          setClipboardHint({
            key: candidate,
            type: detected.type,
            name: detected.label,
          });
        }
      } catch {
        // 剪贴板不可用 → banner 不出现，跟没这个功能一样
      }
    })();
  }, []);

  const acceptClipboardKey = () => {
    if (!clipboardHint) return;
    handleKeyChange(clipboardHint.key);
    setClipboardHint(null);
  };

  const quickProviderTypes = PROVIDER_TYPES.filter(
    (tp) => PROVIDER_PRESETS[tp.value]
  );

  const selectProvider = (type: string) => {
    const label = PROVIDER_TYPES.find((tp) => tp.value === type)?.label ?? type;
    setDetectedProvider(type ? { type, name: label } : null);
  };

  const handleKeyChange = (key: string) => {
    setApiKey(key);
    const result = detectProvider(key);
    setDetectedProvider(
      result ? { type: result.type, name: result.label } : null
    );
  };

  const handleGoToTools = async () => {
    setStep("tools");
    const results: ToolDetection[] = [];
    try {
      const c = await api.detectCodexConfig();
      results.push({
        id: "codex",
        name: "Codex",
        detected: c.exists,
        checked: true,
      });
    } catch {
      results.push({
        id: "codex",
        name: "Codex",
        detected: false,
        checked: false,
      });
    }
    try {
      const c = await api.detectClaudeCodeEnv();
      results.push({
        id: "claude_code",
        name: "Claude Code",
        detected: c.settings_exists,
        checked: true,
      });
    } catch {
      results.push({
        id: "claude_code",
        name: "Claude Code",
        detected: false,
        checked: false,
      });
    }
    try {
      const c = await api.detectOpenCodeConfig();
      results.push({
        id: "opencode",
        name: "OpenCode",
        detected: c.exists,
        checked: c.exists,
      });
    } catch {
      results.push({
        id: "opencode",
        name: "OpenCode",
        detected: false,
        checked: false,
      });
    }
    try {
      const c = await api.detectGeminiConfig();
      results.push({
        id: "gemini",
        name: "Gemini CLI",
        detected: c.exists,
        checked: c.exists,
      });
    } catch {
      results.push({
        id: "gemini",
        name: "Gemini CLI",
        detected: false,
        checked: false,
      });
    }
    try {
      const c = await api.detectAtomCodeConfig();
      results.push({
        id: "atomcode",
        name: "AtomCode",
        detected: c.exists,
        checked: c.exists,
      });
    } catch {
      results.push({
        id: "atomcode",
        name: "AtomCode",
        detected: false,
        checked: false,
      });
    }
    setTools(results);
  };

  const handleSetup = async () => {
    setStep("setup");
    const log: SetupLogEntry[] = [];
    const addLog = (
      label: string,
      status: "pending" | "running" | "ok" | "error",
      detail?: string
    ) => {
      const idx = log.findIndex((l) => l.label === label);
      if (idx >= 0) {
        log[idx] = { ...log[idx], status, detail };
      } else {
        log.push({ label, status, detail });
      }
      setSetupLog([...log]);
    };

    const checkedTools = tools.filter((tool) => tool.checked);
    const hadClientTarget = checkedTools.length > 0;
    setAppliedClientIds([]);

    const preset = resolveProviderPresetForKey(
      detectedProvider!.type,
      apiKey.trim()
    );
    if (!preset) return;

    // 1. Create provider
    addLog(t("onboarding.creating_provider"), "running");
    try {
      const provider = await api.createProvider({
        name: detectedProvider!.name,
        provider_type: detectedProvider!.type,
        base_url: preset.baseUrl,
        api_key: apiKey.trim(),
        default_model: preset.defaultModel,
        reasoning_model: preset.reasoningModel ?? undefined,
        protocol: JSON.stringify(preset.protocols),
        timeout_seconds: 120,
        enabled: true,
        anthropic_base_url: preset.anthropicBaseUrl ?? undefined,
        responses_base_url: preset.responsesBaseUrl ?? undefined,
        extra_headers: preset.extraHeaders ?? undefined,
        auto_cache_control: true,
      });
      await api.setActiveProvider(provider.id);
      addLog(t("onboarding.creating_provider"), "ok");

      addLog(t("onboarding.detecting_capabilities"), "running");
      try {
        const { models } = await fetchDetectAndPersistProviderModels(
          provider.id,
          detectedProvider!.type
        );
        const detail = models.length
          ? `${models.length} ${t("providers.toast_models_and_caps")}`
          : t("providers.test.autofill_none");
        addLog(t("onboarding.detecting_capabilities"), "ok", detail);
      } catch (err) {
        // Capability detect is best-effort; provider is already created.
        addLog(
          t("onboarding.detecting_capabilities"),
          "error",
          err instanceof Error ? err.message : String(err)
        );
      }
    } catch {
      addLog(t("onboarding.creating_provider"), "error");
      setCriteria({
        provider: false,
        gateway: false,
        client: false,
        probeOk: false,
        hadClientTarget,
      });
      setStep("done");
      return;
    }
    const providerOk = true;

    // 2. Start gateway — never mark ok unless running is verified
    addLog(t("onboarding.starting_gateway"), "running");
    let gatewayOk = false;
    try {
      await api.startGateway();
      addLog(t("onboarding.starting_gateway"), "ok");
      gatewayOk = true;
    } catch {
      try {
        const st = await api.getGatewayStatus();
        if (st.running) {
          addLog(t("onboarding.starting_gateway"), "ok");
          gatewayOk = true;
        } else {
          addLog(t("onboarding.starting_gateway"), "error", "not running");
        }
      } catch {
        addLog(t("onboarding.starting_gateway"), "error");
      }
    }

    await new Promise((r) => setTimeout(r, 500));

    // 3. Apply tool configs — complete requires ≥1 successful apply when any selected
    let anyClientOk = false;
    const applied: string[] = [];
    if (!hadClientTarget) {
      addLog(
        `${t("onboarding.configuring")} client`,
        "error",
        "Select and apply at least one client"
      );
    }
    for (const tool of checkedTools) {
      addLog(`${t("onboarding.configuring")} ${tool.name}`, "running");
      try {
        switch (tool.id) {
          case "codex":
            await api.applyCodexConfig();
            break;
          case "claude_code":
            await api.applyClaudeCodeConfig();
            break;
          case "opencode":
            await api.applyOpenCodeConfig();
            break;
          case "gemini":
            await api.applyGeminiConfig();
            break;
          case "atomcode":
            await api.applyAtomCodeConfig();
            break;
        }
        addLog(`${t("onboarding.configuring")} ${tool.name}`, "ok");
        anyClientOk = true;
        applied.push(tool.id);
      } catch {
        addLog(`${t("onboarding.configuring")} ${tool.name}`, "error");
      }
    }
    setAppliedClientIds(applied);
    const clientOk = hadClientTarget && anyClientOk;

    // 4. Probe — failure must not green completion
    addLog(t("onboarding.testing"), "running");
    let probeOk: boolean;
    try {
      const test = await api.testToolConnection();
      const detail = test.provider_ok
        ? undefined
        : (test.error ?? "Gateway or provider test failed");
      addLog(
        t("onboarding.testing"),
        test.provider_ok ? "ok" : "error",
        detail
      );
      probeOk = !!test.provider_ok;
    } catch (err) {
      addLog(
        t("onboarding.testing"),
        "error",
        err instanceof Error ? err.message : String(err)
      );
      probeOk = false;
    }

    setCriteria({
      provider: providerOk,
      gateway: gatewayOk,
      client: clientOk,
      probeOk,
      hadClientTarget,
    });
    setStep("done");
  };

  const setupComplete =
    criteria.provider &&
    criteria.gateway &&
    criteria.hadClientTarget &&
    criteria.client &&
    criteria.probeOk;

  const primaryFailureCta: "providers" | "retry" | "clients" | "logs" | null =
    setupComplete
      ? null
      : !criteria.provider
        ? "providers"
        : !criteria.gateway
          ? "retry"
          : !criteria.hadClientTarget || !criteria.client
            ? "clients"
            : !criteria.probeOk
              ? "logs"
              : "retry";

  const firstCommands: FirstRequestCommand[] =
    firstRequestCommandsFor(appliedClientIds);

  const goPrimaryFailure = () => {
    switch (primaryFailureCta) {
      case "providers":
        navigate("/providers");
        break;
      case "clients":
        navigate("/tools");
        break;
      case "logs":
        navigate("/logs");
        break;
      case "retry":
        void handleSetup();
        break;
      default:
        break;
    }
  };

  return (
    <div className="desktop-page mx-auto w-full max-w-lg">
      {/* Step indicators */}
      <div className="mb-8 flex items-center justify-center gap-3">
        {[
          { key: "key", icon: Key, label: "API Key" },
          { key: "tools", icon: Monitor, label: t("onboarding.select_tools") },
          { key: "setup", icon: Rocket, label: t("onboarding.start_setup") },
        ].map((s, i) => {
          const isActive =
            s.key === step || (step === "done" && s.key === "setup");
          const isPast =
            ["key", "tools", "setup", "done"].indexOf(step) >
            ["key", "tools", "setup"].indexOf(s.key);
          return (
            <div key={s.key} className="flex items-center gap-3">
              {i > 0 && (
                <div
                  className={`h-px w-8 ${isPast ? "bg-accent" : "bg-border"}`}
                />
              )}
              <div
                className={`flex items-center gap-2 rounded-full px-3 py-1.5 text-xs font-medium ${
                  isActive
                    ? "bg-accent-soft text-accent"
                    : isPast
                      ? "text-success"
                      : "text-text-muted"
                }`}
              >
                {isPast ? (
                  <CheckCircle className="h-3.5 w-3.5" />
                ) : (
                  <s.icon className="h-3.5 w-3.5" />
                )}
                {s.label}
              </div>
            </div>
          );
        })}
      </div>

      {/* Step 1: API Key */}
      {step === "key" && (
        <div className="surface-panel space-y-5 p-6">
          <div>
            <h2 className="text-base font-semibold text-text-primary mb-1">
              {t("onboarding.welcome")}
            </h2>
            <p className="text-xs text-text-muted">
              {t("onboarding.welcome_desc")}
            </p>
          </div>

          {clipboardHint && (
            <div className="flex items-center justify-between gap-2 rounded-lg border border-accent/30 bg-accent-soft px-3 py-2">
              <div className="flex min-w-0 items-center gap-2 text-xs">
                <Clipboard className="h-3.5 w-3.5 shrink-0 text-accent" />
                <span className="truncate text-text-secondary">
                  {t("onboarding.clipboard_hint").replace(
                    "{name}",
                    clipboardHint.name
                  )}
                </span>
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <button
                  type="button"
                  onClick={acceptClipboardKey}
                  className="rounded bg-accent px-2 py-1 text-xs font-medium text-on-accent hover:bg-accent/90"
                >
                  {t("onboarding.clipboard_use")}
                </button>
                <button
                  type="button"
                  onClick={() => setClipboardHint(null)}
                  className="rounded p-1 text-text-muted hover:bg-hover hover:text-text-secondary"
                  aria-label="dismiss"
                >
                  <X className="h-3 w-3" />
                </button>
              </div>
            </div>
          )}

          <div className="relative">
            <input
              type={showKey ? "text" : "password"}
              value={apiKey}
              onChange={(e) => handleKeyChange(e.target.value)}
              placeholder="sk-xxx / tp-xxx / deepseek-xxx / sk-ant-xxx ..."
              className="form-input pr-10 text-sm"
              autoComplete="off"
              autoFocus
            />
            <button
              type="button"
              onClick={() => setShowKey((v) => !v)}
              aria-label={
                showKey ? t("providers.hide_key") : t("providers.show_key")
              }
              title={
                showKey ? t("providers.hide_key") : t("providers.show_key")
              }
              className="absolute right-2 top-1/2 grid h-7 w-7 -translate-y-1/2 place-items-center rounded text-text-muted transition-colors hover:bg-card-secondary hover:text-text-primary"
            >
              {showKey ? (
                <EyeOff className="h-4 w-4" />
              ) : (
                <Eye className="h-4 w-4" />
              )}
            </button>
          </div>

          {detectedProvider && (
            <div className="flex items-center gap-2 rounded-lg bg-success-soft px-3 py-2 text-xs text-success">
              <CheckCircle className="h-3.5 w-3.5" />
              {t("onboarding.detected")} {detectedProvider.name}
            </div>
          )}

          {apiKey.trim() && (
            <div className="space-y-1.5">
              <label className="text-xs font-medium text-text-secondary">
                {t("onboarding.provider_label")}
              </label>
              <select
                value={detectedProvider?.type ?? ""}
                onChange={(e) => selectProvider(e.target.value)}
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
              <p className="text-xs text-text-muted">
                {t("onboarding.provider_hint")}
              </p>
            </div>
          )}

          <div className="flex justify-end">
            <button
              onClick={handleGoToTools}
              disabled={!detectedProvider}
              className="btn-primary disabled:opacity-40"
            >
              {t("onboarding.next")} <ArrowRight className="h-3 w-3" />
            </button>
          </div>
        </div>
      )}

      {/* Step 2: Tools */}
      {step === "tools" && (
        <div className="surface-panel space-y-5 p-6">
          <div>
            <h2 className="text-base font-semibold text-text-primary mb-1">
              {t("onboarding.select_tools")}
            </h2>
            <p className="text-xs text-text-muted">
              {t("onboarding.select_tools_desc")}
            </p>
          </div>

          <div className="space-y-2">
            {tools.map((tool) => (
              <label
                key={tool.id}
                className={`flex items-center gap-3 rounded-lg border px-4 py-3 cursor-pointer transition-colors ${
                  tool.checked
                    ? "border-accent bg-accent-soft"
                    : "border-border hover:border-text-muted"
                }`}
              >
                <input
                  type="checkbox"
                  checked={tool.checked}
                  onChange={(e) =>
                    setTools(
                      tools.map((t) =>
                        t.id === tool.id
                          ? { ...t, checked: e.target.checked }
                          : t
                      )
                    )
                  }
                  className="sr-only"
                />
                <div
                  className={`h-4 w-4 rounded border flex items-center justify-center ${tool.checked ? "bg-accent border-accent" : "border-border"}`}
                >
                  {tool.checked && (
                    <CheckCircle className="h-3 w-3 text-on-accent" />
                  )}
                </div>
                <span className="text-sm text-text-primary">{tool.name}</span>
                {tool.detected && (
                  <span className="ml-auto text-xs text-success">
                    {t("tools.config_found")}
                  </span>
                )}
              </label>
            ))}
          </div>

          <div className="flex justify-between">
            <button
              onClick={() => setStep("key")}
              className="text-xs text-text-muted hover:text-text-primary"
            >
              ← {t("onboarding.back")}
            </button>
            <button onClick={handleSetup} className="btn-primary">
              <Rocket className="h-3 w-3" /> {t("onboarding.start_setup")}
            </button>
          </div>
        </div>
      )}

      {/* Step 3+4: Progress & Done */}
      {(step === "setup" || step === "done") && (
        <div className="surface-panel space-y-5 p-6">
          <h2 className="text-base font-semibold text-text-primary">
            {step === "done"
              ? setupComplete
                ? t("onboarding.complete")
                : t("onboarding.incomplete")
              : t("onboarding.setting_up")}
          </h2>

          <div className="space-y-3">
            {setupLog.map((entry, i) => (
              <div key={i} className="flex items-start gap-3 text-sm">
                {entry.status === "running" ? (
                  <Loader2 className="h-4 w-4 animate-spin text-accent" />
                ) : entry.status === "ok" ? (
                  <CheckCircle className="h-4 w-4 text-success" />
                ) : entry.status === "error" ? (
                  <XCircle className="h-4 w-4 text-error" />
                ) : (
                  <div className="h-4 w-4 rounded-full border-2 border-border" />
                )}
                <div className="min-w-0">
                  <div className="text-text-primary">{entry.label}</div>
                  {entry.detail && (
                    <div
                      className={`mt-1 max-w-full break-words text-xs ${
                        entry.status === "error"
                          ? "text-error"
                          : "text-text-muted"
                      }`}
                    >
                      {entry.detail}
                    </div>
                  )}
                </div>
              </div>
            ))}
          </div>

          {step === "done" && (
            <div className="space-y-3">
              {setupComplete ? (
                <>
                  <div className="rounded-lg border border-accent/20 bg-accent-soft/40 p-4 space-y-3">
                    <div className="flex items-start gap-2">
                      <Terminal className="mt-0.5 h-4 w-4 shrink-0 text-accent" />
                      <div className="min-w-0">
                        <p className="text-sm font-medium text-text-primary">
                          {t("onboarding.first_request_title")}
                        </p>
                        <p className="mt-1 text-xs text-text-secondary">
                          {t("onboarding.first_request_desc")}
                        </p>
                      </div>
                    </div>
                    {firstCommands.length > 0 && (
                      <ul className="space-y-2">
                        {firstCommands.map((cmd) => (
                          <li
                            key={cmd.command}
                            className="flex items-center justify-between gap-2 rounded-md border border-border bg-card px-3 py-2"
                          >
                            <div className="min-w-0">
                              <div className="text-xs uppercase tracking-wide text-text-muted">
                                {cmd.name}
                              </div>
                              <code className="block truncate font-mono text-xs text-text-primary">
                                {cmd.command}
                              </code>
                            </div>
                            <CopyButton text={cmd.command} />
                          </li>
                        ))}
                      </ul>
                    )}
                    <p className="text-xs text-text-muted">
                      {t("onboarding.first_request_after")}
                    </p>
                  </div>
                  <div className="flex justify-end gap-2">
                    <button
                      onClick={() => navigate("/")}
                      className="btn-secondary"
                    >
                      {t("onboarding.back_to_overview")}
                    </button>
                    <button
                      onClick={() => navigate("/tools")}
                      className="btn-primary"
                    >
                      {t("onboarding.go_to_clients")}
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <div className="rounded-lg border border-warning/30 bg-warning-soft p-3">
                    <p className="text-xs font-medium text-warning">
                      {t("onboarding.recovery_title")}
                    </p>
                    <p className="mt-1 text-xs leading-relaxed text-text-secondary">
                      {t("onboarding.recovery_desc")}
                    </p>
                  </div>
                  <div className="flex flex-wrap justify-end gap-2">
                    {primaryFailureCta && (
                      <button
                        onClick={goPrimaryFailure}
                        className="btn-primary"
                      >
                        {primaryFailureCta === "retry" &&
                          t("onboarding.next_retry")}
                        {primaryFailureCta === "providers" &&
                          t("onboarding.next_providers")}
                        {primaryFailureCta === "clients" &&
                          t("onboarding.next_clients")}
                        {primaryFailureCta === "logs" &&
                          t("onboarding.next_logs")}
                      </button>
                    )}
                    <button
                      onClick={() => setStep("key")}
                      className="btn-secondary"
                    >
                      {t("onboarding.edit_key")}
                    </button>
                  </div>
                </>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
