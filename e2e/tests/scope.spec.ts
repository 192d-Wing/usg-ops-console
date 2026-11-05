import { expect, test, type Page } from "@playwright/test";
import { DEMO, TID, flashbar, gotoApp, openScope, politeRegion, query } from "./fixtures";

const site = (p: Page) => p.getByTestId(TID.site);
const tenant = (p: Page) => p.getByTestId(TID.tenant);
const env = (p: Page) => p.getByTestId(TID.env);

async function optionValues(sel: ReturnType<typeof site>): Promise<string[]> {
  return sel.locator("option").evaluateAll((os) => os.map((o) => (o as HTMLOptionElement).value));
}

test.describe("deep links", () => {
  test("valid full scope preselects all three pickers", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}&tenant=${DEMO.sharedTenant}&env=${DEMO.sharedEnv}`);
    await expect(site(page)).toHaveValue(DEMO.multiSite);
    await expect(tenant(page)).toHaveValue(DEMO.sharedTenant);
    await expect(env(page)).toHaveValue(DEMO.sharedEnv);
    await expect(flashbar(page)).not.toContainText("Ignored invalid scope");
  });

  test("unknown params and hash are preserved", async ({ page }) => {
    await gotoApp(page, `/?foo=1&site=${DEMO.multiSite}#frag`);
    await expect(site(page)).toHaveValue(DEMO.multiSite);
    const u = new URL(page.url());
    expect(u.searchParams.get("foo")).toBe("1");
    expect(u.hash).toBe("#frag");
  });
});

test.describe("invalid values are dropped, flash names the key only", () => {
  const cases: ReadonlyArray<{ name: string; q: string; marker: string }> = [
    { name: "script payload", q: "site=%3Cscript%3Ealert(1)%3C/script%3E", marker: "alert(1)" },
    { name: "raw angle brackets", q: "site=<img src=x onerror=alert(1)>", marker: "onerror" },
    { name: "wrong case", q: "site=" + DEMO.multiSite.toUpperCase(), marker: DEMO.multiSite.toUpperCase() },
    { name: "duplicate key", q: `site=${DEMO.multiSite}&site=${DEMO.otherSite}`, marker: "&site=" },
    { name: "not in allow-list", q: "site=zzzunknownzzz", marker: "zzzunknownzzz" },
  ];
  for (const c of cases) {
    test(c.name, async ({ page }) => {
      await gotoApp(page, `/?${c.q}`);
      await expect(flashbar(page)).toContainText("Ignored invalid scope in URL: site");
      await expect(site(page)).toHaveValue("");
      await expect.poll(() => query(page)["site"]).toBeUndefined();
      // Only the app DOM: the page shell itself contains Trunk's <script> loader.
      const html = await page.locator("body").innerHTML();
      expect(html).not.toContain(c.marker === "&site=" ? `value="${DEMO.otherSite}" selected` : c.marker);
      expect(await page.title()).not.toContain(c.marker);
    });
  }

  test("empty value is the 'All' state, no warning", async ({ page }) => {
    await gotoApp(page, "/?site=");
    await expect(site(page)).toHaveValue("");
    await expect(flashbar(page)).not.toContainText("Ignored invalid scope");
  });
});

