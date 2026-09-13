import { useState, useEffect, useCallback, useRef } from "react";
import {
  Plus,
  Star,
  ChevronUp,
  ChevronDown,
  Trash2,
  RotateCcw,
  Shield,
  Zap,
  Inbox,
  X,
  Pencil,
  Check,
  Filter,
  Route,
} from "lucide-react";
import { StatusBadge } from "@/components/common/StatusBadge";
import { CapabilityIcons } from "@/components/common/CapabilityIcons";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { EmptyState } from "@/components/common/EmptyState";
import { SummaryTile } from "@/components/routes/SummaryTile";
import { ConditionsDialog } from "@/components/routes/ConditionsDialog";
import { RouteTemplateDialog } from "@/components/routes/RouteTemplateDialog";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import { usePolling } from "@/lib/usePolling";
import { formatCost } from "@/lib/utils";
import * as api from "@/lib/api";
import { useProviders, useRouteProfiles } from "@/store/global";
import type {
  RouteProfileView,
  RouteProfileDetail,
  RoutingConditions,
  RouteProfileStats,
} from "@/types/route-profile";

const PROTOCOL_LABELS: Record<string, string> = {
  openai_responses: "OpenAI Responses (Codex)",
  anthropic_messages: "Anthropic Messages (Claude Code)",
  openai_chat_completions: "Chat Completions (OpenCode)",
};

function protocolLabel(proto: string): string {
  return PROTOCOL_LABELS[proto] ?? proto;
}

function summarizeRouteAvailability(
  detail: RouteProfileDetail,
  providers: { id: string; enabled?: boolean }[]
) {
  const providerMap = new Map(providers.map((p) => [p.id, p]));
  const reasons = new Set<string>();
  let available = 0;

  for (const rp of detail.providers) {
    const provider = providerMap.get(rp.provider_id);
    const inCooldown =
      !!rp.cooldown_until && new Date(rp.cooldown_until) > new Date();
    const blocked =
      !rp.enabled ||
      !provider ||
      provider.enabled === false ||
      !rp.runtime_available ||
      inCooldown;

    if (!blocked) {
      available += 1;
      continue;
    }

    if (!rp.enabled) reasons.add("routes.reason_route_disabled");
    if (!provider) reasons.add("routes.reason_provider_missing");
    if (provider?.enabled === false)
      reasons.add("routes.reason_provider_disabled");
    if (!rp.runtime_available) reasons.add("routes.reason_runtime_unavailable");
    if (inCooldown) reasons.add("routes.reason_cooldown");
  }

  return {
    total: detail.providers.length,
    available,
    reasons: Array.from(reasons),
  };
}

