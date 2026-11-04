import { expect, type Frame, type Page } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { expectAxeClean } from "./auth-helpers";
import { HELLO_TID, TOOL_TID, openTool, toolFrame, toolFrameHandle, toolTest as test } from "./tool-helpers";

// Fix round 1, A11Y-001/003/004 (WCAG 2.4.3 focus order, 2.1.1 keyboard,
// 3.2.1 on focus, 1.3.1/2.4.6 panels, 2.4.7): keyboard focus across the tool
// iframe boundary, the 404 panel, and focus after "Reload tool".
//
// Rules under test (fixes-round-1-shell.md):
// - Natural Tab / Shift+Tab enter the frame at the tool's first / last
//   control; the shell never forces focus on frame focus (no Focus{First} on
//   click or on reverse entry).
// - Focus{edge} is sent only by "Skip to tool content" and F6 / Shift+F6 in
//   the shell; F6 / Shift+F6 inside the tool return to the shell (SDK).
// - "Reload tool" moves focus to the host's loading region / frame wrapper,
//   never to <body> and never into the tool.

const TABBABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

type ToolFocus = { idx: number; n: number; tag: string; testid: string | null; first: string | null; last: string | null };

/** Where focus is inside the tool document (index among visible tabbables; -1 = body/other). */
async function toolFocus(f: Frame): Promise<ToolFocus> {
  return f.evaluate((sel) => {
    const all = [...document.querySelectorAll<HTMLElement>(sel)].filter((e) => e.getClientRects().length > 0 && !e.closest("[inert]"));
    const a = document.activeElement as HTMLElement | null;
    const id = (e: HTMLElement | null | undefined) => e?.getAttribute("data-testid") ?? e?.textContent?.trim().slice(0, 40) ?? null;
    return { idx: a ? all.indexOf(a) : -1, n: all.length, tag: a?.tagName ?? "", testid: id(a), first: id(all[0]), last: id(all[all.length - 1]) };
  }, TABBABLE);
}

/** True if the shell document's focused element is the tool iframe. */
const shellFocusOnFrame = (page: Page) =>
  page.evaluate((tid) => document.activeElement?.getAttribute("data-testid") === tid, TOOL_TID.frame);

/** The shell element focused, as a short description. */
const shellActive = (page: Page) =>
  page.evaluate(() => {
    const a = document.activeElement as HTMLElement | null;
    return { tag: a?.tagName ?? "", testid: a?.getAttribute("data-testid") ?? "", inHeader: Boolean(a?.closest("header")), isBody: a === document.body };
  });

/** Focus the shell tabbable just before (dir=-1) or after (dir=1) the tool iframe; false if none. */
async function focusShellNeighbour(page: Page, dir: 1 | -1): Promise<boolean> {
  return page.evaluate(
    ([sel, tid, d]) => {
      const all = [...document.querySelectorAll<HTMLElement>(`${sel}, iframe`)].filter((e) => e.getClientRects().length > 0 && !e.closest("[inert]"));
      const i = all.findIndex((e) => e.getAttribute("data-testid") === tid);
      const n = all[i + (d as number)];
      if (i < 0 || !n) return false;
      n.focus();
      return document.activeElement === n;
    },
    [TABBABLE, TOOL_TID.frame, dir] as const,
  );
}

async function openHello(page: Page) {
  await openTool(page, "hello");
  const f = await toolFrameHandle(page, ORIGINS.hello);
  // Settle: a stray Focus{First} after load would show up as idx 0 here.
  await page.waitForTimeout(300);
  return f;
}