test.describe("cascade: pickers", () => {
  test("tenant disabled with hint until a site is chosen", async ({ page }) => {
    await gotoApp(page, "/");
    await expect(tenant(page)).toBeDisabled();
    await expect(page.getByTestId("scope-tenant-hint")).toHaveText("Select a site first");
    await expect(tenant(page)).toHaveAttribute("aria-describedby", /scope-tenant-hint|\S+/);
    await (await openScope(page, "site")).selectOption(DEMO.multiSite);
    await expect(tenant(page)).toBeEnabled();
  });

  test("tenant list narrows to the selected site; env list narrows to (site, tenant)", async ({ page }) => {
    await gotoApp(page, "/");
    await (await openScope(page, "site")).selectOption(DEMO.multiSite);
    const multiTenants = await optionValues(tenant(page));
    expect(multiTenants[0]).toBe("");
    expect(multiTenants).toContain(DEMO.sharedTenant);
    expect(multiTenants).toContain(DEMO.multiOnlyTenant);

    await (await openScope(page, "site")).selectOption(DEMO.singleTenantSite);
    const single = (await optionValues(tenant(page))).filter((v) => v !== "");
    expect(single).toHaveLength(1);

    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    const other = await optionValues(tenant(page));
    expect(other).toContain(DEMO.sharedTenant);
    expect(other).not.toContain(DEMO.multiOnlyTenant);

    await (await openScope(page, "tenant")).selectOption(DEMO.sharedTenant);
    const envs = await optionValues(env(page));
    expect(envs).toContain(DEMO.sharedEnv);
    expect(envs).not.toContain(DEMO.multiOnlyEnv);
  });

  test("changing site keeps a tenant valid at both sites", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}&tenant=${DEMO.sharedTenant}`);
    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    await expect(tenant(page)).toHaveValue(DEMO.sharedTenant);
    await expect.poll(() => query(page)["tenant"]).toBe(DEMO.sharedTenant);
  });

  test("changing site clears an invalid tenant/env and announces it politely", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}&tenant=${DEMO.multiOnlyTenant}`);
    await expect(tenant(page)).toHaveValue(DEMO.multiOnlyTenant);
    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    await expect(tenant(page)).toHaveValue("");
    await expect(env(page)).toHaveValue("");
    await expect.poll(() => query(page)).toEqual({ site: DEMO.otherSite });
    const announcer = politeRegion(page);
    await expect(announcer).toHaveAttribute("aria-live", "polite");
    await expect(announcer).toHaveText("Tenant cleared: not available at East DC");
  });

  test("identical consecutive reset is re-announced (cleared, then set again)", async ({ page }) => {
    const msg = "Tenant cleared: not available at East DC";
    await gotoApp(page, `/?site=${DEMO.multiSite}&tenant=${DEMO.multiOnlyTenant}`);
    // Record every text state of the live region.
    await politeRegion(page).evaluate((el) => {
      const w = window as unknown as { __ann: string[] };
      w.__ann = [];
      new MutationObserver(() => w.__ann.push(el.textContent ?? "")).observe(el, {
        childList: true,
        characterData: true,
        subtree: true,
      });
    });
    const seen = (): Promise<string[]> =>
      page.evaluate(() => (window as unknown as { __ann: string[] }).__ann.filter((t) => t !== ""));
    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    await expect(politeRegion(page)).toHaveText(msg);
    await expect.poll(seen).toEqual([msg]);
    // Back restores (site, tenant) from the URL without touching the region,
    // so the next reset is textually identical to the current one.
    await page.goBack();
    await expect(tenant(page)).toHaveValue(DEMO.multiOnlyTenant);
    await expect(politeRegion(page)).toHaveText(msg);
    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    await expect.poll(seen).toEqual([msg, msg]);
    await expect(politeRegion(page)).toHaveText(msg);
  });

  test("site field describes that it enables tenant and environment", async ({ page }) => {
    await gotoApp(page, "/");
    await openScope(page, "site");
    const hint = page.getByTestId("scope-site-hint");
    await expect(hint).toHaveText("Selecting a site enables Tenant, then Environment");
    await expect(site(page)).toBeEnabled();
    await expect(site(page)).toHaveAccessibleDescription("Selecting a site enables Tenant, then Environment");
  });

  test("single-tenant site clears tenant and env together", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.otherSite}&tenant=${DEMO.sharedTenant}&env=${DEMO.sharedEnv}`);
    await (await openScope(page, "site")).selectOption(DEMO.singleTenantSite);
    await expect.poll(() => query(page)).toEqual({ site: DEMO.singleTenantSite });
    await expect(politeRegion(page)).toHaveText("Tenant and environment cleared: not available at West DC");
  });

  test("changing site clears an env invalid for the new pair", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}&tenant=${DEMO.sharedTenant}&env=${DEMO.multiOnlyEnv}`);
    await expect(env(page)).toHaveValue(DEMO.multiOnlyEnv);
    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    await expect(tenant(page)).toHaveValue(DEMO.sharedTenant);
    await expect(env(page)).toHaveValue("");
    await expect.poll(() => query(page)["env"]).toBeUndefined();
  });
});

