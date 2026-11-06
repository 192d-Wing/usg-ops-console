# Plan of Action and Milestones (POA&M)

Status as of M5 (sdk-js and the usg-dcim DCIM tool, finch + otter-go),
2026-11-05. **M5 evidence is local only; CI has not run M5.** The finch
E2E specs run only locally when `DCIM_REPO` is set (CI keeps hello-tool
only), and the usg-dcim changes are uncommitted on its branch
`feat/console-embed`. M5 rows below cite those local runs.

Previous status: M4.5 (redesign: tokens, icon rail, command palette,
breadcrumb scope switcher), 2026-11-04. M4.5 evidence is local only; CI has
not run M4.5. M4 rows below are unchanged.

M4 baseline (tool registry, proxy, tool container, SDK v1),
2026-11-04. Owner and milestone fields are placeholders until the team
assigns them. M4 is green in CI run 41892456284 (all jobs); M4 evidence
below also cites local runs (qa-report "Final (round 3)": `make check` 252
passed, `make test-integration` 99 passed, macOS Playwright 294 passed,
Linux container Playwright 428 passed / 3 flaky WebKit sign-in failures).

Status values used below:

- **Open**: the weakness exists and is not fixed.
- **Pending CI verification**: a fix or test exists and passed locally, but
  the CI job that is meant to prove it has never had a green run. The item
  stays open until a green CI run is recorded here, with its run ID and
  artifacts.
- **Deferred**: accepted for now, with a stated reason.

## Items from `POAM:` markers in code

`POAM:` markers in the repository (as of M4):

| Marker location | Item |
| --- | --- |
| `deny.toml:7`, `.cargo/audit.toml:13` | POAM-001, POAM-002 |
| `deny.toml:11`, `.cargo/audit.toml:4` | POAM-014 |
| `deny.toml:14`, `.cargo/audit.toml:9` (`POAM (M4)`) | POAM-033 |
| `crates/console-gateway/migrations/0003_roles_refresh.sql:8` (`-- POAM: AU-9`) | POAM-017 |
| `crates/console-gateway/migrations/0003_roles_refresh.sql:10` (`-- POAM: SC-28`) | POAM-015 |
| `crates/console-gateway/src/session.rs:438` (`// POAM: SC-28`) | POAM-015 |
| `crates/console-gateway/src/audit.rs:267` (`// POAM: AU-5`) | POAM-016 |
| `crates/console-gateway/src/sweeper.rs:39` (`// POAM: AU-5`) | POAM-016 |
| `crates/console-gateway/src/audit.rs:190` (`// POAM: AU-5`, background queue) | POAM-036 |
| `crates/console-gateway/src/tool_origin.rs:681` (`// POAM: SC-7`) | POAM-034 |
| `crates/console-gateway/src/registry.rs:248` (`// POAM: SC-8`) | POAM-037 |

M5 markers in the usg-dcim repository (`packages/otter-go`, branch
`feat/console-embed`, uncommitted; line numbers as of 2026-11-05):

| Marker location (usg-dcim) | Item |
| --- | --- |
| `packages/otter-go/internal/auth/middleware.go:148` (standalone scope query fails open) | POAM-050 |
| `packages/otter-go/internal/auth/console.go:292` (user cache not invalidated by the admin deactivate API) | POAM-051 |
| `packages/otter-go/internal/auth/console.go:249` (per-process `jti` replay cache) | POAM-052 |
| `packages/otter-go/internal/auth/console.go:596` (production `acr` value) | POAM-028 |

The M3 marker `crates/console-shell/src/demo.rs:14` (POAM-010) was removed
in M4; the remaining `POAM-010` mentions in `tools.rs`, `api.rs`,
`chrome.rs` and `demo.rs` are closure notes, not markers.

