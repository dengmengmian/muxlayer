#!/usr/bin/env node

import fs from "node:fs";

const repo = process.env.GITHUB_REPOSITORY;
const token = process.env.GITHUB_TOKEN;
const tag = process.env.GITHUB_REF_NAME;
// release.yml 的 create-release job 传入草稿 release 的 id;不传时按 tag 在列表里找。
const releaseId = process.env.RELEASE_ID;

if (!repo || !token || !tag) {
  throw new Error("GITHUB_REPOSITORY, GITHUB_TOKEN and GITHUB_REF_NAME are required");
}

const version = tag.replace(/^v/, "");

// 纵深防御:latest.json 的 version 取自 tag,若与 app 自身版本(tauri.conf.json)不一致,
// 已安装的客户端会永远认为有新版本。preflight 已查过一次,这里发布前再硬校验。
const appVersion = JSON.parse(fs.readFileSync("src-tauri/tauri.conf.json", "utf8")).version;
if (appVersion !== version) {
  throw new Error(`Tag ${tag} (version ${version}) does not match src-tauri/tauri.conf.json version ${appVersion}`);
}

const apiBase = `${process.env.GITHUB_API_URL || "https://api.github.com"}/repos/${repo}`;
const headers = {
  "Accept": "application/vnd.github+json",
  "Authorization": `Bearer ${token}`,
  "X-GitHub-Api-Version": "2022-11-28",
};

async function gh(path, options = {}) {
  const res = await fetch(`${apiBase}${path}`, { ...options, headers: { ...headers, ...(options.headers || {}) } });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`${options.method || "GET"} ${path} failed: ${res.status} ${body}`);
  }
  return res;
}

async function getJson(path) {
  const res = await gh(path);
  return res.json();
}

function assetUrl(name) {
  return `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(name).replaceAll("%2F", "/")}`;
}

async function signatureFor(asset, assets) {
  const sig = assets.find((candidate) => candidate.name === `${asset.name}.sig`);
  if (!sig) throw new Error(`Missing signature asset for ${asset.name}`);
  // 草稿 release 的 browser_download_url 匿名访问是 404,走带鉴权的 asset API 下载。
  const res = await gh(`/releases/assets/${sig.id}`, { headers: { Accept: "application/octet-stream" } });
  return (await res.text()).trim();
}

function findAsset(assets, patterns) {
  return assets.find((asset) => patterns.every((pattern) => pattern.test(asset.name)));
}

async function platformEntry(assets, asset) {
  if (!asset) return null;
  return {
    signature: await signatureFor(asset, assets),
    url: assetUrl(asset.name),
  };
}

function releaseNotes(version) {
  const changelog = fs.readFileSync("CHANGELOG.md", "utf8");
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((line) => line.startsWith(`## [${version}]`));
  if (start < 0) return `Release ${tag}`;
  const end = lines.findIndex((line, index) => index > start && line.startsWith("## ["));
  return lines.slice(start + 1, end < 0 ? undefined : end).join("\n").trim() || `Release ${tag}`;
}

// 草稿 release 不会出现在 /releases/tags/{tag},只能按 id 取,或翻列表按 tag_name 匹配。
async function findRelease() {
  if (releaseId) {
    const release = await getJson(`/releases/${encodeURIComponent(releaseId)}`);
    if (release.tag_name !== tag) {
      throw new Error(`Release ${releaseId} has tag ${release.tag_name}, expected ${tag}`);
    }
    return release;
  }
  const matches = [];
  for (let page = 1; ; page += 1) {
    const releases = await getJson(`/releases?per_page=100&page=${page}`);
    matches.push(...releases.filter((candidate) => candidate.tag_name === tag));
    if (releases.length < 100) break;
  }
  if (matches.length !== 1) {
    throw new Error(`Expected exactly one release for ${tag}, found ${matches.length}`);
  }
  return matches[0];
}

const release = await findRelease();
const assets = release.assets;

const macArchives = assets.filter((asset) => /\.app\.tar\.gz$/.test(asset.name));
const macArm = macArchives.find((asset) => /(aarch64|arm64)/i.test(asset.name)) || (macArchives.length === 1 ? macArchives[0] : null);
const macX64 = macArchives.find((asset) => /(x64|x86_64)/i.test(asset.name)) || (macArchives.length === 1 ? macArchives[0] : null);
const linuxAppImage = findAsset(assets, [/\.AppImage$/]);
const linuxDeb = findAsset(assets, [/\.deb$/]);
const windowsMsi = findAsset(assets, [/\.msi$/]);
const windowsNsis = findAsset(assets, [/(setup|installer).*\.exe$/i]);
const windowsDefault = windowsNsis || windowsMsi;

const platforms = {
  "darwin-aarch64": await platformEntry(assets, macArm),
  "darwin-x86_64": await platformEntry(assets, macX64),
  "linux-x86_64": await platformEntry(assets, linuxAppImage),
  "linux-x86_64-appimage": await platformEntry(assets, linuxAppImage),
  "linux-x86_64-deb": await platformEntry(assets, linuxDeb),
  "windows-x86_64": await platformEntry(assets, windowsDefault),
  "windows-x86_64-nsis": await platformEntry(assets, windowsNsis),
};

if (windowsMsi) {
  platforms["windows-x86_64-msi"] = await platformEntry(assets, windowsMsi);
}

for (const [key, value] of Object.entries(platforms)) {
  if (!value) throw new Error(`Missing updater artifact for ${key}`);
}

const latest = {
  version,
  notes: releaseNotes(version),
  pub_date: release.published_at || new Date().toISOString(),
  platforms,
};

// 替换顺序:先以临时名上传新文件,再删旧 latest.json,最后把新文件改名。
// 不先删后传,避免上传失败时 release 上没有 latest.json。正常流程里 release 还是草稿,
// 客户端看不到;只有对已发布 release 重跑时,删旧与改名之间才有一次 API 调用的空窗。
const tempName = "latest.json.tmp";
for (const asset of assets.filter((asset) => asset.name === tempName)) {
  await gh(`/releases/assets/${asset.id}`, { method: "DELETE" });
}

const body = JSON.stringify(latest, null, 2);
const uploadUrl = release.upload_url.replace(/\{.*$/, "");
const upload = await fetch(`${uploadUrl}?name=${tempName}`, {
  method: "POST",
  headers: {
    ...headers,
    "Content-Type": "application/json",
    "Content-Length": Buffer.byteLength(body).toString(),
  },
  body,
});

if (!upload.ok) {
  throw new Error(`Upload ${tempName} failed: ${upload.status} ${await upload.text()}`);
}
const uploaded = await upload.json();

for (const asset of assets.filter((asset) => asset.name === "latest.json")) {
  await gh(`/releases/assets/${asset.id}`, { method: "DELETE" });
}

await gh(`/releases/assets/${uploaded.id}`, {
  method: "PATCH",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ name: "latest.json" }),
});

console.log(`Published latest.json for ${Object.keys(platforms).join(", ")}`);
