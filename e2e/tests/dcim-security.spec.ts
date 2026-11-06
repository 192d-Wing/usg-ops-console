import { readFileSync } from "node:fs";
import { expect, test as plain } from "@playwright/test";
import { DCIM, ORIGINS } from "../playwright.config";
import { TID } from "./fixtures";
import { JWT_RE, NO_AUTH, USERS, loginAt } from "./auth-helpers";
import {
  ASSERTION_HEADER,
  DCIM_CSRF_HEADER,
  DCIM_USERS,
  TOOL_TID,
  DCIM_BASE,
  dcimFetch,
  dcimTest as test,
  launch,
  openDcim,
  toolFrameEl,
  toolFrameHandle,
} from "./tool-helpers";

// M5 (plan U6, plan §7 threat sketch): identity only via the gateway's
// assertion, no tool token in the browser, otter-go's own RBAC and CSRF,
// replay and direct-access refusal. DCIM_REPO only.
test.skip(!DCIM.enabled, "DCIM_REPO not set (usg-dcim E2E is local-only, brief Q9)");
plain.skip(!DCIM.enabled, "DCIM_REPO not set (usg-dcim E2E is local-only, brief Q9)");

const SITE_BODY = JSON.stringify({ name: `e2e-site-${Date.now()}`, slug: `e2e-${Date.now()}` });

test.describe("dcim embedded: no tool token in the browser", () => {
  test("finch sends no Authorization header and keeps no token/identity in localStorage", async ({ page }) => {
    const auth: string[] = [];
    let apiCalls = 0;
    const pending: Promise<void>[] = [];
    page.on("request", (r) => {
      if (!r.url().startsWith(DCIM.origin)) return;
      if (new URL(r.url()).pathname.startsWith(`${DCIM_BASE}/api/`)) apiCalls++;
      pending.push(
        r.allHeaders().then((h) => {
          if (h.authorization !== undefined) auth.push(`${r.method()} ${r.url()}`);
          if (h[ASSERTION_HEADER] !== undefined) auth.push(`assertion from browser: ${r.url()}`);
        }, () => undefined),
      );
    });
    const { frame } = await openDcim(page, DCIM.origin, "/racks");
    await expect.poll(() => apiCalls, { message: "finch made API calls" }).toBeGreaterThan(0);
    await Promise.all(pending);
    expect(auth).toEqual([]);
    const ls = await frame.evaluate(() => Object.entries(localStorage));
    for (const [k, v] of ls) {
      expect(k, `localStorage key ${k}`).not.toMatch(/token|identity|id_token|auth/i);
      expect(v, `localStorage ${k} holds a JWT`).not.toMatch(JWT_RE);
    }
    const ss = await frame.evaluate(() => Object.values(sessionStorage).join("\n"));
    expect(ss).not.toMatch(JWT_RE);
  });
});

test.describe("dcim RBAC and CSRF (otter-go)", () => {
  test("viewer write is refused by otter-go (403, capability), even with the CSRF header", async ({ page }) => {
    const { frame } = await openDcim(page, DCIM.origin, "/sites");
    const read = await dcimFetch(frame, `${DCIM_BASE}/api/v1/inventory/sites`);
    expect(read.status, read.body).toBe(200);
    const w = await dcimFetch(frame, `${DCIM_BASE}/api/v1/inventory/sites`, {
      method: "POST",
      headers: { "Content-Type": "application/json", [DCIM_CSRF_HEADER]: "1" },
      body: SITE_BODY,
    });
    expect(w.status, w.body).toBe(403);
    expect(w.body).not.toMatch(/csrf/i);
  });

  test.describe("operator", () => {
    test.use({ dcimUser: DCIM_USERS.operator });
    test("missing X-DCIM-CSRF on a write is refused; with it the same write passes authz", async ({ page }) => {
      const { frame } = await openDcim(page, DCIM.origin, "/sites");
      const noCsrf = await dcimFetch(frame, `${DCIM_BASE}/api/v1/inventory/sites`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: SITE_BODY,
      });
      expect(noCsrf.status, noCsrf.body).toBe(403);
      expect(noCsrf.body).toMatch(/X-DCIM-CSRF/i);
      // Control: the operator holds inventory:sites:create, so with the header
      // the request is not refused by auth/CSRF (201 created, or 4xx validation).
      const withCsrf = await dcimFetch(frame, `${DCIM_BASE}/api/v1/inventory/sites`, {
        method: "POST",
        headers: { "Content-Type": "application/json", [DCIM_CSRF_HEADER]: "1" },
        body: SITE_BODY,
      });
      expect([401, 403], `with CSRF: ${withCsrf.status} ${withCsrf.body}`).not.toContain(withCsrf.status);
      expect(withCsrf.status, withCsrf.body).toBeLessThan(500);
    });
  });
});

