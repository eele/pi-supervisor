import { test, expect } from "@playwright/test";

/**
 * Browser-layer suite: drives the app against the api.ts mock layer
 * (window.__mock). Window-agnostic — no Tauri APIs involved.
 */

test.beforeEach(async ({ page }) => {
  // Fresh navigation re-evaluates the app module → mock state resets.
  await page.goto("/");
});

test("renders title bar, four tabs, startup row and statusbar", async ({ page }) => {
  await expect(page.locator('[data-testid="title-bar"]')).toBeVisible();
  await expect(page.locator('[data-testid="title-bar"]')).toContainText("Pi Supervisor");
  await expect(page.locator('[data-testid="tab-sessions"]')).toBeVisible();
  await expect(page.locator('[data-testid="tab-services"]')).toBeVisible();
  await expect(page.locator('[data-testid="tab-sandbox"]')).toBeVisible();
  await expect(page.locator('[data-testid="tab-psops"]')).toBeVisible();
  await expect(page.locator('[data-testid="startup-row"]')).toBeVisible();
  await expect(page.locator('[data-testid="startup-toggle"]')).toBeVisible();
  await expect(page.locator('[data-testid="status"]')).toBeVisible();

  // Statusbar composition converges after the first polls.
  await expect(page.locator('[data-testid="status"]')).toContainText(
    "All services healthy · pi-ai detached · 3 live sessions · refresh 3 s",
    { timeout: 8000 }
  );
});

test("default Sessions tab shows mock rows: 3 live chips, dead row with disabled Attach", async ({
  page,
}) => {
  const list = page.locator('[data-testid="sessions-list"]');
  await expect(list).toBeVisible({ timeout: 8000 });
  await expect(page.locator('[data-testid="session-row"]')).toHaveCount(4);
  await expect(page.locator(".chip-live")).toHaveCount(3);
  await expect(page.locator(".session-dead")).toHaveText("dead (stale socket)");

  const deadAttach = page.locator(
    '[data-testid="attach-socket_20261024.221503.102"]'
  );
  await expect(deadAttach).toBeDisabled();
  const liveAttach = page.locator(
    '[data-testid="attach-socket_20261025.150210.447"]'
  );
  await expect(liveAttach).toBeEnabled();
});

test("Services tab: four rows with Stop buttons; Stop on notify flips it to Start", async ({
  page,
}) => {
  await page.click('[data-testid="tab-services"]');
  const list = page.locator('[data-testid="service-list"]');
  await expect(list).toBeVisible();
  await expect(page.locator('[data-testid="service-row-pageant"]')).toContainText(
    "Pageant SSH Agent"
  );
  await expect(page.locator('[data-testid="service-row-bridge"]')).toContainText(
    "Agent TCP Bridge"
  );
  const notify = page.locator('[data-testid="service-row-notify"]');
  await expect(notify).toContainText("Notify Hub");
  await expect(notify.locator("button")).toHaveText("Stop");

  await notify.locator("button").click();
  await expect(notify.locator("button")).toHaveText("Start", { timeout: 8000 });

  // Statusbar reflects the stopped service.
  await expect(page.locator('[data-testid="status"]')).toContainText(
    "3 of 4 services running",
    { timeout: 8000 }
  );
});

test("Services tab: hostps row present with the :37022 · audit-DB subtitle", async ({
  page,
}) => {
  await page.click('[data-testid="tab-services"]');
  const hostps = page.locator('[data-testid="service-row-hostps"]');
  await expect(hostps).toContainText("Host PowerShell MCP");
  await expect(hostps).toContainText("37022");
  await expect(hostps).toContainText("~/.pi-su/ps-audit.db");
  await expect(hostps.locator("button")).toHaveText("Stop");
});

test("Sandbox tab: pi-ai with detached chip", async ({ page }) => {
  await page.click('[data-testid="tab-sandbox"]');
  await expect(page.locator(".sandbox-name")).toHaveText("pi-ai", {
    timeout: 8000,
  });
  await expect(page.locator('[data-testid="sandbox-chip"]')).toHaveText(
    "detached"
  );
  await expect(page.locator(".sandbox-line", { hasText: "Mount path" })).toBeVisible();
});

