import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, act, waitFor, screen, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@/lib/api");
vi.stubGlobal("__TAURI_INTERNALS__", {
  invoke: vi.fn().mockResolvedValue(""),
  transformCallback: vi.fn((cb) => cb),
});
vi.stubGlobal("__TAURI_EVENT_PLUGIN_INTERNALS__", {
  unregisterListener: vi.fn(),
});
vi.mock("@tauri-apps/plugin-autostart", () => ({
  isEnabled: vi.fn().mockResolvedValue(false),
  enable: vi.fn().mockResolvedValue(undefined),
  disable: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn().mockResolvedValue(null),
}));
vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("0.0.0"),
}));

import * as api from "@/lib/api";
import { Settings } from "./Settings";
import { __resetGlobalStoresForTest } from "@/store/global";

afterEach(() => cleanup());

function gatewaySettings(): any {
  return {
    host: "127.0.0.1",
    port: 4141,
    input_protocol: "openai_responses",
    output_protocol: "openai_chat_completions",
    auto_start: true,
    log_retention_days: 30,
    body_filter_global: false,
    thinking_rectifier_global: false,
    error_mapper_global: false,
    health_probe_enabled: false,
    wake_enabled: true,
    wake_request_control: false,
    wake_cooldown_seconds: 900,
    wake_keep_display_awake: false,
  };
}

describe("Settings", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    localStorage.clear();
    vi.mocked(api.getGatewaySettings).mockResolvedValue(gatewaySettings());
    vi.mocked(api.getWakeStatus).mockResolvedValue({
      supported: true,
      platform: "macos",
      enabled: true,
      request_control: false,
      active: true,
      active_requests: 0,
      mode: "continuous",
      cooldown_remaining: 0,
      elapsed_seconds: 60,
      keep_display_awake: false,
      last_error: null,
    });
    vi.mocked(api.getGatewayAuthSettings).mockResolvedValue({
      token_path: "/tmp/token",
    } as any);
    vi.mocked(api.getPetSettings).mockResolvedValue({
      pet_type: "robot",
      visible: true,
    } as any);
    vi.mocked(api.getPetClickThrough).mockResolvedValue(false);
    vi.mocked(api.listModelPricing).mockResolvedValue([]);
    vi.mocked(api.updateGatewaySettings).mockResolvedValue(gatewaySettings());
    vi.mocked(api.updatePetSettings).mockResolvedValue({
      pet_type: "robot",
      visible: true,
    } as any);
    vi.mocked(api.setPetVisible).mockResolvedValue({
      pet_type: "robot",
      visible: false,
    } as any);
    vi.mocked(api.getLocalAccessToken).mockResolvedValue("token");
    vi.mocked(api.regenerateLocalAccessToken).mockResolvedValue({
      token_path: "/tmp/token",
    } as any);
    vi.mocked(api.exportConfigJson).mockResolvedValue("{}");
    vi.mocked(api.importConfigJson).mockResolvedValue({} as any);
  });

  it("renders and loads settings", async () => {
    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );

    await waitFor(() => {
      expect(api.getGatewaySettings).toHaveBeenCalled();
      expect(api.getGatewayAuthSettings).toHaveBeenCalled();
      expect(api.getPetSettings).toHaveBeenCalled();
    });

    expect(screen.getByText("settings.tab.general")).toBeInTheDocument();
  });

  it("offers only the light and dark brand themes", async () => {
    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );

    expect(await screen.findByRole("button", { name: /Dark/i })).toBeVisible();
    expect(screen.getByRole("button", { name: /Light/i })).toBeVisible();
    expect(screen.queryByText("Slate Steel")).not.toBeInTheDocument();
    expect(screen.queryByText("Forest Pine")).not.toBeInTheDocument();
    expect(screen.queryByText("Midnight Violet")).not.toBeInTheDocument();
    expect(screen.queryByText("Linen Cream")).not.toBeInTheDocument();
    expect(screen.queryByText("Mist Blue")).not.toBeInTheDocument();
    expect(screen.queryByText("Sakura")).not.toBeInTheDocument();
  });

  it("migrates an old dark theme before rendering the picker", async () => {
    localStorage.setItem("agentgate_theme", "slate");

    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );

    const dark = await screen.findByRole("button", { name: /Dark/i });
    expect(dark).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem("agentgate_theme")).toBe("dark");
  });

  it("toggles auto start gateway", async () => {
    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );

    await screen.findByText("settings.auto_start_gateway");

    const autoStart = await screen.findByRole("checkbox", {
      name: "settings.auto_start_gateway",
    });
    expect(autoStart.nextElementSibling).toHaveClass(
      "peer-focus-visible:ring-2"
    );
    await act(async () => autoStart.click());

    await waitFor(() =>
      expect(api.updateGatewaySettings).toHaveBeenCalledWith(
        expect.objectContaining({ auto_start: false })
      )
    );
  });

  it("updates the wake master switch", async () => {
    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );

    const title = await screen.findByText("settings.wake.enabled");
    const row = title.parentElement?.parentElement;
    const toggle = row?.querySelector('input[type="checkbox"]') as HTMLElement;
    await act(async () => toggle.click());

    await waitFor(() =>
      expect(api.updateGatewaySettings).toHaveBeenCalledWith({
        wake_enabled: false,
      })
    );
  });

  it("switches tabs and regenerates the local token after confirmation", async () => {
    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );

    await screen.findByText("settings.tab.security");
    await act(async () => screen.getByText("settings.tab.security").click());

    expect(
      await screen.findByText("settings.gateway_security")
    ).toBeInTheDocument();

    await act(async () => {
      screen.getByText("settings.regenerate_token").click();
    });
    expect(await screen.findByText("settings.regen_title")).toBeInTheDocument();

    const regenButtons = screen.getAllByText("settings.regenerate_token");
    const confirm = regenButtons[regenButtons.length - 1];
    await act(async () => confirm.click());

    await waitFor(() =>
      expect(api.regenerateLocalAccessToken).toHaveBeenCalled()
    );
  });

  it("exports config from the data tab without secrets by default", async () => {
    const objectUrl = "blob:agentgate-test";
    vi.stubGlobal("URL", {
      createObjectURL: vi.fn().mockReturnValue(objectUrl),
      revokeObjectURL: vi.fn(),
    });

    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );

    await screen.findByText("settings.tab.data");
    await act(async () => screen.getByText("settings.tab.data").click());

    const exportButton = await screen.findByText("settings.export_config");
    await act(async () => exportButton.click());

    await waitFor(() =>
      expect(api.exportConfigJson).toHaveBeenCalledWith(false)
    );
  });
});

