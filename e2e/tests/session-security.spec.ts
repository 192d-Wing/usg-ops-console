import { expect, test } from "@playwright/test";
import { JWT_RE, KC_REALM, NO_AUTH, SID_COOKIE, USERS, freshClientIp, getSession, loginVia, sidCookie } from "./auth-helpers";

test.use({ storageState: NO_AUTH });

test.describe("unauthenticated contract (M4 tools rely on this)", () => {
  test("/healthz is public", async ({ request }) => {
    expect((await request.get("/healthz")).status()).toBe(200);
  });

  test("XHR without session -> 401 JSON", async ({ request }) => {
    const r = await getSession(request);
    expect(r.status()).toBe(401);
    expect(r.headers()["content-type"]).toContain("application/json");
    expect(r.headers()["cache-control"]).toContain("no-store");
    expect(r.headers()["www-authenticate"]).toMatch(/^Session realm="usg-ops-console"/);
    const body = await r.json();
    expect(body).toMatchObject({ error: "unauthenticated", reason: "no_session", login_url: "/login" });
  });

  for (const path of ["/", "/index.html", "/gallery", "/does/not/exist"]) {
    test(`static/shell ${path} requires a session (XHR -> 401)`, async ({ request }) => {
      const r = await request.get(path, { headers: { Accept: "*/*", "Sec-Fetch-Mode": "cors" }, maxRedirects: 0 });
      expect(r.status()).toBe(401);
    });
  }

  test("navigation without session -> 302 /login with encoded return_to", async ({ request, baseURL }) => {
    const r = await request.get("/gallery?x=1&y=a%20b", {
      headers: { Accept: "text/html", "Sec-Fetch-Mode": "navigate" },
      maxRedirects: 0,
    });
    expect(r.status()).toBe(302);
    const loc = new URL(r.headers()["location"] ?? "", baseURL);
    expect(loc.origin).toBe(new URL(baseURL!).origin);
    expect(loc.pathname).toBe("/login");
    expect(loc.searchParams.get("return_to")).toBe("/gallery?x=1&y=a%20b");
  });

  test("/login never puts return_to in the IdP URL", async ({ request }) => {
    const r = await request.get(`/login?return_to=${encodeURIComponent("//evil.example/x")}`, { maxRedirects: 0 });
    expect(r.status()).toBe(302);
    const loc = r.headers()["location"] ?? "";
    expect(loc.startsWith(`${KC_REALM}/protocol/openid-connect/auth`)).toBe(true);
    expect(loc).not.toContain("evil");
    const setCookie = r.headersArray().filter((h) => h.name.toLowerCase() === "set-cookie").map((h) => h.value);
    const auth = setCookie.find((c) => c.startsWith("__Host-console_auth="));
    expect(auth).toBeTruthy();
    expect(auth!).toMatch(/;\s*Secure/i);
    expect(auth!).toMatch(/;\s*HttpOnly/i);
    expect(auth!).toMatch(/;\s*SameSite=Lax/i);
    expect(auth!).toMatch(/;\s*Path=\//i);
    expect(auth!).not.toMatch(/;\s*Domain=/i);
  });

  test("callback without the auth cookie / with forged state is rejected, no session", async ({ request }) => {
    const r = await request.get("/callback?code=forged&state=forged", { maxRedirects: 0 });
    expect([400, 401, 403]).toContain(r.status());
    const sc = r.headersArray().filter((h) => h.name.toLowerCase() === "set-cookie").map((h) => h.value).join("\n");
    // No session id minted (an empty clearing cookie is fine).
    expect(sc).not.toMatch(new RegExp(`${SID_COOKIE}=[^;\\s]+`));
  });
});

// Open redirect (SI-10): each payload must land on "/" of the console origin.
const BAD_RETURN_TO = [
  "//evil.example",
  "///evil.example",
  "/\\evil.example",
  "\\\\evil.example",
  "/%2f%2fevil.example",
  "/%2F%2Fevil.example",
  "/%5cevil.example",
  "https://evil.example/",
  "http:evil.example",
  "javascript:alert(1)",
  "evil.example",
  "/ok\r\nSet-Cookie: x=1",
  "/ok\tx",
  "/login?return_to=//evil.example",
  "/callback",
  "/logout",
  `/${"a".repeat(3000)}`,
];

test.describe("open redirect via return_to", () => {
  test("hostile return_to values all land on / (one SSO login, then silent)", async ({ page, baseURL }) => {
    test.setTimeout(120_000);
    const origin = new URL(baseURL!).origin;
    // First login establishes Keycloak SSO; the rest bounce through silently.
    await loginVia(page, baseURL!, `/login?return_to=${encodeURIComponent(BAD_RETURN_TO[0]!)}`);
    expect(new URL(page.url()).origin).toBe(origin);
    expect(new URL(page.url()).pathname).toBe("/");
    for (const bad of BAD_RETURN_TO.slice(1)) {
      // Unload the previous shell first: each /login rotates the session, so
      // the old page's in-flight /api/session gets 401 and the shell starts
      // its own /login navigation, aborting ours (CI 41766732456: ERR_ABORTED).
      await page.goto("about:blank");
      // 13 /login hits from one IP would trip the per-IP /login limiter (429
      // on the retry in the same CI run); one TEST-NET IP per bounce.
      await freshClientIp(page);
      await page.goto(`/login?return_to=${encodeURIComponent(bad)}`);
      // Wait for the whole /login -> IdP -> /callback bounce to settle; the
      // starting /login URL itself must not satisfy the predicate (race).
      await page.waitForURL(
        (u) => u.origin === origin && !u.pathname.startsWith("/callback") && !u.pathname.startsWith("/login"),
      );
      const u = new URL(page.url());
      expect(u.origin, bad).toBe(origin);
      expect(u.pathname, bad).toBe("/");
    }
  });

  test("a valid same-origin return_to is honoured", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, `/login?return_to=${encodeURIComponent("/gallery?site=hq")}`);
    const u = new URL(page.url());
    expect(u.pathname).toBe("/gallery");
    expect(u.searchParams.get("site")).toBe("hq");
  });
});

