import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import type { BindingsInput } from "./api";
import {
  listProviders,
  getProvider,
  createProvider,
  updateProvider,
  deleteProvider,
  setActiveProvider,
  fetchProviderModels,
  testProvider,
  detectProviderVision,
  getGatewayStatus,
  getGatewaySettings,
  updateGatewaySettings,
  startGateway,
  stopGateway,
  listRequestLogs,
  aggregateProviderDetailStats,
  clearRequestLogs,
  listTools,
  getGatewayAuthSettings,
  regenerateLocalAccessToken,
  getLocalAccessToken,
  detectCodexConfig,
  applyCodexConfig,
  detectClaudeCodeEnv,
  generateClaudeCodeEnv,
} from "./api";

describe("API client", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("listProviders invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue([{ id: "p1" }]);
    const result = await listProviders();
    expect(invoke).toHaveBeenCalledWith("list_providers");
    expect(result).toEqual([{ id: "p1" }]);
  });

  it("getProvider passes id arg", async () => {
    vi.mocked(invoke).mockResolvedValue({ id: "p1" });
    const result = await getProvider("p1");
    expect(invoke).toHaveBeenCalledWith("get_provider", { id: "p1" });
    expect(result).toEqual({ id: "p1" });
  });

  it("createProvider passes input arg", async () => {
    vi.mocked(invoke).mockResolvedValue({ id: "p1" });
    const input = { name: "Test", provider_type: "openai" } as any;
    const result = await createProvider(input);
    expect(invoke).toHaveBeenCalledWith("create_provider", { input });
    expect(result).toEqual({ id: "p1" });
  });

  it("updateProvider passes id and input", async () => {
    vi.mocked(invoke).mockResolvedValue({ id: "p1" });
    const result = await updateProvider("p1", { name: "New" });
    expect(invoke).toHaveBeenCalledWith("update_provider", {
      id: "p1",
      input: { name: "New" },
    });
    expect(result).toEqual({ id: "p1" });
  });

  it("deleteProvider passes id", async () => {
    vi.mocked(invoke).mockResolvedValue(true);
    const result = await deleteProvider("p1");
    expect(invoke).toHaveBeenCalledWith("delete_provider", { id: "p1" });
    expect(result).toBe(true);
  });

  it("setActiveProvider passes id", async () => {
    vi.mocked(invoke).mockResolvedValue({ id: "p1" });
    const result = await setActiveProvider("p1");
    expect(invoke).toHaveBeenCalledWith("set_active_provider", { id: "p1" });
    expect(result).toEqual({ id: "p1" });
  });

  it("fetchProviderModels passes id", async () => {
    vi.mocked(invoke).mockResolvedValue(["m1"]);
    const result = await fetchProviderModels("p1");
    expect(invoke).toHaveBeenCalledWith("fetch_provider_models", { id: "p1" });
    expect(result).toEqual(["m1"]);
  });

  it("testProvider passes id", async () => {
    vi.mocked(invoke).mockResolvedValue({ success: true });
    const result = await testProvider("p1");
    expect(invoke).toHaveBeenCalledWith("test_provider", { id: "p1" });
    expect(result).toEqual({ success: true });
  });

  it("detectProviderVision passes id", async () => {
    vi.mocked(invoke).mockResolvedValue({ success: true });
    const result = await detectProviderVision("p1");
    expect(invoke).toHaveBeenCalledWith("detect_provider_vision", { id: "p1" });
    expect(result).toEqual({ success: true });
  });

  it("getGatewayStatus invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ running: true });
    const result = await getGatewayStatus();
    expect(invoke).toHaveBeenCalledWith("get_gateway_status");
    expect(result).toEqual({ running: true });
  });

  it("getGatewaySettings invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ port: 9090 });
    const result = await getGatewaySettings();
    expect(invoke).toHaveBeenCalledWith("get_gateway_settings");
    expect(result).toEqual({ port: 9090 });
  });

  it("updateGatewaySettings passes input", async () => {
    vi.mocked(invoke).mockResolvedValue({ port: 8080 });
    const input = { port: 8080 };
    const result = await updateGatewaySettings(input as any);
    expect(invoke).toHaveBeenCalledWith("update_gateway_settings", { input });
    expect(result).toEqual({ port: 8080 });
  });

  it("startGateway invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ running: true });
    const result = await startGateway();
    expect(invoke).toHaveBeenCalledWith("start_gateway");
    expect(result).toEqual({ running: true });
  });

  it("stopGateway invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ running: false });
    const result = await stopGateway();
    expect(invoke).toHaveBeenCalledWith("stop_gateway");
    expect(result).toEqual({ running: false });
  });

  it("listRequestLogs passes filter", async () => {
    vi.mocked(invoke).mockResolvedValue({ items: [], total: 0 });
    const result = await listRequestLogs({
      limit: 10,
      offset: 0,
      model: "m1",
      route_profile_id: "rp1",
      error_type: "rate_limited",
    });
    expect(invoke).toHaveBeenCalledWith("list_request_logs", {
      filter: {
        limit: 10,
        offset: 0,
        model: "m1",
        route_profile_id: "rp1",
        error_type: "rate_limited",
      },
    });
    expect(result).toEqual({ items: [], total: 0 });
  });

  it("aggregateProviderDetailStats passes provider window", async () => {
    vi.mocked(invoke).mockResolvedValue({
      provider: "P",
      latency_points: [],
      model_stats: [],
    });
    const result = await aggregateProviderDetailStats("P", 7, 40);
    expect(invoke).toHaveBeenCalledWith("aggregate_provider_detail_stats", {
      provider: "P",
      days: 7,
      limit: 40,
    });
    expect(result).toEqual({
      provider: "P",
      latency_points: [],
      model_stats: [],
    });
  });

  it("clearRequestLogs invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue(true);
    const result = await clearRequestLogs();
    expect(invoke).toHaveBeenCalledWith("clear_request_logs");
    expect(result).toBe(true);
  });

  it("listTools invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    const result = await listTools();
    expect(invoke).toHaveBeenCalledWith("list_tools");
    expect(result).toEqual([]);
  });

  it("getGatewayAuthSettings invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ token_path: "/path" });
    const result = await getGatewayAuthSettings();
    expect(invoke).toHaveBeenCalledWith("get_gateway_auth_settings");
    expect(result).toEqual({ token_path: "/path" });
  });

  it("regenerateLocalAccessToken invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ token_path: "/path" });
    const result = await regenerateLocalAccessToken();
    expect(invoke).toHaveBeenCalledWith("regenerate_local_access_token");
    expect(result).toEqual({ token_path: "/path" });
  });

  it("getLocalAccessToken invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue("token123");
    const result = await getLocalAccessToken();
    expect(invoke).toHaveBeenCalledWith("get_local_access_token");
    expect(result).toBe("token123");
  });

  it("detectCodexConfig invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ exists: true });
    const result = await detectCodexConfig();
    expect(invoke).toHaveBeenCalledWith("detect_codex_config");
    expect(result).toEqual({ exists: true });
  });

  it("applyCodexConfig invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ success: true });
    const result = await applyCodexConfig();
    expect(invoke).toHaveBeenCalledWith("apply_codex_config");
    expect(result).toEqual({ success: true });
  });

  it("detectClaudeCodeEnv invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue({ settings_exists: true });
    const result = await detectClaudeCodeEnv();
    expect(invoke).toHaveBeenCalledWith("detect_claude_code_env");
    expect(result).toEqual({ settings_exists: true });
  });

  it("generateClaudeCodeEnv invokes correct command", async () => {
    vi.mocked(invoke).mockResolvedValue("export FOO=bar");
    const result = await generateClaudeCodeEnv();
    expect(invoke).toHaveBeenCalledWith("generate_claude_code_env");
    expect(result).toBe("export FOO=bar");
  });
});

