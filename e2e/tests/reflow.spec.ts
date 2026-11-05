import { expect, test } from "@playwright/test";
import { TID, expectInsideViewport, expectNoHorizontalScroll, gotoApp, openPalette, openScope } from "./fixtures";

/**
 * SC 1.4.10 Reflow at 320 x 640 CSS px (= 1280 px wide at 400% zoom).
 * A11Y-001 / A11Y-002: no horizontal scrolling, and every top-bar panel opens
 * fully inside the viewport. Runs in every engine.
 * M4.5: the service menu is gone; the palette, the compact scope popover and
 * the rail drawer (+ scrim) are the new top-bar surfaces.
 */
test.use({ viewport: { width: 320, height: 640 } });

for (const path of ["/", "/gallery"] as const) {
  test(`reflow 320: ${path} has no horizontal scroll`, async ({ page }) => {
    await gotoApp(page, path);
    await expectNoHorizontalScroll(page);
  });
}

test("reflow 320: user menu panel stays inside the viewport", async ({ page }) => {
  await gotoApp(page, "/");
  await page.getByTestId(TID.userMenuButton).click();
  await expectInsideViewport(page, page.getByTestId(TID.userMenuPanel));
  await expectNoHorizontalScroll(page);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId(TID.userMenuPanel)).toBeHidden();
});

test("reflow 320: palette opens as an in-viewport sheet without horizontal scroll", async ({ page }) => {
  await gotoApp(page, "/");
  await openPalette(page);
  await expectInsideViewport(page, page.getByTestId(TID.palette).getByRole("dialog"));
  await expectNoHorizontalScroll(page);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId(TID.palette)).toBeHidden();
  await expect(page.getByTestId(TID.paletteTrigger)).toBeFocused();
});

test("reflow 320: scope collapses to one trigger; its popover with all three selects stays in the viewport", async ({ page }) => {
  await gotoApp(page, "/");
  await expect(page.getByTestId(TID.scopeTrigger)).toBeVisible();
  await expect(page.getByTestId("scope-site-trigger")).toBeHidden();
  const site = await openScope(page, "site");
  const pop = page.getByTestId(TID.scopePopover);
  await expectInsideViewport(page, pop);
  for (const s of [site, page.getByTestId(TID.tenant), page.getByTestId(TID.env)]) {
    await expect(s).toBeVisible();
    const b = await s.boundingBox();
    expect(b && b.x >= 0 && b.x + b.width <= 320.5, `select ${JSON.stringify(b)} inside 320`).toBe(true);
  }
  await expectNoHorizontalScroll(page);
  await page.keyboard.press("Escape");
  await expect(pop).toBeHidden();
  await expect(page.getByTestId(TID.scopeTrigger)).toBeFocused();
});

test("reflow 320: nav drawer + scrim, no horizontal scroll; scrim click closes and focuses the toggle", async ({ page }) => {
  await gotoApp(page, "/");
  const toggle = page.getByTestId(TID.navToggle);
  const region = page.locator("#side-nav-region");
  await expect(region).toBeHidden();
  await expect(page.getByTestId(TID.railScrim)).toHaveCount(0);
  await toggle.click();
  await expect(region).toBeVisible();
  await expect(region.getByRole("link").first()).toBeFocused();
  await expectInsideViewport(page, region);
  const scrim = page.getByTestId(TID.railScrim);
  await expect(scrim).toBeVisible();
  await expectNoHorizontalScroll(page);
  // Click the scrim area to the right of the drawer.
  const rb = await region.boundingBox();
  await page.mouse.click(Math.min(315, (rb?.x ?? 0) + (rb?.width ?? 0) + 10), 320);
  await expect(region).toBeHidden();
  await expect(toggle).toBeFocused();
});
