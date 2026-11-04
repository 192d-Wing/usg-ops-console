# Session management

Status as of M3 (Keycloak OIDC sign-in, server-side sessions, logout, idle
timeout), 2026-11-03. This file describes what the code does today.

**Evidence status.** The gateway unit tests, `oidc_verify` and `router`
tests, and the 15 `pg_flow` integration tests (`make test-integration`
against compose Postgres 17, 0 `NOT RUN` lines) passed locally. The
Playwright suite passed locally on Chromium and WebKit (184 passed, 0
failed, 13 skipped by design; fix round 2). **Firefox has not run**, and
the new CI jobs (`gateway-integration`, the compose-based `e2e` job) have
not run yet (pending push). See POAM-030.

## Overview

`console-gateway` is the authenticated front door. Every route except the
public ones below requires a server-side session (`require_session` in
`crates/console-gateway/src/routes.rs`). The session is created only after
a Keycloak OIDC authorization-code flow with PKCE (see
[ARCHITECTURE.md](../ARCHITECTURE.md#authentication-flow)).

Public paths (no session): `/healthz`, `/login`, the OIDC callback
(`CONSOLE_OIDC_REDIRECT_PATH`, default `/callback`), `/logged-out` and
`/auth.css` (`PUBLIC_PATHS` in `routes.rs`). Everything else, including
`index.html`, the wasm bundle and `/gallery`, needs a session.

Tests: `router::every_non_public_route_requires_a_session`,
`router::public_pages_need_no_session`; E2E `session-security.spec.ts`.

## Cookies

All cookies are built by `session::set_cookie` (`crates/console-gateway/src/session.rs`)
with `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`, no `Domain`, and the
`__Host-` prefix.

| Cookie | Value | Lifetime | Purpose |
| --- | --- | --- | --- |
| `__Host-console_sid` | 32 random bytes from the OS RNG, base64url | Browser session (no `Max-Age`); the server-side expiry is authoritative | Session id. The DB stores only `sha256(raw id)` (`sessions.id_hash`) |
| `__Host-console_auth` | random handle for the pending sign-in | `Max-Age=600` | Binds `/callback` to the browser that called `/login`. Deleted after the callback |
| `__Host-console_reauth` | `1` | `Max-Age` = `CONSOLE_SESSION_ABSOLUTE_SECS` | Reauthentication marker (see below) |

`SameSite=Lax` (not `Strict`) is required because the IdP's redirect to
`/callback` is a cross-site top-level navigation that must carry
`__Host-console_auth`. State-changing endpoints are protected by CSRF checks
instead (below).

No token, session id or CSRF token is stored in `localStorage` or
`sessionStorage`, and `document.cookie` cannot read the session id
(HttpOnly). The shell stores only `cui-theme` in `localStorage`. The
global scope (site, tenant, environment) lives only in the URL query.

Tests: `session::tests::cookie_flags`, `session::tests::opaque_ids`,
`router::malformed_session_cookie_is_not_adopted`; E2E `auth.spec.ts`
(cookie flags, host-only, at least 256-bit id, auth cookie gone after the
callback, no sid in `document.cookie`, no JWT/token/csrf in storage),
`scope.spec.ts` ("scope is never written to localStorage").

WebKit drops `__Host-`/`Secure` cookies on `http://localhost`, so the local
stack and E2E reach the gateway only through a Caddy TLS proxy. The cookie
flags are never weakened for development.

## Server-side session record

Table `sessions` (`crates/console-gateway/migrations/0001_sessions.sql`,
`0003_roles_refresh.sql`): `id_hash`, `user_sub`, `username`,
`display_name`, `roles`, `csrf_token`, `id_token`, `refresh_token`,
`created_at`, `last_seen_at`, `absolute_expires_at`, `client_ip`,
`user_agent` (truncated to 256 bytes).

- **Rotation (SC-23).** Every successful sign-in creates a new id. A session
  id presented to `/callback` is deleted in the same transaction, so a
  planted id is never adopted (no session fixation). Tests:
  `pg_session_fixation_rotates_id`; E2E `session-security.spec.ts`
  (fixation, rotation on re-login).
- **Failed or denied callbacks** (after the `state` check passes) delete the
  presented session, clear the cookie, and audit `logout` reason
  `superseded`. Before the `state` check nothing is touched. Test:
  `pg_failed_callback_ends_presented_session`.
- **Roles** are filtered to `console-*` values (at most 64, each at most 128
  bytes). The required role (`CONSOLE_REQUIRED_ROLE`, default
  `console-user`) is checked on the unfiltered list. Test:
  `oidc_verify::role_filtering_bounds`.
- `id_token` and `refresh_token` are stored server-side **in plaintext**
  (POAM-015, SC-28). They never appear in `/api/session`, cookies, `Debug`
  output or logs.

## Idle and absolute timeouts (AC-11, AC-12)

| Setting | Key | Default | Allowed range |
| --- | --- | --- | --- |
| Idle timeout | `CONSOLE_SESSION_IDLE_SECS` | 900 (15 min) | 60-3600, and less than absolute |
| Absolute lifetime | `CONSOLE_SESSION_ABSOLUTE_SECS` | 43200 (12 h) | 900-86400 |

- Both deadlines are checked on **every** request in `require_session`.
  Deadlines are exclusive, and absolute wins when both have passed
  (`session::expiry`).
- **Only an explicit "Stay signed in" extends the idle deadline** (user
  decision Q6, strict AC-11). Ordinary requests, including
  `GET /api/session`, never move `last_seen_at`. Keepalive never moves the
  absolute deadline.
- An expired session found on access is deleted, audited
  (`session.expired.idle` or `session.expired.absolute`, reason
  `on_access`), the sid cookie is cleared, the reauth marker is set, and the
  client gets 401 (XHR) or a 302 to `/login?return_to=...&reason=expired`
  (navigation).
- The sweeper (`crates/console-gateway/src/sweeper.rs`) deletes expired
  rows every `CONSOLE_SWEEP_INTERVAL_SECS` (default 60, range 5-3600), at
  most 1000 per pass, and audits them with reason `swept`.

Tests: `session::tests::expiry_boundaries`, `pg_idle_and_absolute_expiry`,
`pg_sweeper_deletes_and_audits`, `pg_full_flow_and_audit` (reads do not
extend idle, keepalive does); E2E `idle.spec.ts` (a GET does not extend
idle; overlay clicks do not extend), `session-lifecycle.spec.ts`
(DB-forced idle expiry gives 401 `idle_timeout`).

Not covered by E2E: absolute-expiry reauthentication in the browser (only
idle is exercised end to end; absolute is covered by `pg_flow`).

### Idle warning dialog (shell)

`crates/console-shell/src/idle.rs` (`IdleWatch`):

- The deadline is `min(idle_expires_at, absolute_expires_at)`, corrected for
  client clock skew using `server_time` from `/api/session`.
- The warning opens `min(120 s, idle_timeout_secs / 2)` before the deadline
  (`warn_lead_ms`). Test: `lead_is_min_of_120s_and_half_idle`.
- Buttons: "Stay signed in" (initial focus; sends the keepalive) and "Sign
  out". Escape = "Stay signed in" (decision Q4). **Overlay clicks do
  nothing** and there is no close (X) button (`dismiss_on_overlay=false`,
  `close_button=false`). Tests: console-ui wasm
  `modal_overlay_click_can_be_disabled`; E2E `idle.spec.ts`.
- When only the absolute deadline remains, the dialog says the session
  cannot be extended; "Continue" closes it without extending.
- A polite live region outside the dialog announces at 60 s and 30 s
  (only thresholds below the lead). At 0 the shell navigates to
  `/login?return_to=<path+query>&reason=expired` with `location.replace`.
- Keepalive errors (for example 503) appear inline in the dialog
  (`role=alert`); the session is not extended.

## Keepalive and IdP revalidation (AC-2, AC-12)

`POST /api/session/keepalive` (`routes::keepalive`):

1. CSRF check (header token and Origin). Failure: 403 and `access.denied`.
2. Refresh-token grant against Keycloak with the stored refresh token
   (`oidc::refresh_session`).
3. If the IdP rejects the grant (disabled user, revoked SSO session,
   expired refresh token), the refreshed id_token is invalid, the subject
   changed, there is no refresh token, or the refreshed roles lack the
   required role: the session is deleted, `session.revoked` is audited with
   reason `idp_rejected`, `token_invalid`, `subject_mismatch`,
   `no_refresh_token` or `missing_role`, the reauth marker is set, and the
   response is 401 `{"error":"unauthenticated","reason":"revoked"}`.
4. If the IdP is unreachable or times out (5 s connect, 10 s total): 503
   `{"error":"unavailable"}`. The session is neither extended nor ended
   (POAM-019).
5. On success: roles, id_token and refresh token are rewritten from the
   refresh response, `last_seen_at` moves to now, and the new
   `SessionInfo` is returned. `session.refresh` is audited at most once per
   60 s per session.

Tests: `pg_keepalive_revalidates_with_idp`; E2E `revocation.spec.ts`
(disabled user gives 401 `revoked`; losing `console-user` gives 401).

Keycloak requirements for this to work are in
[DEPLOYMENT.md](../DEPLOYMENT.md#keycloak-client-requirements)
(refresh tokens on, `revokeRefreshToken=false`, client session idle at
least the console idle).

## Reauthentication after expiry or sign-out (AC-11, IA-11)

Keycloak's SSO session can outlive a console session. To stop a silent
re-login after an idle or absolute expiry or a sign-out:

- `__Host-console_reauth` is set on logout, on expiry detected on access,
  when a presented sid no longer exists, and on keepalive revocation. Only a
  successful fresh sign-in clears it.
- While the marker is present, or when `/login` gets `reason=expired` or
  `reason=ended`, `/login` sends `prompt=login` and records when the
  request was issued.
- The callback then requires the id_token `auth_time` to be no earlier than
  the request time minus 30 s. Otherwise: 400 page and `login.failure`
  reason `stale_auth`.

Tests: `pg_reauth_after_logout_requires_fresh_auth`,
`pg_expired_reason_forces_prompt_and_notice`,
`oidc_verify::authorize_url_reauth_and_acr`; E2E
`session-lifecycle.spec.ts` (re-auth form shown despite a live SSO session).
The `stale_auth` rejection path is not exercised by E2E (it needs an IdP
that returns an old `auth_time`).

Residual: the marker is a browser cookie. A user who clears cookies after a
session ends gets normal SSO reuse on the next sign-in. The other bound is
the icam client SSO idle/max, which must be at most the console values
(POAM-029).

## Assurance level (IA-2(12))

`CONSOLE_REQUIRED_ACR` (comma-separated) is required unless
`CONSOLE_DEV_MODE=true`. The values are sent as `acr_values`, and the
id_token `acr` must be one of them. Otherwise: 403 "CAC/PIV sign-in
required", no session, `login.denied` reason `insufficient_assurance`.
Tests: `config::tests::acr_required_outside_dev_mode`,
`pg_insufficient_assurance_is_denied`; E2E `assurance.spec.ts`. The
production value that icam's X.509/CAC flow emits is not yet confirmed
(POAM-028).

## Not authorized (403)

A user without the required role gets a 403 "Not authorized" page. No
session is created, and `login.denied` reason `missing_role` is audited
(decision Q8). The 403 pages link to "Sign out of single sign-on"
(end_session with `client_id` and `post_logout_redirect_uri`) and show
help-desk text. Pages reflect no request input. Tests:
`pg_missing_role_is_denied_without_session`, `routes::tests::pages_reflect_nothing`;
E2E `auth.spec.ts` (`no_role` user, axe).

## CSRF

`POST /logout` and `POST /api/session/keepalive` require:

- the per-session synchronizer token (32 bytes, from `/api/session`
  `csrf_token`), sent as `X-CSRF-Token` or the `csrf` form field and
  compared in constant time; and
- an `Origin` (or `Referer`) equal to `CONSOLE_PUBLIC_URL`.

Failure: 403 `{"error":"csrf","reason":"csrf_token"|"origin"}` and an
`access.denied` audit row. `GET /logout` does not sign out.

Tests: `csrf::tests::token_compare`, `origin_rules`, `check_reports_reason`;
`pg_full_flow_and_audit` (CSRF matrix); E2E `session-security.spec.ts`,
`session-lifecycle.spec.ts` (denials audited).

## Logout (AC-12)

`POST /logout`:

1. CSRF check.
2. The session row (with its id_token and refresh token) is deleted; the
   sid cookie is cleared; the reauth marker is set; `logout` reason `user`
   is audited.
3. The browser is sent to Keycloak's `end_session_endpoint` with
   `id_token_hint`, `client_id` and
   `post_logout_redirect_uri=<CONSOLE_PUBLIC_URL>/logged-out`, so the SSO
   session ends without a confirmation page (decision Q2).
   - XHR (request has `X-CSRF-Token` or `Accept` contains
     `application/json`): 200 JSON `{"redirect": "<end-session URL>",
     "end_session_origin": "<issuer origin>"}`.
   - Otherwise (no-JS form): 303 to the end-session URL. CSP `form-action`
     includes the issuer origin so the browser may follow it.
   - If the IdP has no end-session endpoint, `redirect` is the console's own
     `/logged-out`.

The shell follows `redirect` only if its origin equals
`end_session_origin`, or it is the console's own `/logged-out`, and uses
`location.replace` (no history entry). Other tabs find the session gone on
their next request or `visibilitychange` and go to sign-in.

Tests: `oidc_verify::end_session_url_carries_hint_and_client`,
`pg_form_logout_with_json_accept_and_limits`, shell
`logout_redirect_only_to_end_session_origin_or_logged_out`; E2E
`auth.spec.ts` (id_token_hint, SSO really ended, two tabs),
`session-lifecycle.spec.ts` (forged redirect ignored, `history.length`
unchanged). The no-JS form path is covered by `pg_flow` only, not by E2E.

### The one token that reaches the browser

The **id_token**, as `id_token_hint`, is the only token that ever reaches
the browser, and only inside the end-session URL at sign-out (JSON
`redirect` or 303 `Location`). It is never in `/api/session`, cookies,
storage or console URLs. Exposure: the browser navigates to that URL, so
it can appear in IdP and ingress access logs. `Referrer-Policy:
no-referrer` and `location.replace` limit the Referer and history exposure.
Operators must strip the query string from end-session access logs, and
the icam id_token lifespan should be at most 5 minutes. A POST-based
logout to the IdP is tracked as POAM-021.

## `/api/session`

`GET /api/session` returns `console_protocol::session::SessionInfo`
(`Cache-Control: no-store`): `user {id, username, display_name}`, `roles`,
`idle_expires_at`, `absolute_expires_at`, `idle_timeout_secs`,
`server_time`, `csrf_token`. It never includes the id_token or refresh
token. Without a valid session: 401 JSON (see
[ARCHITECTURE.md](../ARCHITECTURE.md#gateway-http-contract)).

## Return path after sign-in (SI-10)

`return_to` is sanitized by `crates/console-gateway/src/return_to.rs`:
same-origin absolute paths only; `//`, `/\`, encoded slashes, control and
non-ASCII characters, auth endpoints and over-long values fall back to `/`.
`return_to` is stored server-side with the pending request and is never
sent to the IdP. After sign-in following an expiry, `console_notice=expired`
or `ended` is appended (fixed values only); the shell shows an info flash
and strips the parameter.

Tests: `return_to::tests::accepts_plain_paths`,
`rejects_open_redirects_and_loops`, `notices_are_fixed_values`; E2E
`session-security.spec.ts` (17 hostile values land on `/`).
