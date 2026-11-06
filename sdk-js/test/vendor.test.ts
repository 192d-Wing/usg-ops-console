import { describe, expect, it } from "vitest";
import { sha256Hex, splitHeader, verifyVendoredSdk } from "../scripts/check.mjs";
import { PROTOCOL_VERSION, SDK_VERSION } from "../src/index.js";
import { checkEntry, manifest } from "./corpus.js";
import type { Codec } from "./corpus.js";

// Written by test/global-setup.mjs (node scripts/vendor.mjs dist/vendor-test).
const raw = import.meta.glob("../dist/vendor-test/console-sdk.ts", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const mods = import.meta.glob("../dist/vendor-test/console-sdk.ts", { eager: true }) as Record<string, typeof import("../src/index.js")>;
const vendorFixtures = import.meta.glob("../dist/vendor-test/fixtures/MANIFEST.json", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const pkg = import.meta.glob("../package.json", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

const source = Object.values(raw)[0] ?? "";
const vendored = Object.values(mods)[0];

describe("vendored single-file SDK", () => {
  it("SDK_VERSION matches package.json", () => {
    expect((JSON.parse(Object.values(pkg)[0] ?? "{}") as { version?: string }).version).toBe(SDK_VERSION);
  });

  it("has a valid header and hash", async () => {
    const r = await verifyVendoredSdk(source, { protocol: PROTOCOL_VERSION, version: SDK_VERSION, allowDirty: true });
    expect(r).toMatchObject({ ok: true, protocol: 1, version: SDK_VERSION });
    expect(source.split("\n", 1)[0]).toMatch(/^\/\/ @usg-ops-console\/sdk-js \S+ protocol=1 source=\S+ sha256=[0-9a-f]{64}$/);
  });

  it("detects tampering and wrong pins", async () => {
    expect((await verifyVendoredSdk(source.replace("MAX_NOTIFY_CHARS = 500", "MAX_NOTIFY_CHARS = 501"))).ok).toBe(false);
    expect((await verifyVendoredSdk(source.slice(source.indexOf("\n") + 1))).ok).toBe(false);
    expect((await verifyVendoredSdk(source, { protocol: 2 })).ok).toBe(false);
  });

  it("rejects unknown and +dirty sources unless allowDirty", async () => {
    const { body } = splitHeader(source);
    const digest = await sha256Hex(body);
    const withSource = (src: string): string =>
      `// @usg-ops-console/sdk-js ${SDK_VERSION} protocol=1 source=${src} sha256=${digest}\n${body}`;
    for (const src of ["unknown", "0a939d0b1359+dirty"]) {
      const r = await verifyVendoredSdk(withSource(src));
      expect(r.ok, src).toBe(false);
      if (!r.ok) expect(r.error).toMatch(/not a clean commit/);
      expect(await verifyVendoredSdk(withSource(src), { allowDirty: true })).toMatchObject({ ok: true, source: src });
    }
    expect(await verifyVendoredSdk(withSource("0a939d0b1359"))).toMatchObject({ ok: true, source: "0a939d0b1359" });
  });

  it("ships the fixture corpus", () => {
    expect(Object.values(vendorFixtures)[0]).toBeDefined();
  });

  it("vendored codec passes every MANIFEST entry", () => {
    expect(vendored).toBeDefined();
    if (!vendored) return;
    const codecs: Record<string, Codec> = {
      "shell-to-tool": { decode: vendored.decodeShellToTool, encode: (m) => vendored.encodeShellToTool(m as never) },
      "tool-to-shell": { decode: vendored.decodeToolToShell, encode: (m) => vendored.encodeToolToShell(m as never) },
    };
    let n = 0;
    for (const e of manifest.fixtures.filter((x) => x.kind === "sdk-message")) {
      const c = codecs[e.direction];
      if (c) {
        checkEntry(e, c);
        n += 1;
      }
    }
    expect(n).toBe(manifest.fixtures.filter((x) => x.kind === "sdk-message").length);
  });
});
