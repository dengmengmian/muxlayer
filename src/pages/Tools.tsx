import { useState, useEffect, useCallback, useMemo } from "react";
import {
  Zap,
  Activity,
  CheckCircle,
  XCircle,
  Loader2,
  Monitor,
  Eye,
} from "lucide-react";
import { ClientLogo, type ClientLogoId } from "@/components/tools/ClientLogo";
import { StatusBadge } from "@/components/common/StatusBadge";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { PostApplyDialog } from "@/components/tools/PostApplyDialog";
import { ClientHistoryButton } from "@/components/tools/ClientHistoryButton";
import { CodexDetail } from "@/components/tools/clients/CodexDetail";
import { ClaudeDetail } from "@/components/tools/clients/ClaudeDetail";
import { OpenCodeDetail } from "@/components/tools/clients/OpenCodeDetail";
import { GeminiDetail } from "@/components/tools/clients/GeminiDetail";
import { AtomCodeDetail } from "@/components/tools/clients/AtomCodeDetail";
import { KimiCliDetail } from "@/components/tools/clients/KimiCliDetail";
import { GrokBuildDetail } from "@/components/tools/clients/GrokBuildDetail";
import { DeepSeekHarnessDetail } from "@/components/tools/clients/DeepSeekHarnessDetail";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import { usePolling } from "@/lib/usePolling";
import * as api from "@/lib/api";
import type {
  CodexConfigStatus,
  ClaudeCodeEnvStatus,
  OpenCodeConfigStatus,
  GeminiCliConfigStatus,
  AtomCodeConfigStatus,
  ClaudeDesktopStatus,
  KimiCliConfigStatus,
  GrokBuildConfigStatus,
  DeepSeekHarnessConfigStatus,
} from "@/types/config";
import { useGatewayStatus } from "@/store/global";

/// Master-detail 布局：左侧 5 行客户端列表常驻显示状态，右侧渲染选中客户端
/// 的完整详情。比原先的手风琴更适合「同时管理 5 个客户端」的场景——总览不
/// 丢失、详情区不再被卡片 chrome 切碎。
type ClientId = ClientLogoId;

/// 把每个客户端在「列表行」上需要的状态压成统一三态：
/// - `active`：已接入 MuxLayer
/// - `detected`：检测到配置但未接入 MuxLayer
/// - `absent`：未检测到
type ClientPresence = "active" | "detected" | "absent";

/// 需要确认弹窗的「应用配置」流程，按客户端表驱动：确认文案 + 写配置命令 +
/// 应用成功后 PostApplyDialog 用的 clientId / 名称（clientId 同时是进程探测参数，
/// Gemini CLI 在后端叫 "gemini"）。
interface ApplyFlow {
  clientId: string;
  clientName: string;
  titleKey: string;
  messageKey: string;
  apply: () => Promise<api.ApplyConfigResult>;
}

const APPLY_FLOWS = {
  codex: {
    clientId: "codex",
    clientName: "Codex",
    titleKey: "tools.apply_codex_title",
    messageKey: "tools.apply_codex_msg",
    apply: () => api.applyCodexConfig(),
  },
  claude_code: {
    clientId: "claude_code",
    clientName: "Claude Code",
    titleKey: "tools.apply_claude_title",
    messageKey: "tools.apply_claude_msg",
    apply: () => api.applyClaudeCodeConfig(),
  },
  opencode: {
    clientId: "opencode",
    clientName: "OpenCode",
    titleKey: "tools.apply_opencode_title",
    messageKey: "tools.apply_opencode_msg",
    apply: () => api.applyOpenCodeConfig(),
  },
  gemini_cli: {
    clientId: "gemini",
    clientName: "Gemini CLI",
    titleKey: "tools.apply_gemini_title",
    messageKey: "tools.apply_gemini_msg",
    apply: () => api.applyGeminiConfig(),
  },
  atomcode: {
    clientId: "atomcode",
    clientName: "AtomCode",
    titleKey: "tools.apply_atomcode_title",
    messageKey: "tools.apply_atomcode_msg",
    apply: () => api.applyAtomCodeConfig(),
  },
  kimi_cli: {
    clientId: "kimi_cli",
    clientName: "Kimi CLI",
    titleKey: "tools.apply_kimi_title",
    messageKey: "tools.apply_kimi_msg",
    apply: () => api.applyKimiConfig(),
  },
  grok_build: {
    clientId: "grok_build",
    clientName: "Grok Build",
    titleKey: "tools.apply_grok_title",
    messageKey: "tools.apply_grok_msg",
    apply: () => api.applyGrokConfig(),
  },
  deepseek_harness: {
    clientId: "deepseek_harness",
    clientName: "DeepSeek Harness",
    titleKey: "tools.apply_dsh_title",
    messageKey: "tools.apply_dsh_msg",
    apply: () => api.applyDshConfig(),
  },
} satisfies Partial<Record<ClientId, ApplyFlow>>;

