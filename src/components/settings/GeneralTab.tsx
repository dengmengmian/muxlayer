import { useState, useEffect } from "react";
import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
import { ChevronDown } from "lucide-react";
import type { Locale } from "@/lib/i18n";
import type { GatewaySettings as GatewaySettingsType } from "@/types/gateway";
import { toast } from "@/components/common/Toast";
import type { WakeStatus } from "@/lib/bindings";
import type { AppTheme } from "@/lib/theme";
import { WakeSettings } from "./WakeSettings";

interface Props {
  settings: GatewaySettingsType;
  locale: Locale;
  setLocale: (l: Locale) => void;
  theme: AppTheme;
  setTheme: (t: AppTheme) => void;
  handleUpdateAutoStart: (val: boolean) => Promise<void>;
  handleUpdateRefinerGlobal: (
    key:
      | "body_filter_global"
      | "thinking_rectifier_global"
      | "error_mapper_global"
      | "health_probe_enabled",
    val: boolean
  ) => Promise<void>;
  handleUpdateCostAlert: (patch: {
    cost_alert_enabled?: boolean;
    cost_alert_threshold?: number;
    cost_budget_enabled?: boolean;
    cost_budget_threshold?: number;
    cost_budget_strategy?: string;
    auto_compact_enabled?: boolean;
    auto_compact_usage_percent?: number;
  }) => Promise<void>;
  handleUpdateRequestBodyLimit: (mb: number) => Promise<void>;
  handleUpdateProxy: (patch: {
    outbound_proxy_enabled?: boolean;
    outbound_proxy_url?: string;
  }) => Promise<void>;
  wakeStatus: WakeStatus | null;
  handleUpdateWake: (patch: {
    wake_enabled?: boolean;
    wake_request_control?: boolean;
    wake_cooldown_seconds?: number;
    wake_keep_display_awake?: boolean;
  }) => Promise<void>;
  t: (key: string) => string;
  ToggleSwitch: React.ComponentType<{
    checked: boolean;
    onChange: (val: boolean) => void;
    label: string;
  }>;
  ThemePicker: React.ComponentType<{
    value: AppTheme;
    onChange: (id: AppTheme) => void;
  }>;
}

