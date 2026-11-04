import { expect, type Page } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { flashbar } from "./fixtures";
import { getSession } from "./auth-helpers";
import { TOOL_TID, rawRequest, toolFrame, toolFrameHandle, toolSessionFor, toolTest as test } from "./tool-helpers";

// Plan §0 option (a) + §7 threat table: a hostile tool on its own origin
// cannot reach the shell DOM, navigate the top window, act on the console
// session, or inject markup through SDK messages. The `evil` fixture (WU4,
// examples/hello-tool README "evil" table) performs each attempt and reports
// a result; "ESCAPED" always fails the test.

async function openEvil(page: Page) {
  await page.goto("/tools/evil/");
  const f = toolFrame(page);
  await expect(f.getByTestId("evil-root")).toHaveAttribute("data-ready", "true", { timeout: 20_000 });
  return f;
}

async function run(page: Page, name: string): Promise<string> {
  const f = toolFrame(page);
  await f.getByTestId(`evil-run-${name}`).click();
  const out = f.getByTestId(`evil-result-${name}`);
  await expect(out).not.toBeEmpty();
  await expect(out).not.toHaveText(/^(running|pending)/i);
  const text = (await out.textContent()) ?? "";
  expect(text, `${name}: ${text}`).not.toMatch(/ESCAPED/);
  return text;
}

