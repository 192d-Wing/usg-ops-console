import { describe, expect, it, vi } from "vitest";
import { CONTEXT_TIMEOUT_MS, MAX_MESSAGE_BYTES, MAX_NOTIFY_CHARS, SCOPE_HEADER, connect } from "../src/index.js";
import type { BridgeWindow, ConnectOptions } from "../src/index.js";

const CONSOLE = "https://console.local";
const TOOL = "https://hello.tools.local";
const CTX = JSON.stringify({ tool_id: "hello", console_origin: CONSOLE, protocol: 1 });
const INIT = JSON.stringify({
  type: "Init",
  protocol: 1,
  theme: "dark",
  locale: "en-US",
  scope: { site: "dc1", tenant: "bad value!", environment: null },
  user: { id: "u1", display_name: "Jane", roles: [] },
  path: "/racks",
  tool_id: "hello",
});

class FakeParent {
  sent: Array<{ msg: string; target: string }> = [];
  postMessage(msg: string, target: string): void {
    this.sent.push({ msg, target });
  }
}

class FakeWindow implements BridgeWindow {
  location = { origin: TOOL };
  parent: unknown;
  listeners = new Set<(e: MessageEvent) => void>();
  timers: Array<() => void> = [];
  constructor(parent?: unknown) {
    this.parent = parent ?? this;
  }
  addEventListener(_t: "message", l: (e: MessageEvent) => void): void {
    this.listeners.add(l);
  }
  removeEventListener(_t: "message", l: (e: MessageEvent) => void): void {
    this.listeners.delete(l);
  }
  setTimeout(h: () => void): number {
    this.timers.push(h);
    return this.timers.length;
  }
  clearTimeout(): void {}
  dispatch(data: unknown, origin: string, source: unknown): void {
    for (const l of this.listeners) l({ data, origin, source } as unknown as MessageEvent);
  }
}

const okFetch = (body: string, status = 200): NonNullable<ConnectOptions["fetch"]> =>
  vi.fn(async () => new Response(body, { status }));

async function framed() {
  const parent = new FakeParent();
  const win = new FakeWindow(parent);
  const fetch = okFetch(CTX);
  const onInit = vi.fn();
  const onTheme = vi.fn();
  const bridge = await connect({ onInit, onTheme }, { window: win, fetch });
  if (bridge === null) throw new Error("expected a bridge");
  return { parent, win, fetch, bridge, onInit, onTheme };
}

describe("connect", () => {
  it("fetches /_console/context same-origin and sends Ready to the pinned origin", async () => {
    const { parent, fetch } = await framed();
    expect(fetch).toHaveBeenCalledWith("/_console/context", expect.objectContaining({ method: "GET", credentials: "same-origin" }));
    expect(parent.sent).toStrictEqual([{ msg: '{"type":"Ready","protocol":1}', target: CONSOLE }]);
  });

  it("returns null when standalone", async () => {
    const parent = new FakeParent();
    // Context endpoint missing (not behind the gateway).
    expect(await connect({}, { window: new FakeWindow(parent), fetch: okFetch("nope", 404) })).toBeNull();
    // Network failure.
    const reject: NonNullable<ConnectOptions["fetch"]> = async () => {
      throw new TypeError("network");
    };
    expect(await connect({}, { window: new FakeWindow(parent), fetch: reject })).toBeNull();
    // Body is not a ToolContext.
    expect(await connect({}, { window: new FakeWindow(parent), fetch: okFetch("<html>") })).toBeNull();
    // Top level, not framed.
    expect(await connect({}, { window: new FakeWindow(), fetch: okFetch(CTX) })).toBeNull();
    // Invalid console origins.
    for (const o of ["", "null", "javascript:x", TOOL]) {
      const ctx = JSON.stringify({ tool_id: "hello", console_origin: o, protocol: 1 });
      expect(await connect({}, { window: new FakeWindow(parent), fetch: okFetch(ctx) }), o).toBeNull();
    }
    expect(parent.sent).toStrictEqual([]);
  });

  it("resolves null when /_console/context does not answer in time", async () => {
    const parent = new FakeParent();
    let seen: AbortSignal | undefined;
    const hang: NonNullable<ConnectOptions["fetch"]> = (_i, init) => {
      seen = init.signal ?? undefined;
      return new Promise<Response>(() => undefined);
    };
    const t0 = Date.now();
    expect(await connect({}, { window: new FakeWindow(parent), fetch: hang, contextTimeoutMs: 20 })).toBeNull();
    expect(Date.now() - t0).toBeLessThan(1500);
    expect(seen?.aborted).toBe(true);
    expect(parent.sent).toStrictEqual([]);
  });

  it("passes an abort signal with the default 2 s timeout", async () => {
    expect(CONTEXT_TIMEOUT_MS).toBe(2000);
    const { fetch } = await framed();
    expect(fetch).toHaveBeenCalledWith("/_console/context", expect.objectContaining({ signal: expect.any(AbortSignal) }));
  });

  it("retries Ready until Init arrives", async () => {
    const { parent, win } = await framed();
    win.timers[0]?.();
    expect(parent.sent).toHaveLength(2);
    win.dispatch(INIT, CONSOLE, parent);
    win.timers[1]?.();
    expect(parent.sent).toHaveLength(2);
  });
});

