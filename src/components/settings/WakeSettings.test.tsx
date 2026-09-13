import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithProviders } from "@/components/test-utils";
import { WakeSettings } from "./WakeSettings";

const ToggleSwitch = ({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
}) => (
  <input
    type="checkbox"
    aria-label={label}
    checked={checked}
    onChange={(event) => onChange(event.target.checked)}
  />
);

function renderWakeSettings(
  settingsOver: Record<string, unknown> = {},
  statusOver: Record<string, unknown> = {}
) {
  const onUpdate = vi.fn();
  renderWithProviders(
    <WakeSettings
      settings={{
        wake_enabled: true,
        wake_request_control: true,
        wake_cooldown_seconds: 900,
        wake_keep_display_awake: false,
        ...settingsOver,
      }}
      status={{
        supported: true,
        platform: "macos",
        enabled: true,
        request_control: true,
        active: true,
        active_requests: 2,
        mode: "request",
        cooldown_remaining: 0,
        elapsed_seconds: 120,
        keep_display_awake: false,
        last_error: null,
        ...statusOver,
      }}
      onUpdate={onUpdate}
      t={(key) => key}
      ToggleSwitch={ToggleSwitch}
    />
  );
  return onUpdate;
}

describe("WakeSettings", () => {
  it("显示请求控制状态和冷却时间设置", () => {
    renderWakeSettings();

    expect(screen.getByText("settings.wake")).toBeInTheDocument();
    expect(
      screen.getByText(/^settings\.wake\.status\.request/)
    ).toBeInTheDocument();
    expect(screen.getByRole("combobox")).toHaveValue("900");
  });

  it("总开关和请求控制分别保存对应字段", () => {
    const onUpdate = renderWakeSettings();
    const toggles = screen.getAllByRole("checkbox");

    fireEvent.click(toggles[0]);
    fireEvent.click(toggles[1]);

    expect(onUpdate).toHaveBeenNthCalledWith(1, { wake_enabled: false });
    expect(onUpdate).toHaveBeenNthCalledWith(2, {
      wake_request_control: false,
    });
  });

  it("平台不支持时不给出可操作开关", () => {
    renderWakeSettings({}, { supported: false, mode: "unsupported" });

    expect(screen.getByText("settings.wake.unsupported")).toBeInTheDocument();
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
  });

  it("系统申请失败时展示真实错误", () => {
    renderWakeSettings(
      {},
      { mode: "error", active: false, last_error: "boom" }
    );

    expect(screen.getByText(/boom/)).toBeInTheDocument();
  });
});
