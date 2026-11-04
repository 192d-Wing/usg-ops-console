import { expect, test } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { NO_AUTH } from "./auth-helpers";
import { auditSince, dbNow } from "./db-helpers";

// Security NIT-1 / SC-5 / AU-3: the gateway on ORIGINS.rlUntrusted trusts no
// proxy (CONSOLE_TRUSTED_PROXIES empty), so its loopback peer is untrusted.
// A client rotating a spoofed X-Forwarded-For per request must still be keyed
// by its real peer address: the /login limiter trips, and the audit row records
// the peer (127.0.0.1), never a spoofed value.
test.use({ storageState: NO_AUTH });

test("spoofed X-Forwarded-For from an untrusted peer is ignored by limiter and audit", async ({ request, browserName }) => {
  test.skip(browserName !== "chromium", "limiter is per gateway, run once");
  const since = dbNow();
  let limited: import("@playwright/test").APIResponse | undefined;
  for (let i = 0; i < 40 && !limited; i++) {
    const r = await request.get(`${ORIGINS.rlUntrusted}/login`, {
      headers: { Accept: "application/json", "X-Forwarded-For": `203.0.113.${i + 1}` },
      maxRedirects: 0,
    });
    if (r.status() === 429) limited = r;
  }
  expect(limited, "rotating spoofed XFF evaded the limiter (XFF honoured from untrusted peer)").toBeTruthy();
  expect(await limited!.json()).toEqual({ error: "rate_limited" });
  await expect
    .poll(() => auditSince(since, "event_type = 'access.denied' AND reason = 'rate_limited' AND client_ip = '127.0.0.1'").length)
    .toBe(1);
  // No spoofed value may reach the audit trail as the source address.
  expect(auditSince(since, "client_ip << '203.0.113.0/24'::inet")).toEqual([]);
});
