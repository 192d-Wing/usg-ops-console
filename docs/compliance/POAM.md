# Plan of Action and Milestones (POA&M)

Status as of M3 (OIDC sign-in, sessions, logout, idle timeout, audit),
2026-11-03. Owner and
milestone fields are placeholders until the team assigns them.

Status values used below:

- **Open**: the weakness exists and is not fixed.
- **Pending CI verification**: a fix or test exists and passed locally, but
  the CI job that is meant to prove it has never had a green run. The item
  stays open until a green CI run is recorded here, with its run ID and
  artifacts.
- **Deferred**: accepted for now, with a stated reason.

## Items from `POAM:` markers in code

`POAM:` markers in the repository (as of M3):

| Marker location | Item |
| --- | --- |
| `deny.toml:7` | POAM-001, POAM-002 |
| `deny.toml:11` | POAM-014 |
| `crates/console-shell/src/demo.rs:14` | POAM-010 |
| `crates/console-gateway/migrations/0003_roles_refresh.sql:8` (`-- POAM: AU-9`) | POAM-017 |
| `crates/console-gateway/migrations/0003_roles_refresh.sql:10` (`-- POAM: SC-28`) | POAM-015 |
| `crates/console-gateway/src/session.rs:367` (`// POAM: SC-28`) | POAM-015 |
| `crates/console-gateway/src/audit.rs:134` (`// POAM: AU-5`) | POAM-016 |
| `crates/console-gateway/src/sweeper.rs:39` (`// POAM: AU-5`) | POAM-016 |

| ID | Weakness | Source | Control | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- | --- |
| POAM-001 | RUSTSEC-2024-0370: `proc-macro-error` is unmaintained (not a known vulnerability). It comes in through `yew-macro` and is used at build time only. | `deny.toml` `[advisories] ignore` | RA-5, SI-2 | Open | TBD | TBD (re-check on each yew release) |
| POAM-002 | RUSTSEC-2025-0141: `bincode` 1.x is unmaintained (not a known vulnerability). It comes in through `gloo-worker`. The console does not use yew workers. | `deny.toml` `[advisories] ignore` | RA-5, SI-2 | Open | TBD | TBD (re-check on each yew release) |
| POAM-010 | NIST-004: the service menu list (`SERVICES`) is static demo data and is not filtered by role. There is no access enforcement behind it. Marker text: `// POAM: AC-3/AC-6 - service list is static and unfiltered; server-side role filtering arrives with the M4 registry.` | `crates/console-shell/src/demo.rs:14` | AC-3, AC-6 | Open | TBD | M4 (tool registry) |
| POAM-014 | RUSTSEC-2023-0071 (Marvin attack): `rsa` 0.9, pulled in by `openidconnect`, has no constant-time private-key operations. The gateway only **verifies** IdP signatures with public keys, so the timing side channel does not apply. Ignored with reason in `deny.toml`. | `deny.toml:11` | RA-5, SI-2 | Open | TBD | TBD (re-check when `openidconnect` moves to a patched `rsa`, or if the gateway ever signs or decrypts with RSA) |
| POAM-015 | `sessions.id_token` and `sessions.refresh_token` are stored in plaintext at rest (accepted for M3, brief Q2 and NIST-001). A DB or backup leak exposes tokens with PII claims; the refresh token is usable until the client session expires. | `0003_roles_refresh.sql:10`, `session.rs:367` | SC-28 | Open (accepted for M3) | TBD | M7 (app-level or pgcrypto encryption) |
| POAM-016 | Audit writes for logout, expiry, denials, refresh, revocation and the sweeper are best effort. A DB failure is logged at `error` (`target=audit`, "audit write failed") but no metric or alert is wired, and sweeper batches are not retried. Only `login.success` fails closed. | `audit.rs:134`, `sweeper.rs:39` | AU-5 | Open | TBD | TBD (alert on "audit write failed") |
| POAM-017 | Append-only audit can be bypassed by `console_owner` (assumable through the `console_migrator` login) and by DB superusers: they can disable the triggers or drop `audit_events`. | `0003_roles_refresh.sql:8` | AU-9, AU-9(4), AU-4(1), AU-9(2) | Open | TBD | M7 (external write-once audit store) |