describe("API error extraction", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("extracts object error", async () => {
    const err = { code: "DB_ERROR", message: "db down" };
    vi.mocked(invoke).mockRejectedValue(err);
    await expect(listProviders()).rejects.toEqual(err);
  });

  it("extracts string error", async () => {
    vi.mocked(invoke).mockRejectedValue("network timeout");
    await expect(getProvider("p1")).rejects.toEqual({
      code: "UNKNOWN",
      message: "network timeout",
    });
  });

  it("falls back to generic message for non-string errors", async () => {
    vi.mocked(invoke).mockRejectedValue(404);
    await expect(createProvider({} as any)).rejects.toEqual({
      code: "UNKNOWN",
      message: "An unexpected error occurred",
    });
  });
});

describe("API error normalization edge cases", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("falls back to generic error when code field is missing", async () => {
    vi.mocked(invoke).mockRejectedValue({ message: "something broke" });
    await expect(listProviders()).rejects.toEqual({
      code: "UNKNOWN",
      message: "An unexpected error occurred",
    });
  });

  it("falls back to generic error when message field is missing", async () => {
    vi.mocked(invoke).mockRejectedValue({ code: "TIMEOUT" });
    await expect(listProviders()).rejects.toEqual({
      code: "UNKNOWN",
      message: "An unexpected error occurred",
    });
  });

  it("handles error object with extra fields (detail, suggestion)", async () => {
    const err = {
      code: "RATE_LIMIT",
      message: "too many requests",
      detail: "retry after 60s",
      suggestion: "slow down",
    };
    vi.mocked(invoke).mockRejectedValue(err);
    await expect(listProviders()).rejects.toEqual(err);
  });
});

describe("BindingsInput boundary type", () => {
  it("lets Option fields be omitted but keeps Rust-required fields required", () => {
    type RustInput = { name: string; note: string | null; added: number };
    const complete: BindingsInput<RustInput> = { name: "x", added: 1 };
    const frontendWithoutNewField: { name: string; note?: string | null } = {
      name: "x",
    };
    // @ts-expect-error `added` 是 Rust 端必填字段，前端 input 缺失时必须编译失败
    const missing: BindingsInput<RustInput> = frontendWithoutNewField;
    expect([complete, missing]).toHaveLength(2);
  });
});
