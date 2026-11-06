import { expect } from "@playwright/test";
import { DCIM } from "../playwright.config";
import { DEMO, TID, flashbar, openScope } from "./fixtures";
import { expectAxeClean } from "./auth-helpers";
import { DCIM_BASE, dcimTest as test, openDcim } from "./tool-helpers";

// M5 fix round 1 (A11Y-001/002/003): finch embedded UX. DCIM_REPO only.
test.skip(!DCIM.enabled, "DCIM_REPO not set (usg-dcim E2E is local-only, brief Q9)");

test.describe("dcim embedded UX (fix round 1)", () => {
  test("A11Y-002: session ended is announced once in the console and focus moves to the heading; Retry session recovers", async ({ page }) => {
    const { fl, frame } = await openDcim(page, DCIM.origin, "/racks");
    let deny = true;
    await page.route(`${DCIM.origin}${DCIM_BASE}/api/v1/**`, (r) =>
      deny ? r.fulfill({ status: 401, contentType: "application/json", body: '{"detail":"invalid console assertion"}' }) : r.continue(),
    );
    // Console-side navigation makes finch fetch -> 401.
    await page.getByTestId(TID.sideNav).getByRole("link", { name: /^Sites$/ }).click();
    await expect(fl.getByTestId("session-ended")).toBeVisible({ timeout: 20_000 });
    await expect(flashbar(page)).toContainText("DCIM session ended");
    await expect.poll(() => frame.evaluate(() => document.activeElement?.id)).toBe("session-ended-heading");
    expect(await fl.locator('[data-testid="session-ended"] [role="alert"]').count(), "no second announcement in-frame").toBe(0);
    // finch remounts the view once more when a delayed query retry hits 401
    // (finding R3-1); let it settle before clicking.
    await page.waitForTimeout(4000);
    deny = false;
    await fl.getByRole("button", { name: "Retry session" }).click();
    await expect(fl.getByTestId("session-ended")).toHaveCount(0, { timeout: 15_000 });
  });

  test("A11Y-001: console theme 'system' follows prefers-color-scheme in finch", async ({ page }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    const { fl } = await openDcim(page, DCIM.origin, "/racks");
    const toggle = page.getByTestId(TID.themeToggle);
    for (let i = 0; i < 3 && !/system/i.test((await toggle.textContent()) ?? ""); i++) await toggle.click();
    await expect(toggle).toContainText(/system/i);
    await expect(fl.locator("html")).toHaveClass(/\bdark\b/);
    await page.emulateMedia({ colorScheme: "light" });
    await expect(fl.locator("html")).not.toHaveClass(/\bdark\b/);
    await page.emulateMedia({ colorScheme: "dark" });
    await expect(fl.locator("html")).toHaveClass(/\bdark\b/);
  });

  test("A11Y-003: a console site with no DCIM fabric shows the scope-unmatched message", async ({ page }) => {
    const { fl } = await openDcim(page, DCIM.origin, "/racks");
    await (await openScope(page, "site")).selectOption(DEMO.singleTenantSite);
    await expect(fl.getByTestId("scope-unmatched")).toContainText(`No DCIM fabric for site ${DEMO.singleTenantSite}`);
    await (await openScope(page, "site")).selectOption(DEMO.multiSite);
    await expect(fl.getByTestId("scope-unmatched")).toHaveCount(0);
  });

  for (const path of ["/racks", "/sites"]) {
    test(`axe in dark mode: /tools/dcim${path}`, async ({ page, browserName }) => {
      test.skip(browserName !== "chromium", "axe runs on Chromium");
      await page.emulateMedia({ colorScheme: "dark" });
      const { fl } = await openDcim(page, DCIM.origin, path);
      await expect(fl.locator("html")).toHaveClass(/\bdark\b/);
      await expectAxeClean(page, `dcim dark ${path}`);
    });
  }
});