`yew-router` 0.20 also depends on both crates in POAM-001/002. `cargo audit`
reports them as 2 allowed warnings (M2 QA run).

## Verification gaps and deferred findings

### POAM-003 to POAM-006: closed by CI verification (NIST-002)

Closed on 2026-11-03 by CI run
[41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231)
on commit `0a6f40e`, with every job green:

- `wasm-test` (headless Firefox): 16 passed, 0 failed. Artifact `wasm-pack-log`.
- `e2e` (chromium, firefox, webkit, dist-chromium, dist-firefox, dist-webkit):
  147 passed, 24 skipped (axe is Chromium-only by design), 0 failed.
  Artifact `playwright-report`.
- `container build` also succeeded for the first time.

Artifacts are kept for 30 days; the run page and this record are the
durable evidence.

| ID | Weakness | Evidence | Control / reference | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- | --- |
| POAM-003 | wasm-bindgen browser tests in `crates/console-ui/tests/wasm.rs` had never been run. | `make wasm-test WASM_BROWSER=chrome`: 16 passed, 0 failed (local headless Chrome); CI `wasm-test` headless Firefox 16 passed. | SA-11; WCAG 2.1 | Closed — CI run 41740455231, headless Firefox 16 passed | — | M2 |
| POAM-004 | No axe accessibility scan had been run (M1 A11Y-009). | `e2e/tests/a11y.spec.ts`: axe (wcag2a/aa, wcag21a/aa) on `/` and `/gallery`, light and dark, at 1280, 600 and 320 px wide; 0 serious/critical on Chromium. **Closure scope: axe runs on Chromium only.** WebKit and Firefox get keyboard coverage (`keyboard.spec.ts`, `reflow.spec.ts`), not axe (see POAM-012). Keyboard specs pass on Chromium and WebKit locally and on all three engines in CI. | WCAG 2.1 AA | Closed — CI run 41740455231, e2e green on all three engines | — | M2 |
| POAM-005 | M1 A11Y-007: fixed px sizes instead of tokens; checkbox smaller than the target minimum. | Sizes moved to tokens (`--cui-size-control-check` is 24px, plus border-width and component-size tokens). Test `components_css_has_no_raw_px` (`crates/console-ui/tests/qa_logic_css.rs`) passes. The a11y reviewer marked it ready to close. | WCAG 2.1 SC 1.4.4, 2.5.8 | Closed — CI run 41740455231, e2e green on all three engines | — | M2 |
| POAM-006 | M1 A11Y-008: side navigation could not be collapsed on narrow screens. | Below 45rem the nav is hidden behind a `NavToggle` disclosure. `keyboard.spec.ts` "side nav at narrow width" and `reflow.spec.ts` (320x640, no horizontal scroll, dropdown panels inside the viewport) pass on Chromium and WebKit. Also green on Firefox in CI. | WCAG 2.1 SC 1.4.10 | Closed — CI run 41740455231, e2e green on all three engines | — | M2 |

### Other open items

