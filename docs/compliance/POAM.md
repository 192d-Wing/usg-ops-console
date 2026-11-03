# Plan of Action and Milestones (POA&M)

Status as of M1, 2026-11-03. Owner and milestone fields are placeholders
until the team assigns them.

## Items from `POAM:` markers in code

There is one `POAM:` marker in the repository (in `deny.toml`). No `// POAM:`
markers exist in Rust source.

| ID | Weakness | Source | Control | Owner | Milestone |
| --- | --- | --- | --- | --- | --- |
| POAM-001 | RUSTSEC-2024-0370: `proc-macro-error` is unmaintained (not a known vulnerability). It comes in through `yew-macro` and is used at build time only. | `deny.toml` `[advisories] ignore` | RA-5, SI-2 | TBD | TBD (re-check on each yew release) |
| POAM-002 | RUSTSEC-2025-0141: `bincode` 1.x is unmaintained (not a known vulnerability). It comes in through `gloo-worker`. The console does not use yew workers. | `deny.toml` `[advisories] ignore` | RA-5, SI-2 | TBD | TBD (re-check on each yew release) |

`yew-router` 0.20 (added in M1) also depends on both crates. `cargo audit`
reports them as 2 allowed warnings.

## Open verification gaps and deferred findings (M1)

| ID | Weakness | Control / reference | Owner | Milestone |
| --- | --- | --- | --- | --- |
| POAM-003 | wasm-bindgen browser tests in `crates/console-ui/tests/wasm.rs` (Button busy/disabled, FormField `aria-describedby`, Tabs keys, Modal Esc/focus return/inert, Toggle `aria-checked`, Flashbar dismiss focus) compile, but they have **never been run**. `make wasm-test` needs wasm-pack and Firefox, which were not installed. CI does not run them. | SA-11; WCAG 2.1 | TBD | TBD |
| POAM-004 | An axe accessibility scan of `/gallery` has **not been run** (A11Y-009). The M1 exit criterion "axe check passes" is not met. | WCAG 2.1 AA | TBD | TBD (M2 Playwright/axe) |
| POAM-005 | A11Y-007: checkbox, toggle, select arrow, modal width and borders use fixed px sizes instead of tokens. The checkbox (18px) is smaller than `--cui-size-target-min`. | WCAG 2.1 SC 1.4.4 | TBD | TBD |
| POAM-006 | A11Y-008: on narrow screens the side navigation stacks above the content and cannot be collapsed (there is no disclosure button). | WCAG 2.1 SC 1.4.10 (passes); design rule | TBD | TBD (before M2/M4 add tool links) |
| POAM-007 | PERF-002 (partial fix): `Table` still runs `item_key` on every item on each render. Selection lookup is O(1), but key memoisation is deferred until a real tool uses Table. | Performance | TBD | TBD |
| POAM-008 | PERF-003: three render-blocking CSS requests (tokens, components, shell), about 5 KB gz in total. | Performance | TBD | TBD (optional) |
| POAM-009 | SEC-003: the shell's theme storage DOM path (`theme::dom::load/apply/save` in `crates/console-shell/src/theme.rs`) has no test. Only the parse allow-list it relies on is tested. | SI-10 | TBD | TBD (when wasm tests run in CI) |
