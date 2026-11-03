# Architecture (current state, M2)

This describes what is built today. The target design and the milestones are
in [PLAN.md](PLAN.md). Sign-in, the tool registry, the reverse proxy, the
iframe tool container and audit logging are **not built yet**.

## Crates

| Crate | What it does today |
| --- | --- |
| `console-protocol` | Serde types for the tool manifest and shell/tool SDK messages. |
| `console-sdk` | Tool-side encode/decode with origin checks. No transport yet (M4). |
| `console-ui` | Yew 0.23 component library, design tokens (`tokens.css`) and component styles (`components.css`). See [crates/console-ui/README.md](../crates/console-ui/README.md). |
| `console-shell` | Yew CSR app built with Trunk: skip link, top navigation (service menu, global search, user menu, theme toggle), scope picker, page header and breadcrumbs, flashbar, collapsible side navigation, router, and an optional component gallery. |
| `console-gateway` | axum server: serves the shell's static `dist` with SPA fallback, `/healthz`, and security headers. |

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
- `UserMenu` shows "Demo User (demo)". "Profile (not available yet)" and
  "Sign out" only show info flashes ("Sign-in arrives in M3").
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

## console-gateway

Configuration (environment variables):

| Variable | Default | Meaning |
| --- | --- | --- |
| `CONSOLE_LISTEN` | `0.0.0.0:8080` | Listen address (plain HTTP) |
| `CONSOLE_DIST` | `crates/console-shell/dist` | Directory served as the SPA |
| `RUST_LOG` | `info` | Log filter. Logs are JSON. |

Every response gets these headers:

- `Content-Security-Policy`: `default-src 'self'`, `script-src 'self'
  'wasm-unsafe-eval'` plus a SHA-256 hash of each inline script in
  `index.html` (computed at startup), `style-src 'self'`, `frame-src 'self'`,
  `frame-ancestors 'none'`, `base-uri 'none'`, `form-action 'self'`
- `X-Content-Type-Options: nosniff`
- `Referrer-Policy: no-referrer`
- `Strict-Transport-Security: max-age=31536000; includeSubDomains`

The gateway does not terminate TLS.

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

Bundle size (release `dist` wasm, `gzip -9`): baseline 141,918 B (M2),
current 144,520 B. The CI `web` job fails above 1.5 MB (1,572,864 B) and
warns above baseline x1.10 (156,109 B).

## E2E tests (`e2e/`)

Playwright (exact pins in `e2e/package-lock.json`) with `@axe-core/playwright`.
`e2e/playwright.config.ts` starts two gateways with the release
`target/release/console-gateway` binary:

| Port | Bundle | Projects | Specs |
| --- | --- | --- | --- |
| 127.0.0.1:8081 (`E2E_PORT`) | `dist-gallery` | `chromium`, `firefox`, `webkit` | `a11y`, `csp`, `keyboard`, `reflow`, `scope` |
| 127.0.0.1:8082 (`E2E_DIST_PORT`) | release `dist` | `dist-chromium`, `dist-firefox`, `dist-webkit` | `dist-smoke` only |

- `a11y.spec.ts`: axe on `/` and `/gallery`, light and dark, 1280/600/320 px;
  fails on serious/critical. Chromium only.
- `csp.spec.ts`, `dist-smoke.spec.ts`: no CSP violations (WebKit `<select>`
  allowance in `csp-helpers.ts`), no non-empty `[style]`; `/gallery` is
  NotFound in `dist`.
- `keyboard.spec.ts`, `reflow.spec.ts`, `scope.spec.ts`: keyboard, 320 px
  reflow, scope URL/cascade/history.

Run with `make e2e-install` once, then `make e2e` (builds `web`,
`web-gallery` and `gateway` first). `make e2e` is not part of `make check`.
Port 8080 stays reserved for `make run`.

## Compliance

See [compliance/NIST_800-53_MAPPING.md](compliance/NIST_800-53_MAPPING.md)
and [compliance/POAM.md](compliance/POAM.md).
