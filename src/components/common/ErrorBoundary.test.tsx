import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, screen } from "@testing-library/react";
import { ErrorBoundary } from "./ErrorBoundary";
import { renderWithProviders } from "@/components/test-utils";

function Boom(): never {
  throw new Error("render exploded");
}

describe("ErrorBoundary", () => {
  beforeEach(() => {
    // React logs the caught error; keep test output clean.
    vi.spyOn(console, "error").mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders children when nothing throws", () => {
    renderWithProviders(
      <ErrorBoundary>
        <div data-testid="ok">fine</div>
      </ErrorBoundary>
    );
    expect(screen.getByTestId("ok")).toBeInTheDocument();
  });

  it("renders the fallback with the error message and a back link", () => {
    renderWithProviders(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>
    );
    expect(screen.getByText("render exploded")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Back to overview" })
    ).toHaveAttribute("href", "/");
  });

  it("recovers on retry without a route change", async () => {
    let shouldThrow = true;
    function Flaky() {
      if (shouldThrow) throw new Error("transient failure");
      return <div data-testid="recovered">ok</div>;
    }
    renderWithProviders(
      <ErrorBoundary>
        <Flaky />
      </ErrorBoundary>
    );
    expect(screen.getByText("transient failure")).toBeInTheDocument();

    shouldThrow = false;
    await act(async () =>
      screen.getByRole("button", { name: "Retry" }).click()
    );
    expect(screen.getByTestId("recovered")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