| ID | Weakness | Source | Control | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- | --- |
| POAM-001 | RUSTSEC-2024-0370: `proc-macro-error` is unmaintained (not a known vulnerability). It comes in through `yew-macro` and is used at build time only. | `deny.toml` `[advisories] ignore` | RA-5, SI-2 | Open | TBD | TBD (re-check on each yew release) |
| POAM-002 | RUSTSEC-2025-0141: `bincode` 1.x is unmaintained (not a known vulnerability). It comes in through `gloo-worker`. The console does not use yew workers. | `deny.toml` `[advisories] ignore` | RA-5, SI-2 | Open | TBD | TBD (re-check on each yew release) |
| POAM-010 | NIST-004: the service menu list (`SERVICES`) was static demo data, not filtered by role. | formerly `crates/console-shell/src/demo.rs:14` | AC-3, AC-6 | **Closed (M4)**: `SERVICES` deleted; the menu comes from `GET /api/tools`, filtered by role on the server (`tools_api.rs`, marker `AC-3 ... closes POAM-010`); launch and proxy enforce roles. Tests: `router::tool_api_requires_a_session`, `pg_tool_role_denied`, shell `menu_groups_by_category`; E2E `tools.spec.ts` (`/api/tools` filtering), `tools-norole.spec.ts`. Confirmed in CI run [41892456284](https://github.com/192d-Wing/usg-ops-console/actions/runs/41892456284) on `75512c9` | — | M4 |
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
| POAM-011 | NIST-003 / SEC-002: known benign CSP reports on WebKit. WebKit reports a `style-src-attr` violation each time an `HTMLSelectElement` is created (reproduced with a bare `document.createElement('select')`, no app code). The E2E tests allow, **on WebKit only**, reports that match exactly `{violatedDirective: "style-src-attr", blockedURI: "inline", sample: ""}` with a target of `SELECT` or `null`, capped at the number of `<select>` elements plus 1 per page (`e2e/tests/csp-helpers.ts`, `csp.spec.ts`, `dist-smoke.spec.ts`). **Residual risk:** a first-party inline-style violation with a `null` (document-level) target on WebKit would also match; it is bounded by the cap and by the non-empty `[style]` DOM check. Adding `'report-sample'` to `style-src` (SEC-002(b)) is deferred. Upstream WebKit bug link: TBD. Any future CSP report collector must filter this triple. | SC-18, SA-11 | Open. **Re-reviewed at M3 (2026-11-03):** the allowance in `e2e/tests/csp-helpers.ts` is unchanged and still needed; the M3 CSP adds only the IdP issuer origin to `form-action`, which does not affect this report. WebKit E2E passed locally with the allowance. | TBD | TBD (expire when WebKit fixes it). **M4 re-review due:** tool iframes add CSP surface (console `frame-src` = registry origins; gateway CSP on tool responses). The allowance in `csp-helpers.ts` still applies to the console origin only; a reviewer must confirm it is unchanged and does not mask tool-origin reports before this item is re-dated |
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
| POAM-022 | No attributed WASM size breakdown: the stripped release wasm has no name section, so `twiggy` output is anonymous. Release bundle is 173,207 B gz (+22.0% vs M2's 141,918 B; new baseline 173,207 B). | PERF-001 | Performance | Deferred. M4 recorded a symbolised breakdown in the run directory (`perf-001-wu3-size.txt`, debug-name release build) but no `[profile.profiling]` exists in `Cargo.toml` yet. M4 bundle: 206,548 B gz (+18.6% vs `main` 174,214 B, accepted as PERF-001 M4; CI warn baseline kept at 204,977 B) | TBD | TBD |
| POAM-023 | Keepalive refresh-grant latency is not measured (no span or metric, no p50/p99). | PERF-007 | Performance | Partial: span `oidc.refresh_grant{duration_ms,outcome}` added in M4 (`oidc.rs`); p50/p99 from a keepalive loop not yet recorded | TBD | TBD |
| POAM-024 | The refresh grant uses the general IdP timeout (5 s connect, 10 s total). A shorter timeout (3-5 s) would let the idle dialog fail fast. | PERF-008 | Performance | **Closed (M4)**: dedicated refresh client, connect 2 s, total `CONSOLE_OIDC_REFRESH_TIMEOUT_SECS` (default 4, 1-10); timeout gives 503. Test `pg_refresh_timeout_is_retryable_503`; green in CI run [41892456284](https://github.com/192d-Wing/usg-ops-console/actions/runs/41892456284) on `75512c9` | TBD | M4 (close with CI run ID) |
| POAM-025 | M4 SDK docs must state the full gateway error contract: open `error` values (unknown values must be tolerated), logout XHR detection (`X-CSRF-Token` header or `Accept` containing `application/json`; `Accept: */*` without the header gets a 303), keepalive 503 retryable vs 401 `revoked`, and that tools under `/t/<id>/` may receive `?console_notice=` and must ignore unknown parameters. | INT-007, INT-008 | Integration contract | **Closed (M4)**: [INTEGRATING-A-TOOL.md](../INTEGRATING-A-TOOL.md) "Errors" states all four points | — | M4 |
| POAM-026 | The identity-header strip list (`headers.rs`: `x-console-user`, `x-console-roles`, `x-console-session`, `x-forwarded-user`) is narrow. Before the M4 proxy: allow-list upstream headers, drop the session cookie and hop-by-hop headers, and set identity only from the session. | SEC-010 | AC-3 | **Closed (M4)**: the proxy rebuilds request headers from an allow-list, drops the console/tool cookies and hop-by-hop headers, and sets identity only via the signed assertion (`proxy.rs:272` marker; `upstream_request_headers`, `rebuild_cookie`). Tests: `proxy::tests::allow_list_rebuild_drops_identity_and_hop_by_hop`, `pg_tool_launch_proxy_and_assertion`; E2E `tools.spec.ts` "identity header spoofing never reaches the upstream". Confirmed in CI run [41892456284](https://github.com/192d-Wing/usg-ops-console/actions/runs/41892456284) on `75512c9` | — | M4 |
| POAM-027 | The Helm chart must keep `CONSOLE_MIGRATOR_DATABASE_URL` off the long-running gateway: migrations run as a separate `console-gateway migrate` job or init container. When the key is set, `serve` migrates at startup (dev convenience) and the runtime process could assume `console_owner`. | SEC-006 re-verification (e) | AC-6, AU-9 | Open | TBD | M7 (Helm) |
| POAM-028 | The production `CONSOLE_REQUIRED_ACR` value (the `acr` that icam's X.509/CAC flow emits, including on SSO reuse via `acr.loa.map` and a conditional-level flow) must be confirmed with the icam Keycloak admins. The IA-2(12) enforcement in the gateway is only as strong as that mapping. | NIST-002, SEC-002 | IA-2(12), IA-2(1), IA-2(2) | Open | TBD (icam admins) | Before production |
| POAM-029 | The icam client `usg-ops-console` SSO settings must be confirmed with the icam admins: client session idle and max must equal the console values (900 s / 43,200 s by default; idle at least the console idle or keepalive fails, at most so SSO cannot silently outlive the console session), `revokeRefreshToken=false` (or reuse > 0), refresh tokens on. Verified only in the dev realm export. | SEC-001, NIST-001 | AC-11, AC-12 | Open | TBD (icam admins) | Before production |
| POAM-030 | M3 verification gaps: the CI jobs `gateway-integration` and compose-based `e2e` had not run, and Firefox had not run the E2E suite. Closed by CI run [41773314457](https://github.com/192d-Wing/usg-ops-console/actions/runs/41773314457) on commit `d55b641`: all jobs green; `gateway-integration` passed; `e2e` on chromium, firefox and webkit (incl. idle-*, dist-*) 268 passed, 0 failed, 28 skipped (axe Chromium-only), 4 flaky (see POAM-032). Still not covered by E2E at M3: absolute-expiry reauth in the browser, keepalive 503 inline error UI, no-JS form logout through CSP `form-action`, the `stale_auth` rejection. **M4 note:** these four gaps now have E2E (`session-gaps.spec.ts`, `keepalive-503.spec.ts`), passing locally; CI pending. | QA report; CI run 41773314457 | SA-11, CA-2 | Closed — CI run 41773314457 | — | M3 |
| POAM-031 | The expired-session sweeper applies the idle limit configured on the gateway that runs it, not the limit in force when each session was created. Replicas that share one database but disagree on `CONSOLE_IDLE_TIMEOUT_SECS` (for example during a rolling deploy that shortens it) let the shortest limit end other replicas' sessions early. Found in M3 round 4: the E2E idle-test gateway (150 s) swept the main suite's sessions. Fix: store the idle limit (or idle deadline) per session row and sweep on that. | M3 round 4 (CI run 41766732456) | AC-11, AC-12 | **Closed (M4)**: migration `0004_session_idle.sql` stores `idle_timeout_secs`/`idle_expires_at` per row; `require_session`, keepalive and the sweeper use the row value (`session.rs`, `routes.rs`). Tests: `pg_per_session_idle_limit` (two gateways, 150 s and 900 s, one DB); E2E IDLE gateway now sweeps every 5 s and all main-suite sessions survived both full local runs. Confirmed in CI run [41892456284](https://github.com/192d-Wing/usg-ops-console/actions/runs/41892456284) on `75512c9` | — | M4 |
| POAM-032 | Flaky E2E tests: `session-security.spec.ts:87` (open redirect via `return_to`) and `:148` (CSRF on keepalive/logout) each failed once on chromium and webkit in CI run [41773314457](https://github.com/192d-Wing/usg-ops-console/actions/runs/41773314457) and passed on retry (`waitForURL` 120 s timeout; `toHaveURL` mismatch). Assertions are not weakened by retries, but flakes hide real regressions. Fix: isolate each hostile request in its own context and client IP, and wait on the specific response rather than URL polling. | CI run 41773314457 | SA-11 | **Closed (M4)**: `session-security.spec.ts:87`/`:148` rewritten (own context, client IP and SSO cookies; waits on the `/callback` response). CI step `--grep @no-retry --retries=0 --repeat-each=3` passed 30/30 on chromium, firefox and webkit in CI run [41892456284](https://github.com/192d-Wing/usg-ops-console/actions/runs/41892456284) on `75512c9` | TBD | M4 (first CI run) |

## M4 items

New items from the M4 reviews (security, nist-800-53, integration,
performance, ux-a11y) and the release engineer. Rows without a code marker
concern deployment, later milestones or accepted residual risk.

| ID | Weakness | Source | Control / reference | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- | --- |
| POAM-033 | RUSTSEC-2026-0387: `crypto-bigint` 0.5.5 is not constant-time. It comes in through `jsonwebtoken` (`rust_crypto`) to `p256`/`p384` 0.13. Scope: only `hello-tool-server` and gateway **tests**, and only to verify ES256 with public keys. Gateway signing and the `sid` HMAC use `ring`; `crypto-bigint` is not in the gateway's normal or build graph (release-notes). Ignored with reason in `deny.toml` and `.cargo/audit.toml`. Tools that copy hello-server inherit it ([INTEGRATING-A-TOOL.md](../INTEGRATING-A-TOOL.md#identity-assertion)) | NIST-005, SEC-009 | SR-3, RA-5, SI-2 | Open (accepted) | TBD | Re-check trigger: a `jsonwebtoken`/`p256` release on `crypto-bigint` >= 0.6, or any code path that signs with that stack |
| POAM-034 | WebSocket upgrades are not proxied: `Upgrade` and `CONNECT` on tool origins get 501 `not_implemented`. Tools needing live views cannot use WebSockets (SSE works) | brief Q2; marker `tool_origin.rs:681` | SC-7 | Open (deferred by user decision; M5 did not need it, finch uses no WebSockets) | TBD | Retargeted from M5: a later milestone (TBD), when a tool needs WebSockets |
| POAM-035 | Launch-code residual (SEC-001 / SEC-010). (a) Launch acceptance relies on `Sec-Fetch-Site: same-site`, which covers every host under the registrable domain; DEPLOYMENT.md requires a dedicated registrable domain hosting nothing but the console and tool origins (a deployment control this repo cannot verify). (b) A malicious *registered* tool origin is same-site and could frame another tool's launch URL with a code minted for the attacker's own session (`referrerpolicy=no-referrer`); the victim would see the attacker's data. Bounded by registry review. Full fix: a console-signed nonce bound to the shell frame | SEC-001 residual, SEC-010 (accepted LOW) | AC-4, SC-23, SC-7 | Open (accepted LOW) | TBD | TBD (console-signed nonce; not delivered in M5) |
| POAM-036 | Background audit rows (`tool.access`, `tool.request`, proxy-path `tool.denied`, rate-bounded unauthenticated denials) go through a bounded queue (1024). On overflow the row is dropped and counted (`AuditSink::dropped`); the first drop is logged at `error`, then at most one summary line per 10 s. The JSON tracing line is still written synchronously. No metric export or alert exists, and these writes do not fail closed | PERF-002, NIST AU-5, SEC INFO; marker `audit.rs:190` | AU-5 | Open | TBD | Before production: export `dropped()` as a metric and alert on it |
| POAM-037 | `insecure_http_upstream = true` allows cleartext http to a non-loopback upstream. It requires `insecure_http_upstream_reason` and emits a `registry.insecure_upstream` audit event at startup, but relies on a compensating control outside the gateway (for example service-mesh mTLS) that the gateway does not verify. A CM-6 deviation per use. No repo registry uses it | NIST-003; marker `registry.rs:248` | SC-8, CM-6 | Open | TBD | Per deployment: document the compensating control for each tool that sets it |
| POAM-038 | finch (M5) has no consumable SDK or schema: no TypeScript SDK, no TS types or JSON Schema, no non-Rust assertion-verification example. The frozen `sdk_v1/` and `tools_v1/` fixtures are the only contract artifacts | INT-002 | Integration contract | **Closed (M5, local evidence)**: `sdk-js/` (zero-dependency TypeScript SDK, vendored as one file by `scripts/vendor.mjs`) passes the shared protocol corpus `crates/console-protocol/tests/fixtures/MANIFEST.json` (58 entries), which the Rust side also runs (`crates/console-protocol/tests/protocol_v1.rs`, `crates/console-sdk/tests/fixtures.rs`). `make sdk-js-test`: typecheck clean, 82/82 vitest (`test/fixtures.test.ts`, `transport.test.ts`, `vendor.test.ts`). Non-Rust assertion verification reference: otter-go (usg-dcim, uncommitted). A CI job `sdk-js` exists in `.github/workflows/ci.yml` but has not run yet | — | M5 |
| POAM-039 | Refresh-gating residual (SEC-012): before the first observed frame exit, a tool's programmatic self-focus cannot be told from the user's first click into the frame, so at most one faked "user entered the tool" signal per page load can earn one console refresh | SEC-012 (accepted LOW) | AC-11 | Open (accepted LOW) | TBD | TBD |
| POAM-040 | Scope naming: the shell URL key is `env`, the scope header, assertion `ctx` and `Scope` use `environment`; the shell reserves `site`, `tenant`, `env` and drops a tool's own query parameters with those names from the mirrored URL. Documented; namespacing (for example `c.site`) not done | INT-008 | Integration contract | Open (documented) | TBD | Before external tool adoption |
| POAM-041 | DB connection budget: `CONSOLE_DB_MAX_CONNECTIONS` default is 32 plus 4 audit = 36 per replica. Default Postgres/CNPG `max_connections = 100` fits two replicas; 3 or more exceed it unless `max_connections` is raised. Documented in DEPLOYMENT.md; no boot-time check | PERF re-verification (a) | SC-5, CP-2 | Open | TBD | M7 (Helm values / runbook) |
| POAM-042 | The gateway cannot run behind a transaction-mode pooler (PgBouncer or CNPG Pooler in transaction mode): sqlx uses per-connection prepared statements and the gateway has no setting to disable the statement cache. Use session-mode pooling or raise `max_connections` | release engineer | SC-5, CP-2 | Open | TBD | TBD (add a statement-cache toggle, or keep session mode) |
| POAM-043 | Proxy latency budget (p99 overhead <= 5 ms) not met at c=32 on the shared-host bench: final round ping p99 overhead 6.26 ms; navigation 5.02 ms (borderline). The bench ran gateway, hello, load generator and the podman VM on one host (load average 27.4 on 12 cores), which inflates tails. c=1 meets the budget everywhere; c=128 is pool-bound by design. No session cache was added (M3 Q7 decision kept) | QA final round perf | Performance (SC-5) | Open | TBD | Re-measure on separate hosts before acting |
| POAM-044 | New flaky E2E (different from POAM-032): Linux WebKit Keycloak sign-in in the `tools.spec.ts` fixture under 2-worker load on a contended host; `:113` and `:134` hung on the `#kc-login` click (30 s timeout), `:218` got `callback status (400)`. Nothing under test was reached. A rerun (`--repeat-each=3`) passed 43/43 | QA final round | SA-11 | Open: did not recur in CI run [41892456284](https://github.com/192d-Wing/usg-ops-console/actions/runs/41892456284) on `75512c9`; keep open until it stays absent over several runs | TBD | Verify in CI; fix if it recurs |
| POAM-045 | Flaky E2E: `keepalive-503.spec.ts:16` on `ka503-webkit` timed out waiting for the idle dialog after advancing the clock, then passed on retry, in CI runs 41874957314 and 41892456284. Not a product failure (the 503 path is covered by `pg_refresh_timeout_is_retryable_503`), but retries hide regressions. Fix: wait on the shell's session load before fast-forwarding the clock, then move the spec into the no-retry step. | CI runs 41874957314, 41892456284 | SA-11 | Open (not addressed in M5) | TBD | Retargeted from M5: a later milestone (TBD) |
| POAM-046 | M4.5 redesign: manual assistive-technology checks not done. (a) Windows High Contrast (forced colors) on a real Windows host: only a Chromium emulation spot check exists (`rail.spec.ts` "forced-colors spot check"), and "active rail item uses Highlight" is not asserted because emulation reports the authored colour. (b) A VoiceOver pass over the rail (collapsed names and tooltips), command palette (combobox, count announcement) and scope popovers: no screen reader was used; accessible names were checked by accname computation only. | A11Y-006 (ux-a11y review, M4.5), qa-report coverage gaps | WCAG 2.1 SC 1.4.11, 1.4.13, 4.1.2, 4.1.3; Section 508 | Open | TBD | TBD (before the M4.5 VPAT remarks are final) |
| POAM-047 | Idle dialog over an open command palette: `#app-chrome` must stay inert after the palette closes, and the re-assert relies on effect order in `crates/console-shell/src/main.rs`. **Now covered** by `e2e/tests/idle.spec.ts` "SEC-002: idle over palette, palette closed -> #app-chrome still inert until the dialog is answered" (and "M4.5: idle dialog opens above the palette…"), which passed locally on idle-chromium, idle-webkit and Linux `idle-*` (QA round 2). Closed by CI run [42014779167](https://github.com/192d-Wing/usg-ops-console/actions/runs/42014779167) on commit `b229338`: all jobs green, including `e2e` idle-* on chromium, firefox and webkit. The first M4.5 run (41952443000, `6480986`) failed two timing-sensitive tests (`palette_ranking_is_fast`, Firefox wasm `rail_collapsed_keeps_text_names`); `b229338` takes best-of-5 timing and waits up to 1 s for re-render, with budgets and assertions unchanged. | SEC-002 (security review, M4.5, INFO) | AC-11, AC-12 | Closed — CI run 42014779167 | — | M4.5 |
| POAM-048 | `console_ui::BadgeColor` gained five variants in M4.5 and is not `#[non_exhaustive]`, so exhaustive `match`es in tools break whenever variants are added. Documented in `crates/console-ui/CHANGELOG.md`. Fix: mark it `#[non_exhaustive]` at the next breaking console-ui release. | INT-003 (integration review, M4.5) | Integration contract | Open (deferred) | TBD | Next breaking console-ui release |
| POAM-049 | Scope breadcrumb disabled style is unreachable and partly undifferentiated. No shell code sets `disabled` on a `.cui-scope-crumbs__seg` (the selects are disabled, not the segments), so the disabled crumb style in `components.css` exists only in CSS and is exercised only by an E2E that sets `disabled` in the DOM (`scope.spec.ts` A11Y-004 case). `--cui-color-border-disabled` equals `--cui-color-border` in every theme (`tokens.css`), so the disabled state differs by dashed style, sunken fill, text colour and hidden chevron, not by border colour. Design follow-up: decide whether segments can be disabled and give `border-disabled` its own value (with a contrast pair). | A11Y-004 (ux-a11y review, M4.5), QA round 2 observation | WCAG 2.1 SC 1.4.11 | Open (design follow-up) | TBD | TBD |

## M5 items

New items from the M5 reviews (security, nist-800-53, integration,
ux-a11y) and QA (U6). POAM-050 to POAM-052 have markers in usg-dcim; the
rest concern deployment, cross-repo process or test coverage. The
production `acr` / MFA question for otter-go console mode is tracked under
the existing POAM-028 (marker `console.go:596`).

| ID | Weakness | Source | Control / reference | Status | Owner | Milestone |
| --- | --- | --- | --- | --- | --- | --- |
| POAM-050 | otter-go **standalone** path: if the per-user scope query (`GetUserScopedCapabilities`) fails, the principal gets no scopes, which `scope.go` treats as global scope (fail open). The console path was fixed in M5 (503 `auth backend unavailable`, test `TestDispatch_ScopeErrorFailsClosed`); the standalone path is unchanged | NIST-002; marker `middleware.go:148` (usg-dcim) | AC-3 | Open | TBD (usg-dcim) | TBD |
| POAM-051 | otter-go caches the console `sub`-to-user lookup for 15 s (active accounts only). `ConsoleAuth.InvalidateUser` evicts at once, but the admin deactivate API does not call it, and other otter-go replicas are bounded only by the 15 s TTL. A deactivated user can keep DCIM access for up to 15 s | SEC-004, NIST-004; marker `console.go:292` (usg-dcim) | AC-2 | Open | TBD (usg-dcim) | TBD (call `InvalidateUser` from deactivate; shared invalidation before scale-out) |
| POAM-052 | The otter-go `jti` replay cache is per process. With several replicas a replayed assertion that lands on another replica is not caught; `exp` (60 s) plus leeway and `aud` still bound it to about 90 s at DCIM. Run a single replica until a shared store exists | SEC-005, NIST-008; marker `console.go:249` (usg-dcim) | SC-23 | Open | TBD (usg-dcim) | Before otter-go scale-out |
| POAM-053 | Plaintext internal hops: gateway to finch nginx (`http://127.0.0.1:8094` in the dev registries) and finch nginx to otter-go are http. finch nginx forwards `X-Console-Assertion`, so anything that can reach nginx or otter-go directly can present a captured assertion within its window. Production needs https or mTLS on both hops and a NetworkPolicy that allows ingress to finch nginx only from the gateway pods (otter-go only from nginx and API-token clients). The gateway registry should reject http upstreams outside dev | SEC-006, NIST-009 | SC-8, SC-7 | Open | TBD | Before production (NetworkPolicy with the M7 Helm work); registry change TBD |
| POAM-054 | otter-go links console users by `sub` only (`users.sso_subject`); the issuer is not stored per row (no `sso_issuer` column). Mitigation in code: `CheckUpstreamIssuer` refuses startup in `console`/`both` mode unless `OTTER_CONSOLE_UPSTREAM_ISSUER` equals `DCIM_OIDC_ISSUER` when the latter is set (test `TestCheckUpstreamIssuer`). Operator precondition: no `sso_subject` row may come from another IdP or realm | SEC-001 residual (SEC N-1) | AC-2, IA-2 | Open | TBD (usg-dcim) | TBD (store issuer per row) |
| POAM-055 | INT-001: finch's vendored SDK header reads `source=0a939d0b1359+dirty` because `sdk-js` was uncommitted when it was vendored, so it is not reproducible from a commit. `verifyVendoredSdk` now rejects `+dirty`/`unknown` unless `allowDirty` is passed. Closure procedure (brief): (1) commit `sdk-js/`, `crates/console-protocol/tests/fixtures/**` and `crates/console-sdk/tests/fixtures.rs` with a clean tree; (2) from that commit run `cd sdk-js && npm ci && node scripts/vendor.mjs ~/Dev/usg-dcim/packages/finch/src/vendor/console-sdk`; (3) the header must read `source=<sha>` with no `+dirty`, and finch's compat test calls `verifyVendoredSdk(src, {protocol: 1, version: "0.1.0"})` without `allowDirty`; (4) a second re-vendor gives identical bytes and sha256 | INT-001 | Integration contract, CM-3 | Open | TBD | Before finch merges |
| POAM-056 | A11Y-005 residual: no E2E for the skip link or F6 handoff into finch and back; no 320 px reflow check of finch inside the container; no axe scan of the finch session-ended view (covered by unit tests and a focus assertion only); no manual screen reader pass of embedded finch | A11Y-005, QA U6 round 3 | WCAG 2.1 SC 1.4.10, 2.4.1, 2.4.3, 4.1.3; Section 508 | Open | TBD | TBD (before finch VPAT remarks are final) |
| POAM-057 | finch pages render some plain `href="/sites/<id>"` links. A normal click goes through the router (with the `/t/dcim` basename), but opening one in a new tab or with middle-click gives an unprefixed URL that the gateway does not serve under the tool. Not covered by E2E | U5 known limit, QA U6 | Usability (integration) | Open | TBD (usg-dcim) | TBD |

Not POA&M items (accepted by reviewers, documented): the Firefox reverse-Tab
extra stop on the tool `<iframe>` (see [ACCESSIBILITY.md](ACCESSIBILITY.md));
the `allow-modals` sandbox flag (user decision; residual risk in
NIST_800-53_MAPPING.md SC-18); the shell bundle at 206,548 B gz (+18.6%,
PERF-001 accepted) and at M4.5 240,530 B gz (`gzip -9`, about +16.5% over
M4, PERF-001 accepted for M4.5; CI warn baseline moved to 240,530 B, passed
in CI run 42014779167); the exchange path having no unknown-id timing twin
(SEC-011 INFO). The hello-tool `examples/hello-tool/Containerfile` is an
example template and is not built in CI (unverified build).
