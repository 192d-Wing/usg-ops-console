// Protocol v1 wire types, hand-mirrored from `console-protocol`
// (crates/console-protocol/src/message.rs, tools.rs). The shared fixture
// corpus in crates/console-protocol/tests/fixtures enforces the mirror.

/** Upper bound on a serialized message (UTF-8 bytes); larger ones are dropped unparsed. */
export const MAX_MESSAGE_BYTES = 16 * 1024;
/** Upper bound on `Notify.message`, in chars (code points); truncated beyond it. */
export const MAX_NOTIFY_CHARS = 500;
/** Same-origin route on the tool origin answering `ToolContext`. */
export const ROUTE_TOOL_CONTEXT = "/_console/context";
/** Request header carrying the console scope to the tool backend. */
export const SCOPE_HEADER = "x-console-scope";
/** Max length of one scope header value. */
export const MAX_SCOPE_VALUE_LEN = 64;

export interface Scope {
  site: string | null;
  tenant: string | null;
  environment: string | null;
}

/** Display identity only. `roles` is reserved and always empty in v1. */
export interface User {
  id: string;
  display_name: string;
  roles: string[];
}

export type Level = "info" | "success" | "warning" | "error";
export type FocusEdge = "first" | "last";

export type ShellToTool =
  | {
      type: "Init";
      protocol: number;
      theme: string;
      locale: string;
      scope: Scope;
      user: User;
      path?: string;
      tool_id?: string;
    }
  | { type: "ScopeChanged"; scope: Scope }
  | { type: "ThemeChanged"; theme: string }
  | { type: "Navigate"; path: string }
  | { type: "SessionRefreshed"; ok: boolean; idle_expires_at?: string }
  | { type: "Focus"; edge: FocusEdge };

export type ToolToShell =
  | { type: "Ready"; protocol: number }
  | { type: "SetTitle"; title: string }
  | { type: "SetBreadcrumbs"; items: Array<[string, string]> }
  | { type: "Notify"; level: Level; message: string }
  | { type: "SetBadge"; key: string; count: number | null }
  | { type: "NavigateConsole"; tool: string; path: string }
  | { type: "RequestSessionRefresh" }
  | { type: "PathChanged"; path: string }
  | { type: "FocusShell"; reverse: boolean };

/** `GET /_console/context` body. */
export interface ToolContext {
  tool_id: string;
  console_origin: string;
  protocol: number;
}

export type MessageErrorKind = "too_large" | "malformed";

export class MessageError extends Error {
  readonly kind: MessageErrorKind;
  constructor(kind: MessageErrorKind, detail: string) {
    super(kind === "too_large" ? `message is ${detail} bytes, over the ${MAX_MESSAGE_BYTES} byte limit` : `malformed message: ${detail}`);
    this.name = "MessageError";
    this.kind = kind;
  }
}

type Obj = { [k: string]: unknown };
type Body<T> = (o: Obj) => T;

const bad = (what: string): never => {
  throw new MessageError("malformed", what);
};
const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (o: Obj, k: string): string => {
  const v = o[k];
  return typeof v === "string" ? v : bad(`field ${k}: expected string`);
};
const optStr = (o: Obj, k: string): string | null => {
  const v = o[k];
  return v === undefined || v === null ? null : typeof v === "string" ? v : bad(`field ${k}: expected string or null`);
};
const bool = (o: Obj, k: string): boolean => {
  const v = o[k];
  return typeof v === "boolean" ? v : bad(`field ${k}: expected bool`);
};
const isU32 = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= 0xffffffff;
const u32 = (o: Obj, k: string): number => {
  const v = o[k];
  return isU32(v) ? v : bad(`field ${k}: expected u32`);
};
const obj = (o: Obj, k: string): Obj => {
  const v = o[k];
  return isObj(v) ? v : bad(`field ${k}: expected object`);
};
const oneOf = <T extends string>(o: Obj, k: string, allowed: readonly T[]): T => {
  const v = str(o, k);
  return (allowed as readonly string[]).includes(v) ? (v as T) : bad(`field ${k}: unknown variant ${v}`);
};