test.describe("evil tool escape attempts (SC-39, AC-4, SC-18)", () => {
  test("parent DOM, parent storage and frameElement are blocked", async ({ page }) => {
    await openEvil(page);
    expect(await run(page, "parent-dom")).toMatch(/^blocked: .*SecurityError/i);
    expect(await run(page, "parent-storage")).toMatch(/^blocked: .*SecurityError/i);
    expect(await run(page, "frame-element")).toMatch(/^blocked: frameElement is null/);
    // The sandbox attribute on the shell's iframe is untouched.
    await expect(page.getByTestId(TOOL_TID.frame)).toHaveAttribute("sandbox", /allow-scripts/);
  });

  test("top navigation does not move the shell", async ({ page }) => {
    await openEvil(page);
    const before = page.url();
    await run(page, "top-navigation");
    await page.waitForTimeout(500);
    expect(page.url()).toBe(before);
    await expect(page.getByTestId("top-navigation")).toBeVisible();
  });

  test("console API with the console cookie: CORS read blocked, no-cors keepalive and form logout change nothing", async ({ page }) => {
    await openEvil(page);
    const s1 = await (await getSession(page.request)).json();
    expect(await run(page, "api-keepalive-cors")).toMatch(/^blocked: .*TypeError/);
    expect(await run(page, "api-session-read")).toMatch(/^blocked: .*TypeError/);
    const ka = page.waitForResponse((r) => r.url() === `${ORIGINS.main}/api/session/keepalive`).catch(() => null);
    expect(await run(page, "api-keepalive-no-cors")).toMatch(/^sent: opaque|^blocked/);
    const kr = await Promise.race([ka, page.waitForTimeout(3000).then(() => null)]);
    if (kr) expect(kr.status(), "keepalive from tool origin").toBe(403);
    const logout = page.waitForResponse((r) => r.url() === `${ORIGINS.main}/logout`).catch(() => null);
    expect(await run(page, "logout-form")).toMatch(/^submitted|^blocked/);
    const lr = await Promise.race([logout, page.waitForTimeout(3000).then(() => null)]);
    if (lr) expect(lr.status(), "form logout from tool origin").toBe(403);
    const s2 = await getSession(page.request);
    expect(s2.status(), "console session survived").toBe(200);
    expect((await s2.json()).idle_expires_at).toBe(s1.idle_expires_at);
  });

  // Fix round 1, NIST-002: the fixture's own buttons now really post (the
  // WU4 dyn_into bug made every row "blocked: no parent"). Each row must report
  // "sent: N", and the outcome is asserted in the shell. A delivered and
  // displayed probe (evil-html-probe, the first flood items) proves the
  // channel is live, so "ignored" rows are not vacuous.
  test("fixture probes really send; shell ignores junk, renders HTML as text, caps the flood, refuses NavigateConsole", async ({ page }) => {
    await openEvil(page);
    const url = page.url();
    expect(await run(page, "postmessage-shapes")).toBe("sent: 8");
    await page.waitForTimeout(500);
    await expect(flashbar(page)).not.toContainText(/evil-bad-level|evil-object-message|AAAAAAAA/);
    await expect(page.locator('a[href^="javascript:"]')).toHaveCount(0);
    expect(await run(page, "notify-html")).toBe("sent: 1");
    await expect(flashbar(page)).toContainText("evil-html-probe");
    await expect(page.locator("img[data-evil-probe]")).toHaveCount(0);
    expect(await page.title()).not.toBe("pwned");
    expect(await run(page, "notify-flood")).toBe("sent: 20");
    await expect(flashbar(page).getByText(/evil-flood-\d+/).first()).toBeVisible();
    await page.waitForTimeout(500);
    expect(await flashbar(page).getByText(/evil-flood-\d+/).count(), "flood capped").toBeLessThanOrEqual(5);
    expect(await run(page, "navigate-console-bad")).toBe("sent: 1");
    await page.waitForTimeout(500);
    expect(page.url()).toBe(url);
    // evil never sends Ready: the shell must never have posted anything back.
    await expect(toolFrame(page).getByTestId("evil-shell-messages")).toHaveAttribute("data-count", "0");
    await expect(page.getByTestId("top-navigation")).toBeVisible();
  });

  // Same messages posted from the evil frame's realm by the test itself
  // (independent of the fixture implementation).
  test("postMessage junk, HTML notify, notify flood, NavigateConsole to a denied tool", async ({ page }) => {
    await openEvil(page);
    const url = page.url();
    const evil = await toolFrameHandle(page, ORIGINS.evil);
    const post = (msgs: unknown[]) =>
      evil.evaluate(
        ([m, o]) => {
          for (const x of m as unknown[]) window.parent.postMessage(x, o as string);
        },
        [msgs, ORIGINS.main] as const,
      );
    await post([
      "not json",
      "[1,2]",
      JSON.stringify({ type: "Notify" }),
      JSON.stringify({ type: "NoSuchType", message: "x" }),
      JSON.stringify({ type: "SetBreadcrumbs", items: [["click", "javascript:alert(1)"]] }),
      JSON.stringify({ type: "Notify", level: "evil", message: "bad level" }),
      JSON.stringify({ type: "Notify", level: "info", message: "x".repeat(17 * 1024) }),
      { type: "Notify", level: "info", message: "object, not string" },
    ]);
    await page.waitForTimeout(500);
    await expect(flashbar(page)).not.toContainText(/bad level|object, not string|xxxxxxxx/);
    await expect(page.locator('a[href^="javascript:"]')).toHaveCount(0);
    await post([JSON.stringify({ type: "Notify", level: "warning", message: '<img src=x data-evil-probe onerror="document.title=\'pwned\'"> evil-html-probe' })]);
    await expect(flashbar(page)).toContainText("evil-html-probe");
    await expect(page.locator("img[data-evil-probe]")).toHaveCount(0);
    expect(await page.title()).not.toBe("pwned");
    await post(Array.from({ length: 20 }, (_, i) => JSON.stringify({ type: "Notify", level: "info", message: `flood-${i}` })));
    await page.waitForTimeout(500);
    const floods = await flashbar(page).getByText(/^flood-\d+$/).count();
    expect(floods, "flood capped").toBeLessThanOrEqual(5);
    await post([JSON.stringify({ type: "NavigateConsole", tool: "denied", path: "/" })]);
    await page.waitForTimeout(500);
    expect(page.url()).toBe(url);
    await expect(page.getByTestId("top-navigation")).toBeVisible();
  });
});

// Fix round 1 (SEC-002): with no user input in the shell, a tool refresh is
// refused outright (ok:false, no keepalive). The idle-gateway variant with
// real deadlines and the honoured cases live in idle-tool.spec.ts.
test("RequestSessionRefresh spam from a tool without user input: no keepalive, every reply ok:false (AC-11)", async ({ page }) => {
  await openEvil(page);
  let keepalives = 0;
  page.on("request", (r) => {
    if (r.url() === `${ORIGINS.main}/api/session/keepalive` && r.method() === "POST") keepalives++;
  });
  const evil = await toolFrameHandle(page, ORIGINS.evil);
  const replies = await evil.evaluate(async (consoleOrigin) => {
    const got: string[] = [];
    window.addEventListener("message", (e) => {
      if (e.origin === consoleOrigin && typeof e.data === "string" && e.data.includes("SessionRefreshed")) got.push(e.data);
    });
    for (let i = 0; i < 25; i++) window.parent.postMessage(JSON.stringify({ type: "RequestSessionRefresh" }), consoleOrigin);
    await new Promise((r) => setTimeout(r, 3000));
    return got;
  }, ORIGINS.main);
  expect(keepalives, "keepalive POSTs caused by 25 requests").toBe(0);
  expect(replies.filter((m) => JSON.parse(m).ok === true)).toHaveLength(0);
});

