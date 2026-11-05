import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { TID, gotoApp, openPalette, openScope, seedTheme, type ThemeName } from "./fixtures";

// Axe runs in Chromium only (plan §4.6); keyboard/scope specs cover all engines.
const PAGES = ["/", "/gallery"] as const;
const THEMES: readonly ThemeName[] = ["light", "dark"];
const WIDTHS = [
  { name: "desktop", width: 1280, height: 800 },
  { name: "narrow", width: 600, height: 900 },
  // SC 1.4.10 reflow: 320 CSS px is 1280 px at 400% zoom (A11Y-002).
  { name: "reflow-320", width: 320, height: 640 },
] as const;

for (const path of PAGES) {
  for (const theme of THEMES) {
    for (const vp of WIDTHS) {
      test(`axe: ${path} ${theme} ${vp.name} has 0 serious/critical`, async ({ page, browserName }, testInfo) => {
        test.skip(browserName !== "chromium", "axe scans run in chromium only");
        await page.setViewportSize({ width: vp.width, height: vp.height });
        await seedTheme(page, theme);
        await gotoApp(page, path);
        await expect(page.locator("html")).toHaveAttribute("data-theme", theme);

        const results = await new AxeBuilder({ page })
          .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
          .analyze();
        await testInfo.attach(`axe-${path.replace(/\//g, "_") || "root"}-${theme}-${vp.name}.json`, {
          body: JSON.stringify(results, null, 2),
          contentType: "application/json",
        });
        const blocking = results.violations
          .filter((v) => v.impact === "serious" || v.impact === "critical")
          .map((v) => ({ id: v.id, impact: v.impact, nodes: v.nodes.map((n) => n.target.join(" ")) }));
        expect(blocking, JSON.stringify(blocking, null, 2)).toEqual([]);
      });
    }
  }
}

// M4.5 chrome states (plan §7 axe list): palette open (results / empty),
// rail collapsed and expanded, scope popover open (wide + compact), both themes.
const STATES = [
  { name: "palette-results", width: 1280, path: "/", act: async (p: Page) => { await openPalette(p); } },
  { name: "palette-empty", width: 1280, path: "/", act: async (p: Page) => {
      await (await openPalette(p)).fill("zzz-no-match");
      await expect(p.getByTestId(TID.paletteEmpty)).toBeVisible();
    } },
  { name: "palette-320", width: 320, path: "/", act: async (p: Page) => { await openPalette(p); } },
  { name: "rail-expanded", width: 1280, path: "/", act: async (p: Page) => {
      await expect(p.locator("#side-nav-region")).toHaveAttribute("data-rail", "expanded");
    } },
  { name: "rail-collapsed", width: 1280, path: "/", act: async (p: Page) => {
      await p.getByTestId(TID.navToggle).click();
      await expect(p.locator("#side-nav-region")).toHaveAttribute("data-rail", "collapsed");
    } },
  { name: "rail-drawer-320", width: 320, path: "/", act: async (p: Page) => {
      await p.getByTestId(TID.navToggle).click();
      await expect(p.locator("#side-nav-region")).toBeVisible();
    } },
  { name: "scope-site-popover", width: 1280, path: "/?site=hq", act: async (p: Page) => { await openScope(p, "site"); } },
  { name: "scope-tenant-popover", width: 1280, path: "/?site=hq", act: async (p: Page) => { await openScope(p, "tenant"); } },
  { name: "scope-popover-320", width: 320, path: "/?site=hq", act: async (p: Page) => { await openScope(p, "site"); } },
] as const;

for (const st of STATES) {
  for (const theme of THEMES) {
    test(`axe: ${st.name} ${theme} has 0 serious/critical`, async ({ page, browserName }, testInfo) => {
      test.skip(browserName !== "chromium", "axe scans run in chromium only");
      await page.setViewportSize({ width: st.width, height: st.width === 320 ? 640 : 800 });
      await seedTheme(page, theme);
      await gotoApp(page, st.path);
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
      await st.act(page);
      const results = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
        .analyze();
      await testInfo.attach(`axe-${st.name}-${theme}.json`, { body: JSON.stringify(results, null, 2), contentType: "application/json" });
      const blocking = results.violations
        .filter((v) => v.impact === "serious" || v.impact === "critical")
        .map((v) => ({ id: v.id, impact: v.impact, nodes: v.nodes.map((n) => n.target.join(" ")) }));
      expect(blocking, JSON.stringify(blocking, null, 2)).toEqual([]);
    });
  }
}
