import { expect } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { DEMO, TID, flashbar } from "./fixtures";
import { SID_COOKIE, USERS, expectAxeClean, getSession } from "./auth-helpers";
import { dbNow, sessionRef, toolAuditSince, toolSessionCount } from "./db-helpers";
import {
  HELLO_TID,
  TOOL_TID,
  consoleSid,
  csrf,
  IFRAME_LAUNCH,
  launch,
  openTool,
  rawRequest,
  toolFrame,
  toolFrameEl,
  toolSessionFor,
  toolTest as test,
} from "./tool-helpers";

// M4 tool integration in the shell (plan §8 tools.spec.ts). Authenticated
// projects (console_user: console-user + hello:user). The hello tool lives on
// its own origin (ORIGINS.hello) served by the main gateway.

test.describe("service menu and launch", () => {
  test("/api/tools lists permitted tools only, no upstream, no-store", async ({ request }) => {
    const r = await request.get("/api/tools", { headers: { Accept: "application/json" } });
    expect(r.status()).toBe(200);
    expect(r.headers()["cache-control"]).toContain("no-store");
    const body = await r.json();
    const ids = (body.tools as { id: string }[]).map((t) => t.id);
    expect(ids).toEqual(expect.arrayContaining(["hello", "evil"]));
    expect(ids).not.toContain("denied");
    const hello = body.tools.find((t: { id: string }) => t.id === "hello");
    expect(hello).toMatchObject({ title: "Hello Tool", origin: ORIGINS.hello, entry: "/", scopes: ["site", "tenant"] });
    expect(JSON.stringify(body)).not.toMatch(/upstream|127\.0\.0\.1:\d+|forward_/);
  });

  test("launch via the service menu: tool origin iframe, identity verified by the tool matches the user", async ({ page }) => {
    await page.goto("/");
    await page.getByTestId(TID.serviceMenuButton).click();
    const panel = page.getByTestId(TID.serviceMenuPanel);
    await expect(panel.getByTestId("service-menu-item-tool-denied")).toHaveCount(0);
    await expect(panel.getByTestId("service-menu-item-tool-hello")).toHaveAttribute("href", "/tools/hello/");
    const launched = page.waitForResponse((r) => r.url().endsWith("/api/tools/hello/launch") && r.request().method() === "POST");
    await panel.getByTestId("service-menu-item-tool-hello").click();
    expect((await launched).status()).toBe(200);
    await expect(page).toHaveURL(/\/tools\/hello\/?$/);

    const el = toolFrameEl(page);
    await expect(el).toHaveAttribute("src", new RegExp(`^${ORIGINS.hello}/`));
    await expect(el).toHaveAttribute("title", /Hello Tool/);
    const sandbox = (await el.getAttribute("sandbox"))!.split(/\s+/).sort();
    expect(sandbox).toEqual(["allow-downloads", "allow-forms", "allow-modals", "allow-popups", "allow-same-origin", "allow-scripts"]);
    expect(sandbox.join(" ")).not.toMatch(/top-navigation|escape-sandbox/);

    const f = toolFrame(page);
    await expect(f.getByTestId(HELLO_TID.connection)).toHaveAttribute("data-connected", "true", { timeout: 20_000 });
    await expect(f.getByTestId(HELLO_TID.verified)).toHaveText("yes");
    const s = await (await getSession(page.request)).json();
    await expect(f.getByTestId(HELLO_TID.username)).toHaveText(s.user.preferred_username ?? USERS.user.username);
    await expect(f.getByTestId(HELLO_TID.name)).toHaveText(USERS.user.name);
    await expect(f.getByTestId(HELLO_TID.aud)).toHaveText("hello");
    await expect(f.getByTestId(HELLO_TID.iss)).toHaveText(ORIGINS.main);
    // forward_roles = ["hello:"]: only hello roles reach the tool (AC-6).
    await expect(f.getByTestId(HELLO_TID.roles)).toContainText("hello:user");
    await expect(f.getByTestId(HELLO_TID.roles)).not.toContainText("console-user");
  });

  test("deep link + reload relaunches at the same tool path; in-tool nav updates the shell URL", async ({ page }) => {
    const f = await openTool(page, "hello", "/headers");
    await expect(f.getByTestId(HELLO_TID.root)).toHaveAttribute("data-page", "/headers");
    await page.reload();
    await expect(toolFrame(page).getByTestId(HELLO_TID.root)).toHaveAttribute("data-page", "/headers", { timeout: 20_000 });
    await toolFrame(page).getByTestId("hello-nav-overview").click();
    await expect(page).toHaveURL(/\/tools\/hello\/?$/);
    await expect(toolFrame(page).getByTestId(HELLO_TID.root)).toHaveAttribute("data-page", "/");
  });

  test("breadcrumbs and title follow the tool; Notify reaches the shell flashbar", async ({ page }) => {
    const f = await openTool(page, "hello", "/headers");
    const crumbs = page.getByTestId(TID.breadcrumbs);
    await expect(crumbs).toContainText("Hello Tool");
    await expect(crumbs).toContainText("Request headers");
    await expect(crumbs.getByRole("link", { name: "Overview" })).toHaveAttribute("href", "/tools/hello/");
    await expect(page).toHaveTitle(/^Request headers - Hello Tool - .*Console$/);
    await f.getByTestId("hello-nav-overview").click();
    await f.getByTestId(HELLO_TID.notify).click();
    await expect(flashbar(page)).toContainText("Hello from Hello Tool");
  });

  test("scope and theme propagate without reloading the tool; ctx reaches the server", async ({ page }) => {
    const f = await openTool(page, "hello", "/");
    const visits = await f.getByTestId(HELLO_TID.visits).textContent();
    await page.getByTestId(TID.site).selectOption(DEMO.multiSite);
    await expect(f.getByTestId(HELLO_TID.site)).toHaveText(DEMO.multiSite);
    await expect(f.getByTestId(HELLO_TID.ctx)).toContainText(`"site":"${DEMO.multiSite}"`);
    await page.getByTestId(TID.tenant).selectOption(DEMO.sharedTenant);
    await expect(f.getByTestId(HELLO_TID.tenant)).toHaveText(DEMO.sharedTenant);
    const before = (await page.locator("html").getAttribute("data-theme")) ?? "light";
    const after = before === "dark" ? "light" : "dark";
    // The toggle cycles (system -> light -> dark); click until the shell flips.
    for (let i = 0; i < 3 && (await page.locator("html").getAttribute("data-theme")) !== after; i++) {
      await page.getByTestId(TID.themeToggle).click();
    }
    await expect(page.locator("html")).toHaveAttribute("data-theme", after);
    await expect(f.getByTestId(HELLO_TID.theme)).toHaveText(after);
    await expect(f.locator("html")).toHaveAttribute("data-theme", after);
    // Same document: the localStorage visit counter did not move (no reload).
    await expect(f.getByTestId(HELLO_TID.visits)).toHaveText(visits ?? "");
  });

  test("tool POST with audit_writes is audited; navigation and launch are audited", async ({ page, context }) => {
    const since = dbNow();
    const f = await openTool(page, "hello", "/");
    const ref = sessionRef(await consoleSid(context));
    await f.getByTestId(HELLO_TID.counterInc).click();
    await expect(f.getByTestId(HELLO_TID.counterValue)).toContainText(/Count: \d+/);
    await expect
      .poll(() => toolAuditSince(since, `session_ref = '${ref}' AND tool_id = 'hello'`).map((r) => r.event_type))
      .toEqual(expect.arrayContaining(["tool.launch", "tool.access", "tool.request"]));
    const rows = toolAuditSince(since, `session_ref = '${ref}' AND tool_id = 'hello'`);
    const req = rows.find((r) => r.event_type === "tool.request")!;
    expect(req.path).toBe("/t/hello/api/counter");
    expect(req.http_status).toBe("200");
    for (const r of rows) expect(r.path, "no query in audit path").not.toContain("?");
  });
});

