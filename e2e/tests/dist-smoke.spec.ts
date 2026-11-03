import { expect, test } from "@playwright/test";
import { TID, gotoApp } from "./fixtures";
import { domViolations, selectBudget, watchCsp } from "./csp-helpers";

/**
 * NIST-006: runtime evidence against the shipped release bundle (`dist`, no
 * gallery feature), served by console-gateway on the smoke port. Runs only in
 * the `dist-*` projects (see playwright.config.ts).
 * NIST 800-53: SC-18 (strict CSP, zero violations), CM-7 (no /gallery route).
 */
test("release dist: / loads with zero CSP violations", async ({ page, browserName }) => {
  const webkit = browserName === "webkit";
  const watch = await watchCsp(page, webkit);
  const res = await page.goto("/");
  expect(res?.status()).toBe(200);
  expect(res?.headers()["content-security-policy"]).toBeTruthy();
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  await expect(page.getByTestId(TID.pageHeader)).toContainText("Home");
  expect(await page.locator('[style]:not([style=""])').count()).toBe(0);
  const v = await domViolations(page, webkit);
  expect(v.bad).toEqual([]);
  expect(watch.other).toEqual([]);
  const budget = await selectBudget(page);
  expect(v.allowed).toBeLessThanOrEqual(budget);
  if (!webkit) expect(v.allowed).toBe(0);
  expect(watch.allowedConsole()).toBeLessThanOrEqual(budget);
});

test("release dist: /gallery is NotFound (gallery not compiled in)", async ({ page }) => {
  await gotoApp(page, "/gallery");
  await expect(page.getByText("This page does not exist.")).toBeVisible();
  await expect(page).toHaveTitle("Not found - USG Ops Console");
  await expect(page.getByTestId(TID.openModal)).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Component gallery" })).toHaveCount(0);
});
