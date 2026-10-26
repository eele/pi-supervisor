// Tray "Exit" must quit PROMPTLY. Regression: the child-pid reap registry
// was append-only (every 3 s poller/notify spawned a registered child), so
// exit-time taskkill serialized thousands of stale pids — the app appeared
// to hang on Exit for minutes.
import { test, expect, chromium, type Browser, type Page } from "@playwright/test";
import { spawn, execSync } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const CDP_PORT = 9224; // dedicated: this spec KILLS its instance
const EXE = path.resolve(
  fileURLToPath(import.meta.url),
  "../../../src-tauri/target/release/pi-supervisor.exe"
);

let appProc: ReturnType<typeof spawn> | null = null;
let browser: Browser | null = null;
let page: Page;

function pidAlive(pid: number): boolean {
  try {
    const out = execSync(`tasklist /FI "PID eq ${pid}"`, { encoding: "utf8" });
    return out.includes(String(pid));
  } catch {
    return false;
  }
}

test.beforeAll(async () => {
  test.setTimeout(60_000);
  if (!fs.existsSync(EXE)) throw new Error(`release exe not found: ${EXE}`);
  appProc = spawn(EXE, [], {
    detached: true,
    stdio: "ignore",
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT} --remote-allow-origins=*`,
    },
  });
  appProc.unref();
  const deadline = Date.now() + 30_000;
  for (;;) {
    try {
      const r = await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`);
      const targets = (await r.json()) as any[];
      if (targets.some((t) => t.type === "page")) break;
    } catch {
      /* keep polling */
    }
    if (Date.now() > deadline) throw new Error("WebView2 CDP endpoint never came up");
    await new Promise((r) => setTimeout(r, 500));
  }
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
  const ctx = browser.contexts()[0];
  const candidate = ctx.pages().find((p) => p.url().includes("tauri.localhost"));
  if (!candidate) throw new Error("no tauri.localhost page");
  page = candidate;
});

test.afterAll(async () => {
  await browser?.close();
  if (appProc?.pid && pidAlive(appProc.pid)) {
    try {
      execSync(`taskkill /PID ${appProc.pid} /F`, { stdio: "ignore" });
    } catch {
      /* already gone */
    }
  }
});

test("Chinese notify round-trips and tray Exit quits promptly", async () => {
  test.setTimeout(90_000);
  // Let the pollers run a few cycles (they register a child each) and push
  // one REAL Chinese toast through the hub (powershell child too).
  await new Promise((r) => setTimeout(r, 8_000));
  const res = await fetch("http://localhost:60000/notify", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ title: "推理完成", message: "pi 已完成 ✅ <b>&test</b> $pid `0" }),
  });
  const js = (await res.json()) as any;
  expect(res.status, `notify failed: ${JSON.stringify(js)}`).toBe(200);
  expect(js.ok).toBe(true);

  const pid = appProc!.pid!;
  const t0 = Date.now();
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("tray_menu_action", { action: "exit" })
  );
  // The tray-menu window may not exist yet (never opened) — that's fine:
  // the command still exits the app.
  while (Date.now() - t0 < 60_000 && pidAlive(pid)) {
    await new Promise((r) => setTimeout(r, 200));
  }
  const elapsed = Date.now() - t0;
  expect(pidAlive(pid), "app never exited").toBe(false);
  // Generous vs a healthy ~0.5 s, but far below the old multi-minute freeze.
  expect(elapsed, `Exit took ${elapsed} ms`).toBeLessThan(5_000);
});
