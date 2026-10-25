// MUTATING real-host test (explicitly requested): drives the full Recreate
// round-trip on the live pi-ai sandbox — rm → resolve kit from the
// configured zot_repo → sbx run -d → wait ready → push tokens.
// Reuses the app instance the main real suite instruments (CDP 9223), or
// launches one the same way.
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

test.beforeAll(async () => {
  test.setTimeout(600_000); // launch + the recreate itself
  try {
    await cdpPageList();
  } catch {
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
        if ((await cdpPageList()).some((t) => t.type === "page")) break;
      } catch {
        /* keep polling */
      }
      if (Date.now() > deadline) throw new Error("WebView2 CDP endpoint never came up");
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
  const ctx = browser.contexts()[0];
  const candidate = ctx.pages().find((p) => p.url().includes("tauri.localhost"));
  if (!candidate) throw new Error("no tauri.localhost page");
  page = candidate;
});

test.afterAll(async () => {
  await browser?.close();
  if (appProc?.pid) {
    try {
      execSync(`taskkill /PID ${appProc.pid} /F`, { stdio: "ignore" });
    } catch {
      /* already gone */
    }
  }
});

test("recreate button: full real round-trip against the configured zot_repo", async () => {
  test.setTimeout(480_000); // sbx run may pull a fresh image layer set
  await page.getByTestId("tab-sandbox").click();
  // No precondition on the current state: the flow must work from both
  // detached (rm path) and not-created (pure-create path).
  await expect(page.getByTestId("sandbox-chip")).toBeVisible({
    timeout: 20_000,
  });

  // Dialog copy must come from config (kit_repo), not a hardcoded host.
  await page.getByTestId("btn-recreate").click();
  const overlay = page.getByTestId("confirm-overlay");
  await expect(overlay).toBeVisible();
  await expect(overlay.locator("p")).toContainText(
    /from \S+pi-kit \(newest stamp\)/
  );
  await page.getByTestId("btn-confirm-yes").click();

  // Follow the server-side ticker (ground truth, no flash-race): it must
  // finish with every step done and no error.
  const readState = () =>
    page.evaluate(() =>
      (window as any).__TAURI_INTERNALS__.invoke("get_recreate_state")
    );
  await expect
    .poll(
      async () => {
        const st = await readState();
        if (st.error) throw new Error(`recreate failed: ${st.error}`);
        return st.active ? "running" : "settled";
      },
      { timeout: 420_000, intervals: [1_000] }
    )
    .toBe("settled");
  const final = await readState();
  expect(final.error).toBeNull();
  expect(
    final.steps.every((s: any) => s.status === "done"),
    `steps: ${JSON.stringify(final.steps)}`
  ).toBe(true);
  // The resolved-kit label carries the stamp that was actually pulled.
  expect(final.steps[1].label).toMatch(
    /resolve latest kit \(\d{8}\.\d{6}\.\d{3}\)/
  );

  // Card lands back on detached with a stamped kit, status confirms.
  await expect(page.getByTestId("sandbox-chip")).toHaveText("detached", {
    timeout: 30_000,
  });
  await expect(page.locator(".sandbox-line", { hasText: "kit" })).toContainText(
    "pi-kit:"
  );
  await expect(page.getByTestId("status")).toContainText("Recreate complete", {
    timeout: 6_000,
  });
});