test.describe("session fixation and rotation (SC-23)", () => {
  test("attacker-chosen sid is never adopted; login mints a fresh id", async ({ page, baseURL, context }) => {
    const planted = "A".repeat(43);
    await context.addCookies([{ name: SID_COOKIE, value: planted, url: baseURL!, secure: true, httpOnly: true, sameSite: "Lax" }]);
    expect((await getSession(page.request)).status()).toBe(401);
    await loginVia(page, baseURL!, "/");
    const sid = await sidCookie(context);
    expect(sid?.value).toBeTruthy();
    expect(sid!.value).not.toBe(planted);
  });

  test("re-login rotates the session id and the old id is dead", async ({ page, baseURL, context, playwright }) => {
    await loginVia(page, baseURL!, "/");
    const first = (await sidCookie(context))!.value;
    await page.goto("/login?return_to=%2F");
    await page.waitForURL((u) => u.origin === new URL(baseURL!).origin && u.pathname === "/");
    const second = (await sidCookie(context))!.value;
    expect(second).not.toBe(first);
    const stale = await playwright.request.newContext({ baseURL: baseURL!, ignoreHTTPSErrors: true,
      extraHTTPHeaders: { Cookie: `${SID_COOKIE}=${first}` } });
    expect((await getSession(stale)).status()).toBe(401);
    await stale.dispose();
  });
});

test.describe("CSRF on state-changing endpoints", () => {
  test("keepalive and logout need token + same Origin", async ({ page, baseURL }) => {
    await loginVia(page, baseURL!, "/");
    const s = await (await getSession(page.request)).json();
    const tok = String(s.csrf_token);
    expect(tok).not.toMatch(JWT_RE);
    const post = (path: string, headers: Record<string, string>) =>
      page.request.post(path, { headers: { Accept: "application/json", ...headers }, maxRedirects: 0 });
    expect((await post("/api/session/keepalive", { Origin: baseURL! })).status()).toBe(403);
    expect((await post("/api/session/keepalive", { Origin: baseURL!, "X-CSRF-Token": "wrong" })).status()).toBe(403);
    expect((await post("/api/session/keepalive", { Origin: "https://evil.example", "X-CSRF-Token": tok })).status()).toBe(403);
    expect((await post("/logout", { Origin: "https://evil.example", "X-CSRF-Token": tok })).status()).toBe(403);
    // Still signed in after all the rejected attempts.
    expect((await getSession(page.request)).status()).toBe(200);
    const ok = await post("/api/session/keepalive", { Origin: baseURL!, "X-CSRF-Token": tok });
    expect(ok.status()).toBe(200);
    // GET /logout must not sign out (no state change on GET).
    expect((await page.request.get("/logout", { maxRedirects: 0 })).status()).not.toBe(200);
    expect((await getSession(page.request)).status()).toBe(200);
  });
});

test("console_admin carries console-admin role", async ({ page, baseURL }) => {
  await loginVia(page, baseURL!, "/", USERS.admin);
  const s = await (await getSession(page.request)).json();
  expect(s.roles).toEqual(expect.arrayContaining(["console-user", "console-admin"]));
  for (const r of s.roles as string[]) expect(r.startsWith("console-")).toBe(true);
});
