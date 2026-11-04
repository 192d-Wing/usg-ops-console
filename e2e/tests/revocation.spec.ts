import { expect, test } from "@playwright/test";
import { TID } from "./fixtures";
import { NO_AUTH, getSession, loginVia, revocationUser, sidCookie } from "./auth-helpers";
import { auditSince, dbNow, sessionRef } from "./db-helpers";
import { setRealmRole, setUserEnabled } from "./kc-admin";

// NIST-001 / AC-2: every keepalive does a refresh grant; IdP-side revocation
// ends the console session at the next keepalive. One user per browser.
test.use({ storageState: NO_AUTH });
// Same Keycloak user across these tests: never run them concurrently.
test.describe.configure({ mode: "serial" });

async function keepalive(page: import("@playwright/test").Page, origin: string) {
  const tok = (await (await getSession(page.request)).json()).csrf_token as string;
  return page.request.post("/api/session/keepalive", { headers: { Origin: origin, "X-CSRF-Token": tok, Accept: "application/json" } });
}

test("keepalive refreshes against the IdP while the user is valid", async ({ page, baseURL, browserName }) => {
  const u = revocationUser("revoke", browserName);
  await setUserEnabled(u.username, true);
  await loginVia(page, baseURL!, "/", u);
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  const before = (await (await getSession(page.request)).json()).idle_expires_at as string;
  await page.waitForTimeout(1100); // second-granularity timestamps
  const r = await keepalive(page, baseURL!);
  expect(r.status()).toBe(200);
  // session.refresh is audited at most once per 60 s per session (by design),
  // so assert the effect: refresh grant succeeded and idle was extended.
  expect(Date.parse((await r.json()).idle_expires_at)).toBeGreaterThan(Date.parse(before));
});

test("disabling the user in Keycloak ends the session at the next keepalive", async ({ page, baseURL, browserName }) => {
  const u = revocationUser("revoke", browserName);
  await setUserEnabled(u.username, true);
  await loginVia(page, baseURL!, "/", u);
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  const sid = (await sidCookie(page.context()))!.value;
  const since = dbNow();
  try {
    await setUserEnabled(u.username, false);
    const r = await keepalive(page, baseURL!);
    expect(r.status()).toBe(401);
    expect(await r.json()).toMatchObject({ error: "unauthenticated", reason: "revoked" });
    expect((await getSession(page.request)).status()).toBe(401);
    const rows = auditSince(since, `session_ref = '${sessionRef(sid)}'`);
    expect(rows).toContainEqual(expect.objectContaining({ event_type: "session.revoked", reason: "idp_rejected" }));
  } finally {
    await setUserEnabled(u.username, true);
  }
});

test("losing console-user in Keycloak ends the session at the next keepalive (missing_role)", async ({ page, baseURL, browserName }) => {
  const u = revocationUser("derole", browserName);
  await setRealmRole(u.username, "console-user", true);
  await loginVia(page, baseURL!, "/", u);
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  const sid = (await sidCookie(page.context()))!.value;
  const since = dbNow();
  try {
    await setRealmRole(u.username, "console-user", false);
    const r = await keepalive(page, baseURL!);
    expect(r.status()).toBe(401);
    expect((await r.json()).reason).toBe("revoked");
    const rows = auditSince(since, `session_ref = '${sessionRef(sid)}'`);
    expect(rows).toContainEqual(expect.objectContaining({ event_type: "session.revoked", reason: "missing_role" }));
  } finally {
    await setRealmRole(u.username, "console-user", true);
  }
});