const scope = (o: Obj): Scope => ({
  site: optStr(o, "site"),
  tenant: optStr(o, "tenant"),
  environment: optStr(o, "environment"),
});
const user = (o: Obj): User => {
  const r = o["roles"];
  const roles = r === undefined ? [] : Array.isArray(r) && r.every((x) => typeof x === "string") ? (r as string[]) : bad("field roles: expected string[]");
  return { id: str(o, "id"), display_name: str(o, "display_name"), roles: [...roles] };
};

const SHELL_TO_TOOL: { [K in ShellToTool["type"]]: Body<Extract<ShellToTool, { type: K }>> } = {
  Init: (o) => {
    const m: Extract<ShellToTool, { type: "Init" }> = {
      type: "Init",
      protocol: u32(o, "protocol"),
      theme: str(o, "theme"),
      locale: str(o, "locale"),
      scope: scope(obj(o, "scope")),
      user: user(obj(o, "user")),
    };
    const path = optStr(o, "path");
    const toolId = optStr(o, "tool_id");
    if (path !== null) m.path = path;
    if (toolId !== null) m.tool_id = toolId;
    return m;
  },
  ScopeChanged: (o) => ({ type: "ScopeChanged", scope: scope(obj(o, "scope")) }),
  ThemeChanged: (o) => ({ type: "ThemeChanged", theme: str(o, "theme") }),
  Navigate: (o) => ({ type: "Navigate", path: str(o, "path") }),
  SessionRefreshed: (o) => {
    const m: Extract<ShellToTool, { type: "SessionRefreshed" }> = { type: "SessionRefreshed", ok: bool(o, "ok") };
    const at = optStr(o, "idle_expires_at");
    if (at !== null) m.idle_expires_at = at;
    return m;
  },
  Focus: (o) => ({ type: "Focus", edge: oneOf(o, "edge", ["first", "last"] as const) }),
};

const TOOL_TO_SHELL: { [K in ToolToShell["type"]]: Body<Extract<ToolToShell, { type: K }>> } = {
  Ready: (o) => ({ type: "Ready", protocol: u32(o, "protocol") }),
  SetTitle: (o) => ({ type: "SetTitle", title: str(o, "title") }),
  SetBreadcrumbs: (o) => {
    const items = o["items"];
    if (!Array.isArray(items)) return bad("field items: expected array");
    return {
      type: "SetBreadcrumbs",
      items: items.map((it: unknown): [string, string] =>
        Array.isArray(it) && it.length === 2 && typeof it[0] === "string" && typeof it[1] === "string"
          ? [it[0], it[1]]
          : bad("field items: expected [string, string]"),
      ),
    };
  },
  Notify: (o) => ({ type: "Notify", level: oneOf(o, "level", ["info", "success", "warning", "error"] as const), message: str(o, "message") }),
  SetBadge: (o) => {
    const c = o["count"];
    const count = c === undefined || c === null ? null : isU32(c) ? c : bad("field count: expected u32 or null");
    return { type: "SetBadge", key: str(o, "key"), count };
  },
  NavigateConsole: (o) => ({ type: "NavigateConsole", tool: str(o, "tool"), path: str(o, "path") }),
  RequestSessionRefresh: () => ({ type: "RequestSessionRefresh" }),
  PathChanged: (o) => ({ type: "PathChanged", path: str(o, "path") }),
  FocusShell: (o) => {
    const r = o["reverse"];
    return { type: "FocusShell", reverse: r === undefined ? false : typeof r === "boolean" ? r : bad("field reverse: expected bool") };
  },
};

