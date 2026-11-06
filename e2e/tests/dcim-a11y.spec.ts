import { expect } from "@playwright/test";
import { DCIM } from "../playwright.config";
import { expectAxeClean } from "./auth-helpers";
import { dcimTest as test, openDcim, toolFrame } from "./tool-helpers";

// M5 (plan U6/§8): axe (WCAG 2.1 A/AA, no serious/critical) on the console's
// DCIM container route and two finch routes. axe-core/playwright scans the
// tool iframe too. Chromium only, like the other axe checks. DCIM_REPO only.
test.skip(!DCIM.enabled, "DCIM_REPO not set (usg-dcim E2E is local-only, brief Q9)");

test.describe("dcim a11y", () => {
  for (const path of ["/", "/racks", "/sites"]) {
    test(`axe: /tools/dcim${path}`, async ({ page, browserName }) => {
      test.skip(browserName !== "chromium", "axe runs on Chromium");
      await openDcim(page, DCIM.origin, path);
      // Let finch's queries settle so axe sees rendered content, not spinners.
      await expect(toolFrame(page).locator('[class*="spinner"]')).toHaveCount(0, { timeout: 15_000 }).catch(() => undefined);
      await expectAxeClean(page, `dcim container ${path}`);
    });
  }
});
