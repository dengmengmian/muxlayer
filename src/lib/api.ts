// MuxLayer 前端 API 客户端。
//
// **类型来源**: src/lib/bindings.ts 由 Rust 端 cargo test 自动生成
// （tauri-specta 反射 #[tauri::command] + #[specta::specta]）。所有 input /
// return 都从 bindings 拿，前端不再手抄 type。少数 narrow union（PetType /
// PetState / 类似的字符串字面量集合）Rust 端字段就是 String，bindings 给的
// 是 string——我们在 api.ts 边界 `as` cast 到 src/types/*.ts 的窄类型。
// 这是 trade-off，不是 bug：要么前端窄、要么 Rust 端 enum，目前保留前端窄。
//
// **错误处理**: bindings 命令返回 Promise<Result<T, AppError>>。`unwrap`
// helper 把 ok 拆出 T，error 直接 throw AppError，让旧调用方继续 try/catch。
import { commands as bindings } from "./bindings";
import type * as Bindings from "./bindings";
import type { Result } from "./bindings";

// re-export narrow union types & legacy hand-typed types from types/
import type { PetType, PetState, PetGatewayInfo } from "@/types/pet";

// ── Error type & unwrap helper ─────────────────────────────────

export interface AppError {
  code: string;
  message: string;
  detail?: string;
  suggestion?: string;
}

function normalizeError(raw: unknown): AppError {
  if (raw && typeof raw === "object" && "code" in raw && "message" in raw) {
    const e = raw as Partial<AppError>;
    return {
      code: String(e.code ?? "UNKNOWN"),
      message: String(e.message ?? ""),
      detail: e.detail ?? undefined,
      suggestion: e.suggestion ?? undefined,
    };
  }
  if (typeof raw === "string") return { code: "UNKNOWN", message: raw };
  return { code: "UNKNOWN", message: "An unexpected error occurred" };
}

async function unwrap<T>(promise: Promise<Result<T, unknown>>): Promise<T> {
  const r = await promise;
  if (r.status === "ok") return r.data;
  throw normalizeError(r.error);
}

// ── Input boundary ─────────────────────────────────────────────

type NullableKeys<B> = {
  [K in keyof B]-?: null extends B[K] ? K : never;
}[keyof B];

/// bindings 把 Rust `Option<T>` 映射成必填的 `T | null`；前端 input 类型
/// （src/types/*.ts）允许省略这些字段。非 Option 字段仍然必填——Rust 端新增
/// 必填字段、或把 Option 改成必填时，这里的调用点会编译失败。
export type BindingsInput<B> = Omit<B, NullableKeys<B>> & {
  [K in NullableKeys<B>]?: B[K] | undefined;
};

/// serde 把缺省的 Option 字段反序列化为 None，与显式 null 等价，所以运行时
/// 原样透传；类型检查由 BindingsInput<B> 完成。
function toBindingsInput<B>(input: BindingsInput<B>): B {
  return input as B;
}

// ── Re-export bindings types so call sites don't have to import twice ─

export type {
  ProviderView,
  CreateProviderInput,
  UpdateProviderInput,
  ProviderTestResult,
  ProviderSpeedReport,
  GatewayStatus,
  GatewaySettings,
  UpdateGatewaySettingsInput,
  WakeStatus,
  RequestLogListItem,
  RequestLogDetail,
  RequestLogFilter,
  SessionUsageSummary,
  CostBreakdown,
  ConversationMessage,
  ProviderDetailStats,
  ProviderLatencyPoint,
  ProviderModelStats,
  ToolConfigView,
  GatewayAuthSettings,
  CodexConfigStatus,
  ClaudeCodeEnvStatus,
  ProfileDetection,
  OpenCodeConfigStatus,
  GeminiCliConfigStatus,
  AtomCodeConfigStatus,
  ClaudeDesktopStatus,
  ClaudeDesktopApplyResult,
  CodexRestartResult,
  RunningProcess,
  LocalEndpoint,
  HistoryEntry as ClientApplyHistoryEntry,
  McpServer,
  McpEnvVar,
  McpServerSource,
  McpValidationState,
  McpValidationIssue,
  UpsertMcpServerInput,
  SyncMcpServerInput,
  InstructionsTemplate,
  InstructionsBackup,
  InstructionsStatus,
  Skill,
  SkillFile,
  SkillsExport,
  RouteProfileView,
  RouteProfileDetail,
  RouteProfileProviderView,
  CreateRouteProfileInput,
  UpdateRouteProfileInput,
  AddProviderToRouteInput,
  ProviderRuntimeStatus,
  RouteProfileStats,
  RequestStats,
  DailyStat,
  ProviderStat,
  ProviderHealth,
  RecentError,
  RuntimeKpis,
  ModelPricing,
  CheckItem,
  CheckReport,
  FullSelfTestReport,
  ExportResult,
  PetSettings,
  UpdatePetSettingsInput,
  ImportSummary as ConfigImportSummary,
  SyncResult,
} from "./bindings";

