import { expect, test } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { TID } from "./fixtures";
import { NO_AUTH, expectAxeClean, getSession, loginAt } from "./auth-helpers";

// Runs only in ka503-* projects, against the gateway whose IdP token endpoint
// is unreachable for refresh grants (servers/idp-refresh-blackhole.mjs; brief
// Q6). M3 contract: keepalive 503 `unavailable` is retryable; the session is
// neither extended nor ended and the idle dialog shows an inline error.
// Gateway idle is 3600 s (playwright.config.ts gateway()), warning lead 120 s.
const IDLE_SECS = 3600;
const LEAD_SECS = Math.min(120, IDLE_SECS / 2);

test.use({ storageState: NO_AUTH });

test("Stay signed in with the IdP token endpoint down: 503, inline alert, dialog stays, session intact", async ({ page, baseURL, browserName }) => {
  await page.clock.install();
  await loginAt(page, baseURL!, "/");
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  // The idle timers start only once the shell has its session; on slower
  // (remote Linux) browsers fast-forwarding earlier skips them.
  await expect(page.getByTestId("user-menu-loading")).toHaveCount(0);
  const before = await (await getSession(page.request)).json();
  expect(before.idle_timeout_secs).toBe(IDLE_SECS);

  // runFor (not fastForward): fires every intermediate timer tick, which the
  // Linux Firefox run needed for the shell's idle scheduler to open the dialog.
  await page.clock.runFor((IDLE_SECS - LEAD_SECS + 2) * 1000);
  const dialog = page.getByTestId("idle-dialog");
  await expect(dialog).toBeVisible();
  const ka = page.waitForResponse((r) => r.url().endsWith("/api/session/keepalive") && r.request().method() === "POST");
  await page.getByTestId("idle-stay").click();
  const res = await ka;
  expect(res.status()).toBe(503);
  expect(res.headers()["content-type"]).toContain("application/json");

  const alert = page.getByTestId("idle-error");
  await expect(alert).toHaveAttribute("role", "alert");
  await expect(alert).not.toBeEmpty();
  await expect(dialog).toBeVisible(); // not ended, user may retry
  await expect(page.getByTestId("idle-stay")).toBeEnabled();
  if (browserName === "chromium") await expectAxeClean(page, "idle dialog with keepalive 503 error");

  const after = await getSession(page.request);
  expect(after.status()).toBe(200);
  expect((await after.json()).idle_expires_at).toBe(before.idle_expires_at); // not extended

  const proxy = await page.request.get(`${ORIGINS.ka503Proxy}/healthz`);
  expect((await proxy.json()).blackholed).toBeGreaterThan(0);
});
