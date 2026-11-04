# NIST SP 800-53 Rev. 5 control mapping

Status as of M3 (Keycloak OIDC sign-in, server-side sessions, logout, idle
timeout, audit log), 2026-11-03.
This file describes only what the code in this repository does today.

Status values:

- **Implemented**: the code does it, and a test or gate in this repo proves it.
- **Implemented (dependency)**: implemented in the console, but its
  strength depends on an external system's configuration that this repo
  cannot prove (named in the row, with a POA&M item).
- **Partial**: some of the control is in code, or the code is there but no test
  proves all of it.
- **Planned**: nothing in code yet. The milestone is from `docs/PLAN.md`.
- **Inherited**: provided by an external system (Keycloak icam realm, the
  deployment platform). Evidence comes from that system, not this repo.

**M3 evidence scope.** M3 test evidence is from local runs only:
`make check` green; `make test-integration` against compose Postgres 17
(`pg_flow` 15/15, 0 `NOT RUN`); Playwright on Chromium and WebKit through the
compose Keycloak (184 passed, 0 failed, 13 skipped by design, fix round 2).
**Firefox has not run, and the new CI jobs (`gateway-integration`,
compose-based `e2e`) have not run yet (pending push).** M3 rows stay
conditional on the first green CI run (POAM-030). M1/M2 rows cite CI run
[41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231).

## Summary

| Control | Status | Milestone |
| --- | --- | --- |
| AC-2, AC-2(3) Account management: revocation on keepalive | Implemented (dependency: Keycloak icam; POAM-019, POAM-029) | M3 |
| AC-2 Account lifecycle (create, disable, delete users) | Inherited (Keycloak icam) | M3 |
| AC-3 Access enforcement (session on every non-public route) | Implemented | M3 |
| AC-6 Least privilege (required role; DB roles) | Implemented | M3 |
| AC-7 Unsuccessful logon attempts | Inherited (Keycloak icam brute-force detection; POAM-018) | M3 |
| AC-11 Device lock / idle timeout | Implemented (dependency: icam client SSO idle; POAM-029) | M3 |
| AC-12 Session termination | Implemented | M3 |
| AU-2, AU-12 Event logging, audit record generation | Implemented | M3 |
| AU-3 Content of audit records | Implemented | M3 |
| AU-5 Response to audit logging failures | Partial (POAM-016) | M3 |
| AU-8 Time stamps | Implemented | M3 |
| AU-9 Protection of audit information | Partial (POAM-017) | M3 |
| IA-2 Identification and authentication (OIDC) | Implemented | M3 |
| IA-2(12) Acceptance of PIV credentials | Implemented (dependency: icam PIV `acr` mapping; POAM-028) | M3 |
| IA-5 Authenticator management (client secret) | Implemented (dependency: rotation not evidenced) | M3 |
| IA-11 Re-authentication | Implemented | M3 |
| SC-5 Denial-of-service protection | Partial (POAM-020) | M3 |
| SC-8 Transmission confidentiality and integrity | Implemented (dependency: TLS-terminating ingress) | M0-M3 |
| SC-13 Cryptographic protection | Implemented | M3 |
| SC-18 Mobile code (Content Security Policy) | Implemented | M0-M3 |
| SC-23 Session authenticity | Implemented | M3 |
| SC-28 Protection of information at rest (tokens) | Partial (POAM-015) | M3 |
| SI-10 Input validation (`safe_href`, theme, scope query, search, `return_to`, OIDC tokens, config) | Implemented | M1-M3 |
| SI-11 Error handling | Implemented | M2-M3 |
| CM-7 Least functionality (gallery and dev realm excluded from release) | Implemented | M1-M3 |
| SA-11 Developer testing (wasm-bindgen, integration, E2E) | Implemented (M3 CI jobs pending, POAM-030) | M2-M3 |
| SA-11(1) Static code analysis (clippy) | Implemented | M0 |
| SR-3, SR-11, CM-14 Supply chain (pinned actions, images, npm) | Partial | M2-M3 |
| RA-5 / SI-2 Dependency vulnerability scanning | Partial | M0 |
| PL-8 Security architecture (scope is a UI filter only) | Partial (documented design rule) | M2 |
| AC-3, AC-6 Service-menu role filtering | Planned (POAM-010) | M4 |

Session details: [SESSION_MANAGEMENT.md](SESSION_MANAGEMENT.md). Audit
details: [AUDIT_LOGGING.md](AUDIT_LOGGING.md). Unless a path is given,
gateway files are under `crates/console-gateway/`; `pg_*` tests are in
`tests/pg_flow.rs`.

