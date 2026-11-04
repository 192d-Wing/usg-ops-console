import { expect, test } from "@playwright/test";
import { ORIGINS, UNMET_ACR } from "../playwright.config";
import { KC_REALM, NO_AUTH, USERS, expectAxeClean, sidCookie, submitKeycloak } from "./auth-helpers";
import { auditSince, dbNow } from "./db-helpers";

// SEC-002 / IA-2(12): the ACR gateway (CONSOLE_REQUIRED_ACR=UNMET_ACR) must
// deny every dev-realm login (acr urn:usg-ops-console:dev:password).
// The 429 test targets its own gateway process (ORIGINS.rl, direct http), so
// exhausting the in-memory limiter never throttles other specs.
test.use({ storageState: NO_AUTH });

test("acr below the required value -> 403, no session, audited insufficient_assurance", async ({ page, browserName }) => {
  const since = dbNow();
  const authReq = page.waitForRequest((q) => q.url().startsWith(`${KC_REALM}/protocol/openid-connect/auth`));
  await page.goto(`${ORIGINS.acr}/`);
  expect(new URL((await authReq).url()).searchParams.get("acr_values")).toBe(UNMET_ACR);
  const cb = page.waitForResponse((r) => r.url().startsWith(`${ORIGINS.acr}/callback`));
  await submitKeycloak(page, USERS.user);
  expect((await cb).status()).toBe(403);
  await expect(page.locator("body")).toContainText(/CAC\/PIV sign-in required/i);
  expect(await sidCookie(page.context())).toBeUndefined();
  // A11Y-5: the 403 page offers ending SSO via the IdP.
  const sso = page.getByRole("link", { name: /sign out of single sign-on/i });
  await expect(sso).toBeVisible();
  const href = new URL((await sso.getAttribute("href"))!);
  expect(href.origin + href.pathname).toBe(`${KC_REALM}/protocol/openid-connect/logout`);
  expect(href.searchParams.get("client_id")).toBe("usg-ops-console");
  if (browserName === "chromium") await expectAxeClean(page, "403 insufficient assurance");
  await expect
    .poll(() => auditSince(since, `event_type = 'login.denied' AND username = '${USERS.user.username}'`).map((x) => x.reason))
    .toContain("insufficient_assurance");
});

test("/login over the rate limit -> 429 with Retry-After, audited once as rate_limited", async ({ request, browserName }) => {
  // Only one engine exhausts the limiter; the result would be identical.
  test.skip(browserName !== "chromium", "limiter is per gateway, run once");
  const since = dbNow();
  let limited: import("@playwright/test").APIResponse | undefined;
  for (let i = 0; i < 40 && !limited; i++) {
    const r = await request.get(`${ORIGINS.rl}/login`, { headers: { Accept: "application/json" }, maxRedirects: 0 });
    if (r.status() === 429) limited = r;
  }
  expect(limited, "no 429 within 40 /login requests").toBeTruthy();
  expect(limited!.headers()["retry-after"]).toBe("60");
  expect(await limited!.json()).toEqual({ error: "rate_limited" });
  await expect
    .poll(() => auditSince(since, `event_type = 'access.denied' AND reason = 'rate_limited' AND client_ip <> '127.0.0.1'`).length)
    .toBe(1);
});
