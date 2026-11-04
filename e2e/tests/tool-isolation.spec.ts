import { expect, type Page } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { flashbar } from "./fixtures";
import { dbNow, toolAuditSince, toolSessionCount } from "./db-helpers";
import {
  HELLO_TID,
  IFRAME_LAUNCH,
  TOOL_SESSION_COOKIE,
  TOOL_TID,
  consoleSid,
  launch,
  openTool,
  rawRequest,
  toolFrame,
  toolFrameHandle,
  toolSessionFor,
  toolSidFrom,
  toolTest as test,
} from "./tool-helpers";

// Fix round 1 (M4 review): SEC-001 launch-code context binding, INT-005 tool
// sessions per tab, SEC-008 tool A -> tool B isolation, NIST-007 postMessage
// from origins other than the mounted tool's registry origin.

const whoami = (origin: string, id: string, sid: string) =>
  rawRequest(`${origin}/t/${id}/api/whoami`, { headers: { Accept: "application/json", Cookie: `${TOOL_SESSION_COOKIE}=${sid}` } });

test.describe("SEC-001: launch code only redeems inside the console's tool iframe", () => {
  test("top-level navigation to a fresh launch URL -> 401, no tool cookie, code kept, audited launch_context", async ({ page }) => {
    const since = dbNow();
    const lr = await launch(page.request, "hello");
    expect(lr.status()).toBe(200);
    const { url } = (await lr.json()) as { url: string };

    // The attack: a victim clicks a link to an attacker-minted launch URL.
    const nav = await page.goto(url);
    expect(nav, "top-level navigation response").toBeTruthy();
    expect(nav!.status()).toBe(401);
    const setCookie = (await nav!.headersArray()).filter((h) => h.name.toLowerCase() === "set-cookie").map((h) => h.value);
    expect(toolSidFrom(setCookie), "Set-Cookie on refused launch").toBeUndefined();
    expect((await page.context().cookies(ORIGINS.hello)).map((c) => c.name)).not.toContain(TOOL_SESSION_COOKIE);

    // Other contexts the gateway must refuse (same code; refusals must not consume it).
    const refused: [string, Record<string, string>][] = [
      ["top-level document, same-site", { "Sec-Fetch-Dest": "document", "Sec-Fetch-Site": "same-site", "Sec-Fetch-Mode": "navigate" }],
      ["iframe, cross-site", { "Sec-Fetch-Dest": "iframe", "Sec-Fetch-Site": "cross-site", "Sec-Fetch-Mode": "navigate" }],
      ["iframe, Sec-Fetch-Site none", { "Sec-Fetch-Dest": "iframe", "Sec-Fetch-Site": "none", "Sec-Fetch-Mode": "navigate" }],
      ["no fetch metadata, no Referer", {}],
      ["no fetch metadata, foreign Referer", { Referer: `${ORIGINS.evil}/t/evil/` }],
    ];
    for (const [label, h] of refused) {
      const r = await rawRequest(url, { headers: { Accept: "text/html", ...h } });
      expect(r.status, label).toBe(401);
      expect(toolSidFrom(r.headers["set-cookie"]), `${label}: no cookie`).toBeUndefined();
    }
    // Not consumed by the refusals: the real iframe context still redeems it once.
    const ok = await rawRequest(url, { headers: { Accept: "text/html", ...IFRAME_LAUNCH } });
    expect(ok.status, "iframe redeem after refusals").toBe(303);
    expect(toolSidFrom(ok.headers["set-cookie"])).toBeTruthy();

    await expect
      .poll(() => toolAuditSince(since, `tool_id = 'hello' AND event_type = 'tool.denied' AND reason = 'launch_context'`).length, {
        message: "tool.denied reason=launch_context audit rows",
      })
      .toBeGreaterThanOrEqual(1 + refused.length);
  });

  test("Referer fallback: no Sec-Fetch-* but a console-origin Referer redeems", async ({ request }) => {
    const { url } = (await (await launch(request, "hello")).json()) as { url: string };
    const r = await rawRequest(url, { headers: { Accept: "text/html", Referer: `${ORIGINS.main}/tools/hello/` } });
    expect(r.status).toBe(303);
    expect(toolSidFrom(r.headers["set-cookie"])).toBeTruthy();
  });
});