## AC-2, AC-2(3) Account management

**Status: Implemented (dependency).** Account lifecycle (creation,
disabling, role assignment) is **Inherited** from the Keycloak icam realm.
The console's part is to stop honouring an account that Keycloak has
disabled or de-roled.

- Every keepalive (`routes::keepalive`, `oidc::refresh_session`) performs a
  refresh-token grant. IdP rejection, a subject change, an invalid token, a
  missing refresh token, or loss of the required role deletes the session,
  audits `session.revoked`, and returns 401 `revoked`.
- Roles are re-read from the refreshed id_token.
- Without a keepalive the session ends at the idle deadline (15 min by
  default), so a disabled user keeps access for at most one idle period.

Tests: `pg_keepalive_revalidates_with_idp`; E2E `revocation.spec.ts`
(disabled user, removed `console-user`).

Dependencies: Keycloak refresh tokens on, `revokeRefreshToken=false`, client
session idle at least the console idle (POAM-029). An IdP outage blocks
extension (503) and users idle out (POAM-019). There is no back-channel
logout endpoint.

## AC-3 Access enforcement

**Status: Implemented.**

- `require_session` (`src/routes.rs`) guards every route except
  `PUBLIC_PATHS` (`/healthz`, `/login`, `/logged-out`, `/auth.css`) and the
  callback path. This includes `index.html`, the wasm bundle and unknown
  `/api/*` paths. No session: 401 JSON (XHR) or 302 to `/login` (navigation).
- Client-supplied identity headers (`x-console-user`, `x-console-roles`,
  `x-console-session`, `x-forwarded-user`) are dropped before routing
  (`src/headers.rs`, `strip_identity_headers`).

Tests: `router::every_non_public_route_requires_a_session`,
`router::public_pages_need_no_session`,
`router::identity_headers_are_stripped`; E2E `session-security.spec.ts`.

Not covered: per-tool authorization (tools keep their own RBAC) and
service-menu filtering (POAM-010, M4).

## AC-6 Least privilege

**Status: Implemented.**

- A session is created only if the id_token carries `CONSOLE_REQUIRED_ROLE`
  (default `console-user`). Otherwise 403, no session, `login.denied`
  `missing_role`. Stored roles are filtered to `console-*`.
- DB roles (`migrations/0003_roles_refresh.sql`): `console_owner` (NOLOGIN)
  owns all objects; the runtime `console_app` role has DML on `sessions`
  and `auth_requests` only; `console_audit` has INSERT and SELECT on
  `audit_events` only; migrations run separately as `console_migrator`.

Tests: `pg_missing_role_is_denied_without_session`,
`oidc_verify::role_filtering_bounds`, `pg_audit_is_append_only` (role
rights); `deploy/compose/postgres/check-roles.sql` (run locally); E2E
`auth.spec.ts` (`no_role` user gets 403).

Deployment dependency: keep `CONSOLE_MIGRATOR_DATABASE_URL` off the
runtime gateway (POAM-027).

## AC-7 Unsuccessful logon attempts

**Status: Inherited** from Keycloak icam brute-force detection. The console
never sees passwords or PINs.

The gateway's `/login` limiter (`src/rate_limit.rs`, annotated SC-5) counts
requests per IP, not failed logons, and is **not** AC-7 evidence. The dev
realm export sets `bruteForceProtected: true`; the production icam setting
is unverified (POAM-018).

## AC-11 Device lock (idle timeout)

**Status: Implemented (dependency).**

