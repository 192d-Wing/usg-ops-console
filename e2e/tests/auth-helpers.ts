import { expect, type APIRequestContext, type BrowserContext, type Page } from "@playwright/test";

/**
 * DEV ONLY test users from deploy/compose/keycloak/realm-dev.json. The
 * passwords are throwaway values that exist only in the local compose realm.
 */
export const USERS = {
  user: { username: "console_user", password: "dev-only-console_user", name: /Console User|console_user/ },
  admin: { username: "console_admin", password: "dev-only-console_admin", name: /Console Admin|console_admin/ },
  noRole: { username: "no_role", password: "dev-only-no_role", name: /No Role|no_role/ },
} as const;

/**
 * Revocation users (one per browser so parallel engines never disable each
 * other's user): `revoke_*` is disabled/re-enabled, `derole_*` loses/regains
 * console-user via the Keycloak admin API.
 */
export function revocationUser(kind: "revoke" | "derole", browserName: string) {
  const username = `${kind}_${browserName}`;
  return { username, password: `dev-only-${username}`, name: new RegExp(username) };
}
export type TestUser = { username: string; password: string; name: RegExp };

export const SID_COOKIE = "__Host-console_sid";
export const AUTH_COOKIE = "__Host-console_auth";
// IP literal, not localhost: the gateway sends HSTS for localhost and Linux
// WebKit would upgrade a plain-HTTP localhost Keycloak to TLS.
export const KEYCLOAK = "http://127.0.0.1:8180";
export const KC_REALM = `${KEYCLOAK}/realms/icam`;
/** Matches a compact JWS/JWT anywhere in a string. */
export const JWT_RE = /eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*/;
export const TOKEN_PARAM_RE = /[?&#](access_token|id_token|refresh_token|token|code|session_state)=/;

/** Fill the Keycloak username/password form (stand-in for CAC). */
export async function submitKeycloak(page: Page, user: TestUser): Promise<void> {
  await expect(page).toHaveURL(new RegExp(`^${KC_REALM}/`));
  await page.locator("#password").waitFor();
  // Re-authentication form (prompt=login with live SSO) shows the username as
  // fixed text with no #username input.
  if (await page.locator("#username").isVisible()) await page.locator("#username").fill(user.username);
  await page.locator("#password").fill(user.password);
  await page.locator("#kc-login").click();
}

/**
 * Navigate to `path` unauthenticated, sign in at Keycloak, and wait to land
 * back on `origin` (the project baseURL) past /callback.
 */
export async function loginVia(page: Page, origin: string, path: string, user: TestUser = USERS.user): Promise<void> {
  const want = new URL(origin).origin;
  await freshClientIp(page);
  await page.goto(path);
  await submitKeycloak(page, user);
  await page.waitForURL((u) => u.origin === want && !u.pathname.startsWith("/callback"));
}

/**
 * Give this context its own X-Forwarded-For source IP (TEST-NET 198.18/15) so
 * the gateway's per-IP /login limiter buckets tests separately (dev Caddy and
 * E2E gateways trust it; see Caddyfile).
 */
export async function freshClientIp(page: Page): Promise<void> {
  const r = () => 1 + Math.floor(Math.random() * 253);
  await page.context().setExtraHTTPHeaders({ "X-Forwarded-For": `198.19.${r()}.${r()}` });
}

export async function sidCookie(context: BrowserContext) {
  return (await context.cookies()).find((c) => c.name === SID_COOKIE);
}

/** GET /api/session as JSON (XHR-style). */
export async function getSession(req: APIRequestContext) {
  return req.get("/api/session", { headers: { Accept: "application/json" }, maxRedirects: 0 });
}

/** Fresh (unauthenticated) state for specs that create their own sessions. */
export const NO_AUTH = { cookies: [], origins: [] };

/** Assert axe finds no serious/critical WCAG 2.1 A/AA violations. */
export async function expectAxeClean(page: Page, name: string, include?: string): Promise<void> {
  const { default: AxeBuilder } = await import("@axe-core/playwright");
  let b = new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"]);
  if (include) b = b.include(include);
  const results = await b.analyze();
  const blocking = results.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => ({ id: v.id, impact: v.impact, nodes: v.nodes.map((n) => n.target.join(" ")) }));
  expect(blocking, `${name}: ${JSON.stringify(blocking, null, 2)}`).toEqual([]);
}

/**
 * POAM-032: deterministic sign-in. Starts at /login directly (no shell
 * bootstrap racing its own /login navigation), checks the /login 302 to the
 * IdP, submits the form, and waits for the /callback RESPONSE (status +
 * Location) instead of polling the page URL. Returns the callback Location
 * resolved against the console origin.
 */
export async function loginAt(page: Page, origin: string, returnTo = "/", user: TestUser = USERS.user): Promise<URL> {
  const want = new URL(origin).origin;
  await freshClientIp(page);
  const callback = page.waitForResponse((r) => {
    const u = new URL(r.url());
    return u.origin === want && u.pathname === "/callback";
  });
  const loginRes = page.waitForResponse((r) => {
    const u = new URL(r.url());
    return u.origin === want && u.pathname === "/login";
  });
  // Do not await goto: with live IdP SSO the whole chain completes inside it.
  const nav = page.goto(`${want}/login?return_to=${encodeURIComponent(returnTo)}`).catch(() => null);
  const lr = await loginRes;
  expect(lr.status(), "/login must redirect to the IdP").toBe(302);
  const idp = await Promise.race([
    callback.then(() => false),
    page.locator("#password").waitFor().then(() => true),
  ]);
  if (idp) await submitKeycloak(page, user);
  const cb = await callback;
  await nav;
  expect([302, 303], `callback status (${cb.status()})`).toContain(cb.status());
  const loc = new URL(cb.headers()["location"] ?? "", want);
  await page.waitForURL((u) => u.origin === want && u.pathname === loc.pathname, { waitUntil: "commit" });
  return loc;
}

/** Keycloak SSO cookies of a context (to start another context already signed in at the IdP). */
export async function idpCookies(context: BrowserContext) {
  const kc = new URL(KEYCLOAK);
  return (await context.cookies()).filter((c) => c.domain.replace(/^\./, "") === kc.hostname);
}
