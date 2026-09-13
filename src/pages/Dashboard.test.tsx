import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, act, waitFor, screen, cleanup } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@/lib/api");

import * as api from "@/lib/api";
import { Dashboard } from "./Dashboard";
import { __resetGlobalStoresForTest, useGatewayStatus } from "@/store/global";

afterEach(() => cleanup());

function gatewayStatus(): any {
  return {
    running: true,
    host: "127.0.0.1",
    port: 4141,
    input_protocol: "openai_responses",
    output_protocol: "openai_chat_completions",
    active_provider: "OpenAI",
    started_at: "2026-06-16T00:00:00Z",
  };
}

function gatewaySettings(): any {
  return {
    host: "127.0.0.1",
    port: 4141,
    input_protocol: "openai_responses",
    output_protocol: "openai_chat_completions",
    auto_start: true,
    log_retention_days: 30,
  };
}

describe("Dashboard", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.mocked(api.listTools).mockResolvedValue([]);
    vi.mocked(api.listRequestLogs).mockResolvedValue([]);
    vi.mocked(api.getRequestStatsRange).mockResolvedValue({ total: 0 } as any);
    vi.mocked(api.aggregateCostByModel).mockResolvedValue([]);
    vi.mocked(api.aggregateCostByClient).mockResolvedValue([]);
    vi.mocked(api.aggregateRouteProfileStats).mockResolvedValue([]);
    vi.mocked(api.getGatewayStatus).mockResolvedValue(gatewayStatus());
    vi.mocked(api.getGatewaySettings).mockResolvedValue(gatewaySettings());
    vi.mocked(api.getRuntimeKpis).mockResolvedValue({
      active_requests: 0,
      uptime_seconds: 0,
      total_requests: 0,
      total_tokens: 0,
      total_cost: 0,
      success_rate_lifetime: 100,
      gateway_running: false,
    } as any);
    vi.mocked(api.listProviders).mockResolvedValue([]);
    vi.mocked(api.listRouteProfiles).mockResolvedValue([]);
    vi.mocked(api.startGateway).mockResolvedValue(gatewayStatus());
    vi.mocked(api.stopGateway).mockResolvedValue({
      ...gatewayStatus(),
      running: false,
    });
    vi.mocked(api.restartGateway).mockResolvedValue(gatewayStatus());
    // Client detects: default = not MuxLayer-wired (even if config files exist).
    vi.mocked(api.detectCodexConfig).mockResolvedValue({
      exists: true,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectClaudeCodeEnv).mockResolvedValue({
      settings_exists: true,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectOpenCodeConfig).mockResolvedValue({
      exists: true,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectGeminiConfig).mockResolvedValue({
      exists: true,
      has_agentgate: false,
    } as any);
    vi.mocked(api.detectAtomCodeConfig).mockResolvedValue({
      exists: true,
      has_agentgate: false,
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
  });

  it("renders and fetches initial data", async () => {
    vi.mocked(api.listProviders).mockResolvedValue([{ id: "p1" }] as any);
    vi.mocked(api.getRequestStatsRange).mockResolvedValue({
      total: 1,
      today_total: 1,
      today_errors: 0,
      today_input_tokens: 100,
      today_output_tokens: 50,
      today_cost: 0.01,
      avg_latency_ms: 1000,
      today_codex_compact: 0,
      today_cache_read_tokens: 0,
      today_cache_write_tokens: 0,
      daily: [
        {
          date: "2026-07-07",
          total: 1,
          errors: 0,
          input_tokens: 100,
          output_tokens: 50,
        },
      ],
      providers: [{ name: "OpenAI", count: 1 }],
    } as any);

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );

    await waitFor(() => {
      expect(api.listTools).toHaveBeenCalled();
      expect(api.listRequestLogs).toHaveBeenCalledWith({ limit: 5 });
      expect(api.getRequestStatsRange).toHaveBeenCalledWith(7);
      expect(api.detectCodexConfig).toHaveBeenCalled();
    });
    expect(screen.getByText("dashboard.control_console")).toBeInTheDocument();
    expect(screen.getByText("stats.today_realtime")).toBeInTheDocument();
    expect(screen.getByText("stats.hit_rate")).toBeInTheDocument();
    expect(screen.getByText("stats.traffic_monitor")).toBeInTheDocument();
  });

  it("shows a running/info gateway indicator while the gateway is running", async () => {
    useGatewayStatus.setState({
      value: gatewayStatus(),
      loading: false,
      error: null,
    });

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );

    const indicator = await screen.findByRole("status", {
      name: "topbar.running",
    });
    expect(indicator).toHaveClass("bg-info");
    expect(indicator).not.toHaveClass("bg-success");
  });

  it("shows a stopped gateway indicator while the gateway is stopped", async () => {
    const stopped = { ...gatewayStatus(), running: false };
    vi.mocked(api.getGatewayStatus).mockResolvedValue(stopped);
    useGatewayStatus.setState({
      value: stopped,
      loading: false,
      error: null,
    });

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );

    const indicator = await screen.findByRole("status", {
      name: "topbar.stopped",
    });
    expect(indicator).toHaveClass("bg-text-muted");
    expect(indicator).not.toHaveClass("animate-pulse-dot");
  });

  it("shows cache hit rate as a first-class today metric", async () => {
    vi.mocked(api.listProviders).mockResolvedValue([
      { id: "p1", name: "OpenAI", enabled: true, masked_api_key: "sk-***" },
    ] as any);
    vi.mocked(api.getRequestStatsRange).mockResolvedValue({
      total: 1,
      today_total: 1,
      today_errors: 0,
      today_input_tokens: 20,
      today_output_tokens: 10,
      today_cost: 0.01,
      avg_latency_ms: 100,
      today_codex_compact: 0,
      today_cache_read_tokens: 70,
      today_cache_write_tokens: 10,
      daily: [],
      providers: [{ name: "OpenAI", count: 1 }],
    } as any);

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );

    await waitFor(() => {
      expect(screen.getAllByText("stats.hit_rate").length).toBeGreaterThan(0);
    });
    expect(screen.getAllByText("70.0%").length).toBeGreaterThan(0);
  });

  it("stops gateway when stop button is clicked", async () => {
    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );

    const stop = await screen.findByText("dashboard.stop");
    await act(async () => stop.click());
    await waitFor(() => expect(api.stopGateway).toHaveBeenCalled());
  });

  it("treats seeded provider without key as not ready — shows setup CTA", async () => {
    // Fresh install seeds DeepSeek with empty key; local client files may exist.
    vi.mocked(api.listProviders).mockResolvedValue([
      {
        id: "p1",
        name: "DeepSeek",
        enabled: true,
        masked_api_key: null,
      },
    ] as any);
    vi.mocked(api.listTools).mockResolvedValue([
      {
        id: "codex",
        name: "Codex",
        slug: "codex",
        config_exists: true,
      },
      {
        id: "claude-code",
        name: "Claude Code",
        slug: "claude-code",
        config_exists: true,
      },
    ] as any);
    vi.mocked(api.getRequestStatsRange).mockResolvedValue({
      total: 0,
      today_total: 0,
    } as any);

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );

    expect(
      await screen.findByText("dashboard.empty_title")
    ).toBeInTheDocument();
    expect(screen.queryByText("dashboard.no_requests_ready_title")).toBeNull();
    expect(screen.queryByText("codex")).toBeNull();
  });

  it("asks to apply clients when key exists but no MuxLayer-wired client", async () => {
    vi.mocked(api.listProviders).mockResolvedValue([
      {
        id: "p1",
        name: "DeepSeek",
        enabled: true,
        masked_api_key: "sk-s****abcd",
      },
    ] as any);
    // config_exists=true for all tools must NOT mean ready
    vi.mocked(api.listTools).mockResolvedValue([
      { id: "codex", slug: "codex", config_exists: true },
      { id: "claude-code", slug: "claude-code", config_exists: true },
      { id: "opencode", slug: "opencode", config_exists: true },
      { id: "atomcode", slug: "atomcode", config_exists: true },
      { id: "gemini_cli", slug: "gemini-cli", config_exists: true },
    ] as any);
    vi.mocked(api.getRequestStatsRange).mockResolvedValue({
      total: 0,
      today_total: 0,
    } as any);

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );

    expect(
      await screen.findByText("dashboard.no_requests_config_title")
    ).toBeInTheDocument();
    expect(screen.queryByText("dashboard.no_requests_ready_title")).toBeNull();
    // Must not list every local client as launchable
    expect(screen.queryByText("opencode")).toBeNull();
    expect(screen.queryByText("atomcode")).toBeNull();
    expect(screen.queryByText("gemini")).toBeNull();
  });

  it("shows first-request commands only for MuxLayer-wired clients", async () => {
    vi.mocked(api.listProviders).mockResolvedValue([
      {
        id: "p1",
        name: "DeepSeek",
        enabled: true,
        masked_api_key: "sk-s****abcd",
      },
    ] as any);
    vi.mocked(api.listTools).mockResolvedValue([
      { id: "codex", slug: "codex", config_exists: true },
      { id: "claude-code", slug: "claude-code", config_exists: true },
      { id: "opencode", slug: "opencode", config_exists: true },
    ] as any);
    vi.mocked(api.detectCodexConfig).mockResolvedValue({
      exists: true,
      has_agentgate: true,
    } as any);
    vi.mocked(api.detectClaudeCodeEnv).mockResolvedValue({
      settings_exists: true,
      has_agentgate: false,
    } as any);
    vi.mocked(api.getRequestStatsRange).mockResolvedValue({
      total: 0,
      today_total: 0,
      today_errors: 0,
      today_input_tokens: 0,
      today_output_tokens: 0,
      today_cost: 0,
      avg_latency_ms: 0,
      today_codex_compact: 0,
      today_cache_read_tokens: 0,
      today_cache_write_tokens: 0,
      daily: [],
      providers: [],
    } as any);

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );

    expect(
      await screen.findByText("dashboard.no_requests_ready_title")
    ).toBeInTheDocument();
    expect(screen.getByText("codex")).toBeInTheDocument();
    // Claude / OpenCode not wired → must not appear
    expect(screen.queryByText("claude")).toBeNull();
    expect(screen.queryByText("opencode")).toBeNull();
    expect(
      screen.getByText("dashboard.no_requests_ready_cta").closest("a")
    ).toHaveAttribute("href", "/tools");
  });
  it("keeps the latest range's data when an older request resolves last", async () => {
    vi.mocked(api.listProviders).mockResolvedValue([
      { id: "p1", name: "OpenAI", enabled: true, masked_api_key: "sk-***" },
    ] as any);
    vi.mocked(api.getRequestStatsRange).mockResolvedValue({
      total: 3,
      today_total: 0,
      today_errors: 0,
      today_input_tokens: 0,
      today_output_tokens: 0,
      today_cost: 0,
      avg_latency_ms: 0,
      today_codex_compact: 0,
      today_cache_read_tokens: 0,
      today_cache_write_tokens: 0,
      daily: [],
      providers: [],
    } as any);
    const costRow = (key: string) => ({
      key,
      provider: null,
      request_count: 1,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      cost: 0.5,
      has_price: true,
    });
    let sevenDayCalls = 0;
    let resolveStale7d: ((rows: any) => void) | null = null;
    vi.mocked(api.aggregateCostByModel).mockImplementation((days) => {
      if (days === 30) return Promise.resolve([costRow("model-30d")]);
      sevenDayCalls += 1;
      if (sevenDayCalls === 1) return Promise.resolve([costRow("model-7d")]);
      return new Promise((resolve) => {
        resolveStale7d = resolve;
      });
    });

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>
    );
    expect(await screen.findByText("model-7d")).toBeInTheDocument();

    // 7d 的轮询请求挂起中，用户切到 30d。
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(resolveStale7d).not.toBeNull());
    const detectCallsBeforeSwitch = vi.mocked(api.detectCodexConfig).mock.calls
      .length;
    await act(async () => screen.getByRole("button", { name: "30d" }).click());
    expect(await screen.findByText("model-30d")).toBeInTheDocument();
    // 切换时间范围只重拉统计，不重新探测客户端配置。
    expect(api.detectCodexConfig).toHaveBeenCalledTimes(
      detectCallsBeforeSwitch
    );

    await act(async () => {
      resolveStale7d!([costRow("model-7d-stale")]);
    });
    expect(screen.queryByText("model-7d-stale")).toBeNull();
    expect(screen.getByText("model-30d")).toBeInTheDocument();
  });

  it("does not restart a slow stats request on poll ticks and applies its result", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(api.listProviders).mockResolvedValue([
        { id: "p1", name: "OpenAI", enabled: true, masked_api_key: "sk-***" },
      ] as any);
      vi.mocked(api.getRequestStatsRange).mockResolvedValue({
        total: 3,
        today_total: 0,
        today_errors: 0,
        today_input_tokens: 0,
        today_output_tokens: 0,
        today_cost: 0,
        avg_latency_ms: 0,
        today_codex_compact: 0,
        today_cache_read_tokens: 0,
        today_cache_write_tokens: 0,
        daily: [],
        providers: [],
      } as any);
      const pending: ((rows: any) => void)[] = [];
      vi.mocked(api.aggregateCostByModel).mockClear();
      vi.mocked(api.aggregateCostByModel).mockImplementation(
        () =>
          new Promise((resolve) => {
            pending.push(resolve);
          })
      );

      render(
        <MemoryRouter>
          <Dashboard />
        </MemoryRouter>
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(api.aggregateCostByModel).toHaveBeenCalledTimes(1);

      // 聚合查询比 5s 轮询周期还慢：期间的 tick 不应再发新请求。
      await act(async () => {
        await vi.advanceTimersByTimeAsync(15_000);
      });
      expect(api.aggregateCostByModel).toHaveBeenCalledTimes(1);

      await act(async () => {
        pending[0]([
          {
            key: "model-slow",
            provider: null,
            request_count: 1,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost: 0.5,
            has_price: true,
          },
        ]);
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(screen.getByText("model-slow")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("re-detects client configs every 60s", async () => {
    vi.useFakeTimers();
    try {
      render(
        <MemoryRouter>
          <Dashboard />
        </MemoryRouter>
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      vi.mocked(api.detectCodexConfig).mockClear();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(59_999);
      });
      expect(api.detectCodexConfig).not.toHaveBeenCalled();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(api.detectCodexConfig).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
