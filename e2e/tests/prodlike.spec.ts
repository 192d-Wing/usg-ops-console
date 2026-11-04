import { expect, test } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { NO_AUTH, SID_COOKIE, USERS, loginAt } from "./auth-helpers";
import { HELLO_TID, openTool, toolFrameHandle } from "./tool-helpers";

// CI-only `prodlike-chromium` project (E2E_PRODLIKE=1, /etc/hosts maps
// console.ops.test, hello.tools.ops.test, evil.tools.ops.test to 127.0.0.1).
// Real sibling hosts under one registrable domain, unlike the port-distinct
// localhost origins of the default projects (plan §0 "Known dev-only gap").
test.use({ storageState: NO_AUTH });

test("same-site tool cookie works inside the iframe on sibling hosts", async ({ page, baseURL }) => {
  await loginAt(page, baseURL!, "/");
  const f = await openTool(page, "hello", "/");
  await expect(f.getByTestId(HELLO_TID.verified)).toHaveText("yes");
  await expect(f.getByTestId(HELLO_TID.username)).toHaveText(USERS.user.username);
  await expect(f.getByTestId(HELLO_TID.iss)).toHaveText(ORIGINS.prodConsole);
  const toolCookies = await page.context().cookies(ORIGINS.prodHello);
  const tc = toolCookies.find((c) => c.name === "__Host-tool_sid");
  expect(tc?.domain).toBe("hello.tools.ops.test"); // host-only
});

test("cookie tossing from a tool host cannot shadow or replace the console session", async ({ page, baseURL }) => {
  await loginAt(page, baseURL!, "/");
  const consoleHost = new URL(ORIGINS.prodConsole).hostname;
  const sidBefore = (await page.context().cookies(ORIGINS.prodConsole)).find((c) => c.name === SID_COOKIE)!.value;
  await page.goto("/tools/evil/");
  const evil = await toolFrameHandle(page, ORIGINS.prodEvil);
  await evil.evaluate(() => {
    // __Host- with Domain is rejected by the browser; the plain names are
    // accepted for the parent domain and must not be honoured by the console.
    document.cookie = "__Host-console_sid=tossed; Secure; Path=/; Domain=ops.test";
    document.cookie = "__Host-console_sid=tossed; Secure; Path=/";
    document.cookie = "console_sid=tossed; Secure; Path=/; Domain=ops.test";
    document.cookie = "__Host-tool_sid=tossed; Secure; Path=/; Domain=ops.test";
  });
  const consoleCookies = await page.context().cookies(ORIGINS.prodConsole);
  const sids = consoleCookies.filter((c) => c.name === SID_COOKIE);
  expect(sids, JSON.stringify(sids)).toHaveLength(1);
  expect(sids[0]!.value).toBe(sidBefore);
  expect(sids[0]!.domain).toBe(consoleHost);
  // In-page fetch: the Node side may not resolve *.ops.test locally.
  expect(await page.evaluate(async () => (await fetch("/api/session", { headers: { Accept: "application/json" } })).status)).toBe(200);
  // The hello tool still gets its own, untouched tool session.
  const f = await openTool(page, "hello", "/");
  await expect(f.getByTestId(HELLO_TID.verified)).toHaveText("yes");
});
