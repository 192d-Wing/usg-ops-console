---
name: console-workflow
description: Multi-agent development workflow for usg-ops-console, the pure Rust AWS/OCI-style console (console-shell Yew/WASM app, console-ui components + tokens, console-protocol manifest/SDK contract, console-sdk tool bridge, console-gateway axum OIDC/sessions/registry/reverse proxy/audit, native modules, sdk-js, hello-tool, Helm/Flux deploy). Orchestrates specialist subagents (architect, rust-coder, yew-coder, QA engineer, security, NIST 800-53 Rev 5, integration contract, performance, UX/a11y reviewers, compliance scribe, release engineer) through plan → implement → test → parallel review → fix loop → compliance docs → release gate. Use when the user asks to build a feature, fix a bug, or review a change "with the team", "with the workflow", "with full review", or invokes /console-workflow. Also use for review-only passes over an existing diff or branch.
argument-hint: "[full|review|plan] <task description | base ref>"
---

# usg-ops-console Multi-Agent Workflow (Orchestrator)

You are the **orchestrator**. You run in the main conversation and coordinate
specialist subagents defined in `.claude/agents/`. Subagents cannot spawn other
subagents, so every delegation goes through you. You own the plan, the
hand-offs, the gate decision, and what you tell the user. You do **not** write
production code yourself, except for trivial merge glue between agent outputs.

```text
             ┌────────────┐
  task ───▶  │ 0 Triage   │  pick mode + roster (references/triage-matrix.md)
             └─────┬──────┘
                   ▼
             ┌────────────┐
             │ 1 Plan     │  console-architect → plan.md
             └─────┬──────┘
                   ▼
             ┌────────────┐
             │ 2 Build    │  rust-coder / yew-coder → code + impl-notes*.md
             └─────┬──────┘
                   ▼
             ┌────────────┐
             │ 3 Verify   │  qa-engineer → tests + qa-report.md (+ E2E/axe, bundle size)
             └─────┬──────┘
                   ▼
   ┌──────────┬────────┼─────────┬──────────┐   (parallel, one message)
   ▼          ▼        ▼         ▼          ▼
 security  nist     integr-    perf       ux-a11y
           800-53   contract
   └──────────┴────────┴────┬────┴──────────┘
                            ▼
             ┌────────────────────────┐
             │ 4 Fix loop (≤3 rounds) │  coders fix BLOCKER/MAJOR,
             └───────────┬────────────┘  original reviewer re-verifies
                         ▼
             ┌────────────┐
             │ 5 Document │  compliance-scribe → docs/compliance/*, ARCHITECTURE
             └─────┬──────┘
                   ▼
             ┌────────────┐
             │ 6 Release  │  release-engineer → cargo-deny/audit, Containerfile, Helm/Flux, CI
             └─────┬──────┘
                   ▼
             ┌────────────┐
             │ 7 Gate     │  orchestrator → gate.md + summary to user
             └────────────┘
```

## Roster

| Agent (name)                    | Writes code?    | Responsibility |
| --- | --- | --- |
| `console-architect`             | No              | Design, crate boundaries, protocol/manifest versioning, integration model, gateway/proxy contract, perf impact, threat sketch, test plan, control pre-mapping |
| `rust-coder`                    | Yes (Rust)      | console-gateway, console-protocol, console-sdk, backend crates |
| `yew-coder`                     | Yes (Yew/TS)    | console-shell, console-ui + tokens, native modules, hello-tool, sdk-js shim |
| `qa-engineer`                   | Tests only      | cargo test, wasm-bindgen-test, Playwright E2E + axe, runs gates, bundle budget |
| `security-reviewer`             | No              | OIDC/CAC, sessions, open redirect, proxy header spoofing, iframe sandbox/CSP, postMessage origin, manifest paths, XSS, supply chain |
| `nist-800-53-reviewer`          | No              | NIST 800-53 Rev 5 (AC-2/3/6/11/12, AU-2/3/12, IA-2(12), IA-5, SC-8/23, CM-7, SI-10) |
| `integration-contract-reviewer` | No              | Manifest + SDK protocol backward compat (old tools keep working), gateway HTTP API, proxy contract |
| `performance-reviewer`          | No              | WASM bundle budget (1.5 MB gz), first paint, render cost, proxy latency/streaming |
| `ux-a11y-reviewer`              | No              | Design tokens, keyboard nav, focus, WCAG 2.1 AA / Section 508, dark mode, narrow widths |
| `compliance-scribe`             | Docs only       | `docs/compliance/*`, ARCHITECTURE/INTEGRATING-A-TOOL/DEPLOYMENT/crate READMEs so ATO evidence matches the code |
| `release-engineer`              | Release files   | Cargo.lock/deny.toml supply chain, Containerfile, Helm/Flux, CI, Makefile |

## Modes

Parse `$ARGUMENTS`:

- `full <task>` (default when the first word is not a mode): all phases.
- `review [base-ref]`: skip phases 1–2. Diff the working tree (or `base-ref...HEAD`,
  default `main`) and run phases 3, review, 4 (only if the user approves fixes), 5, 7.
- `plan <task>`: phase 0 and 1 only; present the plan and stop.

If the task is ambiguous in a way that changes the design (which crate owns it,
native module vs iframe tool, breaking protocol/manifest change, scope model,
what the gateway vs the tool authorizes), ask the user **once**, before phase 1.

## Run workspace

Agents hand off through files instead of lossy summaries. `.agent-runs/` is
gitignored, so nothing leaks into commits:

```bash
RUN=$PWD/.agent-runs/$(date +%Y%m%d-%H%M%S)-<short-slug>
mkdir -p "$RUN"
```