test.describe("INT-005: up to 8 tool sessions per console session and tool", () => {
  test("same tool in two tabs: both keep working", async ({ page, context }) => {
    const tab2 = await context.newPage();
    await openTool(page, "hello");
    await openTool(tab2, "hello");
    // Interleave backend calls from both tabs after both launched.
    for (const p of [page, tab2, page, tab2]) {
      const post = p.waitForResponse((r) => r.url() === `${ORIGINS.hello}/t/hello/api/counter` && r.request().method() === "POST");
      await toolFrame(p).getByTestId(HELLO_TID.counterInc).click();
      expect((await post).status(), "counter POST from each tab").toBe(200);
    }
    for (const p of [page, tab2]) await expect(toolFrame(p).getByTestId(HELLO_TID.connection)).toHaveAttribute("data-connected", "true");
    expect(toolSessionCount(await consoleSid(context)), "one row per launch").toBeGreaterThanOrEqual(2);
    await tab2.close();
  });

  test("8 concurrent tool sessions all work; the 9th launch evicts the least recently used", async ({ page, context }) => {
    const sid = await consoleSid(context);
    const sids: string[] = [];
    for (let i = 0; i < 8; i++) sids.push((await toolSessionFor(page.request, "hello")).sid);
    expect(new Set(sids).size).toBe(8);
    for (const [i, s] of sids.entries()) expect((await whoami(ORIGINS.hello, "hello", s)).status, `tool session #${i}`).toBe(200);
    expect(toolSessionCount(sid)).toBe(8);
    const ninth = (await toolSessionFor(page.request, "hello")).sid;
    expect(toolSessionCount(sid), "capped at 8").toBe(8);
    expect((await whoami(ORIGINS.hello, "hello", sids[0]!)).status, "oldest evicted").toBe(401);
    for (const s of [...sids.slice(1), ninth]) expect((await whoami(ORIGINS.hello, "hello", s)).status).toBe(200);
  });
});

test.describe("SEC-008: tool A (evil) cannot reach tool B (hello)", () => {
  test("a tool session is bound to its tool: hello's cookie is refused on the evil origin and vice versa", async ({ request }) => {
    const hello = (await toolSessionFor(request, "hello")).sid;
    const evil = (await toolSessionFor(request, "evil")).sid;
    expect((await whoami(ORIGINS.hello, "hello", hello)).status).toBe(200);
    // Dev origins share the "localhost" cookie jar (SEC-007), so the browser may
    // present either cookie on either origin; the gateway must bind each row to its tool.
    expect((await rawRequest(`${ORIGINS.evil}/t/evil/`, { headers: { Accept: "application/json", Cookie: `${TOOL_SESSION_COOKIE}=${hello}` } })).status).toBe(401);
    expect((await whoami(ORIGINS.hello, "hello", evil)).status).toBe(401);
  });

  test("evil frame: credentialed CORS read of hello blocked, no-cors write refused, hello cannot be framed", async ({ page }) => {
    await openTool(page, "hello");
    await page.goto("/tools/evil/");
    await expect(toolFrame(page).getByTestId("evil-root")).toHaveAttribute("data-ready", "true", { timeout: 20_000 });
    const evil = await toolFrameHandle(page, ORIGINS.evil);
    const helloResponses: number[] = [];
    page.on("response", (r) => {
      if (r.url().startsWith(`${ORIGINS.hello}/t/hello/api/`)) helloResponses.push(r.status());
    });
    const out = await evil.evaluate(async (hello) => {
      const res: Record<string, string> = {};
      try {
        const r = await fetch(`${hello}/t/hello/api/whoami`, { credentials: "include" });
        res.read = `ESCAPED ${r.status} ${await r.text()}`;
      } catch (e) {
        res.read = `blocked: ${(e as Error).name}`;
      }
      try {
        const r = await fetch(`${hello}/t/hello/api/counter`, { method: "POST", mode: "no-cors", credentials: "include" });
        res.write = `sent: ${r.type}`;
      } catch (e) {
        res.write = `blocked: ${(e as Error).name}`;
      }
      res.cookie = document.cookie;
      return res;
    }, ORIGINS.hello);
    expect(out.read).toMatch(/^blocked: TypeError/);
    expect(out.write).toMatch(/^sent: opaque|^blocked/);
    expect(out.cookie, "HttpOnly tool cookie never visible to script").not.toContain(TOOL_SESSION_COOKIE);
    await page.waitForTimeout(500);
    for (const s of helloResponses) expect(s, "hello API reached from evil must not succeed").not.toBe(200);

    // Framing: hello's frame-ancestors pins the console origin.
    await evil.evaluate((hello) => {
      const f = document.createElement("iframe");
      f.src = `${hello}/t/hello/`;
      f.id = "e2e-hello-in-evil";
      document.body.appendChild(f);
    }, ORIGINS.hello);
    await page.waitForTimeout(1500);
    for (const f of page.frames().filter((x) => x.url().startsWith(ORIGINS.hello))) {
      expect(await f.locator(`[data-testid="${HELLO_TID.root}"]`).count().catch(() => 0), "hello rendered inside evil").toBe(0);
    }
  });
});

