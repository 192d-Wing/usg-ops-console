# Architecture (current state, M4.5)

This describes what is built today. The target design and the milestones are
in [PLAN.md](PLAN.md). M3 added Keycloak OIDC sign-in, server-side
sessions, logout, idle and absolute timeouts, and the audit log. M4 added
tool hosting: the gateway tool registry, per-tool origins with a
launch-code handoff, the authenticated reverse proxy, the signed identity
assertion with a JWKS, the shell tool container, SDK protocol v1 with the
`console-sdk` transport, and `examples/hello-tool`. The tool contract is in
[INTEGRATING-A-TOOL.md](INTEGRATING-A-TOOL.md). M4 is green in
CI run [41892456284](https://github.com/192d-Wing/usg-ops-console/actions/runs/41892456284).

M4.5 (redesign, 2026-11-04) replaced the visual design and the shell layout:
a "calm neutral" token set, a collapsible icon rail, a Ctrl/Cmd+K command
palette and a breadcrumb scope switcher. Routes, contracts, the CSP and SDK
protocol v1 are unchanged. **M4.5 has local evidence only; CI has not run
this change yet.**

## Crates

| Crate | What it does today |
| --- | --- |
| `console-protocol` | Serde types for the tool manifest, SDK v1 messages (`message`), the gateway session HTTP shapes (`session`), and (M4) the tool-hosting contract (`tools`: `/api/tools`, launch, `/_console/context`, assertion claims, header and route names). Frozen fixtures in `tests/fixtures/sdk_v1/` and `tools_v1/`. |
| `console-sdk` | Tool-side bridge: `codec` (exact-origin encode/decode, native) and, with feature `web`, the browser `postMessage` transport and Yew hooks (M4). |
| `console-ui` | Yew 0.23 component library, design tokens (`tokens.css`) and component styles (`components.css`). See [crates/console-ui/README.md](../crates/console-ui/README.md). |
| `console-shell` | Yew CSR app built with Trunk: skip link, top bar (nav toggle, product mark, palette trigger, breadcrumb scope switcher, theme toggle, user menu), command palette, collapsible icon rail, page header and breadcrumbs, flashbar, router, and an optional component gallery. |
| `console-gateway` | axum server: OIDC sign-in, server-side sessions (Postgres), logout, audit log, security headers, the shell's static `dist` behind the session check, and (M4) the tool registry, tool origins, reverse proxy and identity assertion. |
| `examples/hello-tool/*` | `hello-tool-server` (axum: assertion verifier, API, serves the web build) and `hello-tool-web` (Yew tool using the SDK, plus the hostile `evil` fixture). Template and E2E target. |

## console-shell

- Routing: `yew-router` `BrowserRouter` with `Home` (`/`), `NotFound`,
  (M4) `/tools/:id` and `/tools/:id/*path` (tool container, deep links),
  and `Gallery` (`/gallery`). `Gallery` exists only when the `gallery` Cargo
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

### Design system (`console-ui`, M4.5)

- "Calm neutral" look: neutral grey surfaces, one accent colour (pine teal,
  `#0e7268` light / `#3fb8a9` dark), 1px borders, soft radii, flat
  containers. Light and dark are both full themes. Token names keep the
  `--cui-` prefix; M4.5 only added tokens (none removed or renamed). The
  top bar is a light surface with a hairline border; there is no inverse
  (navy) chrome.
- Cloudscape guard: test `no_cloudscape_values`
  (`crates/console-ui/tests/contrast.rs`) fails if `tokens.css` or
  `components.css` contains any hex from a fixed list of Cloudscape blue
  and navy values. `token_pairs_meet_wcag_aa` checks the declared
  contrast pairs in both themes; `every_dark_color_has_a_light_counterpart`
  checks theme parity.
- The gateway's `/auth.css` (`AUTH_CSS` in
  `crates/console-gateway/src/routes.rs`) copies token hexes; test
  `auth_css_hexes_are_tokens` (`crates/console-ui/tests/guards.rs`) checks
  that every hex in it exists in `tokens.css`.
- Details: [crates/console-ui/README.md](../crates/console-ui/README.md)
  and `crates/console-ui/CHANGELOG.md`.

