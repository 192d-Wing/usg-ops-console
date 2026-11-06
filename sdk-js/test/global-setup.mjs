// Vendors the SDK into dist/vendor-test before the tests run, so
// vendor.test.ts exercises the real script output.
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export default function setup() {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  execFileSync(process.execPath, [join(root, "scripts/vendor.mjs"), join(root, "dist/vendor-test")], { stdio: "inherit" });
}
