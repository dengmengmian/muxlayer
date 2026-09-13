import { useState } from "react";
import {
  X,
  Image as ImageIcon,
  FileText,
  Wrench,
  Moon,
  type LucideIcon,
} from "lucide-react";
import { useI18n } from "@/lib/i18n";
import type { RoutingConditions } from "@/types/route-profile";

type ConditionPreset = {
  key: string;
  icon: LucideIcon;
  conditions: RoutingConditions;
};

const CONDITION_PRESETS: ConditionPreset[] = [
  {
    key: "background",
    icon: Moon,
    conditions: { model_name_match: ["haiku"] },
  },
  { key: "images", icon: ImageIcon, conditions: { has_images: true } },
  { key: "long_text", icon: FileText, conditions: { min_input_chars: 100000 } },
  { key: "tools", icon: Wrench, conditions: { has_tools: true } },
];

function isBackgroundPreset(names?: string[] | null): boolean {
  return names?.length === 1 && names[0] === "haiku";
}

function hasCustomOnlyConditions(c: RoutingConditions): boolean {
  if (c.max_input_chars != null) return true;
  if (c.has_images === false || c.has_tools === false) return true;
  if (c.min_input_chars != null && c.min_input_chars !== 100000) return true;
  if (c.model_name_match?.length && !isBackgroundPreset(c.model_name_match))
    return true;
  return Boolean(c.system_keywords?.length);
}

function detectCheckedPresets(c: RoutingConditions): Set<string> {
  const checked = new Set<string>();
  if (c.has_images === true) checked.add("images");
  if (c.has_tools === true) checked.add("tools");
  if (c.min_input_chars && c.min_input_chars >= 100000)
    checked.add("long_text");
  if (isBackgroundPreset(c.model_name_match)) checked.add("background");
  return checked;
}

function mergePresetConditions(checked: Set<string>): RoutingConditions {
  const c: RoutingConditions = {};
  if (checked.has("images")) c.has_images = true;
  if (checked.has("tools")) c.has_tools = true;
  if (checked.has("long_text")) c.min_input_chars = 100000;
  if (checked.has("background")) c.model_name_match = ["haiku"];
  return c;
}

