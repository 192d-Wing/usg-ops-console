# Architecture (current state, M3)

This describes what is built today. The target design and the milestones are
in [PLAN.md](PLAN.md). M3 added Keycloak OIDC sign-in, server-side
sessions, logout, idle and absolute timeouts, and the audit log. The tool
registry, the reverse proxy and the iframe tool container are **not built
yet** (M4).

## Crates

| Crate | What it does today |
| --- | --- |
| `console-protocol` | Serde types for the tool manifest, shell/tool SDK messages, and (M3) the gateway session HTTP shapes (`session::SessionInfo`, `AuthErrorBody`, `LogoutResponse`). |
| `console-sdk` | Tool-side encode/decode with origin checks. No transport yet (M4). |
| `console-ui` | Yew 0.23 component library, design tokens (`tokens.css`) and component styles (`components.css`). See [crates/console-ui/README.md](../crates/console-ui/README.md). |
| `console-shell` | Yew CSR app built with Trunk: skip link, top navigation (service menu, global search, user menu, theme toggle), scope picker, page header and breadcrumbs, flashbar, collapsible side navigation, router, and an optional component gallery. |
| `console-gateway` | axum server: OIDC sign-in, server-side sessions (Postgres), logout, audit log, security headers, and the shell's static `dist` behind the session check. |

## console-shell

- Routing: `yew-router` `BrowserRouter` with `Home` (`/`), `NotFound`, and
  `Gallery` (`/gallery`). `Gallery` exists only when the `gallery` Cargo
  feature is on. The feature is off by default (CM-7).
- On a route change the shell sets `document.title` to
  `<page> - USG Ops Console` and moves focus to `#main-content`. It does not
  move focus on the first load.
- Layout: `#app-chrome` holds the chrome and main content. `#modal-root` is
  next to it. Modals are portalled into `#modal-root`. While a modal is open,
  it sets `inert` on `#app-chrome` (through the `Modal` `inert_root` prop).
- Theme (`src/theme.rs`): cycles system, light, dark. It stores the choice in
  `localStorage["cui-theme"]` and sets or removes `data-theme` on `<html>`.
  Stored values are checked against an allow-list (SI-10). Storage and DOM
  failures are reported in the flashbar.
- Stylesheets: `index.html` links `console-ui/assets/tokens.css`,
  `console-ui/assets/components.css` and `assets/shell.css` as Trunk CSS
  assets. There are no inline styles or scripts in `index.html`.

### Chrome (`src/chrome.rs`, `src/demo.rs`)

- `TopNavigation` banner with `ServiceMenu` (a disclosure `Dropdown` of the
  static `SERVICES` demo list, 6 entries in 4 categories), `GlobalSearch`,
  `UserMenu` and the theme toggle.
- `ServiceMenu` is unfiltered demo data. It is not access control
  (POAM-010; role filtering comes from the M4 registry).
- `UserMenu` shows the signed-in user's `display_name` from `/api/session`
  ("Signed in as `<username>`") and a working Sign out (POST `/logout` with
  the CSRF token). While loading it shows a skeleton; if `/api/session`
  fails it shows "Session unavailable" with Retry and Sign in. A
  `role=status` region announces "Signing out…".
- `GlobalSearch` is display-only: input truncated to 256 characters, submit
  shows "Search is not available yet", and the query is never echoed, stored
  or put in the URL.
- `ScopePicker` sits in a `.scopebar` row below the banner (with the SDK
  protocol version text): three native `<select>`s (Site, Tenant,
  Environment) in a `role=group` labelled "Scope", and a polite live region
  (`scope-announcer`).
- Page header (H1) and breadcrumbs come from `PageMeta`.
- Side navigation: at `(min-width: 45rem)` (`NAV_BREAKPOINT` in `main.rs`) it
  is always shown. Below that it is hidden behind a `NavToggle` disclosure.
  The initial state comes from `matchMedia(NAV_BREAKPOINT)`, and a change
  listener keeps it in sync. Below 45rem the top-bar dropdown panels are
  anchored to the top bar so they stay inside a 320 px viewport.

### Contexts (`src/ctx.rs`)

