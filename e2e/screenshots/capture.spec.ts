import { mkdirSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { loginVia } from "../tests/auth-helpers";

const LABEL = process.env.SHOT_LABEL === "before" ? "before" : "after";
const OUT = `${process.env.SHOT_OUT ?? "../test-results/screenshots"}/${LABEL}`;
mkdirSync(OUT, { recursive: true });

const THEMES = ["light", "dark"] as const;
const SIZES = [
  { name: "1280x800", width: 1280, height: 800 },
  { name: "320x640", width: 320, height: 640 },
] as const;

async function ready(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await expect(page.getByTestId("top-navigation")).toBeVisible();
  await expect(page.getByTestId("user-menu-loading")).toHaveCount(0);
  if (path.startsWith("/tools/")) {
    const f = page.frameLocator('[data-testid="tool-frame"]');
    await expect(f.getByTestId("hello-connection")).toHaveAttribute("data-connected", "true", { timeout: 20_000 });
  }
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);
}

for (const theme of THEMES) {
  test.describe(theme, () => {
    test.use({ colorScheme: theme });
    test(`${LABEL} ${theme}`, async ({ page, baseURL }) => {
      await page.addInitScript((t: string) => localStorage.setItem("cui-theme", t), theme);
      await loginVia(page, baseURL!, "/");
      for (const s of SIZES) {
        await page.setViewportSize({ width: s.width, height: s.height });
        await ready(page, "/");
        await page.screenshot({ path: `${OUT}/home-${theme}-${s.name}.png` });
        await ready(page, "/tools/hello/");
        await page.screenshot({ path: `${OUT}/tool-${theme}-${s.name}.png` });
        if (LABEL === "after") {
          await ready(page, "/");
          await page.getByTestId("palette-trigger").click();
          await expect(page.getByTestId("palette-input")).toBeFocused();
          await page.waitForTimeout(200);
          await page.screenshot({ path: `${OUT}/palette-${theme}-${s.name}.png` });
          await page.keyboard.press("Escape");
        }
      }
      if (LABEL === "after") {
        await page.setViewportSize({ width: 1280, height: 800 });
        await ready(page, "/tools/hello/");
        await page.getByTestId("nav-toggle").click();
        await expect(page.locator("#side-nav-region")).toHaveAttribute("data-rail", "collapsed");
        await page.waitForTimeout(200);
        await page.screenshot({ path: `${OUT}/rail-collapsed-${theme}-1280x800.png` });
        await page.getByTestId("nav-toggle").click(); // restore the stored default
      }
    });
  });
}
