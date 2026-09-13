import { useState, useEffect, useCallback, useRef } from "react";
import { useSearchParams } from "react-router-dom";
import { events } from "@/lib/bindings";
import {
  Shield,
  FolderOpen,
  RefreshCcw,
  Download,
  Plus,
  Trash2,
  Settings2,
  Database,
  Info,
  PawPrint,
  ChevronDown,
  ChevronRight,
  Share2,
  Upload,
} from "lucide-react";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { ConfirmDialog } from "@/components/common/ConfirmDialog";
import { toast } from "@/components/common/Toast";
import { GeneralTab } from "@/components/settings/GeneralTab";
import { SecurityTab } from "@/components/settings/SecurityTab";
import { DataTab } from "@/components/settings/DataTab";
import { PetTab } from "@/components/settings/PetTab";
import { AboutTab } from "@/components/settings/AboutTab";
import { useI18n } from "@/lib/i18n";
import { readStoredTheme, type AppTheme } from "@/lib/theme";
import { usePolling } from "@/lib/usePolling";
import * as api from "@/lib/api";
import {
  outboundProxyToastKey,
  shouldRestartGatewayForProxy,
} from "@/lib/outboundProxy";
import {
  useGatewaySettings,
  useGatewayStatus,
  usePricing,
} from "@/store/global";
import type { GatewaySettings as GatewaySettingsType } from "@/types/gateway";
import type { WakeStatus } from "@/lib/bindings";
import type { GatewayAuthSettings } from "@/types/config";
import type { ModelPricing } from "@/types/stats";
import type { PetType, PetSettings as PetSettingsType } from "@/types/pet";
import { RobotPet } from "@/pet/pets/RobotPet";
import { PixelCat } from "@/pet/pets/PixelCat";
import { SlimePet } from "@/pet/pets/SlimePet";
import { FoxPet } from "@/pet/pets/FoxPet";
import { OctopusPet } from "@/pet/pets/OctopusPet";
import { GhostPet } from "@/pet/pets/GhostPet";
import { OxPet } from "@/pet/pets/OxPet";
import { SuperSoldierPet } from "@/pet/pets/SuperSoldierPet";
import { CoderPet } from "@/pet/pets/CoderPet";

type Tab = "general" | "security" | "data" | "pet" | "about";

// Gateway 标签曾经在这里，但与顶级"服务"页面（src/pages/Gateway.tsx）
// 语义重叠（都是 host/port/protocol 配置），且这里是只读、那边可改——
// 用户混淆。删掉这里的 tab，统一去顶级"服务"页编辑。
const TABS: { id: Tab; icon: React.ComponentType<{ className?: string }> }[] = [
  { id: "general", icon: Settings2 },
  { id: "security", icon: Shield },
  { id: "data", icon: Database },
  { id: "pet", icon: PawPrint },
  { id: "about", icon: Info },
];

const VALID_TABS: readonly Tab[] = [
  "general",
  "security",
  "data",
  "pet",
  "about",
];

