import { FolderOpen, Zap } from "lucide-react";
import { StatusBadge } from "@/components/common/StatusBadge";
import { ClientHistoryButton } from "@/components/tools/ClientHistoryButton";
import * as api from "@/lib/api";
import type { DeepSeekHarnessConfigStatus } from "@/types/config";
import { DetailHeader, type T } from "@/pages/Tools";

export function DeepSeekHarnessDetail({
  status,
  onApply,
  load,
  t,
}: {
  status: DeepSeekHarnessConfigStatus | null;
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
        clientId="deepseek_harness"
        name={t("tools.deepseek_harness")}
        desc={t("tools.deepseek_harness_desc")}
        badge={badge}
      />

      {status && (
        <div className="mb-4 grid grid-cols-2 gap-y-2 text-xs">
          <div>
            <span className="text-text-muted">settings.yaml</span>
            <p className="font-mono text-[11px] text-text-secondary">
              {status.config_path}
            </p>
          </div>
        </div>
      )}

      <p className="mb-3 text-[11px] text-text-muted">
        {t("tools.deepseek_harness_auth_desc")}
      </p>

      <div className="flex flex-wrap gap-2">
        <button onClick={onApply} className="btn-primary">
          <Zap className="h-3 w-3" />
          {t("tools.apply_config")}
        </button>
        {status?.exists && (
          <button onClick={() => api.openDshConfig()} className="btn-secondary">
            <FolderOpen className="h-4 w-4" />
            {t("tools.open")}
          </button>
        )}
        <ClientHistoryButton
          clientId="deepseek_harness"
          clientName="DeepSeek Harness"
          onRollbackDone={load}
        />
      </div>
    </div>
  );
}