- `ShellCtx { flash, set_page }`: `flash` pushes a flashbar message;
  `set_page` sets the `PageMeta` (title, description, breadcrumbs) for the
  current route. The shell stores the meta with the route path it was set for
  and falls back to a default built from the route, so stale crumbs are never
  shown. Pages use the `use_page_meta` hook.
- `ScopeCtx { scope, set }`: the current `console_protocol::Scope` and a
  setter. It is a separate context so flash-only consumers do not re-render
  on scope changes.
- Both values are memoized (`use_memo`) and their callbacks are stable
  (`use_callback`, `use_reducer` for flashes), so consumers re-render only when
  the scope or route changes.

### Global scope (`src/scope.rs`)

The scope is site, then tenant, then environment. The demo hierarchy is the
`SITES` tree in `src/demo.rs`; tenants exist per site and environments per
(site, tenant).

Cascade: the Tenant select is disabled until a site is chosen, and
Environment until a tenant is chosen (with a visible hint wired through
`aria-describedby`). Changing a higher level clears a lower level that is not
valid under the new choice and announces it politely, for example "Tenant and
environment cleared: not available at West DC". A tenant valid at both sites
is kept.

URL rules (`parse_query` / `to_query`):

- Keys `site`, `tenant`, `env` only; exact, case-sensitive. Other parameters
  and the hash are preserved in order.
- No percent-decoding. Values must match `[a-z0-9][a-z0-9-]{0,62}` and exist
  in the hierarchy. Tenant needs a valid site; env needs a valid tenant.
- Empty value or bare key means "All", with no error. A repeated key is an
  error. Over 2048 bytes or 32 pairs ignores the whole query.
