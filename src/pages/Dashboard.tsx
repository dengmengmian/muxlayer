import {
  useState,
  useEffect,
  useCallback,
  useRef,
  type ReactNode,
} from "react";
import {
  Radio,
  Play,
  Square,
  RotateCcw,
  BarChart3,
  Rocket,
  ArrowRight,
  Coins,
} from "lucide-react";
import { Link } from "react-router-dom";
import { RecentRequests } from "@/components/dashboard/RecentRequests";
import { RuntimeFooter } from "@/components/common/RuntimeFooter";
import { StatusBadge } from "@/components/common/StatusBadge";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import { usePolling } from "@/lib/usePolling";
import { formatCost, formatLatency } from "@/lib/utils";
import {
  cacheHitRatePercent,
  estimateCacheSavingsUsd,
} from "@/lib/requestLogDebug";
import { firstRequestCommandsFor } from "@/lib/firstRequestCommands";
import * as api from "@/lib/api";
import {
  useProviders,
  useRouteProfiles,
  useGatewayStatus,
} from "@/store/global";
import type { ToolConfigView } from "@/types/tool";
import type { RequestLogListItem, CostBreakdown } from "@/types/request-log";
import type { RequestStats } from "@/types/stats";
import { CopyButton } from "@/components/common/CopyButton";