test("Recreate: button opens confirm dialog; confirming runs the step ticker", async ({
  page,
}) => {
  await page.click('[data-testid="tab-sandbox"]');
  const recreateBtn = page.locator('[data-testid="btn-recreate"]');
  await expect(recreateBtn).toBeVisible({ timeout: 8000 });

  await recreateBtn.click();
  const overlay = page.locator('[data-testid="confirm-overlay"]');
  await expect(overlay).toBeVisible();
  await expect(overlay).toContainText("Recreate Sandbox");
  await expect(overlay).toContainText(
    "zot.byte.art/blackbox/pi-kit (newest stamp)"
  );

  await page.locator('[data-testid="btn-confirm-yes"]').click();

  // Button row is replaced by the ticker; steps reach ✔ (mock advances one
  // step per 800 ms poll).
  const ticker = page.locator('[data-testid="step-ticker"]');
  await expect(ticker).toBeVisible({ timeout: 5000 });
  await expect(ticker).toContainText("rm");
  await expect(ticker).toContainText("✔", { timeout: 15000 });

  // After completion the action buttons come back.
  await expect(recreateBtn).toBeVisible({ timeout: 15000 });
});

test("Startup toggle flips its state", async ({ page }) => {
  const toggle = page.locator('[data-testid="startup-toggle"]');
  await expect(toggle).toHaveAttribute("aria-checked", "true"); // mock default ON
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false", {
    timeout: 5000,
  });
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "true", {
    timeout: 5000,
  });
});

test("sandbox not-created: chip reads 'not created', sessions panel gates on it", async ({
  page,
}) => {
  await page.evaluate(() => {
    (window as any).__mock.setSandbox({
      state: "not-created",
      kit: "",
      mount_path: "",
    });
  });

  await page.click('[data-testid="tab-sandbox"]');
  await expect(page.locator('[data-testid="sandbox-chip"]')).toHaveText(
    "not created",
    { timeout: 8000 }
  );
  await expect(
    page.locator(".sandbox-line", { hasText: "residency" })
  ).toHaveCount(0);

  await page.click('[data-testid="tab-sessions"]');
  await expect(
    page.locator('[data-testid="sessions-empty"]', { hasText: "Sandbox not created" })
  ).toBeVisible({ timeout: 3000 });
});

test("PS Audit tab: newest-first list, status chips, count readout", async ({ page }) => {
  await page.click('[data-testid="tab-psops"]');
  const list = page.locator('[data-testid="psop-list"]');
  await expect(list).toBeVisible({ timeout: 8000 });
  await expect(page.locator('[data-testid^="psop-row-"]')).toHaveCount(8);
  // fixture newest ts = id 101 → first row (ts DESC, id DESC)
  await expect(page.locator('[data-testid="psop-row-101"] .psop-chip')).toHaveText("ok");
  await expect(page.locator('[data-testid="psop-row-100"] .psop-chip')).toHaveText("timeout");
  await expect(page.locator('[data-testid="psop-row-99"] .psop-chip')).toHaveText("auth_failed");
  await expect(page.locator('[data-testid="psop-row-97"] .psop-chip')).toHaveText("client_disc.");
  // empty-command rows fall back to the error text in the preview column
  await expect(page.locator('[data-testid="psop-row-99"] .psop-prev-text')).toContainText(
    "invalid token fp"
  );
  await expect(page.locator('[data-testid="psop-count"]')).toContainText(
    "50 / page · total 8 · newest first (ts DESC)"
  );
});

test("PS Audit search filters command/stdout/stderr; statusbar echoes the filter", async ({
  page,
}) => {
  await page.click('[data-testid="tab-psops"]');
  await expect(page.locator('[data-testid="psop-row-101"]')).toBeVisible({ timeout: 8000 });
  // "chrome" only lives in row 101's stdout → matched via output
  await page.fill('[data-testid="psop-search"]', "chrome");
  await expect(page.locator('[data-testid^="psop-row-"]')).toHaveCount(1, { timeout: 5000 });
  await expect(page.locator('[data-testid="status"]')).toContainText(
    'PS Audit: 1 records · showing 1–1 · filter "chrome"',
    { timeout: 5000 }
  );
  await page.fill('[data-testid="psop-search"]', "zzz-nothing");
  await expect(page.locator('[data-testid="psop-empty"]')).toBeVisible({ timeout: 5000 });
});

test("PS Audit row click expands COMMAND / STDOUT / STDERR full text", async ({ page }) => {
  await page.click('[data-testid="tab-psops"]');
  await expect(page.locator('[data-testid="psop-row-100"]')).toBeVisible({ timeout: 8000 });
  await page.click('[data-testid="psop-row-100"]');
  const detail = page.locator('[data-testid="psop-detail"]');
  await expect(detail).toBeVisible({ timeout: 5000 });
  await expect(detail).toContainText("COMMAND");
  await expect(detail.locator(".psop-pre").first()).toContainText(
    "while ($true) { Start-Sleep 5 }"
  );
  await expect(detail).toContainText("STDOUT");
  await expect(detail).toContainText("iperf3: Server listening on 5201");
  await expect(detail).toContainText("STDERR");
  await expect(detail).toContainText("taskkill /T /F");
  await expect(detail).toContainText("req-64"); // request_id meta line
  // second click collapses again
  await page.click('[data-testid="psop-row-100"]');
  await expect(detail).toHaveCount(0);
});

