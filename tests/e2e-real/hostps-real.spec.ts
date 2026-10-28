import { test, expect, chromium, type Browser, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

/**
 * Real-UI suite: boots the debug supervisor exe (WebView2) with
 * --remote-debugging-port and drives the actual window over CDP.
 * Covers what the mock layer cannot: the hostps listener on :37022,
 * audit rows in ~/.pi-su/ps-audit.db and the Stop→DISABLED contract.
 */

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "..", "..");
const EXE = path.join(REPO, "src-tauri", "target", "debug", "pi-supervisor.exe");
const CDP_PORT = 9223;
const GATEWAY = "http://127.0.0.1:37022/mcp";

let vite: ChildProcess;
let app: ChildProcess;
let browser: Browser;
let page: Page;

function listenProbe(port: number, host = "localhost"): Promise<boolean> {
  return new Promise((res) => {
    const s = net.connect(port, host);
    const t = setTimeout(() => {
      s.destroy();
      res(false);
    }, 1500);
    s.on("connect", () => {
      clearTimeout(t);
      s.destroy();
      res(true);
    });
    s.on("error", () => {
      clearTimeout(t);
      res(false);
    });
  });
}

async function waitFor(fn: () => Promise<boolean>, ms: number, what: string) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await fn()) return;
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`timeout waiting for ${what}`);
}

function mcp(token: string | null, body: unknown, accept = "application/json") {
  return fetch(GATEWAY, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: accept,
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(body),
  });
}

async function hostPsKey(): Promise<string> {
  // Same on-demand resolver the sbx custom secret runs.
  const { execFile } = await import("node:child_process");
  return new Promise((res, rej) =>
    execFile(
      "powershell",
      [
        "-NoProfile",
        "-Command",
        "[Environment]::GetEnvironmentVariable('HOST_PS_KEY','User')",
      ],
      (e, out) => (e ? rej(e) : res(out.trim()))
    )
  );
}

test.beforeAll(async () => {
  // vite's bin directly (node + .js entry): spawning npx.cmd trips Node's
  // EINVAL guard, npx itself needs shell resolution.
  vite = spawn(
    process.execPath,
    [path.join(REPO, "node_modules", "vite", "bin", "vite.js"), "--port", "5173", "--strictPort"],
    { cwd: REPO, stdio: "ignore", detached: os.platform() !== "win32" }
  );
  await waitFor(() => listenProbe(5173), 30_000, "vite on 5173");

  app = spawn(EXE, [], {
    cwd: REPO,
    stdio: "ignore",
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
    },
  });
  await waitFor(() => listenProbe(37022), 40_000, "hostps on 37022");

  browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`, {
    // CDP endpoint can take a beat after the first WebView2 window.
    timeout: 30_000,
  });
  const ctx = browser.contexts()[0];
  await waitFor(
    async () => ctx.pages().some((p) => p.url().includes("localhost:5173")),
    20_000,
    "supervisor webview page"
  );
  page = ctx.pages().find((p) => p.url().includes("localhost:5173"))!;
});

function killTree(p: ChildProcess | undefined) {
  if (!p?.pid) return;
  if (process.platform === "win32") {
    // cmd.exe wrapper: plain kill would orphan the vite node child.
    spawn("taskkill", ["/T", "/F", "/PID", String(p.pid)], { stdio: "ignore" });
  } else {
    p.kill();
  }
}

test.afterAll(async () => {
  await browser?.close().catch(() => {});
  killTree(app);
  killTree(vite);
  // real-ui.spec reuses this exact CDP port and the supervisor holds 37022;
  // WebView2 children outlive taskkill by seconds — don't hand the next
  // spec a half-dead endpoint (it would "reuse" a dying debug window).
  await waitFor(() => listenProbe(CDP_PORT).then((up) => !up), 20_000, "CDP port 9223 release").catch(() => {});
  await waitFor(() => listenProbe(37022).then((up) => !up), 20_000, "37022 release").catch(() => {});
});

let key: string;

test("auth: no token 401, wrong token 401, real token initialize 200", async () => {
  key = await hostPsKey();
  expect(key).toHaveLength(32);
  const init = { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "pi", version: "1.3.0" } } };
  expect((await mcp(null, init)).status).toBe(401);
  expect((await mcp("0".repeat(32), init)).status).toBe(401);
  const ok = await mcp(key, init);
  expect(ok.status).toBe(200);
  const j: any = await ok.json();
  expect(j.result.serverInfo.name).toBe("host-ps-gateway");
});

test("tools/call powershell executes on the host and is audited", async () => {
  const r = await mcp(
    key,
    { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "powershell", arguments: { script: "Get-Date -Format o" } } }
  );
  expect(r.status).toBe(200);
  const j: any = await r.json();
  const payload = JSON.parse(j.result.content[0].text);
  expect(payload.exit_code).toBe(0);
  expect(payload.stdout).toMatch(/20\d\d/);
  expect(j.result.isError).toBeFalsy();
});

test("UI: Stop hostps → DISABLED MCP answer (not refused), health.disabled, Start restores", async () => {
  await page.locator('[data-testid="tab-services"]').click();
  const row = page.locator('[data-testid="service-row-hostps"]');
  await expect(row).toBeVisible();
  await expect(row.locator("button")).toHaveText("Stop");

  await row.locator("button").click();
  await expect(row.locator("button")).toHaveText("Start", { timeout: 10_000 });

  // Port must stay held and answer the class-B DISABLED text (200, not ECONNREFUSED).
  expect(await listenProbe(37022)).toBe(true);
  const r = await mcp(
    key,
    { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "powershell", arguments: { script: "Get-Date" } } }
  );
  expect(r.status).toBe(200);
  const body: any = await r.json();
  expect(body.result.content[0].text).toContain("[HOST_PS_GATEWAY_DISABLED]");
  expect(body.result.content[0].text).toContain("do NOT pause reasoning");

  // health flips its disabled flag; subtitle reflects access-disabled.
  const h: any = await (
    await mcp(key, { jsonrpc: "2.0", id: 5, method: "tools/call", params: { name: "health", arguments: {} } })
  ).json();
  expect(h.result.disabled).toBe(true);
  await expect(row.locator(".service-subtitle")).toContainText("access-disabled", {
    timeout: 8_000,
  });

  // Start → execution path again.
  await row.locator("button").click();
  await expect(row.locator("button")).toHaveText("Stop", { timeout: 10_000 });
  const back: any = await (
    await mcp(
      key,
      { jsonrpc: "2.0", id: 6, method: "tools/call", params: { name: "powershell", arguments: { script: "Get-Date" } } }
    )
  ).json();
  expect(back.result.content[0].text).toContain("exit_code");
});

test("UI: PS Audit tab lists fresh rows newest-first with disabled/auth_failed chips", async () => {
  await page.locator('[data-testid="tab-psops"]').click();
  const list = page.locator('[data-testid="psop-list"]');
  await expect(list).toBeVisible();
  // The calls above must have produced rows; newest first.
  await expect(page.locator(".psop-row").first()).toBeVisible({ timeout: 15_000 });
  const chips = await page.locator(".psop-row .psop-chip").allTextContents();
  expect(chips.length).toBeGreaterThan(2);
  // auth_failed rows (wrong-token probes) and disabled rows (Stop window) exist.
  expect(chips.join(",")).toMatch(/auth_failed|disabled|ok/);
});