/** UTF-8 byte length (no allocation for ASCII-sized checks beyond the cap). */
export function utf8Length(s: string): number {
  // Each UTF-16 unit encodes to at least one byte, so a long string is over the cap without encoding.
  if (s.length > MAX_MESSAGE_BYTES) return s.length * 3;
  return new TextEncoder().encode(s).length;
}

function decodeLenient<T extends { type: string }>(data: string, table: { [k: string]: Body<T> }): T | null {
  const n = utf8Length(data);
  if (n > MAX_MESSAGE_BYTES) throw new MessageError("too_large", String(n));
  let value: unknown;
  try {
    value = JSON.parse(data);
  } catch (e) {
    throw new MessageError("malformed", e instanceof Error ? e.message : "invalid JSON");
  }
  if (!isObj(value)) throw new MessageError("malformed", "message is not a JSON object");
  const t = value["type"];
  if (typeof t !== "string" || !Object.prototype.hasOwnProperty.call(table, t)) return null;
  const body = table[t];
  return body === undefined ? null : body(value);
}

/**
 * Decode a shell-to-tool message per the v1 rules: size cap, JSON parse,
 * object check, then type dispatch. Returns `null` for an unknown, missing or
 * non-string `type`; throws `MessageError` for oversize or malformed input.
 */
export function decodeShellToTool(data: string): ShellToTool | null {
  return decodeLenient<ShellToTool>(data, SHELL_TO_TOOL as { [k: string]: Body<ShellToTool> });
}

/** Decode a tool-to-shell message (same rules as `decodeShellToTool`). */
export function decodeToolToShell(data: string): ToolToShell | null {
  return decodeLenient<ToolToShell>(data, TOOL_TO_SHELL as { [k: string]: Body<ToolToShell> });
}

/** Re-validate and serialize in the Rust field order (unset optionals omitted, like serde). */
function encodeWith<T extends { type: string }>(msg: T, table: { [k: string]: Body<T> }): string {
  const body = table[msg.type];
  if (body === undefined) throw new MessageError("malformed", `unknown type ${msg.type}`);
  const s = JSON.stringify(body(msg as unknown as Obj));
  const n = utf8Length(s);
  if (n > MAX_MESSAGE_BYTES) throw new MessageError("too_large", String(n));
  return s;
}

export function encodeToolToShell(msg: ToolToShell): string {
  return encodeWith<ToolToShell>(msg, TOOL_TO_SHELL as { [k: string]: Body<ToolToShell> });
}

export function encodeShellToTool(msg: ShellToTool): string {
  return encodeWith<ShellToTool>(msg, SHELL_TO_TOOL as { [k: string]: Body<ShellToTool> });
}

/** Parse a `ToolContext` body; throws `MessageError` when it does not match. */
export function decodeToolContext(data: string): ToolContext {
  let v: unknown;
  try {
    v = JSON.parse(data);
  } catch {
    return bad("invalid JSON");
  }
  if (!isObj(v)) return bad("not a JSON object");
  return { tool_id: str(v, "tool_id"), console_origin: str(v, "console_origin"), protocol: u32(v, "protocol") };
}

function validScopeValue(v: string): boolean {
  return v.length >= 1 && v.length <= MAX_SCOPE_VALUE_LEN && /^[A-Za-z0-9._-]+$/.test(v);
}

/** Render `X-Console-Scope` (`site=a;tenant=b`); invalid values are omitted, `null` when empty. */
export function formatScopeHeader(s: Scope): string | null {
  const parts: string[] = [];
  for (const k of ["site", "tenant", "environment"] as const) {
    const v = s[k];
    if (v !== null && validScopeValue(v)) parts.push(`${k}=${v}`);
  }
  return parts.length > 0 ? parts.join(";") : null;
}

/** Truncate to `MAX_NOTIFY_CHARS` code points (Rust `chars().take(n)`). */
export function truncateNotify(message: string): string {
  const cps = Array.from(message);
  return cps.length > MAX_NOTIFY_CHARS ? cps.slice(0, MAX_NOTIFY_CHARS).join("") : message;
}
