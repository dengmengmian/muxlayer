import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render,
  act,
  waitFor,
  screen,
  fireEvent,
  cleanup,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@/lib/api");
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  readText: vi.fn().mockResolvedValue(""),
  writeText: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/lib/providerAutoSetup", () => ({
  fetchDetectAndPersistProviderModels: vi
    .fn()
    .mockResolvedValue({ models: [{ id: "gpt-test" }] }),
}));

import * as api from "@/lib/api";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { QuickSetup } from "./QuickSetup";

afterEach(() => cleanup());

async function goThroughKeyAndTools() {
  render(
    <MemoryRouter>
      <QuickSetup />
    </MemoryRouter>
  );

  const input = await screen.findByPlaceholderText(/sk-xxx/);
  fireEvent.change(input, { target: { value: "sk-testkey" } });
  await screen.findByRole("combobox");

  const next = screen.getByText("onboarding.next");
  await act(async () => next.click());

  expect(
    await screen.findByRole("heading", { name: "onboarding.select_tools" })
  ).toBeInTheDocument();
}

describe("QuickSetup", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.detectCodexConfig).mockResolvedValue({
      exists: true,
    } as any);
    vi.mocked(api.detectClaudeCodeEnv).mockResolvedValue({
      settings_exists: false,
      has_api_key: false,
      has_auth_token: false,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectOpenCodeConfig).mockResolvedValue({
      exists: false,
    } as any);
    vi.mocked(api.detectGeminiConfig).mockResolvedValue({
      exists: false,
    } as any);
    vi.mocked(api.detectAtomCodeConfig).mockResolvedValue({
      exists: false,
    } as any);
    vi.mocked(api.createProvider).mockResolvedValue({
      id: "p1",
      name: "OpenAI",
      provider_type: "openai",
    } as any);
    vi.mocked(api.setActiveProvider).mockResolvedValue({} as any);
    vi.mocked(api.startGateway).mockResolvedValue({ running: true } as any);
    vi.mocked(api.getGatewayStatus).mockResolvedValue({
      running: true,
    } as any);
    vi.mocked(api.applyCodexConfig).mockResolvedValue({ success: true } as any);
    vi.mocked(api.applyClaudeCodeConfig).mockResolvedValue({
      success: true,
    } as any);
    vi.mocked(api.applyOpenCodeConfig).mockResolvedValue({
      success: true,
    } as any);
    vi.mocked(api.applyGeminiConfig).mockResolvedValue({
      success: true,
    } as any);
    vi.mocked(api.applyAtomCodeConfig).mockResolvedValue({
      success: true,
    } as any);
    vi.mocked(api.testToolConnection).mockResolvedValue({
      config_ok: true,
      gateway_ok: true,
      provider_ok: true,
    } as any);
  });

  it("renders key step and detects provider", async () => {
    render(
      <MemoryRouter>
        <QuickSetup />
      </MemoryRouter>
    );

    expect(await screen.findByText("onboarding.welcome")).toBeInTheDocument();

    const input = screen.getByPlaceholderText(/sk-xxx/);
    fireEvent.change(input, { target: { value: "sk-testkey" } });

    const select = await screen.findByRole("combobox");
    expect(select).toHaveValue("openai");
  });

  it("offers a detected clipboard key on mount without filling it silently", async () => {
    vi.mocked(readText).mockResolvedValue("sk-ant-api03-secretsecret");
    render(
      <MemoryRouter>
        <QuickSetup />
      </MemoryRouter>
    );

    expect(
      await screen.findByText(/onboarding\.clipboard_hint/)
    ).toBeInTheDocument();
    expect(screen.getByPlaceholderText(/sk-xxx/)).toHaveValue("");

    fireEvent.click(
      screen.getByRole("button", { name: "onboarding.clipboard_use" })
    );
    expect(screen.getByPlaceholderText(/sk-xxx/)).toHaveValue(
      "sk-ant-api03-secretsecret"
    );
  });

  it("stays silent when the clipboard cannot be read", async () => {
    vi.mocked(readText).mockRejectedValue(new Error("permission denied"));
    render(
      <MemoryRouter>
        <QuickSetup />
      </MemoryRouter>
    );

    expect(await screen.findByText("onboarding.welcome")).toBeInTheDocument();
    await act(async () => {});
    expect(screen.queryByText(/onboarding\.clipboard_hint/)).toBeNull();
  });

  it("masks the API key with a show/hide toggle", async () => {
    render(
      <MemoryRouter>
        <QuickSetup />
      </MemoryRouter>
    );

    const input = await screen.findByPlaceholderText(/sk-xxx/);
    expect(input).toHaveAttribute("type", "password");

    fireEvent.click(screen.getByRole("button", { name: "providers.show_key" }));
    expect(input).toHaveAttribute("type", "text");

    fireEvent.click(screen.getByRole("button", { name: "providers.hide_key" }));
    expect(input).toHaveAttribute("type", "password");
  });

  it("completes only when provider+gateway+client+probe ok and shows first-request command", async () => {
    await goThroughKeyAndTools();

    // Uncheck Claude Code (always pre-checked) so only Codex is applied.
    const claudeLabel = screen.getByText("Claude Code").closest("label");
    expect(claudeLabel).toBeTruthy();
    await act(async () => {
      fireEvent.click(claudeLabel!);
    });

    const setup = screen.getAllByText("onboarding.start_setup").pop()!;
    await act(async () => setup.click());

    await waitFor(() => expect(api.createProvider).toHaveBeenCalled());
    await waitFor(() => expect(api.applyCodexConfig).toHaveBeenCalled());
    await waitFor(() => expect(api.testToolConnection).toHaveBeenCalled());

    expect(await screen.findByText("onboarding.complete")).toBeInTheDocument();
    expect(
      screen.getByText("onboarding.first_request_title")
    ).toBeInTheDocument();
    // First-request command for Codex
    expect(screen.getByText("codex")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "onboarding.go_to_clients" })
    ).toBeInTheDocument();
  });

  it("does not mark complete when probe fails; primary CTA opens logs", async () => {
    vi.mocked(api.testToolConnection).mockResolvedValue({
      config_ok: true,
      gateway_ok: true,
      provider_ok: false,
      error: "provider probe failed",
    } as any);

    await goThroughKeyAndTools();
    const setup = screen.getAllByText("onboarding.start_setup").pop()!;
    await act(async () => setup.click());

    await waitFor(() => expect(api.testToolConnection).toHaveBeenCalled());

    expect(
      await screen.findByText("onboarding.incomplete")
    ).toBeInTheDocument();
    expect(screen.queryByText("onboarding.complete")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "onboarding.next_logs" })
    ).toBeInTheDocument();
  });

  it("does not green gateway when start fails and status is not running", async () => {
    vi.mocked(api.startGateway).mockRejectedValue(new Error("bind failed"));
    vi.mocked(api.getGatewayStatus).mockResolvedValue({
      running: false,
    } as any);

    await goThroughKeyAndTools();
    const setup = screen.getAllByText("onboarding.start_setup").pop()!;
    await act(async () => setup.click());

    await waitFor(() => expect(api.startGateway).toHaveBeenCalled());
    await waitFor(() => expect(api.getGatewayStatus).toHaveBeenCalled());

    expect(
      await screen.findByText("onboarding.incomplete")
    ).toBeInTheDocument();
    // Primary CTA for gateway failure is retry
    expect(
      screen.getByRole("button", { name: "onboarding.next_retry" })
    ).toBeInTheDocument();
    // Must not claim full success
    expect(screen.queryByText("onboarding.complete")).not.toBeInTheDocument();
  });

  it("shows providers CTA when create provider fails", async () => {
    vi.mocked(api.createProvider).mockRejectedValue(new Error("bad key"));

    await goThroughKeyAndTools();
    const setup = screen.getAllByText("onboarding.start_setup").pop()!;
    await act(async () => setup.click());

    await waitFor(() => expect(api.createProvider).toHaveBeenCalled());

    expect(
      await screen.findByText("onboarding.incomplete")
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "onboarding.next_providers" })
    ).toBeInTheDocument();
  });
});
