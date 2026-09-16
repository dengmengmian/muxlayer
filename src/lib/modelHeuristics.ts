/// 模型挑选启发式：从上游返回的 model id 列表里挑出"最新/最主力"的 default 和 reasoning。
/// 不 hardcode 具体模型名（模型名换得太勤），全靠 pattern + 版本数字排序。
import { GENERATED_DEEPSEEK_SUPPORTED_MODELS } from "../data/generatedProviderCatalog";

const REASONING = /reasoner|reasoning|thinking|deep-?think|^o\d|-r\d|r1\b/i;
const NON_PROD = /preview|beta|alpha|draft|legacy|deprecated|experimental/i;
const SMALLER_VARIANT = /\b(mini|nano|light|tiny|small)\b/i;
const DEEPSEEK_V4_MODELS = [...GENERATED_DEEPSEEK_SUPPORTED_MODELS];

function isDeepSeekProvider(providerType: string): boolean {
  return providerType.trim().toLowerCase() === "deepseek";
}

export function normalizeModelsForProvider(
  providerType: string,
  models: string[]
): string[] {
  if (!isDeepSeekProvider(providerType)) return models;
  const available = new Set(models.map((model) => model.trim().toLowerCase()));
  return DEEPSEEK_V4_MODELS.filter((model) => available.has(model));
}

/// 把模型名里的数字段拆出来当版本号。日期形式（8 位连续数字）过滤掉，
/// 不然 `claude-haiku-4-5-20251001` 会把 20251001 当成超大版本号排到最前。
function versionTuple(name: string): number[] {
  const nums = name.match(/\d+/g) ?? [];
  const filtered = nums.filter((n) => n.length !== 8);
  return filtered.length > 0 ? filtered.map(Number) : [0];
}

/// 排名：越小越优先当 default
///   0 = 主力模型
///   1 = 小型变体（mini/nano/light）
///   2 = 预览/beta/legacy
function tierRank(name: string): number {
  if (NON_PROD.test(name)) return 2;
  if (SMALLER_VARIANT.test(name)) return 1;
  return 0;
}

/// 比较函数：returns 负数表示 a 优先级更高
function compareDesc(a: string, b: string): number {
  const ra = tierRank(a);
  const rb = tierRank(b);
  if (ra !== rb) return ra - rb;

  const va = versionTuple(a);
  const vb = versionTuple(b);
  const len = Math.max(va.length, vb.length);
  for (let i = 0; i < len; i++) {
    const av = va[i] ?? 0;
    const bv = vb[i] ?? 0;
    if (av !== bv) return bv - av;
  }

  // 同 tier 同版本：按字母序——结果稳定（不依赖 sort 实现）
  return a.localeCompare(b);
}

/// 从模型列表挑出 default + reasoning。
/// - default：排除 reasoning 系，剩下按 tierRank/version 排序，取头
/// - reasoning：匹配 REASONING 的，按同样排序取头；没有就 fallback 到 default
/// 输入空数组返回空字符串。
export function pickModels(models: string[]): {
  default: string;
  reasoning: string;
} {
  if (models.length === 0) return { default: "", reasoning: "" };

  const sorted = models.slice().sort(compareDesc);
  const reasoning = sorted.filter((m) => REASONING.test(m));
  const standard = sorted.filter((m) => !REASONING.test(m));

  const def = standard[0] ?? sorted[0];
  const rsn = reasoning[0] ?? def;
  return { default: def, reasoning: rsn };
}

export function pickModelsForProvider(
  providerType: string,
  models: string[]
): { default: string; reasoning: string } {
  if (!isDeepSeekProvider(providerType)) return pickModels(models);

  const normalized = normalizeModelsForProvider(providerType, models);
  const defaultModel = normalized.includes("deepseek-flash")
    ? "deepseek-flash"
    : (normalized[0] ?? "");
  const reasoningModel = normalized.includes("deepseek-v4-pro")
    ? "deepseek-v4-pro"
    : defaultModel;
  return { default: defaultModel, reasoning: reasoningModel };
}