| ID | Weakness | Control / reference | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- |
| POAM-007 | M1 PERF-002 (partial fix): `Table` still runs `item_key` on every item on each render. Selection lookup is O(1); key memoisation is deferred until a real tool uses Table. | Performance | Open (deferred) | TBD | TBD |
| POAM-008 | M1 PERF-003: three render-blocking CSS requests (tokens, components, shell), about 5 KB gz in total. Not changed in M2. | Performance | Open (optional) | TBD | TBD |
| POAM-009 | M1 SEC-003: the shell's theme storage DOM path (`theme::dom::load/apply/save` in `crates/console-shell/src/theme.rs`) has no test. Only the parse allow-list it relies on is tested. Not changed in M2. | SI-10 | Open | TBD | TBD |
| POAM-011 | NIST-003 / SEC-002: known benign CSP reports on WebKit. WebKit reports a `style-src-attr` violation each time an `HTMLSelectElement` is created (reproduced with a bare `document.createElement('select')`, no app code). The E2E tests allow, **on WebKit only**, reports that match exactly `{violatedDirective: "style-src-attr", blockedURI: "inline", sample: ""}` with a target of `SELECT` or `null`, capped at the number of `<select>` elements plus 1 per page (`e2e/tests/csp-helpers.ts`, `csp.spec.ts`, `dist-smoke.spec.ts`). **Residual risk:** a first-party inline-style violation with a `null` (document-level) target on WebKit would also match; it is bounded by the cap and by the non-empty `[style]` DOM check. Adding `'report-sample'` to `style-src` (SEC-002(b)) is deferred. Upstream WebKit bug link: TBD. Any future CSP report collector must filter this triple. | SC-18, SA-11 | Open. **Re-reviewed at M3 (2026-11-03):** the allowance in `e2e/tests/csp-helpers.ts` is unchanged and still needed; the M3 CSP adds only the IdP issuer origin to `form-action`, which does not affect this report. WebKit E2E passed locally with the allowance. | TBD | TBD (expire when WebKit fixes it; re-review at M4, when tool iframes add CSP surface) |
| POAM-012 | A11Y-006: axe scans run on Chromium only. Firefox E2E has not been run locally (M2 or M3); the M3 auth specs have not run on Firefox at all (see POAM-030). | WCAG 2.1 AA; SA-11 | Open (deferred) | TBD | TBD |
| POAM-013 | M2 PERF-002: optional bundle trims (about 1-3 KB gz) in the scope picker (`crates/console-shell/src/chrome.rs`). Skipped in M2 as not simpler. | Performance | Deferred | TBD | TBD (optional) |

## M3 items without a code marker

These come from the M3 review findings (security, nist-800-53, integration,
performance). They have no `POAM:` marker in code because they concern
deployment configuration, external systems, later milestones or
documentation.