- Server side: `CONSOLE_SESSION_IDLE_SECS` (default 900, range 60-3600) is
  checked on every request. Only `POST /api/session/keepalive` (the "Stay
  signed in" button) moves `last_seen_at`; reads never extend the session.
- Client side: `crates/console-shell/src/idle.rs` warns
  `min(120 s, idle/2)` before the deadline. Overlay clicks do not extend;
  Escape = Stay signed in (decision Q4).
- After an idle expiry the next sign-in must re-authenticate
  (`prompt=login` plus `auth_time` check, see IA-11).

Tests: `session::tests::expiry_boundaries`, `pg_idle_and_absolute_expiry`,
`pg_full_flow_and_audit` (reads do not extend); shell
`lead_is_min_of_120s_and_half_idle`; console-ui wasm
`modal_overlay_click_can_be_disabled`; E2E `idle.spec.ts` (13 passed on
Chromium and WebKit after the round-2 lead fix), `session-lifecycle.spec.ts`.

Dependency: the icam client SSO idle must not let Keycloak re-issue a code
silently for a user whose reauth marker is gone (cookies cleared). Values
to be confirmed with the icam admins (POAM-029).

## AC-12 Session termination

**Status: Implemented.**

- Absolute lifetime `CONSOLE_SESSION_ABSOLUTE_SECS` (default 43200, range
  900-86400) is checked on every request and is never extended.
- Expired rows are deleted on access and by the sweeper.
- `POST /logout` deletes the session row (with its tokens) and sends the
  browser to Keycloak end-session with `id_token_hint`; other tabs get 401 on
  their next request.

Tests: `pg_idle_and_absolute_expiry`, `pg_sweeper_deletes_and_audits`,
`pg_full_flow_and_audit` (logout, cross-tab 401),
`pg_form_logout_with_json_accept_and_limits`,
`oidc_verify::end_session_url_carries_hint_and_client`; E2E `auth.spec.ts`
(SSO ended, two tabs). Not covered by E2E: absolute expiry in the browser
and the no-JS form logout (POAM-030).

## AU-2, AU-12 Event logging and audit record generation

**Status: Implemented.** Nine event types (`login.success`,
`login.failure`, `login.denied`, `logout`, `session.expired.idle`,
`session.expired.absolute`, `session.refresh`, `session.revoked`,
`access.denied`), each written as a JSON log line (`target=audit`) and an
`audit_events` row (`src/audit.rs`). Catalogue and reasons:
[AUDIT_LOGGING.md](AUDIT_LOGGING.md).

Tests: `pg_full_flow_and_audit`, `pg_callback_failures_and_single_use`,
`pg_missing_role_is_denied_without_session`,
`pg_insufficient_assurance_is_denied`, `pg_keepalive_revalidates_with_idp`,
`pg_sweeper_deletes_and_audits`, `pg_login_rate_limit_is_audited_once`,
`pg_failed_callback_ends_presented_session`; E2E audit-table queries in
`session-lifecycle.spec.ts`, `revocation.spec.ts`, `assurance.spec.ts`.

## AU-3 Content of audit records

**Status: Implemented.** Who (`user_sub`, `username`, `session_ref`), what
(`event_type`, `reason`), when (`occurred_at`), where (`client_ip` from the
trusted-proxy `X-Forwarded-For` walk, `user_agent`), outcome, `request_id`.
No token text.

Tests: `pg_full_flow_and_audit` (fields asserted; no `eyJ` in any row);
`client_ip::tests::table`.

## AU-5 Response to audit logging failures

**Status: Partial.** `login.success` fails closed: no audit row, no
session (503). All other events are best effort: the JSON line is always
written, and a DB failure is logged at `error` as `audit write failed`
(`AuditSink::record_best_effort`, marker `// POAM: AU-5` in `src/audit.rs`
and `src/sweeper.rs`). No alert or metric (POAM-016).

## AU-8 Time stamps

**Status: Implemented.** `occurred_at` is `timestamptz`, written by the
gateway in UTC; the JSON line uses RFC 3339 with milliseconds and `Z`.
Tests: `pg_full_flow_and_audit` (record fields).

## AU-9 Protection of audit information

**Status: Partial.** Append-only triggers (UPDATE, DELETE, TRUNCATE),
`audit_events` owned by NOLOGIN `console_owner`, no rights for the runtime
role, INSERT and SELECT only for `console_audit`
(`migrations/0002_audit.sql`, `0003_roles_refresh.sql`). Test:
`pg_audit_is_append_only`. Residual: `console_owner` (via the migrator
login) and superusers can bypass it (`-- POAM: AU-9`, POAM-017).

## IA-2 Identification and authentication

**Status: Implemented.** OIDC authorization code flow with PKCE S256,
`state` (single use, bound to `__Host-console_auth`, constant-time compare)
and `nonce` (`src/oidc.rs`, `src/routes.rs`). id_token checks: signature
(algorithms pinned to RS256, PS256, ES256), `iss`, `aud`, `azp`, `nonce`,
`exp`, `iat` skew 60 s. Confidential client (`client_secret_basic`).

Tests: `oidc_verify::valid_token_yields_identity`, `rejects_bad_claims`,
`accepts_extra_audience_with_matching_azp`,
`rejects_wrong_key_and_alg_none`, `authorize_url_has_pkce_s256_nonce_state`;
`pg_callback_failures_and_single_use`; E2E `auth.spec.ts` (S256, state,
nonce asserted on the real Keycloak redirect).

## IA-2(12) Acceptance of PIV credentials

**Status: Implemented (dependency).** CAC/PIV authentication itself is
performed by Keycloak (X.509). The console enforces the assurance level:
`CONSOLE_REQUIRED_ACR` is required unless `CONSOLE_DEV_MODE=true`, is sent
as `acr_values`, and the id_token `acr` must match, else 403 and
`login.denied` `insufficient_assurance` (`src/config.rs`, `src/routes.rs`).

Tests: `config::tests::acr_required_outside_dev_mode`,
`pg_insufficient_assurance_is_denied`,
`oidc_verify::authorize_url_reauth_and_acr`,
`oidc_verify::acr_and_auth_time_are_surfaced`; E2E `assurance.spec.ts`.

Dependency: this proves PIV only if production sets `CONSOLE_REQUIRED_ACR`
to the value icam's X.509 flow emits (including on SSO reuse). Locally the
dev realm maps **password** login to
`urn:usg-ops-console:dev:password`. Production value to be confirmed with
the icam admins (POAM-028).

## IA-5 Authenticator management (client secret)

**Status: Implemented (dependency).** The OIDC client secret and DB URLs
can be read from `*_FILE` paths; direct env values produce a startup
warning naming the `_FILE` key. `Secret` redacts in `Debug`. Tests:
`config::tests::defaults_and_secret_redaction`, `direct_secret_warns`,
`direct_db_url_warns`. Secret rotation is not evidenced in this repo.

## IA-11 Re-authentication

**Status: Implemented.** After logout, expiry or revocation the
`__Host-console_reauth` marker (or `/login?reason=expired|ended`) makes
`/login` send `prompt=login`, and the callback requires a fresh
`auth_time` (no earlier than the request time minus 30 s), else
`login.failure` `stale_auth`.

Tests: `pg_reauth_after_logout_requires_fresh_auth`,
`pg_expired_reason_forces_prompt_and_notice`; E2E
`session-lifecycle.spec.ts` (re-auth form despite live SSO). The
`stale_auth` path is not exercised by E2E. Residual: clearing cookies
removes the marker (SESSION_MANAGEMENT.md).

## SC-5 Denial-of-service protection

**Status: Partial.** `/login` is rate-limited per IP (IPv6 by /64, 30 per
minute, 10,000 tracked IPs, oldest 10% evicted at the cap); pending sign-ins
are capped at 10,000 (503); IdP I/O has 5 s connect and 10 s total
timeouts; requests time out at 30 s; query strings and the logout form body
are capped at 4 KB. Tests: `rate_limit::tests::ipv6_keyed_by_slash_64`,
`saturation_evicts_oldest_not_everyone`, `pg_pending_auth_requests_are_capped`,
`pg_login_rate_limit_is_audited_once`, `pg_form_logout_with_json_accept_and_limits`
(413). Gaps: `/callback` and other routes are not rate-limited (POAM-020).

## CM-7 Least functionality

**Status: Implemented.**

The component gallery (`/gallery`) is a development aid. It is compiled only
when the `gallery` Cargo feature is on. The feature is off by default, so
release builds leave out both the route and the module.

Code:

- `crates/console-shell/Cargo.toml`: `[features] default = []`, `gallery = []`.
- `crates/console-shell/src/main.rs`: the `Gallery` route and the
  `gallery` module are `#[cfg(feature = "gallery")]`.
- `Makefile` target `web` builds the release bundle into
  `crates/console-shell/dist` without the feature. Target `web-gallery` builds
  into the separate, gitignored `crates/console-shell/dist-gallery`.
- `deploy/Containerfile` runs plain `trunk build --release`, so the image has
  no gallery.

Evidence:

- Test `gallery_not_default_feature` in `crates/console-ui/tests/guards.rs`
  fails if `gallery` appears in the shell's default features.
- Make target `check-no-gallery` (part of `make check`) rebuilds `dist`. It
  fails if no release wasm is present, or if the wasm contains the
  gallery-only string `Component gallery`.
- CI job step "Release bundle is gallery-free (CM-7)" in
  `.github/workflows/ci.yml` runs the same grep on the CI release build.

- E2E `e2e/tests/dist-smoke.spec.ts` test "release dist: /gallery is NotFound
  (gallery not compiled in)" runs against the release `dist` (gateway on
  127.0.0.1:8082, `dist-*` Playwright projects). Passed on all three engines
  in CI run [41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231) (see SA-11).

