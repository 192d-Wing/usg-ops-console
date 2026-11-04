import { expect, test } from "@playwright/test";
import { TID } from "./fixtures";
import {
  AUTH_COOKIE, JWT_RE, KC_REALM, NO_AUTH, SID_COOKIE, TOKEN_PARAM_RE, USERS,
  expectAxeClean, freshClientIp, getSession, loginVia, sidCookie, submitKeycloak,
} from "./auth-helpers";

// Every test here owns its session (fresh context): logout must never touch
// the shared storageState session used by the other specs.
test.use({ storageState: NO_AUTH });

test.describe("sign-in", () => {
  test("unauthenticated deep link goes to Keycloak and returns to the same path+query", async ({ page, baseURL }) => {
    const seen: string[] = [];
    page.on("framenavigated", (f) => { if (f === page.mainFrame()) seen.push(f.url()); });
    await freshClientIp(page);
    await page.goto("/?site=hq&tenant=ops");
    await expect(page).toHaveURL(new RegExp(`^${KC_REALM}/protocol/openid-connect/auth`));
    // PKCE S256 + state + nonce on the authorization request; return_to is server-side only.
    const auth = new URL(page.url());
    expect(auth.searchParams.get("code_challenge_method")).toBe("S256");
    expect(auth.searchParams.get("state")).toBeTruthy();
    expect(auth.searchParams.get("nonce")).toBeTruthy();
    expect(auth.searchParams.get("client_id")).toBe("usg-ops-console");
    expect(auth.searchParams.get("redirect_uri")).toBe(`${baseURL}/callback`);
    expect(page.url()).not.toContain("return_to");
    await submitKeycloak(page, USERS.user);
    await page.waitForURL((u) => u.origin === new URL(baseURL!).origin && u.pathname === "/");
    const u = new URL(page.url());
    expect(u.searchParams.get("site")).toBe("hq");
    expect(u.searchParams.get("tenant")).toBe("ops");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    // Real user in the user menu, no demo placeholder.
    await expect(page.getByTestId(TID.userMenuButton)).toContainText(USERS.user.name);
    await expect(page.getByTestId(TID.userMenuButton)).not.toContainText("(demo)");
    // No tokens in any console URL; the callback code is consumed (final URL clean).
    expect(page.url()).not.toMatch(TOKEN_PARAM_RE);
    for (const url of seen.filter((s) => s.startsWith(baseURL!))) {
      expect(url).not.toMatch(JWT_RE);
      expect(url).not.toMatch(/[?&#](access_token|id_token|refresh_token)=/);
    }
  });

  test("session cookie flags: __Host-, Secure, HttpOnly, SameSite=Lax, Path=/, host-only, session", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/");
    const cookies = await page.context().cookies();
    const sid = cookies.find((c) => c.name === SID_COOKIE);
    expect(sid, JSON.stringify(cookies.map((c) => c.name))).toBeTruthy();
    expect(sid!.secure).toBe(true);
    expect(sid!.httpOnly).toBe(true);
    expect(sid!.sameSite).toBe("Lax");
    expect(sid!.path).toBe("/");
    expect(sid!.domain).toBe("localhost"); // host-only: no leading dot / Domain attr
    expect(sid!.expires).toBe(-1); // session cookie; server-side expiry authoritative
    expect(sid!.value.length).toBeGreaterThanOrEqual(43); // >= 256 bits base64url
    // Pre-login state cookie removed after the callback.
    expect(cookies.find((c) => c.name === AUTH_COOKIE && c.domain === "localhost")).toBeUndefined();
    // Not readable from script.
    expect(await page.evaluate(() => document.cookie)).not.toContain("console_sid");
  });

  test("no tokens in web storage, cookies or /api/session", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/");
    await expect(page.getByTestId(TID.topNav)).toBeVisible();
    const dump = await page.evaluate(() => {
      const all: string[] = [];
      for (const s of [localStorage, sessionStorage]) {
        for (let i = 0; i < s.length; i++) { const k = s.key(i)!; all.push(`${k}=${s.getItem(k)}`); }
      }
      return all;
    });
    for (const kv of dump) {
      expect(kv).not.toMatch(JWT_RE);
      expect(kv.toLowerCase()).not.toMatch(/access_token|id_token|refresh_token|csrf/);
    }
    for (const c of await page.context().cookies(baseURL!)) expect(c.value).not.toMatch(JWT_RE);
    const res = await getSession(page.request);
    expect(res.status()).toBe(200);
    expect(res.headers()["cache-control"]).toContain("no-store");
    const text = await res.text();
    expect(text).not.toMatch(JWT_RE);
    const body = JSON.parse(text);
    expect(body.user.username).toBe(USERS.user.username);
    expect(body.roles).toContain("console-user");
    expect(typeof body.csrf_token).toBe("string");
    expect(typeof body.idle_expires_at).toBe("string");
    expect(typeof body.absolute_expires_at).toBe("string");
  });

  test("user without console-user role gets 403 and no session", async ({ page, baseURL, browserName }) => {
    await freshClientIp(page);
    await page.goto("/");
    const resp = page.waitForResponse((r) => r.url().startsWith(`${baseURL}/callback`));
    await submitKeycloak(page, USERS.noRole);
    expect((await resp).status()).toBe(403);
    await expect(page.locator("body")).toContainText(/not authori[sz]ed/i);
    expect(await sidCookie(page.context())).toBeUndefined();
    expect((await getSession(page.request)).status()).toBe(401);
    // Error page leaks no token or reason detail beyond the generic message.
    expect(await page.content()).not.toMatch(JWT_RE);
    if (browserName === "chromium") await expectAxeClean(page, "403 page");
  });
});

