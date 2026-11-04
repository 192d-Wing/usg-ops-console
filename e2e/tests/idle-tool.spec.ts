import { expect, test, type Frame, type Page } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { TID } from "./fixtures";
import { NO_AUTH, getSession, loginVia } from "./auth-helpers";
import { toolFrame, toolFrameHandle } from "./tool-helpers";

// Fix round 1, SEC-002 / NIST-001 (AC-11, strict idle): a tool's
// RequestSessionRefresh is honoured only when the shell saw real user input
// (trusted pointerdown/keydown in the shell, or the user moving focus into the
// tool frame) within the recent-input window, or while the idle-warning
// dialog is open. Otherwise the shell answers SessionRefreshed{ok:false,
// idle_expires_at: current} and never calls keepalive.
//
// Runs in the idle-* projects: IDLE gateway (CONSOLE_SESSION_IDLE_SECS=150,
// warning lead 75 s) with the evil fixture on its own origin
// (registry.idle.toml, ORIGINS.idleEvil). page.clock drives the shell timers.
const IDLE_SECS = 150;
const LEAD_SECS = Math.min(120, IDLE_SECS / 2);
/** Shell recent-input window (SEC-002 decision: 60 s). */
const INPUT_WINDOW_SECS = 60;

test.use({ storageState: NO_AUTH });

type Session = { idle_expires_at: string; absolute_expires_at: string; idle_timeout_secs: number };
type Refreshed = { type: string; ok: boolean; idle_expires_at?: string | number | null };

async function signedInWithEvil(page: Page, baseURL: string) {
  await page.clock.install();
  await loginVia(page, baseURL, "/tools/evil/");
  await expect(page.getByTestId(TID.topNav)).toBeVisible();
  await expect(page.getByTestId("user-menu-loading")).toHaveCount(0);
  await expect(toolFrame(page).getByTestId("evil-root")).toHaveAttribute("data-ready", "true", { timeout: 20_000 });
  const s = (await (await getSession(page.request)).json()) as Session;
  expect(s.idle_timeout_secs).toBe(IDLE_SECS);
  const evil = await toolFrameHandle(page, ORIGINS.idleEvil);
  // Collect SessionRefreshed replies inside the tool realm.
  await evil.evaluate((consoleOrigin) => {
    const w = window as unknown as { __refreshed: string[] };
    w.__refreshed = [];
    window.addEventListener("message", (e) => {
      if (e.origin === consoleOrigin && typeof e.data === "string" && e.data.includes("SessionRefreshed")) w.__refreshed.push(e.data);
    });
  }, baseURL);
  return { s, evil };
}

const spam = (evil: Frame, origin: string, n: number) =>
  evil.evaluate(
    ([o, k]) => {
      for (let i = 0; i < (k as number); i++) window.parent.postMessage(JSON.stringify({ type: "RequestSessionRefresh" }), o as string);
    },
    [origin, n] as const,
  );

const replies = async (evil: Frame) =>
  ((await evil.evaluate(() => (window as unknown as { __refreshed: string[] }).__refreshed)) as string[]).map((m) => JSON.parse(m) as Refreshed);

const sameInstant = (a: string | number | null | undefined, b: string) =>
  typeof a === "number" ? Math.abs(a - Date.parse(b)) < 1000 : Date.parse(String(a)) === Date.parse(b);

test("idle user + tool spamming RequestSessionRefresh: refused, idle_expires_at unchanged, session lapses on schedule", async ({ page, baseURL }) => {
  const { s: s1, evil } = await signedInWithEvil(page, baseURL!);
  let keepalives = 0;
  page.on("request", (r) => {
    if (r.url().endsWith("/api/session/keepalive")) keepalives++;
  });
  // Move past the recent-input window (no input happened at all), then keep
  // spamming every 5 s of shell time until just before the warning dialog.
  await page.clock.runFor((INPUT_WINDOW_SECS + 2) * 1000);
  for (let t = INPUT_WINDOW_SECS + 2; t < IDLE_SECS - LEAD_SECS - 3; t += 5) {
    await spam(evil, baseURL!, 5);
    await page.clock.runFor(5000);
  }
  await expect(page.getByTestId("idle-dialog")).toBeHidden();
  const got = await replies(evil);
  expect(got.filter((r) => r.ok === true), "honoured refreshes without user input").toHaveLength(0);
  expect(got.length, "the shell answers refused requests").toBeGreaterThan(0);
  for (const r of got) {
    expect(r.ok).toBe(false);
    expect(sameInstant(r.idle_expires_at, s1.idle_expires_at), `reply idle_expires_at ${String(r.idle_expires_at)} is the current deadline`).toBe(true);
  }
  expect(keepalives, "keepalive calls caused by the tool").toBe(0);
  const s2 = (await (await getSession(page.request)).json()) as Session;
  expect(s2.idle_expires_at).toBe(s1.idle_expires_at);

  // On schedule: the warning opens at the original lead and the session lapses at the original deadline.
  const login = page.waitForRequest((r) => new URL(r.url()).pathname === "/login" && r.isNavigationRequest());
  await page.clock.runFor(5000);
  await expect(page.getByTestId("idle-dialog")).toBeVisible();
  await page.clock.fastForward((LEAD_SECS + 2) * 1000);
  const u = new URL((await login).url());
  expect(u.searchParams.get("reason")).toBe("expired");
  expect(keepalives).toBe(0);
});

