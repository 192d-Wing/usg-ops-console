# usg-ops-console — Plan

A pure Rust console shell (AWS/OCI-style) that provides the chrome — sign-in,
top bar, service menu, region/site/tenant selector, side nav, breadcrumbs,
notifications, search — and a **tool container** where independently built
tools (usg-dcim, usg-radius, usg-provision, usg-pki, …) are mounted. Tools
integrate by shipping a manifest and talking to a small SDK, not by forking
the console.

## 1. Goals / non-goals

**Goals**

- One authenticated entry point (CAC/PIV via Keycloak OIDC) for all ops tools.
- Consistent look and navigation across tools (Cloudscape-like design tokens).
- Tools deploy and version independently; adding a tool is a manifest + a
  route, not a console release.
- Rust end to end: Yew (WASM) frontend, axum backend, shared types crate.
- NIST 800-53 evidence from day one (AC, AU, IA, SC, CM), same as usg-dcim.

**Non-goals (v1)**

- Rewriting existing tools (finch stays React; it integrates via the iframe path).
- Cross-tool data aggregation dashboards (later: "home" widgets, §6).

## 2. Architecture

```text
 browser
 ┌──────────────────────────────────────────────────────────────┐
 │ console-shell (Yew/WASM)                                     │
 │  top bar · service menu · scope picker · nav · notifications │
 │  ┌────────────────────────────────────────────────────────┐  │
 │  │ tool container                                         │  │
 │  │   A) native Yew module (compiled in, first-party)      │  │
 │  │   B) sandboxed iframe  /t/<tool>/  + console-sdk bridge│  │
 │  └────────────────────────────────────────────────────────┘  │
 └───────────────▲──────────────────────────────────────────────┘
                 │ same origin
 ┌───────────────┴──────────────────────────────────────────────┐
 │ console-gateway (axum)                                       │
 │  OIDC login/session · tool registry · reverse proxy /t/<id>/ │
 │  audit log · CSP/headers · health aggregation                │
 └──────┬──────────────┬──────────────┬─────────────────────────┘
        ▼              ▼              ▼
     usg-dcim      usg-radius    usg-provision  …  (each own backend + UI)
```

### Integration model (recommended: hybrid, manifest-driven)

| Path | When | How |
|------|------|-----|
| **B — iframe micro-frontend** (default) | Any tool, any stack, deployed separately | Gateway proxies `/t/<id>/` to the tool → same origin, one session. Tool page uses `console-sdk` (Rust crate, plus a tiny JS shim for React tools) over `postMessage` with a typed, versioned protocol. |
| **A — native module** | First-party, tightly coupled pages (home, settings, tool catalog, audit viewer) | Crate implements `ConsoleModule` trait, compiled into the shell behind a cargo feature. |

Why not runtime-loaded WASM modules in-process: Yew/wasm-bindgen has no stable
dynamic linking; each tool would duplicate the runtime anyway, and an in-process
bug or XSS in one tool would own the whole console. The iframe gives a hard
security boundary (`sandbox`, per-tool CSP) and lets finch integrate unchanged.

### Tool manifest (`console-tool.toml`, served by the tool at `/.well-known/console-tool`)

```toml
id = "dcim"
title = "Data Center (DCIM)"
category = "Infrastructure"
icon = "server"
version = "1.4.0"
sdk_protocol = 1
upstream = "http://dcim-otter:8080"     # gateway-side, from registry config not the tool
entry = "/"                              # path under /t/dcim/
required_roles = ["dcim:read"]
scopes = ["site", "tenant"]              # which console scope pickers it honors

[[nav]]
label = "Racks"
path = "/racks"
[[nav]]
label = "Alerts"
path = "/alerts"
badge = "alerts.open"                    # SDK-pushed counter
```

Registry is authoritative config in the gateway (allow-list); manifests only
supply presentation. A tool cannot register itself.

### SDK protocol v1 (shell ⇄ tool)

- Shell → tool: `Init{theme, locale, scope, user, protocol}`, `ScopeChanged`,
  `ThemeChanged`, `Navigate{path}`.
- Tool → shell: `Ready`, `SetBreadcrumbs`, `SetTitle`, `Notify{level,msg}`,
  `SetBadge`, `NavigateConsole{tool, path}`, `RequestSessionRefresh`, `Resize`
  (for non-full-height embeds).
- Origin-checked both directions; messages are `serde` enums in
  `console-protocol` crate → JSON. No tokens cross the bridge — auth is the
  gateway session cookie (HttpOnly, SameSite=Strict); the gateway injects
  identity headers / a short-lived token to upstreams.

## 3. Repository layout (cargo workspace)