export function ConditionsDialog({
  target,
  onSave,
  onClose,
}: {
  target: {
    providerName: string;
    inputProtocol: string;
    current: RoutingConditions;
  };
  onSave: (c: RoutingConditions) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [checked, setChecked] = useState(() =>
    detectCheckedPresets(target.current)
  );
  const [showCustom, setShowCustom] = useState(() =>
    hasCustomOnlyConditions(target.current)
  );
  const [modelOverride, setModelOverride] = useState(
    target.current.model_override ?? ""
  );

  // Custom fields (only used in custom mode)
  const [minChars, setMinChars] = useState(
    target.current.min_input_chars?.toString() ?? ""
  );
  const [maxChars, setMaxChars] = useState(
    target.current.max_input_chars?.toString() ?? ""
  );
  const [hasImages, setHasImages] = useState<string>(
    target.current.has_images === true
      ? "true"
      : target.current.has_images === false
        ? "false"
        : ""
  );
  const [hasTools, setHasTools] = useState<string>(
    target.current.has_tools === true
      ? "true"
      : target.current.has_tools === false
        ? "false"
        : ""
  );
  const [keywords, setKeywords] = useState(
    target.current.system_keywords?.join(", ") ?? ""
  );
  const [modelNameMatch, setModelNameMatch] = useState(
    target.current.model_name_match?.join(", ") ?? ""
  );

  const toggle = (key: string) => {
    const next = new Set(checked);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    setChecked(next);
  };

  const handleSave = () => {
    let c: RoutingConditions;

    if (showCustom) {
      // Custom mode: build from raw fields
      c = {};
      if (minChars) c.min_input_chars = parseInt(minChars, 10) || null;
      if (maxChars) c.max_input_chars = parseInt(maxChars, 10) || null;
      if (hasImages === "true") c.has_images = true;
      else if (hasImages === "false") c.has_images = false;
      if (hasTools === "true") c.has_tools = true;
      else if (hasTools === "false") c.has_tools = false;
      if (keywords.trim())
        c.system_keywords = keywords
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean);
      if (modelNameMatch.trim())
        c.model_name_match = modelNameMatch
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean);
    } else {
      // Preset mode: merge checked presets
      c = mergePresetConditions(checked);
    }

    if (modelOverride.trim()) c.model_override = modelOverride.trim();
    onSave(c);
  };

  const hasAny =
    checked.size > 0 || showCustom || modelOverride.trim().length > 0;
  const isResponsesProfile = target.inputProtocol === "openai_responses";

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center">
      <div className="fixed inset-0 bg-black/50" onClick={onClose} />
      <div className="relative z-10 w-full max-w-md rounded-lg border border-border bg-card shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-5 py-3">
          <h3 className="text-sm font-semibold text-text-primary">
            {t("routes.edit_conditions")} — {target.providerName}
          </h3>
          <button
            onClick={onClose}
            className="rounded p-1 text-text-muted hover:text-text-primary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
        <div className="space-y-3 p-5">
          <p className="text-[11px] text-text-muted">
            {t("routes.conditions_hint")}
          </p>
          {!isResponsesProfile && (
            <p className="rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-[11px] text-warning">
              {t("routes.conditions_protocol_note")}
            </p>
          )}

          {/* Multi-select scene checkboxes */}
          {!showCustom && (
            <>
              <div className="grid grid-cols-2 gap-2">
                {CONDITION_PRESETS.map((p) => {
                  const Icon = p.icon;
                  return (
                    <label
                      key={p.key}
                      className={`flex cursor-pointer items-center gap-2 rounded-md border px-3 py-2 text-xs transition-colors ${checked.has(p.key) ? "border-accent bg-accent-soft text-accent" : "border-border text-text-secondary hover:border-accent/50"}`}
                    >
                      <input
                        type="checkbox"
                        checked={checked.has(p.key)}
                        onChange={() => toggle(p.key)}
                        className="accent-accent"
                      />
                      <Icon className="h-3.5 w-3.5" />{" "}
                      {t(`routes.scene_${p.key}`)}
                    </label>
                  );
                })}
              </div>
              <button
                onClick={() => setShowCustom(true)}
                className="text-[11px] text-accent hover:text-accent/80"
              >
                {t("routes.scene_custom")}
              </button>
            </>
          )}

          {/* Toggle custom mode */}
          {showCustom && (
            <button
              onClick={() => setShowCustom(false)}
              className="text-[11px] text-accent hover:text-accent/80"
            >
              {t("routes.back_to_presets")}
            </button>
          )}

          {/* Custom fields */}
          {showCustom && (
            <div className="space-y-3 rounded-md border border-border/50 bg-card-secondary p-3">
              <p className="text-[10px] text-text-muted">
                {t("routes.custom_conditions_hint")}
              </p>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="mb-1 block text-[10px] text-text-muted">
                    {t("routes.min_chars")}
                  </label>
                  <input
                    type="number"
                    value={minChars}
                    onChange={(e) => setMinChars(e.target.value)}
                    placeholder="100000"
                    className="form-input w-full"
                  />
                </div>
                <div>
                  <label className="mb-1 block text-[10px] text-text-muted">
                    {t("routes.max_chars")}
                  </label>
                  <input
                    type="number"
                    value={maxChars}
                    onChange={(e) => setMaxChars(e.target.value)}
                    placeholder="500000"
                    className="form-input w-full"
                  />
                </div>
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="mb-1 block text-[10px] text-text-muted">
                    {t("routes.has_images")}
                  </label>
                  <select
                    value={hasImages}
                    onChange={(e) => setHasImages(e.target.value)}
                    className="form-input w-full"
                  >
                    <option value="">{t("routes.any")}</option>
                    <option value="true">{t("routes.required")}</option>
                    <option value="false">{t("routes.excluded")}</option>
                  </select>
                </div>
                <div>
                  <label className="mb-1 block text-[10px] text-text-muted">
                    {t("routes.has_tools")}
                  </label>
                  <select
                    value={hasTools}
                    onChange={(e) => setHasTools(e.target.value)}
                    className="form-input w-full"
                  >
                    <option value="">{t("routes.any")}</option>
                    <option value="true">{t("routes.required")}</option>
                    <option value="false">{t("routes.excluded")}</option>
                  </select>
                </div>
              </div>
              <div>
                <label className="mb-1 block text-[10px] text-text-muted">
                  {t("routes.system_keywords")}
                </label>
                <input
                  value={keywords}
                  onChange={(e) => setKeywords(e.target.value)}
                  placeholder="background, subagent"
                  className="form-input w-full"
                />
                <p className="mt-0.5 text-[10px] text-text-muted">
                  {t("routes.keywords_hint")}
                </p>
              </div>
              <div>
                <label className="mb-1 block text-[10px] text-text-muted">
                  {t("routes.model_name_match")}
                </label>
                <input
                  value={modelNameMatch}
                  onChange={(e) => setModelNameMatch(e.target.value)}
                  placeholder="haiku, flash"
                  className="form-input w-full"
                />
                <p className="mt-0.5 text-[10px] text-text-muted">
                  {t("routes.model_name_match_hint")}
                </p>
              </div>
            </div>
          )}

          {/* Model override */}
          {hasAny && (
            <div>
              <label className="mb-1 block text-[10px] text-text-muted">
                {t("routes.condition_model_override")}
              </label>
              <input
                value={modelOverride}
                onChange={(e) => setModelOverride(e.target.value)}
                placeholder="e.g. deepseek-v4-flash"
                className="form-input w-full"
              />
              <p className="mt-0.5 text-[10px] text-text-muted">
                {t("routes.model_override_hint")}
              </p>
            </div>
          )}
        </div>

        <div className="flex justify-end gap-2 border-t border-border px-5 py-3">
          <button
            onClick={() => {
              onSave({});
            }}
            className="rounded-md bg-card-secondary px-4 py-1.5 text-xs text-text-secondary hover:bg-border"
          >
            {t("routes.clear_conditions")}
          </button>
          <button
            onClick={onClose}
            className="rounded-md bg-card-secondary px-4 py-1.5 text-xs text-text-secondary hover:bg-border"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={handleSave}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-on-accent hover:bg-accent/90"
          >
            {t("common.save")}
          </button>
        </div>
      </div>
    </div>
  );
}
