import { expect, test, type Page } from "@playwright/test";
import { DEMO, TID, gotoApp, query } from "./fixtures";

/** WebKit (macOS-style) only tabs to links with Alt held. */
function tabKey(browserName: string): string {
  return browserName === "webkit" ? "Alt+Tab" : "Tab";
}

async function firstTab(page: Page, browserName: string): Promise<void> {
  await page.locator("body").focus();
  await page.keyboard.press(tabKey(browserName));
}

test.describe("skip link", () => {
  test("first Tab reveals skip link; Enter moves focus to main", async ({ page, browserName }) => {
    await gotoApp(page, "/");
    await firstTab(page, browserName);
    const skip = page.locator(".skip-link");
    await expect(skip).toBeFocused();
    await expect(skip).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(page.locator("#main-content")).toBeFocused();
  });
});

test.describe("side nav at narrow width", () => {
  test.use({ viewport: { width: 600, height: 900 } });

  test("toggle expands, focuses first link, Escape collapses and restores focus", async ({ page }) => {
    await gotoApp(page, "/");
    const toggle = page.getByTestId(TID.navToggle);
    const region = page.locator("#side-nav-region");
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await expect(toggle).toHaveAttribute("aria-controls", "side-nav-region");
    await expect(region).toBeHidden();

    await toggle.focus();
    await page.keyboard.press("Enter");
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(region).toBeVisible();
    await expect(region.getByRole("link").first()).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await expect(region).toBeHidden();
    await expect(toggle).toBeFocused();
  });

  test("toggle is hidden and nav shown at >= 45rem", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await gotoApp(page, "/");
    await expect(page.getByTestId(TID.navToggle)).toBeHidden();
    await expect(page.locator("#side-nav-region")).toBeVisible();
  });
});

test.describe("service menu (disclosure)", () => {
  test("open, arrows wrap, Escape returns focus, Tab closes", async ({ page, browserName }) => {
    await gotoApp(page, "/");
    const button = page.getByTestId(TID.serviceMenuButton);
    const panel = page.getByTestId(TID.serviceMenuPanel);
    await expect(button).toHaveAttribute("aria-expanded", "false");
    await expect(panel).toBeHidden();
    const controls = await button.getAttribute("aria-controls");
    expect(controls).toBeTruthy();
    await expect(panel).toHaveAttribute("id", controls ?? "");

    await button.focus();
    await page.keyboard.press("Enter");
    await expect(button).toHaveAttribute("aria-expanded", "true");
    await expect(panel).toBeVisible();
    const items = panel.locator("a, button");
    const count = await items.count();
    expect(count).toBeGreaterThan(1);
    await expect(items.first()).toBeFocused();

    await page.keyboard.press("ArrowDown");
    await expect(items.nth(1)).toBeFocused();
    await page.keyboard.press("ArrowUp");
    await page.keyboard.press("ArrowUp");
    await expect(items.last()).toBeFocused(); // wraps
    await page.keyboard.press("Home");
    await expect(items.first()).toBeFocused();
    await page.keyboard.press("End");
    await expect(items.last()).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(panel).toBeHidden();
    await expect(button).toHaveAttribute("aria-expanded", "false");
    await expect(button).toBeFocused();

    // ArrowDown on the closed button opens it (WU1).
    await page.keyboard.press("ArrowDown");
    await expect(panel).toBeVisible();
    await expect(items.first()).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(button).toBeFocused();

    // Tab out of the open panel closes it. Focus moves to a real element so
    // relatedTarget is set; null-relatedTarget blur intentionally keeps it
    // open (Safari click behaviour), which is not asserted here.
    await page.keyboard.press("Enter");
    await expect(panel).toBeVisible();
    await page.keyboard.press("End");
    await page.keyboard.press(tabKey(browserName));
    await expect(panel).toBeHidden();
    await expect(button).toHaveAttribute("aria-expanded", "false");
  });

  test("outside click closes", async ({ page }) => {
    await gotoApp(page, "/");
    await page.getByTestId(TID.serviceMenuButton).click();
    await expect(page.getByTestId(TID.serviceMenuPanel)).toBeVisible();
    // Click empty space away from the panel (bottom-left of the viewport).
    const vp = page.viewportSize() ?? { width: 1280, height: 720 };
    await page.mouse.click(vp.width - 10, vp.height - 10);
    await expect(page.getByTestId(TID.serviceMenuPanel)).toBeHidden();
  });
});

test.describe("user menu and search", () => {
  test("Sign out shows M3 info flash", async ({ page }) => {
    await gotoApp(page, "/");
    await page.getByTestId(TID.userMenuButton).click();
    await page.getByTestId(TID.userMenuPanel).getByRole("button", { name: "Sign out" }).click();
    await expect(page.getByTestId(TID.flashbar)).toContainText("Sign-in arrives in M3");
  });

  test("search submit shows fixed info flash and never echoes the query", async ({ page }) => {
    await gotoApp(page, "/");
    const input = page.getByTestId(TID.search).getByRole("searchbox");
    await input.fill("needle-xyz<b>");
    await input.press("Enter");
    await expect(page.getByTestId(TID.flashbar)).toContainText("Search is not available yet");
    await expect(page.locator("body")).not.toContainText("needle-xyz");
    expect(page.url()).not.toContain("needle");
    await expect(input).toHaveAttribute("maxlength", "256");
  });
});

test.describe("scope picker keyboard", () => {
  test("choose site via keyboard; URL updates, reload keeps it, Back restores", async ({ page }) => {
    await gotoApp(page, "/");
    const site = page.getByTestId(TID.site);
    await site.focus();
    await expect(site).toBeFocused();
    await site.selectOption(DEMO.multiSite);
    await expect.poll(() => query(page)["site"]).toBe(DEMO.multiSite);

    await page.reload();
    await expect(page.getByTestId(TID.site)).toHaveValue(DEMO.multiSite);

    await page.goBack();
    await expect.poll(() => query(page)["site"]).toBeUndefined();
    await expect(page.getByTestId(TID.site)).toHaveValue("");
  });
});

test.describe("modal focus restore (ORCH-1 regression)", () => {
  test("Escape and Cancel both return focus to the opener; chrome inert while open", async ({ page }) => {
    await gotoApp(page, "/gallery");
    const opener = page.getByTestId(TID.openModal);
    const dialog = page.getByTestId(TID.modal).getByRole("dialog");

    await opener.scrollIntoViewIfNeeded();
    await opener.focus();
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute("aria-modal", "true");
    // Focus is inside the dialog, and the chrome is inert.
    await expect.poll(() => dialog.evaluate((d) => d.contains(document.activeElement))).toBe(true);
    await expect(page.locator("#app-chrome")).toHaveAttribute("inert", "");

    // Tab stays trapped inside the dialog.
    for (let i = 0; i < 6; i++) {
      await page.keyboard.press("Tab");
      expect(await dialog.evaluate((d) => d.contains(document.activeElement))).toBe(true);
    }

    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(opener).toBeFocused();
    await expect(page.locator("#app-chrome")).not.toHaveAttribute("inert", "");

    // Second open/close via the Cancel button (the original ORCH-1 path).
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
    await expect(opener).toBeFocused();
  });
});