test.describe("origin separation", () => {
  test("console origin never serves /t/* or /_console/*", async ({ request }) => {
    for (const p of ["/t/hello/", "/t/hello/api/whoami", "/_console/context"]) {
      const r = await request.get(p, { headers: { Accept: "application/json" }, maxRedirects: 0 });
      expect(r.status(), p).toBe(404);
    }
  });

  test("tool origin without a tool session: 401 page linking to the console, JSON without login_url", async () => {
    const html = await rawRequest(`${ORIGINS.hello}/t/hello/`, { headers: { Accept: "text/html", "Sec-Fetch-Mode": "navigate" } });
    expect(html.status).toBe(401);
    expect(html.body).toContain(`${ORIGINS.main}/tools/hello/`);
    expect(html.headers["location"]).toBeUndefined();
    const json = await rawRequest(`${ORIGINS.hello}/t/hello/api/whoami`, { headers: { Accept: "application/json" } });
    expect(json.status).toBe(401);
    expect(JSON.parse(json.body).login_url).toBeUndefined();
  });

  test("tool origin naming another tool -> 421; frame-ancestors pins the console", async ({ request }) => {
    const { sid } = await toolSessionFor(request, "hello");
    const cookie = { Cookie: `__Host-tool_sid=${sid}`, Accept: "application/json" };
    const other = await rawRequest(`${ORIGINS.hello}/t/evil/`, { headers: cookie });
    expect(other.status).toBe(421);
    expect(JSON.parse(other.body)).toMatchObject({ error: "misdirected_request" });
    const ok = await rawRequest(`${ORIGINS.hello}/t/hello/`, { headers: { ...cookie, Accept: "text/html" } });
    expect(ok.status).toBe(200);
    const csp = ([] as string[]).concat(ok.headers["content-security-policy"] ?? []).join(", ");
    expect(csp).toContain(`frame-ancestors ${ORIGINS.main}`);
    expect(ok.headers["x-content-type-options"]).toBe("nosniff");
    expect(ok.headers["server"]).toBeUndefined();
  });

  test("a foreign page cannot frame a tool origin (clickjacking)", async ({ page, browserName }) => {
    const { sid } = await toolSessionFor(page.request, "hello");
    await page.context().addCookies([{ name: "__Host-tool_sid", value: sid, url: ORIGINS.hello, secure: true, httpOnly: true, sameSite: "Strict" }]);
    // The evil origin is not the console: its document framing hello must be refused.
    await page.route(`${ORIGINS.evil}/__e2e_framer`, (r) =>
      r.fulfill({ status: 200, contentType: "text/html", body: `<!doctype html><iframe id="f" src="${ORIGINS.hello}/t/hello/"></iframe>` }),
    );
    const blocked = page.waitForEvent("console", { predicate: (m) => /frame-ancestors|Refused to (frame|display)/i.test(m.text()), timeout: 10_000 }).catch(() => null);
    await page.goto(`${ORIGINS.evil}/__e2e_framer`);
    const frame = page.frames().find((f) => f.url().startsWith(ORIGINS.hello));
    const loaded = frame ? await frame.locator("[data-testid='hello-root']").count().catch(() => 0) : 0;
    expect(loaded, "tool rendered inside a foreign frame").toBe(0);
    void blocked;
    void browserName;
  });

  test("Content-Security-Policy frame-src of the console lists exactly the registry tool origins", async ({ request }) => {
    const r = await request.get("/", { headers: { Accept: "text/html", "Sec-Fetch-Mode": "navigate" } });
    const csp = r.headers()["content-security-policy"] ?? "";
    const frameSrc = /frame-src ([^;]+)/.exec(csp)?.[1]?.trim().split(/\s+/).sort() ?? [];
    expect(frameSrc).toEqual(expect.arrayContaining([ORIGINS.hello, ORIGINS.evil]));
    expect(frameSrc).not.toContain("'self'");
    expect(frameSrc).not.toContain("*");
    expect(csp).toContain("frame-ancestors 'none'");
  });
});