// 5 客户端的 ApplyConfigResult 在 Rust 端是 5 个不同 struct,bindings 里用了
// 5 个不同名字防冲突；前端调用方传统上叫 ApplyConfigResult,在 api 边界做一次
// 别名,大部分 client 视角下字段是同形的(success / config_path / changed_keys
// / warnings,加上少量 client-specific 字段)。
import type {
  CodexApplyConfigResult,
  ClaudeCodeApplyConfigResult,
  OpenCodeApplyConfigResult,
  GeminiCliApplyConfigResult,
  AtomCodeApplyConfigResult,
  KimiCliApplyConfigResult,
  GrokBuildApplyConfigResult,
  DeepSeekHarnessApplyConfigResult,
  CodexToggleResult,
  ClaudeCodeToggleResult,
  GeminiCliToggleResult,
  AtomCodeToggleResult,
} from "./bindings";

// 历史 API 名字。多数前端组件按通用名引用,union 兼容各 client 的扩展字段。
export type ApplyConfigResult =
  | CodexApplyConfigResult
  | ClaudeCodeApplyConfigResult
  | OpenCodeApplyConfigResult
  | GeminiCliApplyConfigResult
  | AtomCodeApplyConfigResult
  | KimiCliApplyConfigResult
  | GrokBuildApplyConfigResult
  | DeepSeekHarnessApplyConfigResult;

export type ToggleResult =
  | CodexToggleResult
  | ClaudeCodeToggleResult
  | GeminiCliToggleResult
  | AtomCodeToggleResult;

// ── Providers ──────────────────────────────────────────────────

export const listProviders = () => unwrap(bindings.listProviders());
export const getProvider = (id: string) => unwrap(bindings.getProvider(id));
/// Plain-text api keys in storage order. Used by the edit form so each key
/// slot can be repopulated; the masked view alone hides which key is which.
export const getProviderKeys = (id: string) =>
  unwrap(bindings.getProviderKeys(id));
export const createProvider = (
  input: import("@/types/provider").CreateProviderInput
) =>
  unwrap(
    bindings.createProvider(
      toBindingsInput<Bindings.CreateProviderInput>(input)
    )
  );
export const updateProvider = (
  id: string,
  input: import("@/types/provider").UpdateProviderInput
) =>
  unwrap(
    bindings.updateProvider(
      id,
      toBindingsInput<Bindings.UpdateProviderInput>(input)
    )
  );
export const deleteProvider = (id: string) =>
  unwrap(bindings.deleteProvider(id));
export const setActiveProvider = (id: string) =>
  unwrap(bindings.setActiveProvider(id));
export const fetchProviderModels = (id: string) =>
  unwrap(bindings.fetchProviderModels(id));
export const testProvider = (id: string) => unwrap(bindings.testProvider(id));
export const providerSpeedtest = (id: string) =>
  unwrap(bindings.providerSpeedtest(id));
export const providerSpeedtestAll = () =>
  unwrap(bindings.providerSpeedtestAll());
export const detectProviderVision = (id: string) =>
  unwrap(bindings.detectProviderVision(id));
export const detectProviderCache = (id: string) =>
  unwrap(bindings.detectProviderCache(id));
export const seedModelCapabilities = (
  providerType: string,
  modelIds: string[]
) => unwrap(bindings.seedModelCapabilities(providerType, modelIds));
/// Fill missing rows in the provider's model_capabilities matrix from the seed
/// function (provider_type + model id pattern). Preserves manually-edited rows.
/// Returns the number of rows added.
export const autofillProviderCapabilities = (id: string) =>
  unwrap(bindings.autofillProviderCapabilities(id));
export const discoverLocalEndpoints = () =>
  unwrap(bindings.discoverLocalEndpoints());
