import { expect, test, type Page } from "@playwright/test";
import { TID } from "./fixtures";
import { NO_AUTH, expectAxeClean, getSession, loginVia } from "./auth-helpers";

// Runs only in idle-* projects against the gateway with
// CONSOLE_SESSION_IDLE_SECS=150 (CONSOLE_TLS_PORT_IDLE, default https://localhost:9445). Warning lead is
// min(120 s, idle/2) = 75 s, so the dialog opens at t=75 s and the deadline
// is t=150 s. page.clock drives the shell timers; the server keeps real
// time, which is fine because client deadlines precede server expiry.
// AC-11: only the explicit "Stay signed in" (or Escape, brief Q4) extends.
const IDLE_SECS = 150;
const LEAD_SECS = Math.min(120, IDLE_SECS / 2);

test.use({ storageState: NO_AUTH });

function dialog(page: Page) {
  return page.getByTestId("idle-dialog");
}

async function signedIn(page: Page, baseURL: string, path = "/?site=hq") {
  await page.clock.install();
  await loginVia(page, baseURL, path);
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  // The idle timers start only once the shell has its session; on slower
  // (remote Linux) browsers fast-forwarding earlier skips them.
  await expect(page.getByTestId("user-menu-loading")).toHaveCount(0);
  const s = await (await getSession(page.request)).json();
  expect(s.idle_timeout_secs).toBe(IDLE_SECS);
  return s as { idle_expires_at: string; absolute_expires_at: string };
}

test("no warning before the lead window; GET /api/session does not extend idle", async ({ page, baseURL }) => {
  const s1 = await signedIn(page, baseURL!);
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS - 10) * 1000);
  await expect(dialog(page)).toBeHidden();
  const s2 = await (await getSession(page.request)).json();
  expect(s2.idle_expires_at).toBe(s1.idle_expires_at);
});

test("warning dialog: focus on Stay signed in, focus trapped, axe-clean; Stay extends", async ({ page, baseURL, browserName }) => {
  const s1 = await signedIn(page, baseURL!);
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS + 2) * 1000);
  const d = dialog(page);
  await expect(d).toBeVisible();
  await expect(d).toContainText(/session is about to expire/i);
  const stay = page.getByTestId("idle-stay");
  await expect(stay).toBeFocused();
  await expect(page.getByTestId("idle-sign-out")).toBeVisible();
  await expect(page.getByTestId("idle-countdown")).toHaveText(/\d+:\d{2}/);
  for (let i = 0; i < 6; i++) {
    await page.keyboard.press("Tab");
    expect(await d.evaluate((el) => el.contains(document.activeElement))).toBe(true);
  }
  if (browserName === "chromium") await expectAxeClean(page, "idle dialog");
  // Server time is real (page.clock fakes only the page): let >1 s pass so the
  // extended idle_expires_at is strictly later at second granularity.
  await new Promise((r) => setTimeout(r, 1100));
  const keepalive = page.waitForResponse((r) => r.url().endsWith("/api/session/keepalive") && r.request().method() === "POST");
  await stay.click();
  const kr = await keepalive;
  expect(kr.status()).toBe(200);
  expect(kr.request().headers()["x-csrf-token"]).toBeTruthy();
  await expect(d).toBeHidden();
  const s2 = await kr.json();
  expect(Date.parse(s2.idle_expires_at)).toBeGreaterThan(Date.parse(s1.idle_expires_at));
  expect(s2.absolute_expires_at).toBe(s1.absolute_expires_at); // AC-12: never extended
});

test("Escape in the warning dialog = Stay signed in", async ({ page, baseURL }) => {
  await signedIn(page, baseURL!);
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS + 2) * 1000);
  await expect(dialog(page)).toBeVisible();
  const keepalive = page.waitForRequest((r) => r.url().endsWith("/api/session/keepalive") && r.method() === "POST");
  await page.keyboard.press("Escape");
  await keepalive;
  await expect(dialog(page)).toBeHidden();
});

test("letting it lapse sends the user to /login with return_to preserved", async ({ page, baseURL }) => {
  await signedIn(page, baseURL!, "/gallery?site=hq&tenant=ops");
  const login = page.waitForRequest((r) => new URL(r.url()).pathname === "/login" && r.isNavigationRequest());
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS + 2) * 1000);
  await expect(dialog(page)).toBeVisible();
  await page.clock.fastForward((LEAD_SECS + 2) * 1000);
  const u = new URL((await login).url());
  expect(u.origin).toBe(new URL(baseURL!).origin);
  expect(u.searchParams.get("return_to")).toBe("/gallery?site=hq&tenant=ops");
  expect(u.searchParams.get("reason")).toBe("expired"); // A11Y-7 / SEC-001 prompt=login
});

