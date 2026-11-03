# usg-ops-console

Pure Rust (Yew/WASM + axum) operations console: shared chrome, sign-in, and a
tool container that independently deployed USG tools plug into. See
[docs/PLAN.md](docs/PLAN.md) for architecture and milestones.

## Layout

| Crate | Role |
|-------|------|
| `console-protocol` | Tool manifest + shell ⇄ tool SDK message types (compat contract) |
| `console-ui` | Yew components; design tokens live in `console-shell/assets/tokens.css` |
| `console-sdk` | Tool-side bridge (origin-checked encode/decode; transport in M4) |
| `console-shell` | The Yew app, built with Trunk |
| `console-gateway` | axum server: static shell, security headers (OIDC/registry/proxy in M3–M4) |

## Develop

Requires stable Rust with `wasm32-unknown-unknown`, `trunk`, and `cargo-deny`.

```bash
make check
```

```bash
make run
```

`make run` builds the shell and serves it at <http://127.0.0.1:8080>.

## Workflow

Feature work goes through the multi-agent workflow (`/console-workflow`), ported
from usg-dcim; see `.claude/skills/console-workflow/SKILL.md`.
