import { defineConfig, devices } from "@playwright/test";

const CI = !!process.env.CI;

// The suite runs the real frontend in Chromium against Vite, with a fake backend injected by the
// tests (see e2e/support/fakeBackend.ts). It needs no Tauri window, no network and no account, so it
// is platform independent. One retry at most, and only on CI: a test that needs more is flaky and
// must be fixed, not retried.
export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  workers: CI ? 2 : undefined,
  timeout: 30_000,
  expect: { timeout: 7_000 },
  reporter: CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    ...devices["Desktop Chrome"],
    baseURL: "http://127.0.0.1:1420",
    viewport: { width: 1280, height: 720 },
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    // Deterministic rendering: no OS-level motion preference leaks into a run.
    reducedMotion: "no-preference",
    locale: "en-US",
    colorScheme: "light",
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
  webServer: {
    command: "pnpm dev",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !CI,
    timeout: 60_000,
  },
});