```text
crates/
  console-protocol/   serde types: manifest, SDK messages, scope, user (no_std-friendly)
  console-ui/         Yew component library + design tokens (buttons, tables,
                      forms, modals, flashbar, side nav, split panel) — tools may use it
  console-sdk/        tool-side client: postMessage bridge, hooks (use_console_scope, …)
  console-shell/      the Yew app (Trunk build) — chrome + container + native modules
  console-gateway/    axum: OIDC, sessions, registry, proxy, audit, static assets
  modules/
    home/  catalog/  settings/  audit/   native ConsoleModule crates
sdk-js/               ~200-line TS shim of console-sdk for React tools (finch)
examples/hello-tool/  minimal Yew tool used by E2E and as the integration template
deploy/               Containerfile (distroless), Helm chart, Flux overlay
docs/                 ARCHITECTURE, INTEGRATING-A-TOOL, compliance/
```

Stack: Yew 0.21 (CSR) + yew-router + stylist or plain CSS tokens, Trunk,
gloo; axum + tower-http + openidconnect + tower-sessions (Postgres or Redis
store) + hyper reverse proxy; tracing → JSON audit stream.

## 4. Milestones

| M | Deliverable | Exit criteria |
|---|-------------|---------------|
| M0 | Workspace scaffold, CI (fmt, clippy -D warnings, test, trunk build, cargo-deny, cargo-audit), Containerfile, agents/skill | CI green on empty shell |
| M1 | `console-ui` core components + tokens, light/dark, a11y (WCAG 2.1 AA) | Storybook-style gallery page; axe check passes |
| M2 | Shell chrome: top bar, service menu, side nav, breadcrumbs, flashbar, router, scope picker (static) | Playwright smoke of chrome |
| M3 | Gateway: Keycloak OIDC (CAC), sessions, logout, idle timeout (AC-11/AC-12), audit log, security headers | Login E2E against keycloak in compose |
| M4 | Registry + reverse proxy + iframe container + protocol v1 + `console-sdk` + `hello-tool` | hello-tool mounts, gets scope, sets breadcrumbs, notifies |
| M5 | `sdk-js` + integrate **usg-dcim finch** as first real tool | DCIM usable inside console under one login |
| M6 | Native modules: home, tool catalog, settings, audit viewer; global search (tools contribute search providers via manifest endpoint) | — |
| M7 | Helm/Flux deploy to dev cluster, compliance docs, ATO evidence | Gate PASS |

## 5. Multi-agent workflow (ported from usg-dcim)

Same orchestrator pattern (`.claude/skills/console-workflow/SKILL.md`):
triage → plan → build → verify → parallel review → fix loop (≤3) → docs →
release → gate, handing off through `.agent-runs/<ts>-<slug>/`.

| usg-dcim agent | Console agent | Change |
|----------------|---------------|--------|
| dcim-architect | `console-architect` | Owns crate boundaries, protocol versioning, manifest schema |
| go-coder | `rust-coder` | Gateway, protocol, sdk, backend crates |
| finch-coder | `yew-coder` | shell, console-ui, native modules, hello-tool |
| qa-engineer | `qa-engineer` | cargo test, wasm-bindgen-test, Playwright E2E, axe a11y |
| api-contract-reviewer | `integration-contract-reviewer` | Manifest + SDK protocol compat (old tools must keep working), gateway HTTP API |
| security-reviewer | `security-reviewer` | + iframe sandbox/CSP, postMessage origin checks, proxy header spoofing, session fixation |
| nist-800-53-reviewer | keep | AC-11/12, AU-2/3/12, IA-2(12) PIV, SC-8/23, CM-7 |
| performance-reviewer | keep | WASM bundle size budget, first paint, proxy latency |
| — | `ux-a11y-reviewer` (new) | Design-token consistency, keyboard nav, WCAG 2.1 AA, Section 508 |
| db-migration / protocol reviewers | drop (add back if sessions/audit go to Postgres) | — |
| compliance-scribe, release-engineer | keep | Rust/Trunk/cargo-deny supply chain, Helm |

Quality gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo test --workspace`, `wasm-pack test --headless` for UI crates,
`trunk build --release` + bundle-size budget (e.g. shell ≤ 1.5 MB gz),
`cargo deny check`, `cargo audit`, Playwright E2E + axe, `helm lint`.

## 6. Open decisions

1. **Integration model** — hybrid (iframe default + native first-party) as above? *(recommended)*
2. **Scope model** — what the global picker selects: site, tenant/org, environment, or all three?
3. **Session store** — Redis vs Postgres (Postgres reuses dcim infra; Redis simpler for TTL).
4. **Design language** — Cloudscape-like (matches finch) vs OCI Redwood-like vs our own.
5. **Authorization** — console only gates tool visibility by Keycloak role, tools keep their own RBAC/ABAC? *(recommended)*
6. **Repo** — standalone repo here (recommended) vs package inside usg-dcim monorepo.