test.describe("A11Y-001: focus across the tool frame boundary", () => {
  test("Tab from the shell enters the tool at its first control and keeps moving", async ({ page, browserName }) => {
    test.skip(browserName === "webkit", "WebKit Tab skips links/buttons unless Full Keyboard Access is on; covered via F6/skip link");
    const f = await openHello(page);
    expect(await focusShellNeighbour(page, -1), "a shell control precedes the frame").toBe(true);
    await page.keyboard.press("Tab");
    await expect.poll(() => shellFocusOnFrame(page)).toBe(true);
    await expect.poll(async () => (await toolFocus(f)).idx).toBe(0);
    await page.keyboard.press("Tab");
    await expect.poll(async () => (await toolFocus(f)).idx, { message: "second Tab moves on (not pinned to first)" }).toBe(1);
  });

  test("Shift+Tab inside the tool walks backwards and leaves the frame (no trap)", async ({ page, browserName }) => {
    test.skip(browserName === "webkit", "WebKit Tab skips links/buttons unless Full Keyboard Access is on");
    const f = await openHello(page);
    await f.evaluate((sel) => {
      const all = [...document.querySelectorAll<HTMLElement>(sel)].filter((e) => e.getClientRects().length > 0);
      all[1]?.focus();
    }, TABBABLE);
    await expect.poll(async () => (await toolFocus(f)).idx).toBe(1);
    await page.keyboard.press("Shift+Tab");
    await expect.poll(async () => (await toolFocus(f)).idx).toBe(0);
    await page.keyboard.press("Shift+Tab");
    if (browserName === "firefox") {
      // Firefox platform behaviour (ux-a11y ruling, findings-a11y.md "Rounds 1–2
      // re-verification"; probe in fixes-round-1-shell.md Round 2 item 5): on
      // reverse traversal Firefox makes the <iframe> element itself one extra
      // focus stop in the shell document. Allowed only if that stop is a real,
      // visible, named stop outside the tool (2.4.7, 4.1.2), and only one press
      // (max 2 in total); the assertion after it is unchanged.
      await expect.poll(() => shellFocusOnFrame(page), { message: "Firefox: stop on the <iframe> element" }).toBe(true);
      await expect.poll(() => f.evaluate(() => document.hasFocus()), { message: "Firefox: the tool document is not focused at the stop" }).toBe(false);
      const stop = await page.evaluate((tid) => {
        const el = document.querySelector<HTMLIFrameElement>(`[data-testid="${tid}"]`);
        return { title: el?.title ?? "", boxShadow: el ? getComputedStyle(el).boxShadow : "none", ring: el?.hasAttribute("data-frame-focused") ?? false };
      }, TOOL_TID.frame);
      expect(stop.title.trim(), "iframe has an accessible name (title)").not.toBe("");
      expect(stop.ring, "state-driven ring attribute data-frame-focused on the iframe at the stop").toBe(true);
      expect(stop.boxShadow, "visible focus ring on the iframe stop (.tool-frame:focus)").not.toBe("none");
      await page.keyboard.press("Shift+Tab");
    }
    await expect.poll(() => shellFocusOnFrame(page), { message: "Shift+Tab from the first tool control leaves the frame" }).toBe(false);
    // Reverse entry: from a shell control after the frame (if any), Shift+Tab lands on the tool's LAST control.
    if (await focusShellNeighbour(page, 1)) {
      await page.keyboard.press("Shift+Tab");
      await expect.poll(() => shellFocusOnFrame(page)).toBe(true);
      await expect.poll(async () => { const s = await toolFocus(f); return s.idx === s.n - 1; }, { message: "reverse entry at the last control" }).toBe(true);
    }
  });

  test("clicking a control inside the tool does not yank focus to the first control", async ({ page }) => {
    const f = await openHello(page);
    const before = await toolFocus(f);
    expect(before.n).toBeGreaterThan(1);
    const target = toolFrame(page).getByTestId(HELLO_TID.counterInc);
    expect(before.first, "test target must not be the first control").not.toBe(HELLO_TID.counterInc);
    await target.click();
    await page.waitForTimeout(500);
    const after = await toolFocus(f);
    // Chromium/Firefox focus the clicked button; WebKit leaves focus on body. Never the first control.
    expect(after.idx, `focus after click: ${JSON.stringify(after)}`).not.toBe(0);
    if (after.idx >= 0) expect(after.testid).toBe(HELLO_TID.counterInc);
  });

  test("F6 / Shift+F6 in the shell enter the tool at first / last; F6 / Shift+F6 in the tool return to the shell", async ({ page }) => {
    const f = await openHello(page);
    await page.getByTestId("top-navigation").locator("a, button").first().focus();
    await page.keyboard.press("F6");
    await expect.poll(() => shellFocusOnFrame(page)).toBe(true);
    await expect.poll(async () => (await toolFocus(f)).idx, { message: "F6 into the tool: first control" }).toBe(0);

    await page.keyboard.press("F6");
    await expect.poll(() => shellFocusOnFrame(page), { message: "F6 in the tool leaves it" }).toBe(false);
    const out = await shellActive(page);
    expect(out.isBody, "focus landed on a shell control, not <body>").toBe(false);
    expect(out.inHeader, `F6 out goes to the top navigation (${JSON.stringify(out)})`).toBe(true);

    await page.keyboard.press("Shift+F6");
    await expect.poll(() => shellFocusOnFrame(page)).toBe(true);
    await expect.poll(async () => { const s = await toolFocus(f); return s.idx === s.n - 1; }, { message: "Shift+F6 into the tool: last control" }).toBe(true);

    await page.keyboard.press("Shift+F6");
    await expect.poll(() => shellFocusOnFrame(page), { message: "Shift+F6 in the tool leaves it" }).toBe(false);
    expect((await shellActive(page)).isBody).toBe(false);
  });

  test("'Skip to tool content' moves focus to the tool's first control", async ({ page, browserName }) => {
    const f = await openHello(page);
    await page.locator("body").focus();
    const skip = page.getByTestId("skip-to-tool");
    // WebKit only tabs to links with Alt held.
    for (let i = 0; i < 4 && !(await skip.evaluate((e) => e === document.activeElement)); i++) {
      await page.keyboard.press(browserName === "webkit" ? "Alt+Tab" : "Tab");
    }
    await expect(skip).toBeFocused();
    await page.keyboard.press("Enter");
    await expect.poll(() => shellFocusOnFrame(page)).toBe(true);
    await expect.poll(async () => (await toolFocus(f)).idx).toBe(0);
  });
});

