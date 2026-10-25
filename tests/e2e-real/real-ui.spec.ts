// End-to-end tests against the REAL release app: the actual Tauri window,
// real Rust backend, real host state (sbx pi-ai, registry, services).
//
// The suite boots src-tauri/target/release/pi-supervisor.exe with
// WebView2 remote debugging enabled and attaches Playwright over CDP.
// SAFETY: never clicks Recreate / token push / service toggles here —
// these tests only READ live state. The mutating flows are covered by
// the mock suite (tests/e2e).
import { test, expect, chromium, type Browser, type Page } from "@playwright/test";
import { spawn, execSync } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const CDP_PORT = 9223;
const EXE = path.resolve(
  fileURLToPath(import.meta.url),
  "../../../src-tauri/target/release/pi-supervisor.exe"
);

let appProc: ReturnType<typeof spawn> | null = null;
let browser: Browser | null = null;
let page: Page;

async function cdpPageList(): Promise<any[]> {
  const res = await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`);
  return (await res.json()) as any[];
}

async function launchApp() {
  // Reuse an already-instrumented instance if present.
  try {
    await cdpPageList();
    return;
  } catch {
    /* not running: launch it */
  }
  if (!fs.existsSync(EXE)) {
    throw new Error(`release exe not found: ${EXE} (cargo build --release)`);
  }
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
      const targets = await cdpPageList();
      if (targets.some((t) => t.type === "page")) return;
    } catch {
      /* keep polling */
    }
    if (Date.now() > deadline) throw new Error("WebView2 CDP endpoint never came up");
    await new Promise((r) => setTimeout(r, 500));
  }
}

test.beforeAll(async () => {
  test.setTimeout(60_000);
  await launchApp();
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
  const ctx = browser.contexts()[0];
  const candidate = ctx.pages().find((p) => p.url().includes("tauri.localhost"));
  if (!candidate) {
    throw new Error(`no tauri.localhost page; saw: ${ctx.pages().map((p) => p.url()).join(", ")}`);
  }
  page = candidate;
});

test.afterAll(async () => {
  await browser?.close();
  // Kill only the app itself — NEVER /T: Pageant is a shared host tool.
  if (appProc?.pid) {
    try {
      execSync(`taskkill /PID ${appProc.pid} /F`, { stdio: "ignore" });
    } catch {
      /* already gone */
    }
  }
});

test("app shell renders in the real window", async () => {
  await expect(page.getByTestId("title-bar")).toBeVisible();
  await expect(page.getByTestId("tab-sessions")).toBeVisible();
  await expect(page.getByTestId("tab-services")).toBeVisible();
  await expect(page.getByTestId("tab-sandbox")).toBeVisible();
  await expect(page.getByTestId("startup-row")).toBeVisible();
  await expect(page.getByTestId("status")).toBeVisible();
});

test("sandbox tab shows the live pi-ai state", async () => {
  await page.getByTestId("tab-sandbox").click();
  // Prober polls every 3 s; pi-ai is running on this host.
  await expect(page.getByTestId("sandbox-chip")).toHaveText("detached", {
    timeout: 20_000,
  });
  await expect(page.locator(".sandbox-line", { hasText: "kit" })).toContainText(
    "pi-kit"
  );
});

test("sessions tab polls real pi-dtach", async () => {
  await page.getByTestId("tab-sessions").click();
  // Sandbox exists → the poller answers: either real rows or the empty state.
  const list = page.getByTestId("sessions-list");
  const empty = page.getByTestId("sessions-empty");
  await expect(list.or(empty).first()).toBeVisible({ timeout: 20_000 });
  await expect(page.locator("text=Sandbox not created")).toHaveCount(0);
  // Dead rows must not offer Attach.
  const dead = page.locator(".session-row.dead");
  const n = await dead.count();
  for (let i = 0; i < n; i++) {
    await expect(dead.nth(i).locator("button")).toBeDisabled();
  }
});

test("services tab reflects real supervisor state", async () => {
  await page.getByTestId("tab-services").click();
  // Pageant: supervisor launched it at startup (window probe → running).
  await expect(
    page.getByTestId("service-row-pageant").locator(".state-dot.running")
  ).toBeVisible({ timeout: 20_000 });
  // Bridge + notify bind for real. NOTE: if the legacy python stack is
  // still running it holds 60022 via SO_REUSEADDR, so the first bind can
  // lose a race and the watchdog revives it on the next 3 s tick — hence
  // the generous timeout (observed: failed → "thread exited" → running).
  for (const id of ["bridge", "notify"]) {
    await expect(
      page.getByTestId(`service-row-${id}`).locator(".state-dot.running")
    ).toBeVisible({ timeout: 25_000 });
  }
  await expect(page.getByTestId("status")).toContainText("services");
});

test("startup toggle mirrors the registry, not config.json", async () => {
  // Ground truth from HKCU Run key (expect: absent on this host).
  let regHas = true;
  try {
    execSync(
      'reg query "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run" /v PiSupervisor',
      { stdio: "pipe" }
    );
  } catch {
    regHas = false;
  }
  await expect(page.getByTestId("startup-toggle")).toHaveAttribute(
    "aria-checked",
    regHas ? "true" : "false"
  );
});
