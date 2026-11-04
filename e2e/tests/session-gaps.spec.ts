import { expect, test } from "@playwright/test";
import { TID } from "./fixtures";
import { KC_REALM, NO_AUTH, SID_COOKIE, USERS, getSession, loginAt, sidCookie, submitKeycloak } from "./auth-helpers";
import { auditSince, dbNow, forceAbsoluteExpiry, sessionRef } from "./db-helpers";

// POAM-030 residue (M3 gaps now covered in the browser): absolute-expiry
// reauth, no-JS form logout through CSP form-action, stale_auth rejection.
// The keepalive 503 UI is keepalive-503.spec.ts (its own gateway).
// Each test signs in its own fresh context (never the setup storageState).
test.use({ storageState: NO_AUTH });

test.describe("absolute expiry (AC-12, IA-11)", () => {
  test("expired absolute deadline -> 401 absolute_timeout, audit, then prompt=login reauth back to the same page", async ({ page, baseURL }) => {
    await loginAt(page, baseURL!, "/gallery?site=hq");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    const sid = (await sidCookie(page.context()))!.value;
    const since = dbNow();
    // Unload the shell so its own requests do not race ours to the expired row.
    await page.goto("about:blank");
    forceAbsoluteExpiry(sid);

    // XHR contract: 401 with the absolute reason (the shell maps it to reason=expired).
    const r = await getSession(page.request);
    expect(r.status()).toBe(401);
    expect(await r.json()).toMatchObject({ error: "unauthenticated", reason: "absolute_timeout" });
    const rows = auditSince(since, `session_ref = '${sessionRef(sid)}'`);
    expect(rows.map((x) => x.event_type)).toContain("session.expired.absolute");

    // Browser: reload -> /login -> Keycloak WITH prompt=login, so the live
    // IdP SSO session is not silently reused; the password form is shown.
    const authorize = page.waitForRequest((q) => q.url().startsWith(`${KC_REALM}/protocol/openid-connect/auth`));
    await page.goto("/gallery?site=hq");
    const authUrl = new URL((await authorize).url());
    expect(authUrl.searchParams.get("prompt")).toBe("login");
    await expect(page.locator("#password")).toBeVisible();
    await submitKeycloak(page, USERS.user);
    await page.waitForURL((u) => u.pathname === "/gallery" && u.searchParams.get("site") === "hq");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    const fresh = (await sidCookie(page.context()))!.value;
    expect(fresh).not.toBe(sid);
  });
});

test.describe("stale_auth (IA-11)", () => {
  test("reauth answered by silent SSO reuse (auth_time before the request) is rejected", async ({ page, baseURL }) => {
    test.setTimeout(120_000);
    await loginAt(page, baseURL!, "/");
    const sid = (await sidCookie(page.context()))!.value;
    // auth_time must predate the next /login by more than the 30 s skew allowance.
    await page.waitForTimeout(32_000);
    forceAbsoluteExpiry(sid);
    expect((await getSession(page.request)).status()).toBe(401); // sets the reauth marker
    await page.goto("about:blank");

    // Start the reauth sign-in through the context's API client (same cookie
    // jar, so the auth cookie lands in the browser), then strip prompt=login
    // from the IdP URL: Keycloak reuses the SSO session (old auth_time)
    // exactly like a non-compliant IdP would. (WebKit cannot fulfill a
    // rewritten 302 via page.route, so no interception is used.)
    const since = dbNow();
    const lr = await page.request.get(`${baseURL}/login?return_to=%2F&reason=expired`, { maxRedirects: 0 });
    expect(lr.status()).toBe(302);
    const idp = new URL(lr.headers()["location"] ?? "");
    const sawPrompt = idp.searchParams.get("prompt") === "login";
    idp.searchParams.delete("prompt");
    idp.searchParams.delete("max_age");
    const callback = page.waitForResponse((r) => new URL(r.url()).pathname === "/callback");
    await page.goto(idp.toString());
    const cb = await callback;
    expect(sawPrompt, "gateway asked for prompt=login").toBe(true);
    expect(cb.status()).toBe(400);
    await expect(page.getByRole("heading", { name: /sign in again/i })).toBeVisible();
    expect(await sidCookie(page.context()), "no session minted").toBeFalsy();
    const rows = auditSince(since, "reason = 'stale_auth'");
    expect(rows.length).toBeGreaterThanOrEqual(1);
    expect(rows[0]!.outcome).not.toBe("success");
  });
});

test.describe("no-JS form logout (SEC-004, CSP form-action)", () => {
  test("form POST with csrf field ends the session and its 303 to the IdP end-session endpoint is allowed by CSP", async ({ browser, baseURL }) => {
    // Sign in with JS (Keycloak + shell), then do the logout with JS OFF.
    const ctx = await browser.newContext({ baseURL: baseURL!, ignoreHTTPSErrors: true, storageState: NO_AUTH });
    const p = await ctx.newPage();
    await loginAt(p, baseURL!, "/");
    const state = await ctx.storageState();
    const tok = String((await (await getSession(ctx.request)).json()).csrf_token);
    const shell = await ctx.request.get(`${baseURL}/`, { headers: { Accept: "text/html", "Sec-Fetch-Mode": "navigate" } });
    const csp = shell.headers()["content-security-policy"] ?? "";
    expect(csp).toMatch(/form-action 'self' http:\/\/127\.0\.0\.1:8180/);
    await ctx.close();

    const nojs = await browser.newContext({ baseURL: baseURL!, ignoreHTTPSErrors: true, javaScriptEnabled: false, storageState: state });
    const page = await nojs.newPage();
    const violations: string[] = [];
    page.on("console", (m) => {
      if (/Content Security Policy|form-action/i.test(m.text())) violations.push(m.text());
    });
    // A console-origin document carrying the console's real CSP and a plain
    // HTML logout form (what a no-JS fallback page renders).
    const formPath = "/__e2e_nojs_logout";
    await page.route(`${baseURL}${formPath}`, (route) =>
      route.fulfill({
        status: 200,
        headers: { "content-type": "text/html; charset=utf-8", "content-security-policy": csp },
        body: `<!doctype html><html lang="en"><title>logout</title><form method="post" action="/logout"><input type="hidden" name="csrf" value="${tok}"><button type="submit">Sign out</button></form></html>`,
      }),
    );
    await page.goto(`${baseURL}${formPath}`);
    // WebKit does not surface response events inside a no-JS form redirect
    // chain, so the evidence is the end state: the form POST, its 303 to the
    // IdP end-session endpoint (only reachable if CSP form-action allowed
    // it) and the IdP's redirect back to /logged-out.
    await page.getByRole("button", { name: "Sign out" }).click();
    await page.waitForURL((u) => u.pathname === "/logged-out", { timeout: 20_000 });
    await expect(page.getByRole("heading", { name: /signed out/i })).toBeVisible();
    expect(violations, violations.join("\n")).toEqual([]);
    expect((await getSession(nojs.request)).status()).toBe(401);
    const sid = (await nojs.cookies()).find((c) => c.name === SID_COOKIE);
    expect(sid?.value ?? "").toBe("");
    await nojs.close();
  });
});
