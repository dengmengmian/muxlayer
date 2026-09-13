import type { GatewaySettings, WakeStatus } from "@/lib/bindings";

type WakePatch = Pick<
  GatewaySettings,
  | "wake_enabled"
  | "wake_request_control"
  | "wake_cooldown_seconds"
  | "wake_keep_display_awake"
>;

interface Props {
  settings: WakePatch;
  status: WakeStatus | null;
  onUpdate: (patch: Partial<WakePatch>) => void;
  t: (key: string) => string;
  ToggleSwitch: React.ComponentType<{
    checked: boolean;
    onChange: (value: boolean) => void;
    label: string;
  }>;
}

const COOLDOWN_OPTIONS = [0, 60, 300, 900, 1800, 3600];

export function WakeSettings({
  settings,
  status,
  onUpdate,
  t,
  ToggleSwitch,
}: Props) {
  if (status && !status.supported) {
    return (
      <SettingsGroup title={t("settings.wake")}>
        <p className="text-xs leading-5 text-text-muted">
          {t("settings.wake.unsupported")}
        </p>
      </SettingsGroup>
    );
  }

  const statusKey = status?.mode ?? "idle";
  const statusDetail = (() => {
    if (!status) return t("settings.wake.status.loading");
    if (status.mode === "request") {
      return `${t("settings.wake.status.request")} · ${status.active_requests}`;
    }
    if (status.mode === "cooldown") {
      return `${t("settings.wake.status.cooldown")} · ${formatDuration(
        status.cooldown_remaining
      )}`;
    }
    if (status.mode === "error" && status.last_error) {
      return `${t("settings.wake.status.error")} · ${status.last_error}`;
    }
    return t(`settings.wake.status.${statusKey}`);
  })();
  const platformLabel = status?.platform
    ? `${status.platform}${status.supported ? "" : ` · ${t("settings.wake.unsupported_short")}`}`
    : null;

  return (
    <SettingsGroup title={t("settings.wake")}>
      <div className="flex flex-col gap-1 rounded-md border border-border bg-card-secondary px-3 py-2">
        <div className="flex items-center gap-2">
          <span
            className={`h-2 w-2 rounded-full ${
              status?.active ? "bg-success" : "bg-text-muted"
            }`}
          />
          <span className="text-xs text-text-secondary">{statusDetail}</span>
        </div>
        {platformLabel && (
          <p className="pl-4 text-[11px] text-text-muted">{platformLabel}</p>
        )}
        {status?.mode === "cooldown" && status.cooldown_remaining > 0 && (
          <p className="pl-4 text-[11px] font-mono text-text-muted">
            {t("settings.wake.cooldown_left")}:{" "}
            {formatDuration(status.cooldown_remaining)}
          </p>
        )}
      </div>

      <SettingRow
        title={t("settings.wake.enabled")}
        description={t("settings.wake.enabled_desc")}
        control={
          <ToggleSwitch
            checked={settings.wake_enabled}
            label={t("settings.wake.enabled")}
            onChange={(value) => onUpdate({ wake_enabled: value })}
          />
        }
      />

      {settings.wake_enabled && (
        <>
          <SettingRow
            title={t("settings.wake.request_control")}
            description={t("settings.wake.request_control_desc")}
            control={
              <ToggleSwitch
                checked={settings.wake_request_control}
                label={t("settings.wake.request_control")}
                onChange={(value) => onUpdate({ wake_request_control: value })}
              />
            }
          />

          {settings.wake_request_control && (
            <SettingRow
              title={t("settings.wake.cooldown")}
              description={t("settings.wake.cooldown_desc")}
              control={
                <select
                  value={settings.wake_cooldown_seconds}
                  onChange={(event) =>
                    onUpdate({
                      wake_cooldown_seconds: Number(event.target.value),
                    })
                  }
                  className="rounded-md border border-border bg-card-secondary px-3 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
                >
                  {COOLDOWN_OPTIONS.map((seconds) => (
                    <option key={seconds} value={seconds}>
                      {t(`settings.wake.cooldown.${seconds}`)}
                    </option>
                  ))}
                </select>
              }
            />
          )}

          <SettingRow
            title={t("settings.wake.display")}
            description={t("settings.wake.display_desc")}
            control={
              <ToggleSwitch
                checked={settings.wake_keep_display_awake}
                label={t("settings.wake.display")}
                onChange={(value) =>
                  onUpdate({ wake_keep_display_awake: value })
                }
              />
            }
          />
        </>
      )}
    </SettingsGroup>
  );
}

function formatDuration(seconds: number) {
  if (seconds < 60) return `${seconds}s`;
  return `${Math.ceil(seconds / 60)}m`;
}

function SettingsGroup({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-4 px-5 py-4">
      <h4 className="text-xs font-semibold uppercase tracking-wide text-text-muted">
        {title}
      </h4>
      {children}
    </div>
  );
}

function SettingRow({
  title,
  description,
  control,
}: {
  title: string;
  description: string;
  control: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-6">
      <div className="min-w-0 flex-1">
        <p className="text-sm text-text-primary">{title}</p>
        <p className="text-xs leading-5 text-text-muted">{description}</p>
      </div>
      <div className="shrink-0">{control}</div>
    </div>
  );
}