export const getRefinerHint = (providerType: string) =>
  unwrap(bindings.getRefinerHint(providerType));

// ── Gateway ────────────────────────────────────────────────────

export const getGatewayStatus = () => unwrap(bindings.getGatewayStatus());
export const getGatewaySettings = () => unwrap(bindings.getGatewaySettings());
export const updateGatewaySettings = (
  input: import("@/types/gateway").UpdateGatewaySettingsInput
) =>
  unwrap(
    bindings.updateGatewaySettings(
      toBindingsInput<Bindings.UpdateGatewaySettingsInput>(input)
    )
  );
export const getWakeStatus = () => unwrap(bindings.getWakeStatus());
export const startGateway = () => unwrap(bindings.startGateway());
export const stopGateway = () => unwrap(bindings.stopGateway());
export const restartGateway = () => unwrap(bindings.restartGateway());

// ── Logs ───────────────────────────────────────────────────────

export const listRequestLogs = (
  filter: import("@/types/request-log").RequestLogFilter
) =>
  unwrap(
    bindings.listRequestLogs(toBindingsInput<Bindings.RequestLogFilter>(filter))
  );
export const listLogModels = () => unwrap(bindings.listLogModels());
export const getSessionConversation = (sessionId: string) =>
  unwrap(bindings.getSessionConversation(sessionId));
export const deleteSession = (sessionId: string) =>
  unwrap(bindings.deleteSession(sessionId));
export const countRequestLogs = (
  filter: import("@/types/request-log").RequestLogFilter
) =>
  unwrap(
    bindings.countRequestLogs(
      toBindingsInput<Bindings.RequestLogFilter>(filter)
    )
  );
export const getRequestLogDetail = (id: string) =>
  unwrap(bindings.getRequestLogDetail(id));
export const clearRequestLogs = () => unwrap(bindings.clearRequestLogs());
export const aggregateRequestLogsBySession = (
  filter: import("@/types/request-log").RequestLogFilter,
  limit?: number
) =>
  unwrap(
    bindings.aggregateRequestLogsBySession(
      toBindingsInput<Bindings.RequestLogFilter>(filter),
      limit ?? null
    )
  );
export const aggregateCostByModel = (days?: number, limit?: number) =>
  unwrap(bindings.aggregateCostByModel(days ?? null, limit ?? null));
export const aggregateCostByClient = (days?: number, limit?: number) =>
  unwrap(bindings.aggregateCostByClient(days ?? null, limit ?? null));
export const aggregateProviderDetailStats = (
  provider: string,
  days?: number,
  limit?: number
) =>
  unwrap(
    bindings.aggregateProviderDetailStats(provider, days ?? null, limit ?? null)
  );
export const aggregateRouteProfileStats = (days?: number) =>
  unwrap(bindings.aggregateRouteProfileStats(days ?? null));

export const syncClaudeSessions = () => unwrap(bindings.syncClaudeSessions());
export const syncCodexSessions = () => unwrap(bindings.syncCodexSessions());
export const syncGeminiSessions = () => unwrap(bindings.syncGeminiSessions());

// ── Tools ──────────────────────────────────────────────────────

export const listTools = () => unwrap(bindings.listTools());
export const generateCodexConfig = () => unwrap(bindings.generateCodexConfig());

// ── Claude Desktop（接入 MuxLayer 网关）──
// detect_claude_desktop Rust 侧返回裸值不是 Result,bindings 直接 Promise<T>,
// 不走 unwrap。
export const detectClaudeDesktop = () => bindings.detectClaudeDesktop();
export const previewClaudeDesktopProfile = () =>
  unwrap(bindings.previewClaudeDesktopProfile());
export const applyClaudeDesktopConfig = () =>
  unwrap(bindings.applyClaudeDesktopConfig());

// ── Gateway Auth ───────────────────────────────────────────────

export const getGatewayAuthSettings = () =>
  unwrap(bindings.getGatewayAuthSettings());
export const regenerateLocalAccessToken = () =>
  unwrap(bindings.regenerateLocalAccessToken());
export const getLocalAccessToken = () => unwrap(bindings.getLocalAccessToken());
export const ensureLocalAccessToken = () =>
  unwrap(bindings.ensureLocalAccessToken());
export const openTokenFolder = () => unwrap(bindings.openTokenFolder());

// ── Codex Config ───────────────────────────────────────────────

