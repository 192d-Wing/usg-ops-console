# Architecture (current state, M1)

This describes what is built today. The target design and the milestones are
in [PLAN.md](PLAN.md). Sign-in, the tool registry, the reverse proxy, the
iframe tool container and audit logging are **not built yet**.

## Crates

| Crate | What it does today |
| --- | --- |
| `console-protocol` | Serde types for the tool manifest and shell/tool SDK messages. |
| `console-sdk` | Tool-side encode/decode with origin checks. No transport yet (M4). |
| `console-ui` | Yew 0.23 component library, design tokens (`tokens.css`) and component styles (`components.css`). See [crates/console-ui/README.md](../crates/console-ui/README.md). |
| `console-shell` | Yew CSR app built with Trunk: skip link, top bar, theme toggle, flashbar, side navigation, router, and an optional component gallery. |
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

The container is a two-stage build. The final stage is distroless
`cc-debian12:nonroot` with the gateway binary and the release `dist` copied to
`/srv/console`.

## Compliance

See [compliance/NIST_800-53_MAPPING.md](compliance/NIST_800-53_MAPPING.md)
and [compliance/POAM.md](compliance/POAM.md).
