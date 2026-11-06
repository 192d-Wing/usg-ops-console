# @usg-ops-console/sdk-js

Zero-runtime-dependency TypeScript SDK for tools framed by usg-ops-console
(protocol v1). It is a hand mirror of the Rust `console-protocol` /
`console-sdk` crates; the shared corpus in
`crates/console-protocol/tests/fixtures` (see its `MANIFEST.json`) is the
contract both sides must pass. Not published to a registry: tools vendor the
single-file build.

## Security behaviour

- The console origin comes only from the same-origin `GET /_console/context`
  (gateway tool origin, cookie auth). Never from `document.referrer` or the URL.
  The fetch is aborted after `contextTimeoutMs` (default `CONTEXT_TIMEOUT_MS`,
  2000 ms, the same fallback as Rust `console-sdk`); on timeout `connect()`
  resolves `null` and the tool runs standalone.
- Incoming messages are accepted only if `event.origin === consoleOrigin` and
  `event.source === window.parent`, and `event.data` is a string.
- Size cap (`MAX_MESSAGE_BYTES`, UTF-8 bytes) before parse; unknown `type` is
  ignored; malformed messages are dropped and counted (`bridge.dropped()`).
- Outgoing `postMessage` always targets `consoleOrigin`, never `"*"`.
- `notify` truncates to `MAX_NOTIFY_CHARS` code points.
- No tokens: auth is the gateway cookie; the backend authorizes from the
  `X-Console-Assertion` the gateway adds.

## Vendoring

```sh
cd sdk-js && npm ci && node scripts/vendor.mjs ~/Dev/usg-dcim/packages/finch/src/vendor/console-sdk
```

Writes `console-sdk.ts` (first line:
`// @usg-ops-console/sdk-js <ver> protocol=1 source=<git sha> sha256=<digest>`,
digest over everything after that line), `check.mjs` + `check.d.mts`, and
`fixtures/` (copy of the corpus). Do not edit the vendored files; re-run the
script. A compat test in the tool:

```ts
import { readFileSync } from "node:fs";
import { verifyVendoredSdk } from "@/vendor/console-sdk/check.mjs";

it("vendored console-sdk is intact", async () => {
  const src = readFileSync(new URL("../src/vendor/console-sdk/console-sdk.ts", import.meta.url), "utf8");
  expect(await verifyVendoredSdk(src, { protocol: 1 })).toMatchObject({ ok: true });
});
```

`verifyVendoredSdk` rejects a header whose `source` is `unknown` or ends in
`+dirty` (not reproducible from a commit). Vendor from a clean, committed
tree; `vendor.mjs` warns when it is not. Pass `{ allowDirty: true }` only for
local, uncommitted experiments, never in a tool's committed compat test.

Then run each `fixtures/MANIFEST.json` `sdk-message` entry through
`decodeShellToTool` / `decodeToolToShell` and `encode*` with the expectations
in `fixtures/README.md` (see `test/corpus.ts` here for a reference runner).

## Usage (React)

```tsx
import { createContext, useContext, useEffect, useState } from "react";
import { SCOPE_HEADER, connect } from "@/vendor/console-sdk/console-sdk";
import type { Bridge, ConsoleState, Scope } from "@/vendor/console-sdk/console-sdk";

interface ConsoleValue { bridge: Bridge | null; ready: boolean; state: ConsoleState | null; scope: Scope | null }
const ConsoleCtx = createContext<ConsoleValue>({ bridge: null, ready: false, state: null, scope: null });

export function ConsoleBridgeProvider({ children }: { children: React.ReactNode }) {
  const [v, setV] = useState<ConsoleValue>({ bridge: null, ready: false, state: null, scope: null });
  useEffect(() => {
    let b: Bridge | null = null;
    let live = true;
    void connect({
      onInit: (state) => setV((p) => ({ ...p, state, scope: state.scope })),
      onScope: (scope) => setV((p) => ({ ...p, scope })),
      onNavigate: (path) => router.navigate(path), // validate: relative paths only
    }).then((bridge) => {
      b = bridge;
      if (live) setV((p) => ({ ...p, bridge, ready: true })); // bridge === null: standalone
      else bridge?.close();
    });
    return () => { live = false; b?.close(); };
  }, []);
  return <ConsoleCtx.Provider value={v}>{children}</ConsoleCtx.Provider>;
}

export const useConsole = () => useContext(ConsoleCtx);

// In a page:
//   const { bridge } = useConsole();
//   useEffect(() => { bridge?.setTitle("Racks"); bridge?.setBreadcrumbs([["Racks", "/racks"]]); }, [bridge]);
//   bridge?.notify("error", "Save failed");
// In the HTTP client:
//   const h = bridge?.scopeHeader(); if (h) headers.set(SCOPE_HEADER, h);
```

`connect(handlers, { window, fetch })` accepts injected `window`/`fetch` for
tests. It resolves to `null` (standalone) when the context fetch fails, the
context is invalid, or the page is not framed. Embedded mode should still be
confirmed by the backend (e.g. `/auth/me` reporting `auth_mode: "console"`).

## API

`Bridge`: `setTitle`, `setBreadcrumbs`, `notify`, `setBadge`, `navigateConsole`,
`pathChanged`, `requestSessionRefresh`, `focusShell`, `scopeHeader()`, `state()`,
`dropped()`, `close()`. Handlers: `onInit`, `onScope`, `onTheme`, `onNavigate`,
`onSessionRefreshed`, `onFocus` (default: focus first/last tabbable element).
F6 / Shift+F6 sends `FocusShell`. Codec: `decodeShellToTool`,
`decodeToolToShell`, `encodeToolToShell`, `encodeShellToTool`,
`decodeToolContext`, `formatScopeHeader`, `truncateNotify`; constants
`PROTOCOL_VERSION`, `SDK_VERSION`, `MAX_MESSAGE_BYTES`, `MAX_NOTIFY_CHARS`,
`SCOPE_HEADER`, `ROUTE_TOOL_CONTEXT`.

## Development

`make sdk-js-test` (npm ci, `tsc --strict` typecheck, vitest). Minified+gzip
size of the vendored file is about 3.0 KB (budget 4 KB).
