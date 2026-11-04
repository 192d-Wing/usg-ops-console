import https from "node:https";
import { test as base, expect, type APIRequestContext, type BrowserContext, type FrameLocator, type Page } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { NO_AUTH, SID_COOKIE, getSession, loginAt } from "./auth-helpers";

/**
 * M4 tool-integration helpers. Test ids come from the WU3 shell
 * (`tool_host.rs`, `tools.rs`) and the WU4 hello-tool web UI; keep this the
 * single place that names them.
 */
export const TOOL_TID = {
  host: "tool-host",
  frame: "tool-frame",
  loading: "tool-loading",
  error: "tool-error",
  forbidden: "tool-forbidden",
  notFound: "tool-not-found",
  notConnected: "tool-not-connected",
  reload: "tool-reload",
} as const;

export const HELLO_TID = {
  root: "hello-root",
  connection: "hello-connection",
  verified: "hello-verified",
  name: "hello-user-name",
  username: "hello-user-username",
  sub: "hello-user-sub",
  roles: "hello-user-roles",
  aud: "hello-claim-aud",
  iss: "hello-claim-iss",
  ctx: "hello-claim-ctx",
  initUser: "hello-init-user",
  site: "hello-scope-site",
  tenant: "hello-scope-tenant",
  env: "hello-scope-environment",
  theme: "hello-theme",
  visits: "hello-storage-visits",
  notify: "hello-notify",
  counterInc: "hello-counter-increment",
  counterValue: "hello-counter-value",
  headersTable: "hello-headers-table",
} as const;

export const TOOL_SESSION_COOKIE = "__Host-tool_sid";

/**
 * Fetch metadata of the shell's tool iframe loading the launch URL. Fix round
 * 1 (SEC-001): the gateway redeems a launch code only for `Sec-Fetch-Dest:
 * iframe` + `Sec-Fetch-Site: same-site|same-origin` (or, without Sec-Fetch-*,
 * a console-origin Referer); anything else is 401, no cookie, code kept.
 */
export const IFRAME_LAUNCH = { "Sec-Fetch-Dest": "iframe", "Sec-Fetch-Site": "same-site", "Sec-Fetch-Mode": "navigate" } as const;
export const ASSERTION_HEADER = "x-console-assertion";

/** The tool iframe element in the shell. */
export function toolFrameEl(page: Page) {
  return page.getByTestId(TOOL_TID.frame);
}

/** FrameLocator for the tool document. */
export function toolFrame(page: Page): FrameLocator {
  return page.frameLocator(`[data-testid="${TOOL_TID.frame}"]`);
}

/** The tool's Frame object (for evaluate inside the tool origin). */
export async function toolFrameHandle(page: Page, origin: string) {
  await expect
    .poll(() => page.frames().some((f) => f.url().startsWith(origin)), { message: `frame from ${origin}` })
    .toBe(true);
  return page.frames().find((f) => f.url().startsWith(origin))!;
}

/** Open `/tools/<id><path>` and wait until the tool has verified the assertion and connected (Ready/Init done). */
export async function openTool(page: Page, id: string, path = "/") {
  await page.goto(`/tools/${id}${path}`);
  const f = toolFrame(page);
  await expect(f.getByTestId(HELLO_TID.root)).toBeVisible({ timeout: 20_000 });
  await expect(f.getByTestId(HELLO_TID.connection)).toHaveAttribute("data-connected", "true");
  return f;
}

/** CSRF token of the context's console session. */
export async function csrf(req: APIRequestContext): Promise<string> {
  const r = await getSession(req);
  expect(r.status(), "console session").toBe(200);
  return String((await r.json()).csrf_token);
}

/** POST /api/tools/<id>/launch as the shell does (CSRF + Origin). */
export async function launch(req: APIRequestContext, id: string, path?: string) {
  const tok = await csrf(req);
  return req.post(`${ORIGINS.main}/api/tools/${id}/launch`, {
    headers: { Accept: "application/json", Origin: ORIGINS.main, "X-CSRF-Token": tok, "Content-Type": "application/json" },
    data: path === undefined ? {} : { path },
    maxRedirects: 0,
  });
}

/** Minimal raw HTTPS request (Caddy dev CA; full header control incl. Upgrade/Cookie). */
export function rawRequest(
  url: string,
  opts: { method?: string; headers?: Record<string, string>; body?: string } = {},
): Promise<{ status: number; headers: Record<string, string | string[] | undefined>; body: string }> {
  return new Promise((resolve, reject) => {
    const req = https.request(url, { method: opts.method ?? "GET", headers: opts.headers, rejectUnauthorized: false }, (res) => {
      const chunks: Buffer[] = [];
      res.on("data", (c: Buffer) => chunks.push(c));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString() }));
    });
    req.on("error", reject);
    if (opts.body) req.write(opts.body);
    req.end();
  });
}

/** `__Host-tool_sid=<v>` from a Set-Cookie list, or undefined. */
export function toolSidFrom(setCookie: string | string[] | undefined): string | undefined {
  const all = Array.isArray(setCookie) ? setCookie : setCookie ? [setCookie] : [];
  const c = all.find((v) => v.startsWith(`${TOOL_SESSION_COOKIE}=`));
  const v = c?.slice(TOOL_SESSION_COOKIE.length + 1).split(";")[0];
  return v || undefined;
}

/**
 * Launch `id` for the console session in `req` and redeem the code outside
 * the browser. Returns the tool cookie value and the raw launch response.
 */
export async function toolSessionFor(req: APIRequestContext, id: string, path?: string) {
  const lr = await launch(req, id, path);
  expect(lr.status(), `launch ${id}`).toBe(200);
  const { url } = (await lr.json()) as { url: string };
  const redeem = await rawRequest(url, { headers: { Accept: "text/html", ...IFRAME_LAUNCH } });
  expect(redeem.status, "launch code redeem").toBe(303);
  const sid = toolSidFrom(redeem.headers["set-cookie"]);
  expect(sid, "tool session cookie").toBeTruthy();
  return { url, sid: sid!, redeem };
}

/** Console sid cookie value of a browser context. */
export async function consoleSid(context: BrowserContext): Promise<string> {
  const c = (await context.cookies()).find((x) => x.name === SID_COOKIE);
  expect(c, "console sid").toBeTruthy();
  return c!.value;
}

/**
 * Tool specs get their OWN console session per test. The gateway keeps one
 * tool session per (console session, tool) and a new launch replaces it, so
 * parallel tests sharing the setup storageState would revoke each other's
 * tool cookies (by design: one browser = one cookie jar). `request` is the
 * page's request context (same cookies) for API-level checks.
 */
export const toolTest = base.extend<{ signedIn: void }>({
  storageState: async ({}, use) => use(NO_AUTH),
  signedIn: [
    async ({ page, baseURL }, use) => {
      await loginAt(page, baseURL!, "/");
      await use();
    },
    { auto: true },
  ],
  request: async ({ page, signedIn: _ }, use) => use(page.request),
});