test.describe("cascade: URL relationship validation", () => {
  test("tenant without site is rejected", async ({ page }) => {
    await gotoApp(page, `/?tenant=${DEMO.sharedTenant}`);
    await expect(flashbar(page)).toContainText("tenant");
    await expect.poll(() => query(page)["tenant"]).toBeUndefined();
    await expect(tenant(page)).toHaveValue("");
  });

  test("non-existent (site, tenant) pair is rejected; site kept", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.otherSite}&tenant=${DEMO.multiOnlyTenant}`);
    await expect(flashbar(page)).toContainText("Ignored invalid scope in URL: tenant");
    await expect(site(page)).toHaveValue(DEMO.otherSite);
    await expect.poll(() => query(page)).toEqual({ site: DEMO.otherSite });
  });

  test("invalid env for pair is rejected; site and tenant kept", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.otherSite}&tenant=${DEMO.sharedTenant}&env=${DEMO.multiOnlyEnv}`);
    await expect(flashbar(page)).toContainText("env");
    await expect.poll(() => query(page)).toEqual({ site: DEMO.otherSite, tenant: DEMO.sharedTenant });
  });

  test("lower levels drop when a higher level is invalid", async ({ page }) => {
    await gotoApp(page, `/?site=nope&tenant=${DEMO.sharedTenant}&env=${DEMO.sharedEnv}`);
    await expect(flashbar(page)).toContainText("site");
    await expect.poll(() => query(page)).toEqual({});
    await expect(tenant(page)).toHaveValue("");
    await expect(env(page)).toHaveValue("");
  });

  test("rewrite uses replaceState (no extra history entry)", async ({ page }) => {
    await gotoApp(page, "/");
    const before = await page.evaluate(() => history.length);
    await page.goto(`/?site=nope`);
    await expect.poll(() => query(page)["site"]).toBeUndefined();
    expect(await page.evaluate(() => history.length)).toBe(before + 1);
  });
});

test.describe("cascade: history", () => {
  test("each picker change is one entry; Back restores the prior full scope incl. cascaded clears", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}&tenant=${DEMO.multiOnlyTenant}`);
    const start = await page.evaluate(() => history.length);

    // One change that cascades (tenant cleared) = one history entry.
    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    await expect.poll(() => query(page)).toEqual({ site: DEMO.otherSite });
    expect(await page.evaluate(() => history.length)).toBe(start + 1);

    await page.goBack();
    await expect.poll(() => query(page)).toEqual({ site: DEMO.multiSite, tenant: DEMO.multiOnlyTenant });
    await expect(site(page)).toHaveValue(DEMO.multiSite);
    await expect(tenant(page)).toHaveValue(DEMO.multiOnlyTenant);

    await page.goForward();
    await expect(site(page)).toHaveValue(DEMO.otherSite);
    await expect(tenant(page)).toHaveValue("");
  });

  test("scope survives in-app navigation via side nav", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}`);
    await page.getByTestId(TID.sideNav).getByRole("link", { name: "Component gallery" }).click();
    await expect(page).toHaveURL(/\/gallery\?/);
    expect(query(page)["site"]).toBe(DEMO.multiSite);
  });

  test("scope is never written to localStorage", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}`);
    await (await openScope(page, "site")).selectOption(DEMO.otherSite);
    const dump = await page.evaluate(() => JSON.stringify({ ...window.localStorage }));
    expect(dump).not.toContain(DEMO.otherSite);
    expect(dump).not.toContain(DEMO.multiSite);
  });
});

test.describe("QA: URL edge cases", () => {
  test("oversize query (>2048 bytes) ignores all scope with a fixed message", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}&pad=${"a".repeat(2100)}`);
    await expect(flashbar(page)).toContainText("Ignored invalid scope in URL: query too long");
    await expect(site(page)).toHaveValue("");
  });

  test("more than 32 pairs ignores all scope", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}${"&x=1".repeat(32)}`);
    await expect(flashbar(page)).toContainText("query too long");
    await expect(site(page)).toHaveValue("");
  });

  test("percent-encoded valid id is not decoded", async ({ page }) => {
    const enc = [...DEMO.multiSite].map((c) => "%" + c.charCodeAt(0).toString(16)).join("");
    await gotoApp(page, `/?site=${enc}`);
    await expect(flashbar(page)).toContainText("Ignored invalid scope in URL: site");
    await expect(site(page)).toHaveValue("");
  });

  test("uppercase tenant rejected, site kept", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}&tenant=${DEMO.sharedTenant.toUpperCase()}`);
    await expect(flashbar(page)).toContainText("Ignored invalid scope in URL: tenant");
    await expect(site(page)).toHaveValue(DEMO.multiSite);
    await expect(tenant(page)).toHaveValue("");
    await expect.poll(() => query(page)["tenant"]).toBeUndefined();
  });

  test("repeated tenant key rejected, site kept", async ({ page }) => {
    await gotoApp(page, `/?site=${DEMO.multiSite}&tenant=${DEMO.sharedTenant}&tenant=${DEMO.multiOnlyTenant}`);
    await expect(flashbar(page)).toContainText("Ignored invalid scope in URL: tenant");
    await expect(site(page)).toHaveValue(DEMO.multiSite);
    await expect(tenant(page)).toHaveValue("");
  });
});