/// 极简 deep equal：JSON 字符串化对比。dashboard 数据 payload 不大
/// （几个 KB），常数时间。避免 5 秒轮询每次都触发 React 重渲让数字
/// 闪烁、按钮 hover 状态丢失。
function shallowEqual<T>(a: T, b: T): boolean {
  if (a === b) return true;
  if (a === null || b === null) return false;
  try {
    return JSON.stringify(a) === JSON.stringify(b);
  } catch {
    return false;
  }
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

// 成本分解小列表：每行 名称 + 占比条 + 请求数 + 成本，按成本倒序（后端已排）。
function CostList({ title, rows }: { title: string; rows: CostBreakdown[] }) {
  const { t } = useI18n();
  const max = rows.reduce((m, r) => Math.max(m, r.cost), 0) || 1;
  return (
    <div>
      <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-text-secondary">
        {title}
      </div>
      {rows.length === 0 ? (
        <p className="text-xs text-text-muted">—</p>
      ) : (
        <div className="space-y-1.5">
          {rows.map((r) => (
            <div key={r.key} className="flex items-center gap-2 text-xs">
              <span
                className="w-28 shrink-0 truncate font-mono text-text-primary"
                title={r.key}
              >
                {r.key}
              </span>
              <div className="relative h-1.5 flex-1 overflow-hidden rounded-full bg-card-secondary">
                <div
                  className="absolute inset-y-0 left-0 rounded-full bg-accent/60"
                  style={{ width: `${(r.cost / max) * 100}%` }}
                />
              </div>
              <span className="shrink-0 tabular-nums text-text-muted">
                {r.request_count}
              </span>
              {r.has_price ? (
                <span className="w-16 shrink-0 text-right font-mono tabular-nums text-text-primary">
                  {formatCost(r.cost)}
                </span>
              ) : (
                <Link
                  to="/settings?tab=data"
                  className="w-16 shrink-0 text-right text-xs text-accent hover:underline"
                  title={t("stats.no_price_tip")}
                  onClick={(e) => e.stopPropagation()}
                >
                  {t("stats.no_price_set")}
                </Link>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// Single metric in a horizontal strip: label above, value below, no card chrome.
function StripMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: "default" | "error" | "accent";
}) {
  const valueColor =
    tone === "error"
      ? "text-error"
      : tone === "accent"
        ? "text-accent"
        : "text-text-primary";
  return (
    <div className="flex flex-col">
      <span className="text-xs uppercase tracking-wide text-text-muted">
        {label}
      </span>
      <span className={`text-base font-semibold ${valueColor} tabular-nums`}>
        {value}
      </span>
    </div>
  );
}

function OnboardingPrompt({
  icon,
  title,
  desc,
  action,
}: {
  icon: ReactNode;
  title: string;
  desc: string;
  action: ReactNode;
}) {
  return (
    <div className="rounded-xl border-2 border-dashed border-accent/30 bg-accent-soft/30 p-6">
      <div className="flex items-start gap-4">
        <div className="flex h-12 w-12 shrink-0 items-center justify-center rounded-lg bg-accent text-on-accent">
          {icon}
        </div>
        <div className="min-w-0 flex-1">
          <h3 className="text-base font-semibold text-text-primary">{title}</h3>
          <p className="mt-1 text-sm text-text-secondary">{desc}</p>
          <div className="mt-3">{action}</div>
        </div>
      </div>
    </div>
  );
}

/// 命中率 = cache_read / (cache_read + cache_write + 非缓存输入)。
/// 三项分母对 Anthropic 三段式（read / write / non-cached input 各自独立）天然正确；
/// OpenAI 把 cached_tokens 计入 input_tokens 会让分母略偏大，但永远落在 [0, 100]，
/// 不会出现 >100% 这种用户看了懵的情况。颜色阈值参照"系统提示稳定时的健康面"：
/// ≥70% 绿，30-70% 黄（可能在切换 prompt / 上游 TTL 到期），<30% 红。
function CacheHitBadge({
  cacheRead,
  cacheWrite,
  inputTokens,
}: {
  cacheRead: number;
  cacheWrite: number;
  inputTokens: number;
}) {
  const rate = cacheHitRatePercent(cacheRead, cacheWrite, inputTokens);
  if (rate == null) {
    return <StatusBadge variant="muted">—</StatusBadge>;
  }
  const variant = rate >= 70 ? "success" : rate >= 30 ? "warning" : "error";
  return <StatusBadge variant={variant}>{rate.toFixed(1)}%</StatusBadge>;
}

type RangeDays = 1 | 7 | 14 | 30;
const RANGE_OPTIONS: { days: RangeDays; labelZh: string; labelEn: string }[] = [
  { days: 1, labelZh: "今天", labelEn: "Today" },
  { days: 7, labelZh: "7天", labelEn: "7d" },
  { days: 14, labelZh: "14天", labelEn: "14d" },
  { days: 30, labelZh: "30天", labelEn: "30d" },
];

export function Dashboard() {
  const { t, locale } = useI18n();
  // gateway status 走全局 store——Topbar 已经在轮询，这里只订阅。
  const status = useGatewayStatus((s) => s.value);
  const providers = useProviders((s) => s.items);
  const [tools, setTools] = useState<ToolConfigView[]>([]);
  const [recentLogs, setRecentLogs] = useState<RequestLogListItem[]>([]);
  const [stats, setStats] = useState<RequestStats | null>(null);
  const [providerCount, setProviderCount] = useState<number | null>(null);
  /// Clients actually wired to MuxLayer (has_agentgate), not merely config_exists.
  const [wiredClientIds, setWiredClientIds] = useState<string[]>([]);
  const [costByModel, setCostByModel] = useState<CostBreakdown[]>([]);
  const [costByClient, setCostByClient] = useState<CostBreakdown[]>([]);
  const [costByStrategy, setCostByStrategy] = useState<CostBreakdown[]>([]);
  const [rangeDays, setRangeDays] = useState<RangeDays>(7);

  // 快速切换时间范围时，旧范围的慢请求可能后返回；序号守卫只让最新一次落盘。
  const liveSeqRef = useRef(0);
  // 进行中的 loadLive 数量：轮询 tick 遇到进行中的请求直接跳过，避免大库 /
  // 长范围聚合慢于轮询周期时，每个 tick 都让上一次结果作废、面板永远不更新。
  const liveInFlightRef = useRef(0);
  const loadLive = useCallback(async () => {
    const seq = ++liveSeqRef.current;
    liveInFlightRef.current += 1;
    try {
      const [l, st, cm, cc, rs] = await Promise.all([
        api.listRequestLogs({ limit: 5 }),
        api.getRequestStatsRange(rangeDays),
        api.aggregateCostByModel(rangeDays, 8),
        api.aggregateCostByClient(rangeDays, 8),
        api.aggregateRouteProfileStats(rangeDays).catch(() => []),
      ]);
      if (seq !== liveSeqRef.current) return;
      const rp = useRouteProfiles.getState().items;
      const nameMap = Object.fromEntries(rp.map((p) => [p.id, p.name]));
      const byStrategy: CostBreakdown[] = rs
        .map((x) => ({
          key: nameMap[x.route_profile_id] ?? x.route_profile_id,
          provider: null,
          request_count: x.request_count,
          input_tokens: 0,
          output_tokens: 0,
          cache_read_tokens: 0,
          cache_write_tokens: 0,
          cost: x.cost,
          has_price: true,
        }))
        .filter((x) => x.request_count > 0)
        .sort((a, b) => b.cost - a.cost);
      const lifetimeTotal = st.total;
      if (
        lifetimeTotal >= 1 &&
        localStorage.getItem("agentgate_first_req_seen") !== "1"
      ) {
        localStorage.setItem("agentgate_first_req_seen", "1");
        toast("success", t("dashboard.first_request_seen"));
      }

      setRecentLogs((prev) => (shallowEqual(prev, l) ? prev : l));
      setStats((prev) => (shallowEqual(prev, st) ? prev : st));
      setCostByModel((prev) => (shallowEqual(prev, cm) ? prev : cm));
      setCostByClient((prev) => (shallowEqual(prev, cc) ? prev : cc));
      setCostByStrategy((prev) =>
        shallowEqual(prev, byStrategy) ? prev : byStrategy
      );
    } catch (err) {
      if (seq !== liveSeqRef.current) return;
      toast("error", (err as api.AppError).message);
    } finally {
      liveInFlightRef.current -= 1;
    }
  }, [rangeDays, t]);
  const pollLive = useCallback(() => {
    if (liveInFlightRef.current > 0) return;
    loadLive();
  }, [loadLive]);

  const loadClients = useCallback(async () => {
    try {
      const [tl, codex, claude, opencode, gemini, atom, kimi, grok, dsh] =
        await Promise.all([
          api.listTools(),
          api.detectCodexConfig().catch(() => null),
          api.detectClaudeCodeEnv().catch(() => null),
          api.detectOpenCodeConfig().catch(() => null),
          api.detectGeminiConfig().catch(() => null),
          api.detectAtomCodeConfig().catch(() => null),
          api.detectKimiConfig().catch(() => null),
          api.detectGrokConfig().catch(() => null),
          api.detectDshConfig().catch(() => null),
          useProviders.getState().refetch(),
          useRouteProfiles
            .getState()
            .refetch()
            .catch(() => {}),
        ]);
      const ps = useProviders.getState().items;
      const wired: string[] = [];
      if (codex?.has_agentgate) wired.push("codex");
      if (claude?.has_agentgate) wired.push("claude_code");
      if (opencode?.has_agentgate) wired.push("opencode");
      if (gemini?.has_agentgate) wired.push("gemini");
      if (atom?.has_agentgate) wired.push("atomcode");
      if (kimi?.has_agentgate) wired.push("kimi_cli");
      if (grok?.has_agentgate) wired.push("grok_build");
      if (dsh?.has_agentgate) wired.push("deepseek_harness");
      setTools((prev) => (shallowEqual(prev, tl) ? prev : tl));
      setWiredClientIds((prev) => (shallowEqual(prev, wired) ? prev : wired));
      setProviderCount((prev) => (prev === ps.length ? prev : ps.length));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  }, []);

  useEffect(() => {
    loadLive();
  }, [loadLive]);
  useEffect(() => {
    loadClients();
    useGatewayStatus.getState().fetch();
  }, [loadClients]);
  usePolling(pollLive, 5000);
  // 客户端配置探测要读 8 个配置文件，变化频率低；usePolling 在窗口重新聚焦时
  // 会立即补一次，所以周期放宽到 60s。
  usePolling(loadClients, 60_000);

  // 命令返回最新状态，直接写入 store——Topbar 徽章同步更新，无需等下个轮询。
  const setStatus = useGatewayStatus.getState().setValue;
  const handleStart = async () => {
    try {
      setStatus(await api.startGateway());
      toast("success", t("gateway.started"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };
  const handleStop = async () => {
    try {
      setStatus(await api.stopGateway());
      toast("success", t("gateway.stopped"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };
  const handleRestart = async () => {
    try {
      setStatus(await api.restartGateway());
      toast("success", t("gateway.restarted"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  if (!status)
    return (
      <div className="space-y-4">
        <div className="skeleton h-12 w-full" />
        <div className="skeleton h-16 w-full" />
        <div className="skeleton h-40 w-full" />
      </div>
    );

  const todayTokens = stats
    ? stats.today_input_tokens + stats.today_output_tokens
    : 0;
  const todayCacheHitRate = stats
    ? cacheHitRatePercent(
        stats.today_cache_read_tokens,
        stats.today_cache_write_tokens,
        stats.today_input_tokens
      )
    : null;
  // 种子供应商（空 key）不算就绪：要能真正发请求。
  const hasUsableProvider = providers.some(
    (p) => p.enabled !== false && !!p.masked_api_key
  );
  // 兼容旧逻辑：provider 列表尚未加载完时不闪「空态」
  const providersKnown = providerCount !== null;
  const hasWiredClient = wiredClientIds.length > 0;
  const hasProviders = providersKnown && (providerCount ?? 0) > 0;
  const readyCommands = firstRequestCommandsFor(wiredClientIds);

  return (
    <div className="space-y-4">
      {/* ── 1. Gateway strip — active provider, protocol chain, controls.
              host:port + running badge live in the global Topbar; we don't
              repeat them here. ── */}
      <div
        className="relative overflow-hidden rounded-lg border border-accent/25 bg-card px-5 py-3 shadow-sm"
        style={{
          background:
            "linear-gradient(135deg, var(--color-card) 0%, var(--color-accent-soft) 100%)",
        }}
      >
        <div className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-accent/50 to-transparent" />
        <div className="flex items-center justify-between gap-4">
          <div className="flex min-w-0 items-center gap-3">
            <div className="relative flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-accent-soft">
              <span
                role="status"
                aria-label={
                  status.running ? t("topbar.running") : t("topbar.stopped")
                }
                className={`absolute -right-0.5 -top-0.5 h-2.5 w-2.5 rounded-full ${
                  status.running ? "bg-info animate-pulse-dot" : "bg-text-muted"
                }`}
              />
              <Radio className="h-4 w-4 text-accent" />
            </div>
            <div className="min-w-0">
              <span className="text-xs font-semibold uppercase tracking-[0.16em] text-accent">
                {t("dashboard.control_console")}
              </span>
              <div className="mt-1 flex min-w-0 items-baseline gap-3">
                <span className="text-sm font-semibold text-text-primary">
                  {t("dashboard.gateway")}
                </span>
                <span className="truncate text-xs text-text-secondary">
                  {status.active_provider ?? t("common.none")}
                </span>
                <span className="hidden text-text-muted/40 md:inline">·</span>
                <span className="hidden truncate font-mono text-xs text-text-muted md:inline">
                  {status.input_protocol} → {status.output_protocol}
                </span>
              </div>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {status.running ? (
              <>
                <button
                  onClick={handleStop}
                  className="flex items-center gap-1 rounded-md bg-error-soft px-2.5 py-1 text-xs font-medium text-error transition-colors hover:bg-error/20"
                >
                  <Square className="h-3 w-3" />
                  {t("dashboard.stop")}
                </button>
                <button
                  onClick={handleRestart}
                  className="flex items-center gap-1 rounded-md bg-warning-soft px-2.5 py-1 text-xs font-medium text-warning transition-colors hover:bg-warning/20"
                >
                  <RotateCcw className="h-3 w-3" />
                  {t("dashboard.restart")}
                </button>
              </>
            ) : (
              <button
                onClick={handleStart}
                className="flex items-center gap-1 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90"
              >
                <Play className="h-3 w-3" />
                {t("dashboard.start")}
              </button>
            )}
          </div>
        </div>
      </div>

      {/* ── 分层新手引导（互斥，自上而下只显示一层）：
            1) 无可发请求的供应商（无 key 的 seed 也算）→ 快速配置
            2) 有 key 但网关未开 → 启网关
            3) 有 key + 网关开，但没有任何 has_agentgate 客户端 → 去 apply
            4) 已 apply + 尚无网关请求 → 只展示已接入客户端的启动命令
          禁止用 config_exists（本机有配置文件）冒充已接入。 ── */}
      {providersKnown && !hasUsableProvider && (
        <OnboardingPrompt
          icon={<Rocket className="h-6 w-6" />}
          title={t("dashboard.empty_title")}
          desc={t("dashboard.empty_desc")}
          action={
            <Link
              to="/quick-setup"
              className="inline-flex items-center gap-1.5 text-sm font-medium text-accent transition-all hover:gap-2"
            >
              {t("dashboard.empty_cta")}
              <ArrowRight className="h-4 w-4" />
            </Link>
          }
        />
      )}

      {hasUsableProvider && !status.running && (
        <OnboardingPrompt
          icon={<Play className="h-6 w-6" />}
          title={t("dashboard.gateway_empty_title")}
          desc={t("dashboard.gateway_empty_desc")}
          action={
            <button
              onClick={handleStart}
              className="inline-flex items-center gap-1.5 text-sm font-medium text-accent transition-all hover:gap-2"
            >
              {t("dashboard.gateway_empty_cta")}
              <ArrowRight className="h-4 w-4" />
            </button>
          }
        />
      )}

      {hasUsableProvider &&
        status.running &&
        stats &&
        stats.total === 0 &&
        !hasWiredClient && (
          <OnboardingPrompt
            icon={<Rocket className="h-6 w-6" />}
            title={t("dashboard.no_requests_config_title")}
            desc={t("dashboard.no_requests_config_desc")}
            action={
              <Link
                to="/tools"
                className="inline-flex items-center gap-1.5 text-sm font-medium text-accent transition-all hover:gap-2"
              >
                {t("dashboard.no_requests_config_cta")}
                <ArrowRight className="h-4 w-4" />
              </Link>
            }
          />
        )}

      {hasUsableProvider &&
        status.running &&
        stats &&
        stats.total === 0 &&
        hasWiredClient && (
          <OnboardingPrompt
            icon={<Rocket className="h-6 w-6" />}
            title={t("dashboard.no_requests_ready_title")}
            desc={t("dashboard.no_requests_ready_desc")}
            action={
              <div className="space-y-3">
                {readyCommands.length > 0 && (
                  <ul className="space-y-2">
                    {readyCommands.map((cmd) => (
                      <li
                        key={cmd.command}
                        className="flex max-w-md items-center justify-between gap-2 rounded-md border border-border bg-card px-3 py-2"
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
                <div className="flex flex-wrap items-center gap-3">
                  <Link
                    to="/tools"
                    className="inline-flex items-center gap-1.5 text-sm font-medium text-accent transition-all hover:gap-2"
                  >
                    {t("dashboard.no_requests_ready_cta")}
                    <ArrowRight className="h-4 w-4" />
                  </Link>
                  <Link
                    to="/logs"
                    className="text-xs text-text-muted hover:text-text-primary"
                  >
                    {t("dashboard.no_requests_ready_logs")}
                  </Link>
                </div>
              </div>
            }
          />
        )}

      {/* ── 2. Today card — 6 primary metrics + cache inline footer when present ── */}
      {hasProviders && stats && stats.total > 0 && (
        <div
          className="relative overflow-hidden border-y border-border bg-card/45 px-5 py-4"
          style={{
            background:
              "linear-gradient(90deg, var(--color-accent-soft) 0%, transparent 45%)",
          }}
        >
          <div className="pointer-events-none absolute inset-x-6 top-0 h-px bg-gradient-to-r from-accent/0 via-accent/45 to-accent/0" />
          <div className="mb-3 flex items-center justify-between">
            <div>
              <span className="text-xs font-semibold uppercase tracking-wide text-text-secondary">
                {t("stats.today_realtime")}
              </span>
              <p className="mt-0.5 text-xs text-text-muted">
                {t("stats.realtime") || "实时刷新"}
              </p>
            </div>
            <span className="rounded-full border border-success/20 bg-success/10 px-2 py-0.5 text-xs font-medium text-success">
              LIVE
            </span>
          </div>
          <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
            <StripMetric
              label={t("stats.requests")}
              value={String(stats.today_total)}
            />
            <StripMetric
              label={t("stats.errors_label")}
              value={String(stats.today_errors)}
              tone={stats.today_errors > 0 ? "error" : "default"}
            />
            <StripMetric
              label={t("stats.tokens_today") || "Tokens"}
              value={formatTokens(todayTokens)}
            />
            <StripMetric
              label={t("stats.cost_today") || "今日费用"}
              value={formatCost(stats.today_cost)}
            />
            <StripMetric
              label={t("stats.avg_latency")}
              value={formatLatency(stats.avg_latency_ms)}
            />
            <StripMetric
              label={t("stats.hit_rate")}
              value={
                todayCacheHitRate == null
                  ? "—"
                  : `${todayCacheHitRate.toFixed(1)}%`
              }
              tone={
                todayCacheHitRate != null && todayCacheHitRate >= 70
                  ? "accent"
                  : "default"
              }
            />
          </div>
          {stats.today_codex_compact > 0 && (
            <div className="mt-3 flex items-center gap-2 border-t border-border pt-3 text-xs text-text-muted">
              <span className="font-medium text-text-secondary">
                {t("stats.codex_compact") || "Codex 压缩"}
              </span>
              <span>
                {t("stats.codex_compact_today") || "今日触发"}{" "}
                <span className="font-mono text-text-primary">
                  {stats.today_codex_compact}
                </span>{" "}
                {t("stats.codex_compact_times") || "次"}
              </span>
              <span className="ml-auto text-text-muted/70">
                {t("stats.codex_compact_hint") || "本地代替 OpenAI 远程压缩"}
              </span>
            </div>
          )}
          {(stats.today_cache_read_tokens > 0 ||
            stats.today_cache_write_tokens > 0) && (
            <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-1 border-t border-border pt-3 text-xs text-text-muted">
              <span className="font-medium text-text-secondary">
                {t("stats.cache")}
              </span>
              <span>
                {t("stats.cache_write")}{" "}
                <span className="font-mono text-text-primary">
                  {formatTokens(stats.today_cache_write_tokens)}
                </span>
              </span>
              <span className="text-text-muted/40">·</span>
              <span>
                {t("stats.cache_hit")}{" "}
                <span className="font-mono text-text-primary">
                  {formatTokens(stats.today_cache_read_tokens)}
                </span>
              </span>
              <span className="text-text-muted/40">·</span>
              <span>
                {t("stats.input_total")}{" "}
                <span className="font-mono text-text-primary">
                  {formatTokens(stats.today_input_tokens)}
                </span>
              </span>
              <span className="ml-auto flex items-center gap-1.5">
                {t("stats.hit_rate")}
                <CacheHitBadge
                  cacheRead={stats.today_cache_read_tokens}
                  cacheWrite={stats.today_cache_write_tokens}
                  inputTokens={stats.today_input_tokens}
                />
              </span>
              {/* Dollar savings only when real cache_read exists; use avg priced model input if any. */}
              {stats.today_cache_read_tokens > 0 &&
                (() => {
                  const priced = costByModel.find(
                    (m) => m.has_price && m.cost > 0
                  );
                  // Without a reliable unit price, show share only (no invented $).
                  if (!priced) return null;
                  // Derive rough $/1M from cost / non-cache-adjusted tokens when possible.
                  const unit =
                    priced.input_tokens > 0
                      ? (priced.cost /
                          (priced.input_tokens + priced.output_tokens)) *
                        1_000_000
                      : null;
                  const save = estimateCacheSavingsUsd(
                    stats.today_cache_read_tokens,
                    unit
                  );
                  if (save == null || save <= 0) return null;
                  return (
                    <span
                      className="w-full text-xs text-success"
                      title={t("stats.cache_savings_tip")}
                    >
                      {t("stats.cache_savings")}: ~${save.toFixed(4)}
                    </span>
                  );
                })()}
            </div>
          )}
        </div>
      )}

      {/* ── 3. Trend chart + Top providers in one card. Range tabs live in the
              chart header (they only affect the chart, not today's strip). ── */}
      {hasProviders && stats && stats.total > 0 && (
        <>
          <div className="rounded-lg border border-border/80 bg-card p-5">
            <div className="mb-4 flex items-center justify-between gap-2">
              <h3 className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                <span className="flex h-7 w-7 items-center justify-center rounded-md bg-card-secondary">
                  <BarChart3 className="h-4 w-4 text-accent" />
                </span>
                <span>
                  {t("stats.traffic_monitor")}
                  <span className="ml-2 text-text-muted">
                    ·{" "}
                    {rangeDays === 1
                      ? t("stats.range_today")
                      : `${rangeDays} ${t("stats.days_suffix")}`}
                  </span>
                </span>
              </h3>
              <div className="flex items-center gap-3">
                <div className="hidden items-center gap-3 text-xs text-text-muted sm:flex">
                  <div className="flex items-center gap-1">
                    <div className="h-2 w-2 rounded-sm bg-accent/70" />
                    <span>{t("stats.success_rate_label") || "成功"}</span>
                  </div>
                  <div className="flex items-center gap-1">
                    <div className="h-2 w-2 rounded-sm bg-error/60" />
                    <span>{t("stats.errors_label")}</span>
                  </div>
                </div>
                <div className="flex items-center gap-0.5 rounded-md bg-card-secondary p-0.5">
                  {RANGE_OPTIONS.map((opt) => (
                    <button
                      key={opt.days}
                      onClick={() => setRangeDays(opt.days)}
                      className={`rounded px-2.5 py-0.5 text-xs font-medium transition-colors ${
                        rangeDays === opt.days
                          ? "bg-accent text-on-accent"
                          : "text-text-secondary hover:text-accent"
                      }`}
                    >
                      {locale === "zh" ? opt.labelZh : opt.labelEn}
                    </button>
                  ))}
                </div>
              </div>
            </div>
            {(() => {
              const BAR_H = 110; // px
              const maxReq = Math.max(...stats.daily.map((x) => x.total), 1);
              // Pick a clean tick value at the top (round up to nearest "nice" number).
              const niceMax = (() => {
                const orders = [
                  1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000,
                ];
                return orders.find((o) => o >= maxReq) ?? maxReq;
              })();
              const Y_AXIS_W = 36; // px reserved for y-axis tick labels
              return (
                <div className="relative" style={{ paddingLeft: Y_AXIS_W }}>
                  {/* Y-axis tick labels, aligned with grid lines */}
                  <div
                    className="pointer-events-none absolute left-0 top-0 flex flex-col justify-between text-right text-xs font-mono text-text-muted"
                    style={{ height: BAR_H, width: Y_AXIS_W - 6 }}
                  >
                    <span className="-translate-y-1/2">
                      {niceMax.toLocaleString()}
                    </span>
                    <span className="-translate-y-1/2">
                      {Math.round(niceMax / 2).toLocaleString()}
                    </span>
                    <span className="-translate-y-1/2">0</span>
                  </div>
                  {/* Horizontal grid lines */}
                  <div
                    className="pointer-events-none absolute inset-y-0 right-0"
                    style={{ left: Y_AXIS_W, height: BAR_H }}
                  >
                    <div className="h-px w-full bg-border/40" />
                    <div className="absolute top-1/2 h-px w-full bg-border/30" />
                    <div className="absolute bottom-0 h-px w-full bg-border/40" />
                  </div>
                  {/* Density tuning: gap shrinks + bar caps shrink as bar
                      count grows so 30-day view doesn't overflow. */}
                  <div
                    className={`relative flex items-end justify-between px-1 ${stats.daily.length > 20 ? "gap-1" : stats.daily.length > 10 ? "gap-2" : "gap-3"}`}
                    style={{ height: BAR_H }}
                  >
                    {stats.daily.map((d) => {
                      const successCount = Math.max(d.total - d.errors, 0);
                      const totalH =
                        d.total > 0
                          ? Math.max((d.total / niceMax) * BAR_H, 2)
                          : 0;
                      const errH =
                        d.errors > 0 && totalH > 0
                          ? Math.max((d.errors / d.total) * totalH, 2)
                          : 0;
                      const tooltip = `${d.date}\n${t("stats.tooltip_requests")}: ${d.total} (${t("stats.success_rate_label")} ${successCount} / ${t("stats.tooltip_errors")} ${d.errors})\nTokens: in ${formatTokens(d.input_tokens)} · out ${formatTokens(d.output_tokens)}`;
                      return (
                        <div
                          key={d.date}
                          className="group relative flex flex-1 items-end justify-center"
                          style={{ height: BAR_H }}
                          title={tooltip}
                        >
                          {/* Bar */}
                          <div
                            className="flex w-full flex-col items-center justify-end overflow-hidden rounded-md transition-opacity group-hover:opacity-80"
                            style={{
                              maxWidth:
                                stats.daily.length > 20
                                  ? 18
                                  : stats.daily.length > 10
                                    ? 24
                                    : 32,
                            }}
                          >
                            {totalH > 0 ? (
                              <>
                                {errH > 0 && (
                                  <div
                                    className="w-full bg-error/65"
                                    style={{ height: errH }}
                                  />
                                )}
                                <div
                                  className="w-full bg-accent/70"
                                  style={{ height: totalH - errH }}
                                />
                              </>
                            ) : (
                              // Empty day — show a faint baseline so the column is visible.
                              <div
                                className="w-full bg-border/40"
                                style={{ height: 2 }}
                              />
                            )}
                          </div>
                          {/* Hover total badge */}
                          {d.total > 0 && (
                            <div
                              className="pointer-events-none absolute opacity-0 transition-opacity group-hover:opacity-100"
                              style={{ bottom: totalH + 4 }}
                            >
                              <span className="rounded bg-text-primary px-1.5 py-0.5 font-mono text-xs text-card whitespace-nowrap">
                                {d.total}
                              </span>
                            </div>
                          )}
                        </div>
                      );
                    })}
                  </div>
                  {/* X-axis labels: date + counts row, aligned with bars (already
                      indented by parent padding-left). */}
                  <div
                    className={`mt-2 flex items-start justify-between px-1 ${stats.daily.length > 20 ? "gap-1" : stats.daily.length > 10 ? "gap-2" : "gap-3"}`}
                  >
                    {stats.daily.map((d, i) => {
                      const tokTotal = d.input_tokens + d.output_tokens;
                      const n = stats.daily.length;
                      const stride = n > 20 ? 3 : n > 10 ? 2 : 1;
                      const showDate = i % stride === 0 || i === n - 1;
                      const showTokens = n <= 14;
                      return (
                        <div
                          key={d.date}
                          className="flex flex-1 flex-col items-center gap-0.5"
                        >
                          <span className="text-xs text-text-muted">
                            {showDate ? d.date.slice(5) : ""}
                          </span>
                          <span className="font-mono text-xs font-medium text-text-primary tabular-nums">
                            {d.total > 0 ? d.total.toLocaleString() : "—"}
                          </span>
                          {showTokens && (
                            <span className="font-mono text-[9px] text-text-muted tabular-nums">
                              {tokTotal > 0 ? formatTokens(tokTotal) : " "}
                            </span>
                          )}
                        </div>
                      );
                    })}
                  </div>
                </div>
              );
            })()}
            {/* Top providers — inline strip under the chart in the same card. */}
            {(() => {
              const visible = stats.providers.filter(
                (p) => p.name !== "unknown"
              );
              if (visible.length === 0) return null;
              return (
                <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2 border-t border-border pt-3 text-xs">
                  <span className="font-medium text-text-secondary">
                    {t("stats.top_providers")}
                  </span>
                  {visible.slice(0, 6).map((p, i) => (
                    <div key={p.name} className="flex items-center gap-1">
                      {i > 0 && <span className="text-text-muted/40">·</span>}
                      <span className="text-text-primary">{p.name}</span>
                      <span className="font-mono text-text-muted">
                        {p.count.toLocaleString()}
                      </span>
                    </div>
                  ))}
                </div>
              );
            })()}
          </div>
        </>
      )}

      {/* ── 3.5 成本分解：钱花在哪个模型 / 哪个客户端。仅有数据时显示。 ── */}
      {(costByModel.length > 0 || costByClient.length > 0) && (
        <div className="rounded-lg border border-border/80 bg-card p-5">
          <h3 className="mb-4 flex items-center gap-2 text-sm font-semibold text-text-primary">
            <span className="flex h-7 w-7 items-center justify-center rounded-md bg-card-secondary">
              <Coins className="h-4 w-4 text-accent" />
            </span>
            {t("stats.analytics_panel")}
          </h3>
          <div
            className={`grid gap-6 sm:grid-cols-2${costByStrategy.length > 0 ? " lg:grid-cols-3" : ""}`}
          >
            <CostList title={t("stats.cost_by_model")} rows={costByModel} />
            <CostList title={t("stats.cost_by_client")} rows={costByClient} />
            {costByStrategy.length > 0 && (
              <CostList
                title={t("stats.cost_by_strategy")}
                rows={costByStrategy}
              />
            )}
          </div>
        </div>
      )}

      {/* ── 4. Recent requests with inline tool status header chip. ── */}
      <RecentRequests requests={recentLogs} tools={tools} />

      {/* ── 7. Runtime KPI footer ── */}
      <RuntimeFooter />
    </div>
  );
}