export const detectCodexConfig = () => unwrap(bindings.detectCodexConfig());
export const applyCodexConfig = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.applyCodexConfig());
export const toggleCodexProvider = (): Promise<ToggleResult> =>
  unwrap(bindings.toggleCodexProvider());
/** Switch Codex back to native mode (official ChatGPT path). Used to bring
 * IDE plugin entries back from their compat-mode grey state. */
export const disableCodexAgentgate = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.disableCodexAgentgate());
export const openCodexConfig = () => unwrap(bindings.openCodexConfig());

// ── Claude Code ────────────────────────────────────────────────

export const detectClaudeCodeEnv = () => unwrap(bindings.detectClaudeCodeEnv());
export const applyClaudeCodeConfig = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.applyClaudeCodeConfig());
export const toggleClaudeCodeProvider = (): Promise<ToggleResult> =>
  unwrap(bindings.toggleClaudeCodeProvider());
export const openClaudeCodeConfig = () =>
  unwrap(bindings.openClaudeCodeConfig());
export const generateClaudeCodeEnv = () =>
  unwrap(bindings.generateClaudeCodeEnv());

// ── OpenCode Config ───────────────────────────────────────────

export const detectOpenCodeConfig = () =>
  unwrap(bindings.detectOpencodeConfig());
export const applyOpenCodeConfig = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.applyOpencodeConfig());
export const generateOpenCodeConfig = () =>
  unwrap(bindings.generateOpencodeConfig());
export const openOpenCodeConfig = () => unwrap(bindings.openOpencodeConfig());

// ── Gemini CLI ────────────────────────────────────────────────

export const detectGeminiConfig = () => unwrap(bindings.detectGeminiConfig());
export const applyGeminiConfig = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.applyGeminiConfig());
export const generateGeminiConfig = () =>
  unwrap(bindings.generateGeminiConfig());
export const toggleGeminiProvider = (): Promise<ToggleResult> =>
  unwrap(bindings.toggleGeminiProvider());
export const openGeminiConfig = () => unwrap(bindings.openGeminiConfig());

// ── AtomCode ──────────────────────────────────────────────────

export const detectAtomCodeConfig = () =>
  unwrap(bindings.detectAtomcodeConfig());
export const applyAtomCodeConfig = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.applyAtomcodeConfig());
export const generateAtomCodeConfig = () =>
  unwrap(bindings.generateAtomcodeConfig());
export const toggleAtomCodeProvider = (): Promise<ToggleResult> =>
  unwrap(bindings.toggleAtomcodeProvider());
export const openAtomCodeConfig = () => unwrap(bindings.openAtomcodeConfig());

export const detectKimiConfig = () => unwrap(bindings.detectKimiConfig());
export const applyKimiConfig = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.applyKimiConfig());
export const generateKimiConfig = () => unwrap(bindings.generateKimiConfig());
export const openKimiConfig = () => unwrap(bindings.openKimiConfig());

export const detectGrokConfig = () => unwrap(bindings.detectGrokConfig());
export const applyGrokConfig = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.applyGrokConfig());
export const generateGrokConfig = () => unwrap(bindings.generateGrokConfig());
export const openGrokConfig = () => unwrap(bindings.openGrokConfig());

export const detectDshConfig = () => unwrap(bindings.detectDshConfig());
export const applyDshConfig = (): Promise<ApplyConfigResult> =>
  unwrap(bindings.applyDshConfig());
export const generateDshConfig = () => unwrap(bindings.generateDshConfig());
export const openDshConfig = () => unwrap(bindings.openDshConfig());

// ── Post-apply process detection ───────────────────────────────

/// Match `client_id` ∈ {codex, claude_code, opencode, gemini, atomcode}.
/// Returns empty list on Windows (no detection yet) — caller should treat
/// empty as "couldn't detect" rather than "definitely not running".
export const detectClientRunning = (clientId: string) =>
  unwrap(bindings.detectClientRunning(clientId));

export const killClientProcess = (clientId: string, pid: number) =>
  unwrap(bindings.killClientProcess(clientId, pid));

/// Restart Codex Desktop so the freshly-written config picks up. macOS only
/// for now; on other platforms returns `supported: false` and the UI hides
/// the button.
export const restartCodexDesktop = () => unwrap(bindings.restartCodexDesktop());
export const codexDesktopAvailable = () => bindings.codexDesktopAvailable();

