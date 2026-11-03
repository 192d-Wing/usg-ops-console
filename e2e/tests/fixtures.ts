import { expect, type Page, type Locator } from "@playwright/test";

/**
 * Demo scope data as compiled into console-shell/src/demo.rs (cascading
 * site -> tenant -> env, brief "Cascading scope rules"). Update here only if
 * demo.rs changes; every spec reads these constants.
 */
export const DEMO = {
  /** Site with several tenants. */
  multiSite: "hq",
  /** A second site that shares `sharedTenant` with `multiSite`. */
  otherSite: "east",
  /** Site with exactly one tenant. */
  singleTenantSite: "west",
  /** Tenant present at both `multiSite` and `otherSite`. */
  sharedTenant: "ops",
  /** Tenant present at `multiSite` but NOT at `otherSite`. */
  multiOnlyTenant: "intel",
  /** Env valid for (multiSite, sharedTenant) and (otherSite, sharedTenant). */
  sharedEnv: "prod",
  /** Env valid for (multiSite, sharedTenant) but NOT for (otherSite, sharedTenant). */
  multiOnlyEnv: "dev",
} as const;

export type ThemeName = "light" | "dark";

export const TID = {
  topNav: "top-navigation",
  serviceMenu: "service-menu",
  serviceMenuButton: "service-menu-button",
  serviceMenuPanel: "service-menu-panel",
  search: "global-search",
  site: "scope-site",
  tenant: "scope-tenant",
  env: "scope-env",
  userMenuButton: "user-menu-button",
  userMenuPanel: "user-menu-panel",
  themeToggle: "theme-toggle",
  navToggle: "nav-toggle",
  sideNav: "side-navigation",
  breadcrumbs: "breadcrumbs",
  pageHeader: "page-header",
  flashbar: "shell-flashbar",
  openModal: "open-modal",
  modal: "modal-demo",
} as const;

/** Seed the theme preference before any app script runs. */
export async function seedTheme(page: Page, theme: ThemeName): Promise<void> {
  await page.addInitScript((t: string) => {
    window.localStorage.setItem("cui-theme", t);
  }, theme);
}

/** Navigate and wait for the Yew app to mount (banner present). */
export async function gotoApp(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
}

export function flashbar(page: Page): Locator {
  return page.getByTestId(TID.flashbar);
}

/** Current query string parsed into a plain record (for URL assertions). */
export function query(page: Page): Record<string, string> {
  const u = new URL(page.url());
  const out: Record<string, string> = {};
  u.searchParams.forEach((v, k) => {
    out[k] = v;
  });
  return out;
}

/** The polite live region announcing cascade resets. */
export function politeRegion(page: Page): Locator {
  return page.getByTestId("scope-announcer");
}

/**
 * Press Tab until `target` is focused (bounded). Webkit on macOS does not tab
 * to links by default, so callers using links should use `focusVia`.
 */
export async function tabUntilFocused(page: Page, target: Locator, max = 40): Promise<void> {
  for (let i = 0; i < max; i++) {
    if (await target.evaluate((el) => el === document.activeElement)) return;
    await page.keyboard.press("Tab");
  }
  await expect(target).toBeFocused();
}
