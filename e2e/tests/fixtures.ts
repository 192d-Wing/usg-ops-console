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
  // M4.5: service menu and global search were replaced by the rail + palette.
  paletteTrigger: "palette-trigger",
  palette: "command-palette",
  paletteInput: "palette-input",
  paletteList: "palette-list",
  paletteEmpty: "palette-empty",
  paletteStatus: "palette-status",
  scopePicker: "scope-picker",
  /** Single compact "Scope" button below 45rem. */
  scopeTrigger: "scope-trigger",
  scopePopover: "scope-popover",
  railScrim: "rail-scrim",
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

export type ScopeSeg = "site" | "tenant" | "env";

/**
 * M4.5: the scope selects live in popovers that are `hidden` while closed.
 * Open the popover holding `seg` (wide: `scope-<seg>-trigger`; below 45rem:
 * the single `scope-trigger`) and return the select. Idempotent: an already
 * open popover is left open (it stays open after a change, plan Q6).
 */
export async function openScope(page: Page, seg: ScopeSeg): Promise<Locator> {
  const select = page.getByTestId(`scope-${seg}`);
  if (await select.isVisible()) return select;
  const compact = page.getByTestId(TID.scopeTrigger);
  const trigger = (await compact.isVisible()) ? compact : page.getByTestId(`scope-${seg}-trigger`);
  await trigger.click();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  await expect(select).toBeVisible();
  return select;
}

/** Open the command palette from its visible top-bar trigger. */
export async function openPalette(page: Page): Promise<Locator> {
  await page.getByTestId(TID.paletteTrigger).click();
  const input = page.getByTestId(TID.paletteInput);
  await expect(input).toBeFocused();
  return input;
}

/** No horizontal page scroll (SC 1.4.10). */
export async function expectNoHorizontalScroll(page: Page): Promise<void> {
  const { scrollWidth, clientWidth } = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(scrollWidth, `scrollWidth ${scrollWidth} > clientWidth ${clientWidth}`).toBeLessThanOrEqual(clientWidth);
}

/** `panel` is visible and fully inside the viewport horizontally (and not above it). */
export async function expectInsideViewport(page: Page, panel: Locator): Promise<void> {
  await expect(panel).toBeVisible();
  const box = await panel.boundingBox();
  expect(box).not.toBeNull();
  if (!box) return;
  const { w, h } = await page.evaluate(() => ({ w: window.innerWidth, h: window.innerHeight }));
  expect(box.x, `panel left ${box.x}`).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width, `panel right ${box.x + box.width} > ${w}`).toBeLessThanOrEqual(w + 0.5);
  expect(box.y, `panel top ${box.y}`).toBeGreaterThanOrEqual(0);
  expect(box.y, `panel top ${box.y} below viewport ${h}`).toBeLessThan(h);
}
