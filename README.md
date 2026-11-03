# usg-ops-console

Pure Rust (Yew/WASM + axum) operations console: shared chrome, sign-in, and a
tool container that independently deployed USG tools plug into. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for what is built today and
[docs/PLAN.md](docs/PLAN.md) for the target design and milestones. Compliance
evidence is in [docs/compliance/](docs/compliance/).

## Layout

| Crate | Role |
| --- | --- |
| `console-protocol` | Tool manifest + shell ⇄ tool SDK message types (compat contract) |
| `console-ui` | Yew component library + design tokens (`assets/tokens.css`, `assets/components.css`); see [its README](crates/console-ui/README.md) |
| `console-sdk` | Tool-side bridge (origin-checked encode/decode; transport in M4) |
| `console-shell` | The Yew app, built with Trunk |
| `console-gateway` | axum server: static shell, security headers (OIDC/registry/proxy in M3–M4) |

## Develop

Requires stable Rust with `wasm32-unknown-unknown`, `trunk`, and `cargo-deny`.
The E2E tests also need Node 24 or later; `make wasm-test` needs `wasm-pack`.

```bash
make check
```

`make check` runs fmt, lint (host and wasm32 clippy), test, web, web-gallery,
check-no-gallery and deny.

| Target | What it does |
| --- | --- |
| `make web` | Release shell bundle in `crates/console-shell/dist` (no gallery) |
| `make web-gallery` | Shell with the component gallery in `crates/console-shell/dist-gallery` (development only) |
| `make check-no-gallery` | Rebuilds `dist` and fails if it has no wasm or the wasm contains gallery code (CM-7) |
| `make run` | Builds `web-gallery` and serves it at <http://127.0.0.1:8080> (gallery at `/gallery`) |
| `make wasm-test` | console-ui browser tests with wasm-pack (headless Firefox by default; `WASM_BROWSER=chrome` to override). Not part of `make check`. CI job `wasm-test` exists but has not had a green run yet |
| `make e2e-install` | `npm ci` in `e2e/` and installs Playwright browsers (chromium, firefox, webkit) |
| `make e2e` | Builds `web`, `web-gallery` and `gateway`, then runs Playwright + axe. Starts gateways on 127.0.0.1:8081 (`dist-gallery`) and 127.0.0.1:8082 (release `dist`). Pass options with `E2E_ARGS`, for example `E2E_ARGS="--project=chromium"`. Not part of `make check` |
| `make e2e-typecheck` | `npm ci` and `tsc --noEmit` in `e2e/` |

## Workflow

Feature work goes through the multi-agent workflow (`/console-workflow`), ported
from usg-dcim; see `.claude/skills/console-workflow/SKILL.md`.
