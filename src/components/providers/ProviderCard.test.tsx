import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { screen, fireEvent, waitFor, act } from "@testing-library/react";
import { ProviderCard } from "./ProviderCard";
import { renderWithProviders } from "@/components/test-utils";
import * as api from "@/lib/api";
import type { ProviderView } from "@/types/provider";
import type { ProviderHealth } from "@/types/stats";
import { __resetGlobalStoresForTest } from "@/store/global";

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...actual,
    getProviderHealth: vi.fn(),
  };
});

function makeProvider(overrides: Partial<ProviderView> = {}): ProviderView {
  return {
    id: "p1",
    name: "DeepSeek",
    provider_type: "deepseek",
    base_url: "https://api.deepseek.com",
    api_key: "sk-***",
    masked_api_key: "sk-***",
    default_model: "deepseek-v4-flash",
    reasoning_model: null,
    supported_models: null,
    model_capabilities: null,
    model_context_windows: null,
    model_mapping: null,
    extra_headers: null,
    anthropic_base_url: null,
    responses_base_url: null,
    auto_cache_control: true,
    protocol: JSON.stringify(["openai_chat_completions"]),
    timeout_seconds: 120,
    enabled: true,
    is_active: false,
    status: "not_tested",
    supports_vision: false,
    supports_cache: null,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    ...overrides,
  } as ProviderView;
}

describe("ProviderCard", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.mocked(api.getProviderHealth).mockResolvedValue(
      null as unknown as ProviderHealth
    );
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("renders provider details and active badge", async () => {
    const provider = makeProvider({ is_active: true, status: "connected" });
    renderWithProviders(
      <ProviderCard
        provider={provider}
        onEdit={() => {}}
        onDelete={() => {}}
        onSetActive={() => {}}
        onTest={() => {}}
      />
    );

    expect(screen.getByText("DeepSeek")).toBeInTheDocument();
    expect(screen.getByText("https://api.deepseek.com")).toBeInTheDocument();
    expect(screen.getByText("Active")).toBeInTheDocument();

    await waitFor(() =>
      expect(api.getProviderHealth).toHaveBeenCalledWith("DeepSeek")
    );
  });

  it("does not put engineer refiner jargon on the card face", async () => {
    const provider = makeProvider({ is_active: true });
    renderWithProviders(
      <ProviderCard
        provider={provider}
        onEdit={() => {}}
        onDelete={() => {}}
        onSetActive={() => {}}
        onTest={() => {}}
      />
    );

    // Old always-on banner wording must stay gone by default.
    expect(screen.queryByText(/精炼层建议/)).toBeNull();
    expect(screen.queryByText(/budget_tokens/)).toBeNull();
    expect(screen.queryByText(/Request field filter/i)).toBeNull();

    // Plain-language tip only after expanding details.
    fireEvent.click(screen.getByText("Configuration"));
    expect(
      await screen.findByText(/If requests return 400|如果请求报 400/)
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Settings|设置.*网关精炼层|Gateway Refiner/)
    ).toBeInTheDocument();
  });

  it("fires edit, test, set active and delete callbacks", async () => {
    const provider = makeProvider({ is_active: false });
    const onEdit = vi.fn();
    const onDelete = vi.fn();
    const onSetActive = vi.fn();
    const onTest = vi.fn();

    renderWithProviders(
      <ProviderCard
        provider={provider}
        onEdit={onEdit}
        onDelete={onDelete}
        onSetActive={onSetActive}
        onTest={onTest}
      />
    );

    await waitFor(() =>
      expect(api.getProviderHealth).toHaveBeenCalledWith("DeepSeek")
    );

    fireEvent.click(screen.getByRole("button", { name: /Edit/i }));
    expect(onEdit).toHaveBeenCalledWith(provider);

    fireEvent.click(screen.getByRole("button", { name: /Test/i }));
    expect(onTest).toHaveBeenCalledWith(provider);

    fireEvent.click(screen.getByRole("button", { name: /Set Active/i }));
    expect(onSetActive).toHaveBeenCalledWith(provider);

    expect(screen.queryByRole("button", { name: /Delete/i })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Configuration/i }));
    fireEvent.click(screen.getByRole("button", { name: /Delete/i }));
    expect(onDelete).toHaveBeenCalledWith(provider);
  });

  it("keeps analytics details distinct from inline configuration", () => {
    const provider = makeProvider();
    renderWithProviders(
      <ProviderCard
        provider={provider}
        onEdit={() => {}}
        onDelete={() => {}}
        onSetActive={() => {}}
        onTest={() => {}}
        onDetails={() => {}}
      />
    );

    expect(screen.getAllByRole("button", { name: "Details" })).toHaveLength(1);
    const configuration = screen.getByRole("button", {
      name: "Configuration",
    });
    expect(configuration).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(configuration);
    expect(configuration).toHaveAttribute("aria-expanded", "true");
  });

  it("disables test button while testing", () => {
    const provider = makeProvider();
    renderWithProviders(
      <ProviderCard
        provider={provider}
        onEdit={() => {}}
        onDelete={() => {}}
        onSetActive={() => {}}
        onTest={() => {}}
        testing
      />
    );

    const testBtn = screen.getByRole("button", { name: /Test/i });
    expect(testBtn).toBeDisabled();
  });

  it("shows health sections and humanizes recent failures", () => {
    const provider = makeProvider();
    renderWithProviders(
      <ProviderCard
        provider={provider}
        runtime={
          {
            provider_id: provider.id,
            available: true,
            consecutive_failures: 0,
            last_error: "PASS_THROUGH_STREAM_FAILED",
            last_error_code: "PASS_THROUGH_STREAM_FAILED",
            last_error_at: new Date().toISOString(),
            cooldown_until: null,
            quota_exhausted: false,
            last_probe_ok: true,
            last_probe_at: new Date().toISOString(),
            last_probe_latency_ms: 90,
            last_probe_error: null,
            updated_at: new Date().toISOString(),
          } as any
        }
        onEdit={() => {}}
        onDelete={() => {}}
        onSetActive={() => {}}
        onTest={() => {}}
      />
    );

    expect(screen.getByText("Health status")).toBeInTheDocument();
    expect(screen.queryByText("Primary actions")).not.toBeInTheDocument();
    expect(screen.getByText(/Stream forwarding failed/i)).toBeInTheDocument();
    expect(
      screen.queryByText(/PASS_THROUGH_STREAM_FAILED/)
    ).not.toBeInTheDocument();
  });

  it("toggles detail section", async () => {
    const provider = makeProvider();
    renderWithProviders(
      <ProviderCard
        provider={provider}
        onEdit={() => {}}
        onDelete={() => {}}
        onSetActive={() => {}}
        onTest={() => {}}
      />
    );

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Configuration/i }));
    });
    expect(screen.getByText(/Type/i)).toBeInTheDocument();
    expect(screen.getByText("deepseek")).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Configuration/i }));
    });
    expect(screen.queryByText(/Type/i)).not.toBeInTheDocument();
  });

  it("shows round-robin hint when masked key reports extras", () => {
    renderWithProviders(
      <ProviderCard
        provider={makeProvider({
          masked_api_key: "sk-ab****cdef (+2 more)",
        })}
        onEdit={() => {}}
        onDelete={() => {}}
        onSetActive={() => {}}
        onTest={() => {}}
      />
    );
    expect(
      screen.getByText(/3 keys · round-robin|3 把 key/)
    ).toBeInTheDocument();
  });
});