// ── Client apply history ───────────────────────────────────────

export const listClientApplyHistory = (clientId: string) =>
  unwrap(bindings.listClientApplyHistory(clientId));
/// 曾经 apply 过配置的客户端 id 列表（用于配置漂移判断）。
export const clientsWithApplyHistory = () =>
  unwrap(bindings.clientsWithApplyHistory());

export const listMcpServers = () => unwrap(bindings.listMcpServers());
export const upsertMcpServer = (
  input: import("./bindings").UpsertMcpServerInput
) => unwrap(bindings.upsertMcpServer(input));
export const deleteMcpServer = (client: string, name: string) =>
  unwrap(bindings.deleteMcpServer(client, name));
export const syncMcpServer = (input: import("./bindings").SyncMcpServerInput) =>
  unwrap(bindings.syncMcpServer(input));
export const exportMcpServers = (includeSecrets: boolean) =>
  unwrap(bindings.exportMcpServers(includeSecrets));
export const importMcpServers = (payload: string, targetClients: string[]) =>
  unwrap(bindings.importMcpServers(payload, targetClients));

export const rollbackClientApply = (historyId: string) =>
  unwrap(bindings.rollbackClientApply(historyId));

export const deleteClientApplyHistory = (historyId: string) =>
  unwrap(bindings.deleteClientApplyHistory(historyId));

// narrow union types: Rust 端是 String,bindings 给 string,这里给前端用的窄类型。
// 调用 api 时直接传 union literal,内部 cast 到 string 喂给 bindings。
export type InstructionsScope = "claude_global" | "codex_global";
export type InstructionsApplyMode = "overwrite" | "append";
export type McpValidationStatus = "valid" | "warning" | "invalid" | "unknown";

// ── Global Instructions (CLAUDE.md / AGENTS.md) ────────────────

export const listInstructionsTemplates = () =>
  unwrap(bindings.listInstructionsTemplates());
export const readGlobalInstructions = (scope: InstructionsScope) =>
  unwrap(bindings.readGlobalInstructions(scope));
export const writeGlobalInstructions = (
  scope: InstructionsScope,
  content: string
) => unwrap(bindings.writeGlobalInstructions(scope, content));
export const applyInstructionsTemplate = (
  scope: InstructionsScope,
  templateId: string,
  mode: InstructionsApplyMode
) => unwrap(bindings.applyInstructionsTemplate(scope, templateId, mode));
export const exportInstructions = () => unwrap(bindings.exportInstructions());
export const importInstructions = (payload: string) =>
  unwrap(bindings.importInstructions(payload));

// ── Local Skills (~/.claude/skills) ────────────────────────────

export const listSkills = () => unwrap(bindings.listSkills());
export const setSkillEnabled = (source: string, id: string, enabled: boolean) =>
  unwrap(bindings.setSkillEnabled(source, id, enabled));
export const deleteSkill = (source: string, id: string) =>
  unwrap(bindings.deleteSkill(source, id));
export const importSkillFromZip = (source: string, bytes: number[]) =>
  unwrap(bindings.importSkillFromZip(source, bytes));
export const exportSkills = () => unwrap(bindings.exportSkills());
export const importSkills = (payload: string) =>
  unwrap(bindings.importSkills(payload));

// ── Route Profiles ─────────────────────────────────────────────

export const listRouteProfiles = () => unwrap(bindings.listRouteProfiles());
export const getRouteProfile = (id: string) =>
  unwrap(bindings.getRouteProfile(id));
export const createRouteProfile = (
  input: import("@/types/route-profile").CreateRouteProfileInput
) =>
  unwrap(
    bindings.createRouteProfile(
      toBindingsInput<Bindings.CreateRouteProfileInput>(input)
    )
  );
export const updateRouteProfile = (
  id: string,
  input: import("@/types/route-profile").UpdateRouteProfileInput
) =>
  unwrap(
    bindings.updateRouteProfile(
      id,
      toBindingsInput<Bindings.UpdateRouteProfileInput>(input)
    )
  );
export const deleteRouteProfile = (id: string) =>
  unwrap(bindings.deleteRouteProfile(id));
export const setDefaultRouteProfile = (id: string) =>
  unwrap(bindings.setDefaultRouteProfile(id));
export const setRouteProfileMode = (id: string, mode: string) =>
  unwrap(bindings.setRouteProfileMode(id, mode));
