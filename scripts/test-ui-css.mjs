import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const cssSource = readFileSync(
  new URL("../src/index.css", import.meta.url),
  "utf8"
);

function luminance(hex) {
  const channels = hex
    .slice(1)
    .match(/.{2}/g)
    .map((channel) => Number.parseInt(channel, 16) / 255)
    .map((channel) =>
      channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
    );
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrast(foreground, background) {
  const light = Math.max(luminance(foreground), luminance(background));
  const dark = Math.min(luminance(foreground), luminance(background));
  return (light + 0.05) / (dark + 0.05);
}

function themeBlock(selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = cssSource.match(
    new RegExp(`${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`)
  );
  assert.ok(match, `missing CSS block ${selector}`);
  return match[1];
}

function color(block, token) {
  const match = block.match(
    new RegExp(`--color-${token}:\\s*(#[0-9a-fA-F]{6})`)
  );
  assert.ok(match, `missing color token ${token}`);
  return match[1];
}

function softColorOver(block, token, background) {
  const match = block.match(
    new RegExp(
      `--color-${token}-soft:\\s*rgba\\((\\d+),\\s*(\\d+),\\s*(\\d+),\\s*([\\d.]+)\\)`
    )
  );
  assert.ok(match, `missing soft color token ${token}`);
  const alpha = Number(match[4]);
  const backdrop = background
    .slice(1)
    .match(/.{2}/g)
    .map((channel) => Number.parseInt(channel, 16));
  return `#${[Number(match[1]), Number(match[2]), Number(match[3])]
    .map((channel, index) =>
      Math.round(channel * alpha + backdrop[index] * (1 - alpha))
        .toString(16)
        .padStart(2, "0")
    )
    .join("")}`;
}

assert.match(cssSource, /:focus-visible\s*\{/);
assert.match(cssSource, /outline:\s*2px solid var\(--color-accent\)/);
assert.match(cssSource, /@media\s*\(prefers-reduced-motion:\s*reduce\)/);
assert.match(cssSource, /animation-duration:\s*0\.01ms\s*!important/);
assert.match(cssSource, /scroll-behavior:\s*auto\s*!important/);

for (const selector of ["@theme", '[data-theme="light"]']) {
  const block = themeBlock(selector);
  const muted = color(block, "text-muted");
  for (const surface of ["bg", "card"]) {
    const ratio = contrast(muted, color(block, surface));
    assert.ok(
      ratio >= 4.5,
      `${selector} muted text contrast on ${surface} is ${ratio.toFixed(2)}:1`
    );
  }
  const accentRatio = contrast(
    color(block, "on-accent"),
    color(block, "accent")
  );
  assert.ok(
    accentRatio >= 4.5,
    `${selector} on-accent contrast is ${accentRatio.toFixed(2)}:1`
  );
  const warningRatio = contrast(color(block, "warning"), color(block, "bg"));
  assert.ok(
    warningRatio >= 4.5,
    `${selector} warning contrast is ${warningRatio.toFixed(2)}:1`
  );
  const successRatio = contrast(color(block, "success"), color(block, "card"));
  assert.ok(
    successRatio >= 4.5,
    `${selector} success text contrast on card is ${successRatio.toFixed(2)}:1`
  );
  for (const [token, surface] of [
    ["accent", "sidebar"],
    ["success", "card"],
    ["warning", "bg"],
    ["error", "bg"],
  ]) {
    const foreground = color(block, token);
    const softBackground = softColorOver(block, token, color(block, surface));
    const ratio = contrast(foreground, softBackground);
    assert.ok(
      ratio >= 4.5,
      `${selector} ${token} text contrast on ${token}-soft is ${ratio.toFixed(2)}:1`
    );
  }
}

for (const buttonClass of ["btn-primary", "btn-secondary", "btn-danger"]) {
  const block = themeBlock(`.${buttonClass}`);
  const size = block.match(/font-size:\s*([\d.]+)rem/);
  assert.ok(size, `${buttonClass} must declare a rem font size`);
  assert.ok(
    Number(size[1]) >= 0.78125,
    `${buttonClass} font size must be at least 12.5px`
  );
}

for (const legacyTheme of [
  "slate",
  "forest",
  "violet",
  "linen",
  "mist",
  "sakura",
]) {
  assert.ok(
    !cssSource.includes(`[data-theme="${legacyTheme}"]`),
    `legacy theme ${legacyTheme} must not define a separate palette`
  );
}

for (const className of [
  "desktop-page",
  "desktop-page-header",
  "surface-panel",
  "surface-scroll",
]) {
  assert.ok(
    cssSource.includes(`.${className}`),
    `missing shared ${className} class`
  );
}

const pageNames = [
  "Providers",
  "ProviderDetail",
  "Gateway",
  "Logs",
  "Diagnostics",
  "Tools",
  "Instructions",
  "Mcp",
  "Skills",
  "Settings",
  "Routes",
  "PetChat",
];
for (const pageName of pageNames) {
  const source = readFileSync(
    new URL(`../src/pages/${pageName}.tsx`, import.meta.url),
    "utf8"
  );
  assert.ok(
    source.includes("desktop-page"),
    `${pageName} must use the shared desktop page rhythm`
  );
  assert.ok(
    source.includes("desktop-page-header"),
    `${pageName} must use the shared page header surface`
  );
}

for (const pageName of [
  "Logs",
  "Tools",
  "Instructions",
  "Mcp",
  "Skills",
  "ProviderDetail",
  "PetChat",
]) {
  const source = readFileSync(
    new URL(`../src/pages/${pageName}.tsx`, import.meta.url),
    "utf8"
  );
  assert.ok(
    !source.includes("h-24 bg-gradient-to-b from-accent/10"),
    `${pageName} must use a compact non-decorative intro`
  );
}

const quickSetupSource = readFileSync(
  new URL("../src/pages/QuickSetup.tsx", import.meta.url),
  "utf8"
);
assert.ok(
  quickSetupSource.includes("desktop-page"),
  "QuickSetup must use the shared desktop page rhythm"
);
assert.ok(
  quickSetupSource.includes("surface-panel"),
  "QuickSetup steps must use shared persistent surfaces"
);

const toolsSource = readFileSync(
  new URL("../src/pages/Tools.tsx", import.meta.url),
  "utf8"
);
assert.ok(
  toolsSource.includes("grid-cols-[220px_minmax(0,1fr)]"),
  "Tools must keep its master-detail layout at the minimum desktop width"
);

for (const pageName of ["Instructions", "PetChat"]) {
  const source = readFileSync(
    new URL(`../src/pages/${pageName}.tsx`, import.meta.url),
    "utf8"
  );
  assert.ok(
    !source.includes("calc(100vh"),
    `${pageName} must use the app flex height contract`
  );
}

for (const pageName of ["ProviderDetail", "Logs", "Instructions", "Mcp"]) {
  const source = readFileSync(
    new URL(`../src/pages/${pageName}.tsx`, import.meta.url),
    "utf8"
  );
  assert.ok(
    source.includes("surface-scroll"),
    `${pageName} must own long-content scrolling locally`
  );
}

console.log("UI CSS accessibility checks passed.");
