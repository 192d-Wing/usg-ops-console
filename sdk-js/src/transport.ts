// postMessage bridge for the tool side. Behaviour mirrors
// crates/console-sdk/src/transport.rs: the console origin comes only from the
// same-origin `GET /_console/context`; incoming messages must match that
// origin exactly and come from `window.parent`; unknown types are ignored;
// the size cap applies before parsing; outgoing messages always name the
// console origin as targetOrigin (never "*").

import { PROTOCOL_VERSION } from "./version.js";
import { ROUTE_TOOL_CONTEXT, decodeShellToTool, decodeToolContext, encodeToolToShell, formatScopeHeader, truncateNotify } from "./protocol.js";
import type { FocusEdge, Level, Scope, ShellToTool, ToolContext, ToolToShell, User } from "./protocol.js";

export const READY_RETRIES_MS: readonly number[] = [250, 500, 1000];

export interface ConsoleState {
  user: User;
  scope: Scope;
  theme: string;
  locale: string;
  path: string | null;
  toolId: string | null;
  protocol: number;
}

export interface Handlers {
  onInit?: (state: ConsoleState) => void;
  onScope?: (scope: Scope) => void;
  onTheme?: (theme: string) => void;
  onNavigate?: (path: string) => void;
  onSessionRefreshed?: (ok: boolean, idleExpiresAt: string | null) => void;
  /** Focus is entering the frame; the default moves focus to the first/last tabbable element. */
  onFocus?: (edge: FocusEdge) => void;
}

/** The subset of `Window` the bridge uses (injectable for tests). */
export interface BridgeWindow {
  readonly location: { readonly origin: string };
  readonly parent: unknown;
  addEventListener(type: "message", listener: (e: MessageEvent) => void): void;
  removeEventListener(type: "message", listener: (e: MessageEvent) => void): void;
  setTimeout(handler: () => void, ms: number): number;
  clearTimeout(id: number): void;
  readonly document?: Document;
}

export interface ParentLike {
  postMessage(message: string, targetOrigin: string): void;
}

export interface ConnectOptions {
  /** Defaults to `globalThis.window`. */
  window?: BridgeWindow;
  /** Defaults to `globalThis.fetch`. */
  fetch?: (input: string, init: RequestInit) => Promise<Response>;
  /**
   * Timeout for `GET /_console/context`, in ms (default
   * `CONTEXT_TIMEOUT_MS`, matching `console-sdk`). On timeout `connect`
   * resolves `null` (standalone).
   */
  contextTimeoutMs?: number;
}

/** Default `/_console/context` timeout; same 2 s fallback as `console-sdk`. */
export const CONTEXT_TIMEOUT_MS = 2000;

export interface Bridge {
  readonly consoleOrigin: string;
  readonly toolId: string;
  /** Latest state from Init/ScopeChanged/ThemeChanged/Navigate; `null` before Init. */
  state(): ConsoleState | null;
  /** Count of messages dropped for origin/source/size/shape. */
  dropped(): number;
  setTitle(title: string): void;
  setBreadcrumbs(items: ReadonlyArray<readonly [label: string, path: string]>): void;
  notify(level: Level, message: string): void;
  setBadge(key: string, count: number | null): void;
  navigateConsole(tool: string, path: string): void;
  pathChanged(path: string): void;
  requestSessionRefresh(): void;
  focusShell(reverse?: boolean): void;
  /** `X-Console-Scope` value for the current scope, or `null`. Header name: `SCOPE_HEADER`. */
  scopeHeader(): string | null;
  /** Remove listeners and timers. */
  close(): void;
}

const TABBABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type=hidden]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"]):not([disabled])';

function focusEdge(doc: Document | undefined, edge: FocusEdge): void {
  if (doc === undefined) return;
  const list = Array.from(doc.querySelectorAll<HTMLElement>(TABBABLE)).filter(
    (el) => el.getClientRects().length > 0 && el.closest('[inert], [hidden], [aria-hidden="true"]') === null && el.tabIndex >= 0,
  );
  const el = edge === "first" ? list[0] : list[list.length - 1];
  el?.focus();
}

function isParent(p: unknown): p is ParentLike {
  return typeof p === "object" && p !== null && typeof (p as { postMessage?: unknown }).postMessage === "function";
}

/** Validate a context the same way as `ConsoleBridge::with_context`. */
export function checkContext(ctx: ToolContext, ownOrigin: string): boolean {
  const o = ctx.console_origin;
  return o !== "" && o !== "null" && o.startsWith("http") && o !== ownOrigin;
}

/** `GET /_console/context` (same origin, cookie credentials). Rejects on any failure. */
export async function fetchContext(
  f: NonNullable<ConnectOptions["fetch"]>,
  timeoutMs: number = CONTEXT_TIMEOUT_MS,
): Promise<ToolContext> {
  const signal = AbortSignal.timeout(timeoutMs);
  // Also race the timeout, so a fetch that ignores the signal cannot hang connect().
  const timedOut = new Promise<never>((_, reject) => {
    signal.addEventListener("abort", () => reject(new Error("console context unavailable: timeout")), { once: true });
  });
  const load = async (): Promise<ToolContext> => {
    const resp = await f(ROUTE_TOOL_CONTEXT, {
      method: "GET",
      credentials: "same-origin",
      headers: { Accept: "application/json" },
      signal,
    });
    if (!resp.ok) throw new Error(`console context unavailable: HTTP ${resp.status}`);
    return decodeToolContext(await resp.text());
  };
  const p = load();
  p.catch(() => undefined);
  return Promise.race([p, timedOut]);
}