test("overlay click does nothing: no keepalive, idle_expires_at unchanged (AC-11, NIST-003)", async ({ page, baseURL }) => {
  const s1 = await signedIn(page, baseURL!);
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS + 2) * 1000);
  const d = dialog(page);
  await expect(d).toBeVisible();
  let keepalives = 0;
  page.on("request", (r) => { if (r.url().endsWith("/api/session/keepalive")) keepalives++; });
  // Click the backdrop outside the dialog box (top-left corner of the viewport).
  await page.mouse.click(4, 4);
  await page.mouse.click(4, (page.viewportSize()?.height ?? 600) - 4);
  await expect(d).toBeVisible();
  expect(keepalives).toBe(0);
  const s2 = await (await getSession(page.request)).json();
  expect(s2.idle_expires_at).toBe(s1.idle_expires_at);
});

test("no X / 'Close dialog' button; every dialog button is Stay or Sign out (A11Y-1)", async ({ page, baseURL }) => {
  await signedIn(page, baseURL!);
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS + 2) * 1000);
  const d = dialog(page);
  await expect(d).toBeVisible();
  await expect(d.getByRole("button", { name: /close/i })).toHaveCount(0);
  await expect(d.locator(".cui-icon-btn")).toHaveCount(0);
  const names = await d.getByRole("button").allInnerTexts();
  for (const n of names) expect(n).toMatch(/stay signed in|sign out/i);
  // A11Y-6: the announcer lives outside the dialog.
  await expect(d.getByTestId("idle-announcer")).toHaveCount(0);
  await expect(page.getByTestId("idle-announcer")).toHaveCount(1);
});

test("M4.5: idle dialog opens above the palette: palette closes, chrome inert, shortcut ignored", async ({ page, baseURL }) => {
  await signedIn(page, baseURL!);
  await page.getByTestId("palette-trigger").click();
  await expect(page.getByTestId("palette-input")).toBeFocused();
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS + 2) * 1000);
  const d = dialog(page);
  await expect(d).toBeVisible();
  await expect(page.getByTestId("command-palette")).toBeHidden();
  await expect(page.getByTestId("idle-stay")).toBeFocused();
  await expect(page.locator("#app-chrome")).toHaveAttribute("inert", "");
  // Topmost at the dialog centre is the idle dialog (stacking above palette layers).
  const top = await d.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
    return !!hit && el.contains(hit);
  });
  expect(top).toBe(true);
  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.press("Control+k");
  await expect(page.getByTestId("command-palette")).toBeHidden();
  await expect(page.getByTestId("idle-stay")).toBeFocused();
  for (let i = 0; i < 4; i++) {
    await page.keyboard.press("Tab");
    expect(await d.evaluate((el) => el.contains(document.activeElement))).toBe(true);
  }
});

// Fix round 2 (SEC-002): once the idle dialog has closed the palette, the
// chrome must stay inert (the palette's own un-inert must not win).
test("SEC-002: idle over palette, palette closed -> #app-chrome still inert until the dialog is answered", async ({ page, baseURL }) => {
  await signedIn(page, baseURL!);
  await page.getByTestId("palette-trigger").click();
  await expect(page.getByTestId("palette-input")).toBeFocused();
  await expect(page.locator("#app-chrome")).toHaveAttribute("inert", "");
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS + 2) * 1000);
  await expect(dialog(page)).toBeVisible();
  await expect(page.getByTestId("command-palette")).toBeHidden();
  await expect(page.locator("#app-chrome")).toHaveAttribute("inert", "");
  // Let any deferred palette close/focus-restore tasks run, then re-check.
  await page.clock.runFor(1000);
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => setTimeout(r, 0))));
  await expect(page.locator("#app-chrome")).toHaveAttribute("inert", "");
  expect(await page.getByTestId("palette-trigger").evaluate((el) => !!el.closest("[inert]"))).toBe(true);
  await expect(page.getByTestId("idle-stay")).toBeFocused();
  await page.getByTestId("idle-stay").click();
  await expect(dialog(page)).toBeHidden();
  await expect(page.locator("#app-chrome")).not.toHaveAttribute("inert", "");
});
