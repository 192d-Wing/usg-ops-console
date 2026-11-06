import { expect } from "vitest";
import { MAX_MESSAGE_BYTES } from "../src/index.js";
import type { ShellToTool, ToolToShell } from "../src/index.js";

// Loads the shared protocol v1 fixture corpus (crates/console-protocol/tests/fixtures).
const raw = import.meta.glob("../../crates/console-protocol/tests/fixtures/**/*", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const PREFIX = "../../crates/console-protocol/tests/fixtures/";

export const files: Map<string, string> = new Map(
  Object.entries(raw).map(([k, v]) => [k.slice(PREFIX.length), v]),
);

/** Fixture input: contents with trailing space/tab/CR/LF removed (README "Reading a fixture"). */
export function input(file: string): string {
  const v = files.get(file);
  if (v === undefined) throw new Error(`fixture ${file} not found`);
  return v.replace(/[ \t\r\n]+$/, "");
}

export interface Entry {
  file: string;
  kind: "sdk-message" | "http-json";
  direction: string;
  type: string | null;
  case: string;
  valid: boolean;
  expect: "decode" | "ignore" | "malformed" | "too_large";
  canonical?: string;
  description: string;
}

export interface Manifest {
  format: string;
  format_version: number;
  protocol: number;
  limits: { max_message_bytes: number; max_notify_chars: number };
  fixtures: Entry[];
}

export const manifest = JSON.parse(input("MANIFEST.json")) as Manifest;

export type Decoded = ShellToTool | ToolToShell | null;
export interface Codec {
  decode(s: string): Decoded;
  encode(m: ShellToTool | ToolToShell): string;
}

export function errorKind(f: () => unknown): string {
  try {
    f();
  } catch (e) {
    // Duck-typed so a second copy of the SDK (the vendored bundle) is judged the same way.
    if (e instanceof Error && e.name === "MessageError" && "kind" in e) return String(e.kind);
    return `other:${String(e)}`;
  }
  return "none";
}

/** Asserts one manifest entry against a codec (same expectations as protocol_v1.rs). */
export function checkEntry(e: Entry, c: Codec): void {
  const data = input(e.file);
  switch (e.expect) {
    case "decode": {
      const m = c.decode(data);
      expect(m, e.file).not.toBeNull();
      expect(m?.type, e.file).toBe(e.type);
      const want: unknown = JSON.parse(input(e.canonical ?? e.file));
      expect(JSON.parse(c.encode(m as ShellToTool | ToolToShell)), e.file).toStrictEqual(want);
      break;
    }
    case "ignore":
      expect(c.decode(data), e.file).toBeNull();
      break;
    case "malformed":
      expect(errorKind(() => c.decode(data)), e.file).toBe("malformed");
      break;
    case "too_large":
      expect(new TextEncoder().encode(data).length).toBeGreaterThan(MAX_MESSAGE_BYTES);
      expect(errorKind(() => c.decode(data)), e.file).toBe("too_large");
      break;
  }
}

