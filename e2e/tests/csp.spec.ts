import { expect, test } from "@playwright/test";
import { DEMO, TID, gotoApp, openScope } from "./fixtures";
import { domViolations, selectBudget, watchCsp } from "./csp-helpers";

test("no CSP violations and no non-empty [style] attributes across interactions", async ({ page, browserName }) => {
  const webkit = browserName === "webkit";
  const watch = await watchCsp(page, webkit);
  const res = await page.goto("/");
  expect(res?.headers()["content-security-policy"]).toBeTruthy();
  await expect(page.getByTestId(TID.topNav)).toBeVisible();

  // M4.5: palette (replaces the service menu) and rail collapse.
  await page.getByTestId(TID.paletteTrigger).click();
  await page.getByTestId(TID.paletteInput).fill("hello");
  await page.keyboard.press("Escape");
  await page.getByTestId(TID.navToggle).click();
  await page.getByTestId(TID.navToggle).click();
  await page.getByTestId(TID.userMenuButton).click();
  await page.keyboard.press("Escape");
  await (await openScope(page, "site")).selectOption(DEMO.multiSite);
  await page.keyboard.press("Escape");
  await page.getByTestId(TID.themeToggle).click();
  await page.getByTestId(TID.themeToggle).click();
  expect(await page.locator('[style]:not([style=""])').count()).toBe(0);
  const home = await domViolations(page, webkit);
  expect(home.bad).toEqual([]);
  const homeBudget = await selectBudget(page);
  expect(home.allowed).toBeLessThanOrEqual(homeBudget);
  if (!webkit) expect(home.allowed).toBe(0);

  await gotoApp(page, "/gallery");
  await page.getByTestId(TID.openModal).click();
  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 600, height: 900 });
  await page.getByTestId(TID.navToggle).click();
  await page.keyboard.press("Escape");
  expect(await page.locator('[style]:not([style=""])').count()).toBe(0);
  // gotoApp is a fresh document, so the counter restarted.
  const gallery = await domViolations(page, webkit);
  expect(gallery.bad).toEqual([]);
  const galleryBudget = await selectBudget(page);
  expect(gallery.allowed).toBeLessThanOrEqual(galleryBudget);
  expect(watch.other).toEqual([]);
  // One console line per allowed violation, across both loads.
  expect(watch.allowedConsole()).toBeLessThanOrEqual(homeBudget + galleryBudget);
  test.info().annotations.push({
    type: "webkit-select-csp-noise",
    description: `home=${home.allowed}/${homeBudget} gallery=${gallery.allowed}/${galleryBudget}`,
  });
});