// Fix round 2 (A11Y-002): a non-embedded popover closes when focus Tabs out,
// without pulling focus back.
test.describe("popover focus-out (A11Y-002)", () => {
  test.use({ viewport: { width: 1280, height: 800 } });
  test("Tab out of the site popover closes it with aria-expanded=false; focus stays where it went", async ({ page, browserName }) => {
    const tab = browserName === "webkit" ? "Alt+Tab" : "Tab";
    await gotoApp(page, "/");
    const trigger = page.getByTestId("scope-site-trigger");
    const pop = page.getByTestId("scope-site-popover");
    await openScope(page, "site");
    await expect(page.getByTestId(TID.site)).toBeFocused();
    let left = false;
    for (let i = 0; i < 8 && !left; i++) {
      await page.keyboard.press(tab);
      left = await pop.evaluate((el) => !el.contains(document.activeElement));
    }
    expect(left, "focus left the popover").toBe(true);
    await expect(pop).toBeHidden();
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
    const where = await page.evaluate(() => document.activeElement?.getAttribute("data-testid") ?? document.activeElement?.tagName);
    expect(where).not.toBe("BODY");
    expect(where).not.toBe("scope-site-trigger");
  });
});

// Fix round 2 (A11Y-004): segments show an at-rest boundary and chevron;
// the disabled style differs (dashed, no chevron). No shell path disables a
// segment today, so the disabled state is exercised by setting `disabled` on
// the real button from the test (DOM only; production code untouched).
test.describe("scope crumb affordance (A11Y-004)", () => {
  test.use({ viewport: { width: 1280, height: 800 } });
  test("enabled: solid border + visible chevron; disabled: dashed border, no chevron", async ({ page }) => {
    await gotoApp(page, "/");
    const seg = page.getByTestId("scope-site-trigger");
    await page.mouse.move(1, 790);
    const read = () =>
      seg.evaluate((el) => {
        const cs = getComputedStyle(el);
        const chev = el.querySelector(".cui-scope-crumbs__chev");
        const ccs = chev ? getComputedStyle(chev) : null;
        return {
          style: cs.borderTopStyle,
          width: parseFloat(cs.borderTopWidth),
          color: cs.borderTopColor,
          bg: cs.backgroundColor,
          chev: !!chev && ccs!.visibility === "visible" && ccs!.display !== "none" && chev.getBoundingClientRect().width > 0,
        };
      });
    const on = await read();
    expect(on.style, JSON.stringify(on)).toBe("solid");
    expect(on.width).toBeGreaterThanOrEqual(1);
    expect(on.chev, JSON.stringify(on)).toBe(true);
    await seg.evaluate((el) => el.setAttribute("disabled", ""));
    const off = await read();
    expect(off.style, JSON.stringify(off)).toBe("dashed");
    expect(off.chev, JSON.stringify(off)).toBe(false);
    // Border colour is not compared: tokens.css sets --cui-color-border-disabled
    // equal to --cui-color-border (3:1), so the difference is style + fill.
    expect(off.bg).not.toBe(on.bg);
  });
});
