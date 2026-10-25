import { test, expect } from "@playwright/test";

/**
 * Browser-layer suite: drives the app against the api.ts mock layer
 * (window.__mock). Window-agnostic — no Tauri APIs involved.
 */

test.beforeEach(async ({ page }) => {
  // Fresh navigation re-evaluates the app module → mock state resets.
  await page.goto("/");
});

test("renders title bar, three tabs, startup row and statusbar", async ({ page }) => {
  await expect(page.locator('[data-testid="title-bar"]')).toBeVisible();
  await expect(page.locator('[data-testid="title-bar"]')).toContainText("Pi Supervisor");
  await expect(page.locator('[data-testid="tab-sessions"]')).toBeVisible();
  await expect(page.locator('[data-testid="tab-services"]')).toBeVisible();
  await expect(page.locator('[data-testid="tab-sandbox"]')).toBeVisible();
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

test("Services tab: three rows with Stop buttons; Stop on notify flips it to Start", async ({
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
    "2 of 3 services running",
    { timeout: 8000 }
  );
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