/**
 * Connect to the console shell. Resolves to `null` when the tool is
 * standalone: no gateway context (including no answer within
 * `contextTimeoutMs`), an invalid context, or not framed.
 */
export async function connect(handlers: Handlers = {}, opts: ConnectOptions = {}): Promise<Bridge | null> {
  const win: BridgeWindow | undefined = opts.window ?? (typeof window === "undefined" ? undefined : window);
  const f = opts.fetch ?? (typeof fetch === "undefined" ? undefined : (i: string, init: RequestInit) => fetch(i, init));
  if (win === undefined || f === undefined) return null;
  let ctx: ToolContext;
  try {
    ctx = await fetchContext(f, opts.contextTimeoutMs ?? CONTEXT_TIMEOUT_MS);
  } catch {
    return null;
  }
  const parent = win.parent;
  if (!checkContext(ctx, win.location.origin) || parent === win || !isParent(parent)) return null;
  return createBridge(win, parent, ctx, handlers);
}

function createBridge(win: BridgeWindow, parent: ParentLike, ctx: ToolContext, h: Handlers): Bridge {
  const origin = ctx.console_origin;
  let state: ConsoleState | null = null;
  let dropped = 0;
  let closed = false;
  const timers: number[] = [];

  const send = (msg: ToolToShell): void => {
    if (closed) return;
    try {
      parent.postMessage(encodeToolToShell(msg), origin);
    } catch (e) {
      console.warn("console-sdk:", e);
    }
  };

  const apply = (m: ShellToTool): void => {
    switch (m.type) {
      case "Init":
        state = {
          user: m.user,
          scope: m.scope,
          theme: m.theme,
          locale: m.locale,
          path: m.path ?? null,
          toolId: m.tool_id ?? null,
          protocol: Math.min(Math.max(m.protocol, 1), PROTOCOL_VERSION),
        };
        h.onInit?.(state);
        break;
      case "ScopeChanged":
        if (state) state = { ...state, scope: m.scope };
        h.onScope?.(m.scope);
        break;
      case "ThemeChanged":
        if (state) state = { ...state, theme: m.theme };
        h.onTheme?.(m.theme);
        break;
      case "Navigate":
        if (state) state = { ...state, path: m.path };
        h.onNavigate?.(m.path);
        break;
      case "SessionRefreshed":
        h.onSessionRefreshed?.(m.ok, m.idle_expires_at ?? null);
        break;
      case "Focus":
        if (h.onFocus) h.onFocus(m.edge);
        else focusEdge(win.document, m.edge);
        break;
    }
  };

  const onMessage = (e: MessageEvent): void => {
    if (closed) return;
    let m: ShellToTool | null;
    try {
      if (e.source !== parent || e.origin !== origin || origin === "null" || typeof e.data !== "string") throw new Error("untrusted");
      m = decodeShellToTool(e.data);
    } catch {
      dropped += 1;
      console.debug(`console-sdk: dropped ${dropped} message(s)`);
      return;
    }
    if (m !== null) apply(m);
  };

  const onKey = (e: KeyboardEvent): void => {
    if (closed || e.key !== "F6" || e.altKey || e.metaKey) return;
    e.preventDefault();
    send({ type: "FocusShell", reverse: e.shiftKey });
    const a = win.document?.activeElement as HTMLElement | null | undefined;
    if (a && typeof a.blur === "function") a.blur();
  };

  win.addEventListener("message", onMessage);
  win.document?.addEventListener("keydown", onKey);

  const ready: ToolToShell = { type: "Ready", protocol: PROTOCOL_VERSION };
  send(ready);
  for (const ms of READY_RETRIES_MS) {
    timers.push(win.setTimeout(() => {
      if (state === null) send(ready);
    }, ms));
  }

  return {
    consoleOrigin: origin,
    toolId: ctx.tool_id,
    state: () => state,
    dropped: () => dropped,
    setTitle: (title) => send({ type: "SetTitle", title }),
    setBreadcrumbs: (items) => send({ type: "SetBreadcrumbs", items: items.map(([l, p]): [string, string] => [l, p]) }),
    notify: (level, message) => send({ type: "Notify", level, message: truncateNotify(message) }),
    setBadge: (key, count) => send({ type: "SetBadge", key, count }),
    navigateConsole: (tool, path) => send({ type: "NavigateConsole", tool, path }),
    pathChanged: (path) => send({ type: "PathChanged", path }),
    requestSessionRefresh: () => send({ type: "RequestSessionRefresh" }),
    focusShell: (reverse = false) => send({ type: "FocusShell", reverse }),
    scopeHeader: () => (state === null ? null : formatScopeHeader(state.scope)),
    close: () => {
      closed = true;
      win.removeEventListener("message", onMessage);
      win.document?.removeEventListener("keydown", onKey);
      for (const t of timers) win.clearTimeout(t);
    },
  };
}
