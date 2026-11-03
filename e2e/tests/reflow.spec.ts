import { expect, test, type Locator, type Page } from "@playwright/test";
import { TID, gotoApp } from "./fixtures";

/**
 * SC 1.4.10 Reflow at 320 x 640 CSS px (= 1280 px wide at 400% zoom).
 * A11Y-001 / A11Y-002: no horizontal scrolling, and both top-bar dropdown
 * panels open fully inside the viewport. Runs in every engine.
 */
test.use({ viewport: { width: 320, height: 640 } });

async function expectNoHorizontalScroll(page: Page): Promise<void> {
  const { scrollWidth, clientWidth } = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(scrollWidth, `scrollWidth ${scrollWidth} > clientWidth ${clientWidth}`).toBeLessThanOrEqual(clientWidth);
}

async function expectInsideViewport(page: Page, panel: Locator): Promise<void> {
  await expect(panel).toBeVisible();
  const box = await panel.boundingBox();
  expect(box).not.toBeNull();
  if (!box) return;
  const innerWidth = await page.evaluate(() => window.innerWidth);
  expect(box.x, `panel left ${box.x}`).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width, `panel right ${box.x + box.width} > ${innerWidth}`).toBeLessThanOrEqual(innerWidth);
}

for (const path of ["/", "/gallery"] as const) {
  test(`reflow 320: ${path} has no horizontal scroll`, async ({ page }) => {
    await gotoApp(page, path);
    await expectNoHorizontalScroll(page);
  });
}

const MENUS = [
  { name: "service menu", button: TID.serviceMenuButton, panel: TID.serviceMenuPanel },
  { name: "user menu", button: TID.userMenuButton, panel: TID.userMenuPanel },
] as const;

for (const m of MENUS) {
  test(`reflow 320: ${m.name} panel stays inside the viewport`, async ({ page }) => {
    await gotoApp(page, "/");
    await page.getByTestId(m.button).click();
    await expectInsideViewport(page, page.getByTestId(m.panel));
    await expectNoHorizontalScroll(page);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId(m.panel)).toBeHidden();
  });
}
