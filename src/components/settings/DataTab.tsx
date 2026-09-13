import { DollarSign } from "lucide-react";
import { toast } from "@/components/common/Toast";
import * as api from "@/lib/api";
import type { GatewaySettings as GatewaySettingsType } from "@/types/gateway";
import type { ModelPricing } from "@/types/stats";
import {
  ConfigBackupSection,
  CollapsibleSection,
  PricingRow,
  PricingAddForm,
} from "@/pages/Settings";

interface Props {
  settings: GatewaySettingsType;
  pricing: ModelPricing[];
  setPricing: (
    next: ModelPricing[] | ((prev: ModelPricing[]) => ModelPricing[])
  ) => void;
  handleUpdateRetention: (days: number) => Promise<void>;
  t: (key: string) => string;
}

export function DataTab({
  settings,
  pricing,
  setPricing,
  handleUpdateRetention,
  t,
}: Props) {
  return (
    <>
      <section className="surface-panel p-5">
        <h3 className="mb-4 text-sm font-semibold text-text-primary">
          {t("settings.data")}
        </h3>
        <div className="flex items-center justify-between">
          <div>
            <p className="text-sm text-text-primary">
              {t("settings.log_retention")}
            </p>
            <p className="text-xs text-text-muted">
              {t("settings.log_retention_desc")}
            </p>
          </div>
          <select
            value={settings.log_retention_days}
            onChange={(e) =>
              handleUpdateRetention(parseInt(e.target.value, 10))
            }
            className="rounded-md border border-border bg-card-secondary px-3 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
          >
            <option value={7}>7 {t("common.days")}</option>
            <option value={14}>14 {t("common.days")}</option>
            <option value={30}>30 {t("common.days")}</option>
            <option value={90}>90 {t("common.days")}</option>
          </select>
        </div>
      </section>

      <ConfigBackupSection />

      {/* 模型定价表默认折叠——用户配 1 次就再也不动，平铺时挤压日常常看的
          日志保留 / 配置导入导出。点击 header 展开。 */}
      <CollapsibleSection
        icon={DollarSign}
        title={t("settings.model_pricing")}
        hint={t("settings.model_pricing_desc")}
        badge={`${pricing.length}`}
      >
        <div className="overflow-hidden rounded-md border border-border">
          <table className="w-full text-xs">
            <thead>
              <tr className="border-b border-border bg-card-secondary">
                <th className="px-3 py-2 text-left font-medium text-text-muted">
                  Provider
                </th>
                <th className="px-3 py-2 text-left font-medium text-text-muted">
                  Model
                </th>
                <th className="px-3 py-2 text-right font-medium text-text-muted">
                  Input ($/1M)
                </th>
                <th className="px-3 py-2 text-right font-medium text-text-muted">
                  Output ($/1M)
                </th>
                <th className="px-3 py-2 text-center font-medium text-text-muted">
                  {t("settings.source")}
                </th>
                <th className="px-3 py-2 w-8"></th>
              </tr>
            </thead>
            <tbody>
              {pricing.map((p) => (
                <PricingRow
                  key={p.id}
                  item={p}
                  onUpdate={async (inputPrice, outputPrice) => {
                    try {
                      const updated = await api.upsertModelPricing(
                        p.provider,
                        p.model_pattern,
                        inputPrice,
                        outputPrice
                      );
                      setPricing(
                        pricing
                          .map((x) =>
                            x.id === p.id || x.id === updated.id ? updated : x
                          )
                          .sort((a, b) =>
                            `${a.provider}${a.model_pattern}`.localeCompare(
                              `${b.provider}${b.model_pattern}`
                            )
                          )
                      );
                      toast("success", t("settings.pricing_saved"));
                    } catch (err) {
                      toast("error", (err as api.AppError).message);
                    }
                  }}
                  onDelete={async () => {
                    await api.deleteModelPricing(p.id);
                    setPricing(pricing.filter((x) => x.id !== p.id));
                    toast("success", t("common.deleted"));
                  }}
                />
              ))}
            </tbody>
          </table>
        </div>

        <PricingAddForm
          onAdd={async (provider, model, inputPrice, outputPrice) => {
            try {
              const p = await api.upsertModelPricing(
                provider,
                model,
                inputPrice,
                outputPrice
              );
              setPricing(
                [...pricing.filter((x) => x.id !== p.id), p].sort((a, b) =>
                  `${a.provider}${a.model_pattern}`.localeCompare(
                    `${b.provider}${b.model_pattern}`
                  )
                )
              );
              toast("success", t("settings.pricing_saved"));
            } catch (err) {
              toast("error", (err as api.AppError).message);
            }
          }}
        />
      </CollapsibleSection>
    </>
  );
}
