import { expect, test } from "@playwright/test";
import { TID } from "./fixtures";
import { KC_REALM, NO_AUTH, USERS, getSession, loginVia, sidCookie, submitKeycloak } from "./auth-helpers";
import { auditSince, dbNow, forceIdleExpiry, sessionRef } from "./db-helpers";

// Round-1 behaviour on the main gateway. Every test owns a fresh session.
test.use({ storageState: NO_AUTH });

async function csrf(page: import("@playwright/test").Page): Promise<string> {
  return (await (await getSession(page.request)).json()).csrf_token as string;
}

test.describe("re-authentication (SEC-001)", () => {
  test("normal sign-in reuses Keycloak SSO without the form and keeps the required acr (not 403)", async ({ page, baseURL, browser }) => {
    await loginVia(page, baseURL!, "/");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    // Second fresh console context carrying only the Keycloak SSO cookies.
    const kc = (await page.context().cookies()).filter((c) => c.domain === "localhost" && !c.name.startsWith("__Host-console"));
    const ctx = await browser.newContext({ ignoreHTTPSErrors: true, baseURL: baseURL! });
    await ctx.addCookies(kc);
    const p2 = await ctx.newPage();
    let formShown = false;
    p2.on("framenavigated", async (f) => { if (f === p2.mainFrame() && f.url().includes("/login-actions/")) formShown = true; });
    const cb = p2.waitForResponse((r) => r.url().startsWith(`${baseURL}/callback`));
    await p2.goto("/?site=hq");
    expect((await cb).status()).not.toBe(403);
    await p2.waitForURL((u) => u.origin === new URL(baseURL!).origin && u.pathname === "/");
    await expect(p2.getByTestId(TID.topNav)).toBeVisible();
    expect(formShown).toBe(false);
    expect((await sidCookie(ctx))?.value).not.toBe((await sidCookie(page.context()))?.value);
    await ctx.close();
  });

  test("after idle expiry the next login sends prompt=login and shows the Keycloak form despite SSO", async ({ page, baseURL }) => {
    const since = dbNow();
    await loginVia(page, baseURL!, "/?site=hq");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    const sid = (await sidCookie(page.context()))!.value;
    forceIdleExpiry(sid);
    // XHR first: 401 with the idle reason, and the reauth marker is set.
    const r = await getSession(page.request);
    expect(r.status()).toBe(401);
    expect((await r.json()).reason).toBe("idle_timeout");
    // Navigation: /login?...&reason=expired -> IdP with prompt=login.
    const authReq = page.waitForRequest((q) => q.url().startsWith(`${KC_REALM}/protocol/openid-connect/auth`));
    await page.goto("/?site=hq");
    const auth = new URL((await authReq).url());
    expect(auth.searchParams.get("prompt")).toBe("login");
    // Keycloak SSO is still alive, yet a credential form is shown (fresh
    // CAC/PIN stand-in): Keycloak's re-authentication form (username fixed).
    await expect(page.locator("#password")).toBeVisible();
    await submitKeycloak(page, USERS.user);
    await page.waitForURL((u) => u.origin === new URL(baseURL!).origin && u.pathname === "/");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    // console_notice is consumed by the shell and stripped (A11Y-7).
    await expect.poll(() => new URL(page.url()).searchParams.get("console_notice")).toBeNull();
    expect(new URL(page.url()).searchParams.get("site")).toBe("hq");
    const ref = sessionRef(sid);
    const rows = auditSince(since, `session_ref = '${ref}'`);
    expect(rows.map((x) => x.event_type)).toContain("session.expired.idle");
  });

  test("after explicit sign-out the next login shows the Keycloak form (prompt=login)", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/");
    await page.getByTestId(TID.userMenuButton).click();
    await page.getByTestId(TID.userMenuPanel).getByRole("button", { name: /sign out/i }).click();
    await page.waitForURL(`${baseURL}/logged-out`);
    const authReq = page.waitForRequest((q) => q.url().startsWith(`${KC_REALM}/protocol/openid-connect/auth`));
    await page.goto("/");
    expect(new URL((await authReq).url()).searchParams.get("prompt")).toBe("login");
    await expect(page.locator("#password")).toBeVisible();
  });
});

