#!/usr/bin/env bash
# Usage: ./scripts/bump-version.sh 0.2.0
#
# 同步所有嵌入版本号的文件,最后跑 check-version-consistency.sh 自检。
# 原地替换统一走「先写临时文件,成功后再回写原文件」,不用 sed -i,macOS(BSD sed)和
# Linux(GNU sed)都能跑;回写用 cat > 而不是 mv,保留原文件权限(mktemp 建的是 0600)。
set -euo pipefail

NEW_VERSION="${1:-}"

if [ -z "$NEW_VERSION" ]; then
  echo "Usage: $0 <version>"
  echo "Example: $0 0.2.0"
  exit 1
fi

# Validate semver format
if ! echo "$NEW_VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "Error: version must be semver format (e.g. 0.2.0)"
  exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# sed -E 替换整个文件:rewrite_sed <file> <sed 表达式>
rewrite_sed() {
  local file="$1" expr="$2" tmp
  tmp="$(mktemp "${file}.XXXXXX")"
  sed -E "$expr" "$file" > "$tmp"
  cat "$tmp" > "$file"
  rm -f "$tmp"
}

# awk 改写:rewrite_awk <file> <awk 程序>,awk 里可用变量 v(新版本号)
rewrite_awk() {
  local file="$1" prog="$2" tmp
  tmp="$(mktemp "${file}.XXXXXX")"
  awk -v v="$NEW_VERSION" "$prog" "$file" > "$tmp"
  cat "$tmp" > "$file"
  rm -f "$tmp"
}

# 1. package.json / tauri.conf.json:只改第一个 "version"(顶层字段)
for file in package.json src-tauri/tauri.conf.json; do
  rewrite_awk "$file" '!done && /"version":[[:space:]]*"[^"]*"/ { sub(/"version":[[:space:]]*"[^"]*"/, "\"version\": \"" v "\""); done = 1 } { print }'
done

# 2. src-tauri/Cargo.toml:只改 [package] 的第一行 version
rewrite_awk src-tauri/Cargo.toml '!done && /^version[[:space:]]*=[[:space:]]*"/ { $0 = "version = \"" v "\""; done = 1 } { print }'

# 3. src-tauri/Cargo.lock:只改 name = "agentgate" 这个 [[package]] 块的 version
rewrite_awk src-tauri/Cargo.lock '/^name = "agentgate"$/ { found = 1; print; next } found && /^version = / { $0 = "version = \"" v "\""; found = 0 } { print }'

# 4. 官网:JSON-LD softwareVersion + <span data-version>vX.Y.Z</span>(与 check-version-consistency.sh 检查的模式一致)
for file in site/index.html site/zh/index.html; do
  rewrite_sed "$file" "s/(\"softwareVersion\":[[:space:]]*\")[^\"]*\"/\1${NEW_VERSION}\"/g; s/(data-version[^>]*>)v[0-9]+\.[0-9]+\.[0-9]+</\1v${NEW_VERSION}</g"
done

# 5. README / full-reference 的下载直链(docs:download:check 要求与 package.json 一致)
for file in README.md README_ZH.md docs/full-reference.md docs/full-reference-zh.md; do
  rewrite_sed "$file" "s#(github\.com/dengmengmian/muxlayer/releases/download/v)[0-9]+\.[0-9]+\.[0-9]+/#\1${NEW_VERSION}/#g; s/MuxLayer_[0-9]+\.[0-9]+\.[0-9]+_/MuxLayer_${NEW_VERSION}_/g; s/\[MuxLayer [0-9]+\.[0-9]+\.[0-9]+\]/[MuxLayer ${NEW_VERSION}]/g"
done

# 6. Homebrew cask 参考副本的 version(sha256 由 release.yml 的 homebrew-cask job 写进 tap 仓库)
for file in packaging/homebrew/muxlayer.rb packaging/homebrew/agentgate.rb; do
  rewrite_sed "$file" "s/^(  version \")[0-9.]+\"/\1${NEW_VERSION}\"/"
done

echo "Version updated to $NEW_VERSION in:"
echo "  - package.json"
echo "  - src-tauri/Cargo.toml, src-tauri/Cargo.lock, src-tauri/tauri.conf.json"
echo "  - site/index.html, site/zh/index.html"
echo "  - README.md, README_ZH.md, docs/full-reference.md, docs/full-reference-zh.md (download links)"
echo "  - packaging/homebrew/muxlayer.rb, packaging/homebrew/agentgate.rb (version only; sha256 not changed)"
echo "  (Settings & Sidebar read version from Tauri API at runtime)"
echo ""

bash scripts/check-version-consistency.sh "$NEW_VERSION"

echo ""
echo "Next steps:"
echo "  1. Update CHANGELOG.md (and docs/release-notes/$NEW_VERSION.md for important releases)"
echo "  2. git add -A && git commit -m \"release: v$NEW_VERSION\""
echo "  3. git tag v$NEW_VERSION"
echo "  4. git push origin main --tags"
echo "  5. pnpm tauri build"
