import { useState, useEffect, useCallback, useRef } from "react";
import { useSearchParams } from "react-router-dom";
import {
  ScrollText,
  RefreshCcw,
  ChevronLeft,
  ChevronRight,
  LayoutList,
  Layers,
  Download,
} from "lucide-react";
import {
  RequestLogTable,
  sourceLabel,
} from "@/components/logs/RequestLogTable";
import { RequestDetailDrawer } from "@/components/logs/RequestDetailDrawer";
import { SessionGroupView } from "@/components/logs/SessionGroupView";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { EmptyState } from "@/components/common/EmptyState";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import { useDebouncedValue } from "@/lib/useDebouncedValue";
import { formatOptionalLatency } from "@/lib/utils";
import * as api from "@/lib/api";
import { useProviders, useRouteProfiles } from "@/store/global";
import type { RequestLogListItem } from "@/types/request-log";
import type { RequestLogDetail } from "@/types/request-log";

// 客户端候选——detect_client_from_ua 用的固定列表。
const KNOWN_CLIENTS = [
  "Codex",
  "Claude Code",
  "OpenCode",
  "Gemini CLI",
  "AtomCode",
  "Generic",
];

const PAGE_SIZE = 100;
const VALID_SOURCE_FILTERS = new Set([
  "gateway",
  "session_log",
  "claude_session",
  "codex_session",
  "gemini_session",
]);

