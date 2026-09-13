import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  act,
  waitFor,
  screen,
  cleanup,
  fireEvent,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@/lib/api");

import * as api from "@/lib/api";
import { Tools } from "./Tools";
import { ToastContainer } from "@/components/common/Toast";
import { __resetGlobalStoresForTest, useGatewayStatus } from "@/store/global";

afterEach(() => cleanup());

function gatewayStatus(): any {
  return {
    running: true,
    host: "127.0.0.1",
    port: 4141,
  };
}

describe("Tools", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.clearAllMocks();
    sessionStorage.clear();
    vi.mocked(api.detectCodexConfig).mockResolvedValue({
      exists: true,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectClaudeCodeEnv).mockResolvedValue({
      settings_exists: false,
      has_api_key: false,
      has_auth_token: false,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectOpenCodeConfig).mockResolvedValue({
      exists: false,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectGeminiConfig).mockResolvedValue({
      exists: false,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectAtomCodeConfig).mockResolvedValue({
      exists: false,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectClaudeDesktop).mockResolvedValue({
      installed: false,
      supported: false,
      has_agentgate_profile: false,
    } as any);
    vi.mocked(api.detectKimiConfig).mockResolvedValue({
      exists: false,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectGrokConfig).mockResolvedValue({
      exists: false,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectDshConfig).mockResolvedValue({
      exists: false,
      has_agentgate: false,
    } as any);
    vi.mocked(api.getGatewayStatus).mockResolvedValue(gatewayStatus());
    vi.mocked(api.clientsWithApplyHistory).mockResolvedValue([]);
    vi.mocked(api.generateCodexConfig).mockResolvedValue("{}");
    vi.mocked(api.testToolConnection).mockResolvedValue({
      config_ok: true,
      gateway_ok: true,
      provider_ok: true,
    } as any);
    vi.mocked(api.detectClientRunning).mockResolvedValue([]);
    vi.mocked(api.codexDesktopAvailable).mockResolvedValue(false);
  });

  it("renders client list and loads statuses", async () => {
    render(
      <MemoryRouter>
        <Tools />
      </MemoryRouter>
    );

    await waitFor(() => {
      expect(api.detectCodexConfig).toHaveBeenCalled();
      expect(api.getGatewayStatus).toHaveBeenCalled();
    });

    expect(screen.getAllByText("tools.clients").length).toBeGreaterThan(0);
    expect(screen.getByText("tools.connection_path")).toBeInTheDocument();
    expect(screen.getAllByTestId("client-logo-codex").length).toBeGreaterThan(
      0
    );
    expect(screen.getByTestId("client-logo-claude_code")).toBeInTheDocument();
    expect(
      screen.getByTestId("client-logo-deepseek_harness")
    ).toBeInTheDocument();
    expect(
      screen.getAllByRole("img", { name: "Codex" }).length
    ).toBeGreaterThan(0);
    expect(screen.getByRole("img", { name: "Kimi CLI" })).toBeInTheDocument();
  });

  it("runs connection test", async () => {
    render(
      <MemoryRouter>
        <Tools />
      </MemoryRouter>
    );

    await screen.findByText("tools.test_connection");

    const testBtn = screen.getByText("tools.test_connection");
    await act(async () => testBtn.click());

    await waitFor(() => expect(api.testToolConnection).toHaveBeenCalled());
  });
  it("applies Codex config after confirmation and shows the post-apply summary", async () => {
    vi.mocked(api.applyCodexConfig).mockResolvedValue({
      success: true,
      config_path: "/home/u/.codex/config.toml",
    } as any);
    render(
      <MemoryRouter>
        <Tools />
      </MemoryRouter>
    );

    fireEvent.click(await screen.findByText("tools.apply_config"));
    expect(screen.getByText("tools.apply_codex_title")).toBeInTheDocument();
    expect(api.applyCodexConfig).not.toHaveBeenCalled();

    await act(async () => fireEvent.click(screen.getByText("common.apply")));

    expect(api.applyCodexConfig).toHaveBeenCalledTimes(1);
    expect(api.detectClientRunning).toHaveBeenCalledWith("codex");
    expect(
      await screen.findByText("/home/u/.codex/config.toml")
    ).toBeInTheDocument();
    expect(screen.queryByText("tools.apply_codex_title")).toBeNull();
  });

  it("applies Gemini CLI config through its own confirmation", async () => {
    vi.mocked(api.applyGeminiConfig).mockResolvedValue({
      success: true,
      config_path: "/home/u/.gemini/settings.json",
    } as any);
    render(
      <MemoryRouter>
        <Tools />
      </MemoryRouter>
    );

    fireEvent.click(await screen.findByText("tools.gemini_cli"));
    fireEvent.click(await screen.findByText("tools.apply_config"));
    expect(screen.getByText("tools.apply_gemini_title")).toBeInTheDocument();
    expect(screen.getByText("tools.apply_gemini_msg")).toBeInTheDocument();

    await act(async () => fireEvent.click(screen.getByText("common.apply")));

    expect(api.applyGeminiConfig).toHaveBeenCalledTimes(1);
    expect(api.applyCodexConfig).not.toHaveBeenCalled();
    expect(api.detectClientRunning).toHaveBeenCalledWith("gemini");
    expect(
      await screen.findByText("/home/u/.gemini/settings.json")
    ).toBeInTheDocument();
  });

  it("keeps the confirmation open and toasts when apply fails", async () => {
    vi.mocked(api.applyCodexConfig).mockRejectedValue({
      message: "config locked",
    });
    render(
      <MemoryRouter>
        <Tools />
        <ToastContainer />
      </MemoryRouter>
    );

    fireEvent.click(await screen.findByText("tools.apply_config"));
    await act(async () => fireEvent.click(screen.getByText("common.apply")));

    expect(await screen.findByText("config locked")).toBeInTheDocument();
    expect(screen.getByText("tools.apply_codex_title")).toBeInTheDocument();
  });

  it("reads gateway status from the shared store instead of fetching it", async () => {
    useGatewayStatus.setState({
      value: { ...gatewayStatus(), running: false },
      loading: false,
      error: null,
    });
    vi.mocked(api.startGateway).mockResolvedValue(gatewayStatus());
    render(
      <MemoryRouter>
        <Tools />
      </MemoryRouter>
    );

    expect(
      await screen.findByText("tools.gateway_not_running_hint")
    ).toBeInTheDocument();
    expect(api.getGatewayStatus).not.toHaveBeenCalled();

    await act(async () => fireEvent.click(screen.getByText("gateway.start")));

    expect(useGatewayStatus.getState().value?.running).toBe(true);
    expect(
      await screen.findByText(
        /tools\.gateway_running http:\/\/127\.0\.0\.1:4141/
      )
    ).toBeInTheDocument();
  });

  it("re-detects client configs and processes every 60s", async () => {
    vi.mocked(api.detectCodexConfig).mockResolvedValue({
      exists: true,
      has_agentgate: true,
    } as any);
    vi.useFakeTimers();
    try {
      render(
        <MemoryRouter>
          <Tools />
        </MemoryRouter>
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      vi.mocked(api.detectCodexConfig).mockClear();
      vi.mocked(api.detectClientRunning).mockClear();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(59_999);
      });
      expect(api.detectCodexConfig).not.toHaveBeenCalled();
      expect(api.detectClientRunning).not.toHaveBeenCalled();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(api.detectCodexConfig).toHaveBeenCalledTimes(1);
      expect(api.detectClientRunning).toHaveBeenCalledWith("codex");
    } finally {
      vi.useRealTimers();
    }
  });
});
