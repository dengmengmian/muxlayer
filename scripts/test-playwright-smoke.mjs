import http from "node:http";
import { spawn } from "node:child_process";
import fs from "node:fs";
import { chromium } from "playwright";

const root = new URL("..", import.meta.url).pathname;
const host = "127.0.0.1";
const port = Number(
  process.env.AGENTGATE_PLAYWRIGHT_PORT || (await freePort())
);
const baseUrl = `http://${host}:${port}`;
const useDevServer = process.env.AGENTGATE_PLAYWRIGHT_DEV === "1";

const server = spawn(
  "pnpm",
  [
    "exec",
    "vite",
    ...(useDevServer ? [] : ["preview"]),
    "--host",
    host,
    "--port",
    String(port),
  ],
  {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, BROWSER: "none" },
  }
);

let stdout = "";
let stderr = "";
let page;
let pageErrors = [];
let browser;
server.stdout.on("data", (chunk) => {
  stdout += chunk.toString();
});
server.stderr.on("data", (chunk) => {
  stderr += chunk.toString();
});

try {
  await waitForHttp(baseUrl);

  browser = await launchBrowser();
  const context = await browser.newContext({
    viewport: { width: 960, height: 600 },
  });
  await context.addInitScript({ content: tauriMockScript() });
  // 用打包时同一份 CSP 跑页面:策略把界面拦坏时,违规会作为 console error 让冒烟失败。
  const csp = JSON.parse(
    fs.readFileSync(new URL("src-tauri/tauri.conf.json", `file://${root}`), "utf8")
  ).app?.security?.csp;
  if (csp) {
    await context.route("**/*", async (route) => {
      const response = await route.fetch();
      await route.fulfill({
        response,
        headers: { ...response.headers(), "content-security-policy": csp },
      });
    });
  }
  page = await context.newPage();
  pageErrors = [];
  page.on("pageerror", (error) =>
    pageErrors.push(error.stack || error.message)
  );
  page.on("console", (message) => {
    if (message.type() === "error") pageErrors.push(message.text());
  });

  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await assertHealthyPage(page, "/");
  await assertNoDocumentOverflow(page, "/");

  const paths = [
    "/providers",
    "/providers/p1",
    "/routes",
    "/gateway",
    "/logs",
    "/diagnostics",
    "/tools",
    "/instructions",
    "/mcp",
    "/skills",
    "/pet-chat",
    "/quick-setup",
    "/settings",
  ];
  for (const path of paths) {
    await page.goto(`${baseUrl}${path}`, { waitUntil: "networkidle" });
    await assertHealthyPage(page, path);
    await assertNoDocumentOverflow(page, path);
  }

  await page.goto(`${baseUrl}/instructions`, { waitUntil: "networkidle" });
  await page
    .locator("main button")
    .filter({ hasText: /^(Edit|编辑|common\.edit)$/ })
    .first()
    .click();
  await assertLocatorWithinViewport(page, "main textarea", "/instructions");

  await expectLocator(page, 'a[href="/providers"]');
  await expectLocator(page, 'a[href="/gateway"]');
  await expectLocator(page, 'a[href="/logs"]');

  if (pageErrors.length > 0) {
    throw new Error(`Browser errors:\n${pageErrors.join("\n")}`);
  }

  // 宠物窗口与主窗口共用 index.html,按 window label 分流;同样在 CSP 下渲染一遍,
  // 只把 CSP 违规当失败(宠物窗口的 IPC 在 mock 里没覆盖全,其它报错不在这里判定)。
  const cspViolations = [];
  const petPage = await context.newPage();
  await petPage.addInitScript({
    content: tauriMockScript().replaceAll('"main"', '"pet"'),
  });
  petPage.on("console", (message) => {
    if (/Content Security Policy/i.test(message.text())) {
      cspViolations.push(message.text());
    }
  });
  await petPage.goto(baseUrl, { waitUntil: "networkidle" });
  await petPage.waitForTimeout(500);
  if (cspViolations.length > 0) {
    throw new Error(`Pet window CSP violations:\n${cspViolations.join("\n")}`);
  }
  await petPage.close();

  await browser.close();
  console.log("Playwright smoke passed.");
} catch (error) {
  console.error("Playwright smoke failed.");
  console.error(error instanceof Error ? error.message : String(error));
  if (typeof pageErrors !== "undefined" && pageErrors.length > 0) {
    console.error("\nBrowser errors:\n" + pageErrors.join("\n"));
  }
  try {
    if (typeof page !== "undefined") {
      const html = await page.content();
      console.error("\nPage HTML excerpt:\n" + html.slice(0, 2000));
    }
  } catch {
    // ignore diagnostics failure
  }
  console.error("\nVite preview stdout:\n" + stdout);
  console.error("\nVite preview stderr:\n" + stderr);
  process.exitCode = 1;
} finally {
  try {
    await browser?.close();
  } catch {
    // ignore cleanup failure
  }
  server.kill("SIGTERM");
  await new Promise((resolve) => {
    const timer = setTimeout(resolve, 2000);
    server.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function freePort() {
  return await new Promise((resolve, reject) => {
    const srv = http.createServer();
    srv.on("error", reject);
    srv.listen(0, host, () => {
      const address = srv.address();
      srv.close(() => resolve(address.port));
    });
  });
}

async function waitForHttp(url) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try {
      const ok = await new Promise((resolve) => {
        const req = http.get(url, (res) => {
          res.resume();
          resolve(res.statusCode && res.statusCode < 500);
        });
        req.on("error", () => resolve(false));
        req.setTimeout(1000, () => {
          req.destroy();
          resolve(false);
        });
      });
      if (ok) return;
    } catch {
      // retry below
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Vite preview did not become ready at ${url}`);
}

async function assertHealthyPage(page, path) {
  await page.locator("main").waitFor({ state: "visible", timeout: 10_000 });
  const text = (await page.locator("main").innerText()).trim();
  if (text.length < 8) {
    throw new Error(
      `Page ${path} rendered too little content: ${JSON.stringify(text)}`
    );
  }
  const viteOverlay = await page.locator("vite-error-overlay").count();
  if (viteOverlay > 0) {
    throw new Error(`Page ${path} rendered a Vite error overlay`);
  }
}

async function assertNoDocumentOverflow(page, path) {
  const { clientWidth, scrollWidth } = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  if (scrollWidth > clientWidth) {
    throw new Error(
      `Page ${path} overflows horizontally at 960x600: ${scrollWidth}px > ${clientWidth}px`
    );
  }
}

async function expectLocator(page, selector) {
  const count = await page.locator(selector).count();
  if (count < 1) throw new Error(`Expected selector ${selector}`);
}

async function assertLocatorWithinViewport(page, selector, path) {
  const box = await page.locator(selector).first().boundingBox();
  const viewport = page.viewportSize();
  if (!box || !viewport || box.y < 0 || box.y + box.height > viewport.height) {
    throw new Error(
      `Expected ${selector} on ${path} to remain inside the ${viewport?.height ?? "unknown"}px viewport`
    );
  }
}

function tauriMockScript() {
  return String.raw`
    const gatewayStatus = {
      running: true,
      host: "127.0.0.1",
      port: 4141,
      input_protocol: "openai_responses",
      output_protocol: "openai_chat_completions",
      active_provider: "Mock Provider",
      started_at: new Date().toISOString(),
    };
    const gatewaySettings = {
      host: "127.0.0.1",
      port: 4141,
      active_provider_id: null,
      input_protocol: "openai_responses",
      output_protocol: "openai_chat_completions",
      auto_start: true,
      log_retention_days: 30,
      body_filter_global: false,
      thinking_rectifier_global: false,
      error_mapper_global: false,
      health_probe_enabled: false,
      codex_compact_enabled: false,
      codex_compact_summary_max_tokens: null,
    };
    const providers = [{
      id: "p1",
      name: "Mock Provider",
      provider_type: "openai",
      base_url: "https://api.openai.com",
      api_key: null,
      masked_api_key: "sk-***",
      default_model: "gpt-4",
      reasoning_model: null,
      supported_models: JSON.stringify(["gpt-4"]),
      model_capabilities: null,
      model_context_windows: null,
      model_mapping: null,
      extra_headers: null,
      anthropic_base_url: null,
      responses_base_url: null,
      auto_cache_control: true,
      protocol: JSON.stringify(["openai_chat_completions", "openai_responses"]),
      timeout_seconds: 120,
      enabled: true,
      is_active: true,
      status: "connected",
      supports_vision: false,
      supports_cache: null,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    }];
    const routeProfiles = [{
      id: "r1",
      name: "Default Route",
      input_protocol: "openai_responses",
      mode: "manual",
      selection_strategy: "priority",
      is_default: true,
      active_provider_id: "p1",
      active_provider_name: "Mock Provider",
      providers_count: 1,
      enabled: true,
    }];
    const routeProvider = {
      id: "rp1",
      provider_id: "p1",
      provider_name: "Mock Provider",
      provider_type: "openai",
      provider_protocol: JSON.stringify(["openai_responses", "openai_chat_completions"]),
      priority: 1,
      model_override: null,
      routing_conditions: null,
      supports_vision: false,
      supports_cache: null,
      model_capabilities: null,
      has_anthropic_url: false,
      consecutive_failures: 0,
      cooldown_until: null,
    };
    const stats = {
      total: 1,
      success: 1,
      error: 0,
      avg_latency_ms: 12,
      today_total: 1,
      today_success: 1,
      today_error: 0,
      today_errors: 0,
      today_input_tokens: 8,
      today_output_tokens: 4,
      today_cost: 0,
      today_cache_read_tokens: 0,
      today_cache_write_tokens: 0,
      today_codex_compact: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      daily: [
        {
          date: new Date().toISOString().slice(0, 10),
          total: 1,
          errors: 0,
          input_tokens: 8,
          output_tokens: 4,
        },
      ],
      providers: [{ name: "Mock Provider", count: 1 }],
    };
    const commandData = {
      list_providers: providers,
      get_provider: providers[0],
      get_provider_health: {
        provider: "Mock Provider",
        h1_total: 1,
        h1_success: 1,
        h1_success_rate: 100,
        h1_avg_latency_ms: 12,
        h1_p95_latency_ms: 12,
        h24_total: 1,
        h24_success: 1,
        h24_success_rate: 100,
        h24_avg_latency_ms: 12,
        recent_errors: [],
      },
      aggregate_provider_detail_stats: {
        provider: "Mock Provider",
        latency_points: [],
        model_stats: [],
      },
      list_provider_runtime_status: [],
      list_route_profiles: routeProfiles,
      get_route_profile: { profile: routeProfiles[0], providers: [routeProvider] },
      aggregate_route_profile_stats: [],
      get_gateway_status: gatewayStatus,
      get_gateway_settings: gatewaySettings,
      update_gateway_settings: gatewaySettings,
      start_gateway: gatewayStatus,
      stop_gateway: { ...gatewayStatus, running: false },
      restart_gateway: gatewayStatus,
      list_request_logs: [],
      count_request_logs: 0,
      list_log_models: [],
      aggregate_request_logs_by_session: [],
      aggregate_cost_by_model: [],
      aggregate_cost_by_client: [],
      get_request_stats: stats,
      get_request_stats_range: stats,
      get_runtime_kpis: {
        gateway_running: true,
        providers_enabled: 1,
        active_requests: 0,
        uptime_seconds: 60,
        total_requests: 1,
        total_tokens: 12,
        total_cost: 0,
        success_rate_lifetime: 100,
        avg_latency_ms_lifetime: 12,
        total_cost_lifetime: 0,
      },
      list_tools: [
        { id: "codex", name: "Codex", slug: "codex", icon: "code", config_path: "~/.codex/config.toml", description: "Codex", config_exists: false },
      ],
      detect_codex_config: { exists: true, has_agentgate: false },
      detect_claude_code_env: { settings_exists: false, has_api_key: false, has_auth_token: false, has_agentgate: false },
      detect_opencode_config: { exists: false, has_agentgate: false },
      detect_gemini_config: { exists: false, has_agentgate: false },
      detect_atomcode_config: { exists: false, has_agentgate: false },
      detect_claude_desktop: { installed: false, supported: false, has_agentgate_profile: false },
      clients_with_apply_history: [],
      test_tool_connection: { config_ok: true, gateway_ok: true, provider_ok: true },
      list_mcp_servers: [],
      list_model_pricing: [],
      get_gateway_auth_settings: { token_path: "/tmp/agentgate-token" },
      get_local_access_token: "ag_local_mock_token",
      get_pet_settings: { pet_type: "robot", visible: true, pos_x: null, pos_y: null },
      get_pet_click_through: false,
      list_instructions_templates: [],
      read_global_instructions: {
        scope: "claude_global",
        path: "/tmp/CLAUDE.md",
        exists: false,
        content: "",
        size_bytes: 0,
      },
      list_skills: [],
      run_full_self_test: [],
      run_health_check: [],
      run_database_check: [],
      run_gateway_auth_check: [],
      run_provider_check: [],
      run_codex_config_check: [],
      run_claude_code_config_check: [],
      run_route_profile_check: [],
      plugin: null,
    };
    const callbacks = new Map();
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener() {},
    };
    window.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { windowLabel: "main", label: "main" },
      },
      callbacks,
      transformCallback(callback) {
        const id = Math.floor(Math.random() * 1_000_000_000);
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id) {
        callbacks.delete(id);
      },
      runCallback(id, payload) {
        callbacks.get(id)?.(payload);
      },
      unregisterListener() {},
      invoke(cmd, args) {
        if (cmd.startsWith("plugin:event|")) return Promise.resolve(1);
        if (cmd.startsWith("plugin:updater|")) return Promise.resolve(null);
        if (cmd.startsWith("plugin:process|")) return Promise.resolve(null);
        if (cmd.startsWith("plugin:autostart|is_enabled")) return Promise.resolve(false);
        if (cmd.startsWith("plugin:autostart|")) return Promise.resolve(null);
        if (Object.prototype.hasOwnProperty.call(commandData, cmd)) {
          return Promise.resolve(commandData[cmd]);
        }
        console.warn("Unhandled Tauri command in Playwright smoke:", cmd, args);
        return Promise.resolve(null);
      },
    };
  `;
}

async function launchBrowser() {
  try {
    return await chromium.launch({ headless: true });
  } catch (error) {
    const fallback = systemChromiumPath();
    if (!fallback) throw error;
    console.warn(
      `Playwright Chromium is not installed; using system browser: ${fallback}`
    );
    return await chromium.launch({
      headless: true,
      executablePath: fallback,
    });
  }
}

function systemChromiumPath() {
  const candidates = [
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].filter(Boolean);
  return candidates.find((candidate) => fs.existsSync(candidate));
}