test.describe("dcim assertion handling at otter-go", () => {
  test("a consumed assertion replayed directly to otter-go is refused (jti cache)", async ({ page }) => {
    const { frame } = await openDcim(page, DCIM.origin, "/racks");
    const ok = await dcimFetch(frame, `${DCIM_BASE}/api/v1/auth/me`);
    expect(ok.status, ok.body).toBe(200);
    // The rendered E2E nginx config logs each /api/ request's assertion (dcim-stack.mjs).
    let token = "";
    await expect
      .poll(() => {
        const lines = readFileSync(DCIM.assertionLog, "utf8").split("\n").filter((l) => JWT_RE.test(l));
        token = lines.at(-1) ?? "";
        return token;
      })
      .toMatch(JWT_RE);
    const claims = JSON.parse(Buffer.from(token.split(".")[1]!, "base64url").toString()) as { aud: string; exp: number; jti: string };
    expect(claims.aud).toBe("dcim");
    expect(claims.jti).toBeTruthy();
    expect(claims.exp * 1000, "replayed well inside the assertion lifetime").toBeGreaterThan(Date.now() + 5_000);
    const r = await fetch(`${DCIM.otterDirect}/api/v1/auth/me`, { headers: { [ASSERTION_HEADER]: token } });
    expect(r.status, await r.text()).toBe(401);
  });

  plain("direct access to otter-go without an assertion is 401; standalone login is disabled in console mode", async () => {
    const me = await fetch(`${DCIM.otterDirect}/api/v1/auth/me`);
    expect(me.status).toBe(401);
    const sites = await fetch(`${DCIM.otterDirect}/api/v1/inventory/sites`);
    expect(sites.status).toBe(401);
    const forged = await fetch(`${DCIM.otterDirect}/api/v1/auth/me`, { headers: { [ASSERTION_HEADER]: "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ4In0." } });
    expect(forged.status).toBe(401);
    const login = await fetch(`${DCIM.otterDirect}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "x@example.invalid", password: "x" }),
    });
    expect(login.status).toBe(404);
  });
});

plain.describe("user without dcim:viewer", () => {
  plain.use({ storageState: NO_AUTH });
  plain("console_user: DCIM not listed, no rail entry, launch 404, /tools/dcim/ 'Tool not found'", async ({ page, baseURL }) => {
    await loginAt(page, baseURL!, "/", USERS.user);
    const tools = await (await page.request.get("/api/tools", { headers: { Accept: "application/json" } })).json();
    expect((tools.tools as { id: string }[]).map((t) => t.id)).not.toContain("dcim");
    const listed = page.waitForResponse((r) => new URL(r.url()).pathname === "/api/tools" && r.status() === 200);
    await page.reload();
    await listed;
    await expect(page.getByTestId("rail-item-home")).toBeVisible();
    await expect(page.getByTestId(TID.sideNav).getByTestId("rail-item-tool-dcim")).toHaveCount(0);
    const r = await launch(page.request, "dcim");
    expect(r.status()).toBe(404);
    expect(await r.json()).toMatchObject({ error: "not_found" });
    await page.goto("/tools/dcim/");
    await expect(page.getByTestId(TOOL_TID.notFound)).toBeVisible();
    await expect(toolFrameEl(page)).toHaveCount(0);
  });
});

test.describe("dcim: header spoofing and cross-tool messages (plan §8)", () => {
  test("client-sent X-Console-*/Authorization from the finch frame are stripped by the gateway", async ({ page }) => {
    const { frame } = await openDcim(page, DCIM.origin, "/racks");
    // If any of these reached otter-go it would answer 401: a bearer next to
    // an assertion, or a garbage/second assertion, is refused there.
    const r = await dcimFetch(frame, `${DCIM_BASE}/api/v1/auth/me`, {
      headers: {
        [ASSERTION_HEADER]: "eyJhbGciOiJub25lIn0.eyJzdWIiOiJldmlsIn0.",
        Authorization: "Bearer dcim_spoofed",
        "X-Console-User": "evil",
        "X-Console-Roles": "dcim:admin",
        "X-Console-Scope": "site=evil",
      },
    });
    expect(r.status, r.body).toBe(200);
    const me = JSON.parse(r.body) as { auth_mode: string; capabilities: string[] };
    expect(me.auth_mode).toBe("console");
    expect(me.capabilities, "no admin wildcard from a spoofed role header").not.toContain("*");
  });

  test("a message from another tool origin (evil) does not drive finch", async ({ page }) => {
    const { frame } = await openDcim(page, DCIM.origin, "/racks");
    await expect.poll(() => new URL(frame.url()).pathname).toBe(`${DCIM_BASE}/racks`);
    // A real document on the evil tool origin, framed next to finch in the
    // shell, posts Navigate + ScopeChanged at the finch frame.
    await page.route(`${ORIGINS.evil}/e2e-dcim-attacker`, (r) =>
      r.fulfill({
        status: 200,
        contentType: "text/html",
        body: `<!doctype html><title>attacker</title><script>
          const t = []; for (let i = 0; i < parent.length; i++) if (parent[i] !== window) t.push(parent[i]);
          let n = 0;
          for (const w of t) for (const m of [{type:"Navigate",path:"/ipam"},{type:"ScopeChanged",scope:{site:"east"}}]) { w.postMessage(JSON.stringify(m), "*"); w.postMessage(m, "*"); n++; }
          document.title = "sent:" + n;
        </script>`,
      }),
    );
    await page.evaluate((src) => {
      const f = document.createElement("iframe");
      f.src = src;
      f.id = "e2e-attacker";
      document.body.appendChild(f);
    }, `${ORIGINS.evil}/e2e-dcim-attacker`);
    const attacker = await toolFrameHandle(page, ORIGINS.evil);
    await expect.poll(() => attacker.title()).toMatch(/^sent:[1-9]/);
    // Self-sent spoof inside finch (origin = tool origin, source != parent).
    await frame.evaluate(() => window.postMessage(JSON.stringify({ type: "Navigate", path: "/ipam" }), "*"));
    await page.waitForTimeout(1500);
    expect(new URL(frame.url()).pathname, "finch did not navigate").toBe(`${DCIM_BASE}/racks`);
    await expect(page).toHaveURL(/\/tools\/dcim\/racks\/?(\?|$)/);
  });
});