export const setRouteActiveProvider = (
  routeProfileId: string,
  providerId: string
) => unwrap(bindings.setRouteActiveProvider(routeProfileId, providerId));
export const addProviderToRoute = (
  routeProfileId: string,
  providerId: string,
  input: import("@/types/route-profile").AddProviderToRouteInput
) =>
  unwrap(
    bindings.addProviderToRoute(
      routeProfileId,
      providerId,
      toBindingsInput<Bindings.AddProviderToRouteInput>(input)
    )
  );
export const removeProviderFromRoute = (
  routeProfileId: string,
  providerId: string
) => unwrap(bindings.removeProviderFromRoute(routeProfileId, providerId));
export const reorderRouteProviders = (
  routeProfileId: string,
  providerIds: string[]
) => unwrap(bindings.reorderRouteProviders(routeProfileId, providerIds));
export const updateRouteProviderConditions = (
  routeProfileId: string,
  providerId: string,
  routingConditions: string | null
) =>
  unwrap(
    bindings.updateRouteProviderConditions(
      routeProfileId,
      providerId,
      routingConditions
    )
  );
export const listProviderRuntimeStatus = () =>
  unwrap(bindings.listProviderRuntimeStatus());
export const resetProviderRuntimeStatus = (providerId: string) =>
  unwrap(bindings.resetProviderRuntimeStatus(providerId));
export const resetAllProviderRuntimeStatus = () =>
  unwrap(bindings.resetAllProviderRuntimeStatus());
export const previewRouteTemplate = (
  input: import("@/lib/bindings").RouteTemplateInput
) => unwrap(bindings.previewRouteTemplate(input));
export const applyRouteTemplate = (
  input: import("@/lib/bindings").RouteTemplateInput
) => unwrap(bindings.applyRouteTemplate(input));
export const rollbackRouteTemplate = (profileId: string) =>
  unwrap(bindings.rollbackRouteTemplate(profileId));
export const hasRouteTemplateRollback = (profileId: string) =>
  unwrap(bindings.hasRouteTemplateRollback(profileId));

// ── Stats ──────────────────────────────────────────────────────

export const getRequestStats = () => unwrap(bindings.getRequestStats());
/** Stats over a sliding window — used by the Dashboard date-range tabs. */
export const getRequestStatsRange = (days: number) =>
  unwrap(bindings.getRequestStatsRange(days));
/** Live gateway KPIs (active requests, uptime, today success rate). */
export const getRuntimeKpis = () => unwrap(bindings.getRuntimeKpis());
export const getProviderHealth = (provider: string) =>
  unwrap(bindings.getProviderHealth(provider));

// ── Pricing ───────────────────────────────────────────────────

export const listModelPricing = () => unwrap(bindings.listModelPricing());
export const upsertModelPricing = (
  provider: string,
  model_pattern: string,
  input_price: number,
  output_price: number
) =>
  unwrap(
    bindings.upsertModelPricing(
      provider,
      model_pattern,
      input_price,
      output_price
    )
  );
export const deleteModelPricing = (id: string) =>
  unwrap(bindings.deleteModelPricing(id));

// ── Diagnostics ────────────────────────────────────────────────

// CheckItem.status 在 Rust 端是 String,bindings 给 string。前端 @/types/diagnostics
// narrow 成 "ok" | "warning" | "failed" | "skipped" union,这里 boundary cast 一次。
import type {
  CheckReport as NarrowCheckReport,
  FullSelfTestReport as NarrowFullSelfTestReport,
} from "@/types/diagnostics";

export const runHealthCheck = () =>
  unwrap(bindings.runHealthCheck()) as Promise<NarrowCheckReport>;
export const runDatabaseCheck = () =>
  unwrap(bindings.runDatabaseCheck()) as Promise<NarrowCheckReport>;
export const runGatewayAuthCheck = () =>
  unwrap(bindings.runGatewayAuthCheck()) as Promise<NarrowCheckReport>;
export const runProviderCheck = () =>
  unwrap(bindings.runProviderCheck()) as Promise<NarrowCheckReport>;
export const runCodexConfigCheck = () =>
  unwrap(bindings.runCodexConfigCheck()) as Promise<NarrowCheckReport>;
export const runClaudeCodeConfigCheck = () =>
  unwrap(bindings.runClaudeCodeConfigCheck()) as Promise<NarrowCheckReport>;
