import { PawPrint } from "lucide-react";
import type { PetType, PetSettings as PetSettingsType } from "@/types/pet";
import { PetTypeCard } from "@/pages/Settings";

interface Props {
  petSettings: PetSettingsType;
  petClickThrough: boolean;
  handlePetVisibleChange: (visible: boolean) => Promise<void>;
  handlePetClickThroughChange: (v: boolean) => void;
  handlePetTypeChange: (type: PetType) => Promise<void>;
  t: (key: string) => string;
  ToggleSwitch: React.ComponentType<{
    checked: boolean;
    onChange: (val: boolean) => void;
    label: string;
  }>;
}

export function PetTab({
  petSettings,
  petClickThrough,
  handlePetVisibleChange,
  handlePetClickThroughChange,
  handlePetTypeChange,
  t,
  ToggleSwitch,
}: Props) {
  return (
    <section className="surface-panel p-5">
      <h3 className="mb-1 flex items-center gap-2 text-sm font-semibold text-text-primary">
        <PawPrint className="h-4 w-4 text-accent" />
        {t("settings.pet.title")}
      </h3>
      <p className="mb-5 text-xs text-text-muted">{t("settings.pet.desc")}</p>

      {/* Visibility toggle */}
      <div className="mb-6 flex items-center justify-between">
        <div>
          <p className="text-sm text-text-primary">
            {t("settings.pet.visible")}
          </p>
          <p className="text-xs text-text-muted">
            {t("settings.pet.visible_desc")}
          </p>
        </div>
        <ToggleSwitch
          checked={petSettings.visible}
          onChange={handlePetVisibleChange}
          label={t("settings.pet.visible")}
        />
      </div>

      {/* Click-through toggle */}
      <div className="mb-6 flex items-center justify-between">
        <div>
          <p className="text-sm text-text-primary">
            {t("settings.pet.click_through")}
          </p>
          <p className="text-xs text-text-muted">
            {t("settings.pet.click_through_desc")}
          </p>
        </div>
        <ToggleSwitch
          checked={petClickThrough}
          onChange={handlePetClickThroughChange}
          label={t("settings.pet.click_through")}
        />
      </div>

      {/* Pet type selection */}
      <div>
        <p className="mb-3 text-sm text-text-primary">
          {t("settings.pet.type")}
        </p>
        <p className="mb-4 text-xs text-text-muted">
          {t("settings.pet.type_desc")}
        </p>
        <div className="grid grid-cols-3 gap-3">
          {(
            [
              "robot",
              "pixel-cat",
              "slime",
              "fox",
              "octopus",
              "ghost",
              "ox",
              "soldier",
              "coder",
            ] as PetType[]
          ).map((type) => (
            <PetTypeCard
              key={type}
              type={type}
              selected={petSettings.pet_type === type}
              name={t(`settings.pet.${type}`)}
              desc={t(`settings.pet.${type}_desc`)}
              onClick={() => handlePetTypeChange(type)}
            />
          ))}
        </div>
      </div>

      {/* FAQ */}
      <div className="mt-8 border-t border-border pt-5">
        <p className="mb-3 text-sm font-semibold text-text-primary">
          {t("settings.pet.faq.title")}
        </p>
        <details className="rounded-lg border border-border bg-card-secondary p-3">
          <summary className="cursor-pointer text-xs text-text-primary">
            {t("settings.pet.faq.q_windows_bg")}
          </summary>
          <p className="mt-2 text-xs leading-relaxed text-text-secondary">
            {t("settings.pet.faq.a_windows_bg")}
          </p>
        </details>
      </div>
    </section>
  );
}