export function Settings() {
  const { t, locale, setLocale } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const initialTab = (() => {
    const q = searchParams.get("tab") as Tab | null;
    return q && VALID_TABS.includes(q) ? q : "general";
  })();
  const [tab, setTabState] = useState<Tab>(initialTab);
  // 同一个 Settings 页面已经挂载时,从宠物菜单跳进来 query 变了但组件不重挂——
  // 监听 query 主动切 tab。
  useEffect(() => {
    const q = searchParams.get("tab") as Tab | null;
    if (q && VALID_TABS.includes(q)) setTabState(q);
  }, [searchParams]);
  const setTab = useCallback(
    (next: Tab) => {
      setTabState(next);
      if (searchParams.get("tab")) {
        const sp = new URLSearchParams(searchParams);
        sp.delete("tab");
        setSearchParams(sp, { replace: true });
      }
    },
    [searchParams, setSearchParams]
  );
  const [theme, setThemeState] = useState(() => readStoredTheme());
  // gateway settings / pricing 走全局 store——跨页只读,Gateway 页改 host:port
  // 后 store.refetch() 同步给这里。
  const settings = useGatewaySettings(
    (s) => s.value
  ) as GatewaySettingsType | null;
  const pricing = usePricing((s) => s.items);
  const setPricing = (
    next: ModelPricing[] | ((prev: ModelPricing[]) => ModelPricing[])
  ) => {
    const value =
      typeof next === "function"
        ? (next as (prev: ModelPricing[]) => ModelPricing[])(
            usePricing.getState().items
          )
        : next;
    usePricing.getState().setItems(value);
  };
  const [auth, setAuth] = useState<GatewayAuthSettings | null>(null);
  const [confirmRegen, setConfirmRegen] = useState(false);
  const [petSettings, setPetSettings] = useState<PetSettingsType | null>(null);
  const [petClickThrough, setPetClickThroughState] = useState(false);
  const [wakeStatus, setWakeStatus] = useState<WakeStatus | null>(null);
  useEffect(() => {
    api
      .getPetClickThrough()
      .then(setPetClickThroughState)
      .catch(() => {});
    const un = events.petClickThroughChanged.listen((e) =>
      setPetClickThroughState(e.payload)
    );
    return () => {
      un.then((fn) => fn());
    };
  }, []);
  const handlePetClickThroughChange = useCallback((v: boolean) => {
    api
      .setPetClickThrough(v)
      .catch((err) => toast("error", (err as api.AppError).message));
  }, []);
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    getVersion()
      .then(setAppVersion)
      .catch(() => {});
  }, []);

  const load = useCallback(async () => {
    try {
      // gateway settings / pricing 走 store.refetch()——确保 mutation 后值是新的;
      // auth / pet 是这页专属、暂不进 store。
      const [a, pet, wake] = await Promise.all([
        api.getGatewayAuthSettings(),
        api.getPetSettings(),
        api.getWakeStatus(),
        useGatewaySettings.getState().refetch(),
        usePricing.getState().refetch(),
      ]);
      setAuth(a);
      setPetSettings(pet);
      setWakeStatus(wake);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // 唤醒状态只在 General tab（WakeSettings 所在）可见时刷新；usePolling 在窗口
  // 隐藏时跳过、回到前台时立即补一次。
  const refreshWakeStatus = useCallback(() => {
    if (tab !== "general") return;
    api
      .getWakeStatus()
      .then(setWakeStatus)
      .catch((error) => console.warn("Failed to refresh wake status", error));
  }, [tab]);
  usePolling(refreshWakeStatus, 5000);
  // 从其它 tab 切回 General 时立即补一次，不等下一个 5s 周期；首次挂载已由 load() 拉过。
  const prevTabRef = useRef(tab);
  useEffect(() => {
    if (tab === "general" && prevTabRef.current !== "general") {
      refreshWakeStatus();
    }
    prevTabRef.current = tab;
  }, [tab, refreshWakeStatus]);

  const handleUpdateRetention = async (days: number) => {
    try {
      await api.updateGatewaySettings({ log_retention_days: days });
      toast("success", t("settings.updated"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleUpdateAutoStart = async (val: boolean) => {
    try {
      await api.updateGatewaySettings({ auto_start: val });
      toast("success", t("settings.updated"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  // 全局网关开关更新（多个独立开关共用一个 handler，通过 key 区分）
  const handleUpdateRefinerGlobal = async (
    key:
      | "body_filter_global"
      | "thinking_rectifier_global"
      | "error_mapper_global"
      | "health_probe_enabled",
    val: boolean
  ) => {
    try {
      await api.updateGatewaySettings({ [key]: val });
      toast("success", t("settings.updated"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleUpdateCostAlert = async (patch: {
    cost_alert_enabled?: boolean;
    cost_alert_threshold?: number;
    cost_budget_enabled?: boolean;
    cost_budget_threshold?: number;
    cost_budget_strategy?: string;
    auto_compact_enabled?: boolean;
    auto_compact_usage_percent?: number;
  }) => {
    try {
      await api.updateGatewaySettings(patch);
      toast("success", t("settings.updated"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleUpdateRequestBodyLimit = async (mb: number) => {
    try {
      await api.updateGatewaySettings({ request_body_limit_mb: mb });
      toast("success", t("settings.updated"));
      load();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleUpdateProxy = async (patch: {
    outbound_proxy_enabled?: boolean;
    outbound_proxy_url?: string;
  }) => {
    try {
      await api.updateGatewaySettings(patch);
      await useGatewaySettings.getState().refetch();
      const after = useGatewaySettings.getState().value;
      const hasUrl = Boolean(
        (patch.outbound_proxy_url ?? after?.outbound_proxy_url ?? "").trim()
      );
      const running = Boolean(useGatewayStatus.getState().value?.running);
      const toastKey = outboundProxyToastKey(running, patch, hasUrl);
      if (shouldRestartGatewayForProxy(running, patch, hasUrl)) {
        try {
          useGatewayStatus.getState().setValue(await api.restartGateway());
        } catch (err) {
          toast("error", (err as api.AppError).message);
          return;
        }
      }
      toast("success", t(toastKey));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleUpdateWake = async (patch: {
    wake_enabled?: boolean;
    wake_request_control?: boolean;
    wake_cooldown_seconds?: number;
    wake_keep_display_awake?: boolean;
  }) => {
    try {
      await api.updateGatewaySettings(patch);
      const [, status] = await Promise.all([
        useGatewaySettings.getState().refetch(),
        api.getWakeStatus(),
      ]);
      setWakeStatus(status);
      toast("success", t("settings.updated"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleCopyToken = async () => {
    try {
      const token = await api.getLocalAccessToken();
      await navigator.clipboard.writeText(token);
      toast("success", t("settings.token_copied"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleRegenToken = async () => {
    try {
      const a = await api.regenerateLocalAccessToken();
      setAuth(a);
      toast("success", t("settings.regen_done"));
      setConfirmRegen(false);
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handlePetTypeChange = async (type: PetType) => {
    try {
      const updated = await api.updatePetSettings({ pet_type: type });
      setPetSettings(updated);
      toast("success", t("settings.updated"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handlePetVisibleChange = async (visible: boolean) => {
    try {
      const updated = await api.setPetVisible(visible);
      setPetSettings(updated);
      toast("success", t("settings.updated"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const setTheme = (t: AppTheme) => {
    setThemeState(t);
    document.documentElement.setAttribute("data-theme", t);
    localStorage.setItem("agentgate_theme", t);
  };

  if (!settings)
    return (
      <div className="space-y-5">
        <div className="flex gap-2">
          {Array.from({ length: 5 }).map((_, i) => (
            <div key={i} className="skeleton h-9 w-24 rounded-lg" />
          ))}
        </div>
        <div className="skeleton mx-auto h-80 max-w-[860px] rounded-lg" />
      </div>
    );

  return (
    <div className="desktop-page mx-auto min-h-0 w-full max-w-[1100px]">
      {/* Tab Navigation */}
      <nav className="desktop-page-header surface-scroll sticky top-0 z-10 !p-1">
        <div className="inline-flex rounded-lg border border-border bg-card-secondary p-1">
          {TABS.map(({ id, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setTab(id)}
              className={`flex items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors ${
                tab === id
                  ? "bg-accent-soft text-accent font-medium"
                  : "text-text-secondary hover:bg-hover hover:text-text-primary"
              }`}
            >
              <Icon className="h-4 w-4 shrink-0" />
              {t(`settings.tab.${id}`)}
            </button>
          ))}
        </div>
      </nav>

      {/* Content */}
      <div className="min-w-0 space-y-6">
        {tab === "general" && (
          <GeneralTab
            settings={settings}
            locale={locale}
            setLocale={setLocale}
            theme={theme}
            setTheme={setTheme}
            handleUpdateAutoStart={handleUpdateAutoStart}
            handleUpdateRefinerGlobal={handleUpdateRefinerGlobal}
            handleUpdateCostAlert={handleUpdateCostAlert}
            handleUpdateRequestBodyLimit={handleUpdateRequestBodyLimit}
            handleUpdateProxy={handleUpdateProxy}
            wakeStatus={wakeStatus}
            handleUpdateWake={handleUpdateWake}
            t={t}
            ToggleSwitch={ToggleSwitch}
            ThemePicker={ThemePicker}
          />
        )}

        {tab === "security" && auth && (
          <SecurityTab
            auth={auth}
            handleCopyToken={handleCopyToken}
            setConfirmRegen={setConfirmRegen}
            t={t}
          />
        )}

        {tab === "data" && (
          <DataTab
            settings={settings}
            pricing={pricing}
            setPricing={setPricing}
            handleUpdateRetention={handleUpdateRetention}
            t={t}
          />
        )}

        {tab === "pet" && petSettings && (
          <PetTab
            petSettings={petSettings}
            petClickThrough={petClickThrough}
            handlePetVisibleChange={handlePetVisibleChange}
            handlePetClickThroughChange={handlePetClickThroughChange}
            handlePetTypeChange={handlePetTypeChange}
            t={t}
            ToggleSwitch={ToggleSwitch}
          />
        )}

        {tab === "about" && <AboutTab appVersion={appVersion} t={t} />}
      </div>

      <ConfirmDialog
        open={confirmRegen}
        title={t("settings.regen_title")}
        message={t("settings.regen_msg")}
        confirmLabel={t("settings.regenerate_token")}
        variant="danger"
        onConfirm={handleRegenToken}
        onCancel={() => setConfirmRegen(false)}
      />
    </div>
  );
}

// ── Pet Type Card ──

const PET_PREVIEWS: Record<PetType, React.ComponentType<{ state: "idle" }>> = {
  robot: RobotPet,
  "pixel-cat": PixelCat,
  slime: SlimePet,
  fox: FoxPet,
  octopus: OctopusPet,
  ghost: GhostPet,
  ox: OxPet,
  soldier: SuperSoldierPet,
  coder: CoderPet,
};

/// 把配置 JSON 编码成一行可复制的「分享码」(UTF-8 → base64),方便发给对方粘贴导入。
function encodeShareCode(json: string): string {
  const bytes = new TextEncoder().encode(json);
  let bin = "";
  bytes.forEach((b) => {
    bin += String.fromCharCode(b);
  });
  return btoa(bin);
}
function decodeShareCode(code: string): string {
  const bin = atob(code.trim());
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

/// 配置导入/导出 section。语义是 replace（导入 = 覆盖），所以导入前用
/// ConfirmDialog 弹窗确认。API key 默认**不导出**——把含密钥的 JSON 文件
/// 随手发到群里/截图/丢仓库是常见泄密路径，所以默认安全；用户明确勾选
/// "含密钥"才会写入，仅用于本机迁移。
export function ConfigBackupSection() {
  const { t } = useI18n();
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [pendingImportJson, setPendingImportJson] = useState<string | null>(
    null
  );
  const [importing, setImporting] = useState(false);
  const [shareCodeInput, setShareCodeInput] = useState("");

  const handleExport = async () => {
    try {
      const json = await api.exportConfigJson(includeSecrets);
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const ts = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
      const a = document.createElement("a");
      a.href = url;
      a.download = `agentgate-config-${ts}${includeSecrets ? "-with-secrets" : ""}.json`;
      a.click();
      URL.revokeObjectURL(url);
      toast(
        "success",
        includeSecrets
          ? t("settings.exported_with_secrets")
          : t("settings.exported_no_secrets")
      );
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  // 分享码绝不带密钥——它就是给"发给别人"用的,带 key 等于泄密。
  const handleCopyShareCode = async () => {
    try {
      const json = await api.exportConfigJson(false);
      await navigator.clipboard.writeText(encodeShareCode(json));
      toast("success", t("settings.share_code_copied"));
    } catch (err) {
      toast("error", (err as api.AppError).message);
    }
  };

  const handleImportShareCode = () => {
    const code = shareCodeInput.trim();
    if (!code) return;
    try {
      const json = decodeShareCode(code);
      JSON.parse(json); // 校验是合法 JSON，再走和文件导入一样的确认弹窗
      setPendingImportJson(json);
    } catch {
      toast("error", t("settings.share_code_invalid"));
    }
  };

  const handleFilePicked = async (file: File) => {
    try {
      const text = await file.text();
      // 触发确认弹窗前先校验是 MuxLayer 格式，避免误导入随机 JSON。
      JSON.parse(text);
      setPendingImportJson(text);
    } catch {
      toast("error", t("settings.import_invalid_json"));
    }
  };

  const handleConfirmImport = async () => {
    if (!pendingImportJson) return;
    setImporting(true);
    try {
      const summary = await api.importConfigJson(pendingImportJson);
      toast(
        "success",
        `${t("settings.imported_prefix")} ${summary.providers_imported} ${t("settings.imported_providers")}、${summary.route_profiles_imported} ${t("settings.imported_route_profiles")}${
          summary.secrets_applied
            ? t("settings.imported_with_secrets")
            : t("settings.imported_no_secrets")
        }`
      );
      setPendingImportJson(null);
      setShareCodeInput("");
      // 让上层数据刷新——最稳的是 reload 一次。
      window.location.reload();
    } catch (err) {
      toast("error", (err as api.AppError).message);
    } finally {
      setImporting(false);
    }
  };

  return (
    <section className="surface-panel p-5">
      <h3 className="mb-1 text-sm font-semibold text-text-primary">
        {t("settings.config_backup")}
      </h3>
      <p className="mb-4 text-xs text-text-muted">
        {t("settings.config_backup_desc_1")}
        <span className="text-warning">
          {t("settings.config_backup_replace")}
        </span>
        {t("settings.config_backup_desc_2")}
      </p>

      <div className="space-y-3">
        <label className="flex items-center gap-2 text-xs text-text-secondary">
          <input
            type="checkbox"
            checked={includeSecrets}
            onChange={(e) => setIncludeSecrets(e.target.checked)}
            className="h-3.5 w-3.5 rounded border-border bg-card-secondary accent-accent"
          />
          {t("settings.include_secrets")}
        </label>

        <div className="flex flex-wrap items-center gap-2">
          <button
            onClick={handleExport}
            className="btn-primary inline-flex items-center gap-1.5 text-xs"
          >
            <Download className="h-3.5 w-3.5" />
            {t("settings.export_config")}
          </button>
          <button
            onClick={handleCopyShareCode}
            className="btn-secondary inline-flex items-center gap-1.5 text-xs"
          >
            <Share2 className="h-3.5 w-3.5" />
            {t("settings.copy_share_code")}
          </button>
          <label className="btn-secondary inline-flex cursor-pointer items-center gap-1.5 text-xs">
            <FolderOpen className="h-3.5 w-3.5" />
            {t("settings.import_from_file")}
            <input
              type="file"
              accept="application/json,.json"
              className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (f) handleFilePicked(f);
                e.target.value = "";
              }}
            />
          </label>
        </div>

        {/* 分享码导入：对方「复制分享码」后,把那串文本粘到这里导入 */}
        <div className="space-y-2 border-t border-border pt-3">
          <p className="text-xs text-text-muted">
            {t("settings.import_share_code_hint")}
          </p>
          <div className="flex items-start gap-2">
            <textarea
              value={shareCodeInput}
              onChange={(e) => setShareCodeInput(e.target.value)}
              placeholder={t("settings.share_code_placeholder")}
              rows={2}
              className="form-input flex-1 resize-none font-mono text-xs"
            />
            <button
              onClick={handleImportShareCode}
              disabled={!shareCodeInput.trim()}
              className="btn-secondary inline-flex shrink-0 items-center gap-1.5 text-xs disabled:opacity-40"
            >
              <Upload className="h-3.5 w-3.5" />
              {t("settings.import")}
            </button>
          </div>
        </div>
      </div>

      <ConfirmDialog
        open={!!pendingImportJson}
        variant="danger"
        title={t("settings.import_confirm_title")}
        message={t("settings.import_confirm_msg")}
        confirmLabel={
          importing ? t("settings.importing") : t("settings.import_confirm")
        }
        onConfirm={handleConfirmImport}
        onCancel={() => setPendingImportJson(null)}
      />
    </section>
  );
}

/// 可折叠 section：header 永远显示，body 默认折叠、点击展开。用于低频
/// 但占空间的设置（如模型定价表）——避免它把日常常用的"日志保留/配置
/// 导入导出"挤到下面要滚屏才能看见。
export function CollapsibleSection({
  icon: Icon,
  title,
  hint,
  badge,
  children,
}: {
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  hint?: string;
  badge?: string;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <section className="surface-panel p-5">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full items-center justify-between gap-3 text-left"
      >
        <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
          <Icon className="h-4 w-4 text-accent" />
          {title}
          {badge && (
            <span className="rounded-full bg-card-secondary px-1.5 py-0.5 text-xs font-normal text-text-muted">
              {badge}
            </span>
          )}
        </div>
        {open ? (
          <ChevronDown className="h-4 w-4 text-text-muted" />
        ) : (
          <ChevronRight className="h-4 w-4 text-text-muted" />
        )}
      </button>
      {hint && open && <p className="mt-3 text-xs text-text-muted">{hint}</p>}
      {open && <div className="mt-4">{children}</div>}
    </section>
  );
}

export function PetTypeCard({
  type,
  selected,
  name,
  desc,
  onClick,
}: {
  type: PetType;
  selected: boolean;
  name: string;
  desc: string;
  onClick: () => void;
}) {
  const Preview = PET_PREVIEWS[type];
  return (
    <button
      onClick={onClick}
      className={`flex flex-col items-center gap-2 rounded-lg border p-4 transition-all duration-150 hover:scale-[1.02] ${
        selected
          ? "border-accent bg-accent/5"
          : "border-border bg-card-secondary hover:border-text-muted"
      }`}
    >
      <div className="h-16 w-16 flex items-center justify-center">
        <div className="scale-[0.55] origin-center">
          <Preview state="idle" />
        </div>
      </div>
      <div className="text-center">
        <p
          className={`text-xs font-medium ${selected ? "text-accent" : "text-text-primary"}`}
        >
          {name}
        </p>
        <p className="mt-0.5 text-xs text-text-muted leading-tight">{desc}</p>
      </div>
      {selected && (
        <span className="text-xs text-accent font-medium">● Active</span>
      )}
    </button>
  );
}

// ── Shared Components ──

function ToggleSwitch({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (val: boolean) => void;
  label: string;
}) {
  return (
    <label className="relative inline-flex cursor-pointer items-center">
      <input
        type="checkbox"
        aria-label={label}
        className="peer sr-only"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <div className="h-5 w-9 rounded-full bg-border transition-colors after:absolute after:left-[2px] after:top-[2px] after:h-4 after:w-4 after:rounded-full after:bg-text-muted after:transition-all peer-checked:bg-accent peer-checked:after:translate-x-full peer-checked:after:bg-white peer-focus-visible:ring-2 peer-focus-visible:ring-accent peer-focus-visible:ring-offset-2 peer-focus-visible:ring-offset-card" />
    </label>
  );
}

export function CheckUpdateButton({ t }: { t: (key: string) => string }) {
  const [checking, setChecking] = useState(false);
  const [status, setStatus] = useState<"idle" | "latest" | "available">("idle");
  const [newVersion, setNewVersion] = useState("");

  const handleCheck = async () => {
    setChecking(true);
    setStatus("idle");
    try {
      const update = await check();
      if (update) {
        setStatus("available");
        setNewVersion(update.version);
      } else {
        setStatus("latest");
      }
    } catch {
      setStatus("latest");
    } finally {
      setChecking(false);
    }
  };

  const [installing, setInstalling] = useState(false);

  const handleInstall = async () => {
    setInstalling(true);
    try {
      const update = await check();
      if (!update) {
        setInstalling(false);
        return;
      }
      await update.downloadAndInstall();
      // Auto-relaunch into the freshly installed version.
      toast("success", t("update.relaunching"));
      await new Promise((r) => setTimeout(r, 800));
      await relaunch();
    } catch {
      toast("error", t("update.install_failed"));
      setInstalling(false);
    }
  };

  return (
    <div className="flex items-center gap-2">
      {status === "available" ? (
        <button
          onClick={handleInstall}
          disabled={installing}
          className="btn-primary"
        >
          <Download className="h-3 w-3" />
          {installing
            ? t("update.installing")
            : `${t("update.now")} v${newVersion}`}
        </button>
      ) : (
        <button
          onClick={handleCheck}
          disabled={checking}
          className="btn-secondary"
        >
          <RefreshCcw className={`h-3 w-3 ${checking ? "animate-spin" : ""}`} />
          {checking ? t("update.checking") : t("update.check")}
        </button>
      )}
      {status === "latest" && (
        <span className="text-xs text-green-400">{t("update.is_latest")}</span>
      )}
    </div>
  );
}

export function PricingRow({
  item,
  onUpdate,
  onDelete,
}: {
  item: ModelPricing;
  onUpdate: (inputPrice: number, outputPrice: number) => void;
  onDelete: () => void;
}) {
  const { t } = useI18n();
  const [editInput, setEditInput] = useState(String(item.input_price));
  const [editOutput, setEditOutput] = useState(String(item.output_price));
  const [editing, setEditing] = useState(false);

  const save = () => {
    const inp = parseFloat(editInput);
    const outp = parseFloat(editOutput);
    if (
      !isNaN(inp) &&
      !isNaN(outp) &&
      (inp !== item.input_price || outp !== item.output_price)
    ) {
      onUpdate(inp, outp);
    }
    setEditing(false);
  };

  return (
    <tr className="border-b border-border/50 last:border-0">
      <td className="px-3 py-1.5 text-text-primary">{item.provider}</td>
      <td className="px-3 py-1.5 font-mono text-text-secondary">
        {item.model_pattern}
      </td>
      <td className="px-3 py-1.5 text-right">
        {editing ? (
          <input
            type="number"
            step="0.01"
            value={editInput}
            onChange={(e) => setEditInput(e.target.value)}
            onBlur={save}
            onKeyDown={(e) => e.key === "Enter" && save()}
            className="w-20 rounded border border-accent/50 bg-bg px-1.5 py-0.5 text-right text-xs text-text-primary outline-none"
            autoFocus
          />
        ) : (
          <span
            className="cursor-pointer text-text-primary hover:text-accent"
            onClick={() => setEditing(true)}
          >
            {item.input_price.toFixed(2)}
          </span>
        )}
      </td>
      <td className="px-3 py-1.5 text-right">
        {editing ? (
          <input
            type="number"
            step="0.01"
            value={editOutput}
            onChange={(e) => setEditOutput(e.target.value)}
            onBlur={save}
            onKeyDown={(e) => e.key === "Enter" && save()}
            className="w-20 rounded border border-accent/50 bg-bg px-1.5 py-0.5 text-right text-xs text-text-primary outline-none"
          />
        ) : (
          <span
            className="cursor-pointer text-text-primary hover:text-accent"
            onClick={() => setEditing(true)}
          >
            {item.output_price.toFixed(2)}
          </span>
        )}
      </td>
      <td className="px-3 py-1.5 text-center">
        <span
          className={`rounded px-1.5 py-0.5 text-xs ${item.is_custom ? "bg-accent-soft text-accent" : "bg-card-secondary text-text-muted"}`}
        >
          {item.is_custom ? t("settings.custom") : t("settings.builtin")}
        </span>
      </td>
      <td className="px-3 py-1.5 text-center">
        {item.is_custom && (
          <button
            onClick={onDelete}
            className="text-text-muted hover:text-error"
          >
            <Trash2 className="h-3 w-3" />
          </button>
        )}
      </td>
    </tr>
  );
}

export function PricingAddForm({
  onAdd,
}: {
  onAdd: (
    provider: string,
    model: string,
    inputPrice: number,
    outputPrice: number
  ) => void;
}) {
  const { t } = useI18n();
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [inputPrice, setInputPrice] = useState("");
  const [outputPrice, setOutputPrice] = useState("");

  const handleSubmit = () => {
    if (!provider.trim() || !model.trim()) return;
    const inp = parseFloat(inputPrice);
    const outp = parseFloat(outputPrice);
    if (isNaN(inp) || isNaN(outp)) return;
    onAdd(provider.trim(), model.trim(), inp, outp);
    setProvider("");
    setModel("");
    setInputPrice("");
    setOutputPrice("");
  };

  return (
    <div className="mt-3 flex items-end gap-2">
      <div className="flex-1">
        <label className="mb-1 block text-xs text-text-muted">Provider</label>
        <input
          value={provider}
          onChange={(e) => setProvider(e.target.value)}
          placeholder="deepseek"
          className="form-input w-full"
        />
      </div>
      <div className="flex-1">
        <label className="mb-1 block text-xs text-text-muted">Model</label>
        <input
          value={model}
          onChange={(e) => setModel(e.target.value)}
          placeholder="model-name or *"
          className="form-input w-full"
        />
      </div>
      <div className="w-24">
        <label className="mb-1 block text-xs text-text-muted">Input $/1M</label>
        <input
          type="number"
          step="0.01"
          value={inputPrice}
          onChange={(e) => setInputPrice(e.target.value)}
          placeholder="0.00"
          className="form-input w-full"
        />
      </div>
      <div className="w-24">
        <label className="mb-1 block text-xs text-text-muted">
          Output $/1M
        </label>
        <input
          type="number"
          step="0.01"
          value={outputPrice}
          onChange={(e) => setOutputPrice(e.target.value)}
          placeholder="0.00"
          className="form-input w-full"
        />
      </div>
      <button onClick={handleSubmit} className="btn-primary mb-0.5">
        <Plus className="h-3 w-3" />
        {t("routes.add")}
      </button>
    </div>
  );
}

// ── Theme picker — swatch grid ──────────────────────────────────────
// Each card previews the theme's surface + accent + text triplet so the
// user can pick by sight, not by name guessing. The check overlay marks
// the active theme.

interface ThemeSwatch {
  id: AppTheme; // localStorage value + data-theme attribute
  labelEn: string;
  labelZh: string;
  bg: string;
  card: string;
  accent: string;
  textPrimary: string;
  border: string;
}

const THEME_SWATCHES: ThemeSwatch[] = [
  {
    id: "dark",
    labelEn: "Dark",
    labelZh: "深色",
    bg: "#0B1516",
    card: "#132122",
    accent: "#2DD4BF",
    textPrimary: "#E6F1F0",
    border: "#2B4142",
  },
  {
    id: "light",
    labelEn: "Light",
    labelZh: "浅色",
    bg: "#F3F7F6",
    card: "#FFFFFF",
    accent: "#0F766E",
    textPrimary: "#142322",
    border: "#CCDAD8",
  },
];

function ThemePicker({
  value,
  onChange,
}: {
  value: AppTheme;
  onChange: (id: AppTheme) => void;
}) {
  return (
    <div className="grid max-w-md grid-cols-2 gap-3">
      {THEME_SWATCHES.map((s) => {
        const selected = value === s.id;
        return (
          <button
            key={s.id}
            type="button"
            onClick={() => onChange(s.id)}
            className={`group rounded-lg border p-2 text-left transition-all ${
              selected
                ? "border-accent ring-2 ring-accent/40"
                : "border-border hover:border-text-muted"
            }`}
            style={{ backgroundColor: s.bg }}
            aria-pressed={selected}
          >
            {/* Mini preview: a faux card with accent dot + text bars */}
            <div
              className="mb-1.5 flex items-center gap-2 rounded-md border px-2 py-1.5"
              style={{ backgroundColor: s.card, borderColor: s.border }}
            >
              <span
                className="h-2.5 w-2.5 shrink-0 rounded-full"
                style={{ backgroundColor: s.accent }}
              />
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <span
                  className="h-1 w-3/4 rounded-full"
                  style={{ backgroundColor: s.textPrimary, opacity: 0.85 }}
                />
                <span
                  className="h-1 w-1/2 rounded-full"
                  style={{ backgroundColor: s.textPrimary, opacity: 0.45 }}
                />
              </div>
            </div>
            <div className="flex items-baseline justify-between gap-2">
              <span
                className="text-xs font-medium"
                style={{ color: s.textPrimary }}
              >
                {s.labelZh}
              </span>
              <span
                className="text-xs"
                style={{ color: s.textPrimary, opacity: 0.6 }}
              >
                {s.labelEn}
              </span>
            </div>
          </button>
        );
      })}
    </div>
  );
}
