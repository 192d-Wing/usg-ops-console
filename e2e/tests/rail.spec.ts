import { expect, test, type Page } from "@playwright/test";
import { TID, gotoApp } from "./fixtures";
import { HELLO_TID, openTool, toolFrameEl, toolTest } from "./tool-helpers";

// M4.5 icon rail (plan §4.1, §7). Desktop width unless noted.
const KEY = "usg-console.rail";
const region = (p: Page) => p.locator("#side-nav-region");
const toggle = (p: Page) => p.getByTestId(TID.navToggle);

test.use({ viewport: { width: 1280, height: 800 } });

test("default is expanded; collapse persists across reload; keyboard toggles; names survive collapse", async ({ page }) => {
  await gotoApp(page, "/");
  await expect(region(page)).toHaveAttribute("data-rail", "expanded");
  const home = region(page).getByTestId("rail-item-home");
  const gallery = region(page).getByTestId("rail-item-gallery");
  await expect(home).toHaveAccessibleName("Home");
  await expect(gallery).toHaveAccessibleName("Component gallery");

  await toggle(page).focus();
  await page.keyboard.press("Enter");
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");
  await expect(region(page)).toHaveAttribute("data-rail", "collapsed");
  expect(await page.evaluate((k) => localStorage.getItem(k), KEY)).toBe("collapsed");

  await page.reload();
  await expect(region(page)).toHaveAttribute("data-rail", "collapsed");
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");
  await expect(toggle(page)).toHaveAccessibleName("Expand navigation");
  // Collapsed: icon only, but each link keeps its text name (not aria-label) and a tooltip label.
  for (const [loc, name] of [[home, "Home"], [gallery, "Component gallery"]] as const) {
    await expect(loc).toBeVisible();
    await expect(loc).toHaveAccessibleName(name);
    await expect(loc).not.toHaveAttribute("aria-label", /.*/);
    await expect(loc).toHaveAttribute("data-label", name);
    await expect(region(page).getByRole("link", { name, exact: true })).toBeVisible();
    const b = await loc.boundingBox();
    expect(b && b.width >= 24 && b.height >= 24, `target ${JSON.stringify(b)}`).toBe(true);
  }
  // Collapsed is narrower than expanded.
  const collapsedW = (await region(page).boundingBox())!.width;
  await toggle(page).click();
  await expect(region(page)).toHaveAttribute("data-rail", "expanded");
  expect((await region(page).boundingBox())!.width).toBeGreaterThan(collapsedW);
  expect(await page.evaluate((k) => localStorage.getItem(k), KEY)).toBe("expanded");
});

test("collapsed rail link is keyboard reachable and follows", async ({ page }) => {
  await page.addInitScript((k) => localStorage.setItem(k, "collapsed"), KEY);
  await gotoApp(page, "/");
  await expect(region(page)).toHaveAttribute("data-rail", "collapsed");
  const gallery = region(page).getByTestId("rail-item-gallery");
  await gallery.focus();
  await expect(gallery).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/gallery/);
  await expect(gallery).toHaveAttribute("aria-current", "page");
});

for (const bad of ["COLLAPSED", "true", "<b>x</b>", ""]) {
  test(`invalid stored value ${JSON.stringify(bad)} falls back to expanded`, async ({ page }) => {
    await page.addInitScript(([k, v]) => localStorage.setItem(k!, v!), [KEY, bad]);
    await gotoApp(page, "/");
    await expect(region(page)).toHaveAttribute("data-rail", "expanded");
    await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
  });
}

