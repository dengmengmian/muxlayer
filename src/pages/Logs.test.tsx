import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, act, waitFor, screen } from "@testing-library/react";
import { fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

vi.mock("@/lib/api");

import * as api from "@/lib/api";
import { Logs } from "./Logs";
import { __resetGlobalStoresForTest } from "@/store/global";

function logItem(id: string, model: string) {
  return {
    id,
    request_id: id,
    timestamp: "2026-06-13T00:00:00Z",
    client: null,
    provider: null,
    model,
    route: null,
    status_code: 200,
    latency_ms: 1,
    error_message: null,
    source: "gateway",
    session_id: null,
  } as any;
}

function logDetail(id: string): any {
  return {
    ...logItem(id, "gpt-4"),
    provider: "OpenAI",
    client: "Codex",
    route: "/v1/chat/completions",
    input_tokens: 3,
    output_tokens: 4,
    cost: 0.001,
    cache_write_tokens: null,
    cache_read_tokens: null,
    raw_request: '{"messages":[{"role":"user","content":"hi"}]}',
    converted_request: null,
    raw_response: '{"choices":[{"message":{"content":"hello"}}]}',
    converted_response: null,
    sse_events: null,
    tool_calls: null,
    trace_json: null,
    external_id: null,
  };
}

describe("Logs", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.mocked(api.listRequestLogs).mockResolvedValue([] as any);
    vi.mocked(api.countRequestLogs).mockResolvedValue(0);
    vi.mocked(api.listLogModels).mockResolvedValue([]);
    vi.mocked(api.listProviders).mockResolvedValue([] as any);
    vi.mocked(api.listRouteProfiles).mockResolvedValue([] as any);
    vi.mocked(api.getRequestLogDetail).mockResolvedValue(logDetail("a"));
    vi.mocked(api.clearRequestLogs).mockResolvedValue(true);
    vi.mocked(api.syncClaudeSessions).mockResolvedValue({
      files_scanned: 0,
      imported: 0,
      skipped: 0,
      errors: [],
    } as any);
    vi.mocked(api.syncCodexSessions).mockResolvedValue({
      files_scanned: 0,
      imported: 0,
      skipped: 0,
      errors: [],
    } as any);
    vi.mocked(api.syncGeminiSessions).mockResolvedValue({
      files_scanned: 0,
      imported: 0,
      skipped: 0,
      errors: [],
    } as any);
  });

  it("慢返回的旧请求结果不覆盖新请求", async () => {
    // listRequestLogs 每次调用返回受控 promise，按序号入队，
    // 由测试决定 resolve 顺序——模拟旧请求比新请求晚返回。
    const resolvers: Array<(v: any) => void> = [];
    vi.mocked(api.listRequestLogs).mockImplementation(
      () => new Promise<any>((resolve) => resolvers.push(resolve))
    );

    const { container } = render(
      <MemoryRouter>
        <Logs />
      </MemoryRouter>
    );

    expect(screen.getByText("nav.logs")).toBeInTheDocument();
    expect(screen.queryByText("logs.traffic_snapshot")).toBeNull();
    expect(screen.getByText("logs.view_list")).toBeInTheDocument();
    expect(screen.getByText("logs.view_session")).toBeInTheDocument();

    await waitFor(() => expect(resolvers.length).toBe(1));

    const statusSelect = container.querySelector("select")!;
    await act(async () => {
      statusSelect.value = "error";
      statusSelect.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await waitFor(() => expect(resolvers.length).toBe(2));

    // 新请求先返回
    await act(async () => {
      resolvers[1]([logItem("new", "NEW-MODEL")]);
    });
    expect(await screen.findByText("NEW-MODEL")).toBeInTheDocument();

    // 旧请求后返回——绝不能覆盖新结果
    await act(async () => {
      resolvers[0]([logItem("old", "OLD-MODEL")]);
    });
    expect(screen.queryByText("OLD-MODEL")).not.toBeInTheDocument();
    expect(screen.getByText("NEW-MODEL")).toBeInTheDocument();
  });

  it("翻页后滚动回顶部", async () => {
    vi.mocked(api.listRequestLogs).mockResolvedValue([
      logItem("a", "M"),
    ] as any);
    vi.mocked(api.countRequestLogs).mockResolvedValue(250); // 250/100 → 3 页，出现翻页

    const scrollSpy = vi.fn();
    (Element.prototype as any).scrollIntoView = scrollSpy;

    render(
      <MemoryRouter>
        <Logs />
      </MemoryRouter>
    );

    const next = await screen.findByText("logs.page_next");
    scrollSpy.mockClear();
    await act(async () => {
      next.click();
    });
    expect(scrollSpy).toHaveBeenCalled();
  });

  it("opens the detail drawer and copies raw payload content", async () => {
    const clipboard = { writeText: vi.fn().mockResolvedValue(undefined) };
    Object.assign(navigator, { clipboard });
    vi.mocked(api.listRequestLogs).mockResolvedValue([
      {
        ...logItem("a", "gpt-4"),
        provider: "OpenAI",
        route: "/v1/chat/completions",
      },
    ] as any);
    vi.mocked(api.countRequestLogs).mockResolvedValue(1);

    render(
      <MemoryRouter>
        <Logs />
      </MemoryRouter>
    );

    const modelCell = await screen.findByText("gpt-4");
    await act(async () => fireEvent.click(modelCell.closest("tr")!));

    await waitFor(() =>
      expect(api.getRequestLogDetail).toHaveBeenCalledWith("a")
    );
    // Drawer now shows body Diff (raw vs converted) instead of a single raw_request block.
    expect(await screen.findByText("logs.body_diff")).toBeInTheDocument();
    expect(screen.getAllByText("logs.raw").length).toBeGreaterThan(0);

    await act(async () => {
      screen.getAllByTitle("common.copy")[0].click();
    });
    expect(clipboard.writeText).toHaveBeenCalledWith(
      '{"messages":[{"role":"user","content":"hi"}]}'
    );
  });

  it("syncs imported logs and confirms clearing logs", async () => {
    render(
      <MemoryRouter>
        <Logs />
      </MemoryRouter>
    );

    await screen.findByText("logs.no_logs");
    await act(async () => screen.getByText("logs.sync_logs").click());
    await act(async () => screen.getByText("logs.sync_codex").click());
    await waitFor(() => expect(api.syncCodexSessions).toHaveBeenCalled());

    await act(async () => screen.getByText("logs.clear").click());
    expect(await screen.findByText("logs.clear_title")).toBeInTheDocument();
    await act(async () => screen.getByText("logs.clear_confirm").click());

    await waitFor(() => expect(api.clearRequestLogs).toHaveBeenCalled());
  });
  it("changing a filter on page 3 issues exactly one list query for page 1", async () => {
    vi.mocked(api.listRequestLogs).mockResolvedValue([
      logItem("a", "M"),
    ] as any);
    vi.mocked(api.countRequestLogs).mockResolvedValue(250);
    (Element.prototype as any).scrollIntoView = vi.fn();

    const { container } = render(
      <MemoryRouter>
        <Logs />
      </MemoryRouter>
    );

    fireEvent.click(await screen.findByText("logs.page_next"));
    await waitFor(() =>
      expect(api.listRequestLogs).toHaveBeenLastCalledWith(
        expect.objectContaining({ offset: 100 })
      )
    );
    fireEvent.click(await screen.findByText("logs.page_next"));
    await waitFor(() =>
      expect(api.listRequestLogs).toHaveBeenLastCalledWith(
        expect.objectContaining({ offset: 200 })
      )
    );
    vi.mocked(api.listRequestLogs).mockClear();

    const statusSelect = container.querySelector("select")!;
    await act(async () => {
      fireEvent.change(statusSelect, { target: { value: "error" } });
    });

    await waitFor(() => expect(api.listRequestLogs).toHaveBeenCalled());
    expect(api.listRequestLogs).toHaveBeenCalledTimes(1);
    expect(api.listRequestLogs).toHaveBeenCalledWith(
      expect.objectContaining({ status: "error", offset: 0 })
    );
  });
});
