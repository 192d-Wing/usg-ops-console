#!/usr/bin/env node
// Usage: node scripts/vendor.mjs <dest-dir>
// Writes <dest>/console-sdk.ts (single file, header with version, protocol,
// git source and sha256), <dest>/check.mjs (+ check.d.mts) to verify it, and
// <dest>/fixtures/ (the shared protocol v1 corpus) for the consumer's
// compat test. Existing files in <dest> written by this script are replaced.
import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { BUNDLE_FILES, bundle } from "./bundle.mjs";
import { verifyVendoredSdk } from "./check.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const repo = resolve(root, "..");
const fixtures = join(repo, "crates/console-protocol/tests/fixtures");

const dest = process.argv[2];
if (!dest) {
  console.error("usage: node scripts/vendor.mjs <dest-dir>");
  process.exit(2);
}

function gitSource() {
  try {
    const sha = execFileSync("git", ["-C", repo, "rev-parse", "--short=12", "HEAD"], { encoding: "utf8" }).trim();
    const dirty = execFileSync(
      "git",
      ["-C", repo, "status", "--porcelain", "--", "sdk-js/src", "crates/console-protocol/tests/fixtures"],
      { encoding: "utf8" },
    ).trim();
    return dirty ? `${sha}+dirty` : sha;
  } catch {
    return "unknown";
  }
}

const src = Object.fromEntries(BUNDLE_FILES.map((f) => [f, readFileSync(join(root, "src", f), "utf8")]));
const version = /SDK_VERSION = "([^"]+)"/.exec(src["version.ts"])?.[1];
const protocol = Number(/PROTOCOL_VERSION = (\d+)/.exec(src["version.ts"])?.[1]);
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
if (!version || version !== pkg.version || !Number.isInteger(protocol)) {
  console.error(`vendor: SDK_VERSION (${version}) must equal package.json version (${pkg.version})`);
  process.exit(1);
}
const manifest = JSON.parse(readFileSync(join(fixtures, "MANIFEST.json"), "utf8"));
if (manifest.protocol !== protocol) {
  console.error(`vendor: fixtures pin protocol ${manifest.protocol}, SDK speaks ${protocol}`);
  process.exit(1);
}

const out = await bundle(src, { version, protocol, source: gitSource() });
const check = await verifyVendoredSdk(out, { protocol, version, allowDirty: true });
if (!check.ok) {
  console.error(`vendor: self-check failed: ${check.error}`);
  process.exit(1);
}

const d = resolve(dest);
mkdirSync(d, { recursive: true });
writeFileSync(join(d, "console-sdk.ts"), out);
writeFileSync(join(d, "check.mjs"), readFileSync(join(here, "check.mjs"), "utf8"));
writeFileSync(join(d, "check.d.mts"), readFileSync(join(here, "check.d.mts"), "utf8"));
rmSync(join(d, "fixtures"), { recursive: true, force: true });
cpSync(fixtures, join(d, "fixtures"), { recursive: true });
if (check.source === "unknown" || check.source.endsWith("+dirty"))
  console.warn(
    `vendor: WARNING source=${check.source} is not a clean commit; verifyVendoredSdk rejects it unless {allowDirty:true}. Commit sdk-js and the fixtures, then re-vendor.`,
  );
console.log(`vendored sdk-js ${version} protocol=${protocol} source=${check.source} sha256=${check.sha256} -> ${d}`);
