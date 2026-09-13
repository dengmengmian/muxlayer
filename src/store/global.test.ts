import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("@/lib/api", () => ({
  listProviders: vi.fn(),
  getGatewaySettings: vi.fn(),
  listModelPricing: vi.fn(),
  listRouteProfiles: vi.fn(),
  getGatewayStatus: vi.fn(),
}));

import * as api from "@/lib/api";
import {
  useProviders,
  useGatewaySettings,
  usePricing,
  useRouteProfiles,
  useGatewayStatus,
  __resetGlobalStoresForTest,
} from "./global";

describe("global store", () => {
  beforeEach(() => {
    __resetGlobalStoresForTest();
    vi.mocked(api.listProviders).mockReset();
    vi.mocked(api.getGatewaySettings).mockReset();
    vi.mocked(api.listModelPricing).mockReset();
    vi.mocked(api.listRouteProfiles).mockReset();
    vi.mocked(api.getGatewayStatus).mockReset();
  });

  afterEach(() => {
    __resetGlobalStoresForTest();
  });

  describe("useProviders", () => {
    it("fetch loads items and clears loading", async () => {
      const fake = [{ id: "p1", name: "OpenAI" }] as any;
      vi.mocked(api.listProviders).mockResolvedValue(fake);
      await useProviders.getState().fetch();
      const s = useProviders.getState();
      expect(s.items).toEqual(fake);
      expect(s.loading).toBe(false);
      expect(s.error).toBeNull();
      expect(api.listProviders).toHaveBeenCalledTimes(1);
    });

    it("concurrent fetch coalesces into single invoke", async () => {
      let resolve!: (v: any) => void;
      vi.mocked(api.listProviders).mockReturnValue(
        new Promise((r) => {
          resolve = r;
        })
      );
      const p1 = useProviders.getState().fetch();
      const p2 = useProviders.getState().fetch();
      resolve([{ id: "p1" }]);
      await Promise.all([p1, p2]);
      expect(api.listProviders).toHaveBeenCalledTimes(1);
      expect(useProviders.getState().items).toEqual([{ id: "p1" }]);
    });

    it("fetch records error message on failure", async () => {
      vi.mocked(api.listProviders).mockRejectedValue({ message: "boom" });
      await useProviders.getState().fetch();
      const s = useProviders.getState();
      expect(s.error).toBe("boom");
      expect(s.loading).toBe(false);
      expect(s.items).toEqual([]);
    });

    it("fetch joins the latest in-flight request after overlapping refetches", async () => {
      const resolvers: Array<(v: any) => void> = [];
      vi.mocked(api.listProviders).mockImplementation(
        () =>
          new Promise((r) => {
            resolvers.push(r);
          })
      );
      const initial = useProviders.getState().fetch();
      const refetchA = useProviders.getState().refetch();
      const refetchB = useProviders.getState().refetch();
      resolvers[0]([]);
      await initial;
      await vi.waitFor(() => expect(resolvers).toHaveLength(3));

      // A 先完成、B 仍在飞：A 的收尾不能把 B 的 in-flight 句柄清掉。
      resolvers[1]([{ id: "a" }]);
      await refetchA;
      const joined = useProviders.getState().fetch();
      expect(api.listProviders).toHaveBeenCalledTimes(3);

      resolvers[2]([{ id: "b" }]);
      await Promise.all([refetchB, joined]);
      expect(useProviders.getState().items).toEqual([{ id: "b" }]);
    });

    it("refetch re-invokes api even after success", async () => {
      vi.mocked(api.listProviders)
        .mockResolvedValueOnce([{ id: "a" }] as any)
        .mockResolvedValueOnce([{ id: "a" }, { id: "b" }] as any);
      await useProviders.getState().fetch();
      expect(useProviders.getState().items).toHaveLength(1);
      await useProviders.getState().refetch();
      expect(useProviders.getState().items).toHaveLength(2);
      expect(api.listProviders).toHaveBeenCalledTimes(2);
    });
  });

  describe("useGatewaySettings", () => {
    it("fetch stores value as single object", async () => {
      const settings = { id: 1, host: "127.0.0.1", port: 11451 } as any;
      vi.mocked(api.getGatewaySettings).mockResolvedValue(settings);
      await useGatewaySettings.getState().fetch();
      expect(useGatewaySettings.getState().value).toEqual(settings);
    });

    it("concurrent fetch coalesces", async () => {
      let resolve!: (v: any) => void;
      vi.mocked(api.getGatewaySettings).mockReturnValue(
        new Promise((r) => {
          resolve = r;
        })
      );
      const p1 = useGatewaySettings.getState().fetch();
      const p2 = useGatewaySettings.getState().fetch();
      resolve({ id: 1 });
      await Promise.all([p1, p2]);
      expect(api.getGatewaySettings).toHaveBeenCalledTimes(1);
    });
  });

  describe("usePricing", () => {
    it("fetch + setItems both update items", async () => {
      vi.mocked(api.listModelPricing).mockResolvedValue([
        { id: "a", provider: "openai", model_pattern: "gpt-4" },
      ] as any);
      await usePricing.getState().fetch();
      expect(usePricing.getState().items).toHaveLength(1);
      usePricing
        .getState()
        .setItems([
          { id: "a", provider: "openai", model_pattern: "gpt-4" } as any,
          { id: "b", provider: "anthropic", model_pattern: "claude" } as any,
        ]);
      expect(usePricing.getState().items).toHaveLength(2);
    });
  });

  describe("useRouteProfiles", () => {
    it("fetch loads profile list", async () => {
      const profiles = [
        { id: "default", name: "Default" },
        { id: "p2", name: "Secondary" },
      ] as any;
      vi.mocked(api.listRouteProfiles).mockResolvedValue(profiles);
      await useRouteProfiles.getState().fetch();
      expect(useRouteProfiles.getState().items).toEqual(profiles);
    });

    it("error message captured", async () => {
      vi.mocked(api.listRouteProfiles).mockRejectedValue({
        message: "rpc err",
      });
      await useRouteProfiles.getState().fetch();
      const s = useRouteProfiles.getState();
      expect(s.error).toBe("rpc err");
      expect(s.items).toEqual([]);
    });
  });

  describe("useGatewayStatus", () => {
    const running = { running: true, host: "127.0.0.1", port: 9090 } as any;

    it("fetch loads status value", async () => {
      vi.mocked(api.getGatewayStatus).mockResolvedValue(running);
      await useGatewayStatus.getState().fetch();
      const s = useGatewayStatus.getState();
      expect(s.value).toEqual(running);
      expect(s.loading).toBe(false);
      expect(s.error).toBeNull();
    });

    it("concurrent fetch coalesces into single invoke", async () => {
      let resolve!: (v: any) => void;
      vi.mocked(api.getGatewayStatus).mockReturnValue(
        new Promise((r) => {
          resolve = r;
        })
      );
      const p1 = useGatewayStatus.getState().fetch();
      const p2 = useGatewayStatus.getState().fetch();
      resolve(running);
      await Promise.all([p1, p2]);
      expect(api.getGatewayStatus).toHaveBeenCalledTimes(1);
      expect(useGatewayStatus.getState().value).toEqual(running);
    });

    it("setValue 直接写入，不发请求", () => {
      const stopped = { running: false } as any;
      useGatewayStatus.getState().setValue(stopped);
      expect(useGatewayStatus.getState().value).toEqual(stopped);
      expect(api.getGatewayStatus).not.toHaveBeenCalled();
    });

    it("fetch records error message on failure", async () => {
      vi.mocked(api.getGatewayStatus).mockRejectedValue({ message: "down" });
      await useGatewayStatus.getState().fetch();
      const s = useGatewayStatus.getState();
      expect(s.error).toBe("down");
      expect(s.value).toBeNull();
    });
  });

  describe("useGatewaySettings refetch", () => {
    it("refetch re-invokes api even after success", async () => {
      vi.mocked(api.getGatewaySettings)
        .mockResolvedValueOnce({ id: 1, port: 9090 } as any)
        .mockResolvedValueOnce({ id: 1, port: 8080 } as any);
      await useGatewaySettings.getState().fetch();
      expect(useGatewaySettings.getState().value).toEqual({
        id: 1,
        port: 9090,
      });
      await useGatewaySettings.getState().refetch();
      expect(useGatewaySettings.getState().value).toEqual({
        id: 1,
        port: 8080,
      });
      expect(api.getGatewaySettings).toHaveBeenCalledTimes(2);
    });
  });

  describe("usePricing error path", () => {
    it("fetch records error message on failure", async () => {
      vi.mocked(api.listModelPricing).mockRejectedValue({ message: "db err" });
      await usePricing.getState().fetch();
      const s = usePricing.getState();
      expect(s.error).toBe("db err");
      expect(s.loading).toBe(false);
      expect(s.items).toEqual([]);
    });
  });

  describe("useRouteProfiles refetch", () => {
    it("refetch re-invokes api even after success", async () => {
      vi.mocked(api.listRouteProfiles)
        .mockResolvedValueOnce([{ id: "a" }] as any)
        .mockResolvedValueOnce([{ id: "a" }, { id: "b" }] as any);
      await useRouteProfiles.getState().fetch();
      expect(useRouteProfiles.getState().items).toHaveLength(1);
      await useRouteProfiles.getState().refetch();
      expect(useRouteProfiles.getState().items).toHaveLength(2);
      expect(api.listRouteProfiles).toHaveBeenCalledTimes(2);
    });
  });

  describe("useGatewayStatus refetch", () => {
    it("refetch re-invokes api even after success", async () => {
      const running = { running: true, host: "127.0.0.1", port: 9090 } as any;
      const stopped = { running: false, host: "127.0.0.1", port: 9090 } as any;
      vi.mocked(api.getGatewayStatus)
        .mockResolvedValueOnce(running)
        .mockResolvedValueOnce(stopped);
      await useGatewayStatus.getState().fetch();
      expect(useGatewayStatus.getState().value).toEqual(running);
      await useGatewayStatus.getState().refetch();
      expect(useGatewayStatus.getState().value).toEqual(stopped);
      expect(api.getGatewayStatus).toHaveBeenCalledTimes(2);
    });
  });
});