/**
 * Serve a page at `origin` (Playwright route; never hits a server) that opens
 * the console at `/tools/hello/` in a popup and can post into it.
 */
async function openerAt(page: Page, origin: string) {
  await page.route(`${origin}/__e2e_opener`, (r) =>
    r.fulfill({
      status: 200,
      contentType: "text/html",
      body: `<!doctype html><title>opener</title><button id="open">open</button><script>
        document.getElementById("open").onclick = () => { window.__shell = window.open(${JSON.stringify(`${ORIGINS.main}/tools/hello/`)}, "_blank"); };
      </script>`,
    }),
  );
  await page.goto(`${origin}/__e2e_opener`);
  const popupP = page.context().waitForEvent("page");
  await page.locator("#open").click();
  const shell = await popupP;
  const f = toolFrame(shell);
  await expect(f.getByTestId(HELLO_TID.connection)).toHaveAttribute("data-connected", "true", { timeout: 20_000 });
  const post = (msgs: unknown[]) =>
    page.evaluate(
      ([m, o]) => {
        const w = (window as unknown as { __shell: Window }).__shell;
        for (const x of m as unknown[]) w.postMessage(x, o as string);
        return w.closed;
      },
      [msgs, ORIGINS.main] as const,
    );
  return { shell, post };
}

test.describe("NIST-007: postMessage from any origin other than the mounted tool is ignored", () => {
  for (const [label, origin] of [
    ["foreign (non-registry) origin", "https://foreign.invalid"],
    ["another registered tool origin (evil, not mounted)", ORIGINS.evil],
  ] as const) {
    test(`${label}: tool messages ignored, mounted tool still heard`, async ({ page }) => {
      const { shell, post } = await openerAt(page, origin);
      const url = shell.url();
      const title = await shell.title();
      let keepalives = 0;
      shell.on("request", (r) => {
        if (r.url() === `${ORIGINS.main}/api/session/keepalive`) keepalives++;
      });
      const closed = await post([
        JSON.stringify({ type: "Notify", level: "error", message: "foreign-notify-probe" }),
        JSON.stringify({ type: "SetTitle", title: "foreign-title-probe" }),
        JSON.stringify({ type: "SetBreadcrumbs", items: [["foreign-crumb-probe", "/"]] }),
        JSON.stringify({ type: "NavigateConsole", tool: "evil", path: "/" }),
        JSON.stringify({ type: "Navigate", path: "/foreign-nav-probe" }),
        JSON.stringify({ type: "RequestSessionRefresh" }),
        JSON.stringify({ type: "Ready", protocol_version: 1 }),
      ]);
      expect(closed, "opener still holds the shell window (probe is not vacuous)").toBe(false);
      await shell.waitForTimeout(1000);
      await expect(flashbar(shell)).not.toContainText("foreign-notify-probe");
      await expect(shell.locator("body")).not.toContainText(/foreign-(title|crumb|nav)-probe/);
      expect(await shell.title()).toBe(title);
      expect(shell.url()).toBe(url);
      expect(keepalives, "no keepalive from a foreign RequestSessionRefresh").toBe(0);
      await expect(shell.getByTestId(TOOL_TID.frame)).toHaveAttribute("src", new RegExp(`^${ORIGINS.hello}`));

      // Control: the same message from the mounted tool's frame is honoured.
      const hello = await toolFrameHandle(shell, ORIGINS.hello);
      await hello.evaluate((o) => window.parent.postMessage(JSON.stringify({ type: "Notify", level: "info", message: "mounted-notify-control" }), o), ORIGINS.main);
      await expect(flashbar(shell)).toContainText("mounted-notify-control");
      await shell.close();
    });
  }
});
