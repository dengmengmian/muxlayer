import { useState, useEffect, useCallback, useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { Plus, Inbox, Search, Zap, HardDrive } from "lucide-react";
import { ProviderCard } from "@/components/providers/ProviderCard";
import { ProviderFormDialog } from "@/components/providers/ProviderFormDialog";
import { TestConnectionDialog } from "@/components/providers/TestConnectionDialog";
import { SpeedtestDialog } from "@/components/providers/SpeedtestDialog";
import { LocalModelsDialog } from "@/components/providers/LocalModelsDialog";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { EmptyState } from "@/components/common/EmptyState";
import { toast } from "@/components/common/Toast";
import { useI18n } from "@/lib/i18n";
import { usePolling } from "@/lib/usePolling";
import { fetchDetectAndPersistProviderModels } from "@/lib/providerAutoSetup";
import { useProviders } from "@/store/global";
import * as api from "@/lib/api";
import type {
  ProviderView,
  CreateProviderInput,
  UpdateProviderInput,
} from "@/types/provider";
import type { ProviderRuntimeStatus } from "@/types/route-profile";

export function Providers() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const providers = useProviders((s) => s.items);
  const [runtimeMap, setRuntimeMap] = useState<
    Record<string, ProviderRuntimeStatus>
  >({});
  const [loading, setLoading] = useState(true);
  const [formOpen, setFormOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<ProviderView | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ProviderView | null>(null);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [testingProvider, setTestingProvider] = useState<ProviderView | null>(
    null
  );
  const [search, setSearch] = useState("");
  const [speedtestOpen, setSpeedtestOpen] = useState(false);
  const [localModelsOpen, setLocalModelsOpen] = useState(false);

  // 过滤：name / provider_type / default_model 任一匹配（大小写不敏感）。
  // provider 数少时不显示搜索框（减少视觉噪音），>= 5 个才出现。
  const filteredProviders = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return providers;
    return providers.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.provider_type.toLowerCase().includes(q) ||
        p.default_model.toLowerCase().includes(q)
    );
  }, [providers, search]);

  const loadProviders = useCallback(async () => {
    try {
      // providers 走全局 store——别页 mount 时已经填好,这里走 refetch 拿最新值;
      // runtime status 单独拉取(profile-specific 数据,不进 store),失败不影响列表。
      const [, statuses] = await Promise.all([
        useProviders.getState().refetch(),
        api
          .listProviderRuntimeStatus()
          .catch(() => [] as ProviderRuntimeStatus[]),
      ]);
      setRuntimeMap(
        Object.fromEntries(statuses.map((s) => [s.provider_id, s]))
      );
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setLoading(false);
    }
  }, []);

  const handleResetRuntime = useCallback(
    async (providerId: string) => {
      try {
        await api.resetProviderRuntimeStatus(providerId);
        toast("success", t("routes.cooldown_reset"));
        loadProviders();
      } catch (err) {
        toast("error", (err as api.AppError).message);
      }
    },
    [loadProviders, t]
  );

  const handleDetails = useCallback(
    (provider: ProviderView) => {
      navigate(`/providers/${provider.id}`);
    },
    [navigate]
  );

  useEffect(() => {
    loadProviders();
  }, [loadProviders]);

  // 周期刷新 + window focus 时刷新——让后台 runtime_status 变化（如请求
  // 失败被 cooldown）能实时反映到这页的 badge 上。
  usePolling(loadProviders);

  const handleCreate = async (
    input: CreateProviderInput | UpdateProviderInput
  ) => {
    try {
      const created = await api.createProvider(input as CreateProviderInput);
      setFormOpen(false);
      // Auto-detect vision support for new provider
      api.detectProviderVision(created.id).catch(() => {});
      api.detectProviderCache(created.id).catch(() => {});
      // 添加后总 provider ≥2 时提示用户可以做失败转移——跨页提示，避免
      // 用户配完想做 failover 还得自己探索"Routing"页在哪。
      if (providers.length >= 1) {
        toast(
          "success",
          `${t("providers.created")} · ${t("providers.hint_setup_failover")}`,
          {
            action: {
              label: t("providers.go_routing"),
              onClick: () => navigate("/routes"),
            },
          }
        );
      } else {
        toast("success", t("providers.created"));
      }
      loadProviders();

      // 自动跑"拉取模型 + 识别能力 + 挑选最新 default/reasoning"——新建用户
      // 不该被要求保存后再手动来一次。失败静默（用户可在编辑里手动拉）。
      const providerType = (input as CreateProviderInput).provider_type;
      (async () => {
        try {
          const { models } = await fetchDetectAndPersistProviderModels(
            created.id,
            providerType
          );
          if (!models.length) return;
          loadProviders();
          toast(
            "success",
            `${models.length} ${t("providers.toast_auto_setup")}`
          );
        } catch {
          /* silent: 用户可去编辑页手动拉 */
        }
      })();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const addLocalEndpoint = async (ep: api.LocalEndpoint) => {
    const defaultModel = ep.models[0] ?? "local-model";
    await handleCreate({
      name: ep.name,
      provider_type: ep.provider_type,
      base_url: ep.base_url,
      api_key: "local",
      default_model: defaultModel,
      protocol: JSON.stringify(["openai_chat_completions"]),
      timeout_seconds: 120,
      enabled: true,
    });
  };

  const handleEdit = (provider: ProviderView) => {
    setEditTarget(provider);
    setFormOpen(true);
  };

  const handleUpdate = async (
    input: CreateProviderInput | UpdateProviderInput
  ) => {
    if (!editTarget) return;
    try {
      await api.updateProvider(editTarget.id, input as UpdateProviderInput);
      toast("success", t("providers.updated"));
      setFormOpen(false);
      setEditTarget(null);
      // Auto-detect vision support after update
      api.detectProviderVision(editTarget.id).catch(() => {});
      api.detectProviderCache(editTarget.id).catch(() => {});
      loadProviders();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await api.deleteProvider(deleteTarget.id);
      toast("success", `"${deleteTarget.name}" ${t("providers.deleted")}`);
      setDeleteTarget(null);
      loadProviders();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleSetActive = async (provider: ProviderView) => {
    try {
      await api.setActiveProvider(provider.id);
      toast("success", `"${provider.name}" ${t("providers.now_active")}`);
      loadProviders();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleTest = (provider: ProviderView) => {
    // 不再用 3 个 toast 串行——改为 TestConnectionDialog 实时显示三步进度
    // （连接 / capability autofill / cache detect）。dialog 自己管异步。
    setTestingId(provider.id);
    setTestingProvider(provider);
  };

  const handleTestDone = () => {
    setTestingId(null);
    setTestingProvider(null);
    loadProviders();
  };

  return (
    <div className="desktop-page">
      <div className="desktop-page-header flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-1 items-center gap-3">
          <p className="shrink-0 text-xs text-text-muted">
            {search
              ? `${filteredProviders.length} / ${providers.length}`
              : providers.length}{" "}
            {t("providers.configured")}
          </p>
          {/* provider 数 >=5 时显示搜索框——少于 5 个时直接看完更快 */}
          {providers.length >= 5 && (
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-text-muted" />
              <input
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t("providers.search_placeholder")}
                className="form-input w-full pl-8 text-xs"
              />
            </div>
          )}
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setLocalModelsOpen(true)}
            className="flex items-center gap-1.5 rounded-md border border-border bg-card-secondary px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-card hover:text-text-primary"
            title={t("providers.local_discover_hint")}
          >
            <HardDrive className="h-3.5 w-3.5" />
            {t("providers.local_discover")}
          </button>
          <button
            onClick={() => setSpeedtestOpen(true)}
            disabled={providers.length === 0}
            className="flex items-center gap-1.5 rounded-md border border-border bg-card-secondary px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:bg-card hover:text-text-primary disabled:opacity-40"
            title={t("providers.speedtest_button_tooltip")}
          >
            <Zap className="h-3.5 w-3.5" />
            {t("providers.speedtest_button")}
          </button>
          <button
            onClick={() => {
              setEditTarget(null);
              setFormOpen(true);
            }}
            className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-on-accent transition-colors hover:bg-accent/90"
          >
            <Plus className="h-3.5 w-3.5" />
            {t("providers.add")}
          </button>
        </div>
      </div>

      {loading ? (
        <p className="text-xs text-text-muted">{t("common.loading")}</p>
      ) : providers.length === 0 ? (
        <EmptyState
          icon={Inbox}
          title={t("providers.no_providers")}
          description={t("providers.add_first")}
          action={
            <button
              onClick={() => {
                setEditTarget(null);
                setFormOpen(true);
              }}
              className="btn-primary"
            >
              <Plus className="h-3.5 w-3.5" />
              {t("providers.add")}
            </button>
          }
        />
      ) : filteredProviders.length === 0 ? (
        <p className="text-xs text-text-muted">{t("providers.no_match")}</p>
      ) : (
        <div
          data-testid="provider-grid"
          className="grid grid-cols-1 gap-4 xl:grid-cols-2"
        >
          {filteredProviders.map((provider) => (
            <ProviderCard
              key={provider.id}
              provider={provider}
              onEdit={handleEdit}
              onDelete={setDeleteTarget}
              onSetActive={handleSetActive}
              onTest={handleTest}
              onDetails={handleDetails}
              testing={testingId === provider.id}
              runtime={runtimeMap[provider.id]}
              onResetRuntime={handleResetRuntime}
            />
          ))}
        </div>
      )}

      <ProviderFormDialog
        open={formOpen}
        provider={editTarget}
        onSubmit={editTarget ? handleUpdate : handleCreate}
        onClose={() => {
          setFormOpen(false);
          setEditTarget(null);
        }}
      />

      <ConfirmDialog
        open={!!deleteTarget}
        title={t("providers.delete")}
        message={`${t("providers.delete_confirm")} "${deleteTarget?.name}"? ${t("providers.delete_warn")}`}
        confirmLabel={t("common.delete")}
        variant="danger"
        onConfirm={handleDelete}
        onCancel={() => setDeleteTarget(null)}
      />

      <TestConnectionDialog
        provider={testingProvider}
        onClose={handleTestDone}
        onSuccess={loadProviders}
      />

      <SpeedtestDialog
        open={speedtestOpen}
        onClose={() => setSpeedtestOpen(false)}
      />

      <LocalModelsDialog
        open={localModelsOpen}
        onClose={() => setLocalModelsOpen(false)}
        onAdd={addLocalEndpoint}
      />
    </div>
  );
}