| ID | Weakness | Source | Control / reference | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- | --- |
| POAM-018 | Failed-logon lockout (AC-7) is entirely Keycloak's (brute-force detection). The gateway's `/login` limiter counts requests per IP, not failed logons, and `/callback` is not rate-limited. `bruteForceProtected` is verified only in the dev realm export; the production icam setting is unverified. | NIST-008 | AC-7 | Open (evidence to collect from icam realm config) | TBD (icam admins) | TBD |
| POAM-019 | Keepalive depends on the IdP. During a Keycloak outage keepalive returns 503: sessions cannot be extended and users idle out. This fails closed by design but is an availability dependency. | NIST-001 caveat | AC-2, CP-8 | Open (accepted) | TBD | TBD |
| POAM-020 | Only `/login` is rate-limited (per IP, IPv6 by /64, 30/min, 10,000 tracked IPs; global cap of 10,000 pending sign-ins). `/callback`, keepalive and other routes are not. Ingress-level rate limiting is recommended but not configured in this repo. | NIST-008, SEC-009 | SC-5 | Open | TBD | M7 (ingress) |
| POAM-021 | Sign-out sends the id_token as `id_token_hint` in the end-session URL query (GET). It can appear in IdP and ingress access logs. Mitigations in place: `Referrer-Policy: no-referrer`, `location.replace`. Operators must strip the query from end-session access logs; icam id_token lifespan should be at most 5 min. A POST form logout to the IdP would remove the query exposure. | SEC-003 | SC-8, SI-12, AU-9 | Open | TBD | TBD |
| POAM-022 | No attributed WASM size breakdown: the stripped release wasm has no name section, so `twiggy` output is anonymous. Release bundle is 173,207 B gz (+22.0% vs M2's 141,918 B; new baseline 173,207 B). | PERF-001 | Performance | Deferred | TBD | M4 (symbolised profiling build; record top 10) |
| POAM-023 | Keepalive refresh-grant latency is not measured (no span or metric, no p50/p99). | PERF-007 | Performance | Deferred | TBD | M4 |
| POAM-024 | The refresh grant uses the general IdP timeout (5 s connect, 10 s total). A shorter timeout (3-5 s) would let the idle dialog fail fast. | PERF-008 | Performance | Deferred | TBD | M4 |
| POAM-025 | M4 SDK docs must state the full gateway error contract: open `error` values (unknown values must be tolerated), logout XHR detection (`X-CSRF-Token` header or `Accept` containing `application/json`; `Accept: */*` without the header gets a 303), keepalive 503 retryable vs 401 `revoked`, and that tools under `/t/<id>/` may receive `?console_notice=` and must ignore unknown parameters. | INT-007, INT-008 | Integration contract | Open | TBD | M4 (`docs/INTEGRATING-A-TOOL.md`) |
| POAM-026 | The identity-header strip list (`headers.rs`: `x-console-user`, `x-console-roles`, `x-console-session`, `x-forwarded-user`) is narrow. Before the M4 proxy: allow-list upstream headers, drop the session cookie and hop-by-hop headers, and set identity only from the session. | SEC-010 | AC-3 | Open | TBD | M4 (reverse proxy) |
| POAM-027 | The Helm chart must keep `CONSOLE_MIGRATOR_DATABASE_URL` off the long-running gateway: migrations run as a separate `console-gateway migrate` job or init container. When the key is set, `serve` migrates at startup (dev convenience) and the runtime process could assume `console_owner`. | SEC-006 re-verification (e) | AC-6, AU-9 | Open | TBD | M7 (Helm) |
| POAM-028 | The production `CONSOLE_REQUIRED_ACR` value (the `acr` that icam's X.509/CAC flow emits, including on SSO reuse via `acr.loa.map` and a conditional-level flow) must be confirmed with the icam Keycloak admins. The IA-2(12) enforcement in the gateway is only as strong as that mapping. | NIST-002, SEC-002 | IA-2(12), IA-2(1), IA-2(2) | Open | TBD (icam admins) | Before production |
| POAM-029 | The icam client `usg-ops-console` SSO settings must be confirmed with the icam admins: client session idle and max must equal the console values (900 s / 43,200 s by default; idle at least the console idle or keepalive fails, at most so SSO cannot silently outlive the console session), `revokeRefreshToken=false` (or reuse > 0), refresh tokens on. Verified only in the dev realm export. | SEC-001, NIST-001 | AC-11, AC-12 | Open | TBD (icam admins) | Before production |
| POAM-030 | M3 verification gaps: the CI jobs `gateway-integration` and compose-based `e2e` had not run, and Firefox had not run the E2E suite. Closed by CI run [41773314457](https://github.com/192d-Wing/usg-ops-console/actions/runs/41773314457) on commit `d55b641`: all jobs green; `gateway-integration` passed; `e2e` on chromium, firefox and webkit (incl. idle-*, dist-*) 268 passed, 0 failed, 28 skipped (axe Chromium-only), 4 flaky (see POAM-032). Still not covered by E2E: absolute-expiry reauth in the browser, keepalive 503 inline error UI, no-JS form logout through CSP `form-action`, the `stale_auth` rejection (covered by gateway integration tests only). | QA report; CI run 41773314457 | SA-11, CA-2 | Closed — CI run 41773314457 | — | M3 |
| POAM-031 | The expired-session sweeper applies the idle limit configured on the gateway that runs it, not the limit in force when each session was created. Replicas that share one database but disagree on `CONSOLE_IDLE_TIMEOUT_SECS` (for example during a rolling deploy that shortens it) let the shortest limit end other replicas' sessions early. Found in M3 round 4: the E2E idle-test gateway (150 s) swept the main suite's sessions. Fix: store the idle limit (or idle deadline) per session row and sweep on that. | M3 round 4 (CI run 41766732456) | AC-11, AC-12 | Open | TBD | M4 |
| POAM-032 | Flaky E2E tests: `session-security.spec.ts:87` (open redirect via `return_to`) and `:148` (CSRF on keepalive/logout) each failed once on chromium and webkit in CI run [41773314457](https://github.com/192d-Wing/usg-ops-console/actions/runs/41773314457) and passed on retry (`waitForURL` 120 s timeout; `toHaveURL` mismatch). Assertions are not weakened by retries, but flakes hide real regressions. Fix: isolate each hostile request in its own context and client IP, and wait on the specific response rather than URL polling. | CI run 41773314457 | SA-11 | Open | TBD | M4 |
