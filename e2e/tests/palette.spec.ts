import { expect, test, type Page } from "@playwright/test";
import { TID, gotoApp, openPalette } from "./fixtures";
import { NO_AUTH, loginVia } from "./auth-helpers";
import { HELLO_TID, openTool, toolFrame, toolTest } from "./tool-helpers";

// M4.5 command palette (plan §4.2, §7). Authenticated projects, all engines.

const dialog = (p: Page) => p.getByTestId(TID.palette).getByRole("dialog");
const input = (p: Page) => p.getByTestId(TID.paletteInput);
const opt = (p: Page, id: string) => p.getByTestId(`palette-option-${id}`);

async function activeId(p: Page): Promise<string | null> {
  return input(p).getAttribute("aria-activedescendant");
}

test.describe("open and close", () => {
  for (const combo of ["Control+k", "Meta+k"] as const) {
    test(`${combo} opens from the shell; again closes and restores focus`, async ({ page }) => {
      await gotoApp(page, "/");
      const theme = page.getByTestId(TID.themeToggle);
      await theme.focus();
      await page.keyboard.press(combo);
      await expect(dialog(page)).toBeVisible();
      await expect(dialog(page)).toHaveAttribute("aria-modal", "true");
      await expect(dialog(page)).toHaveAccessibleName("Command palette");
      await expect(input(page)).toBeFocused();
      await expect(page.locator("#app-chrome")).toHaveAttribute("inert", "");
      await page.keyboard.press(combo);
      await expect(page.getByTestId(TID.palette)).toBeHidden();
      await expect(theme).toBeFocused();
      await expect(page.locator("#app-chrome")).not.toHaveAttribute("inert", "");
    });
  }

  test("visible trigger opens it; scrim click closes; trigger has shortcut semantics", async ({ page }) => {
    await gotoApp(page, "/");
    const trigger = page.getByTestId(TID.paletteTrigger);
    await expect(trigger).toHaveAttribute("aria-haspopup", "dialog");
    await expect(trigger).toHaveAttribute("aria-keyshortcuts", "Meta+K Control+K");
    await openPalette(page);
    await page.mouse.click(5, (page.viewportSize()?.height ?? 600) - 5);
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    await expect(trigger).toBeFocused();
  });

  test("shortcut is ignored while a modal is open", async ({ page }) => {
    await gotoApp(page, "/gallery");
    const opener = page.getByTestId(TID.openModal);
    await opener.scrollIntoViewIfNeeded();
    await opener.click();
    const modal = page.getByTestId(TID.modal).getByRole("dialog");
    await expect(modal).toBeVisible();
    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.press("Control+k");
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    await expect.poll(() => modal.evaluate((d) => d.contains(document.activeElement))).toBe(true);
    await page.keyboard.press("Escape");
    await expect(modal).toBeHidden();
  });
});

test.describe("keyboard (APG combobox + listbox)", () => {
  test("ARIA wiring, arrows wrap, Home/End, Tab trapped, Escape returns focus", async ({ page }) => {
    await gotoApp(page, "/");
    const trigger = page.getByTestId(TID.paletteTrigger);
    await trigger.focus();
    await page.keyboard.press("ControlOrMeta+k");
    const box = input(page);
    await expect(box).toBeFocused();
    await expect(box).toHaveAttribute("role", "combobox");
    await expect(box).toHaveAttribute("aria-controls", "palette-list");
    await expect(box).toHaveAttribute("aria-autocomplete", "list");
    await expect(box).toHaveAttribute("autocomplete", "off");
    await expect(box).toHaveAttribute("spellcheck", "false");
    const list = page.getByTestId(TID.paletteList);
    await expect(list).toHaveAttribute("id", "palette-list");
    await expect(list).toHaveAttribute("role", "listbox");

    const options = list.locator('[role="option"]:not([aria-disabled="true"])');
    const n = await options.count();
    expect(n).toBeGreaterThan(2);
    const ids = await options.evaluateAll((os) => os.map((o) => o.id));
    expect(ids.every((i) => i.length > 0)).toBe(true);

    await page.keyboard.press("ArrowDown");
    const first = await activeId(page);
    expect(first).toBeTruthy();
    await expect(page.locator(`[id="${first}"]`)).toHaveAttribute("aria-selected", "true");
    await page.keyboard.press("ArrowDown");
    const second = await activeId(page);
    expect(second).not.toBe(first);
    await expect(page.locator(`[id="${first}"]`)).toHaveAttribute("aria-selected", "false");
    await page.keyboard.press("End");
    expect(await activeId(page)).toBe(ids[ids.length - 1]);
    await page.keyboard.press("ArrowDown"); // wraps
    expect(await activeId(page)).toBe(ids[0]);
    await page.keyboard.press("ArrowUp"); // wraps back
    expect(await activeId(page)).toBe(ids[ids.length - 1]);
    await page.keyboard.press("Home");
    expect(await activeId(page)).toBe(ids[0]);

    for (let i = 0; i < 4; i++) {
      await page.keyboard.press("Tab");
      await expect(box).toBeFocused();
    }
    await page.keyboard.press("Shift+Tab");
    await expect(box).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    await expect(trigger).toBeFocused();
  });

  test("typing filters and announces a count; Enter activates the active option (navigates)", async ({ page }) => {
    await gotoApp(page, "/");
    await openPalette(page);
    await input(page).fill("gallery");
    await expect(opt(page, "nav-gallery")).toBeVisible();
    await expect(page.getByTestId(TID.paletteStatus)).toHaveText(/\d+ results?/);
    await page.keyboard.press("ArrowDown");
    expect(await activeId(page)).toBe(await opt(page, "nav-gallery").getAttribute("id"));
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/gallery$/);
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    // The query was not carried into the URL.
    expect(page.url()).not.toContain("q=");
  });

  test("Enter with no active option runs the first match; options are >= 24x24 targets", async ({ page }) => {
    await gotoApp(page, "/gallery");
    await openPalette(page);
    const all = page.getByTestId(TID.paletteList).locator('[role="option"]');
    for (const b of await all.evaluateAll((os) => os.map((o) => o.getBoundingClientRect().toJSON()))) {
      expect(b.height, "option height").toBeGreaterThanOrEqual(24);
      expect(b.width, "option width").toBeGreaterThanOrEqual(24);
    }
    await input(page).fill("home");
    await expect(opt(page, "nav-home")).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/(\?.*)?$/);
  });
});