export const runRouteProfileCheck = () =>
  unwrap(bindings.runRouteProfileCheck()) as Promise<NarrowCheckReport>;
export const runFullSelfTest = () =>
  unwrap(bindings.runFullSelfTest()) as Promise<NarrowFullSelfTestReport>;
export const exportDiagnosticBundle = (
  includeLogs?: boolean,
  maxLogs?: number
) =>
  unwrap(bindings.exportDiagnosticBundle(includeLogs ?? null, maxLogs ?? null));
export const openAppDataDir = () => unwrap(bindings.openAppDataDir());

// ── Tool Connection Test ──────────────────────────────────────

// Backend 返回 serde_json::Value（JsonValue），保留宽 interface 给调用方。
export interface ConnectionTestResult {
  config_ok: boolean;
  gateway_ok: boolean;
  provider_ok: boolean;
  /** 第四步：已接入客户端是否有进程在跑（null = 未检测 / 无已接入客户端）。 */
  client_process_ok?: boolean | null;
  /** 各已接入客户端的进程摘要，供 UI 展示。 */
  client_processes?: Array<{
    client_id: string;
    running: boolean;
    count: number;
  }>;
  test_model?: string;
  error?: string;
}

export const testToolConnection = (): Promise<ConnectionTestResult> =>
  unwrap(
    bindings.testToolConnection()
  ) as unknown as Promise<ConnectionTestResult>;

// ── Pet ───────────────────────────────────────────────────────

// PetSettings.pet_type 在 bindings 里是 string,前端 narrow 成 PetType union——
// 边界 cast 一次保留前端窄类型 + DB 字符串运行时灵活的折中。
import type { PetSettings as PetSettingsWide } from "./bindings";

type PetSettingsNarrow = Omit<PetSettingsWide, "pet_type"> & {
  pet_type: PetType;
};

export const getPetSettings = async (): Promise<PetSettingsNarrow> =>
  (await unwrap(bindings.getPetSettings())) as PetSettingsNarrow;

export const updatePetSettings = async (
  input: import("@/types/pet").UpdatePetSettingsInput
): Promise<PetSettingsNarrow> =>
  (await unwrap(
    bindings.updatePetSettings(
      toBindingsInput<Bindings.UpdatePetSettingsInput>(input)
    )
  )) as PetSettingsNarrow;

export const setPetVisible = async (
  visible: boolean
): Promise<PetSettingsNarrow> =>
  (await unwrap(bindings.setPetVisible(visible))) as PetSettingsNarrow;

export const getPetGatewayState = (): Promise<PetGatewayInfo> =>
  unwrap(bindings.getPetGatewayState()) as unknown as Promise<PetGatewayInfo>;
/// 轻量版:只 state + last_error(走索引)。10s 轮询用。
export const getPetGatewayStateLite = (): Promise<
  Pick<PetGatewayInfo, "state" | "last_error" | "active_count">
> =>
  unwrap(bindings.getPetGatewayStateLite()) as unknown as Promise<
    Pick<PetGatewayInfo, "state" | "last_error" | "active_count">
  >;
export const getPetMemory = () => unwrap(bindings.getPetMemory());
export const savePetMemory = (memory: string) =>
  unwrap(bindings.savePetMemory(memory));
export const getPetChatHistory = () => unwrap(bindings.getPetChatHistory());
/// 保存聊天历史(Rust 侧封顶 + 广播 PetChatUpdated),返回封顶后的历史 JSON。
export const savePetChatHistory = (history: string) =>
  unwrap(bindings.savePetChatHistory(history));
export const petChat = (messages: Array<{ role: string; content: string }>) =>
  unwrap(bindings.petChat(messages));
export const petOpenSettings = () => unwrap(bindings.petOpenSettings());
export const getPetClickThrough = () => unwrap(bindings.getPetClickThrough());
export const setPetClickThrough = (value: boolean) =>
  unwrap(bindings.setPetClickThrough(value));
export const showPetContextMenu = () => unwrap(bindings.showPetContextMenu());

// ── Config Import / Export ─────────────────────────────────────

export const exportConfigJson = (includeSecrets: boolean) =>
  unwrap(bindings.exportConfigJson(includeSecrets));
export const importConfigJson = (json: string) =>
  unwrap(bindings.importConfigJson(json));

// re-export narrow union types for downstream consumers
export type { PetType, PetState, PetGatewayInfo };