test.describe("authorization", () => {
  // Fix round 1, INT-009: a registered tool the user may not use is
  // indistinguishable from an unknown id (launch API and deep link), the role
  // check runs before the body is parsed, and tool.denied is still audited.
  test("role-denied tool: hidden, launch 404 not_found like an unknown id, deep link 'Tool not found', audited", async ({ page, context }) => {
    const since = dbNow();
    const ref = sessionRef(await consoleSid(context));
    const denied = await launch(page.request, "denied");
    const unknown = await launch(page.request, "nope");
    expect(denied.status()).toBe(404);
    expect(unknown.status()).toBe(404);
    const dj = await denied.json();
    expect(dj).toMatchObject({ error: "not_found" });
    expect(dj).toEqual(await unknown.json());
    // Permission before parsing: a malformed body still answers 404, not 400.
    const tok = await csrf(page.request);
    const bad = await page.request.post(`${ORIGINS.main}/api/tools/denied/launch`, {
      headers: { Accept: "application/json", Origin: ORIGINS.main, "X-CSRF-Token": tok, "Content-Type": "application/json" },
      data: "{not json",
    });
    expect(bad.status(), "malformed body for a hidden tool").toBe(404);
    await page.goto("/tools/denied/");
    const err = page.getByTestId(TOOL_TID.notFound);
    await expect(err).toBeVisible();
    await expect(page.getByRole("heading", { name: /tool not found/i })).toBeVisible();
    await expect(page.getByTestId(TOOL_TID.forbidden)).toHaveCount(0);
    await expect(page.locator("body")).not.toContainText(/Denied Tool/);
    await expect(toolFrameEl(page)).toHaveCount(0);
    await expect
      .poll(() => toolAuditSince(since, `session_ref = '${ref}' AND tool_id = 'denied'`).map((x) => `${x.event_type}:${x.outcome}:${x.reason}`))
      .toEqual(expect.arrayContaining([expect.stringMatching(/^tool\.denied:(denied|failure):missing_role$/)]));
  });

  test("unknown tool id -> 'Tool not found' panel; launch 404", async ({ page }) => {
    expect((await launch(page.request, "nope")).status()).toBe(404);
    await page.goto("/tools/nope/");
    await expect(page.getByTestId(TOOL_TID.notFound)).toBeVisible();
    await expect(page.getByRole("heading", { name: /tool not found/i })).toBeVisible();
  });
});

