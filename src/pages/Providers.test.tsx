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
vi.mock("@/lib/providerAutoSetup", () => ({
  fetchDetectAndPersistProviderModels: vi.fn().mockResolvedValue(undefined),
}));

import * as api from "@/lib/api";
import { Providers } from "./Providers";
import { __resetGlobalStoresForTest } from "@/store/global";

afterEach(() => cleanup());

function provider(id: string, overrides: Record<string, unknown> = {}): any {
  return {
    id,
    name: "Test Provider",
    provider_type: "openai",
    base_url: "https://api.openai.com",
    default_model: "gpt-4",
    enabled: true,
    is_active: false,
    protocol: "[]",
    ...overrides,
  };
}

describe("Providers", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.mocked(api.listProviders).mockResolvedValue([]);
    vi.mocked(api.listProviderRuntimeStatus).mockResolvedValue([] as any);
    vi.mocked(api.getProviderHealth).mockResolvedValue({
      h24_total: 0,
      h24_success: 0,
      h24_success_rate: 0,
      h24_avg_latency_ms: 0,
      recent_errors: [],
    } as any);
    vi.mocked(api.getRefinerHint).mockResolvedValue(null);
    vi.mocked(api.discoverLocalEndpoints).mockResolvedValue([]);
    vi.mocked(api.createProvider).mockResolvedValue(provider("new"));
    vi.mocked(api.updateProvider).mockResolvedValue(provider("upd"));
    vi.mocked(api.deleteProvider).mockResolvedValue(true);
    vi.mocked(api.setActiveProvider).mockResolvedValue(provider("active"));
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

  it("renders empty state and opens add dialog", async () => {
    render(
      <MemoryRouter>
        <Providers />
      </MemoryRouter>
    );

    expect(
      await screen.findByText("providers.no_providers")
    ).toBeInTheDocument();
    // Local models is a toolbar entry, not an always-on panel listing endpoints.
    expect(screen.getByText("providers.local_discover")).toBeInTheDocument();
    expect(screen.queryByText("Ollama")).toBeNull();

    const addButtons = screen.getAllByText("providers.add");
    await act(async () => addButtons[addButtons.length - 1].click());

    expect(await screen.findByText("providers.create")).toBeInTheDocument();
  });

  it("opens local models dialog and scans on demand", async () => {
    vi.mocked(api.discoverLocalEndpoints).mockResolvedValue([
      {
        name: "Ollama",
        port: 11434,
        base_url: "http://127.0.0.1:11434/v1",
        provider_type: "custom_openai_compatible",
        reachable: false,
        models: [],
      },
    ] as any);

    render(
      <MemoryRouter>
        <Providers />
      </MemoryRouter>
    );

    await screen.findByText("providers.no_providers");
    expect(api.discoverLocalEndpoints).not.toHaveBeenCalled();

    await act(async () => {
      fireEvent.click(screen.getByText("providers.local_discover"));
    });

    await waitFor(() => expect(api.discoverLocalEndpoints).toHaveBeenCalled());
    expect(await screen.findByText("Ollama")).toBeInTheDocument();
  });

  it("lists providers and fetches runtime status", async () => {
    vi.mocked(api.listProviders).mockResolvedValue([
      provider("p1", { name: "Provider One" }),
      provider("p2", { name: "Provider Two" }),
    ]);

    render(
      <MemoryRouter>
        <Providers />
      </MemoryRouter>
    );

    await waitFor(() => {
      expect(api.listProviders).toHaveBeenCalled();
      expect(api.listProviderRuntimeStatus).toHaveBeenCalled();
    });

    expect(screen.getByText("Provider One")).toBeInTheDocument();
    expect(screen.getByText("Provider Two")).toBeInTheDocument();
    expect(screen.getByTestId("provider-grid")).toHaveClass("xl:grid-cols-2");
  });

  it("creates a provider from the add dialog", async () => {
    vi.mocked(api.createProvider).mockResolvedValue(
      provider("new", { name: "Quick DeepSeek", provider_type: "deepseek" })
    );

    render(
      <MemoryRouter>
        <Providers />
      </MemoryRouter>
    );

    const addButtons = await screen.findAllByText("providers.add");
    await act(async () => addButtons[addButtons.length - 1].click());

    const keyInput = await screen.findByPlaceholderText(/sk-xxx/);
    await act(async () => {
      fireEvent.change(keyInput, { target: { value: "deepseek-testkey" } });
    });

    const createButton = screen
      .getAllByRole("button", { name: "providers.create" })
      .find((button) => button.getAttribute("type") === "button")!;
    await waitFor(() => expect(createButton).toBeEnabled());

    await act(async () => createButton.click());

    await waitFor(() =>
      expect(api.createProvider).toHaveBeenCalledWith(
        expect.objectContaining({
          provider_type: "deepseek",
          api_key: "deepseek-testkey",
        })
      )
    );
  });

  it("sets a listed provider active", async () => {
    vi.mocked(api.listProviders).mockResolvedValue([
      provider("p1", { name: "Inactive Provider" }),
    ]);

    render(
      <MemoryRouter>
        <Providers />
      </MemoryRouter>
    );

    await screen.findByText("Inactive Provider");
    const setActive = screen.getByRole("button", {
      name: /providers\.set_active|Set Active/i,
    });
    await act(async () => setActive.click());

    await waitFor(() =>
      expect(api.setActiveProvider).toHaveBeenCalledWith("p1")
    );
  });

  it("confirms provider deletion before deleting", async () => {
    vi.mocked(api.listProviders).mockResolvedValue([
      provider("p1", { name: "Delete Me" }),
    ]);

    render(
      <MemoryRouter>
        <Providers />
      </MemoryRouter>
    );

    await screen.findByText("Delete Me");
    await act(async () => {
      screen
        .getByRole("button", {
          name: /providers\.configuration_details|Configuration/i,
        })
        .click();
    });
    await act(async () => {
      screen.getByRole("button", { name: /providers\.delete|Delete/i }).click();
    });

    const confirmButtons = await screen.findAllByRole("button", {
      name: /common\.delete|Delete/i,
    });
    await act(async () => confirmButtons[confirmButtons.length - 1].click());

    await waitFor(() => expect(api.deleteProvider).toHaveBeenCalledWith("p1"));
  });
});