test.describe("sign-out redirect (SEC-005)", () => {
  test("sign-out uses location.replace: no extra history entry, lands on issuer then /logged-out", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/?site=hq");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    const before = await page.evaluate(() => history.length);
    const navs: string[] = [];
    page.on("framenavigated", (f) => { if (f === page.mainFrame()) navs.push(new URL(f.url()).origin); });
    await page.getByTestId(TID.userMenuButton).click();
    await page.getByTestId(TID.userMenuPanel).getByRole("button", { name: /sign out/i }).click();
    await page.waitForURL(`${baseURL}/logged-out`);
    const allowed = new Set([new URL(baseURL!).origin, new URL(KC_REALM).origin]);
    for (const o of navs) expect(allowed.has(o), `navigated to ${o}`).toBe(true);
    expect(await page.evaluate(() => history.length)).toBe(before);
  });

  test("a logout response redirecting off the issuer origin is not followed", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    await page.route("**/logout", (route) =>
      route.request().method() === "POST"
        ? route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({ redirect: "https://evil.example/phish", end_session_origin: new URL(KC_REALM).origin }),
          })
        : route.continue(),
    );
    const evil: string[] = [];
    page.on("request", (r) => { if (r.url().startsWith("https://evil.example")) evil.push(r.url()); });
    await page.getByTestId(TID.userMenuButton).click();
    await page.getByTestId(TID.userMenuPanel).getByRole("button", { name: /sign out/i }).click();
    await page.waitForTimeout(1500);
    expect(evil).toEqual([]);
    expect(new URL(page.url()).origin).not.toBe("https://evil.example");
  });
});

test.describe("API contract", () => {
  test("/api/unknown -> 401 unauthenticated, 404 JSON when signed in", async ({ page, baseURL, request }) => {
    const anon = await request.get("/api/unknown", { headers: { Accept: "application/json" }, maxRedirects: 0 });
    expect(anon.status()).toBe(401);
    await loginVia(page, baseURL!, "/");
    const r = await page.request.get("/api/unknown", { headers: { Accept: "application/json" }, maxRedirects: 0 });
    expect(r.status()).toBe(404);
    expect(r.headers()["content-type"]).toContain("application/json");
    expect(r.headers()["cache-control"]).toContain("no-store");
    expect(await r.json()).toEqual({ error: "not_found" });
  });

  test("CSRF denial is audited as access.denied csrf_token", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/");
    const since = dbNow();
    const r = await page.request.post("/api/session/keepalive", { headers: { Origin: baseURL!, "X-CSRF-Token": "wrong" } });
    expect(r.status()).toBe(403);
    const o = await page.request.post("/api/session/keepalive", { headers: { Origin: "https://evil.example", "X-CSRF-Token": await csrf(page) } });
    expect(o.status()).toBe(403);
    await expect
      .poll(() => auditSince(since, "event_type = 'access.denied'").map((x) => x.reason))
      .toEqual(expect.arrayContaining(["csrf_token", "origin"]));
  });
});

test.describe("session failure UI (A11Y-3)", () => {
  test("/api/session failure shows 'Session unavailable' with Retry and Sign in; Retry recovers", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    let fail = true;
    await page.route("**/api/session", (route) =>
      fail ? route.fulfill({ status: 500, contentType: "application/json", body: "{}" }) : route.continue(),
    );
    await page.reload();
    const failed = page.getByTestId("user-menu-failed");
    await expect(failed).toBeVisible();
    await expect(failed).toContainText(/session unavailable/i);
    await failed.getByRole("button").first().click();
    await expect(page.getByRole("menuitem", { name: /retry/i }).or(page.getByRole("button", { name: /retry/i }))).toBeVisible();
    const signIn = page.getByRole("link", { name: /sign in/i }).or(page.getByRole("menuitem", { name: /sign in/i }));
    await expect(signIn).toBeVisible();
    const href = await signIn.getAttribute("href");
    if (href) expect(href).toMatch(/^\/login\?return_to=/);
    fail = false;
    await page.getByRole("menuitem", { name: /retry/i }).or(page.getByRole("button", { name: /retry/i })).click();
    await expect(page.getByTestId(TID.userMenuButton)).toContainText(USERS.user.name);
  });
});
