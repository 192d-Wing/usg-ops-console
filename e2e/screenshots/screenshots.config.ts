import { defineConfig, devices } from "@playwright/test";

// M4.5 before/after screenshots (plan §7). Not part of the E2E suite: run
// against an already running gateway (no webServer here):
//   SHOT_BASE=https://localhost:9443 SHOT_LABEL=before|after SHOT_OUT=<dir> \
//     npx playwright test -c screenshots/screenshots.config.ts
export default defineConfig({
  testDir: ".",
  testMatch: /capture\.spec\.ts$/,
  workers: 1,
  reporter: [["list"]],
  outputDir: "../test-results/screenshots",
  use: {
    ...devices["Desktop Chrome"],
    baseURL: process.env.SHOT_BASE ?? "https://localhost:9443",
    ignoreHTTPSErrors: true,
    reducedMotion: "reduce",
  },
});
