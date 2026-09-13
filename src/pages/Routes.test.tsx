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
import { Routes } from "./Routes";
import { __resetGlobalStoresForTest } from "@/store/global";

afterEach(() => cleanup());

function profile(id: string): any {
  return {
    id,
    name: "Default Route",
    input_protocol: "openai_responses",
    mode: "manual",
    selection_strategy: "priority",
    is_default: true,
    active_provider_name: "OpenAI",
    providers_count: 1,
  };
}

function profileDetail(id: string): any {
  return {
    profile: profile(id),
    providers: [
      {
        id: "rp1",
        provider_id: "p1",
        provider_name: "OpenAI",
        provider_type: "openai",
        provider_protocol: JSON.stringify(["openai_responses"]),
        priority: 1,
        enabled: true,
        model_override: null,
        routing_conditions: null,
        supports_vision: false,
        supports_cache: null,
        model_capabilities: null,
        has_anthropic_url: false,
        runtime_available: true,
        consecutive_failures: 0,
        cooldown_until: null,
      },
    ],
  };
}

describe("Routes", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.mocked(api.listRouteProfiles).mockResolvedValue([]);
    vi.mocked(api.listProviders).mockResolvedValue([]);
    vi.mocked(api.aggregateRouteProfileStats).mockResolvedValue([]);
    vi.mocked(api.getRouteProfile).mockResolvedValue(profileDetail("r1"));
    vi.mocked(api.setRouteProfileMode).mockResolvedValue(true);
    vi.mocked(api.setDefaultRouteProfile).mockResolvedValue(true);
    vi.mocked(api.createRouteProfile).mockResolvedValue(profile("new"));
    vi.mocked(api.deleteRouteProfile).mockResolvedValue(true);
    vi.mocked(api.updateRouteProfile).mockResolvedValue(profile("r1"));
    vi.mocked(api.setRouteActiveProvider).mockResolvedValue(true);
    vi.mocked(api.addProviderToRoute).mockResolvedValue(true);
    vi.mocked(api.removeProviderFromRoute).mockResolvedValue(true);
    vi.mocked(api.reorderRouteProviders).mockResolvedValue(true);
    vi.mocked(api.hasRouteTemplateRollback).mockResolvedValue(false);
    vi.mocked(api.previewRouteTemplate).mockResolvedValue({
      template_id: "task_split",
      profile_ids: ["r1"],
      profile_names: ["Default Route"],
      roles: [],
      warnings: [],
      can_apply: true,
      can_rollback: false,
      switches_to_failover: true,
    } as never);
    vi.mocked(api.applyRouteTemplate).mockResolvedValue({} as never);
    vi.mocked(api.rollbackRouteTemplate).mockResolvedValue(true);
    vi.mocked(api.resetProviderRuntimeStatus).mockResolvedValue({
      provider_id: "p1",
      available: true,
      consecutive_failures: 0,
      cooldown_until: null,
      last_failure_code: null,
      last_failure_message: null,
      last_probe_at: null,
      last_probe_success: null,
      last_probe_latency_ms: null,
      last_probe_error: null,
    } as any);
  });

  it("renders empty state when no route profiles", async () => {
    render(
      <MemoryRouter>
        <Routes />
      </MemoryRouter>
    );

    expect(await screen.findByText("routes.no_profiles")).toBeInTheDocument();
  });

  it("loads first profile detail and toggles mode", async () => {
    vi.mocked(api.listRouteProfiles).mockResolvedValue([profile("r1")]);
    vi.mocked(api.listProviders).mockResolvedValue([
      { id: "p1", name: "OpenAI", enabled: true },
    ] as any);

    render(
      <MemoryRouter>
        <Routes />
      </MemoryRouter>
    );

    await waitFor(() => expect(api.getRouteProfile).toHaveBeenCalledWith("r1"));
    expect(screen.getAllByText("Default Route").length).toBeGreaterThanOrEqual(
      1
    );
    expect(screen.getByText(/routes\.current_provider/)).toBeInTheDocument();
    expect(screen.getByText("routes.route_result")).toBeInTheDocument();
    expect(screen.getByText("routes.match_settings")).toBeInTheDocument();
    expect(screen.getByText("routes.route_metrics")).toBeInTheDocument();
    expect(
      screen.getByText("OpenAI Responses (Codex) → OpenAI")
    ).toBeInTheDocument();
    expect(screen.getByText("routes.mode_plain_manual")).toBeInTheDocument();

    const failover = screen.getByText("routes.mode_failover");
    await act(async () => failover.click());

    await waitFor(() =>
      expect(api.setRouteProfileMode).toHaveBeenCalledWith("r1", "failover")
    );
  });

  it("summarizes route availability and disabled providers", async () => {
    vi.mocked(api.listRouteProfiles).mockResolvedValue([profile("r1")]);
    vi.mocked(api.listProviders).mockResolvedValue([
      { id: "p1", name: "OpenAI", enabled: false },
    ] as any);

    render(
      <MemoryRouter>
        <Routes />
      </MemoryRouter>
    );

    expect(await screen.findByText("routes.availability")).toBeInTheDocument();
    expect(screen.getByText("0 / 1")).toBeInTheDocument();
    expect(
      screen.getByText("routes.reason_provider_disabled")
    ).toBeInTheDocument();
  });

  it("creates and renames a route profile", async () => {
    vi.mocked(api.listRouteProfiles).mockResolvedValue([profile("r1")]);

    render(
      <MemoryRouter>
        <Routes />
      </MemoryRouter>
    );

    expect(
      (await screen.findAllByText("Default Route")).length
    ).toBeGreaterThan(0);
    await act(async () => screen.getByText("routes.create_profile").click());

    const nameInput = screen.getByPlaceholderText("My Route");
    await act(async () => {
      fireEvent.change(nameInput, { target: { value: "Custom Route" } });
    });
    await act(async () => screen.getByText("common.save").click());

    await waitFor(() =>
      expect(api.createRouteProfile).toHaveBeenCalledWith(
        expect.objectContaining({ name: "Custom Route" })
      )
    );

    await act(async () => screen.getByTitle("routes.rename_profile").click());
    const renameInput = screen.getByDisplayValue("Default Route");
    await act(async () => {
      fireEvent.change(renameInput, { target: { value: "Renamed Route" } });
    });
    const saveRename = renameInput.parentElement!.querySelector("button")!;
    await act(async () => saveRename.click());

    await waitFor(() =>
      expect(api.updateRouteProfile).toHaveBeenCalledWith("r1", {
        name: "Renamed Route",
      })
    );
  });

  it("adds, removes, and deletes route providers with confirmation for profile delete", async () => {
    vi.mocked(api.listRouteProfiles).mockResolvedValue([profile("r1")]);
    vi.mocked(api.listProviders).mockResolvedValue([
      {
        id: "p2",
        name: "Fallback Provider",
        provider_type: "openai",
        protocol: JSON.stringify(["openai_responses"]),
      },
    ] as any);

    render(
      <MemoryRouter>
        <Routes />
      </MemoryRouter>
    );

    expect((await screen.findAllByText("OpenAI")).length).toBeGreaterThan(0);
    const addSelect = screen.getByDisplayValue(
      "routes.add_provider"
    ) as HTMLSelectElement;
    await act(async () => {
      fireEvent.change(addSelect, { target: { value: "p2" } });
    });
    await act(async () => screen.getByText("routes.add").click());
    await waitFor(() =>
      expect(api.addProviderToRoute).toHaveBeenCalledWith("r1", "p2", {})
    );

    await act(async () => screen.getByTitle("common.delete").click());
    await waitFor(() =>
      expect(api.removeProviderFromRoute).toHaveBeenCalledWith("r1", "p1")
    );

    await act(async () => screen.getByTitle("routes.delete_profile").click());
    expect(await screen.findByText("routes.delete_title")).toBeInTheDocument();
    const deleteButtons = screen.getAllByRole("button", {
      name: "common.delete",
    });
    const confirmDelete = deleteButtons[deleteButtons.length - 1];
    await act(async () => confirmDelete.click());
    await waitFor(() =>
      expect(api.deleteRouteProfile).toHaveBeenCalledWith("r1")
    );
  });

  it("opens the routing template dialog from the header", async () => {
    vi.mocked(api.listRouteProfiles).mockResolvedValue([profile("r1")]);
    vi.mocked(api.listProviders).mockResolvedValue([
      {
        id: "p1",
        name: "OpenAI",
        base_url: "https://api.openai.com",
        enabled: true,
      },
    ] as any);

    render(
      <MemoryRouter>
        <Routes />
      </MemoryRouter>
    );

    await waitFor(() => expect(api.getRouteProfile).toHaveBeenCalledWith("r1"));
    await act(async () => screen.getByText("routes.template").click());
    expect(
      await screen.findByText("routes.template_title")
    ).toBeInTheDocument();
  });
  it("keeps the clicked profile when an older poll resolves last", async () => {
    const named = (id: string, name: string) => ({
      ...profileDetail(id),
      profile: { ...profile(id), name },
    });
    vi.mocked(api.listRouteProfiles).mockResolvedValue([
      named("r1", "Route One").profile,
      named("r2", "Route Two").profile,
    ]);
    let r1Calls = 0;
    let resolveStaleR1: ((d: any) => void) | null = null;
    vi.mocked(api.getRouteProfile).mockImplementation((id) => {
      if (id === "r2") return Promise.resolve(named("r2", "Route Two"));
      r1Calls += 1;
      if (r1Calls === 1) return Promise.resolve(named("r1", "Route One"));
      return new Promise((resolve) => {
        resolveStaleR1 = resolve;
      });
    });

    render(
      <MemoryRouter>
        <Routes />
      </MemoryRouter>
    );
    const profileButton = async (name: string) =>
      (await screen.findByText(new RegExp(`^${name} · `))).closest("button")!;
    await waitFor(async () =>
      expect(await profileButton("Route One")).toHaveClass("border-accent/40")
    );

    // 轮询在读 r1 详情时挂起，用户点了 r2。
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(resolveStaleR1).not.toBeNull());
    await act(async () => (await profileButton("Route Two")).click());
    await waitFor(async () =>
      expect(await profileButton("Route Two")).toHaveClass("border-accent/40")
    );

    await act(async () => {
      resolveStaleR1!(named("r1", "Route One"));
    });
    expect(await profileButton("Route Two")).toHaveClass("border-accent/40");
    expect(await profileButton("Route One")).not.toHaveClass(
      "border-accent/40"
    );
  });
  it("does not restart a slow poll on later ticks and applies its result", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(api.listRouteProfiles).mockResolvedValue([profile("r1")]);
      const pending: ((d: any) => void)[] = [];
      let detailCalls = 0;
      vi.mocked(api.getRouteProfile).mockImplementation(() => {
        detailCalls += 1;
        if (detailCalls === 1) return Promise.resolve(profileDetail("r1"));
        return new Promise((resolve) => {
          pending.push(resolve);
        });
      });

      render(
        <MemoryRouter>
          <Routes />
        </MemoryRouter>
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(detailCalls).toBe(1);

      // 第一个轮询 tick 发出的详情请求比 10s 周期还慢。
      await act(async () => {
        await vi.advanceTimersByTimeAsync(10_000);
      });
      expect(detailCalls).toBe(2);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(detailCalls).toBe(2);

      await act(async () => {
        pending[0]({
          ...profileDetail("r1"),
          profile: { ...profile("r1"), name: "Slow Poll Name" },
        });
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(screen.getByText("Slow Poll Name")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
  it("keeps the clicked profile when a poll resumes before its detail arrives", async () => {
    const named = (id: string, name: string) => ({
      ...profileDetail(id),
      profile: { ...profile(id), name },
    });
    const list = [
      named("r1", "Route One").profile,
      named("r2", "Route One B").profile,
    ];
    let listCalls = 0;
    let resolvePollList: ((v: any) => void) | null = null;
    vi.mocked(api.listRouteProfiles).mockImplementation(() => {
      listCalls += 1;
      if (listCalls === 1) return Promise.resolve(list);
      return new Promise((resolve) => {
        resolvePollList = resolve;
      });
    });
    let r2Calls = 0;
    let resolveClickR2: ((d: any) => void) | null = null;
    vi.mocked(api.getRouteProfile).mockImplementation((id) => {
      if (id === "r1") return Promise.resolve(named("r1", "Route One"));
      r2Calls += 1;
      if (r2Calls > 1) return Promise.resolve(named("r2", "Route One B"));
      return new Promise((resolve) => {
        resolveClickR2 = resolve;
      });
    });

    render(
      <MemoryRouter>
        <Routes />
      </MemoryRouter>
    );
    const profileButton = async (name: string) =>
      (await screen.findByText(new RegExp(`^${name} · `))).closest("button")!;
    await waitFor(async () =>
      expect(await profileButton("Route One")).toHaveClass("border-accent/40")
    );

    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(resolvePollList).not.toBeNull());
    await act(async () => (await profileButton("Route One B")).click());
    await waitFor(() => expect(resolveClickR2).not.toBeNull());

    await act(async () => {
      resolvePollList!(list);
    });
    await act(async () => {
      resolveClickR2!(named("r2", "Route One B"));
    });
    await waitFor(async () =>
      expect(await profileButton("Route One B")).toHaveClass("border-accent/40")
    );
  });
});