test.describe("A11Y-003: not-found panel", () => {
  for (const id of ["nope", "denied"]) {
    test(`/tools/${id}/: heading 'Tool not found', matching title, way back to Console home, announced`, async ({ page, browserName }) => {
      await page.goto(`/tools/${id}/`);
      const panel = page.getByTestId(TOOL_TID.notFound);
      await expect(panel).toBeVisible();
      await expect(page.getByRole("heading", { name: /^tool not found$/i })).toBeVisible();
      await expect(page).toHaveTitle(/tool not found/i);
      const announced = await panel.evaluate(
        (e) => Boolean(e.closest('[role="alert"], [role="status"]') ?? e.querySelector('[role="alert"], [role="status"]')),
      );
      expect(announced, "panel is (inside) a live region").toBe(true);
      if (browserName === "chromium") await expectAxeClean(page, `tool not found (${id})`);
      await page.getByTestId("tool-not-found-home").click();
      await expect(page).toHaveURL((u) => u.pathname === "/");
      await expect(page.getByTestId("top-navigation")).toBeVisible();
    });
  }
});

test.describe("A11Y-004: focus after 'Reload tool'", () => {
  test("launch error -> Reload tool: focus moves to the host, never <body>, never into the tool", async ({ page, browserName }) => {
    let fail = true;
    await page.route("**/api/tools/hello/launch", (r) =>
      fail ? r.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: "unavailable" }) }) : r.continue(),
    );
    await page.goto("/tools/hello/");
    await expect(page.getByTestId(TOOL_TID.error)).toBeVisible();
    if (browserName === "chromium") await expectAxeClean(page, "tool launch error");
    fail = false;
    const reload = page.getByTestId(TOOL_TID.reload);
    await reload.focus();
    await page.keyboard.press("Enter");
    // Right after activation the button is gone: focus must have a home.
    await expect.poll(async () => (await shellActive(page)).isBody, { message: "focus not dropped to <body>" }).toBe(false);
    await expect(toolFrame(page).getByTestId(HELLO_TID.connection)).toHaveAttribute("data-connected", "true", { timeout: 20_000 });
    await page.waitForTimeout(300);
    const a = await shellActive(page);
    expect(a.isBody, `after connect: ${JSON.stringify(a)}`).toBe(false);
    expect(await shellFocusOnFrame(page), "reload never moves focus into the tool").toBe(false);
    expect(await page.evaluate(() => Boolean(document.activeElement?.closest('[data-testid="tool-host"]')))).toBe(true);
  });

  test("not connected -> Reload tool: focus stays in the host region", async ({ page }) => {
    test.setTimeout(60_000);
    // Serve a page that never says Ready on the tool origin (first load only).
    let blank = true;
    await page.route(`${ORIGINS.hello}/**`, (r) =>
      blank && r.request().isNavigationRequest()
        ? r.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html><title>silent</title><p>silent tool</p>" })
        : r.continue(),
    );
    await page.goto("/tools/hello/");
    await expect(page.getByTestId(TOOL_TID.notConnected)).toBeVisible({ timeout: 20_000 });
    await expect(page.getByTestId(TOOL_TID.notConnected)).toContainText(/has not connected/i);
    blank = false;
    await page.getByTestId(TOOL_TID.reload).focus();
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await shellActive(page)).isBody).toBe(false);
    await expect(toolFrame(page).getByTestId(HELLO_TID.connection)).toHaveAttribute("data-connected", "true", { timeout: 20_000 });
    expect(await shellFocusOnFrame(page)).toBe(false);
  });
});
