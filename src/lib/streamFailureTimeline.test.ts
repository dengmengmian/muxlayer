import { describe, it, expect } from "vitest";
import {
  buildStreamFailureTimeline,
  extractLastSseEvent,
  shouldShowFailureTimeline,
} from "./streamFailureTimeline";

describe("shouldShowFailureTimeline", () => {
  it("shows for error status and message", () => {
    expect(
      shouldShowFailureTimeline({ status_code: 503, error_message: "busy" })
    ).toBe(true);
    expect(
      shouldShowFailureTimeline({ status_code: 200, error_message: null })
    ).toBe(false);
  });

  it("shows when sse has error frame", () => {
    expect(
      shouldShowFailureTimeline({
        status_code: 200,
        sse_events: 'event: error\ndata: {"error":{"message":"x"}}\n\n',
      })
    ).toBe(true);
  });
});

describe("extractLastSseEvent", () => {
  it("reads last event name and data type", () => {
    const sse = [
      "event: response.created",
      'data: {"type":"response.created"}',
      "",
      "event: response.failed",
      'data: {"type":"response.failed","error":{"message":"quota"}}',
      "",
    ].join("\n");
    const r = extractLastSseEvent(sse);
    expect(r.eventType).toBe("response.failed");
    expect(r.dataHint).toContain("quota");
  });

  it("handles chat completions delta without event: line", () => {
    const sse = [
      'data: {"choices":[{"delta":{"content":"hi"}}]}',
      "",
      "data: [DONE]",
      "",
    ].join("\n");
    const r = extractLastSseEvent(sse);
    expect(r.eventType).toBe("[DONE]");
  });
});

describe("buildStreamFailureTimeline", () => {
  it("builds start → route → upstream → failover → end for 503", () => {
    const steps = buildStreamFailureTimeline({
      timestamp: "2026-08-12T00:00:00Z",
      client: "Codex",
      provider: "DeepSeek",
      model: "deepseek-flash",
      route: "/v1/chat/completions",
      status_code: 503,
      latency_ms: 1200,
      error_message: "Service is too busy",
      trace_json: JSON.stringify({
        mode: "native_pass_through",
        target_url: "https://api.deepseek.com/v1/chat/completions",
        upstream_status: 503,
        route_decision: {
          profile_name: "Chat Completions Default",
          mode: "manual",
          selected_provider_name: "DeepSeek",
          selected_model: "deepseek-flash",
          fallback_chain: [
            {
              step: 1,
              role: "primary",
              provider_name: "DeepSeek",
              selected: true,
            },
            {
              step: 2,
              role: "fallback",
              provider_name: "OpenAI",
              selected: false,
            },
          ],
        },
      }),
    });

    expect(steps.map((s) => s.phase)).toEqual([
      "start",
      "route",
      "upstream",
      "failover",
      "end",
    ]);
    expect(steps.find((s) => s.phase === "upstream")?.status).toBe("error");
    expect(steps.find((s) => s.phase === "upstream")?.detail).toContain("503");
    expect(steps.find((s) => s.phase === "failover")?.detail).toContain(
      "DeepSeek"
    );
    expect(steps.find((s) => s.phase === "failover")?.detail).toContain(
      "OpenAI"
    );
    expect(steps.find((s) => s.phase === "end")?.status).toBe("error");
    expect(steps.find((s) => s.phase === "end")?.detail).toContain("busy");
  });

  it("includes last SSE phase when sse_events present", () => {
    const steps = buildStreamFailureTimeline({
      status_code: 500,
      error_message: "stream broken",
      sse_events: 'event: error\ndata: {"error":{"message":"mid-stream"}}\n\n',
      provider: "Kimi",
    });
    const sse = steps.find((s) => s.phase === "sse");
    expect(sse).toBeTruthy();
    expect(sse?.status).toBe("error");
    expect(sse?.detail).toMatch(/error|mid-stream/);
  });

  it("marks single-candidate failover as info not multi-hop", () => {
    const steps = buildStreamFailureTimeline({
      status_code: 400,
      error_message: "bad",
      trace_json: JSON.stringify({
        route_decision: {
          selected_provider_name: "DeepSeek",
          fallback_chain: [
            {
              step: 1,
              role: "primary",
              provider_name: "DeepSeek",
              selected: true,
            },
          ],
        },
      }),
    });
    const fo = steps.find((s) => s.phase === "failover");
    expect(fo?.titleKey).toBe("logs.timeline.failover_single");
  });
});
