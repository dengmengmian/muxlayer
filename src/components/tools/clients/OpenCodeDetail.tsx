import { FolderOpen, Zap } from "lucide-react";
import { StatusBadge } from "@/components/common/StatusBadge";
import { ClientHistoryButton } from "@/components/tools/ClientHistoryButton";
import * as api from "@/lib/api";
import type { OpenCodeConfigStatus } from "@/types/config";
import { DetailHeader, type T } from "@/pages/Tools";

export function OpenCodeDetail({
  status,
  onApply,
  load,
  t,
}: {
  status: OpenCodeConfigStatus | null;
  onApply: () => void;
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
        clientId="opencode"
        name={t("tools.opencode")}
        desc={t("tools.opencode_desc")}
        badge={badge}
      />

      {status && (
        <div className="mb-4 grid grid-cols-2 gap-y-2 text-xs">
          <div>
            <span className="text-text-muted">opencode.json</span>
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

      <p className="mb-3 text-[11px] text-text-muted">
        {t("tools.opencode_auth_desc")}
      </p>

      <div className="flex flex-wrap gap-2">
        <button onClick={onApply} className="btn-primary">
          <Zap className="h-3 w-3" />
          {t("tools.apply_config")}
        </button>
        {status?.exists && (
          <button
            onClick={() => api.openOpenCodeConfig()}
            className="btn-secondary"
          >
            <FolderOpen className="h-3 w-3" />
            {t("tools.open")}
          </button>
        )}
        <ClientHistoryButton
          clientId="opencode"
          clientName="OpenCode"
          onRollbackDone={load}
        />
      </div>
    </div>
  );
}