### Chrome (`src/chrome.rs`, `src/main.rs`, `src/rail.rs`, `src/palette.rs`, `src/demo.rs`)

- Top bar (`TopNavigation`), in DOM order: nav toggle, product mark and
  name, palette trigger ("Search…" with a ⌘K or Ctrl K hint), breadcrumb
  scope switcher, theme toggle, user menu. The M4 service menu and global
  search box are removed. The SDK protocol version text moved from the old
  scope bar into the user menu.
- `UserMenu` shows the signed-in user's `display_name` from `/api/session`
  ("Signed in as `<username>`") and a working Sign out (POST `/logout` with
  the CSRF token). While loading it shows a skeleton; if `/api/session`
  fails it shows "Session unavailable" with Retry and Sign in. A
  `role=status` region announces "Signing out…".

#### Icon rail

- `Rail` (`console-ui`) in `#side-nav-region`: Home, Gallery (gallery
  builds only), and the permitted tools from `GET /api/tools` (filtered by
  role in the gateway) with the active tool's sub-pages. Tools without an
  icon show a two-letter monogram.
- At `(min-width: 45rem)` (`NAV_BREAKPOINT`) the nav toggle switches the
  rail between expanded (icons and labels) and collapsed (icons only).
  Collapsed links keep the text label as their accessible name (visually
  hidden) and show an `aria-hidden` tooltip on hover and focus. Escape
  hides the tooltip without moving focus, and the pointer can move onto it
  (WCAG 1.4.13). `#side-nav-region` carries `data-rail="expanded|collapsed"`.
- Persistence: the wide-screen mode is stored per browser in
  `localStorage["usg-console.rail"]` (`RAIL_STORAGE_KEY`). The stored value
  goes through an allow-list (`rail::parse_stored`, `RailMode::parse`;
  SI-10); anything else falls back to the default, expanded. Storage errors
  are ignored.
- Below 45rem the rail is a drawer behind the nav toggle, as in M2: hidden
  by default, focus moves to the first link on open, Escape and a scrim
  click close it and return focus to the toggle. The stored collapsed mode
  is ignored there.
- Toggling the rail never remounts the tool container (the `ToolHost` vnode
  keeps its key).

#### Command palette

- `CommandPalette` (`console-ui`) is a modal dialog with an APG combobox
  (`palette-input`) and listbox (`palette-list`), arrow, Home/End and Enter
  keys, a polite result count (debounced 300 ms), a Tab trap, `inert` on
  `#app-chrome`, and focus return to the opener (or to the palette trigger,
  then `#main-content`).
- Opening: the visible trigger, or Ctrl+K / Cmd+K (no Alt or Shift) through
  a listener on the shell document (`use_palette_shortcut` in
  `src/main.rs`). The same combo closes it. The shortcut is ignored on key
  repeat, when already handled, while the idle dialog is open, and while
  `#app-chrome` is inert (for example under a modal). It does nothing while
  focus is inside a tool frame, because key events in the frame do not
  reach the shell document; the SDK protocol is unchanged.
- Sources (`src/palette.rs`), built only while the palette is open:
  Navigation (Home, Gallery in gallery builds), Tools (the permitted tools
  from `/api/tools`, a disabled "Loading" row or an enabled Retry row),
  Scope (site; tenant only once a site is set; env only once a tenant is
  set), Preferences (light, dark, system; the current one is hinted), and
  Account (Sign out, once the session has loaded).
- Selecting an item closes the palette, then resolves its id through a
  closed `Action` enum. Navigation uses the same router path as the rail;
  a tool id resolves only by lookup in the loaded tool list (never from the
  query text); theme uses the same code as the theme toggle; Sign out uses
  the same CSRF POST as the user menu. An id that no longer resolves shows a
  Warning flash (`resolve_palette_select`, test
  `palette_select_unresolved_closes_and_warns`).
- Ranking (`logic::rank_palette`): exact, prefix, word prefix, substring,
  keyword, subsequence; ties by group, then alphabetically; at most 50
  results.