test.describe("sources", () => {
  test("permitted tools listed, denied absent; selecting a tool launches it", async ({ page }) => {
    await gotoApp(page, "/");
    await expect(page.getByTestId("rail-item-tool-hello")).toBeVisible(); // tools loaded
    await openPalette(page);
    await expect(opt(page, "tool-hello")).toBeVisible();
    await expect(opt(page, "tool-denied")).toHaveCount(0);
    await expect(opt(page, "tools-loading")).toHaveCount(0);
    await input(page).fill("hello");
    const launched = page.waitForResponse((r) => r.url().endsWith("/api/tools/hello/launch") && r.request().method() === "POST");
    await opt(page, "tool-hello").click();
    expect((await launched).status()).toBe(200);
    await expect(page).toHaveURL(/\/tools\/hello\/?$/);
  });

  test("scope actions open the matching popover with focus on its select; tenant listed only once a site is set", async ({ page }) => {
    await gotoApp(page, "/");
    await openPalette(page);
    await expect(opt(page, "scope-site")).toBeVisible();
    await expect(opt(page, "scope-tenant")).toHaveCount(0);
    await expect(opt(page, "scope-env")).toHaveCount(0);
    await opt(page, "scope-site").click();
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    await expect(page.getByTestId("scope-site-popover")).toBeVisible();
    await expect(page.getByTestId(TID.site)).toBeFocused();
    await page.getByTestId(TID.site).selectOption("hq");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("scope-site-trigger")).toBeFocused();
    await openPalette(page);
    await expect(opt(page, "scope-tenant")).toBeVisible();
    await expect(opt(page, "scope-env")).toHaveCount(0);
  });

  test("theme actions set data-theme; current theme is hinted", async ({ page }) => {
    await gotoApp(page, "/");
    for (const t of ["dark", "light"] as const) {
      await openPalette(page);
      await input(page).fill(`theme: ${t}`);
      await opt(page, `theme-${t}`).click();
      await expect(page.locator("html")).toHaveAttribute("data-theme", t);
      await openPalette(page);
      await expect(opt(page, `theme-${t}`)).toContainText("Current");
      await page.keyboard.press("Escape");
    }
  });

  test("Sign out from the palette ends the session (own context)", async ({ browser, baseURL }) => {
    const ctx = await browser.newContext({ storageState: NO_AUTH, ignoreHTTPSErrors: true, baseURL: baseURL! });
    const page = await ctx.newPage();
    await loginVia(page, baseURL!, "/");
    await expect(page.getByTestId("user-menu-loading")).toHaveCount(0);
    await openPalette(page);
    await input(page).fill("sign out");
    const post = page.waitForRequest((r) => r.method() === "POST" && /logout|sign-?out/.test(new URL(r.url()).pathname));
    await opt(page, "sign-out").click();
    const req = await post;
    expect(new URL(req.url()).pathname).toBe("/logout");
    expect(req.headers()["x-csrf-token"]).toBeTruthy();
    await page.waitForURL(`${baseURL}/logged-out`);
    await ctx.close();
  });
});