export function GeneralTab({
  settings,
  locale,
  setLocale,
  theme,
  setTheme,
  handleUpdateAutoStart,
  handleUpdateRefinerGlobal,
  handleUpdateCostAlert,
  handleUpdateRequestBodyLimit,
  handleUpdateProxy,
  wakeStatus,
  handleUpdateWake,
  t,
  ToggleSwitch,
  ThemePicker,
}: Props) {
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);

  useEffect(() => {
    isEnabled()
      .then(setLaunchAtLogin)
      .catch(() => {});
  }, []);

  const handleToggleLaunchAtLogin = async (val: boolean) => {
    try {
      if (val) await enable();
      else await disable();
      setLaunchAtLogin(val);
    } catch (e) {
      toast("error", String(e));
    }
  };

  return (
    <section className="w-full rounded-lg border border-border bg-card">
      <div className="border-b border-border px-5 py-4">
        <h3 className="text-sm font-semibold text-text-primary">
          {t("settings.general")}
        </h3>
      </div>

      <div className="divide-y divide-border">
        <SettingsGroup title={t("settings.general.basic")}>
          <SettingRow
            title={t("settings.auto_start_gateway")}
            description={t("settings.auto_start_desc")}
            control={
              <ToggleSwitch
                checked={settings.auto_start}
                onChange={handleUpdateAutoStart}
                label={t("settings.auto_start_gateway")}
              />
            }
          />
          <SettingRow
            title={t("settings.launch_at_login")}
            description={t("settings.launch_at_login_desc")}
            control={
              <ToggleSwitch
                checked={launchAtLogin}
                onChange={handleToggleLaunchAtLogin}
                label={t("settings.launch_at_login")}
              />
            }
          />
          <SettingRow
            title={t("settings.request_body_limit")}
            description={t("settings.request_body_limit_desc")}
            control={
              <div className="flex items-center gap-2">
                <input
                  type="number"
                  min="1"
                  max="128"
                  step="1"
                  defaultValue={settings.request_body_limit_mb}
                  onBlur={(e) => {
                    const v = Math.floor(Number(e.target.value));
                    if (
                      Number.isFinite(v) &&
                      v > 0 &&
                      v <= 128 &&
                      v !== settings.request_body_limit_mb
                    ) {
                      handleUpdateRequestBodyLimit(v);
                    }
                  }}
                  className="form-input w-20"
                />
                <span className="text-sm text-text-muted">MB</span>
              </div>
            }
          />
          <div className="space-y-2">
            <SettingRow
              title={t("settings.outbound_proxy")}
              description={t("settings.outbound_proxy_desc")}
              control={
                <ToggleSwitch
                  checked={settings.outbound_proxy_enabled ?? false}
                  label={t("settings.outbound_proxy")}
                  onChange={(v) =>
                    handleUpdateProxy({ outbound_proxy_enabled: v })
                  }
                />
              }
            />
            {settings.outbound_proxy_enabled && (
              <div className="space-y-1">
                <input
                  type="url"
                  defaultValue={settings.outbound_proxy_url ?? ""}
                  placeholder={t("settings.outbound_proxy_url_ph")}
                  aria-label={t("settings.outbound_proxy_url")}
                  onBlur={(e) => {
                    const v = e.target.value.trim();
                    if (v !== (settings.outbound_proxy_url ?? "")) {
                      handleUpdateProxy({ outbound_proxy_url: v });
                    }
                  }}
                  className="form-input w-full font-mono text-xs"
                />
                {!(settings.outbound_proxy_url ?? "").trim() && (
                  <p className="text-xs text-warning">
                    {t("settings.outbound_proxy_empty")}
                  </p>
                )}
              </div>
            )}
          </div>
          <SettingRow
            title={t("settings.language")}
            description={t("settings.lang_desc")}
            control={
              <select
                value={locale}
                onChange={(e) => setLocale(e.target.value as Locale)}
                className="rounded-md border border-border bg-card-secondary px-3 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
              >
                <option value="en">English</option>
                <option value="zh">中文</option>
              </select>
            }
          />
        </SettingsGroup>

        <WakeSettings
          settings={settings}
          status={wakeStatus}
          onUpdate={handleUpdateWake}
          t={t}
          ToggleSwitch={ToggleSwitch}
        />

        <SettingsGroup title={t("settings.auto_compact")}>
          <SettingRow
            title={t("settings.auto_compact.enabled")}
            description={t("settings.auto_compact.enabled_desc")}
            control={
              <ToggleSwitch
                checked={settings.auto_compact_enabled ?? true}
                label={t("settings.auto_compact.enabled")}
                onChange={(v) =>
                  handleUpdateCostAlert({ auto_compact_enabled: v })
                }
              />
            }
          />
          {(settings.auto_compact_enabled ?? true) && (
            <SettingRow
              title={t("settings.auto_compact.usage_percent")}
              description={t("settings.auto_compact.usage_percent_desc")}
              control={
                <div className="flex items-center gap-2">
                  <input
                    type="number"
                    min={1}
                    max={95}
                    defaultValue={settings.auto_compact_usage_percent ?? 85}
                    onBlur={(e) => {
                      const v = Math.floor(Number(e.target.value));
                      if (
                        Number.isFinite(v) &&
                        v >= 1 &&
                        v <= 95 &&
                        v !== settings.auto_compact_usage_percent
                      ) {
                        handleUpdateCostAlert({
                          auto_compact_usage_percent: v,
                        });
                      }
                    }}
                    className="form-input w-16"
                  />
                  <span className="text-xs text-text-muted">%</span>
                </div>
              }
            />
          )}
        </SettingsGroup>

        <SettingsGroup title={t("settings.general.appearance")}>
          <div>
            <div className="mb-3">
              <p className="text-sm text-text-primary">{t("settings.theme")}</p>
              <p className="text-xs text-text-muted">
                {t("settings.theme_desc")}
              </p>
            </div>
            <ThemePicker value={theme} onChange={setTheme} />
          </div>
        </SettingsGroup>

        <div>
          <button
            type="button"
            onClick={() => setAdvancedOpen((v) => !v)}
            className="flex w-full items-center justify-between px-5 py-4 text-left"
          >
            <div>
              <p className="text-sm font-medium text-text-primary">
                {t("settings.general.advanced")}
              </p>
              <p className="text-xs text-text-muted">
                {t("settings.general.advanced_desc")}
              </p>
            </div>
            <ChevronDown
              className={`h-4 w-4 text-text-muted transition-transform ${
                advancedOpen ? "rotate-180" : ""
              }`}
            />
          </button>

          {advancedOpen && (
            <div className="space-y-5 border-t border-border px-5 py-4">
              <SettingRow
                title={t("settings.show_quick_setup")}
                description={t("settings.show_quick_setup_desc")}
                control={
                  <ToggleSwitch
                    checked={
                      localStorage.getItem("agentgate_show_quick_setup") === "1"
                    }
                    label={t("settings.show_quick_setup")}
                    onChange={(val) => {
                      if (val) {
                        localStorage.setItem("agentgate_show_quick_setup", "1");
                        localStorage.removeItem("agentgate_hide_quick_setup");
                      } else {
                        localStorage.removeItem("agentgate_show_quick_setup");
                        localStorage.setItem("agentgate_hide_quick_setup", "1");
                      }
                      window.location.reload();
                    }}
                  />
                }
              />

              <div>
                <div className="mb-3">
                  <p className="text-sm font-medium text-text-primary">
                    {t("settings.refiner")}
                  </p>
                  <p className="text-xs text-text-muted">
                    {t("settings.refiner_desc")}
                  </p>
                </div>
                <div className="space-y-3">
                  <SettingRow
                    title={t("settings.body_filter")}
                    description={t("settings.body_filter_desc")}
                    control={
                      <ToggleSwitch
                        checked={settings.body_filter_global}
                        label={t("settings.body_filter")}
                        onChange={(v) =>
                          handleUpdateRefinerGlobal("body_filter_global", v)
                        }
                      />
                    }
                  />
                  <SettingRow
                    title={t("settings.thinking_rectifier")}
                    description={t("settings.thinking_rectifier_desc")}
                    control={
                      <ToggleSwitch
                        checked={settings.thinking_rectifier_global}
                        label={t("settings.thinking_rectifier")}
                        onChange={(v) =>
                          handleUpdateRefinerGlobal(
                            "thinking_rectifier_global",
                            v
                          )
                        }
                      />
                    }
                  />
                  <SettingRow
                    title={t("settings.error_mapper")}
                    description={t("settings.error_mapper_desc")}
                    control={
                      <ToggleSwitch
                        checked={settings.error_mapper_global}
                        label={t("settings.error_mapper")}
                        onChange={(v) =>
                          handleUpdateRefinerGlobal("error_mapper_global", v)
                        }
                      />
                    }
                  />
                  <SettingRow
                    title={t("settings.health_probe")}
                    description={t("settings.health_probe_desc")}
                    control={
                      <ToggleSwitch
                        checked={settings.health_probe_enabled}
                        label={t("settings.health_probe")}
                        onChange={(v) =>
                          handleUpdateRefinerGlobal("health_probe_enabled", v)
                        }
                      />
                    }
                  />
                  <SettingRow
                    title={t("settings.cost_alert")}
                    description={t("settings.cost_alert_desc")}
                    control={
                      <ToggleSwitch
                        checked={settings.cost_alert_enabled}
                        label={t("settings.cost_alert")}
                        onChange={(v) =>
                          handleUpdateCostAlert({ cost_alert_enabled: v })
                        }
                      />
                    }
                  />
                  {settings.cost_alert_enabled && (
                    <SettingRow
                      title={t("settings.cost_alert_threshold")}
                      description={t("settings.cost_alert_threshold_desc")}
                      control={
                        <div className="flex items-center gap-1">
                          <span className="text-sm text-text-muted">$</span>
                          <input
                            type="number"
                            min="0"
                            step="0.5"
                            defaultValue={settings.cost_alert_threshold ?? ""}
                            onBlur={(e) => {
                              const v = parseFloat(e.target.value);
                              if (
                                !Number.isNaN(v) &&
                                v > 0 &&
                                v !== settings.cost_alert_threshold
                              ) {
                                handleUpdateCostAlert({
                                  cost_alert_threshold: v,
                                });
                              }
                            }}
                            placeholder="10"
                            className="form-input w-20"
                          />
                        </div>
                      }
                    />
                  )}
                  <SettingRow
                    title={t("settings.cost_budget")}
                    description={t("settings.cost_budget_desc")}
                    control={
                      <ToggleSwitch
                        checked={settings.cost_budget_enabled ?? false}
                        label={t("settings.cost_budget")}
                        onChange={(v) =>
                          handleUpdateCostAlert({ cost_budget_enabled: v })
                        }
                      />
                    }
                  />
                  {settings.cost_budget_enabled && (
                    <>
                      <SettingRow
                        title={t("settings.cost_budget_threshold")}
                        description={t("settings.cost_budget_desc")}
                        control={
                          <div className="flex items-center gap-1">
                            <span className="text-sm text-text-muted">$</span>
                            <input
                              type="number"
                              min="0"
                              step="0.5"
                              defaultValue={
                                settings.cost_budget_threshold ?? ""
                              }
                              onBlur={(e) => {
                                const v = parseFloat(e.target.value);
                                if (
                                  !Number.isNaN(v) &&
                                  v > 0 &&
                                  v !== settings.cost_budget_threshold
                                ) {
                                  handleUpdateCostAlert({
                                    cost_budget_threshold: v,
                                  });
                                }
                              }}
                              placeholder="10"
                              className="form-input w-20"
                            />
                          </div>
                        }
                      />
                      <SettingRow
                        title={t("settings.cost_budget_strategy")}
                        description={t("settings.cost_budget_desc")}
                        control={
                          <select
                            value={
                              settings.cost_budget_strategy ?? "notify_only"
                            }
                            onChange={(e) =>
                              handleUpdateCostAlert({
                                cost_budget_strategy: e.target.value,
                              })
                            }
                            className="rounded-md border border-border bg-card-secondary px-2 py-1.5 text-xs"
                          >
                            <option value="notify_only">
                              {t("settings.cost_budget_notify_only")}
                            </option>
                            <option value="block">
                              {t("settings.cost_budget_block")}
                            </option>
                            <option value="force_cheapest">
                              {t("settings.cost_budget_force_cheapest")}
                            </option>
                          </select>
                        }
                      />
                    </>
                  )}
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
    </section>
  );
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
