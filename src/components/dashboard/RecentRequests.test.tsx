import { describe, it, expect } from "vitest";
import { screen } from "@testing-library/react";
import { RecentRequests } from "./RecentRequests";
import { renderWithProviders } from "@/components/test-utils";
import type { RequestLogListItem } from "@/types/request-log";
import type { ToolConfigView } from "@/types/tool";

function makeRequest(
  overrides: Partial<RequestLogListItem> = {}
): RequestLogListItem {
  return {
    id: "r1",
    timestamp: new Date().toISOString(),
    client: "codex",
    provider: "DeepSeek",
    model: "deepseek-flash",
    status_code: 200,
    latency_ms: 345,
    ...overrides,
  } as RequestLogListItem;
}

describe("RecentRequests", () => {
  it("returns null when there are no requests", () => {
    const { container } = renderWithProviders(
      <RecentRequests requests={[]} tools={[]} />
    );

    expect(container.firstChild).toBeNull();
  });

  it("renders recent requests and tool status", () => {
    const tools: ToolConfigView[] = [
      {
        id: "codex",
        name: "Codex",
        description: "OpenAI CLI",
        icon: "terminal",
        config_exists: true,
        config_path: "~/.codex/config.toml",
      } as ToolConfigView,
    ];
    const requests = [
      makeRequest(),
      makeRequest({ id: "r2", status_code: 500 }),
    ];

    renderWithProviders(<RecentRequests requests={requests} tools={tools} />);

    expect(screen.getByText("Request Stream")).toBeInTheDocument();
    expect(screen.getAllByText("DeepSeek")).toHaveLength(2);
    expect(screen.getAllByText("codex")).toHaveLength(2);
    expect(screen.getByText("Codex")).toBeInTheDocument();
    expect(screen.getAllByText(/200|500/)).toHaveLength(2);
  });
});
