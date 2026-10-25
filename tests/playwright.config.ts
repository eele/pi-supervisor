import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  retries: 1,
  reporter: "html",
  use: {
    baseURL: "http://localhost:5173",
    browserName: "chromium",
    viewport: { width: 800, height: 600 },
  },
  webServer: {
    command: "npx vite --port 5173 --strictPort",
    cwd: "..",
    url: "http://localhost:5173",
    reuseExistingServer: true,
    timeout: 30000,
  },
});
