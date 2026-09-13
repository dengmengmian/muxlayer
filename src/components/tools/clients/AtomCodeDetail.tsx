import { FolderOpen, Zap, ToggleLeft, ToggleRight } from "lucide-react";
import { StatusBadge } from "@/components/common/StatusBadge";
import { ClientHistoryButton } from "@/components/tools/ClientHistoryButton";
import * as api from "@/lib/api";
import type { AtomCodeConfigStatus } from "@/types/config";
import { DetailHeader, type T } from "@/pages/Tools";

export function AtomCodeDetail({
  status,
  onApply,
  onToggle,
  load,
  t,
}: {
  status: AtomCodeConfigStatus | null;
  onApply: () => void;
  onToggle: () => void;
  load: () => void;
  t: T;
}) {
  const badge = (
    <StatusBadge
      variant={
        status?.has_agentgate ? "success" : status?.exists ? "warning" : "muted"
      }
    >
      {status?.has_agentgate
        ? t("tools.agentgate_configured")
        : status?.exists
          ? t("tools.not_configured")
          : t("tools.no_config")}
    </StatusBadge>
  );

  return (
    <div className="surface-panel p-5">
      <DetailHeader
        clientId="atomcode"
        name={t("tools.atomcode")}
        desc={t("tools.atomcode_desc")}
        badge={badge}
      />

      {status && (
        <div className="mb-4 grid grid-cols-2 gap-y-2 text-xs">
          <div>
            <span className="text-text-muted">config.toml</span>
            <p className="font-mono text-text-secondary text-[11px]">
              {status.config_path}
            </p>
          </div>
          <div>
            <span className="text-text-muted">{t("logs.model")}</span>
            <p className="text-text-primary">{status.current_model ?? "—"}</p>
          </div>
        </div>
      )}

      <div className="flex flex-wrap gap-2">
        <button onClick={onApply} className="btn-primary">
          <Zap className="h-3 w-3" />
          {t("tools.apply_config")}
        </button>
        {status?.has_agentgate && status?.has_saved_official && (
          <button onClick={onToggle} className="btn-secondary">
            <ToggleRight className="h-3 w-3" />
            {t("tools.switch_to_official")}
          </button>
        )}
        {!status?.has_agentgate && status?.has_saved_official && (
          <button onClick={onToggle} className="btn-primary">
            <ToggleLeft className="h-3 w-3" />
            {t("tools.switch_to_agentgate")}
          </button>
        )}
        {status?.exists && (
          <button
            onClick={() => api.openAtomCodeConfig()}
            className="btn-secondary"
          >
            <FolderOpen className="h-3 w-3" />
            {t("tools.open")}
          </button>
        )}
        <ClientHistoryButton
          clientId="atomcode"
          clientName="AtomCode"
          onRollbackDone={load}
        />
      </div>
    </div>
  );
}