- Errors name only the key, in one Warning flash ("Ignored invalid scope in
  URL: site, tenant"). The URL is then cleaned with `replaceState`.
- `to_query` writes keys in the order site, tenant, env and never emits an
  empty `?`.

History semantics:

- Scope is derived from the URL on startup and on every location change,
  including `popstate` (`use_location`), so Back and Forward restore the full
  prior scope.
- Each picker change is one `push`; cascaded clears go into the same entry.
- Following an in-app link (side nav, breadcrumbs, service menu) appends the
  current scope keys to the target path.
- History is written through yew-router's shared `BrowserHistory`. Every URL
  passes through `same_origin_url`, which collapses leading `/` or `\` runs to
  one `/`.
- Scope is never stored in `localStorage`.

Scope is a UI filter, not an authorization claim (PL-8). Servers must not
trust it; M4 must re-authorize scope on the server.

### Session (`src/session.rs`, `src/api.rs`, `src/idle.rs`)

- `use_session` loads `/api/session` on mount and when the tab becomes
  visible. Fetches use `credentials: same-origin` and send `X-CSRF-Token`
  on POSTs. JSON is decoded with the browser's `Response.json()` into the
  protocol types (no `serde_json` in the bundle).
- Any 401 goes to `/login?return_to=<path+query>` (built from
  `window.location` only) with `location.replace`; `reason=expired` or
  `ended` is added after an expiry or revocation. After sign-in the shell
  shows an info flash for `console_notice` and strips that parameter.
- `IdleWatch` shows the idle warning dialog `min(120 s, idle/2)` before
  the deadline (see [compliance/SESSION_MANAGEMENT.md](compliance/SESSION_MANAGEMENT.md)).
- Sign out follows the gateway's `redirect` only to `end_session_origin`
  or the console's own `/logged-out`.

## console-gateway

The gateway is the authenticated front door: Keycloak OIDC sign-in,
server-side sessions in Postgres, logout, idle and absolute timeouts, and
the audit log. Modules (`crates/console-gateway/src/`): `config` (env,
fail closed), `oidc` (discovery, authorize URL, code exchange, id_token
verification, refresh grant, end-session URL), `session` (ids, cookies,
expiry math, `SessionStore`), `routes` (handlers and `require_session`),
`csrf`, `return_to`, `client_ip`, `headers`, `rate_limit`, `audit`,
`sweeper`. Session and cookie rules: [compliance/SESSION_MANAGEMENT.md](compliance/SESSION_MANAGEMENT.md).
Audit events: [compliance/AUDIT_LOGGING.md](compliance/AUDIT_LOGGING.md).

The binary has two subcommands: `console-gateway` / `console-gateway serve`
(the server) and `console-gateway migrate` (one-shot migrations, see
[DEPLOYMENT.md](DEPLOYMENT.md#database-roles-and-migrations)).

Startup order (`src/main.rs`): config (exit 1 naming the missing or invalid
key), `index.html`, main pool, migrations if `CONSOLE_MIGRATOR_DATABASE_URL`
is set (else a schema check: `database schema not migrated; run
console-gateway migrate`), audit pool, OIDC discovery with retries, sweeper,
then serve with graceful shutdown on SIGINT/SIGTERM.

### Authentication flow

```text
Browser                 Gateway                         Keycloak (icam)      Postgres
  | GET /some/page        |                                 |                  |
  |---------------------->| require_session: no/expired sid |                  |
  |<-- 302 /login?return_to=/some/page[&reason=expired] ----|                  |
  | GET /login            |                                 |                  |
  |---------------------->| rate limit; sanitize return_to; |                  |
  |                       | new state, nonce, PKCE verifier | INSERT auth_requests
  |                       |-------------------------------------------------->|
  |<-- 302 authorize?code_challenge(S256)&state&nonce&acr_values[&prompt=login]
  |    Set-Cookie: __Host-console_auth (600 s)              |                  |
  |------------------------------------------------------->| CAC/PIV (X.509)  |
  |<-- 302 /callback?code&state ----------------------------|                  |
  | GET /callback         |                                 |                  |
  |---------------------->| take auth_request (single use) <--------------------|
  |                       | check state; code exchange ---->|                  |
  |                       |<---- id_token, refresh_token ---|                  |
  |                       | verify sig/iss/aud/azp/nonce/exp; acr; role;        |
  |                       | auth_time if reauth             |                  |
  |                       | tx: delete presented sid, INSERT session ---------->|
  |                       | audit login.success (fail closed), commit -------->|
  |<-- 302 return_to[?console_notice=...]                   |                  |
  |    Set-Cookie: __Host-console_sid; clear auth/reauth    |                  |
  | GET /api/session      |                                 |                  |
  |---------------------->| SessionInfo + csrf_token (no tokens)               |
  | POST /api/session/keepalive (X-CSRF-Token)              |                  |
  |---------------------->| CSRF + Origin; refresh grant -->|                  |
  |                       |<---- new id_token (roles) ------| UPDATE session ->|
  | POST /logout (X-CSRF-Token)                             |                  |
  |---------------------->| DELETE session; audit logout -------------------->|
  |<-- {"redirect": end_session?id_token_hint&client_id&post_logout_redirect_uri}
  |------------------------------------------------------->| end SSO session  |
  |<-- 302 /logged-out -------------------------------------|                  |
```

Denials: no required role gives 403 "Not authorized"; an `acr` that is not
in `CONSOLE_REQUIRED_ACR` gives 403 "CAC/PIV sign-in required"; neither
creates a session. Both are audited as `login.denied`.

### Gateway HTTP contract

Public routes (no session): `GET /healthz`, `GET /login`,
`GET <CONSOLE_OIDC_REDIRECT_PATH>` (default `/callback`),
`GET /logged-out`, `GET /auth.css`. Everything else is behind
`require_session`, including the SPA static files.

| Route | Success | Notes |
| --- | --- | --- |
| `GET /api/session` | 200 `SessionInfo` (`console_protocol::session`): `user {id, username, display_name}`, `roles`, `idle_expires_at`, `absolute_expires_at`, `idle_timeout_secs`, `server_time`, `csrf_token` | Does not extend the session |
| `POST /api/session/keepalive` | 200 `SessionInfo` | Requires `X-CSRF-Token` and a same-origin `Origin`. Performs an IdP refresh grant. 401 `revoked` if the IdP or roles say no; 503 `unavailable` if the IdP is unreachable (not extended) |
| `POST /logout` | XHR (has `X-CSRF-Token`, or `Accept` contains `application/json`): 200 `LogoutResponse {redirect, end_session_origin?}`. Otherwise 303 to `redirect` | CSRF token as header or `csrf` form field (body at most 4 KB). `GET /logout` does not sign out |
| `/api`, `/api/`, `/api/*` (unknown) | 404 `{"error":"not_found"}` | After the session check |
| any other path | SPA static file or `index.html` | |

Error responses (`Cache-Control: no-store`; JSON for XHR, small static
HTML pages for navigations, decided by `Sec-Fetch-Mode: navigate` or
`Accept: text/html`):

| Status | Body | When |
| --- | --- | --- |
| 401 | `{"error":"unauthenticated","reason":"no_session"\|"idle_timeout"\|"absolute_timeout"\|"revoked","login_url":"/login"}` plus `WWW-Authenticate` | No, unknown or expired session (XHR). Navigations get 302 to `/login?return_to=...` (with `&reason=expired` after an expiry) |
| 403 | `{"error":"csrf","reason":"csrf_token"\|"origin","login_url":"/login"}` | CSRF failure on keepalive or logout |
| 403 | HTML page | Missing role or insufficient assurance at the callback |
| 400 | HTML page | Callback failure, or stale `auth_time` on a required reauthentication |
| 413 | `{"error":"payload_too_large"}` | Logout form body over 4 KB |
| 429 | `{"error":"rate_limited"}` plus `Retry-After: 60` | `/login` over 30 per minute per IP |
| 503 | `{"error":"unavailable"}` (plus `Retry-After` when the pending-sign-in cap is hit) | DB down, IdP down on keepalive, or more than 10,000 pending sign-ins |

Clients must tolerate unknown `error` and `reason` values (the protocol
enums fall back to `Unknown`). `PROTOCOL_VERSION` is unchanged (1): these
are HTTP shapes, not postMessage shapes. The v1 fixture is
`crates/console-gateway/tests/fixtures/session_v1.json` (frozen).

Every response gets:

- `Content-Security-Policy`: `default-src 'self'`, `script-src 'self'
  'wasm-unsafe-eval'` plus a SHA-256 hash of each inline script in
  `index.html` (computed at startup), `style-src 'self'`, `frame-src 'self'`,
  `frame-ancestors 'none'`, `base-uri 'none'`, `form-action 'self'
  <issuer origin>`
- `X-Content-Type-Options: nosniff`
- `Referrer-Policy: no-referrer`
- `Strict-Transport-Security: max-age=31536000; includeSubDomains`

Requests time out after 30 s. Client-supplied `x-console-user`,
`x-console-roles`, `x-console-session` and `x-forwarded-user` headers are
dropped. Request trace spans record method and path only (no query). The
gateway does not terminate TLS.

### Configuration

Environment variables (`src/config.rs`). A missing required key or an
invalid value stops startup with the key name (never the value).

| Variable | Default | Meaning |
| --- | --- | --- |
| `CONSOLE_LISTEN` | `0.0.0.0:8080` | Listen address (plain HTTP) |
| `CONSOLE_DIST` | `crates/console-shell/dist` | Directory served as the SPA |
| `CONSOLE_PUBLIC_URL` | required | External origin (https, except loopback). Used for redirect URIs, Origin checks, `post_logout_redirect_uri` |
| `CONSOLE_OIDC_ISSUER` | required | Keycloak realm issuer URL (https, except loopback) |
| `CONSOLE_OIDC_CLIENT_ID` | required | OIDC client id |
| `CONSOLE_OIDC_CLIENT_SECRET_FILE` / `CONSOLE_OIDC_CLIENT_SECRET` | one required | Client secret. The direct form logs a warning |
| `CONSOLE_OIDC_REDIRECT_PATH` | `/callback` | Callback path |
| `CONSOLE_OIDC_DISCOVERY_RETRIES` | `10` | Discovery retries at startup (0-1000, 2 s apart) |
| `CONSOLE_DATABASE_URL[_FILE]` | required | Runtime role (`console`, member of `console_app`) |
| `CONSOLE_AUDIT_DATABASE_URL[_FILE]` | required | Audit writer (`console_audit`) |
| `CONSOLE_MIGRATOR_DATABASE_URL[_FILE]` | unset | Migrator (`console_migrator`). Required by `migrate`. If set for `serve`, it migrates at startup (dev only; logs a warning) |
| `CONSOLE_DB_MAX_CONNECTIONS` | `10` | Main pool size (1-200) |
| `CONSOLE_AUDIT_DB_MAX_CONNECTIONS` | `4` | Audit pool size (1-100) |
| `CONSOLE_SESSION_IDLE_SECS` | `900` | Idle timeout (60-3600, less than absolute) |
| `CONSOLE_SESSION_ABSOLUTE_SECS` | `43200` | Absolute lifetime (900-86400) |
| `CONSOLE_SWEEP_INTERVAL_SECS` | `60` | Expired-session sweeper interval (5-3600) |
| `CONSOLE_TRUSTED_PROXIES` | empty | Comma-separated CIDRs whose `X-Forwarded-For` is trusted (walked right to left) |
| `CONSOLE_REQUIRED_ROLE` | `console-user` | Realm role required for a session |
| `CONSOLE_REQUIRED_ACR` | required unless dev mode | Comma-separated accepted `acr` values; sent as `acr_values` |
| `CONSOLE_DEV_MODE` | `false` | `true`/`1` relaxes two checks: `CONSOLE_REQUIRED_ACR` may be unset, and non-loopback DB hosts may omit `sslmode=verify-full`/`verify-ca`. Logs a warning. Never in production |
| `RUST_LOG` | `info` | Log filter. Logs are JSON; audit lines use `target=audit` |

DB URLs with a non-loopback host must use `sslmode=verify-full` or
`verify-ca` unless dev mode is on. A DB URL given directly (not `_FILE`)
logs a warning.

### Database

Migrations in `crates/console-gateway/migrations/` (sqlx, runtime-checked
queries, advisory lock): `0001_sessions.sql` (`sessions`,
`auth_requests`), `0002_audit.sql` (`audit_events`, append-only triggers),
`0003_roles_refresh.sql` (ownership, grants, `refresh_token`, reauth
columns).

| Role | Login | Rights |
| --- | --- | --- |
| `console_owner` | NOLOGIN | Owns every table, function and `_sqlx_migrations` (and should own the database and schema `public`) |
| `console_app` | NOLOGIN group | SELECT, INSERT, UPDATE, DELETE on `sessions` and `auth_requests`; nothing on `audit_events` |
| `console_audit` | LOGIN | INSERT, SELECT on `audit_events` only |
| `console_migrator` | LOGIN | Member of `console_owner`; used only by `migrate` (`SET ROLE console_owner` per connection) |
| `console` | LOGIN, NOCREATEDB, NOCREATEROLE | Member of `console_app`; `CONSOLE_DATABASE_URL` |

Migrations fail if `console_owner`, `console_app` or `console_audit` do not
exist. Provisioning SQL: `deploy/compose/postgres/init/01-roles.sh`;
post-migration check: `deploy/compose/postgres/check-roles.sql`.

## Local stack (`deploy/compose/`, DEV ONLY)

`make up` starts `postgres`, `keycloak` and `tls-proxy` with
`podman compose` (override with `COMPOSE="docker compose"`). The gateway
runs on the host (`make run`, or Playwright's `webServer`). A `gateway`
service exists under the compose profile `full`.

| Service | Image (digest-pinned) | Host binding |
| --- | --- | --- |
| `postgres` | `postgres:17-alpine` | `127.0.0.1:5432` (`CONSOLE_PG_PORT`). Runs `postgres/init/01-roles.sh` (roles and the `console` DB) |
| `keycloak` | `keycloak:26.8.1`, `start-dev --import-realm` | `http://127.0.0.1:8180` (an IP literal, so browsers never apply the gateway's HSTS to it). Imports `keycloak/realm-dev.json`: realm `icam` (DEV ONLY) |
| `tls-proxy` | `caddy:2-alpine`, `tls internal` | `https://localhost:<CONSOLE_TLS_PORT_*>` to host gateways on `127.0.0.1:<CONSOLE_GW_PORT_*>` |

Browsers reach gateways only through Caddy TLS because WebKit drops
`__Host-`/`Secure` cookies on `http://localhost`. Keycloak stays on plain
http so the issuer matches.

Ports have one source, `deploy/compose/ports.env`, read by the Makefile
(environment wins), compose (`--env-file`), the Caddyfile, the realm
import (redirect URI placeholders) and `e2e/playwright.config.ts`:

| TLS (browser) | Gateway (host) | Use |
| --- | --- | --- |
| 9442 `CONSOLE_TLS_PORT_RUN` | 8080 `CONSOLE_GW_PORT_RUN` | `make run` |
| 9443 `CONSOLE_TLS_PORT_MAIN` | 8081 `CONSOLE_GW_PORT_MAIN` | Main E2E (`dist-gallery`) |
| 9444 `CONSOLE_TLS_PORT_DIST` | 8082 `CONSOLE_GW_PORT_DIST` | Release `dist` smoke |
| 9445 `CONSOLE_TLS_PORT_IDLE` | 8083 `CONSOLE_GW_PORT_IDLE` | Idle E2E (idle 150 s) |
| 9446 `CONSOLE_TLS_PORT_ACR` | 8084 `CONSOLE_GW_PORT_ACR` | Gateway requiring an `acr` the dev realm never issues |
| (none, direct http) | 8085 `CONSOLE_GW_PORT_RL` | Rate-limit spec gateway |

`make ports-free` (run by `make up`) fails fast if a TLS port is busy.
Dev realm users (password `dev-only-<username>`): `console_user`,
`console_admin`, `no_role`, plus per-browser `revoke_*` and `derole_*`
users for the revocation specs. Gateway dev config:
`deploy/compose/gateway.env.example`. Linux/CI: add
`-f deploy/compose/compose.hostnet.yaml` (Caddy on the host network).
Setup and troubleshooting: [DEPLOYMENT.md](DEPLOYMENT.md#local-development-with-podman).

## Builds

| Output | Command | Gallery |
| --- | --- | --- |
| `crates/console-shell/dist` | `make web` (`trunk build --release`) | No (release) |
| `crates/console-shell/dist-gallery` | `make web-gallery` | Yes (development only) |
| Container image | `make image` (`deploy/Containerfile`) | No |

The container is a two-stage build. Both base images are pinned by
`@sha256` digest (`rust:1.99-bookworm`, distroless `cc-debian12:nonroot`).
The final stage contains the gateway binary and the release `dist` copied to
`/srv/console`.

Bundle size (release `dist` wasm, `gzip -9`): 173,207 B at M3 (+22.0% over
the M2 baseline of 141,918 B, mostly `Modal`, `Button`, `Alert` and the
session code entering the release bundle). 173,207 B is the new baseline
(`WASM_GZ_BASELINE` in `.github/workflows/ci.yml`). The CI `web` job fails
above 1.5 MB (1,572,864 B) and warns above baseline x1.10 (190,527 B).

## E2E tests (`e2e/`)

Playwright (exact pins in `e2e/package-lock.json`) with `@axe-core/playwright`,
through the compose Keycloak and Caddy TLS (`make up` first;
`ignoreHTTPSErrors` for Caddy's internal CA). `e2e/playwright.config.ts`
starts five gateways from `target/release/console-gateway` with
`gateway.env.example` values (ports above).

| Project | Target | Specs |
| --- | --- | --- |
| `setup` | main | `auth.setup.ts`: signs in `console_user` and saves the storage state |
| `chromium`, `firefox`, `webkit` | main (signed in) | all specs except `dist-smoke` and `idle`: `a11y`, `csp`, `keyboard`, `reflow`, `scope`, `auth`, `session-security`, `session-lifecycle`, `revocation`, `assurance` (uses the ACR and RL gateways) |
| `idle-chromium`, `idle-firefox`, `idle-webkit` | idle gateway (fresh sign-in) | `idle.spec.ts` |
| `dist-chromium`, `dist-firefox`, `dist-webkit` | release `dist` (signed in) | `dist-smoke.spec.ts` |

Helpers: `auth-helpers.ts` (Keycloak login), `db-helpers.ts` (audit
queries and forced expiry via the container engine's psql), `kc-admin.ts`
(disable user, drop role). axe runs on Chromium only.

Latest local run (fix round 2): `setup`, Chromium and WebKit projects, 184
passed, 0 failed, 13 skipped by design, 30.4 s. Firefox projects have not
run, and the CI `e2e` job has not run the M3 suite yet.

## Compliance

See [compliance/NIST_800-53_MAPPING.md](compliance/NIST_800-53_MAPPING.md),
[compliance/SESSION_MANAGEMENT.md](compliance/SESSION_MANAGEMENT.md),
[compliance/AUDIT_LOGGING.md](compliance/AUDIT_LOGGING.md)
and [compliance/POAM.md](compliance/POAM.md). Deployment requirements:
[DEPLOYMENT.md](DEPLOYMENT.md).
