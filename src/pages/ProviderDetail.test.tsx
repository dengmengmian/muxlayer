import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor, screen, cleanup } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";

vi.mock("@/lib/api");

import * as api from "@/lib/api";
import { ProviderDetail } from "./ProviderDetail";

afterEach(() => cleanup());

describe("ProviderDetail", () => {
  beforeEach(() => {
    vi.mocked(api.getProvider).mockResolvedValue({
      id: "p1",
      name: "OpenAI",
      provider_type: "openai",
      base_url: "https://api.openai.com",
      default_model: "gpt-4",
      enabled: true,
      is_active: true,
    } as any);
    vi.mocked(api.getProviderHealth).mockResolvedValue({
      h24_total: 10,
      h24_success: 9,
      h24_success_rate: 90,
      h24_avg_latency_ms: 120,
      recent_errors: [],
    } as any);
    vi.mocked(api.aggregateProviderDetailStats).mockResolvedValue({
      latency_points: [],
      model_stats: [],
    } as any);
    vi.mocked(api.listProviderRuntimeStatus).mockResolvedValue([]);
  });

  it("renders provider details", async () => {
    render(
      <MemoryRouter initialEntries={["/providers/p1"]}>
        <Routes>
          <Route path="/providers/:id" element={<ProviderDetail />} />
        </Routes>
      </MemoryRouter>
    );

    await waitFor(() => expect(api.getProvider).toHaveBeenCalledWith("p1"));
    expect(
      screen.queryByText("providers.detail.console")
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("providers.detail.health_strip")
    ).toBeInTheDocument();
    expect(
      screen.getByText("providers.detail.latency_monitor")
    ).toBeInTheDocument();
    expect(screen.getByText("OpenAI")).toBeInTheDocument();
    expect(screen.getByText("providers.active")).toBeInTheDocument();
  });

  it("displays error state when provider load fails", async () => {
    vi.mocked(api.getProvider).mockRejectedValue(new Error("not found"));

    render(
      <MemoryRouter initialEntries={["/providers/missing"]}>
        <Routes>
          <Route path="/providers/:id" element={<ProviderDetail />} />
        </Routes>
      </MemoryRouter>
    );

    await waitFor(() =>
      expect(api.getProvider).toHaveBeenCalledWith("missing")
    );
    expect(screen.getByText("not found")).toBeInTheDocument();
  });
});