test.describe("sign-out", () => {
  test("Sign out ends the Keycloak session (no confirm page) and lands on /logged-out", async ({ page, baseURL, browserName }) => {
    await loginVia(page, baseURL!, "/");
    const endSession = page.waitForRequest((r) => r.url().startsWith(`${KC_REALM}/protocol/openid-connect/logout`));
    await page.getByTestId(TID.userMenuButton).click();
    await page.getByTestId(TID.userMenuPanel).getByRole("button", { name: /sign out/i }).click();
    const es = new URL((await endSession).url());
    // id_token_hint is used (brief Q2) so Keycloak skips its confirmation page.
    expect(es.searchParams.get("id_token_hint")).toBeTruthy();
    expect(es.searchParams.get("post_logout_redirect_uri")).toBe(`${baseURL}/logged-out`);
    await page.waitForURL(`${baseURL}/logged-out`);
    await expect(page.getByRole("link", { name: /sign in/i })).toBeVisible();
    expect(await sidCookie(page.context())).toBeUndefined();
    expect((await getSession(page.request)).status()).toBe(401);
    if (browserName === "chromium") await expectAxeClean(page, "/logged-out");
    // Keycloak SSO ended too: / requires a fresh login form, not a silent SSO bounce.
    await page.goto("/");
    await expect(page).toHaveURL(new RegExp(`^${KC_REALM}/`));
    await expect(page.locator("#username")).toBeVisible();
  });

  test("logout POST without CSRF token is rejected and the session survives", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/");
    const r = await page.request.post("/logout", { headers: { Origin: baseURL!, Accept: "application/json" }, maxRedirects: 0 });
    expect(r.status()).toBe(403);
    expect((await r.json()).error).toBe("csrf");
    expect((await getSession(page.request)).status()).toBe(200);
  });

  test("sign out in one tab signs out the other tab", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/?site=hq");
    const other = await page.context().newPage();
    await other.goto("/?site=east");
    await expect(other.getByTestId(TID.topNav)).toBeVisible();
    await page.bringToFront();
    await page.getByTestId(TID.userMenuButton).click();
    await page.getByTestId(TID.userMenuPanel).getByRole("button", { name: /sign out/i }).click();
    await page.waitForURL(`${baseURL}/logged-out`);
    // Background tab becomes visible -> shell re-fetches /api/session -> 401 -> login.
    await other.bringToFront();
    await other.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
    await expect(other).toHaveURL(new RegExp(`^${KC_REALM}/`), { timeout: 15_000 });
  });
});