test("recent trusted input in the shell: the tool's refresh is honoured once", async ({ page, baseURL }) => {
  const { s: s1, evil } = await signedInWithEvil(page, baseURL!);
  // Server deadlines have second granularity; let real time pass.
  await new Promise((r) => setTimeout(r, 1100));
  await page.getByTestId(TID.topNav).click({ position: { x: 2, y: 2 } });
  const ka = page.waitForResponse((r) => r.url().endsWith("/api/session/keepalive") && r.request().method() === "POST");
  await spam(evil, baseURL!, 1);
  expect((await ka).status()).toBe(200);
  await expect.poll(async () => (await replies(evil)).some((r) => r.ok === true)).toBe(true);
  const s2 = (await (await getSession(page.request)).json()) as Session;
  expect(Date.parse(s2.idle_expires_at)).toBeGreaterThan(Date.parse(s1.idle_expires_at));
  expect(s2.absolute_expires_at).toBe(s1.absolute_expires_at);
});

test("user clicking into the tool frame counts as input: refresh honoured", async ({ page, baseURL }) => {
  const { s: s1, evil } = await signedInWithEvil(page, baseURL!);
  await new Promise((r) => setTimeout(r, 1100));
  await toolFrame(page).getByTestId("evil-root").click({ position: { x: 4, y: 4 } });
  const ka = page.waitForResponse((r) => r.url().endsWith("/api/session/keepalive") && r.request().method() === "POST");
  await spam(evil, baseURL!, 1);
  expect((await ka).status()).toBe(200);
  const s2 = (await (await getSession(page.request)).json()) as Session;
  expect(Date.parse(s2.idle_expires_at)).toBeGreaterThan(Date.parse(s1.idle_expires_at));
});

test("idle-warning dialog open: the tool's refresh is honoured", async ({ page, baseURL }) => {
  const { s: s1, evil } = await signedInWithEvil(page, baseURL!);
  await page.clock.fastForward((IDLE_SECS - LEAD_SECS + 2) * 1000);
  await expect(page.getByTestId("idle-dialog")).toBeVisible();
  await new Promise((r) => setTimeout(r, 1100));
  const ka = page.waitForResponse((r) => r.url().endsWith("/api/session/keepalive") && r.request().method() === "POST");
  await spam(evil, baseURL!, 1);
  expect((await ka).status()).toBe(200);
  const s2 = (await (await getSession(page.request)).json()) as Session;
  expect(Date.parse(s2.idle_expires_at)).toBeGreaterThan(Date.parse(s1.idle_expires_at));
});

// Fix round 3, SEC-012: a polled frame entry counts only if it is the first
// since load, or a trusted shell pointerdown/keydown happened after the last
// observed frame exit. Here the user used the tool once, returned to the shell
// and went idle; the tool then focuses itself and spams refresh requests.
test("SEC-012: after the user returned to the shell, tool self-focus + refresh spam does not extend the session", async ({ page, baseURL }) => {
  const { evil } = await signedInWithEvil(page, baseURL!);
  // Legit first entry (rule a), then the user returns to the shell.
  await toolFrame(page).getByTestId("evil-root").click({ position: { x: 4, y: 4 } });
  await page.getByTestId(TID.topNav).click({ position: { x: 2, y: 2 } });
  await expect.poll(() => page.evaluate(() => document.activeElement?.tagName !== "IFRAME")).toBe(true);
  // Let any keepalive caused by that real input settle, then snapshot the deadline.
  await page.clock.runFor(2000);
  await page.waitForLoadState("networkidle");
  const s1 = (await (await getSession(page.request)).json()) as Session;
  // Leave the recent-input window with no further input.
  await page.clock.runFor((INPUT_WINDOW_SECS + 2) * 1000);
  let keepalives = 0;
  page.on("request", (r) => {
    if (r.url().endsWith("/api/session/keepalive")) keepalives++;
  });
  await evil.evaluate(() => {
    const b = document.createElement("button");
    b.id = "sec012-self-focus";
    b.textContent = "x";
    document.body.appendChild(b);
  });
  // Self-focus + spam, sampled by the shell's 1 s poll, staying before the warning lead.
  for (let i = 0; i < 2; i++) {
    await evil.evaluate(() => {
      window.focus();
      document.getElementById("sec012-self-focus")?.focus();
    });
    await page.clock.runFor(1500);
    await spam(evil, baseURL!, 5);
    await page.clock.runFor(1500);
  }
  test.info().annotations.push({
    type: "sec012-self-focus-landed",
    description: String(await page.evaluate(() => document.activeElement?.tagName === "IFRAME")),
  });
  await expect(page.getByTestId("idle-dialog")).toBeHidden();
  const got = await replies(evil);
  const after = got.slice(-10);
  expect(after.length, "the shell answers the spam").toBeGreaterThan(0);
  expect(after.filter((r) => r.ok === true), "honoured refreshes after self-focus").toHaveLength(0);
  for (const r of after) expect(sameInstant(r.idle_expires_at, s1.idle_expires_at), `reply ${String(r.idle_expires_at)} is the current deadline`).toBe(true);
  expect(keepalives, "keepalive calls caused by the tool").toBe(0);
  const s2 = (await (await getSession(page.request)).json()) as Session;
  expect(s2.idle_expires_at, "server idle deadline unchanged").toBe(s1.idle_expires_at);

  // On schedule: the session lapses at the unchanged deadline.
  const login = page.waitForRequest((r) => new URL(r.url()).pathname === "/login" && r.isNavigationRequest());
  await page.clock.fastForward((IDLE_SECS + 5) * 1000);
  expect(new URL((await login).url()).searchParams.get("reason")).toBe("expired");
  expect(keepalives).toBe(0);
});