test.describe("narrow", () => {
  test.use({ viewport: { width: 600, height: 900 } });
  test("stored collapsed is ignored below 45rem: rail hidden, drawer opens with full labels", async ({ page }) => {
    await page.addInitScript((k) => localStorage.setItem(k, "collapsed"), KEY);
    await gotoApp(page, "/");
    await expect(region(page)).toBeHidden();
    await expect(region(page)).not.toHaveAttribute("data-rail", "collapsed");
    await toggle(page).click();
    await expect(region(page)).toBeVisible();
    await expect(region(page).getByTestId("rail-item-home")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(region(page)).toBeHidden();
    await expect(toggle(page)).toBeFocused();
  });
});

test("forced-colors spot check: focus outline visible, icons use currentColor", async ({ page, browserName }) => {
  test.skip(browserName !== "chromium", "forced-colors emulation is Chromium-only in Playwright");
  await page.emulateMedia({ forcedColors: "active" });
  await gotoApp(page, "/");
  expect(await page.evaluate(() => matchMedia("(forced-colors: active)").matches)).toBe(true);
  const home = region(page).getByTestId("rail-item-home");
  await home.focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Tab"); // keyboard focus => :focus-visible
  const style = await page.evaluate(() => {
    const a = document.activeElement as HTMLElement;
    const cs = getComputedStyle(a);
    const svg = a.querySelector("svg");
    return {
      testid: a.dataset["testid"],
      outlineStyle: cs.outlineStyle,
      outlineWidth: parseFloat(cs.outlineWidth),
      svgStroke: svg ? getComputedStyle(svg).stroke : null,
      svgColor: svg ? getComputedStyle(svg).color : null,
    };
  });
  expect(style.testid).toBe("rail-item-home");
  expect(style.outlineStyle).not.toBe("none");
  expect(style.outlineWidth).toBeGreaterThanOrEqual(1);
  // Icons are stroke glyphs (fill none): the stroke must track currentColor.
  expect(style.svgColor, JSON.stringify(style)).toBeTruthy();
  expect(style.svgStroke, JSON.stringify(style)).toBe(style.svgColor);
  // Palette border is visible in forced colours.
  await page.getByTestId(TID.paletteTrigger).click();
  const border = await page
    .getByTestId(TID.palette)
    .getByRole("dialog")
    .evaluate((d) => {
      const cs = getComputedStyle(d);
      return { s: cs.borderTopStyle, w: parseFloat(cs.borderTopWidth) };
    });
  expect(border.s).not.toBe("none");
  expect(border.w).toBeGreaterThanOrEqual(1);
});

toolTest.describe("tool frame", () => {
  toolTest.use({ viewport: { width: 1280, height: 800 } });
  toolTest("rail toggle resizes without reloading the tool (counter and visits survive)", async ({ page }) => {
    const f = await openTool(page, "hello", "/");
    await f.getByTestId(HELLO_TID.counterInc).click();
    await expect(f.getByTestId(HELLO_TID.counterValue)).toHaveText(/Count: [1-9]\d*/);
    const counter = await f.getByTestId(HELLO_TID.counterValue).textContent();
    const visits = await f.getByTestId(HELLO_TID.visits).textContent();
    const frame = toolFrameEl(page);
    const w0 = (await frame.boundingBox())!.width;
    await toggle(page).click();
    await expect(region(page)).toHaveAttribute("data-rail", "collapsed");
    await expect.poll(async () => (await frame.boundingBox())!.width).toBeGreaterThan(w0);
    await expect(f.getByTestId(HELLO_TID.counterValue)).toHaveText(counter ?? "");
    await expect(f.getByTestId(HELLO_TID.visits)).toHaveText(visits ?? "");
    await toggle(page).click();
    await expect(region(page)).toHaveAttribute("data-rail", "expanded");
    await expect(f.getByTestId(HELLO_TID.visits)).toHaveText(visits ?? "");
  });
});

// Fix round 2 (A11Y-001/005, WCAG 1.4.13): the collapsed tooltip is an
// aria-hidden descendant of the link, dismissible with Escape without moving
// focus or pointer, and hoverable (pointer onto the tip keeps it).
test.describe("collapsed tooltip (WCAG 1.4.13)", () => {
  const tip = (p: Page, tid: string) => region(p).getByTestId(tid).locator(".cui-rail__tip");
  const shown = (l: ReturnType<typeof tip>) =>
    l.evaluate((el) => {
      const cs = getComputedStyle(el);
      return cs.visibility === "visible" && cs.display !== "none";
    });

  test.beforeEach(async ({ page }) => {
    await page.addInitScript((k) => localStorage.setItem(k, "collapsed"), KEY);
    await page.emulateMedia({ reducedMotion: "reduce" });
    await gotoApp(page, "/");
    await expect(region(page)).toHaveAttribute("data-rail", "collapsed");
  });

  test("accessible names are exactly the label while tips render; tips are aria-hidden", async ({ page }) => {
    for (const [tid, name] of [["rail-item-home", "Home"], ["rail-item-gallery", "Component gallery"]] as const) {
      const link = region(page).getByTestId(tid);
      await link.hover();
      await expect.poll(() => shown(tip(page, tid))).toBe(true);
      await expect(tip(page, tid)).toHaveAttribute("aria-hidden", "true");
      await expect(tip(page, tid)).toHaveText(name);
      await expect(link).toHaveAccessibleName(name); // exact match, not "Home Home"
      await expect(region(page).getByRole("link", { name, exact: true })).toHaveCount(1);
    }
  });

  test("Escape dismisses the hover tip without moving focus or pointer; mouseleave re-arms", async ({ page }) => {
    const theme = page.getByTestId(TID.themeToggle);
    await theme.focus();
    const home = region(page).getByTestId("rail-item-home");
    const b = (await home.boundingBox())!;
    await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
    await expect.poll(() => shown(tip(page, "rail-item-home"))).toBe(true);
    await page.keyboard.press("Escape");
    await expect(region(page).getByTestId("side-navigation")).toHaveAttribute("data-tip-dismissed", "");
    await expect.poll(() => shown(tip(page, "rail-item-home"))).toBe(false);
    await expect(theme).toBeFocused();
    // Pointer did not move: the link is still the hovered element.
    expect(await home.evaluate((el) => el.matches(":hover"))).toBe(true);
    await expect(region(page)).toHaveAttribute("data-rail", "collapsed");
    // Leave and come back: the tip shows again.
    await page.mouse.move(b.x + 600, b.y + 300);
    await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
    await expect.poll(() => shown(tip(page, "rail-item-home"))).toBe(true);
  });

  test("Escape dismisses the focus tip and focus stays on the link", async ({ page, browserName }) => {
    const home = region(page).getByTestId("rail-item-home");
    // macOS WebKit only Tabs to links with Alt+Tab.
    const tab = browserName === "webkit" ? "Alt+Tab" : "Tab";
    await home.focus();
    await page.keyboard.press(`Shift+${tab}`);
    await page.keyboard.press(tab); // :focus-visible
    await expect(home).toBeFocused();
    await expect.poll(() => shown(tip(page, "rail-item-home"))).toBe(true);
    await page.keyboard.press("Escape");
    await expect.poll(() => shown(tip(page, "rail-item-home"))).toBe(false);
    await expect(home).toBeFocused();
  });

  test("hoverable: moving the pointer from the link onto the tip keeps it shown", async ({ page }) => {
    const home = region(page).getByTestId("rail-item-home");
    const b = (await home.boundingBox())!;
    await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
    await expect.poll(() => shown(tip(page, "rail-item-home"))).toBe(true);
    const t = (await tip(page, "rail-item-home").boundingBox())!;
    expect(t.width > 0 && t.height > 0, JSON.stringify(t)).toBe(true);
    // Walk across the gap in steps so every intermediate point is hit.
    await page.mouse.move(t.x + t.width / 2, t.y + t.height / 2, { steps: 12 });
    expect(await page.evaluate(([x, y]) => {
      const hit = document.elementFromPoint(x!, y!);
      return !!hit?.closest(".cui-rail__tip");
    }, [t.x + t.width / 2, t.y + t.height / 2])).toBe(true);
    await expect.poll(() => shown(tip(page, "rail-item-home"))).toBe(true);
  });
});