test.describe("tool sessions and launch codes (SC-23)", () => {
  test("launch code is single use; tool cookie is host-only __Host-, Secure, HttpOnly", async ({ request }) => {
    const lr = await launch(request, "hello", "/headers?x=1");
    expect(lr.status()).toBe(200);
    const { url } = await lr.json();
    const u = new URL(url);
    expect(u.origin).toBe(ORIGINS.hello);
    expect(u.pathname).toBe("/_console/launch");
    const first = await rawRequest(url, { headers: { Accept: "text/html", ...IFRAME_LAUNCH } });
    expect(first.status).toBe(303);
    expect(first.headers["location"]).toBe("/t/hello/headers?x=1");
    expect(first.headers["referrer-policy"]).toBe("no-referrer");
    const sc = ([] as string[]).concat(first.headers["set-cookie"] ?? []).find((c) => c.startsWith("__Host-tool_sid="))!;
    expect(sc).toMatch(/;\s*Secure/i);
    expect(sc).toMatch(/;\s*HttpOnly/i);
    expect(sc).toMatch(/;\s*SameSite=Strict/i);
    expect(sc).toMatch(/;\s*Path=\//i);
    expect(sc).not.toMatch(/;\s*Domain=/i);
    const again = await rawRequest(url, { headers: { Accept: "application/json", ...IFRAME_LAUNCH } });
    expect(again.status).toBe(401);
    expect(JSON.parse(again.body)).toMatchObject({ error: "unauthenticated" });
    expect(JSON.parse(again.body).login_url).toBeUndefined();
  });

  test("a code redeemed at the wrong tool origin is refused", async ({ request }) => {
    const { url } = await (await launch(request, "hello")).json();
    const wrong = url.replace(ORIGINS.hello, ORIGINS.evil);
    expect((await rawRequest(wrong, { headers: { Accept: "application/json", ...IFRAME_LAUNCH } })).status).toBe(401);
  });

  test.describe("own session", () => {
    test("tool cookie dies when the console session ends (logout cascades)", async ({ page, baseURL, context }) => {
      const sid = await consoleSid(context);
      const { sid: toolSid } = await toolSessionFor(page.request, "hello");
      expect(toolSessionCount(sid)).toBeGreaterThanOrEqual(1);
      const who = () =>
        rawRequest(`${ORIGINS.hello}/t/hello/api/whoami`, { headers: { Accept: "application/json", Cookie: `__Host-tool_sid=${toolSid}` } });
      expect((await who()).status).toBe(200);
      const tok = await csrf(page.request);
      const out = await page.request.post("/logout", { headers: { Accept: "application/json", Origin: baseURL!, "X-CSRF-Token": tok } });
      expect(out.status()).toBe(200);
      const after = await who();
      expect(after.status).toBe(401);
      expect(toolSessionCount(sid)).toBe(0);
    });
  });

  test("WebSocket upgrade on a tool origin -> 501 not_implemented (M4)", async ({ request }) => {
    const { sid } = await toolSessionFor(request, "hello");
    const r = await rawRequest(`${ORIGINS.hello}/t/hello/ws`, {
      headers: {
        Accept: "application/json",
        Cookie: `__Host-tool_sid=${sid}`,
        Connection: "Upgrade",
        Upgrade: "websocket",
        "Sec-WebSocket-Version": "13",
        "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
      },
    });
    expect(r.status).toBe(501);
    expect(JSON.parse(r.body)).toMatchObject({ error: "not_implemented" });
  });
});

test.describe("identity header spoofing never reaches the upstream (SEC-010, POAM-026)", () => {
  const SPOOF: Record<string, string> = {
    "X-Console-User": "admin",
    "X-Console-Roles": "console-admin",
    "X-Console-Assertion": "eyJhbGciOiJub25lIn0.eyJzdWIiOiJhZG1pbiJ9.",
    "X-Console-Scope": "site=evil",
    Authorization: "Bearer forged",
    Forwarded: "for=1.2.3.4;host=evil.example",
    "X-Forwarded-User": "admin",
    "X-Remote-User": "admin",
    "X-Real-IP": "1.2.3.4",
    "Proxy-Authorization": "Basic Zm9vOmJhcg==",
  };

  test("API echo: every spoofed header is dropped; assertion is the gateway's; console/tool cookies stripped", async ({ request, context }) => {
    const since = dbNow();
    const ref = sessionRef(await consoleSid(context));
    const { sid } = await toolSessionFor(request, "hello");
    const consoleCookie = (await context.cookies()).find((c) => c.name === SID_COOKIE)!.value;
    const r = await rawRequest(`${ORIGINS.hello}/t/hello/api/headers`, {
      headers: {
        ...SPOOF,
        Accept: "application/json",
        Cookie: `__Host-tool_sid=${sid}; ${SID_COOKIE}=${consoleCookie}; __Host-console_auth=x; tool_pref=dark`,
      },
    });
    expect(r.status).toBe(200);
    const got = new Map((JSON.parse(r.body).headers as { name: string; value: string }[]).map((h) => [h.name, h.value]));
    for (const name of ["x-console-user", "x-console-roles", "x-console-scope", "authorization", "forwarded", "x-forwarded-user", "x-remote-user", "x-real-ip", "proxy-authorization"]) {
      expect(got.has(name), `${name} reached the upstream`).toBe(false);
    }
    expect(got.get("x-console-assertion")).toBe("<verified>");
    const cookies = got.get("cookie") ?? "";
    expect(cookies).not.toContain("__Host-console");
    expect(cookies).not.toContain("__Host-tool_sid");
    expect(cookies).toContain("tool_pref");
    expect(got.get("x-forwarded-proto")).toBe("https");
    expect(got.get("x-forwarded-for") ?? "").not.toContain("1.2.3.4");
    await expect
      .poll(() => toolAuditSince(since, `session_ref = '${ref}' AND reason = 'identity_header_stripped'`).length)
      .toBeGreaterThanOrEqual(1);
  });

  test("browser: spoofed headers on every request still show only the verified identity", async ({ browser, baseURL, context }) => {
    const { ["Proxy-Authorization"]: _drop, ...browserSpoof } = SPOOF; // Chromium refuses to send it
    const ctx = await browser.newContext({
      baseURL: baseURL!,
      ignoreHTTPSErrors: true,
      storageState: await context.storageState(),
      extraHTTPHeaders: { ...browserSpoof, "X-Forwarded-For": "198.18.7.7" },
    });
    try {
      const page = await ctx.newPage();
      const f = await openTool(page, "hello", "/headers");
      const table = f.getByTestId(HELLO_TID.headersTable);
      await expect(table).toBeVisible();
      for (const name of ["x-console-user", "authorization", "forwarded", "x-remote-user", "x-forwarded-user"]) {
        await expect(f.getByTestId(`hello-header-${name}`)).toHaveCount(0);
      }
      await expect(f.getByTestId("hello-header-x-console-assertion")).toContainText("<verified>");
      await f.getByTestId("hello-nav-overview").click();
      await expect(f.getByTestId(HELLO_TID.username)).toHaveText(USERS.user.username);
    } finally {
      await ctx.close();
    }
  });
});

test.describe("axe on tool container states (chromium)", () => {
  test("loaded, hidden-tool and unknown-tool (404) states have no serious/critical violations", async ({ page, browserName }) => {
    test.skip(browserName !== "chromium", "axe scans run in chromium only");
    await openTool(page, "hello", "/");
    await expectAxeClean(page, "tool container: hello loaded");
    await page.goto("/tools/denied/");
    await expect(page.getByTestId(TOOL_TID.notFound)).toBeVisible();
    await expectAxeClean(page, "tool container: 404 (no role)");
    await page.goto("/tools/nope/");
    await expect(page.getByTestId(TOOL_TID.notFound)).toBeVisible();
    await expectAxeClean(page, "tool container: 404");
  });

  test("loading state is aria-busy and axe-clean", async ({ page, browserName }) => {
    test.skip(browserName !== "chromium", "axe scans run in chromium only");
    // Hold the launch response so the skeleton stays up.
    let release: () => void = () => {};
    const held = new Promise<void>((r) => (release = r));
    await page.route("**/api/tools/hello/launch", async (route) => {
      await held;
      await route.continue();
    });
    await page.goto("/tools/hello/");
    const loading = page.getByTestId(TOOL_TID.loading).first();
    await expect(loading).toBeVisible();
    await expect(page.locator("[aria-busy='true']")).not.toHaveCount(0);
    await expectAxeClean(page, "tool container: loading");
    release();
    await expect(toolFrame(page).getByTestId(HELLO_TID.connection)).toHaveAttribute("data-connected", "true", { timeout: 20_000 });
  });
});
