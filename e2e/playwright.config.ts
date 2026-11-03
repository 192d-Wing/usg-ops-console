import { defineConfig, devices } from "@playwright/test";

// console-gateway serves the gallery bundle (built by `make web-gallery`).
// Port 8081 is reserved for E2E so it never collides with `make run` (8080).
const PORT = Number(process.env.E2E_PORT ?? "8081");
const BASE_URL = `http://127.0.0.1:${PORT}`;
// A second gateway serves the release bundle `dist` (built by `make web`) for
// the dist-* smoke projects (NIST-006: SC-18 / CM-7 evidence on what ships).
const DIST_PORT = Number(process.env.E2E_DIST_PORT ?? "8082");
const DIST_URL = `http://127.0.0.1:${DIST_PORT}`;
const SMOKE = /dist-smoke\.spec\.ts$/;
const isCI = Boolean(process.env.CI);

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: isCI,
  retries: isCI ? 1 : 0,
  ...(isCI ? { workers: 2 } : {}),
  reporter: isCI
    ? [["list"], ["html", { open: "never" }], ["json", { outputFile: "test-results/results.json" }]]
    : [["list"], ["html", { open: "never" }]],
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "chromium", testIgnore: SMOKE, use: { ...devices["Desktop Chrome"] } },
    { name: "firefox", testIgnore: SMOKE, use: { ...devices["Desktop Firefox"] } },
    { name: "webkit", testIgnore: SMOKE, use: { ...devices["Desktop Safari"] } },
    { name: "dist-chromium", testMatch: SMOKE, use: { ...devices["Desktop Chrome"], baseURL: DIST_URL } },
    { name: "dist-firefox", testMatch: SMOKE, use: { ...devices["Desktop Firefox"], baseURL: DIST_URL } },
    { name: "dist-webkit", testMatch: SMOKE, use: { ...devices["Desktop Safari"], baseURL: DIST_URL } },
  ],
  webServer: [
    {
      command: "../target/release/console-gateway",
      url: `${BASE_URL}/`,
      env: {
        CONSOLE_LISTEN: `127.0.0.1:${PORT}`,
        CONSOLE_DIST: "../crates/console-shell/dist-gallery",
      },
      reuseExistingServer: !isCI,
      timeout: 30_000,
      stdout: "pipe",
      stderr: "pipe",
    },
    {
      command: "../target/release/console-gateway",
      url: `${DIST_URL}/`,
      env: {
        CONSOLE_LISTEN: `127.0.0.1:${DIST_PORT}`,
        CONSOLE_DIST: "../crates/console-shell/dist",
      },
      reuseExistingServer: !isCI,
      timeout: 30_000,
      stdout: "pipe",
      stderr: "pipe",
    },
  ],
});
