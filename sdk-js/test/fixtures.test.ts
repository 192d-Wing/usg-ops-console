import { describe, expect, it } from "vitest";
import {
  MAX_MESSAGE_BYTES,
  MAX_NOTIFY_CHARS,
  MessageError,
  PROTOCOL_VERSION,
  decodeShellToTool,
  decodeToolContext,
  decodeToolToShell,
  encodeShellToTool,
  encodeToolToShell,
} from "../src/index.js";
import type { ShellToTool, ToolToShell } from "../src/index.js";
import { checkEntry, errorKind, files, input, manifest } from "./corpus.js";
import type { Codec } from "./corpus.js";

const codecs: Record<string, Codec> = {
  "shell-to-tool": { decode: decodeShellToTool, encode: (m) => encodeShellToTool(m as ShellToTool) },
  "tool-to-shell": { decode: decodeToolToShell, encode: (m) => encodeToolToShell(m as ToolToShell) },
};

describe("fixture manifest", () => {
  it("header matches the SDK constants", () => {
    expect(manifest.format).toBe("usg-ops-console.protocol-fixtures");
    expect(manifest.format_version).toBe(1);
    expect(manifest.protocol).toBe(PROTOCOL_VERSION);
    expect(manifest.limits.max_message_bytes).toBe(MAX_MESSAGE_BYTES);
    expect(manifest.limits.max_notify_chars).toBe(MAX_NOTIFY_CHARS);
  });

  it("lists every fixture file", () => {
    const listed = new Set(manifest.fixtures.flatMap((e) => (e.canonical ? [e.file, e.canonical] : [e.file])));
    const onDisk = [...files.keys()].filter((f) => /^(sdk_v1|sdk_v1_cases|tools_v1)\//.test(f));
    expect(onDisk.length).toBeGreaterThan(0);
    expect(onDisk.filter((f) => !listed.has(f))).toStrictEqual([]);
    for (const f of listed) expect(files.has(f), f).toBe(true);
  });

  it("valid flag agrees with expect", () => {
    for (const e of manifest.fixtures) {
      expect(e.valid, e.file).toBe(e.expect === "decode" || e.expect === "ignore");
    }
  });
});

describe("sdk-message fixtures", () => {
  for (const e of manifest.fixtures.filter((x) => x.kind === "sdk-message")) {
    it(`${e.direction} ${e.case} ${e.file} -> ${e.expect}`, () => {
      const c = codecs[e.direction];
      expect(c, e.direction).toBeDefined();
      if (c) checkEntry(e, c);
    });
  }
});

describe("http-json fixtures", () => {
  for (const e of manifest.fixtures.filter((x) => x.kind === "http-json")) {
    it(`${e.type} ${e.file}`, () => {
      expect(e.expect).toBe("decode");
      const want: unknown = JSON.parse(input(e.file));
      if (e.type === "ToolContext") {
        // The only HTTP body the tool-side SDK consumes; mirrored exactly.
        expect(decodeToolContext(input(e.file))).toStrictEqual(want);
      } else {
        // Shell/gateway/backend bodies are not part of the tool SDK; check they are JSON objects.
        expect(typeof want === "object" && want !== null && !Array.isArray(want)).toBe(true);
      }
    });
  }
});

describe("codec rules", () => {
  it("rejects bad ToolContext bodies", () => {
    for (const b of ["", "[]", "{}", '{"tool_id":"x","console_origin":"https://c","protocol":-1}']) {
      expect(() => decodeToolContext(b), b).toThrow(MessageError);
    }
  });

  it("encode refuses oversize output", () => {
    expect(errorKind(() => encodeToolToShell({ type: "SetTitle", title: "a".repeat(MAX_MESSAGE_BYTES) }))).toBe("too_large");
  });

  it("measures the cap in UTF-8 bytes", () => {
    const pad = MAX_MESSAGE_BYTES / 2; // 2-byte chars
    const msg = `{"type":"SetTitle","title":"${"é".repeat(pad)}"}`;
    expect(msg.length).toBeLessThan(MAX_MESSAGE_BYTES);
    expect(errorKind(() => decodeToolToShell(msg))).toBe("too_large");
  });
});