export function Logs() {
  const { t } = useI18n();
  const [searchParams] = useSearchParams();
  const [logs, setLogs] = useState<RequestLogListItem[]>([]);
  const [selected, setSelected] = useState<RequestLogDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [confirmClear, setConfirmClear] = useState(false);

  // Filters
  const [keyword, setKeyword] = useState("");
  // 搜索防抖：输入框即时响应，查询用停止输入 300ms 后的值——
  // 避免每个键击都打 listRequestLogs + countRequestLogs 两次查询。
  const debouncedKeyword = useDebouncedValue(keyword, 300);
  const [statusFilter, setStatusFilter] = useState("");
  const [providerFilter, setProviderFilter] = useState("");
  const [modelFilter, setModelFilter] = useState("");
  const [routeProfileFilter, setRouteProfileFilter] = useState("");
  const [errorTypeFilter, setErrorTypeFilter] = useState("");
  const [clientFilter, setClientFilter] = useState("");
  // 'all' / 'gateway' / 'session_log'（聚合所有客户端日志）/ 单一来源（'claude_session' 等）
  const [sourceFilter, setSourceFilter] = useState<string>("");
  const [sessionIdFilter, setSessionIdFilter] = useState("");
  // 'list'（按时间逐条）/ 'session'（按会话聚合）
  const [viewMode, setViewMode] = useState<"list" | "session">("list");
  const providerOptions = useProviders((s) => s.items);
  const routeProfileOptions = useRouteProfiles((s) => s.items);
  const [modelOptions, setModelOptions] = useState<string[]>([]);
  const [showAdvancedFilters, setShowAdvancedFilters] = useState(false);
  const [showSyncActions, setShowSyncActions] = useState(false);

  // Pagination
  const [page, setPage] = useState(1); // 1-indexed
  const [total, setTotal] = useState(0);

  // 初次加载 provider 候选——用 name 而不是 id，因为 request_logs.provider
  // 字段存的是 name 字符串（见 routes.rs log_request_success 调用）。
  // providers / route profiles 走全局 store(其它页可能已经填好,这里直接 fetch 防重入)。
  useEffect(() => {
    useProviders
      .getState()
      .fetch()
      .catch(() => {});
    useRouteProfiles
      .getState()
      .fetch()
      .catch(() => {});
    api
      .listLogModels()
      .then(setModelOptions)
      .catch(() => {});
  }, []);

  useEffect(() => {
    const source = searchParams.get("source");
    if (source && VALID_SOURCE_FILTERS.has(source)) setSourceFilter(source);
  }, [searchParams]);

  // 筛选条件变化时回到第 1 页。在渲染阶段同步调整（React 会在提交前用新
  // page 重渲染），而不是放在 effect 里——effect 版本会让 loadLogs 先用旧
  // page 发一次 list+count，再用 page 1 发第二次。关键字取防抖后的值，
  // URL ?source= 带进来的筛选也走同一条路径。
  const filterKey = JSON.stringify([
    debouncedKeyword,
    statusFilter,
    providerFilter,
    modelFilter,
    routeProfileFilter,
    errorTypeFilter,
    clientFilter,
    sourceFilter,
    sessionIdFilter,
  ]);
  const [pageFilterKey, setPageFilterKey] = useState(filterKey);
  if (pageFilterKey !== filterKey) {
    setPageFilterKey(filterKey);
    setPage(1);
  }

  // 请求序号守卫：筛选/翻页快速变化或手动刷新交错时，会同时有多个
  // loadLogs 在飞；只让最后一次发起的请求写入结果，丢弃先发后到的旧响应，
  // 避免列表被过期数据覆盖。loadLogs 被多处调用（筛选 effect / 刷新按钮 /
  // 清空 / 同步），所以用序号守卫而非只能绑 effect 生命周期的 cancelled 标记。
  const reqSeqRef = useRef(0);
  const loadLogs = useCallback(async () => {
    const seq = ++reqSeqRef.current;
    setLoading(true);
    try {
      const filter = {
        keyword: debouncedKeyword || undefined,
        status: statusFilter || undefined,
        provider: providerFilter || undefined,
        model: modelFilter || undefined,
        route_profile_id: routeProfileFilter || undefined,
        error_type: errorTypeFilter || undefined,
        client: clientFilter || undefined,
        source: sourceFilter || undefined,
        session_id: sessionIdFilter || undefined,
        limit: PAGE_SIZE,
        offset: (page - 1) * PAGE_SIZE,
      };
      const [data, count] = await Promise.all([
        api.listRequestLogs(filter),
        api.countRequestLogs(filter),
      ]);
      if (seq !== reqSeqRef.current) return; // 已有更新的请求发出，丢弃本次旧结果
      setLogs(data);
      setTotal(count);
    } catch (err) {
      if (seq !== reqSeqRef.current) return;
      toast("error", (err as api.AppError).message);
    } finally {
      if (seq === reqSeqRef.current) setLoading(false);
    }
  }, [
    debouncedKeyword,
    statusFilter,
    providerFilter,
    modelFilter,
    routeProfileFilter,
    errorTypeFilter,
    clientFilter,
    sourceFilter,
    sessionIdFilter,
    page,
  ]);

  useEffect(() => {
    loadLogs();
  }, [loadLogs]);

  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const knownStatusLogs = logs.filter((log) => log.status_code !== null);
  const pageErrorCount = knownStatusLogs.filter((log) => {
    const status = log.status_code ?? 0;
    return status < 200 || status >= 400;
  }).length;
  const pageSuccessCount = knownStatusLogs.filter((log) => {
    const status = log.status_code ?? 0;
    return status >= 200 && status < 400;
  }).length;
  const pageSuccessRate =
    knownStatusLogs.length > 0
      ? `${Math.round((pageSuccessCount / knownStatusLogs.length) * 100)}%`
      : "—";
  const pageRecordedLatencies = logs
    .map((log) => log.latency_ms)
    .filter((latency): latency is number => latency !== null && latency > 0);
  const pageAvgLatency =
    pageRecordedLatencies.length > 0
      ? Math.round(
          pageRecordedLatencies.reduce((sum, latency) => sum + latency, 0) /
            pageRecordedLatencies.length
        )
      : null;
  const advancedFilterCount = [
    errorTypeFilter,
    clientFilter,
    sourceFilter,
    sessionIdFilter,
  ].filter(Boolean).length;

  const handleSelect = async (item: RequestLogListItem) => {
    try {
      const detail = await api.getRequestLogDetail(item.id);
      setSelected(detail);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleClear = async () => {
    try {
      await api.clearRequestLogs();
      toast("success", t("logs.cleared"));
      setConfirmClear(false);
      loadLogs();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const [syncing, setSyncing] = useState<null | "claude" | "codex" | "gemini">(
    null
  );
  const runSync = async (
    kind: "claude" | "codex" | "gemini",
    label: string,
    fn: () => Promise<api.SyncResult>,
    missingHint: string
  ) => {
    setSyncing(kind);
    try {
      const r = await fn();
      if (r.files_scanned === 0) {
        toast("success", missingHint);
      } else {
        toast(
          "success",
          `${label} ${t("logs.sync_scanned")} ${r.files_scanned} ${t("logs.sync_files")}: ${t("logs.sync_imported")} ${r.imported}, ${t("logs.sync_skipped")} ${r.skipped}` +
            (r.errors.length > 0
              ? `, ${r.errors.length} ${t("logs.sync_errors")}`
              : "")
        );
      }
      loadLogs();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setSyncing(null);
    }
  };
  const handleSyncClaude = () =>
    runSync(
      "claude",
      "Claude",
      api.syncClaudeSessions,
      t("logs.sync_claude_missing")
    );
  const handleSyncCodex = () =>
    runSync(
      "codex",
      "Codex",
      api.syncCodexSessions,
      t("logs.sync_codex_missing")
    );
  const handleSyncGemini = () =>
    runSync(
      "gemini",
      "Gemini",
      api.syncGeminiSessions,
      t("logs.sync_gemini_missing")
    );

  // 翻页后把列表滚回顶部——长列表翻到下一页却停在原滚动位置很难用。
  const topRef = useRef<HTMLDivElement>(null);
  const goToPage = (target: number) => {
    setPage(target);
    topRef.current?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  return (
    <div ref={topRef} className="desktop-page">
      <div className="desktop-page-header">
        <div>
          <h2 className="flex items-center gap-2 text-lg font-semibold text-text-primary">
            <ScrollText className="h-4 w-4" />
            {t("nav.logs")}
          </h2>
          <p className="mt-1 max-w-2xl text-xs text-text-muted">
            {t("logs.console_hint")}
          </p>
        </div>
      </div>

      <div className="space-y-3">
        {/* 请求视图才展示页级流量快照；会话视图不相关，省掉整块空白。 */}
        {viewMode === "list" && total > 0 && (
          <section className="surface-panel p-4">
            <div className="mb-3">
              <h3 className="text-sm font-semibold text-text-primary">
                {t("logs.traffic_snapshot")}
              </h3>
              <p className="mt-0.5 text-xs text-text-muted">
                {t("logs.traffic_snapshot_hint")}
              </p>
            </div>
            <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
              <LogSummaryItem
                label={t("logs.summary_matched")}
                value={total.toLocaleString()}
                hint={t("logs.requests")}
              />
              <LogSummaryItem
                label={t("logs.summary_page_errors")}
                value={pageErrorCount.toLocaleString()}
                hint={`${knownStatusLogs.length.toLocaleString()} ${t("logs.summary_with_status")}`}
              />
              <LogSummaryItem
                label={t("logs.summary_page_success_rate")}
                value={pageSuccessRate}
                hint={`${pageSuccessCount.toLocaleString()} ${t("logs.summary_succeeded")}`}
              />
              <LogSummaryItem
                label={t("logs.summary_page_avg_latency")}
                value={formatOptionalLatency(pageAvgLatency)}
                hint={`${pageRecordedLatencies.length.toLocaleString()} ${t("logs.summary_recorded")}`}
              />
            </div>
          </section>
        )}

        <div className="surface-panel px-3 py-2.5">
          {/* 单行工具条：视图切换 + 筛选 + 操作，避免上下两层大留白 */}
          <div className="flex flex-wrap items-center gap-2">
            <div
              className="flex shrink-0 items-center gap-0.5 rounded-lg bg-card-secondary p-0.5"
              title={
                viewMode === "session"
                  ? t("logs.query_builder_hint_session")
                  : t("logs.query_builder_hint")
              }
            >
              <button
                type="button"
                onClick={() => setViewMode("list")}
                className={`flex items-center gap-1 rounded-md px-2.5 py-1 text-xs font-medium transition-colors ${viewMode === "list" ? "bg-card text-text-primary" : "text-text-muted hover:text-text-primary"}`}
              >
                <LayoutList className="h-3.5 w-3.5" />
                {t("logs.view_list")}
              </button>
              <button
                type="button"
                onClick={() => setViewMode("session")}
                className={`flex items-center gap-1 rounded-md px-2.5 py-1 text-xs font-medium transition-colors ${viewMode === "session" ? "bg-card text-text-primary" : "text-text-muted hover:text-text-primary"}`}
              >
                <Layers className="h-3.5 w-3.5" />
                {t("logs.view_session")}
              </button>
            </div>

            <input
              type="text"
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              placeholder={
                viewMode === "session"
                  ? t("logs.search_session")
                  : t("logs.search")
              }
              className="form-input min-w-[10rem] max-w-sm flex-1 !py-1.5 text-xs"
            />
            {viewMode === "list" && (
              <select
                value={statusFilter}
                onChange={(e) => setStatusFilter(e.target.value)}
                className="form-input !w-28 shrink-0 !py-1.5 text-xs"
              >
                <option value="">{t("logs.all")}</option>
                <option value="success">{t("logs.success")}</option>
                <option value="error">{t("logs.error")}</option>
              </select>
            )}
            {providerOptions.length > 1 && (
              <select
                value={providerFilter}
                onChange={(e) => setProviderFilter(e.target.value)}
                className="form-input !w-32 shrink-0 !py-1.5 text-xs"
                title={t("logs.filter_provider")}
              >
                <option value="">{t("logs.all_providers")}</option>
                {providerOptions.map((p) => (
                  <option key={p.id} value={p.name}>
                    {p.name}
                  </option>
                ))}
              </select>
            )}
            {viewMode === "list" && modelOptions.length > 0 && (
              <select
                value={modelFilter}
                onChange={(e) => setModelFilter(e.target.value)}
                className="form-input !w-36 shrink-0 !py-1.5 text-xs"
                title={t("logs.filter_model")}
              >
                <option value="">{t("logs.all_models")}</option>
                {modelOptions.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            )}
            {viewMode === "list" && routeProfileOptions.length > 0 && (
              <select
                value={routeProfileFilter}
                onChange={(e) => setRouteProfileFilter(e.target.value)}
                className="form-input !w-36 shrink-0 !py-1.5 text-xs"
                title={t("logs.filter_route_profile")}
              >
                <option value="">{t("logs.all_routes")}</option>
                {routeProfileOptions.map((profile) => (
                  <option key={profile.id} value={profile.id}>
                    {profile.name}
                  </option>
                ))}
              </select>
            )}
            {viewMode === "session" && (
              <>
                <select
                  value={clientFilter}
                  onChange={(e) => setClientFilter(e.target.value)}
                  className="form-input !w-28 shrink-0 !py-1.5 text-xs"
                  title={t("logs.filter_client")}
                >
                  <option value="">{t("logs.all_clients")}</option>
                  {KNOWN_CLIENTS.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
                <select
                  value={sourceFilter}
                  onChange={(e) => setSourceFilter(e.target.value)}
                  className="form-input !w-32 shrink-0 !py-1.5 text-xs"
                  title={t("logs.filter_source")}
                >
                  <option value="">{t("logs.all_sources")}</option>
                  <option value="gateway">{sourceLabel("gateway", t)}</option>
                  <option value="session_log">
                    {t("logs.source_session_log")}
                  </option>
                  <option value="claude_session">
                    {sourceLabel("claude_session", t)}
                  </option>
                  <option value="codex_session">
                    {sourceLabel("codex_session", t)}
                  </option>
                  <option value="gemini_session">
                    {sourceLabel("gemini_session", t)}
                  </option>
                </select>
              </>
            )}
            {viewMode === "list" && (
              <button
                type="button"
                onClick={() => setShowAdvancedFilters((v) => !v)}
                className={`shrink-0 whitespace-nowrap rounded-md px-2.5 py-1 text-xs font-medium transition-colors ${showAdvancedFilters ? "bg-card-secondary text-text-primary" : "text-text-muted hover:bg-card-secondary hover:text-text-primary"}`}
              >
                {showAdvancedFilters
                  ? t("logs.filters_collapse")
                  : `${t("logs.filters_advanced")}${advancedFilterCount > 0 ? ` (${advancedFilterCount})` : ""}`}
              </button>
            )}
            {sessionIdFilter && (
              <button
                type="button"
                onClick={() => setSessionIdFilter("")}
                className="max-w-[200px] truncate rounded-md bg-accent/10 px-2 py-1 font-mono text-xs text-accent transition-colors hover:bg-accent/15"
                title={sessionIdFilter}
              >
                session:{sessionIdFilter.slice(0, 12)}…
                <span className="ml-1 opacity-70">×</span>
              </button>
            )}

            <div className="ml-auto flex shrink-0 items-center gap-1">
              <button
                onClick={loadLogs}
                disabled={loading}
                className="flex items-center gap-1 rounded-md px-2 py-1 text-xs font-medium text-text-secondary transition-colors hover:bg-card-secondary hover:text-text-primary"
              >
                <RefreshCcw
                  className={`h-3 w-3 ${loading ? "animate-spin" : ""}`}
                />
                {t("common.refresh")}
              </button>
              <button
                type="button"
                onClick={() => setShowSyncActions((v) => !v)}
                className={`flex items-center gap-1 rounded-md px-2 py-1 text-xs font-medium transition-colors ${showSyncActions ? "bg-card-secondary text-text-primary" : "text-text-muted hover:bg-card-secondary hover:text-text-primary"}`}
              >
                <Download className="h-3 w-3" />
                {t("logs.sync_logs")}
              </button>
              <button
                onClick={() => setConfirmClear(true)}
                className="rounded-md px-2 py-1 text-xs font-medium text-text-muted transition-colors hover:bg-card-secondary hover:text-text-primary"
              >
                {t("logs.clear")}
              </button>
            </div>
          </div>

          {viewMode === "list" && showAdvancedFilters && (
            <div className="mt-3 flex flex-wrap items-center gap-2 border-t border-border pt-3">
              <select
                value={errorTypeFilter}
                onChange={(e) => setErrorTypeFilter(e.target.value)}
                className="form-input !w-36 shrink-0"
                title={t("logs.filter_error_type")}
              >
                <option value="">{t("logs.all_error_types")}</option>
                <option value="auth_failed">
                  {t("logs.error_type.auth_failed")}
                </option>
                <option value="rate_limited">
                  {t("logs.error_type.rate_limited")}
                </option>
                <option value="quota_or_balance">
                  {t("logs.error_type.quota_or_balance")}
                </option>
                <option value="server_error">
                  {t("logs.error_type.server_error")}
                </option>
                <option value="network_error">
                  {t("logs.error_type.network_error")}
                </option>
                <option value="protocol_error">
                  {t("logs.error_type.protocol_error")}
                </option>
                <option value="other_error">
                  {t("logs.error_type.other_error")}
                </option>
              </select>
              <select
                value={clientFilter}
                onChange={(e) => setClientFilter(e.target.value)}
                className="form-input !w-32 shrink-0"
                title={t("logs.filter_client")}
              >
                <option value="">{t("logs.all_clients")}</option>
                {KNOWN_CLIENTS.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
              <select
                value={sourceFilter}
                onChange={(e) => setSourceFilter(e.target.value)}
                className="form-input !w-36 shrink-0"
                title={t("logs.filter_source")}
              >
                <option value="">{t("logs.all_sources")}</option>
                <option value="gateway">{sourceLabel("gateway", t)}</option>
                <option value="session_log">
                  {t("logs.source_session_log")}
                </option>
                <option value="claude_session">
                  {sourceLabel("claude_session", t)}
                </option>
                <option value="codex_session">
                  {sourceLabel("codex_session", t)}
                </option>
                <option value="gemini_session">
                  {sourceLabel("gemini_session", t)}
                </option>
              </select>
            </div>
          )}
        </div>

        {showSyncActions && (
          <div className="surface-panel flex flex-wrap items-center gap-2 p-3">
            <button
              onClick={handleSyncClaude}
              disabled={syncing !== null}
              className="flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md bg-card-secondary px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary disabled:opacity-50"
              title={t("logs.sync_claude_hint")}
            >
              <Download
                className={`h-3 w-3 ${syncing === "claude" ? "animate-pulse" : ""}`}
              />
              {t("logs.sync_claude")}
            </button>
            <button
              onClick={handleSyncCodex}
              disabled={syncing !== null}
              className="flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md bg-card-secondary px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary disabled:opacity-50"
              title={t("logs.sync_codex_hint")}
            >
              <Download
                className={`h-3 w-3 ${syncing === "codex" ? "animate-pulse" : ""}`}
              />
              {t("logs.sync_codex")}
            </button>
            <button
              onClick={handleSyncGemini}
              disabled={syncing !== null}
              className="flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md bg-card-secondary px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary disabled:opacity-50"
              title={t("logs.sync_gemini_hint")}
            >
              <Download
                className={`h-3 w-3 ${syncing === "gemini" ? "animate-pulse" : ""}`}
              />
              {t("logs.sync_gemini")}
            </button>
          </div>
        )}
      </div>

      {viewMode === "session" ? (
        <SessionGroupView
          filter={{
            keyword: debouncedKeyword || undefined,
            provider: providerFilter || undefined,
            client: clientFilter || undefined,
            source: sourceFilter || undefined,
            session_id: sessionIdFilter || undefined,
          }}
          onPickSession={(sid) => {
            setViewMode("list");
            setSessionIdFilter(sid);
          }}
          onEmptyAction={() => setShowSyncActions(true)}
        />
      ) : loading ? (
        <p className="text-xs text-text-muted">{t("common.loading")}</p>
      ) : logs.length === 0 ? (
        <EmptyState
          icon={ScrollText}
          title={t("logs.no_logs")}
          description={t("logs.no_logs_desc")}
        />
      ) : (
        <>
          <div className="surface-scroll">
            <RequestLogTable requests={logs} onSelect={handleSelect} />
          </div>
          {totalPages > 1 && (
            <div className="surface-panel flex items-center justify-between px-5 py-2.5">
              <span className="text-xs text-text-muted">
                {t("logs.page_prefix")}{" "}
                <span className="font-mono text-text-primary">{page}</span> /{" "}
                {totalPages} {t("logs.page_suffix")}
                <span className="ml-3 text-text-muted/60">
                  {t("logs.page_showing")} {(page - 1) * PAGE_SIZE + 1}–
                  {Math.min(page * PAGE_SIZE, total)}
                </span>
              </span>
              <div className="flex items-center gap-1">
                <button
                  onClick={() => goToPage(Math.max(1, page - 1))}
                  disabled={page <= 1 || loading}
                  className="flex items-center gap-1 rounded-md bg-card-secondary px-2.5 py-1 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary disabled:opacity-40 disabled:hover:bg-card-secondary disabled:hover:text-text-secondary"
                >
                  <ChevronLeft className="h-3 w-3" />
                  {t("logs.page_prev")}
                </button>
                <button
                  onClick={() => goToPage(Math.min(totalPages, page + 1))}
                  disabled={page >= totalPages || loading}
                  className="flex items-center gap-1 rounded-md bg-card-secondary px-2.5 py-1 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary disabled:opacity-40 disabled:hover:bg-card-secondary disabled:hover:text-text-secondary"
                >
                  {t("logs.page_next")}
                  <ChevronRight className="h-3 w-3" />
                </button>
              </div>
            </div>
          )}
        </>
      )}

      <RequestDetailDrawer
        request={selected}
        onClose={() => setSelected(null)}
      />

      <ConfirmDialog
        open={confirmClear}
        title={t("logs.clear_title")}
        message={t("logs.clear_msg")}
        confirmLabel={t("logs.clear_confirm")}
        variant="danger"
        onConfirm={handleClear}
        onCancel={() => setConfirmClear(false)}
      />
    </div>
  );
}

function LogSummaryItem({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint: string;
}) {
  return (
    <div className="rounded-lg border border-border/70 bg-card-secondary/45 px-4 py-3">
      <div className="text-xs font-medium text-text-muted">{label}</div>
      <div className="mt-1 flex items-baseline gap-2">
        <span className="font-mono text-lg font-semibold text-text-primary">
          {value}
        </span>
        <span className="truncate text-xs text-text-muted">{hint}</span>
      </div>
    </div>
  );
}
