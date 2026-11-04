# Quality Gates

Commands mirror the `Makefile` and `.github/workflows/ci.yml`. Scope with
`-p <crate>` while iterating; run the full workspace form once before the final gate.

| Gate | Command (from repo root) | Required |
| --- | --- | --- |
| Format | `cargo fmt --all -- --check` (`make fmt`) | Any Rust touched |
| Clippy (host) | `cargo clippy --workspace --all-targets --locked -- -D warnings` | Any Rust touched |
| Clippy (wasm) | `cargo clippy -p console-shell --target wasm32-unknown-unknown --locked -- -D warnings` | Shell, `console-ui`, `console-sdk`, `console-protocol`, modules touched |
| Tests | `cargo test --workspace --locked` (`make test`) | Any Rust touched |
| WASM tests | `wasm-pack test --headless --firefox <crate>` (or `--chrome`) | UI crates with `wasm_bindgen_test` tests touched; NOT RUN if none exist or no browser |
| Shell build | `cd crates/console-shell && trunk build --release` (`make web`) | Shell, `console-ui`, `console-sdk`, `console-protocol`, modules touched |
| Bundle budget | `gzip -c crates/console-shell/dist/*.wasm \| wc -c` ≤ `1572864` | Whenever the shell build runs |
| Gateway build | `cargo build --release --locked -p console-gateway` (`make gateway`) | Gateway touched |
| E2E + axe | `make run` (background), then the Playwright suite with axe; stop the gateway after | Shell or gateway touched **and** a Playwright suite exists — otherwise NOT RUN |
| sdk-js | `npx tsc --noEmit` and its tests in `sdk-js/` | `sdk-js/` exists and is touched |
| Supply chain | `cargo deny check` (`make deny`); `cargo audit` (`make audit`) | `Cargo.toml`/`Cargo.lock`/`deny.toml` touched; always before final gate |
| Container | `podman build -f deploy/Containerfile -t usg-ops-console:dev .` (`make image`) | `deploy/Containerfile`, toolchain, or build layout touched |
| Helm | `helm lint deploy/helm/<chart>` and `helm template deploy/helm/<chart> >/dev/null` | `deploy/helm/**` exists and is touched |
| Flux/kustomize | `kustomize build` on touched `deploy/flux` paths | Those paths touched |
| Annotations | `grep -rn "NIST 800-53:" crates \| wc -l` (count should not drop) | Code changes |
| Markdown | `npx markdownlint-cli "docs/**/*.md"` | Docs changed |

`make check` runs fmt, lint, test, web, deny; it does not check the bundle
budget or run `cargo audit`, so record those separately.

## Performance budget

- Shell WASM ≤ 1.5 MB gzip (CI `web` job enforces it). Growth > 10% vs `main`
  is a MAJOR finding unless justified in plan.md.
- Baseline: 204,977 B gzip (M4 final, fix round 1, 2026-11-04; +18.3% over M3 accepted by
  the orchestrator as PERF-001 with trims applied. M3 was 173,207 B, M2 141,918 B). CI `web` job warns above
  baseline x1.10 (225,474 B) and fails above 1,572,864 B.
- Release profile keeps `opt-level = "z"`, `lto = true`, `codegen-units = 1`.
- Chrome renders before any tool fetch; no blocking work on first paint.
- Gateway proxy streams bodies; no full buffering, no per-request client
  construction, timeouts on every upstream call.
- No blocking calls on the tokio runtime; no locks held across `.await`.

## Rules

- Record every command with exit status and the last relevant output lines in
  `qa-report.md`. "Tests pass" without the command is not evidence.
- Missing environment or suite (no trunk, no wasm target, no browsers, no
  podman, no Playwright suite yet) → gate is **NOT RUN**, never PASS.
- No new `#[ignore]`, `#[allow(...)]`, `test.skip`, `// @ts-ignore`, or
  `eslint-disable` without a justification comment and a finding entry.
