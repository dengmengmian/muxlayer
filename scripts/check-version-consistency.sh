#!/usr/bin/env bash
# 检查所有嵌入版本号的文件是否一致。
#
# 用法:
#   scripts/check-version-consistency.sh            # 本地模式:以 package.json 为准
#   scripts/check-version-consistency.sh 2.0.5      # 发版模式:以 tag 剥掉 v 后的版本为准
#
# 发版流水线里 latest.json 的 version 直接取自 git tag(publish-updater-json.mjs),
# 只要 tag 和 tauri.conf.json 的 version 不一致,更新器就会永远提示"有新版本"。
# 所以这里是硬检查:任何一个文件不一致就 exit 1。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# 只用 grep/sed/awk,不依赖 node/jq,方便作为 release preflight 的第一步跑。
package_version() {
  sed -n -E 's/^[[:space:]]*"version":[[:space:]]*"([^"]+)".*/\1/p' package.json | head -n 1
}

if [[ $# -gt 1 ]]; then
  echo "Usage: $0 [expected-version]" >&2
  exit 2
fi

expected="${1:-}"
if [[ -z "$expected" ]]; then
  expected="$(package_version)"
  mode="local (package.json)"
else
  expected="${expected#v}"
  mode="explicit"
fi

if [[ ! "$expected" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::expected version '$expected' is not X.Y.Z" >&2
  exit 2
fi

failed=0
report() { # file actual
  if [[ "$2" == "$expected" ]]; then
    printf '  ok    %-24s %s\n' "$1" "$2"
  else
    printf '  FAIL  %-24s %s (expected %s)\n' "$1" "${2:-<missing>}" "$expected"
    failed=1
  fi
}

echo "Version consistency check — expected $expected [$mode]"

report package.json "$(package_version)"

report src-tauri/Cargo.toml \
  "$(sed -n -E 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' src-tauri/Cargo.toml | head -n 1)"

report src-tauri/tauri.conf.json \
  "$(sed -n -E 's/^[[:space:]]*"version":[[:space:]]*"([^"]+)".*/\1/p' src-tauri/tauri.conf.json | head -n 1)"

# Cargo.lock:只看 name = "agentgate" 这一个 [[package]] 块的 version。
report "src-tauri/Cargo.lock" \
  "$(awk '/^name = "agentgate"$/ {found=1; next} found && /^version = / {gsub(/"/, "", $3); print $3; exit}' src-tauri/Cargo.lock)"

# 官网:JSON-LD softwareVersion + 所有 <span data-version>vX.Y.Z</span>。
# 页面里其他形如 "legacy adoption · v2.0.0" 的文案是功能落地版本,不在检查范围。
for html in site/index.html site/zh/index.html; do
  report "$html (softwareVersion)" \
    "$(sed -n -E 's/.*"softwareVersion":[[:space:]]*"([^"]+)".*/\1/p' "$html" | head -n 1)"
  spans="$(grep -o -E 'data-version[^>]*>v[0-9]+\.[0-9]+\.[0-9]+<' "$html" | sed -E 's/.*>v([0-9.]+)</\1/' | sort -u | tr '\n' ' ' | sed 's/ $//')"
  report "$html (data-version)" "$spans"
done

if [[ "$failed" -ne 0 ]]; then
  echo "::error::version mismatch — run scripts/bump-version.sh $expected to sync every file" >&2
  exit 1
fi
echo "All version strings match $expected."