export function Routes() {
  const { t } = useI18n();
  const profiles = useRouteProfiles((s) => s.items);
  const providers = useProviders((s) => s.items);
  const [detail, setDetail] = useState<RouteProfileDetail | null>(null);
  const [profileStats, setProfileStats] = useState<
    Record<string, RouteProfileStats>
  >({});
  const [loading, setLoading] = useState(true);
  const [deleteTarget, setDeleteTarget] = useState<RouteProfileView | null>(
    null
  );
  const selectedIdRef = useRef<string | null>(null);
  // 轮询 load 与用户点击 selectProfile 都会拉详情；序号守卫保证只有最后发出的
  // 那次请求能写入 detail，避免慢的旧请求把用户刚选的 profile 覆盖回去。
  const detailSeqRef = useRef(0);
  // 进行中的 load / selectProfile 数量：轮询 tick 遇到进行中的请求直接跳过，
  // 避免慢查询期间每个 tick 都让上一次请求作废、页面一直停在旧数据。
  const inFlightRef = useRef(0);
  const [showCreate, setShowCreate] = useState(false);
  const [showTemplate, setShowTemplate] = useState(false);
  const [canRollbackTemplate, setCanRollbackTemplate] = useState(false);
  const [newName, setNewName] = useState("");
  const [newProtocol, setNewProtocol] = useState("openai_responses");
  const [editingName, setEditingName] = useState(false);
  const [editName, setEditName] = useState("");
  const [addProviderId, setAddProviderId] = useState("");
  const [conditionsTarget, setConditionsTarget] = useState<{
    profileId: string;
    providerId: string;
    providerName: string;
    inputProtocol: string;
    current: RoutingConditions;
  } | null>(null);

  const load = useCallback(async () => {
    // 序号在发起时就占位：之后的用户点击一定比这次 load 新。
    const seq = ++detailSeqRef.current;
    inFlightRef.current += 1;
    try {
      // profiles / providers 走全局 store——usePolling 10s 周期会顺带刷新这两份,
      // 别的页打开时不重复 invoke。
      const [, , stats] = await Promise.all([
        useRouteProfiles.getState().refetch(),
        useProviders.getState().refetch(),
        api.aggregateRouteProfileStats(7),
      ]);
      const p = useRouteProfiles.getState().items;
      setProfileStats(
        Object.fromEntries(stats.map((s) => [s.route_profile_id, s]))
      );
      if (p.length > 0) {
        const currentId = selectedIdRef.current;
        const toLoad =
          currentId && p.find((x) => x.id === currentId) ? currentId : p[0].id;
        const d = await api.getRouteProfile(toLoad);
        if (seq !== detailSeqRef.current) return;
        selectedIdRef.current = toLoad;
        setDetail(d);
        const canRollback = await api
          .hasRouteTemplateRollback(toLoad)
          .catch(() => false);
        if (seq !== detailSeqRef.current) return;
        setCanRollbackTemplate(canRollback);
      }
    } catch (err) {
      // 已被更新的请求取代的旧请求报错不再弹 toast(与 selectProfile 一致)。
      if (seq !== detailSeqRef.current) return;
      toast("error", (err as api.AppError).message);
    } finally {
      inFlightRef.current -= 1;
      setLoading(false);
    }
  }, []);
  const pollLoad = useCallback(() => {
    if (inFlightRef.current > 0) return;
    load();
  }, [load]);

  useEffect(() => {
    load();
  }, [load]);
  // 周期 + focus 刷新——让后台 cooldown 变化、新加的 provider 立即可见
  usePolling(pollLoad);

  const selectProfile = async (id: string) => {
    const seq = ++detailSeqRef.current;
    // 点击即记录选择：详情返回前触发的轮询也会去拉这个 profile。
    selectedIdRef.current = id;
    inFlightRef.current += 1;
    try {
      const d = await api.getRouteProfile(id);
      if (seq !== detailSeqRef.current) return;
      setDetail(d);
      const canRollback = await api
        .hasRouteTemplateRollback(id)
        .catch(() => false);
      if (seq !== detailSeqRef.current) return;
      setCanRollbackTemplate(canRollback);
    } catch (err) {
      if (seq !== detailSeqRef.current) return;
      toast("error", (err as api.AppError).message);
    } finally {
      inFlightRef.current -= 1;
    }
  };

  const handleSetDefault = async (id: string) => {
    try {
      await api.setDefaultRouteProfile(id);
      toast("success", t("routes.default_updated"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleToggleMode = async () => {
    if (!detail) return;
    const newMode = detail.profile.mode === "manual" ? "failover" : "manual";
    try {
      await api.setRouteProfileMode(detail.profile.id, newMode);
      toast("success", `${t("routes.mode_changed")} ${newMode}`);
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleStrategyChange = async (strategy: string) => {
    if (!detail) return;
    try {
      await api.updateRouteProfile(detail.profile.id, {
        selection_strategy: strategy,
      });
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleSetActive = async (providerId: string) => {
    if (!detail) return;
    try {
      await api.setRouteActiveProvider(detail.profile.id, providerId);
      toast("success", t("routes.active_updated"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleAddProvider = async (providerId: string) => {
    if (!detail) return;
    try {
      await api.addProviderToRoute(detail.profile.id, providerId, {});
      toast("success", t("routes.provider_added"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleRemoveProvider = async (providerId: string) => {
    if (!detail) return;
    try {
      await api.removeProviderFromRoute(detail.profile.id, providerId);
      toast("success", t("routes.provider_removed"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleReorder = async (
    providerId: string,
    direction: "up" | "down"
  ) => {
    if (!detail) return;
    const ids = detail.providers.map((p) => p.provider_id);
    const idx = ids.indexOf(providerId);
    if (idx < 0) return;
    const swapIdx = direction === "up" ? idx - 1 : idx + 1;
    if (swapIdx < 0 || swapIdx >= ids.length) return;
    [ids[idx], ids[swapIdx]] = [ids[swapIdx], ids[idx]];
    try {
      await api.reorderRouteProviders(detail.profile.id, ids);
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleResetCooldown = async (providerId: string) => {
    try {
      await api.resetProviderRuntimeStatus(providerId);
      toast("success", t("routes.cooldown_reset"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await api.deleteRouteProfile(deleteTarget.id);
      toast("success", t("routes.deleted"));
      setDeleteTarget(null);
      selectedIdRef.current = null;
      setDetail(null);
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleRename = async () => {
    if (!detail || !editName.trim()) return;
    try {
      await api.updateRouteProfile(detail.profile.id, {
        name: editName.trim(),
      });
      setEditingName(false);
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleCreate = async () => {
    if (!newName.trim()) return;
    try {
      await api.createRouteProfile({
        name: newName.trim(),
        input_protocol: newProtocol,
      });
      toast("success", t("routes.created"));
      setShowCreate(false);
      setNewName("");
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const availableProviders = detail
    ? providers.filter(
        (p) => !detail.providers.some((rp) => rp.provider_id === p.id)
      )
    : [];
  const currentStats = detail ? profileStats[detail.profile.id] : undefined;
  const availability = detail
    ? summarizeRouteAvailability(detail, providers)
    : null;

  if (loading)
    return <p className="text-xs text-text-muted">{t("common.loading")}</p>;

  return (
    <div className="desktop-page">
      {/* Header */}
      <div className="desktop-page-header flex flex-wrap items-center justify-between gap-3">
        <p className="text-xs text-text-muted">
          {profiles.length} {t("routes.route_profiles")}
        </p>
        <div className="flex items-center gap-2">
          {detail && canRollbackTemplate && (
            <button
              onClick={async () => {
                try {
                  await api.rollbackRouteTemplate(detail.profile.id);
                  toast("success", t("routes.template_rolled_back"));
                  setCanRollbackTemplate(false);
                  load();
                } catch (err) {
                  toast("error", (err as api.AppError).message);
                }
              }}
              className="flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-xs font-medium text-text-secondary hover:bg-hover"
            >
              <RotateCcw className="h-3 w-3" />
              {t("routes.template_rollback")}
            </button>
          )}
          <button
            onClick={() => setShowTemplate(true)}
            disabled={!detail}
            className="flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-xs font-medium text-text-secondary hover:bg-hover disabled:opacity-40"
          >
            <Route className="h-3 w-3" />
            {t("routes.template")}
          </button>
          <button
            onClick={() => setShowCreate(true)}
            className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-on-accent hover:bg-accent/90"
          >
            <Plus className="h-3 w-3" />
            {t("routes.create_profile")}
          </button>
        </div>
      </div>

      {/* Create form */}
      {showCreate && (
        <div className="surface-panel border-accent/30 p-4">
          <div className="mb-3 flex items-center justify-between">
            <h4 className="text-xs font-semibold text-text-primary">
              {t("routes.create_profile")}
            </h4>
            <button
              onClick={() => setShowCreate(false)}
              className="text-text-muted hover:text-text-primary"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="mb-1 block text-xs text-text-muted">
                {t("routes.profile_name")}
              </label>
              <input
                type="text"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                placeholder="My Route"
                className="form-input w-full"
              />
            </div>
            <div>
              <label className="mb-1 block text-xs text-text-muted">
                {t("routes.protocol")}
              </label>
              <select
                value={newProtocol}
                onChange={(e) => setNewProtocol(e.target.value)}
                className="form-input w-full"
              >
                {Object.entries(PROTOCOL_LABELS).map(([val, label]) => (
                  <option key={val} value={val}>
                    {label}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="mt-3 flex justify-end gap-2">
            <button
              onClick={() => setShowCreate(false)}
              className="btn-secondary"
            >
              {t("common.cancel")}
            </button>
            <button
              onClick={handleCreate}
              disabled={!newName.trim()}
              className="btn-primary"
            >
              {t("common.save")}
            </button>
          </div>
        </div>
      )}

      {profiles.length === 0 ? (
        <EmptyState
          icon={Inbox}
          title={t("routes.no_profiles")}
          description={t("routes.auto_created")}
        />
      ) : (
        <div className="grid min-w-0 grid-cols-[220px_minmax(0,1fr)] gap-4">
          {/* Profile selector (left) */}
          <div className="space-y-2">
            {profiles.map((p) => (
              <button
                key={p.id}
                onClick={() => selectProfile(p.id)}
                className={`w-full rounded-lg border p-3.5 text-left transition-colors ${
                  detail?.profile.id === p.id
                    ? "border-accent/40 bg-accent/5"
                    : "border-border bg-card hover:bg-hover"
                }`}
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <span className="block truncate text-sm font-semibold text-text-primary">
                      {protocolLabel(p.input_protocol)}
                    </span>
                    <span className="mt-1 block truncate text-xs text-text-muted">
                      {t("routes.current_provider")}:{" "}
                      {p.active_provider_name ?? t("common.none")}
                    </span>
                  </div>
                  <StatusBadge
                    variant={p.mode === "failover" ? "accent" : "muted"}
                  >
                    {p.mode === "failover"
                      ? t("routes.mode_failover")
                      : t("routes.mode_manual")}
                  </StatusBadge>
                </div>
                <p className="mt-2 truncate text-xs text-text-muted">
                  {p.name} · {p.providers_count} provider
                  {p.providers_count !== 1 ? "s" : ""}
                </p>
              </button>
            ))}
          </div>

          {/* Profile detail (right) */}
          {detail && (
            <div className="flex-1 space-y-4">
              {availability && (
                <div
                  className={`surface-panel p-5 ${
                    availability.available === 0
                      ? "border-warning/30 bg-warning/10"
                      : "border-border bg-card"
                  }`}
                >
                  <div className="flex flex-wrap items-start justify-between gap-4">
                    <div className="min-w-0">
                      <div className="mb-3 flex items-center gap-2">
                        <p className="text-xs font-semibold text-text-muted">
                          {t("routes.route_result")}
                        </p>
                        {editingName ? (
                          <div className="flex items-center gap-2">
                            <input
                              type="text"
                              value={editName}
                              onChange={(e) => setEditName(e.target.value)}
                              onKeyDown={(e) =>
                                e.key === "Enter" && handleRename()
                              }
                              className="form-input text-sm font-semibold"
                              autoFocus
                            />
                            <button
                              onClick={handleRename}
                              className="rounded p-1 text-accent hover:bg-accent-soft"
                            >
                              <Check className="h-3.5 w-3.5" />
                            </button>
                            <button
                              onClick={() => setEditingName(false)}
                              className="rounded p-1 text-text-muted hover:bg-border"
                            >
                              <X className="h-3.5 w-3.5" />
                            </button>
                          </div>
                        ) : (
                          <button
                            onClick={() => {
                              setEditName(detail.profile.name);
                              setEditingName(true);
                            }}
                            className="rounded p-1 text-text-muted hover:bg-border hover:text-text-primary"
                            title={t("routes.rename_profile")}
                          >
                            <Pencil className="h-3 w-3" />
                          </button>
                        )}
                      </div>
                      <h4 className="mt-1 truncate text-lg font-semibold text-text-primary">
                        {protocolLabel(detail.profile.input_protocol)} →{" "}
                        {detail.profile.active_provider_name ??
                          t("common.none")}
                      </h4>
                      <p className="mt-1 text-xs text-text-muted">
                        {detail.profile.name}
                      </p>
                    </div>
                    <div className="shrink-0 text-right">
                      <p className="mb-1 text-xs font-medium uppercase tracking-wide text-text-muted">
                        {t("routes.availability")}
                      </p>
                      <StatusBadge
                        variant={
                          availability.available === 0 ? "warning" : "success"
                        }
                      >
                        {availability.available} / {availability.total}
                      </StatusBadge>
                    </div>
                  </div>
                  <div className="mt-5 grid grid-cols-1 gap-4 lg:grid-cols-[1fr_260px]">
                    <div className="rounded-lg border border-border/70 bg-card-secondary p-4">
                      <div className="mb-3 flex items-center justify-between gap-3">
                        <p className="text-xs font-semibold text-text-primary">
                          {t("routes.match_settings")}
                        </p>
                        <StatusBadge
                          variant={
                            detail.profile.is_default ? "accent" : "muted"
                          }
                        >
                          {detail.profile.is_default
                            ? t("routes.default_profile")
                            : t("routes.custom_profile")}
                        </StatusBadge>
                      </div>
                      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                        <div>
                          <p className="text-xs font-medium uppercase tracking-wide text-text-muted">
                            {t("routes.flow_entry")}
                          </p>
                          <p className="mt-1 truncate text-sm font-medium text-text-primary">
                            {protocolLabel(detail.profile.input_protocol)}
                          </p>
                        </div>
                        <div>
                          <p className="text-xs font-medium uppercase tracking-wide text-text-muted">
                            {t("routes.flow_mode")}
                          </p>
                          <p className="mt-1 text-sm font-medium text-text-primary">
                            {detail.profile.mode === "failover"
                              ? t("routes.mode_plain_failover")
                              : t("routes.mode_plain_manual")}
                          </p>
                        </div>
                      </div>
                    </div>
                    <div className="rounded-lg border border-border/70 bg-card-secondary p-4">
                      <p className="text-xs font-semibold text-text-primary">
                        {t("routes.edit_route")}
                      </p>
                      <div className="mt-3 flex rounded-md border border-border bg-card p-0.5">
                        <button
                          onClick={() =>
                            detail.profile.mode !== "manual" &&
                            handleToggleMode()
                          }
                          className={`flex flex-1 items-center justify-center gap-1.5 rounded px-2.5 py-1.5 text-xs font-medium transition-colors ${
                            detail.profile.mode === "manual"
                              ? "bg-card-secondary text-text-primary shadow-sm"
                              : "text-text-muted hover:text-text-primary"
                          }`}
                        >
                          <Shield className="h-3 w-3" />
                          {t("routes.mode_manual")}
                        </button>
                        <button
                          onClick={() =>
                            detail.profile.mode !== "failover" &&
                            handleToggleMode()
                          }
                          className={`flex flex-1 items-center justify-center gap-1.5 rounded px-2.5 py-1.5 text-xs font-medium transition-colors ${
                            detail.profile.mode === "failover"
                              ? "bg-card-secondary text-accent shadow-sm"
                              : "text-text-muted hover:text-text-primary"
                          }`}
                        >
                          <Zap className="h-3 w-3" />
                          {t("routes.mode_failover")}
                        </button>
                      </div>
                      {detail.profile.mode === "failover" && (
                        <label className="mt-3 flex items-center justify-between gap-2 rounded-md border border-border bg-card px-2.5 py-1.5 text-xs text-text-secondary">
                          {t("routes.strategy")}
                          <select
                            value={detail.profile.selection_strategy}
                            onChange={(e) =>
                              handleStrategyChange(e.target.value)
                            }
                            className="bg-transparent text-text-primary outline-none"
                            title={t("routes.strategy_hint")}
                          >
                            <option value="priority">
                              {t("routes.strategy_priority")}
                            </option>
                            <option value="cheapest">
                              {t("routes.strategy_cheapest")}
                            </option>
                            <option value="fastest">
                              {t("routes.strategy_fastest")}
                            </option>
                          </select>
                        </label>
                      )}
                      <div className="mt-3 flex flex-wrap gap-2">
                        {!detail.profile.is_default &&
                          profiles.filter(
                            (p) =>
                              p.input_protocol === detail.profile.input_protocol
                          ).length > 1 && (
                            <button
                              onClick={() =>
                                handleSetDefault(detail.profile.id)
                              }
                              className="flex items-center gap-1.5 rounded-md bg-card px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-border hover:text-text-primary"
                            >
                              <Star className="h-3 w-3" />
                              {t("routes.set_default")}
                            </button>
                          )}
                        <button
                          onClick={() => setDeleteTarget(detail.profile)}
                          className="flex items-center gap-1.5 rounded-md bg-card px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-error/20 hover:text-error"
                          title={t("routes.delete_profile")}
                        >
                          <Trash2 className="h-3 w-3" />
                        </button>
                      </div>
                    </div>
                  </div>
                  <div className="mt-3 border-t border-border/70 pt-3">
                    {availability.reasons.length > 0 ? (
                      <div className="flex flex-wrap gap-2">
                        {availability.reasons.map((reason) => (
                          <span
                            key={reason}
                            className="rounded-full bg-card-secondary px-2 py-0.5 text-xs text-text-secondary"
                          >
                            {t(reason)}
                          </span>
                        ))}
                      </div>
                    ) : (
                      <p className="text-xs text-success">
                        {t("routes.all_candidates_available")}
                      </p>
                    )}
                  </div>
                </div>
              )}

              <details className="surface-panel p-4">
                <summary className="cursor-pointer text-xs font-semibold text-text-primary">
                  {t("routes.route_metrics")}
                </summary>
                <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-4">
                  <SummaryTile
                    label={t("routes.stats_requests")}
                    value={(currentStats?.request_count ?? 0).toString()}
                    hint={t("routes.stats_window")}
                  />
                  <SummaryTile
                    label={t("routes.stats_success_rate")}
                    value={formatPercent(currentStats?.success_rate)}
                    hint={`${currentStats?.success_count ?? 0} ${t("routes.stats_success")} / ${currentStats?.error_count ?? 0} ${t("routes.stats_errors")}`}
                  />
                  <SummaryTile
                    label={t("routes.stats_avg_latency")}
                    value={formatStatLatency(currentStats?.avg_latency_ms)}
                    hint={t("routes.stats_gateway_only")}
                  />
                  <SummaryTile
                    label={t("routes.stats_cost")}
                    value={formatCost(currentStats?.cost ?? 0)}
                    hint={t("routes.stats_priced_only")}
                  />
                </div>
              </details>

              {/* Fallback chain — 仅故障转移模式有意义,固定模式不展示空占位 */}
              {detail.profile.mode === "failover" && (
                <div className="surface-panel p-5">
                  <div className="mb-3 flex items-center justify-between gap-3">
                    <div>
                      <h4 className="text-xs font-semibold text-text-primary">
                        {t("routes.fallback_section")}
                      </h4>
                      <p className="mt-0.5 text-xs text-text-muted">
                        {t("routes.fallback_section_hint")}
                      </p>
                    </div>
                    <StatusBadge variant="accent">
                      {t("routes.strategy")}:{" "}
                      {strategyLabel(detail.profile.selection_strategy, t)}
                    </StatusBadge>
                  </div>
                  {detail.providers.length > 1 ? (
                    <div className="flex flex-wrap items-center gap-2">
                      {detail.providers.map((rp, idx) => (
                        <div key={rp.id} className="flex items-center gap-2">
                          <span
                            className={`rounded-md border px-3 py-1.5 text-xs ${idx === 0 ? "border-accent/30 bg-accent/5 text-accent" : "border-border bg-card-secondary text-text-secondary"}`}
                          >
                            {idx === 0
                              ? t("routes.primary_provider")
                              : `${t("routes.fallback_provider")} ${idx}`}{" "}
                            · {rp.provider_name}
                          </span>
                          {idx < detail.providers.length - 1 && (
                            <Route className="h-3.5 w-3.5 text-text-muted" />
                          )}
                        </div>
                      ))}
                    </div>
                  ) : (
                    <p className="rounded-md border border-warning/30 bg-warning/10 px-4 py-3 text-xs text-warning">
                      {t("routes.fallback_needs_more")}
                    </p>
                  )}
                </div>
              )}

              {/* Provider order */}
              <div className="surface-panel p-5">
                <div className="mb-3 flex items-center justify-between">
                  <div>
                    <h4 className="text-xs font-semibold text-text-primary">
                      {t("routes.provider_order")}
                    </h4>
                    <p className="mt-0.5 text-xs text-text-muted">
                      {t("routes.provider_order_hint")}
                    </p>
                  </div>
                  {detail.profile.active_provider_name && (
                    <span className="text-xs text-text-muted">
                      {t("routes.active")}:{" "}
                      <span className="text-text-primary">
                        {detail.profile.active_provider_name}
                      </span>
                    </span>
                  )}
                </div>
                <div className="space-y-2">
                  {detail.providers.map((rp, idx) => {
                    const isActive =
                      rp.provider_id === detail.profile.active_provider_id;
                    const isCooldown =
                      rp.cooldown_until &&
                      new Date(rp.cooldown_until) > new Date();
                    return (
                      <div
                        key={rp.id}
                        className={`flex items-center justify-between rounded-md border px-4 py-3 ${
                          isActive
                            ? "border-accent/30 bg-accent/5"
                            : "border-border/50 bg-card-secondary"
                        }`}
                      >
                        <div className="flex items-center gap-3">
                          <span className="w-5 text-center text-xs font-mono text-text-muted">
                            {rp.priority}
                          </span>
                          <div>
                            <div className="flex items-center gap-2">
                              <span className="text-sm font-medium text-text-primary">
                                {rp.provider_name}
                              </span>
                              {isActive && (
                                <StatusBadge variant="accent">
                                  {t("routes.active")}
                                </StatusBadge>
                              )}
                              {isCooldown && (
                                <StatusBadge variant="warning">
                                  {t("routes.cooldown")}
                                </StatusBadge>
                              )}
                              {rp.consecutive_failures > 0 && (
                                <StatusBadge variant="error">
                                  {rp.consecutive_failures}{" "}
                                  {t("routes.failures")}
                                </StatusBadge>
                              )}
                              {(() => {
                                const providerProtocols: string[] = (() => {
                                  try {
                                    return JSON.parse(rp.provider_protocol);
                                  } catch {
                                    return [rp.provider_protocol];
                                  }
                                })();
                                const inputProto =
                                  detail.profile.input_protocol;
                                const isTransparent =
                                  (inputProto === "openai_chat_completions" &&
                                    providerProtocols.includes(
                                      "openai_chat_completions"
                                    )) ||
                                  (inputProto === "anthropic_messages" &&
                                    rp.has_anthropic_url) ||
                                  (inputProto === "openai_responses" &&
                                    providerProtocols.includes(
                                      "openai_responses"
                                    ));
                                return (
                                  <StatusBadge
                                    variant={
                                      isTransparent ? "muted" : "success"
                                    }
                                  >
                                    {isTransparent
                                      ? t("routes.proxy_mode_transparent")
                                      : t("routes.proxy_mode_convert")}
                                  </StatusBadge>
                                );
                              })()}
                              <CapabilityIcons
                                modelCapabilities={rp.model_capabilities}
                                legacyVision={rp.supports_vision}
                              />
                              {rp.routing_conditions && (
                                <StatusBadge variant="success">
                                  <Filter className="inline h-2.5 w-2.5 mr-0.5" />
                                  {t("routes.has_conditions")}
                                </StatusBadge>
                              )}
                            </div>
                            <p className="text-xs text-text-muted">
                              {rp.provider_type}
                              {rp.model_override && (
                                <> · model: {rp.model_override}</>
                              )}
                              {rp.routing_conditions &&
                                (() => {
                                  try {
                                    const c: RoutingConditions = JSON.parse(
                                      rp.routing_conditions
                                    );
                                    const parts: string[] = [];
                                    if (c.has_images === true)
                                      parts.push("images");
                                    if (c.has_tools === true)
                                      parts.push("tools");
                                    if (c.min_input_chars)
                                      parts.push(
                                        `≥${(c.min_input_chars / 1000).toFixed(0)}K chars`
                                      );
                                    if (c.system_keywords?.length)
                                      parts.push(
                                        `keywords: ${c.system_keywords.join(",")}`
                                      );
                                    if (c.model_override)
                                      parts.push(`→ ${c.model_override}`);
                                    return parts.length > 0 ? (
                                      <>
                                        {" "}
                                        ·{" "}
                                        <span className="text-accent">
                                          {parts.join(" + ")}
                                        </span>
                                      </>
                                    ) : null;
                                  } catch {
                                    return (
                                      <>
                                        {" "}
                                        ·{" "}
                                        <span className="text-error">
                                          {t("routes.invalid_conditions")}
                                        </span>
                                      </>
                                    );
                                  }
                                })()}
                            </p>
                          </div>
                        </div>
                        <div className="flex items-center gap-1">
                          <button
                            onClick={() => {
                              let current: RoutingConditions = {};
                              try {
                                if (rp.routing_conditions)
                                  current = JSON.parse(rp.routing_conditions);
                              } catch {
                                /* 存储的条件 JSON 非法时回退为空条件 */
                              }
                              setConditionsTarget({
                                profileId: detail.profile.id,
                                providerId: rp.provider_id,
                                providerName: rp.provider_name,
                                inputProtocol: detail.profile.input_protocol,
                                current,
                              });
                            }}
                            className="rounded p-1 text-text-muted hover:bg-border hover:text-accent"
                            title={t("routes.edit_conditions")}
                          >
                            <Filter className="h-3.5 w-3.5" />
                          </button>
                          {!isActive && (
                            <button
                              onClick={() => handleSetActive(rp.provider_id)}
                              className="rounded p-1 text-text-muted hover:bg-border hover:text-text-primary"
                              title={t("routes.set_active")}
                            >
                              <Star className="h-3.5 w-3.5" />
                            </button>
                          )}
                          <button
                            onClick={() => handleReorder(rp.provider_id, "up")}
                            disabled={idx === 0}
                            className="rounded p-1 text-text-muted hover:bg-border hover:text-text-primary disabled:opacity-30"
                            title={t("routes.move_up")}
                          >
                            <ChevronUp className="h-3.5 w-3.5" />
                          </button>
                          <button
                            onClick={() =>
                              handleReorder(rp.provider_id, "down")
                            }
                            disabled={idx === detail.providers.length - 1}
                            className="rounded p-1 text-text-muted hover:bg-border hover:text-text-primary disabled:opacity-30"
                            title={t("routes.move_down")}
                          >
                            <ChevronDown className="h-3.5 w-3.5" />
                          </button>
                          {isCooldown && (
                            <button
                              onClick={() =>
                                handleResetCooldown(rp.provider_id)
                              }
                              className="rounded p-1 text-text-muted hover:bg-border hover:text-warning"
                              title={t("routes.reset_cooldown")}
                            >
                              <RotateCcw className="h-3.5 w-3.5" />
                            </button>
                          )}
                          <button
                            onClick={() => handleRemoveProvider(rp.provider_id)}
                            className="rounded p-1 text-text-muted hover:bg-error/20 hover:text-error"
                            title={t("common.delete")}
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      </div>
                    );
                  })}
                </div>

                {/* Add provider */}
                {availableProviders.length > 0 && (
                  <div className="mt-3 flex items-center gap-2">
                    <select
                      value={addProviderId}
                      onChange={(e) => setAddProviderId(e.target.value)}
                      className="form-input flex-1"
                    >
                      <option value="" disabled>
                        {t("routes.add_provider")}
                      </option>
                      {availableProviders.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.name}
                        </option>
                      ))}
                    </select>
                    <button
                      onClick={() => {
                        if (addProviderId) {
                          handleAddProvider(addProviderId);
                          setAddProviderId("");
                        }
                      }}
                      className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-on-accent hover:bg-accent/90"
                    >
                      <Plus className="h-3 w-3" />
                      {t("routes.add")}
                    </button>
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      )}

      <ConfirmDialog
        open={!!deleteTarget}
        title={t("routes.delete_title")}
        message={`${t("common.delete")} "${deleteTarget?.name}"? ${t("routes.delete_confirm")}`}
        confirmLabel={t("common.delete")}
        variant="danger"
        onConfirm={handleDelete}
        onCancel={() => setDeleteTarget(null)}
      />

      {showTemplate && detail && (
        <RouteTemplateDialog
          profileId={detail.profile.id}
          providers={providers.map((p) => ({
            id: p.id,
            name: p.name,
            base_url: p.base_url,
          }))}
          canRollback={canRollbackTemplate}
          onClose={() => setShowTemplate(false)}
          onApplied={() => {
            setShowTemplate(false);
            setCanRollbackTemplate(true);
            load();
          }}
          onRollback={async () => {
            try {
              await api.rollbackRouteTemplate(detail.profile.id);
              toast("success", t("routes.template_rolled_back"));
              setShowTemplate(false);
              setCanRollbackTemplate(false);
              load();
            } catch (err) {
              toast("error", (err as api.AppError).message);
            }
          }}
        />
      )}

      {/* Routing Conditions Dialog */}
      {conditionsTarget && (
        <ConditionsDialog
          target={conditionsTarget}
          onSave={async (conditions) => {
            const json = Object.values(conditions).some(
              (v) =>
                v != null && v !== "" && !(Array.isArray(v) && v.length === 0)
            )
              ? JSON.stringify(conditions)
              : null;
            try {
              await api.updateRouteProviderConditions(
                conditionsTarget.profileId,
                conditionsTarget.providerId,
                json
              );
              toast("success", t("routes.conditions_saved"));
              setConditionsTarget(null);
              load();
            } catch (err) {
              toast("error", (err as api.AppError).message);
            }
          }}
          onClose={() => setConditionsTarget(null)}
        />
      )}
    </div>
  );
}

function formatPercent(value: number | undefined): string {
  if (value == null) return "0%";
  return `${Math.round(value * 100)}%`;
}

function formatStatLatency(value: number | undefined): string {
  if (!value) return "0 ms";
  return value >= 1000
    ? `${(value / 1000).toFixed(1)} s`
    : `${Math.round(value)} ms`;
}

function strategyLabel(strategy: string, t: (key: string) => string): string {
  if (strategy === "cheapest") return t("routes.strategy_cheapest");
  if (strategy === "fastest") return t("routes.strategy_fastest");
  return t("routes.strategy_priority");
}
