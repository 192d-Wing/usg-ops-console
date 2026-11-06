import { expect } from "@playwright/test";
import { DCIM } from "../playwright.config";
import { DEMO, TID, openScope } from "./fixtures";
import { KEYCLOAK } from "./auth-helpers";
import { DCIM_BASE, DCIM_FABRICS, dcimFetch, dcimTest as test, openDcim, toolFrame, toolFrameEl, toolFrameHandle } from "./tool-helpers";

// M5 (plan U6): finch embedded in the console under one login. Needs
// DCIM_REPO (e2e/servers/dcim-stack.mjs); otherwise this file is ignored by
// the config, and the skip below is a second guard.
test.skip(!DCIM.enabled, "DCIM_REPO not set (usg-dcim E2E is local-only, brief Q9)");

test.describe("dcim embedded in the console", () => {
  test("dcim_viewer launches DCIM from the rail: tool-origin frame, no second login, console auth mode", async ({ page }) => {
    await page.goto("/");
    const rail = page.getByTestId(TID.sideNav);
    const item = rail.getByTestId("rail-item-tool-dcim");
    await expect(item).toBeVisible();
    await expect(item).toHaveAttribute("href", "/tools/dcim/");
    // Any IdP traffic after this point would be a second login.
    const idp: string[] = [];
    page.on("request", (r) => {
      if (r.url().startsWith(KEYCLOAK)) idp.push(r.url());
    });
    const launched = page.waitForResponse((r) => r.url().endsWith("/api/tools/dcim/launch") && r.request().method() === "POST");
    await item.click();
    expect((await launched).status()).toBe(200);
    await expect(page).toHaveURL(/\/tools\/dcim\/?$/);
    await expect(toolFrameEl(page)).toHaveAttribute("src", new RegExp(`^${DCIM.origin}/`));
    await expect(toolFrameEl(page)).toHaveAttribute("title", /DCIM/);

    const f = await toolFrameHandle(page, DCIM.origin);
    await expect(toolFrame(page).locator("main").first()).toBeVisible({ timeout: 30_000 });
    expect(new URL(f.url()).pathname, "finch never routed to its own /login").not.toMatch(/^(\/t\/dcim)?\/login/);
    await expect(toolFrame(page).locator('input[type="password"]')).toHaveCount(0);
    const me = await dcimFetch(f, `${DCIM_BASE}/api/v1/auth/me`);
    expect(me.status, me.body).toBe(200);
    expect(JSON.parse(me.body)).toMatchObject({ auth_mode: "console", via_token: false });
    expect(idp, "no IdP request after the console login").toEqual([]);
  });

  test("finch chrome is hidden: no top navigation, side navigation, services menu or fabric picker", async ({ page }) => {
    const { fl } = await openDcim(page, DCIM.origin, "/racks");
    await expect(fl.locator("#topnav")).toHaveCount(0);
    await expect(fl.getByText("USG DCIM", { exact: true })).toHaveCount(0);
    await expect(fl.getByRole("button", { name: /fabric scope/i })).toHaveCount(0);
    await expect(fl.getByRole("button", { name: /^services$/i })).toHaveCount(0);
    await expect(fl.locator('nav[aria-label], [class*="side-navigation"]')).toHaveCount(0);
    await expect(fl.getByRole("button", { name: /sign out|log ?out/i })).toHaveCount(0);
    // The console keeps its own chrome.
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
  });

  test("title and breadcrumbs from finch reach the console", async ({ page }) => {
    await openDcim(page, DCIM.origin, "/racks");
    const crumbs = page.getByTestId(TID.breadcrumbs);
    await expect(crumbs).toContainText("DCIM");
    await expect(crumbs).toContainText("Racks");
    await expect(page).toHaveTitle(/Racks.*DCIM.*Console/);
  });

  test("in-tool path changes are reflected in the console URL; console tool nav drives finch", async ({ page }) => {
    const { frame } = await openDcim(page, DCIM.origin, "/racks");
    await expect.poll(() => new URL(frame.url()).pathname).toBe(`${DCIM_BASE}/racks`);
    // Console-side navigation (registry tool nav in the rail) -> finch route.
    const sites = page.getByTestId(TID.sideNav).getByRole("link", { name: /^Sites$/ });
    await sites.click();
    await expect(page).toHaveURL(/\/tools\/dcim\/sites\/?(\?|$)/);
    await expect(page.getByTestId(TID.breadcrumbs)).toContainText("Sites");
    // Tool-side navigation (finch router) -> console URL via pathChanged.
    await frame.evaluate(() => {
      history.pushState({}, "", "/t/dcim/ipam");
      dispatchEvent(new PopStateEvent("popstate"));
    });
    await expect(page).toHaveURL(/\/tools\/dcim\/ipam\/?(\?|$)/);
    await expect(page.getByTestId(TID.breadcrumbs)).toContainText("IPAM");
  });

  test("console site scope selects the finch fabric (matched by slug) without reloading the frame", async ({ page }) => {
    const fabricReqs: string[] = [];
    page.on("request", (r) => {
      if (r.url().startsWith(`${DCIM.origin}${DCIM_BASE}/api/v1/dns/dashboard`)) fabricReqs.push(r.url());
    });
    const { frame, fl } = await openDcim(page, DCIM.origin, "/dns");
    await frame.evaluate(() => ((window as unknown as { __e2eMarker: number }).__e2eMarker = 42));
    await (await openScope(page, "site")).selectOption(DEMO.multiSite);
    await expect(fl.getByText(`fabric: ${DCIM_FABRICS.hq.name}`)).toBeVisible({ timeout: 20_000 });
    await expect.poll(() => fabricReqs.some((u) => u.includes(`fabric_id=${DCIM_FABRICS.hq.id}`))).toBe(true);
    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    await expect(fl.getByText(`fabric: ${DCIM_FABRICS.east.name}`)).toBeVisible({ timeout: 20_000 });
    await expect.poll(() => fabricReqs.some((u) => u.includes(`fabric_id=${DCIM_FABRICS.east.id}`))).toBe(true);
    // A site with no matching fabric slug: empty/global state, never another fabric.
    await (await openScope(page, "site")).selectOption(DEMO.singleTenantSite);
    await expect(fl.getByTestId("scope-unmatched")).toContainText(DEMO.singleTenantSite, { timeout: 20_000 });
    await expect(fl.getByText(/no matching fabric/)).toBeVisible({ timeout: 20_000 });
    await expect(fl.getByText(/all fabrics/)).toHaveCount(0);
    expect(await frame.evaluate(() => (window as unknown as { __e2eMarker?: number }).__e2eMarker), "same document").toBe(42);
  });
});
