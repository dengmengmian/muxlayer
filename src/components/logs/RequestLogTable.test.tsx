import { describe, it, expect, vi } from "vitest";
import { screen, fireEvent, act } from "@testing-library/react";
import { RequestLogTable } from "./RequestLogTable";
import { renderWithProviders } from "@/components/test-utils";
import type { RequestLogListItem } from "@/types/request-log";

function makeRequest(
  overrides: Partial<RequestLogListItem> = {}
): RequestLogListItem {
  return {
    id: "r1",
    timestamp: new Date().toISOString(),
    route: "/v1/chat/completions",
    client: "codex",
    source: "gateway",
    provider: "DeepSeek",
    model: "deepseek-flash",
    status_code: 200,
    latency_ms: 234,
    ...overrides,
  } as RequestLogListItem;
}

describe("RequestLogTable", () => {
  it("renders requests and calls onSelect when a row is clicked", () => {
    const requests = [makeRequest()];
    const onSelect = vi.fn();

    renderWithProviders(
      <RequestLogTable requests={requests} onSelect={onSelect} />
    );

    expect(screen.getByText("DeepSeek")).toBeInTheDocument();
    expect(screen.getByText("codex")).toBeInTheDocument();
    expect(screen.getByText("deepseek-flash")).toBeInTheDocument();

    act(() => {
      fireEvent.click(screen.getByText("/v1/chat/completions"));
    });

    expect(onSelect).toHaveBeenCalledWith(requests[0]);
  });

  it("renders error status badge for failed requests", () => {
    const requests = [
      makeRequest({ id: "r2", status_code: 500 }),
      makeRequest({ id: "r3", status_code: 200 }),
    ];

    renderWithProviders(
      <RequestLogTable requests={requests} onSelect={() => {}} />
    );

    const badges = screen.getAllByText(/200|500/);
    expect(badges).toHaveLength(2);
  });

  it("renders em dash for missing fields", () => {
    const requests = [
      makeRequest({
        client: null,
        provider: null,
        model: null,
        status_code: null,
        latency_ms: null,
      }),
    ];

    renderWithProviders(
      <RequestLogTable requests={requests} onSelect={() => {}} />
    );

    expect(screen.getAllByText("—").length).toBeGreaterThanOrEqual(3);
  });
});
