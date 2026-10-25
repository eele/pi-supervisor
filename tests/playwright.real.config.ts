// Real-UI E2E: drives the actual release exe's WebView2 over CDP
// (the spec itself boots the app with --remote-debugging-port).
// Kept separate from the mock suite: tests/e2e + playwright.config.ts.
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e-real",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60_000,
  reporter: [["list"]],
});