test("PS Audit preview shows an orange … only when the line is width-clipped", async ({
  page,
}) => {
  await page.evaluate(() => {
    const long =
      "Get-Process | Sort-Object WS -Descending | Select-Object -First 10 " +
      "Name, WS, Id, Company, StartTime, Path, Handles, Threads -Property ".repeat(6);
    (window as any).__mock.setPsOps([
      {
        id: 1, ts: "2026-10-26T09:00:00Z", status: "ok", tool: "powershell",
        exit_code: 0, duration_ms: 5, method: "tools-call", stream: false,
        stdout_truncated: false, error: null, command: long, stdout: "x",
        stderr: "", request_id: "r1", client: "c1",
      },
      {
        id: 2, ts: "2026-10-26T09:10:00Z", status: "ok", tool: "powershell",
        exit_code: 0, duration_ms: 5, method: "tools-call", stream: false,
        stdout_truncated: false, error: null, command: "Get-Date", stdout: "",
        stderr: "", request_id: "r2", client: "c1",
      },
    ]);
  });
  await page.click('[data-testid="tab-psops"]');
  const longRow = page.locator('[data-testid="psop-row-1"]');
  await expect(longRow).toBeVisible({ timeout: 8000 });
  await expect(longRow.locator(".psop-prev-text")).toHaveClass(/clipped/);
  const ell = longRow.locator(".psop-ellipsis");
  await expect(ell).toBeVisible();
  await expect(ell).toHaveCSS("color", "rgb(255, 149, 0)");
  // short command stays unclipped → no marker
  const shortRow = page.locator('[data-testid="psop-row-2"]');
  await expect(shortRow.locator(".psop-prev-text")).not.toHaveClass(/clipped/);
  await expect(shortRow.locator(".psop-ellipsis")).not.toBeVisible();
});

test("PS Audit pagination: 120 fixtures → 3 pages with working Prev/Next + statusbar window", async ({
  page,
}) => {
  await page.evaluate(() => {
    const rows = Array.from({ length: 120 }, (_, i) => ({
      id: 120 - i,
      ts: `2026-10-26T0${(i % 9) + 1}:00:00Z`,
      status: i % 2 ? "ok" : "exec_failed",
      tool: "powershell",
      exit_code: 0,
      duration_ms: 5,
      method: "tools-call",
      stream: false,
      stdout_truncated: false,
      error: null,
      command: `cmd-${120 - i}`,
      stdout: "",
      stderr: "",
      request_id: `r${i}`,
      client: "c",
    }));
    (window as any).__mock.setPsOps(rows);
  });
  await page.click('[data-testid="tab-psops"]');
  await expect(page.locator('[data-testid="psop-count"]')).toContainText("total 120", {
    timeout: 8000,
  });
  await expect(page.locator('[data-testid^="psop-row-"]')).toHaveCount(50);
  await expect(page.locator('[data-testid="psop-row-120"]')).toBeVisible();
  await expect(page.locator('[data-testid="psop-row-71"]')).toBeVisible();
  await expect(page.locator('[data-testid="psop-row-70"]')).toHaveCount(0);
  await expect(page.locator('[data-testid="psop-prev"]')).toBeDisabled();

  await page.click('[data-testid="psop-page-2"]');
  await expect(page.locator('[data-testid="psop-row-70"]')).toBeVisible();
  await expect(page.locator('[data-testid="status"]')).toContainText("showing 51–100", {
    timeout: 5000,
  });

  await page.click('[data-testid="psop-next"]');
  await expect(page.locator('[data-testid="psop-row-20"]')).toBeVisible();
  await expect(page.locator('[data-testid="psop-next"]')).toBeDisabled();
  await page.click('[data-testid="psop-prev"]');
  await expect(page.locator('[data-testid="psop-row-70"]')).toBeVisible();
});

test("20 session rows make the list scrollable (overlay scrollbar-able)", async ({
  page,
}) => {
  await page.evaluate(() => {
    const sessions = Array.from({ length: 20 }, (_, i) => ({
      name: `socket_202610${String(20 + (i % 6)).padStart(2, "0")}.${String(
        100000 + i * 12345
      )}.0${i % 10}`,
      live: i < 3,
    }));
    (window as any).__mock.setSessions({
      has_data: true,
      poll_error: "",
      sessions,
    });
  });

  const list = page.locator('[data-testid="sessions-list"]');
  await expect(list).toBeVisible({ timeout: 8000 });
  await expect
    .poll(
      async () =>
        list.evaluate(
          (el) => (el as HTMLElement).scrollHeight > (el as HTMLElement).clientHeight
        ),
      { timeout: 10000 }
    )
    .toBe(true);
});
