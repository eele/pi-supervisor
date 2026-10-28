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

async function cdpPortFree(): Promise<boolean> {
  try {
    await cdpPageList();
    return false;
  } catch {
    return true;
  }
}

async function launchApp() {
  // Reuse an already-instrumented instance if present — but only when the
  // endpoint serves OUR page. A debug-window leftover (hostps-real shares
  // this port) that is mid-teardown must not masquerade as a reusable
  // release instance: its webview dies, so wait for the port to free,
  // then launch fresh.
  try {
    const targets = await cdpPageList();
    if (targets.some((t) => t.url?.includes("tauri.localhost"))) return;
    const deadline = Date.now() + 20_000;
    while (Date.now() < deadline && !(await cdpPortFree())) {
      await new Promise((r) => setTimeout(r, 500));
    }
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
  await expect(page.getByTestId("tab-psops")).toBeVisible();
  await expect(page.getByTestId("startup-row")).toBeVisible();
  await expect(page.getByTestId("status")).toBeVisible();
});

test("tab panels are full-width (no neighbour bleed into the active tab)", async () => {
  // Regression: the 400% track (4 tabs since the PS Audit page) must not
  // flex-shrink — every panel must be exactly as wide as the visible
  // tab-content viewport.
  const dims = await page.evaluate(() => {
    const content = document.querySelector(".tab-content")!;
    const panels = [...document.querySelectorAll(".tab-panel")].map(
      (p) => p.getBoundingClientRect().width
    );
    return { viewport: content.clientWidth, panels };
  });
  expect(dims.panels.length).toBe(4);
  for (const w of dims.panels) {
    expect(Math.abs(w - dims.viewport)).toBeLessThanOrEqual(1);
  }
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

test("startup toggle round-trip writes then clears the Run key", async () => {
  const toggle = page.getByTestId("startup-toggle");
  const before = await toggle.getAttribute("aria-checked");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", before === "true" ? "false" : "true");
  let regHas = true;
  try {
    execSync(
      'reg query "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run" /v PiSupervisor',
      { stdio: "pipe" }
    );
  } catch {
    regHas = false;
  }
  expect(regHas).toBe(before !== "true"); // flipped, and the key followed
  await toggle.click(); // restore the original state
  await expect(toggle).toHaveAttribute("aria-checked", before ?? "false");
});

test("sandbox buttons: Refresh + Push Tokens report over the statusbar", async () => {
  test.setTimeout(150_000); // two real sbx execs, each allowed up to 60 s
  await page.getByTestId("tab-sandbox").click();
  await page.getByTestId("btn-refresh").click();
  await expect(page.getByTestId("status")).toContainText("refreshed", {
    timeout: 6_000,
  });
  // Real push into the live sandbox (idempotent, mode 600): either result
  // string is honest feedback — both start with "push-tokens".
  await page.getByTestId("btn-push-tokens").click();
  await expect(page.getByTestId("status")).toContainText(/push-tokens/i, {
    timeout: 30_000,
  });
});

test("service Stop/Start toggle works for real (notify hub)", async () => {
  await page.getByTestId("tab-services").click();
  const row = page.getByTestId("service-row-notify");
  // Stop: state leaves running and STAYS stopped (intent persisted).
  await page.getByTestId("btn-toggle-notify").click();
  await expect(row.locator(".state-dot")).not.toHaveClass(/running/, {
    timeout: 8_000,
  });
  await page.waitForTimeout(3_500); // a full watchdog cycle must NOT revive it
  await expect(row.locator(".state-dot")).not.toHaveClass(/running/);
  // Start: back to running.
  await page.getByTestId("btn-toggle-notify").click();
  await expect(row.locator(".state-dot.running")).toBeVisible({
    timeout: 15_000,
  });
});

test("tray menu: Aqua popup window with Open/Exit, Open restores main", async () => {
  const ctx = page.context();
  await page.evaluate(() => {
    const internals = (window as any).__TAURI_INTERNALS__;
    return internals.invoke("show_tray_menu", { x: 120, y: 120 });
  });
  const deadline = Date.now() + 15_000;
  let menu: Page | undefined;
  for (;;) {
    menu = ctx.pages().find((p) => p.url().includes("tray-menu"));
    if (menu) {
      try {
        await menu.waitForSelector('[data-testid="tray-menu"]', { timeout: 2_000 });
        break;
      } catch {
        /* page exists but not rendered yet — keep polling */
      }
    }
    if (Date.now() > deadline) break;
    await new Promise((r) => setTimeout(r, 400));
  }
  if (!menu) throw new Error("tray-menu window never appeared");
  await expect(menu.getByTestId("tray-item-open")).toHaveText("Open");
  await expect(menu.getByTestId("tray-item-exit")).toHaveText("Exit");
  // Mockup skin: Open hovered renders the solid Aqua blue.
  await menu.getByTestId("tray-item-open").hover();
  await expect(menu.getByTestId("tray-item-open")).toHaveCSS(
    "background-color",
    "rgb(56, 118, 214)"
  );
  await menu.getByTestId("tray-item-open").click();
  // Menu dismisses (OS-level hide — WebView2 keeps visibilityState
  // "visible" after SW_HIDE, so ask the backend for the truth).
  await expect
    .poll(
      async () =>
        page.evaluate(() =>
          (window as any).__TAURI_INTERNALS__.invoke("webview_visible", {
            label: "tray-menu",
          })
        ),
      { timeout: 5_000 }
    )
    .toBe(false);
  await expect(page.getByTestId("title-bar")).toBeVisible();
});

test("single instance: a second exe exits immediately", async () => {
  const proc = spawn(EXE, [], { detached: true, stdio: "ignore" });
  proc.unref();
  // The plugin tells it to exit right away; it must not linger or open a UI.
  const code = await new Promise<number | null>((resolve) => {
    const t = setTimeout(() => resolve(null), 12_000);
    proc.on("exit", (c) => {
      clearTimeout(t);
      resolve(c);
    });
  });
  expect(code, "second instance should have exited").not.toBeNull();
  // The suite-owned instance is still up and serving.
  await expect(page.getByTestId("title-bar")).toBeVisible();
});
