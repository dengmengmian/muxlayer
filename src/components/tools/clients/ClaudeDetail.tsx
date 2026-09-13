import {
  Code,
  FolderOpen,
  AlertTriangle,
  Zap,
  Shield,
  ToggleLeft,
  ToggleRight,
} from "lucide-react";
import { StatusBadge } from "@/components/common/StatusBadge";
import { JsonCodeBlock } from "@/components/common/JsonCodeBlock";
import { ClientHistoryButton } from "@/components/tools/ClientHistoryButton";
import * as api from "@/lib/api";
import type { ClaudeCodeEnvStatus } from "@/types/config";
import { DetailHeader, type T } from "@/pages/Tools";

export function ClaudeDetail({
  env,
  snippet,
  onApply,
  onToggle,
  onGenerateSnippet,
  load,
  t,
}: {
  env: ClaudeCodeEnvStatus | null;
  snippet: string;
  onApply: () => void;
  onToggle: () => void;
  onGenerateSnippet: () => void;
  load: () => void;
  t: T;
}) {
  const badge = (
    <StatusBadge
      variant={
        env?.has_agentgate
          ? "success"
          : env?.has_api_key || env?.has_auth_token
            ? "warning"
            : "muted"
      }
    >
      {env?.has_agentgate
        ? t("tools.agentgate_configured")
        : env?.has_api_key || env?.has_auth_token
          ? t("tools.direct_credentials")
          : t("tools.no_credentials")}
    </StatusBadge>
  );

  return (
    <div className="surface-panel p-5">
      <DetailHeader
        clientId="claude_code"
        name={t("tools.claude_code")}
        desc={t("tools.claude_code_desc")}
        badge={badge}
      />

      {env && (
        <>
          <div className="mb-4 grid grid-cols-2 gap-y-2 text-xs">
            <div>
              <span className="text-text-muted">Settings Path</span>
              <p className="font-mono text-text-secondary text-[11px]">
                {env.settings_path}
              </p>
            </div>
            <div>
              <span className="text-text-muted">{t("settings.auth_mode")}</span>
              <p className="flex items-center gap-1 text-text-primary">
                <Shield className="h-3 w-3 text-accent" />
                {env.auth_mode}
              </p>
            </div>
            <div>
              <span className="text-text-muted">{t("providers.base_url")}</span>
              <p className="font-mono text-text-secondary">
                {env.active_base_url ?? "default"}
              </p>
            </div>
            <div>
              <span className="text-text-muted">{t("logs.model")}</span>
              <p className="font-mono text-text-primary">
                {env.active_model ?? "default"}
              </p>
            </div>
          </div>

          {env.conflicts.length > 0 && (
            <div className="mb-4 rounded-md border border-warning/30 bg-warning/5 p-3">
              <div className="flex items-center gap-2 text-xs font-medium text-warning">
                <AlertTriangle className="h-3.5 w-3.5" />
                {env.conflicts.length} {t("tools.conflicts_detected")}
              </div>
              {env.conflicts.map((c, i) => (
                <p key={i} className="mt-1 text-[11px] text-text-secondary">
                  {c}
                </p>
              ))}
            </div>
          )}
        </>
      )}

      <p className="mb-3 text-[11px] text-text-muted">
        {t("tools.claude_auth_desc")}
      </p>

      <div className="mb-4 flex flex-wrap gap-2">
        <button onClick={onApply} className="btn-primary">
          <Zap className="h-3 w-3" />
          {t("tools.apply_config")}
        </button>
        {env?.has_agentgate && env?.has_saved_official && (
          <button onClick={onToggle} className="btn-secondary">
            <ToggleRight className="h-3 w-3" />
            {t("tools.switch_to_official")}
          </button>
        )}
        {!env?.has_agentgate && env?.has_saved_official && (
          <button onClick={onToggle} className="btn-primary">
            <ToggleLeft className="h-3 w-3" />
            {t("tools.switch_to_agentgate")}
          </button>
        )}
        {env?.settings_exists && (
          <button
            onClick={() => api.openClaudeCodeConfig()}
            className="btn-secondary"
          >
            <FolderOpen className="h-3 w-3" />
            {t("tools.open")}
          </button>
        )}
        <ClientHistoryButton
          clientId="claude_code"
          clientName="Claude Code"
          onRollbackDone={load}
        />
        <button onClick={onGenerateSnippet} className="btn-secondary">
          <Code className="h-3 w-3" />
          {t("tools.env_snippet")}
        </button>
      </div>

      {snippet && (
        <JsonCodeBlock
          title="Claude Code Environment"
          content={snippet}
          language="bash"
        />
      )}
    </div>
  );
}
