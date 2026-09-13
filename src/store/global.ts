// 全局只读资源 store——4 个 slice 共享 providers / gatewaySettings / pricing /
// routeProfiles。这些资源跨页只读，原来每个组件各自 useState + useEffect + invoke
// 拉，造成 8 个组件每次 mount 都重新请求一遍。
//
// 设计原则：
// - 每个 slice 内部维护 items / loading / error；fetch() 防重入——已经 loading
//   时直接返回当前 in-flight promise，避免 dashboard 5s 轮询 + sidebar mount 同
//   时打两次 invoke。
// - refetch() 强制忽略 loading 重新拉一次（用户手动 refresh 场景）。
// - hook 直接 selector 整个对象——组件 destructure 拿 items / loading；订阅粒度
//   足够细，单 slice 内部 setState 不会让别的 slice consumer 重渲染。
//
// 不做的事：
// - 不维护 stale time / TTL，新数据由调用方在 mutation 后显式 refetch。
// - 不替代轮询逻辑，Providers/Routes 的 usePolling 仍然存在，只是把
//   useState + useEffect 那一对替换成 store.fetch()。

import { create } from "zustand";
import * as api from "@/lib/api";
import type {
  ProviderView,
  GatewaySettings,
  ModelPricing,
  RouteProfileView,
} from "@/lib/bindings";
import type { GatewayStatus } from "@/types/gateway";

/// 一个 slice 的通用形态：list 资源放在 `items: T[]`，单值资源放在
/// `value: T | null`，其余字段一致。
type ResourceSlice<K extends string, V> = { [P in K]: V } & {
  loading: boolean;
  error: string | null;
  /// 首次拉取。已经 loading 时直接返回当前 in-flight promise，调用方拿到的
  /// 始终是同一次远端调用结果，避免 invoke 风暴。
  fetch: () => Promise<void>;
  /// 强制重新拉取（mutation 后调用）。会 reset error、设置 loading。
  refetch: () => Promise<void>;
};

type ListSlice<T> = ResourceSlice<"items", T[]>;
type ValueSlice<T> = ResourceSlice<"value", T | null>;

/// 5 个 slice 共用的 fetch / refetch 实现。
///
/// 防重入用的 in-flight promise 放在闭包里——zustand state 内放 promise 会触发
/// 不必要的订阅者重渲染。
function createResourceStore<
  K extends string,
  V,
  Extra extends object = Record<never, never>,
>(
  key: K,
  initial: V,
  fetcher: () => Promise<V>,
  extra?: (set: (patch: Partial<ResourceSlice<K, V>>) => void) => Extra
) {
  type Store = ResourceSlice<K, V> & Extra;
  let inflight: Promise<void> | null = null;

  const useStore = create<Store>()((set, get) => {
    const patch = (next: Partial<ResourceSlice<K, V>>) =>
      set(next as Partial<Store>);

    const load = (): Promise<void> => {
      const run = async () => {
        patch({ loading: true, error: null } as Partial<ResourceSlice<K, V>>);
        try {
          const data = await fetcher();
          patch({ [key]: data, loading: false } as Partial<
            ResourceSlice<K, V>
          >);
        } catch (err) {
          const message = (err as api.AppError)?.message ?? String(err);
          patch({ loading: false, error: message } as Partial<
            ResourceSlice<K, V>
          >);
        }
      };
      // 只在自己仍是当前 in-flight 时才清空：并发 refetch 时后发的请求会覆盖
      // 句柄，先完成的请求不能把它清掉，否则后续 fetch() 会绕过防重入。
      const promise: Promise<void> = run().finally(() => {
        if (inflight === promise) inflight = null;
      });
      inflight = promise;
      return promise;
    };

    return {
      [key]: initial,
      loading: false,
      error: null,
      fetch: async () => {
        if (inflight) return inflight;
        if (get().loading) return;
        return load();
      },
      refetch: async () => {
        // 先等当前 in-flight 结束再发新请求，避免与之交错写入。
        if (inflight) {
          await inflight;
        }
        return load();
      },
      ...(extra?.(patch) ?? {}),
    } as Store;
  });

  const reset = () => {
    inflight = null;
    useStore.setState({
      [key]: initial,
      loading: false,
      error: null,
    } as Partial<Store>);
  };

  return { useStore, reset };
}

// ── providers ──────────────────────────────────────────────────────

const providers = createResourceStore<"items", ProviderView[]>(
  "items",
  [],
  () => api.listProviders()
);
export const useProviders = providers.useStore;

// ── gateway settings ───────────────────────────────────────────────

const gatewaySettings = createResourceStore<"value", GatewaySettings | null>(
  "value",
  null,
  () => api.getGatewaySettings()
);
export const useGatewaySettings = gatewaySettings.useStore;

// ── pricing ────────────────────────────────────────────────────────

const pricing = createResourceStore<
  "items",
  ModelPricing[],
  {
    /// pricing 在 Settings 里需要本地 mutate（add / update / delete 即时反映），
    /// 暴露一个 setter 让组件 mutation 后无需再次 refetch。
    setItems: (items: ModelPricing[]) => void;
  }
>(
  "items",
  [],
  () => api.listModelPricing(),
  (patch) => ({
    setItems: (items) => patch({ items } as Partial<ListSlice<ModelPricing>>),
  })
);
export const usePricing = pricing.useStore;

// ── route profiles ─────────────────────────────────────────────────

const routeProfiles = createResourceStore<"items", RouteProfileView[]>(
  "items",
  [],
  () => api.listRouteProfiles()
);
export const useRouteProfiles = routeProfiles.useStore;

// ── gateway status ─────────────────────────────────────────────────
// Topbar（常驻）是唯一轮询源，Dashboard 等页面只订阅；start/stop/restart
// 返回的新状态通过 setValue 直接写入，所有订阅者即时更新。

const gatewayStatus = createResourceStore<
  "value",
  GatewayStatus | null,
  {
    /// start/stop/restart 命令的返回值直接写入，不用再发一次查询。
    setValue: (value: GatewayStatus) => void;
  }
>(
  "value",
  null,
  () => api.getGatewayStatus(),
  (patch) => ({
    setValue: (value) => patch({ value } as Partial<ValueSlice<GatewayStatus>>),
  })
);
export const useGatewayStatus = gatewayStatus.useStore;

/// 测试辅助：清空所有 store——单测之间互不污染。生产代码不应调用。
export function __resetGlobalStoresForTest() {
  for (const slice of [
    providers,
    gatewaySettings,
    pricing,
    routeProfiles,
    gatewayStatus,
  ]) {
    slice.reset();
  }
}
