import { Shield, FolderOpen, RefreshCcw, Copy } from "lucide-react";
import * as api from "@/lib/api";
import type { GatewayAuthSettings } from "@/types/config";

interface Props {
  auth: GatewayAuthSettings;
  handleCopyToken: () => Promise<void>;
  setConfirmRegen: (v: boolean) => void;
  t: (key: string) => string;
}

export function SecurityTab({
  auth,
  handleCopyToken,
  setConfirmRegen,
  t,
}: Props) {
  return (
    <section className="surface-panel p-5">
      <h3 className="mb-4 flex items-center gap-2 text-sm font-semibold text-text-primary">
        <Shield className="h-4 w-4 text-accent" />
        {t("settings.gateway_security")}
      </h3>
      <div className="space-y-3 text-xs">
        <div className="flex justify-between">
          <span className="text-text-muted">{t("settings.auth_mode")}</span>
          <span className="text-text-primary">{auth.auth_mode}</span>
        </div>
        <div className="flex justify-between">
          <span className="text-text-muted">{t("settings.token_path")}</span>
          <span className="font-mono text-text-secondary text-[11px]">
            {auth.token_path}
          </span>
        </div>
        <div className="flex justify-between">
          <span className="text-text-muted">{t("settings.local_token")}</span>
          <span className="font-mono text-text-secondary">
            {auth.masked_token}
          </span>
        </div>
        <div className="flex justify-between">
          <span className="text-text-muted">{t("settings.codex_auth")}</span>
          <span className="text-text-primary">{auth.codex_auth_type}</span>
        </div>
        <div className="flex justify-between">
          <span className="text-text-muted">{t("settings.claude_auth")}</span>
          <span className="text-text-primary">
            {auth.claude_code_auth_type}
          </span>
        </div>
      </div>
      <div className="mt-4 flex gap-2">
        <button onClick={handleCopyToken} className="btn-secondary">
          <Copy className="h-3 w-3" />
          {t("settings.copy_token")}
        </button>
        <button onClick={() => setConfirmRegen(true)} className="btn-secondary">
          <RefreshCcw className="h-3 w-3" />
          {t("settings.regenerate_token")}
        </button>
        <button onClick={() => api.openTokenFolder()} className="btn-secondary">
          <FolderOpen className="h-3 w-3" />
          {t("settings.open_token_folder")}
        </button>
      </div>
    </section>
  );
}