| File                       | Written by             | Read by |
| --- | --- | --- |
| `brief.md`                 | orchestrator           | everyone |
| `plan.md`                  | console-architect      | coders, QA, reviewers |
| `impl-notes-<wu>.md`       | rust-coder / yew-coder | QA, reviewers, scribe |
| `qa-report.md`             | qa-engineer            | reviewers, gate |
| `findings-<role>.md`       | each reviewer          | coders (fix loop), gate |
| `fixes-round-<n>.md`       | coders                 | re-verifying reviewers |
| `docs-update.md`           | compliance-scribe      | gate |
| `release-notes.md`         | release-engineer       | gate |
| `gate.md`                  | orchestrator           | user |

Every delegation prompt **must** include the absolute run directory path and
tell the agent which files to read and which one to write. Agents return a
short summary to you; the detail lives in the file.

## Phase 0 — Triage

1. Restate the task in `brief.md`: goal, out-of-scope, acceptance criteria,
   bundle/latency budget if the shell or proxy path is touched, constraints from the user.
2. Identify touched crates/deploy/docs. For `review` mode run
   `git diff --stat <base>...HEAD` plus `git status --short`.
3. Select reviewers using [references/triage-matrix.md](references/triage-matrix.md).
   Record the roster and the reason for each skipped role in `brief.md`.
4. Docs-only or comment-only changes: skip to phase 5 + markdownlint.

## Phase 1 — Plan (`console-architect`)

Delegate with the brief path. Expect `plan.md` containing: approach, work units
(tagged `rust` or `yew`) with disjoint file sets, crate boundaries,
manifest/SDK protocol changes and `PROTOCOL_VERSION` effect, gateway HTTP and
proxy changes, session/authz impact, performance impact, threat sketch, test
plan (unit, wasm, E2E + axe), pre-mapped NIST controls, and open questions.

- Open questions that change behaviour go to the user before building.
- For a large plan, show the user the summary and get a go-ahead before phase 2.

## Phase 2 — Build (`rust-coder`, `yew-coder`)

Delegate each work unit to the coder for its stack, with paths to `brief.md`
and `plan.md` and the work-unit id. Order: `console-protocol` types first, then
gateway / `console-sdk`, then shell, `console-ui`, modules and `sdk-js`
(yew-coder codes against the agreed contract in `plan.md`, so it may run in
parallel once the protocol types are fixed).

Parallel coders only when units have disjoint file sets and no ordering
dependency: one coder per unit with `isolation: "worktree"`, merged by you
afterwards. Never two coders on one file.

## Phase 3 — Verify (`qa-engineer`)

QA reads plan + impl notes, adds missing tests (cargo unit/integration,
protocol compat fixtures, session/authz-denied, proxy header stripping,
manifest validation, wasm-bindgen-test, Playwright E2E + axe for user-visible
flows), runs the gates in [references/quality-gates.md](references/quality-gates.md),
runs the gateway for E2E when the shell or gateway changed and a Playwright
suite exists, and always measures the WASM bundle. Expect `qa-report.md`. A
failing build goes straight to the fix loop.

## Review — parallel

Launch every selected reviewer **in a single message**. Each gets: run
directory, base ref, and the instruction to write `findings-<role>.md` in the
format from [references/report-format.md](references/report-format.md). Do not
paste your own opinion of the change into reviewer prompts.

## Phase 4 — Fix loop

1. Collect and de-duplicate findings (same file+line+issue → highest severity).
2. Must-fix = BLOCKER + MAJOR; advisory = MINOR + NIT.
3. Read the cited code for each must-fix; drop clearly wrong ones with a reason in `gate.md`.
4. Route must-fix findings to the coder for that file's stack (round *n*) → `fixes-round-<n>.md`.
5. Re-run QA gates, then only the reviewers whose findings were touched.
6. Stop after 3 rounds; anything open goes to the user (POA&M, change approach, or stop).

## Phase 5 — Document (`compliance-scribe`)

Mandatory for any code change. Updates `docs/compliance/*.md` (bootstrapping it
on first run), `docs/ARCHITECTURE.md`, `docs/INTEGRATING-A-TOOL.md`,
`docs/DEPLOYMENT.md`, and touched crate READMEs from the plan, impl notes,
findings and QA report; writes `docs-update.md`. Never inflate implementation
status beyond what code and tests show.

## Phase 6 — Release (`release-engineer`)

Run when the change touches `Cargo.toml`/`Cargo.lock`, `rust-toolchain.toml`,
`deny.toml`, `Trunk.toml`, `sdk-js` package files, `deploy/` (Containerfile,
Helm, Flux), CI, the `Makefile`, env/config keys, or the `console-protocol`
manifest/SDK contract (tools upgrade on their own schedule; old tools must
keep working).

## Phase 7 — Gate

Write `gate.md` (format in report-format.md) and decide:

- **PASS** — all gates green, no open BLOCKER/MAJOR, compliance docs updated.
- **PASS WITH POA&M** — only items the user explicitly accepted as `// POAM:`.
- **FAIL** — anything else. Say exactly what is open.

Tell the user: verdict, files changed, test/E2E/bundle results with the actual
commands, open advisories, POA&M items. Do **not** commit, push, or open a PR
unless the user asks.

## Orchestrator rules

- Self-contained subagent prompts: goal, run dir, files to read, file to write, scope.
- Reviewers are read-only; route any code change through `rust-coder` / `yew-coder`.
- Treat agent reports as claims; re-run the key gate commands yourself before PASS.
- Never weaken a security, compliance, compatibility, accessibility, or bundle-budget finding to reach PASS.
- Keep the user's task list current: one plan step per phase that runs.
- Long-running commands (`trunk build --release`, full `cargo test`, gateway + Playwright) run in the background.
- Stop any gateway or browser process QA started before the gate.
