# usg-ops-console

Pure Rust (Yew/WASM + axum) operations console: shared chrome, sign-in, and a
tool container that independently deployed USG tools plug into. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for what is built today and
[docs/PLAN.md](docs/PLAN.md) for the target design and milestones. Compliance
evidence is in [docs/compliance/](docs/compliance/). Deployment and local
stack setup: [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md). Building a tool that
plugs into the console: [docs/INTEGRATING-A-TOOL.md](docs/INTEGRATING-A-TOOL.md)
(reference tool: [examples/hello-tool](examples/hello-tool/README.md)).

## Layout

| Crate | Role |
| --- | --- |
| `console-protocol` | Tool manifest + shell ⇄ tool SDK message types, and the gateway session HTTP types (compat contract) |
| `console-ui` | Yew component library + design tokens (`assets/tokens.css`, `assets/components.css`); see [its README](crates/console-ui/README.md) |
| `console-sdk` | Tool-side bridge: origin-checked codec; with feature `web`, the postMessage transport and Yew hooks |
| `console-shell` | The Yew app, built with Trunk |
| `console-gateway` | axum server: Keycloak OIDC sign-in, Postgres sessions, logout, audit log, security headers, static shell, tool registry, per-tool origins, reverse proxy and signed identity assertion |

## Develop

Requires stable Rust with `wasm32-unknown-unknown`, `trunk`, and `cargo-deny`.
The E2E tests also need Node 24 or later; `make wasm-test` needs `wasm-pack`.
The local stack (`make up`, `make run`, `make e2e`, `make test-integration`)
needs podman with `podman-compose` (or Docker with `COMPOSE="docker compose"`);
on macOS run `podman machine init` and `podman machine start` once. See
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#local-development-with-podman).

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
| `make up` | Runs `ports-free`, then starts the DEV ONLY compose stack: postgres, keycloak (realm `icam`, <http://127.0.0.1:8180>) and the Caddy TLS proxy |
| `make down` | Stops and removes the compose stack (volumes kept) |
| `make down-clean` | Same, and deletes the volumes (`down -v`) |
| `make ports-free` | Fails fast, naming the busy ports, if a TLS port from `deploy/compose/ports.env` is in use. Override ports from the environment |
| `make run` | Builds `web-gallery`, runs `up`, then the gateway on the host with `deploy/compose/gateway.env.example`. Browse <https://localhost:9442> and sign in as `console_user` / `dev-only-console_user` |
| `make test-integration` | Gateway tests including the Postgres `pg_flow` tests, against the compose Postgres (`CONSOLE_TEST_DATABASE_URL`). Without a DB those tests print `NOT RUN` |
| `make wasm-test` | console-ui and console-shell browser tests with wasm-pack (headless Firefox by default; `WASM_BROWSER=chrome` to override). Not part of `make check` |
| `make e2e-install` | `npm ci` in `e2e/` and installs Playwright browsers (chromium, firefox, webkit) |
| `make e2e` | Builds `web`, `web-gallery` and `gateway`, runs `up`, then Playwright + axe through the compose Keycloak and Caddy TLS. Playwright starts the host gateways (ports in `deploy/compose/ports.env`). Pass options with `E2E_ARGS`, for example `E2E_ARGS="--project=setup --project=chromium"`. Not part of `make check`. CI runs it on Chromium, Firefox and WebKit |
| `make e2e-typecheck` | `npm ci` and `tsc --noEmit` in `e2e/` |

## Workflow

Feature work goes through the multi-agent workflow (`/console-workflow`), ported
from usg-dcim; see `.claude/skills/console-workflow/SKILL.md`.