M3: the dev realm export and dev config (`deploy/compose/`, `e2e/`) are
excluded from the image build context (`.containerignore`, `.dockerignore`).
The CI `container` job asserts that the image file list has no
`realm-dev.json`, `gateway.env` or `deploy/compose` (not yet run, POAM-030).

Limits: the gallery has no authentication; in M3 it sits behind
`require_session` like every other non-public route. That is acceptable only while it
stays compiled out of release builds. `make run` serves the gallery bundle and
is for local development only.

## SI-10 Information input validation

**Status: Implemented** for the input points listed here. Manifests and
SDK messages do not exist yet (M4).

### `safe_href`: rendered link targets

`crates/console-ui/src/logic.rs` `safe_href` (annotated
`// NIST 800-53: SI-10`). `Link`, `SideNavigation` and `Breadcrumbs` route every
`href` through it. It removes tab, CR and LF characters and trims leading
control characters and spaces. It then allows only `http:`, `https:`,
`mailto:` and relative URLs, and returns `#` for anything else. It also
returns `#` for protocol-relative and backslash forms (`//`, `\\`, `/\`, `\/`),
which browsers resolve to another origin. External links get
`rel="noopener noreferrer"`.

Tests:

- `logic::tests::safe_href_allow_list` (`crates/console-ui/src/logic.rs`)
- `safe_href_more_vectors` (`crates/console-ui/tests/qa_logic_css.rs`)

### Theme preference from `localStorage`

`Theme::parse` in `crates/console-ui/src/logic.rs` (annotated) accepts only the
listed theme values. `crates/console-shell/src/theme.rs` `load` (annotated)
passes the stored `cui-theme` value through that parse before anything is
written to `data-theme` on `<html>`.

Tests:

- `logic::tests::theme_parse` (`crates/console-ui/src/logic.rs`)
- `theme_allow_list_rejects_near_misses` (`crates/console-ui/tests/qa_logic_css.rs`)
- `tests::parse_allow_list_only`, `tests::system_clears_attribute`
  (`crates/console-shell/src/theme.rs`)

Gap: the wasm-only DOM path (`theme::dom::load/apply/save`) has no test. See
POA&M item POAM-009.

### Scope URL query (`site`, `tenant`, `env`)

`crates/console-shell/src/scope.rs` (annotated `NIST 800-53: SI-10` and
`SI-11`) parses `location.search`:

- The query is ignored as a whole if it is over 2048 bytes or 32 pairs.
- Keys are matched exactly and case-sensitively. A repeated key is rejected.
- No percent-decoding and no `+` handling. A value is accepted only if it
  matches `[a-z0-9][a-z0-9-]{0,62}` (checked byte by byte) **and** exists in
  the demo hierarchy (`crates/console-shell/src/demo.rs`): tenant must exist
  under the site, env under the (site, tenant) pair.
- Rejected keys are dropped and the URL is rewritten with `replaceState`.

`same_origin_url` in `crates/console-shell/src/main.rs` (SEC-001) collapses a
leading run of `/` or `\` to one `/` before every history push/replace, so a
crafted pathname such as `//evil.example/` cannot make `pushState` throw.

Tests (native, `crates/console-shell/src/scope.rs` `tests`): `charset`,
`rejects_bad_values_without_decoding`, `keys_exact_and_case_sensitive`,
`duplicates_rejected`, `oversize_ignored`, `relationship_checks`,
`qa_size_limits_exact_boundaries`,
`qa_uppercase_and_percent_encoding_rejected_at_every_level`,
`qa_repeated_lower_keys_rejected_upper_kept`, `qa_tenant_or_env_without_site`,
`qa_cross_site_pairs`, `qa_cascade_results_are_always_valid`; and
`tests::same_origin_url_collapses_leading_slashes`
(`crates/console-shell/src/main.rs`). E2E: `e2e/tests/scope.spec.ts`
(relationship validation and "QA: URL edge cases", for example "oversize query
(>2048 bytes) ignores all scope with a fixed message", "percent-encoded valid id
is not decoded").

### Global search box

`SearchInput` (`crates/console-ui/src/components/search.rs:33`, annotated
`SI-10 - input length-bounded (maxlength + truncation)`) sets `maxlength`. The
shell's `GlobalSearch` (`crates/console-shell/src/chrome.rs`) truncates to 256
characters and is display-only: submit shows a fixed "Search is not available
yet" flash and the query is not put in the URL or storage.

Tests: wasm `search_input_maxlength_and_submit`
(`crates/console-ui/tests/wasm.rs`, local Chrome only, see POAM-003); E2E
"search submit shows fixed info flash and never echoes the query"
(`e2e/tests/keyboard.spec.ts`).

### `return_to` after sign-in (gateway, M3)

`src/return_to.rs` (annotated SI-10) accepts only same-origin absolute
paths; `//`, `/\`, encoded slashes, control and non-ASCII characters, auth
endpoints and over-long values fall back to `/`. `console_notice` takes
fixed values only. The shell builds its login URL from `window.location`
only and follows a logout redirect only to the end-session origin or its
own `/logged-out`.

Tests: `return_to::tests::accepts_plain_paths`,
`rejects_open_redirects_and_loops`, `notices_are_fixed_values`;
shell `login_url_is_same_origin_and_encoded`,
`logout_redirect_only_to_end_session_origin_or_logged_out`; E2E
`session-security.spec.ts` (17 hostile `return_to` values land on `/`),
`session-lifecycle.spec.ts` (forged logout redirect not followed).

### OIDC responses and configuration (gateway, M3)

Callback query capped at 4 KB; `state` compared in constant time; id_token
claims validated (IA-2); roles bounded (64 entries, 128 bytes each).
Configuration fails closed: a missing or invalid key stops startup with the
key name only (`Config::from_lookup`). Tests:
`config::tests::missing_required_keys_fail_closed`,
`invalid_values_rejected`, `oidc_verify::role_filtering_bounds`.

### `/api/session` decoding (shell, M3)

The shell's `FromJs` decoder for `SessionInfo` rejects non-array `roles`
and is checked against the frozen v1 fixture. Tests (wasm, `crates/console-shell/src/api.rs`): `fixture_decodes_like_serde`,
`roles_must_be_an_array`, `missing_field_is_rejected`;
protocol `frozen_v1_fixture_still_parses`.

## SI-11 Error handling

**Status: Implemented** for scope URL errors and gateway auth errors.

`crates/console-shell/src/scope.rs`: errors carry only the key name, never the
value. The shell shows one Warning flash, for example `Ignored invalid scope in
URL: site, tenant`, or `... : query too long` for oversize queries. Cascade
announcements are built from allow-list labels only. `same_origin_url` (see
SI-10) prevents a crafted path from turning into a wasm panic.

Tests: `errors_never_contain_value` (`scope.rs`); E2E invalid-value cases in
`e2e/tests/scope.spec.ts` check that the value appears in neither the DOM nor
the title.

**M3 gateway errors.** 401 and 403 bodies are fixed JSON
(`{"error", "reason", "login_url"}`); 503, 413, 429 and unknown `/api/*`
paths return fixed `{"error": ...}` JSON for XHR and small static HTML pages
for navigations. Gateway pages reflect no request input. Config errors
name the key, never the value. Tests:
`routes::tests::unauthenticated_json_shape`, `json_errors_and_xhr_detection`,
`pages_reflect_nothing`, `config::tests::direct_db_url_warns` (no password
in warnings).

## SC-8 Transmission confidentiality and integrity

**Status: Implemented (dependency).** The gateway listens on plain HTTP
(`CONSOLE_LISTEN`) and does not terminate TLS; a TLS-terminating ingress or
proxy is a deployment requirement ([DEPLOYMENT.md](../DEPLOYMENT.md#tls)).
Within the gateway:

- `Strict-Transport-Security: max-age=31536000; includeSubDomains` on every
  response (`src/lib.rs`, annotated `SC-8 (HSTS), SC-18 (CSP)`). No test
  asserts the HSTS header.
- `CONSOLE_PUBLIC_URL` and `CONSOLE_OIDC_ISSUER` must be `https`, except
  for loopback hosts. Test: `config::tests::https_public_url_and_proxies`.
- A DB URL with a non-loopback host must use `sslmode=verify-full` or
  `verify-ca`, else the gateway refuses to start (downgraded to a warning
  only with `CONSOLE_DEV_MODE=true`). Applies to the main, audit and
  migrator URLs. Test: `config::tests::remote_db_requires_verified_tls`.
- All cookies are `Secure`. Test: `session::tests::cookie_flags`.
- IdP HTTP client: rustls, no redirects followed.

## SC-13 Cryptographic protection

**Status: Implemented.** Session ids and CSRF tokens are 32 bytes from the
OS RNG; only `sha256(session id)` is stored; PKCE S256 with a 64-byte
verifier; id_token signatures verified with RS256, PS256 or ES256 only
(`alg=none` and HS256 rejected); TLS via rustls with `ring`.
Tests: `session::tests::opaque_ids`,
`oidc_verify::rejects_wrong_key_and_alg_none`,
`authorize_url_has_pkce_s256_nonce_state`. Advisory waiver for `rsa`
(verify-only use): POAM-014.

## SC-18 Mobile code: Content Security Policy and browser hardening

**Status: Implemented.**

`src/headers.rs` `content_security_policy` builds:

```text
default-src 'self'; script-src 'self' 'wasm-unsafe-eval' <sha256 hashes>;
style-src 'self'; frame-src 'self'; frame-ancestors 'none'; base-uri 'none';
form-action 'self' <issuer origin>
```

At startup it hashes the inline `<script>` bodies in the served
`index.html`. It never uses `'unsafe-inline'`. M3 adds the IdP issuer
origin to `form-action` (derived from config, filtered against directive
injection) so the no-JS logout form may follow the 303 to Keycloak. The
gateway also sets `X-Content-Type-Options: nosniff` and
`Referrer-Policy: no-referrer`. Gateway pages (403, `/logged-out`, errors)
use the same-origin `/auth.css` and no inline styles.

Tests: `router::healthz_ok_with_security_headers` (`frame-ancestors
'none'`, issuer origin in `form-action`, no `unsafe-inline`, `nosniff`);
`headers::tests::hashes_only_inline_scripts` (hashing and the injection
case); `router::auth_css_is_public_and_same_origin`; `no_inline_styles`,
`no_raw_html` (`crates/console-ui/tests/guards.rs`); `index_html_is_csp_clean`
(`crates/console-ui/tests/qa_logic_css.rs`); E2E `csp.spec.ts` and
`dist-smoke.spec.ts` (green on all three engines in CI run 41740455231;
in M3 locally on Chromium and WebKit).

Not tested: the `Referrer-Policy` header value.

### Known benign CSP reports

WebKit reports a `style-src-attr` violation every time an `HTMLSelectElement`
is created, even with no app code (minimal repro with
`document.createElement('select')`). It is engine behaviour, not a style the
console sets. The tests allow it **on WebKit only**, and only when the report
matches exactly `{violatedDirective: "style-src-attr", blockedURI: "inline",
sample: ""}` with a target tag of `SELECT` or `null`, up to the number of
`<select>` elements plus 1 per page (`e2e/tests/csp-helpers.ts`). Every other
report fails on every engine, and Chromium must report 0. Tracked as
POAM-011 (re-reviewed at M3, unchanged).

## SC-23 Session authenticity

**Status: Implemented.** Opaque 256-bit session id in
`__Host-console_sid` (`Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`, no
`Domain`); only its sha256 is stored; a new id on every sign-in with the
old row deleted in the same transaction; a client-chosen id is never
adopted; CSRF synchronizer token plus Origin check on keepalive and logout.

Tests: `session::tests::cookie_flags`, `opaque_ids`,
`pg_session_fixation_rotates_id`, `pg_failed_callback_ends_presented_session`,
`router::malformed_session_cookie_is_not_adopted`, `csrf::tests::token_compare`,
`origin_rules`; E2E `auth.spec.ts` (cookie flags),
`session-security.spec.ts` (fixation, rotation, CSRF matrix).

## SC-28 Protection of information at rest

**Status: Partial.** Session ids are stored only as sha256 hashes. The
id_token and refresh token are stored in plaintext in `sessions`
(markers `-- POAM: SC-28` in `migrations/0003_roles_refresh.sql` and
`// POAM: SC-28` in `src/session.rs`; POAM-015). Disk or volume encryption
is a deployment concern and is not evidenced here.

## SA-11 Developer testing and evaluation

**Status: Implemented.**

`.github/workflows/ci.yml` has two test jobs, first green in CI run [41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231) on commit `0a6f40e`:

- `wasm-test` (comment `POAM-003 / SA-11`): `wasm-pack test --headless
  --firefox crates/console-ui`, uploads artifact `wasm-pack-log`.
- `e2e`: builds `dist`, `dist-gallery` and the gateway, runs `npm ci`, `tsc`,
  `npm audit --audit-level=high`, then `npx playwright test` on chromium,
  firefox and webkit plus the `dist-*` smoke projects; uploads artifact
  `playwright-report`.

CI evidence: `wasm-test` (headless Firefox) 16 passed; `e2e` on chromium,
firefox, webkit and the three `dist-*` projects 147 passed, 24 skipped (axe is
Chromium-only by design), 0 failed. Local runs (headless Chrome wasm; chromium
and webkit e2e) agree. POAM-003 to POAM-006 are closed; axe engine scope stays
open as POAM-012.

M3 additions, **not yet run in CI** (pending push, POAM-030):

- `gateway-integration`: Postgres service container, `01-roles.sh`,
  `console-gateway migrate` as `console_migrator`, `check-roles.sql`, then
  `cargo test -p console-gateway --locked -- --nocapture`; fails on any
  `NOT RUN` line. Locally (`make test-integration`): 15/15 `pg_flow`.
- `e2e` now starts the compose stack (Postgres, Keycloak, Caddy TLS) and
  runs the auth specs; `wasm-test` also runs the console-shell wasm tests.
  Locally: Chromium and WebKit 184 passed, 0 failed; console-shell wasm
  tests 4 passed (headless Chrome). Firefox: not run.

## SA-11(1) Static code analysis

**Status: Implemented.**

`make lint` and the CI `rust` job (comment `SA-11(1)` in
`.github/workflows/ci.yml`) run `cargo clippy ... -D warnings` on the host
workspace (all targets) and on wasm32 for console-shell (with and without
`gallery`) and console-ui (all targets). A warning fails the build.

## SR-3, SR-11, CM-14 Supply chain controls

**Status: Partial.** The build inputs are pinned; signatures are not verified.

- Every third-party GitHub Action in `.github/workflows/ci.yml` is pinned to a
  full commit SHA with a version comment. `trunk-action` pins trunk
  `v0.22.1`, the same version the Containerfile installs.
- `deploy/Containerfile` pins `rust:1.99-bookworm` and
  `distroless/cc-debian12:nonroot` by `@sha256` digest, and installs
  `trunk@0.22.1 --locked`.
- `e2e/.npmrc` sets `save-exact=true`, `ignore-scripts=true` and
  `engine-strict=true`. `e2e/package.json` requires Node `>=24`, CI uses Node
  24, and `npm audit --audit-level=high` fails the CI job.

Why only Partial: these are configuration, not tests. The digest-pinned
container build and the SHA-pinned workflow ran green in CI run [41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231). There is no signature or provenance verification, and no automated update
process (Dependabot/Renovate) for the pins.

## PL-8 Security and privacy architecture: scope

**Status: Partial (documented design rule, nothing to enforce yet).**

The global scope (site, tenant, environment) is a **UI filter, not an
authorization claim**. It lives only in the URL query (never in
`localStorage`; E2E "scope is never written to localStorage"). Upstreams must
not trust scope values a tool echoes back, and M4 must re-authorize scope on
the server. No server-side check exists yet.

## RA-5 / SI-2 Dependency vulnerability scanning

**Status: Partial.**

`make deny` and the CI `cargo-deny` job run `cargo deny check`. `make audit`
runs `cargo audit`. Three advisories are waived in `deny.toml`
(RUSTSEC-2024-0370, RUSTSEC-2025-0141, RUSTSEC-2023-0071). All are recorded
in the POA&M (POAM-001, POAM-002, POAM-014).

These are gates, not tests of the system. Flaw remediation (SI-2) has no
process documented in code yet.

## Planned controls

- **AC-3, AC-6 service-menu filtering**: the service menu is static demo
  data, not filtered by role (POAM-010, M4 registry).
- **Per-tool proxy controls** (identity-header allow-list, SEC-010): M4
  (POAM-026).
- **AU audit retention and SIEM export**: no code (see
  [AUDIT_LOGGING.md](AUDIT_LOGGING.md)).
- **SC-28 token encryption at rest** and **AU-9 external audit store**: M7
  (POAM-015, POAM-017).

## Related evidence (not a NIST control)

WCAG 2.1 AA colour contrast: test `token_pairs_meet_wcag_aa` in
`crates/console-ui/tests/contrast.rs` checks 40 declared token pairs in each
theme against 4.5:1 (text) or 3:1 (UI). axe scans run in
`e2e/tests/a11y.spec.ts` on Chromium only (0 serious/critical locally and
in CI run [41740455231](https://github.com/192d-Wing/usg-ops-console/actions/runs/41740455231)). See POAM-004 and POAM-012.