/// 客户端配置 / 进程探测的轮询周期。读多个配置文件 + pgrep，变化频率低；
/// usePolling 在窗口重新聚焦时会立即补一次。
const DETECT_POLL_MS = 60_000;

export function Tools() {
  const { t } = useI18n();
  const [codexStatus, setCodexStatus] = useState<CodexConfigStatus | null>(
    null
  );
  const [claudeEnv, setClaudeEnv] = useState<ClaudeCodeEnvStatus | null>(null);
  const [codexConfig, setCodexConfig] = useState("");
  const [claudeSnippet, setClaudeSnippet] = useState("");
  const [loading, setLoading] = useState(true);
  const [testResult, setTestResult] = useState<api.ConnectionTestResult | null>(
    null
  );
  const [testing, setTesting] = useState(false);
  /// 各客户端当前是否有进程在跑（best-effort）。key 与 ClientId / detect 参数对齐。
  const [processRunning, setProcessRunning] = useState<
    Partial<Record<ClientId, number>>
  >({});
  const [openCodeStatus, setOpenCodeStatus] =
    useState<OpenCodeConfigStatus | null>(null);
  const [geminiStatus, setGeminiStatus] =
    useState<GeminiCliConfigStatus | null>(null);
  const [atomCodeStatus, setAtomCodeStatus] =
    useState<AtomCodeConfigStatus | null>(null);
  const [claudeDesktopStatus, setClaudeDesktopStatus] =
    useState<ClaudeDesktopStatus | null>(null);
  const [kimiStatus, setKimiStatus] = useState<KimiCliConfigStatus | null>(
    null
  );
  const [grokStatus, setGrokStatus] = useState<GrokBuildConfigStatus | null>(
    null
  );
  const [dshStatus, setDshStatus] =
    useState<DeepSeekHarnessConfigStatus | null>(null);
  const [cdPreview, setCdPreview] = useState("");
  const [historyClients, setHistoryClients] = useState<string[]>([]);
  // gateway status 走全局 store——Topbar 常驻轮询，这里只订阅。
  const gatewayStatus = useGatewayStatus((s) => s.value);
  const [startingGateway, setStartingGateway] = useState(false);

  /// 等待用户确认的「应用配置」流程；null = 没有确认弹窗。
  const [pendingApply, setPendingApply] = useState<ApplyFlow | null>(null);

  /// Post-apply summary: shown once per apply with config path + running
  /// process warning. Null means "no dialog open right now". Detect failure
  /// degrades to processes=[] so the dialog still shows the success state.
  const [postApply, setPostApply] = useState<{
    clientId: string;
    clientName: string;
    configPath: string;
    processes: api.RunningProcess[];
  } | null>(null);

  // 当前选中的客户端。默认选第一个「已应用 / 检测到」的客户端，没有则回退
  // 到 codex（catalog 的第一项）。用 sessionStorage 记住一下，刷新不丢。
  const [selectedClientId, setSelectedClientId] = useState<ClientId>(() => {
    const saved = sessionStorage.getItem(
      "agentgate_tools_selected"
    ) as ClientId | null;
    return saved ?? "codex";
  });
  useEffect(() => {
    sessionStorage.setItem("agentgate_tools_selected", selectedClientId);
  }, [selectedClientId]);

  const showPostApply = async (
    clientId: string,
    clientName: string,
    configPath: string
  ) => {
    let processes: api.RunningProcess[] = [];
    try {
      processes = await api.detectClientRunning(clientId);
    } catch {
      // Detection is best-effort. Permission denied / Windows / pgrep
      // missing all degrade to "we don't know" — dialog renders without
      // the warning band.
    }
    setPostApply({ clientId, clientName, configPath, processes });
  };

  const load = useCallback(async () => {
    try {
      const [c, cc, oc, gc, ac, cd, kimi, grok, dsh, hist] = await Promise.all([
        api.detectCodexConfig(),
        api.detectClaudeCodeEnv(),
        api.detectOpenCodeConfig(),
        api.detectGeminiConfig(),
        api.detectAtomCodeConfig(),
        api.detectClaudeDesktop().catch(() => null),
        api.detectKimiConfig().catch(() => null),
        api.detectGrokConfig().catch(() => null),
        api.detectDshConfig().catch(() => null),
        api.clientsWithApplyHistory().catch(() => [] as string[]),
      ]);
      setCodexStatus(c);
      setClaudeEnv(cc);
      setOpenCodeStatus(oc);
      setGeminiStatus(gc);
      setAtomCodeStatus(ac);
      setClaudeDesktopStatus(cd);
      setKimiStatus(kimi);
      setGrokStatus(grok);
      setDshStatus(dsh);
      setHistoryClients(hist);
      const snippet = await api.generateCodexConfig();
      setCodexConfig(snippet);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
    // Topbar 通常已经拉过；store 为空时（如直接渲染本页）补一次。
    if (!useGatewayStatus.getState().value) {
      useGatewayStatus.getState().fetch();
    }
  }, [load]);
  // window focus 时刷新——从终端切回时立刻看到 Codex 应用配置后的状态变化
  usePolling(load, DETECT_POLL_MS);

  // 客户端进程探测：配置已写盘后，进程仍跑旧配置是常见坑。
  // 只对已接入的客户端查 pgrep/tasklist，失败当 unknown（count 不写）。
  const refreshProcessStatus = useCallback(async () => {
    const detectId = (id: ClientId): string | null => {
      if (id === "claude_desktop") return null; // GUI，basename 不稳
      if (id === "gemini_cli") return "gemini";
      if (id === "kimi_cli") return "kimi";
      if (id === "grok_build") return "grok";
      if (id === "deepseek_harness") return "dsh";
      return id;
    };
    const activeIds = (
      [
        ["codex", codexStatus?.has_agentgate],
        ["claude_code", claudeEnv?.has_agentgate],
        ["opencode", openCodeStatus?.has_agentgate],
        ["gemini_cli", geminiStatus?.has_agentgate],
        ["atomcode", atomCodeStatus?.has_agentgate],
        ["kimi_cli", kimiStatus?.has_agentgate],
        ["grok_build", grokStatus?.has_agentgate],
        ["deepseek_harness", dshStatus?.has_agentgate],
      ] as const
    )
      .filter(([, ok]) => !!ok)
      .map(([id]) => id as ClientId);

    const next: Partial<Record<ClientId, number>> = {};
    await Promise.all(
      activeIds.map(async (id) => {
        const needle = detectId(id);
        if (!needle) return;
        try {
          const procs = await api.detectClientRunning(needle);
          next[id] = procs.length;
        } catch {
          // 探测失败不写 key → UI 显示 unknown
        }
      })
    );
    setProcessRunning(next);
  }, [
    codexStatus?.has_agentgate,
    claudeEnv?.has_agentgate,
    openCodeStatus?.has_agentgate,
    geminiStatus?.has_agentgate,
    atomCodeStatus?.has_agentgate,
    kimiStatus?.has_agentgate,
    grokStatus?.has_agentgate,
    dshStatus?.has_agentgate,
  ]);

  useEffect(() => {
    refreshProcessStatus();
  }, [refreshProcessStatus]);
  usePolling(refreshProcessStatus, DETECT_POLL_MS);

  const confirmPendingApply = async () => {
    const flow = pendingApply;
    if (!flow) return;
    try {
      const result = await flow.apply();
      setPendingApply(null);
      load();
      if (result.success) {
        await showPostApply(flow.clientId, flow.clientName, result.config_path);
      }
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleApplyClaudeDesktop = async () => {
    try {
      const result = await api.applyClaudeDesktopConfig();
      load();
      if (result.success) {
        await showPostApply(
          "claude_desktop",
          "Claude Desktop",
          result.profile_path
        );
      }
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handlePreviewClaudeDesktop = async () => {
    try {
      setCdPreview(await api.previewClaudeDesktopProfile());
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleToggleCodex = async () => {
    try {
      const result = await api.toggleCodexProvider();
      if (result.success) {
        const label =
          result.new_provider === "agentgate"
            ? "MuxLayer"
            : result.new_provider;
        toast("success", `${t("tools.switched_to")} ${label}`);
        await showPostApply("codex", "Codex", result.config_path);
      }
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  // `disable_codex_agentgate` is exposed via api.disableCodexAgentgate() and
  // does the same restore as `handleToggleCodex` going compat → native (the
  // existing "切换到官方" button covers it). Kept as a backend primitive for
  // future direct callers; UI keeps the single toggle.

  const handleToggleClaude = async () => {
    try {
      const result = await api.toggleClaudeCodeProvider();
      if (result.success) {
        const label =
          result.new_provider === "agentgate"
            ? "MuxLayer"
            : t("tools.official");
        toast("success", `${t("tools.switched_to")} ${label}`);
        await showPostApply("claude_code", "Claude Code", result.config_path);
      }
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleGenerateClaudeSnippet = async () => {
    try {
      const snippet = await api.generateClaudeCodeEnv();
      setClaudeSnippet(snippet);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleToggleGemini = async () => {
    try {
      const result = await api.toggleGeminiProvider();
      if (result.success) {
        const label =
          result.new_provider === "agentgate"
            ? "MuxLayer"
            : t("tools.official");
        toast("success", `${t("tools.switched_to")} ${label}`);
        await showPostApply("gemini", "Gemini CLI", result.config_path);
      }
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleToggleAtomCode = async () => {
    try {
      const result = await api.toggleAtomCodeProvider();
      if (result.success) {
        const label =
          result.new_provider === "agentgate"
            ? "MuxLayer"
            : t("tools.official");
        toast("success", `${t("tools.switched_to")} ${label}`);
        await showPostApply("atomcode", "AtomCode", result.config_path);
      }
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleTestConnection = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const result = await api.testToolConnection();
      setTestResult(result);
    } catch {
      setTestResult({
        config_ok: false,
        gateway_ok: false,
        provider_ok: false,
        error: "Test failed",
      });
    } finally {
      setTesting(false);
    }
  };

  const handleStartGateway = async () => {
    setStartingGateway(true);
    try {
      useGatewayStatus.getState().setValue(await api.startGateway());
      toast("success", t("gateway.started"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setStartingGateway(false);
      load();
    }
  };

  // 列表行的元数据。每个客户端的 presence 直接从对应 status 推断，避免
  // 列表/详情两边对「是否检测到」的判定标准不一致。
  const clientRows: {
    id: ClientId;
    name: string;
    desc: string;
    presence: ClientPresence;
    drifted: boolean;
  }[] = useMemo(() => {
    // 配置漂移:接入过(在 apply 历史里)但当前掉成 detected,说明配置被改回去了。
    // 注意 Gemini CLI 在 apply 历史里 client_id 存的是 "gemini",和前端 id 不一致。
    const drifted = (id: ClientId, presence: ClientPresence) => {
      const histId = id === "gemini_cli" ? "gemini" : id;
      return presence === "detected" && historyClients.includes(histId);
    };
    const codexPresence: ClientPresence = codexStatus?.has_agentgate
      ? "active"
      : codexStatus?.exists
        ? "detected"
        : "absent";
    const claudePresence: ClientPresence = claudeEnv?.has_agentgate
      ? "active"
      : claudeEnv?.has_api_key || claudeEnv?.has_auth_token
        ? "detected"
        : "absent";
    const opencodePresence: ClientPresence = openCodeStatus?.has_agentgate
      ? "active"
      : openCodeStatus?.exists
        ? "detected"
        : "absent";
    const geminiPresence: ClientPresence = geminiStatus?.has_agentgate
      ? "active"
      : geminiStatus?.exists
        ? "detected"
        : "absent";
    const atomPresence: ClientPresence = atomCodeStatus?.has_agentgate
      ? "active"
      : atomCodeStatus?.exists
        ? "detected"
        : "absent";
    const kimiPresence: ClientPresence = kimiStatus?.has_agentgate
      ? "active"
      : kimiStatus?.exists
        ? "detected"
        : "absent";
    const grokPresence: ClientPresence = grokStatus?.has_agentgate
      ? "active"
      : grokStatus?.exists
        ? "detected"
        : "absent";
    const dshPresence: ClientPresence = dshStatus?.has_agentgate
      ? "active"
      : dshStatus?.exists
        ? "detected"
        : "absent";
    const claudeDesktopPresence: ClientPresence =
      claudeDesktopStatus?.has_agentgate_profile
        ? "active"
        : claudeDesktopStatus?.installed
          ? "detected"
          : "absent";
    return [
      {
        id: "codex",
        name: t("tools.codex"),
        desc: t("tools.codex_desc"),
        presence: codexPresence,
        drifted: drifted("codex", codexPresence),
      },
      {
        id: "claude_code",
        name: t("tools.claude_code"),
        desc: t("tools.claude_code_desc"),
        presence: claudePresence,
        drifted: drifted("claude_code", claudePresence),
      },
      {
        id: "opencode",
        name: t("tools.opencode"),
        desc: t("tools.opencode_desc"),
        presence: opencodePresence,
        drifted: drifted("opencode", opencodePresence),
      },
      {
        id: "gemini_cli",
        name: t("tools.gemini_cli"),
        desc: t("tools.gemini_cli_desc"),
        presence: geminiPresence,
        drifted: drifted("gemini_cli", geminiPresence),
      },
      {
        id: "atomcode",
        name: t("tools.atomcode"),
        desc: t("tools.atomcode_desc"),
        presence: atomPresence,
        drifted: drifted("atomcode", atomPresence),
      },
      {
        id: "claude_desktop",
        name: "Claude Desktop",
        desc: t("tools.claude_desktop_desc"),
        presence: claudeDesktopPresence,
        drifted: drifted("claude_desktop", claudeDesktopPresence),
      },
      {
        id: "kimi_cli",
        name: t("tools.kimi_cli"),
        desc: t("tools.kimi_cli_desc"),
        presence: kimiPresence,
        drifted: drifted("kimi_cli", kimiPresence),
      },
      {
        id: "grok_build",
        name: t("tools.grok_build"),
        desc: t("tools.grok_build_desc"),
        presence: grokPresence,
        drifted: drifted("grok_build", grokPresence),
      },
      {
        id: "deepseek_harness",
        name: t("tools.deepseek_harness"),
        desc: t("tools.deepseek_harness_desc"),
        presence: dshPresence,
        drifted: drifted("deepseek_harness", dshPresence),
      },
    ];
  }, [
    codexStatus,
    claudeEnv,
    openCodeStatus,
    geminiStatus,
    atomCodeStatus,
    claudeDesktopStatus,
    kimiStatus,
    grokStatus,
    dshStatus,
    historyClients,
    t,
  ]);

  if (loading)
    return <p className="text-xs text-text-muted">{t("common.loading")}</p>;

  return (
    <div className="desktop-page">
      <header className="desktop-page-header">
        <div>
          <h2 className="flex items-center gap-2 text-lg font-semibold text-text-primary">
            <Monitor className="h-4 w-4" />
            {t("tools.clients")}
          </h2>
          <p className="mt-1 max-w-2xl text-xs text-text-muted">
            {t("tools.console_hint")}
          </p>
        </div>
      </header>

      {/* Connection Status Bar */}
      <div className="surface-panel p-4">
        <div className="mb-3">
          <h3 className="text-sm font-semibold text-text-primary">
            {t("tools.connection_path")}
          </h3>
          <p className="mt-0.5 text-xs text-text-muted">
            {t("tools.connection_path_hint")}
          </p>
        </div>
        <div className="flex min-w-0 items-center justify-between gap-3">
          <div className="surface-scroll flex min-w-0 items-center gap-6 pb-1">
            <ConnectionStep
              label={t("tools.step_config")}
              ok={testResult?.config_ok ?? null}
              testing={testing}
            />
            <div className="h-px w-6 bg-border" />
            <ConnectionStep
              label={t("tools.step_gateway")}
              ok={testResult?.gateway_ok ?? null}
              testing={testing}
            />
            <div className="h-px w-6 bg-border" />
            <ConnectionStep
              label={t("tools.step_provider")}
              ok={testResult?.provider_ok ?? null}
              testing={testing}
            />
            <div className="h-px w-6 bg-border" />
            <ConnectionStep
              label={t("tools.step_client_process")}
              ok={
                testResult?.client_process_ok === undefined
                  ? null
                  : testResult.client_process_ok
              }
              testing={testing}
            />
          </div>
          <button
            onClick={handleTestConnection}
            disabled={testing}
            className="btn-secondary"
          >
            {testing ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <Activity className="h-3 w-3" />
            )}
            {t("tools.test_connection")}
          </button>
        </div>
        {gatewayStatus && (
          <div
            className={`mt-3 flex items-center justify-between rounded-md border px-3 py-2 ${
              gatewayStatus.running
                ? "border-info/30 bg-info-soft"
                : "border-warning/30 bg-warning/5"
            }`}
          >
            <p
              className={`text-xs ${gatewayStatus.running ? "text-info" : "text-warning"}`}
            >
              {gatewayStatus.running
                ? `${t("tools.gateway_running")} http://${gatewayStatus.host}:${gatewayStatus.port}`
                : t("tools.gateway_not_running_hint")}
            </p>
            {!gatewayStatus.running && (
              <button
                onClick={handleStartGateway}
                disabled={startingGateway}
                className="btn-primary"
              >
                {startingGateway ? (
                  <Loader2 className="h-3 w-3 animate-spin" />
                ) : (
                  <Activity className="h-3 w-3" />
                )}
                {t("gateway.start")}
              </button>
            )}
          </div>
        )}
        {testResult?.error && (
          <p className="mt-2 text-xs text-error">{testResult.error}</p>
        )}
        {testResult?.client_processes &&
          testResult.client_processes.length > 0 && (
            <ul className="mt-2 space-y-1 text-xs text-text-muted">
              {testResult.client_processes.map((c) => (
                <li key={c.client_id}>
                  <span className="font-medium text-text-secondary">
                    {c.client_id}
                  </span>
                  {": "}
                  {c.running
                    ? t("tools.client_process_running")
                    : t("tools.client_process_idle")}
                  {c.running ? ` (${c.count})` : ""}
                </li>
              ))}
            </ul>
          )}
      </div>

      {/* Master-detail */}
      <div className="grid min-w-0 grid-cols-[220px_minmax(0,1fr)] gap-4">
        {/* Left list */}
        <aside className="surface-panel p-2">
          {/* 4.1 状态总汇 + 4.2 漂移提示 */}
          <div className="flex items-center justify-between px-2.5 py-1.5 text-xs text-text-muted">
            <span>{t("tools.clients")}</span>
            <span>
              {t("tools.connected_count")}{" "}
              {clientRows.filter((r) => r.presence === "active").length}/
              {clientRows.length}
            </span>
          </div>
          {clientRows.some((r) => r.drifted) && (
            <div className="mb-1 px-2.5 text-xs text-warning">
              {clientRows.filter((r) => r.drifted).length}{" "}
              {t("tools.drift_count_hint")}
            </div>
          )}
          <ul className="space-y-1">
            {clientRows.map((row) => {
              const selected = selectedClientId === row.id;
              return (
                <li key={row.id}>
                  <button
                    type="button"
                    onClick={() => setSelectedClientId(row.id)}
                    className={
                      "flex w-full items-center gap-3 rounded-lg px-2.5 py-2 text-left transition-colors " +
                      (selected
                        ? "bg-accent-soft text-accent"
                        : "text-text-secondary hover:bg-hover hover:text-text-primary")
                    }
                  >
                    <PresenceDot presence={row.presence} />
                    <ClientLogo id={row.id} className="h-5 w-5 shrink-0" />
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-xs font-medium">
                        {row.name}
                      </div>
                      <div
                        className={
                          "truncate text-xs " +
                          (row.drifted
                            ? "text-warning"
                            : selected
                              ? "text-accent/80"
                              : "text-text-muted")
                        }
                      >
                        {row.drifted
                          ? t("tools.drifted_label")
                          : presenceLabel(row.presence, t)}
                      </div>
                      {row.presence === "active" &&
                        processRunning[row.id] !== undefined && (
                          <div
                            className={
                              "truncate text-xs " +
                              (processRunning[row.id]! > 0
                                ? "text-warning"
                                : "text-text-muted")
                            }
                            title={
                              processRunning[row.id]! > 0
                                ? t("tools.client_process_running")
                                : t("tools.client_process_idle")
                            }
                          >
                            {processRunning[row.id]! > 0
                              ? t("tools.client_process_running")
                              : t("tools.client_process_idle")}
                          </div>
                        )}
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>
        </aside>

        {/* Right detail */}
        <section className="min-w-0">
          {selectedClientId === "codex" && (
            <CodexDetail
              status={codexStatus}
              codexConfig={codexConfig}
              onApply={() => setPendingApply(APPLY_FLOWS.codex)}
              onToggle={handleToggleCodex}
              load={load}
              t={t}
            />
          )}
          {selectedClientId === "claude_code" && (
            <ClaudeDetail
              env={claudeEnv}
              snippet={claudeSnippet}
              onApply={() => setPendingApply(APPLY_FLOWS.claude_code)}
              onToggle={handleToggleClaude}
              onGenerateSnippet={handleGenerateClaudeSnippet}
              load={load}
              t={t}
            />
          )}
          {selectedClientId === "opencode" && (
            <OpenCodeDetail
              status={openCodeStatus}
              onApply={() => setPendingApply(APPLY_FLOWS.opencode)}
              load={load}
              t={t}
            />
          )}
          {selectedClientId === "gemini_cli" && (
            <GeminiDetail
              status={geminiStatus}
              onApply={() => setPendingApply(APPLY_FLOWS.gemini_cli)}
              onToggle={handleToggleGemini}
              load={load}
              t={t}
            />
          )}
          {selectedClientId === "atomcode" && (
            <AtomCodeDetail
              status={atomCodeStatus}
              onApply={() => setPendingApply(APPLY_FLOWS.atomcode)}
              onToggle={handleToggleAtomCode}
              load={load}
              t={t}
            />
          )}
          {selectedClientId === "kimi_cli" && (
            <KimiCliDetail
              status={kimiStatus}
              onApply={() => setPendingApply(APPLY_FLOWS.kimi_cli)}
              load={load}
              t={t}
            />
          )}
          {selectedClientId === "grok_build" && (
            <GrokBuildDetail
              status={grokStatus}
              onApply={() => setPendingApply(APPLY_FLOWS.grok_build)}
              load={load}
              t={t}
            />
          )}
          {selectedClientId === "deepseek_harness" && (
            <DeepSeekHarnessDetail
              status={dshStatus}
              onApply={() => setPendingApply(APPLY_FLOWS.deepseek_harness)}
              load={load}
              t={t}
            />
          )}
          {selectedClientId === "claude_desktop" && (
            <div className="surface-panel p-5">
              <DetailHeader
                clientId="claude_desktop"
                name="Claude Desktop"
                desc={t("tools.claude_desktop_detail_desc")}
                badge={
                  <StatusBadge
                    variant={
                      claudeDesktopStatus?.has_agentgate_profile
                        ? "success"
                        : claudeDesktopStatus?.installed
                          ? "warning"
                          : "muted"
                    }
                  >
                    {claudeDesktopStatus?.has_agentgate_profile
                      ? t("tools.agentgate_configured")
                      : claudeDesktopStatus?.installed
                        ? t("tools.not_configured")
                        : t("tools.no_config")}
                  </StatusBadge>
                }
              />

              {!claudeDesktopStatus?.supported ? (
                <p className="text-xs text-error">
                  {t("tools.claude_desktop_unsupported")}
                </p>
              ) : !claudeDesktopStatus?.installed ? (
                <p className="text-xs text-text-muted">
                  {t("tools.claude_desktop_not_detected")}
                </p>
              ) : (
                <>
                  <div className="mb-4 text-xs">
                    <span className="text-text-muted">
                      {t("tools.claude_desktop_profile_label")}
                    </span>
                    <p className="break-all font-mono text-xs text-text-secondary">
                      {claudeDesktopStatus.profile_path}
                    </p>
                  </div>

                  <div className="flex flex-wrap gap-2">
                    <button
                      onClick={handleApplyClaudeDesktop}
                      className="btn-primary"
                    >
                      <Zap className="h-3 w-3" />
                      {t("tools.apply_config")}
                    </button>
                    <button
                      onClick={handlePreviewClaudeDesktop}
                      className="btn-secondary"
                    >
                      <Eye className="h-3 w-3" />
                      {t("tools.preview_profile")}
                    </button>
                    <ClientHistoryButton
                      clientId="claude_desktop"
                      clientName="Claude Desktop"
                      onRollbackDone={load}
                    />
                  </div>

                  {cdPreview && (
                    <pre className="mt-3 max-h-60 overflow-auto rounded-md bg-card-secondary p-3 text-xs text-text-primary">
                      {cdPreview}
                    </pre>
                  )}
                  <p className="mt-3 text-xs text-text-muted">
                    {t("tools.claude_desktop_restart_hint")}
                  </p>
                </>
              )}
            </div>
          )}
        </section>
      </div>

      <ConfirmDialog
        open={pendingApply !== null}
        title={pendingApply ? t(pendingApply.titleKey) : ""}
        message={pendingApply ? t(pendingApply.messageKey) : ""}
        confirmLabel={t("common.apply")}
        variant="default"
        onConfirm={confirmPendingApply}
        onCancel={() => setPendingApply(null)}
      />

      <PostApplyDialog
        open={postApply !== null}
        clientId={postApply?.clientId}
        clientName={postApply?.clientName ?? ""}
        configPath={postApply?.configPath ?? ""}
        processes={postApply?.processes ?? []}
        onClose={() => setPostApply(null)}
      />
    </div>
  );
}

// ── Helpers ────────────────────────────────────────────────────

function PresenceDot({ presence }: { presence: ClientPresence }) {
  const cls =
    presence === "active"
      ? "bg-success"
      : presence === "detected"
        ? "bg-warning"
        : "bg-border";
  return <span className={`h-2 w-2 shrink-0 rounded-full ${cls}`} />;
}

function presenceLabel(p: ClientPresence, t: (k: string) => string): string {
  switch (p) {
    case "active":
      return t("tools.agentgate_configured");
    case "detected":
      return t("tools.not_configured");
    case "absent":
      return t("tools.no_config");
  }
}

export type T = (k: string) => string;

/// 详情区共用的页眉：图标 + 标题 + 描述 + 状态徽章。
export function DetailHeader({
  clientId,
  name,
  desc,
  badge,
}: {
  clientId: ClientLogoId;
  name: string;
  desc: string;
  badge: React.ReactNode;
}) {
  return (
    <div className="mb-4 flex items-start justify-between gap-3">
      <div className="flex items-center gap-3">
        <ClientLogo id={clientId} className="h-10 w-10 shrink-0" />
        <div>
          <h3 className="text-sm font-semibold text-text-primary">{name}</h3>
          <p className="text-xs text-text-muted">{desc}</p>
        </div>
      </div>
      <div>{badge}</div>
    </div>
  );
}

// ── Per-client detail components ───────────────────────────────

function ConnectionStep({
  label,
  ok,
  testing,
}: {
  label: string;
  ok: boolean | null;
  testing: boolean;
}) {
  return (
    <div className="flex items-center gap-2">
      {testing ? (
        <Loader2 className="h-4 w-4 animate-spin text-text-muted" />
      ) : ok === null ? (
        <div className="h-4 w-4 rounded-full border-2 border-border" />
      ) : ok ? (
        <CheckCircle className="h-4 w-4 text-success" />
      ) : (
        <XCircle className="h-4 w-4 text-error" />
      )}
      <span
        className={`text-xs ${ok === true ? "text-success" : ok === false ? "text-error" : "text-text-muted"}`}
      >
        {label}
      </span>
    </div>
  );
}