describe("Settings wake status polling", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.mocked(api.getGatewaySettings).mockResolvedValue(gatewaySettings());
    vi.mocked(api.getWakeStatus).mockResolvedValue(null as any);
    vi.mocked(api.getGatewayAuthSettings).mockResolvedValue({
      token_path: "/tmp/token",
    } as any);
    vi.mocked(api.getPetSettings).mockResolvedValue({
      pet_type: "robot",
      visible: true,
    } as any);
    vi.mocked(api.getPetClickThrough).mockResolvedValue(false);
    vi.mocked(api.listModelPricing).mockResolvedValue([]);
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  async function advance(ms: number) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  }

  it("refreshes wake status every 5s while the general tab is shown", async () => {
    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );
    await advance(0);
    vi.mocked(api.getWakeStatus).mockClear();

    await advance(4999);
    expect(api.getWakeStatus).not.toHaveBeenCalled();

    await advance(1);
    expect(api.getWakeStatus).toHaveBeenCalledTimes(1);
  });

  it("does not poll wake status while another tab is active", async () => {
    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );
    await advance(0);
    await act(async () => screen.getByText("settings.tab.security").click());
    vi.mocked(api.getWakeStatus).mockClear();

    await advance(15000);
    expect(api.getWakeStatus).not.toHaveBeenCalled();
  });

  it("refreshes wake status immediately when switching back to the general tab", async () => {
    render(
      <MemoryRouter>
        <Settings />
      </MemoryRouter>
    );
    await advance(0);
    await act(async () => screen.getByText("settings.tab.security").click());
    vi.mocked(api.getWakeStatus).mockClear();

    await act(async () => screen.getByText("settings.tab.general").click());
    await advance(0);
    expect(api.getWakeStatus).toHaveBeenCalledTimes(1);
  });
});