describe("incoming messages", () => {
  it("accepts Init from the console origin and the parent", async () => {
    const { parent, win, bridge, onInit } = await framed();
    win.dispatch(INIT, CONSOLE, parent);
    expect(onInit).toHaveBeenCalledOnce();
    expect(bridge.state()?.path).toBe("/racks");
    expect(bridge.scopeHeader()).toBe("site=dc1");
    expect(SCOPE_HEADER).toBe("x-console-scope");
  });

  it("rejects foreign origins", async () => {
    const { parent, win, bridge, onInit } = await framed();
    for (const o of ["https://evil.example", "null", "", `${CONSOLE}:443x`, TOOL]) win.dispatch(INIT, o, parent);
    expect(onInit).not.toHaveBeenCalled();
    expect(bridge.dropped()).toBe(5);
  });

  it("rejects messages whose source is not window.parent", async () => {
    const { win, bridge, onInit } = await framed();
    win.dispatch(INIT, CONSOLE, new FakeParent());
    win.dispatch(INIT, CONSOLE, win);
    win.dispatch(INIT, CONSOLE, null);
    expect(onInit).not.toHaveBeenCalled();
    expect(bridge.dropped()).toBe(3);
  });

  it("ignores unknown types, drops non-string, malformed and oversize data", async () => {
    const { parent, win, bridge, onTheme } = await framed();
    win.dispatch('{"type":"Later","x":1}', CONSOLE, parent);
    expect(bridge.dropped()).toBe(0);
    win.dispatch({ type: "ThemeChanged", theme: "dark" }, CONSOLE, parent);
    win.dispatch("{", CONSOLE, parent);
    win.dispatch(`{"type":"ThemeChanged","theme":"${"a".repeat(MAX_MESSAGE_BYTES)}"}`, CONSOLE, parent);
    expect(bridge.dropped()).toBe(3);
    expect(onTheme).not.toHaveBeenCalled();
    win.dispatch('{"type":"ThemeChanged","theme":"dark"}', CONSOLE, parent);
    expect(onTheme).toHaveBeenCalledWith("dark");
  });

  it("stops listening after close()", async () => {
    const { parent, win, bridge, onInit } = await framed();
    bridge.close();
    win.dispatch(INIT, CONSOLE, parent);
    expect(onInit).not.toHaveBeenCalled();
    expect(win.listeners.size).toBe(0);
  });
});

describe("outgoing messages", () => {
  it("always targets the console origin and truncates Notify", async () => {
    const { parent, bridge } = await framed();
    bridge.setTitle("Racks");
    bridge.setBreadcrumbs([["Racks", "/racks"], ["R12", ""]]);
    bridge.notify("warning", "x".repeat(MAX_NOTIFY_CHARS + 50));
    bridge.setBadge("alerts", null);
    bridge.navigateConsole("other", "/");
    bridge.pathChanged("/racks?row=4");
    bridge.requestSessionRefresh();
    bridge.focusShell();
    const sent = parent.sent.slice(1);
    expect(sent.every((s) => s.target === CONSOLE)).toBe(true);
    expect(sent.map((s) => JSON.parse(s.msg) as unknown)).toStrictEqual([
      { type: "SetTitle", title: "Racks" },
      { type: "SetBreadcrumbs", items: [["Racks", "/racks"], ["R12", ""]] },
      { type: "Notify", level: "warning", message: "x".repeat(MAX_NOTIFY_CHARS) },
      { type: "SetBadge", key: "alerts", count: null },
      { type: "NavigateConsole", tool: "other", path: "/" },
      { type: "PathChanged", path: "/racks?row=4" },
      { type: "RequestSessionRefresh" },
      { type: "FocusShell", reverse: false },
    ]);
  });

  it("truncates Notify by code points", async () => {
    const { parent, bridge } = await framed();
    bridge.notify("info", "\u{1F600}".repeat(MAX_NOTIFY_CHARS + 1));
    const last = parent.sent.at(-1);
    const m = JSON.parse(last?.msg ?? "{}") as { message: string };
    expect(Array.from(m.message)).toHaveLength(MAX_NOTIFY_CHARS);
  });
});
