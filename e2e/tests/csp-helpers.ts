import type { Page } from "@playwright/test";

/**
 * WebKit-only allowance (F3, run 20261103-163848-m2-shell-chrome,
 * fixes-round-0.md "Orchestrator verification of F3"). Minimal repro with no
 * Yew and no app code: under `style-src 'self'`, `document.createElement('select')`
 * makes WebKit 27.2 fire securitypolicyviolation {style-src-attr, "inline", ""}
 * plus a "Refused to apply a stylesheet" console line. That is engine
 * behaviour, not our markup. Only this exact triple is allowed, only on
 * WebKit, and only up to the number of <select> elements created
 * (rendered selects + one Yew template per page load). Anything else fails.
 */
export const WEBKIT_SELECT_NOISE = { violatedDirective: "style-src-attr", blockedURI: "inline", sample: "" } as const;

/**
 * `target` is the event target's tagName, or null when the target is not an
 * Element (WebKit dispatches at the document for a detached <select>).
 * SEC-002(a): the allowance only applies to SELECT or null targets.
 */
type Violation = { violatedDirective: string; blockedURI: string; sample: string; target: string | null };

const ALLOWED_TARGETS: readonly (string | null)[] = ["SELECT", null];

function isWebkitSelectNoise(v: Violation): boolean {
  return (
    v.violatedDirective === WEBKIT_SELECT_NOISE.violatedDirective &&
    v.blockedURI === WEBKIT_SELECT_NOISE.blockedURI &&
    v.sample === WEBKIT_SELECT_NOISE.sample &&
    ALLOWED_TARGETS.includes(v.target)
  );
}

/** The console text WebKit logs alongside each allowed select violation. */
const WEBKIT_SELECT_CONSOLE = /^Refused to apply a stylesheet because its hash, its nonce, or 'unsafe-inline' does not appear in the style-src directive of the Content Security Policy\.?$/;

export async function watchCsp(page: Page, webkit: boolean): Promise<{ other: string[]; allowedConsole: () => number }> {
  const other: string[] = [];
  let allowedConsole = 0;
  page.on("console", (m) => {
    const t = m.text();
    if (!/content security policy|content-security-policy|refused to/i.test(t)) return;
    if (webkit && WEBKIT_SELECT_CONSOLE.test(t)) allowedConsole++;
    else other.push(`console: ${t}`);
  });
  await page.addInitScript(() => {
    document.addEventListener("securitypolicyviolation", (e) => {
      const w = window as unknown as { __csp?: Violation[] };
      const target = e.target instanceof Element ? e.target.tagName : null;
      (w.__csp ??= []).push({
        violatedDirective: e.violatedDirective,
        blockedURI: e.blockedURI,
        sample: e.sample,
        target,
      });
    });
  });
  return { other, allowedConsole: () => allowedConsole };
}

/** Returns disallowed violations (as strings) and the count of allowed WebKit hits. */
export async function domViolations(page: Page, webkit: boolean): Promise<{ bad: string[]; allowed: number }> {
  const all: Violation[] = await page.evaluate(() => (window as unknown as { __csp?: Violation[] }).__csp ?? []);
  const bad: string[] = [];
  let allowed = 0;
  for (const v of all) {
    if (webkit && isWebkitSelectNoise(v)) allowed++;
    else bad.push(`${v.violatedDirective} ${v.blockedURI} ${JSON.stringify(v.sample)} target=${v.target ?? "null"}`);
  }
  return { bad, allowed };
}

/** Upper bound of allowed hits: rendered selects + one Yew template per load. */
export async function selectBudget(page: Page): Promise<number> {
  return (await page.locator("select").count()) + 1;
}