- Privacy: the query is component state only. It is capped at 256
  characters (`PALETTE_MAX_QUERY`), rendered as text, and never stored,
  sent, or put in the URL or history (E2E "palette query is never echoed
  outside the palette, URL, storage or network", `keyboard.spec.ts`).
  Recent picks are not stored.

#### Breadcrumb scope switcher

- `ScopePicker` renders the scope as breadcrumb segments (site › tenant ›
  env) in a `<nav aria-label="Scope">`. At 45rem and wider, each segment
  opens its own `Popover` (non-modal dialog) holding that level's native
  `<select>`. Below 45rem a single "Scope" button opens one popover with
  all three selects. One DOM tree serves both widths, so the selects are not
  re-created on a width change.
- The cascade, URL and history rules and validation are unchanged from M2
  (see "Global scope" below). The `scope-announcer` live region sits
  outside the popovers. A popover stays open after a change; it closes on
  Escape (focus returns to the segment), an outside pointerdown, or focus
  leaving it.

#### Layering

- z-order (tokens): rail drawer overlay 950, palette 1050, flash just
  below dropdowns, idle dialog and skip link `--cui-z-critical` 1200. When
  the idle warning opens, the shell closes the palette and keeps
  `#app-chrome` inert until the dialog is answered (E2E `idle.spec.ts`
  "M4.5: idle dialog opens above the palette…" and "SEC-002: idle over
  palette…").
- Page header (H1) and breadcrumbs come from `PageMeta`.

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
- Following an in-app link (rail, breadcrumbs, command palette) appends the
  current scope keys to the target path.
- History is written through yew-router's shared `BrowserHistory`. Every URL
  passes through `same_origin_url`, which collapses leading `/` or `\` runs to
  one `/`.
- Scope is never stored in `localStorage`.

Scope is a UI filter, not an authorization claim (PL-8). Servers must not
trust it. M4 passes it to tools as the assertion `ctx` claim (a
selection, not authorization); tools must authorize it.

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

### Tool container (M4: `src/tools.rs`, `src/tool_host.rs`, `src/tool_wire.rs`, `src/activity.rs`)

- `tools.rs` fetches `/api/tools` once the session is ready and provides
  `ToolsCtx`. `accept_tool` re-checks each entry defensively (valid id,
  exact `https://` origin that is not the console origin, cleaned titles,
  safe nav paths).
- `ToolHost` (keyed by tool id): `POST /api/tools/{id}/launch {path}`,
  accept the URL only as `<registry origin>/_console/launch?...`, set the
  iframe `src`. Sandbox `allow-scripts allow-same-origin allow-forms
  allow-downloads allow-popups allow-modals`, `allow=""`,
  `referrerpolicy="no-referrer"`, `title` = tool title. Scope and theme
  changes are sent as messages, never by reloading the frame. 404 (unknown
  or not permitted) shows "Tool not found"; other errors show an alert
  with "Reload tool". No `Ready` within 10 s shows a non-blocking warning.
- Inbound messages are accepted only if `origin` equals the registry
  origin and `source` is the iframe's window, then size-capped and
  decoded by `tool_wire` (a `JSON`/`Reflect` codec that mirrors the serde
  codec, tested against every frozen fixture, so `serde_json` stays out of
  the bundle). Outbound messages target the registry origin.
- `activity.rs` gates tool `RequestSessionRefresh` on trusted user
  presence (60 s window) or an open idle dialog (see
  [SESSION_MANAGEMENT.md](compliance/SESSION_MANAGEMENT.md#tool-refresh-gating)).
- Keyboard: "Skip to tool content", F6/Ctrl+F6/Shift+F6, focus edges after
  native Tab into the frame, a state-driven focus ring for the Firefox
  reverse-Tab stop ([compliance/ACCESSIBILITY.md](compliance/ACCESSIBILITY.md)).

### console-sdk (feature `web`)

`ConsoleBridge::connect()` fetches `/_console/context` to pin the console
origin, sends `Ready` (retries at 250 ms, 500 ms, 1 s; `Standalone` after
2 s or when top-level), accepts only messages from the pinned origin whose
`source` is `window.parent`, handles F6 keys and `Focus` edges, and offers
`console_fetch` (adds `X-Console-Scope`) and Yew hooks. Details:
[INTEGRATING-A-TOOL.md](INTEGRATING-A-TOOL.md#sdk-protocol-v1-postmessage).

## console-gateway

The gateway is the authenticated front door: Keycloak OIDC sign-in,
server-side sessions in Postgres, logout, idle and absolute timeouts, and
the audit log. Modules (`crates/console-gateway/src/`): `config` (env,
fail closed), `oidc` (discovery, authorize URL, code exchange, id_token
verification, refresh grant, end-session URL), `session` (ids, cookies,
expiry math, `SessionStore`), `routes` (handlers and `require_session`),
`csrf`, `return_to`, `client_ip`, `headers`, `rate_limit`, `audit`,
`sweeper`; M4 adds `registry` (TOML load and validation, `ToolsState`),
`tool_origin` (Host dispatch, launch exchange, `/_console/context`, proxy
handler, tool-origin hardening headers), `proxy` (hyper client per tool,
header allow-list, cookie and `Location` rules, streaming bodies),
`assertion` (ES256 signing with `ring`, JWKS, `sid` HMAC), `tool_session`
(launch codes, tool sessions), `tools_api` (`/api/tools`, launch) and
`jwks`. Session and cookie rules: [compliance/SESSION_MANAGEMENT.md](compliance/SESSION_MANAGEMENT.md).
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

### Tool hosting (M4)

```text
                      +-------------------- console-gateway ---------------------+
 console.<domain> --> | console app: /api/*, /login, SPA, /.well-known/jwks.json  |
                      |   /t/* and /_console/* -> 404                            |
 <id>.tools.<domain>->| tool origin router (Host match against the registry):    |
                      |   /_console/launch?code=  launch-code exchange           |
                      |   /_console/context       SDK origin pin                 |
                      |   /t/<id>/*  tool session -> console session check ->    |
                      |              role check -> header rebuild -> sign -> proxy --> tool upstream
                      +----------------------------------------------------------+
```

- **Registry**: `CONSOLE_TOOLS_REGISTRY_FILE`, validated at startup, fail
  closed ([DEPLOYMENT.md](DEPLOYMENT.md#tool-registry)).
- **Tool origins**: each tool has its own origin; the outer `dispatch`
  layer routes on `Host`. An unknown `Host` is the console app; another
  tool's path on a tool origin is 421.
- **Launch handoff**: the shell calls `POST /api/tools/{id}/launch`
  (session, CSRF, Origin, 60/min); the gateway stores a single-use 30 s
  code and returns the tool-origin launch URL. The tool origin checks the
  launch context (iframe Fetch Metadata or console `Referer`), takes the
  code, creates a tool session (at most 8 per console session and tool),
  sets `__Host-tool_sid` and redirects 303 to `/t/<id><path>`.
- **Proxy**: per-tool pooled hyper client over hyper-rustls (`ring`),
  http/1.1 and h2; connect 3 s, per-tool header timeout, 60 s body idle;
  streaming both ways with a body cap; per-tool concurrency semaphore;
  allow-list request headers; `Set-Cookie`, hop-by-hop, HSTS and `server`
  filtering; `Location` restricted; gateway CSP `frame-ancestors <console>`
  added; WebSocket upgrades get 501.
- **Assertion and JWKS**: a fresh ES256 JWS per request in
  `X-Console-Assertion` (60 s, unique `jti`, per-tool `sid`), signed after
  authorization; not cached (sign p50 is about 23 µs in the bench). Public
  keys at `/.well-known/jwks.json`.
- **Tool sessions**: one indexed join per proxied request; console idle
  and absolute deadlines and roles rechecked, never extended; no session
  cache (M3 Q7 decision kept). Rows cascade with the console session.
- **Audit queue**: proxy-path audit rows go through a bounded background
  queue (1024, batches of 64); drops are counted
  ([AUDIT_LOGGING.md](compliance/AUDIT_LOGGING.md)).

Proxy overhead (QA final round, loopback, n=5000, p50 / p99 ms; gateway
minus direct hello). **Caveat:** gateway, hello, load generator and the
podman VM shared one host (load average 27.4 on 12 cores), which inflates
the c>=32 tails; re-measure on separate hosts before acting (POAM-043).

| Case | Direct | Via gateway | Overhead |
| --- | --- | --- | --- |
| ping c=1 | 0.52 / 1.11 | 1.04 / 1.64 | 0.52 / 0.53 |
| ping c=32 | 2.04 / 12.20 | 4.03 / 18.45 | 1.99 / 6.26 |
| ping c=128 | 9.98 / 30.45 | 18.88 / 46.88 | 8.90 / 16.43 |
| navigation c=1 | 0.56 / 1.45 | 1.07 / 1.81 | 0.51 / 0.35 |
| navigation c=32 | 2.04 / 5.79 | 4.40 / 10.81 | 2.36 / 5.02 |
| POST (`audit_writes`) c=1 | 0.53 / 0.64 | 1.01 / 1.36 | 0.48 / 0.72 |
| POST c=32 | 1.83 / 4.06 | 4.11 / 9.08 | 2.28 / 5.02 |

Target: overhead p50 <= 2 ms and p99 <= 5 ms. Met at c=1; at c=32 missed
for ping p99 (6.26 ms) and borderline for navigation and POST (5.02 ms);
c=128 is pool-bound by design (pool 32). Tail overhead p99 is computed as
the difference of two p99 values, so it can be smaller than p50.
`assertion.sign` p99 stayed at or below 174 µs at c<=32.

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
| `GET /api/tools` (M4) | 200 `ToolsResponse` filtered by role, `no-store` | No upstreams in the response |
| `POST /api/tools/{id}/launch` (M4) | 200 `LaunchResponse {url}` | CSRF + Origin; 404 for unknown or not-permitted ids; 400 `bad_request`; 429 over 60/min |
| `GET /.well-known/jwks.json` (M4) | 200 JWKS, `public, max-age=300` | Public |
| `/t/*`, `/_console/*` on the console origin (M4) | 404 | Never proxied |
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
  `index.html` (computed at startup), `style-src 'self'`, `frame-src`
  = the registry tool origins (`'none'` with no tools; M4),
  `frame-ancestors 'none'`, `base-uri 'none'`, `form-action 'self'
  <issuer origin>`
- `X-Content-Type-Options: nosniff`
- `Referrer-Policy: no-referrer`
- `Strict-Transport-Security: max-age=31536000; includeSubDomains`

Tool-origin responses and errors are listed in
[INTEGRATING-A-TOOL.md](INTEGRATING-A-TOOL.md#errors) (400, 401, 403,
404, 413, 421, 429, 501, 502, 503, 504).

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
| `CONSOLE_DB_MAX_CONNECTIONS` | `32` (M4; was 10) | Main pool size (1-200). No ping before acquire; idle 300 s, lifetime 30 min |
| `CONSOLE_AUDIT_DB_MAX_CONNECTIONS` | `4` | Audit pool size (1-100) |
| `CONSOLE_SESSION_IDLE_SECS` | `900` | Idle timeout (60-3600, less than absolute) |
| `CONSOLE_SESSION_ABSOLUTE_SECS` | `43200` | Absolute lifetime (900-86400) |
| `CONSOLE_SWEEP_INTERVAL_SECS` | `60` | Expired-session sweeper interval (5-3600) |
| `CONSOLE_TRUSTED_PROXIES` | empty | Comma-separated CIDRs whose `X-Forwarded-For` is trusted (walked right to left) |
| `CONSOLE_REQUIRED_ROLE` | `console-user` | Realm role required for a session |
| `CONSOLE_REQUIRED_ACR` | required unless dev mode | Comma-separated accepted `acr` values; sent as `acr_values` |
| `CONSOLE_DEV_MODE` | `false` | `true`/`1` relaxes two checks: `CONSOLE_REQUIRED_ACR` may be unset, and non-loopback DB hosts may omit `sslmode=verify-full`/`verify-ca`. Logs a warning. Never in production |
| `CONSOLE_OIDC_REFRESH_TIMEOUT_SECS` | `4` | Keepalive refresh-grant total timeout (1-10); timeout gives 503 |
| `CONSOLE_TOOLS_REGISTRY_FILE` | unset | Tool registry TOML; unset = no tools |
| `CONSOLE_ASSERTION_KEY_FILE` | unset | P-256 PKCS#8 signing key; required with tools |
| `CONSOLE_ASSERTION_PUBLISH_KEYS_FILE` | unset | Extra public keys for the JWKS |
| `CONSOLE_TOOL_SESSION_HMAC_KEY_FILE` | unset | `sid` HMAC key (at least 32 bytes); required with tools outside dev mode |
| `RUST_LOG` | `info` | Log filter. Logs are JSON; audit lines use `target=audit` |

DB URLs with a non-loopback host must use `sslmode=verify-full` or
`verify-ca` unless dev mode is on. A DB URL given directly (not `_FILE`)
logs a warning.

### Database

Migrations in `crates/console-gateway/migrations/` (sqlx, runtime-checked
queries, advisory lock): `0001_sessions.sql` (`sessions`,
`auth_requests`), `0002_audit.sql` (`audit_events`, append-only triggers),
`0003_roles_refresh.sql` (ownership, grants, `refresh_token`, reauth
columns), `0004_session_idle.sql` (per-row idle limit, `acr`),
`0005_tool_sessions.sql` (`tool_launch_codes`, `tool_sessions`, audit
columns), `0006_tool_sessions_multi.sql` (up to 8 tool sessions per
console session and tool).

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
| (none, direct http) | 8086 `CONSOLE_GW_PORT_RL_UNTRUSTED` | Rate-limit gateway trusting no proxy |
| 9447 `CONSOLE_TLS_PORT_TOOL_HELLO` | main gateway | `hello` tool origin (E2E) |
| 9448 `CONSOLE_TLS_PORT_TOOL_EVIL` | main gateway | `evil` hostile tool origin (E2E) |
| 9449 `CONSOLE_TLS_PORT_TOOL_RUN` | run gateway | `hello` tool origin for `make run` |
| 9451 `CONSOLE_TLS_PORT_KA503` | 8088 `CONSOLE_GW_PORT_KA503` | Gateway whose IdP token endpoint is unreachable (keepalive 503), via forward proxy 8091 |
| 9452 `CONSOLE_TLS_PORT_PRODLIKE` | 8089 `CONSOLE_GW_PORT_PRODLIKE` | prodlike: `console.ops.test`, `hello.tools.ops.test`, `evil.tools.ops.test` (SNI) |
| 9453 `CONSOLE_TLS_PORT_TOOL_IDLE` | idle gateway | Tool origin on the idle gateway (refresh-gating E2E) |
| (none) | 8090, 8092, 8093 | hello-tool servers for main, prodlike and idle gateways |

Dev tool origins are port-distinct on `localhost`. Cookies ignore ports,
so the dev stack cannot show cookie-tossing resistance; the
`prodlike-chromium` project uses real host names for that.

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

Bundle size at M4.5: **240,530 B** gz (`gzip -9`, fixes round 3;
QA round 2 measured 241,796 B with `gzip -c`, before the round 3 fixes),
about 15.3% of the 1.5 MB budget and about +16.5% over M4 (206,548 B).
Accepted (PERF-001, M4.5: the rail, palette and popovers are the
milestone's features; breakdown in the run's `perf-wu2-size.txt`). The CI
warn baseline (`WASM_GZ_BASELINE` in `.github/workflows/ci.yml`) is moved
to 240,530 B (warn above 264,583 B). That change is not yet committed or
run in CI. Note that the CI step measures with `gzip -c`, which gives a
slightly larger number than the `gzip -9` baseline.

Bundle size at M4: **206,548 B** gz (`gzip -c`, QA final round), 13.1%
of the 1.5 MB budget, +18.6% over `main` (174,214 B). Accepted (PERF-001
M4: the tool host is the milestone's feature). The CI warn baseline stayed
at 204,977 B (warn above 225,474 B) for M4. hello-tool web wasm: about 150 KB gz
(target under 250 KB).

M3 history: bundle size (release `dist` wasm, `gzip -9`): 173,207 B at M3 (+22.0% over
the M2 baseline of 141,918 B, mostly `Modal`, `Button`, `Alert` and the
session code entering the release bundle). 173,207 B is the new baseline
(`WASM_GZ_BASELINE` in `.github/workflows/ci.yml`). The CI `web` job fails
above 1.5 MB (1,572,864 B) and warns above baseline x1.10 (190,527 B).

## E2E tests (`e2e/`)

Playwright (exact pins in `e2e/package-lock.json`) with `@axe-core/playwright`,
through the compose Keycloak and Caddy TLS (`make up` first;
`ignoreHTTPSErrors` for Caddy's internal CA). `e2e/playwright.config.ts`
starts the gateways from `target/release/console-gateway` with
`gateway.env.example` values (ports above), the hello-tool servers, and
the IdP blackhole proxy; it renders the tool registries into
`target/e2e-tools/`.

| Project | Target | Specs |
| --- | --- | --- |
| `setup` | main | `auth.setup.ts`: signs in `console_user` and saves the storage state |
| `chromium`, `firefox`, `webkit` | main (signed in) | all specs except `dist-smoke` and `idle`: `a11y`, `csp`, `keyboard`, `reflow`, `scope`, `auth`, `session-security`, `session-lifecycle`, `revocation`, `assurance` (uses the ACR and RL gateways) |
| `idle-chromium`, `idle-firefox`, `idle-webkit` | idle gateway (fresh sign-in) | `idle.spec.ts` |
| `dist-chromium`, `dist-firefox`, `dist-webkit` | release `dist` (signed in) | `dist-smoke.spec.ts` |
| M4 additions to the main projects | main + tool origins | `tools`, `tools-norole`, `tool-security`, `tool-isolation`, `tool-a11y`, `session-gaps`, opt-in `proxy-bench` (`E2E_BENCH=1`) |
| `idle-*` (M4 adds) | idle gateway + tool origin 9453 | `idle-tool.spec.ts` |
| `ka503-chromium`, `ka503-firefox`, `ka503-webkit` | KA503 gateway | `keepalive-503.spec.ts` |
| M4.5 additions to the main projects | main | `rail.spec.ts`, `palette.spec.ts`; `idle.spec.ts` gains idle-over-palette cases |
| `prodlike-chromium` | prodlike gateway (env `E2E_PRODLIKE=1`; `--host-resolver-rules` locally, `/etc/hosts` in CI) | `prodlike.spec.ts` |

Helpers: `auth-helpers.ts` (Keycloak login), `db-helpers.ts` (audit
queries and forced expiry via the container engine's psql), `kc-admin.ts`
(disable user, drop role). axe runs on Chromium only.

Latest local runs (M4 QA final round): macOS chromium/webkit projects plus
`prodlike-chromium`, 294 passed, 0 failed, 20 skipped; Linux container
(`make e2e-linux`, digest-pinned Playwright 1.64.1 image; chromium,
firefox, webkit; prodlike excluded), 428 passed, 37 skipped, 3 failed (Linux
WebKit Keycloak sign-in flake, POAM-044; rerun 43/43). The CI `e2e` job
passed in CI run 41892456284 (1 flaky, POAM-045).

M4.5 local runs (QA round 2, after fixes round 2): macOS chromium/webkit
projects 373 passed, 4 failed; Linux container (chromium, firefox, webkit)
543 passed, 6 failed. All 10 failures are the two rail tooltip Escape
tests (R2-D1, WCAG 1.4.13). Fixes round 3 changed the CSS and added a wasm
test (`rail_collapsed_keeps_text_names`) that checks computed visibility,
but the E2E suite has **not** been re-run after round 3, and CI has not
run M4.5.

## Compliance

See [compliance/NIST_800-53_MAPPING.md](compliance/NIST_800-53_MAPPING.md),
[compliance/SESSION_MANAGEMENT.md](compliance/SESSION_MANAGEMENT.md),
[compliance/AUDIT_LOGGING.md](compliance/AUDIT_LOGGING.md)
[compliance/POAM.md](compliance/POAM.md) and
[compliance/ACCESSIBILITY.md](compliance/ACCESSIBILITY.md). Deployment requirements:
[DEPLOYMENT.md](DEPLOYMENT.md).