toolTest.describe("palette and the tool frame", () => {
  toolTest("theme from the palette reaches the tool (ThemeChanged); shortcut inside the tool frame does not open the palette", async ({ page }) => {
    const f = await openTool(page, "hello", "/");
    const visits = await f.getByTestId(HELLO_TID.visits).textContent();
    const cur = (await page.locator("html").getAttribute("data-theme")) ?? "light";
    const next = cur === "dark" ? "light" : "dark";
    await openPalette(page);
    await input(page).fill(`theme: ${next}`);
    await opt(page, `theme-${next}`).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", next);
    await expect(f.getByTestId(HELLO_TID.theme)).toHaveText(next);
    await expect(f.getByTestId(HELLO_TID.visits)).toHaveText(visits ?? "");

    // Focus inside the tool document, then press the combo: stays with the tool.
    await f.getByTestId(HELLO_TID.counterInc).focus();
    await page.keyboard.press("Control+k");
    await page.keyboard.press("Meta+k");
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    await expect(toolFrame(page).getByTestId(HELLO_TID.counterInc)).toBeFocused();
  });
});

test.describe("reduced motion", () => {
  test.use({ reducedMotion: "reduce" });
  test("palette and rail have no transition under prefers-reduced-motion", async ({ page }) => {
    await gotoApp(page, "/");
    await openPalette(page);
    const durs = await page.evaluate(() =>
      [".cui-palette__dialog", ".cui-palette", "#side-nav-region", ".cui-rail"].flatMap((s) => {
        const el = document.querySelector(s);
        return el ? [getComputedStyle(el).transitionDuration] : [];
      }),
    );
    expect(durs.length).toBeGreaterThan(0);
    for (const d of durs) expect(d.split(",").every((x) => parseFloat(x) === 0), `transition ${d}`).toBe(true);
  });
});

// Fix round 2: A11Y-006 (tools Retry row) and the SEC-001 invariant (every
// select closes the palette; chrome never left live under an open palette).
test.describe("tools unavailable (A11Y-006, SEC-001)", () => {
  const chrome = (p: Page) => p.locator("#app-chrome");

  test("/api/tools 500: Retry row is listed, selecting it closes the palette and reloads tools", async ({ page }) => {
    let fail = true;
    await page.route("**/api/tools", (r) =>
      fail ? r.fulfill({ status: 500, contentType: "application/json", body: '{"error":"x"}' }) : r.fallback(),
    );
    const first = page.waitForResponse((r) => new URL(r.url()).pathname === "/api/tools");
    await gotoApp(page, "/");
    expect((await first).status()).toBe(500);
    await openPalette(page);
    const retry = opt(page, "tools-retry");
    await expect(retry).toBeVisible();
    await expect(retry).toContainText("Tools unavailable");
    await expect(retry).not.toHaveAttribute("aria-disabled", "true");
    await expect(opt(page, "tool-hello")).toHaveCount(0);
    fail = false;
    const again = page.waitForResponse((r) => new URL(r.url()).pathname === "/api/tools");
    await retry.click();
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    await expect(chrome(page)).not.toHaveAttribute("inert", "");
    expect((await again).status()).toBe(200);
    await expect(page.getByTestId("rail-item-tool-hello")).toBeVisible();
    await openPalette(page);
    await expect(opt(page, "tool-hello")).toBeVisible();
    await expect(opt(page, "tools-retry")).toHaveCount(0);
  });

  test("Retry by keyboard; tools then Loading: loading row is inert-safe (not selectable, chrome stays inert)", async ({ page }) => {
    let mode: "fail" | "hang" = "fail";
    const held: Array<() => Promise<void>> = [];
    await page.route("**/api/tools", (r) => {
      if (mode === "fail") return r.fulfill({ status: 500, contentType: "application/json", body: "{}" });
      held.push(() => r.fallback());
      return undefined;
    });
    await gotoApp(page, "/");
    await openPalette(page);
    await input(page).fill("retry");
    await expect(opt(page, "tools-retry")).toBeVisible();
    mode = "hang";
    await page.keyboard.press("Enter");
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    await expect(chrome(page)).not.toHaveAttribute("inert", "");
    await expect.poll(() => held.length).toBeGreaterThan(0);
    await openPalette(page);
    const loading = opt(page, "tools-loading");
    await expect(loading).toBeVisible();
    await expect(loading).toHaveAttribute("aria-disabled", "true");
    // Playwright will not click an aria-disabled option, so use the keyboard:
    // the only match is the disabled row, and Enter must not select it.
    await input(page).fill("tools are loading");
    await expect(loading).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(dialog(page)).toBeVisible();
    await expect(chrome(page)).toHaveAttribute("inert", "");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId(TID.palette)).toBeHidden();
    await expect(chrome(page)).not.toHaveAttribute("inert", "");
    for (const h of held) await h();
  });
});
