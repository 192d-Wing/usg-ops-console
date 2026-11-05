// Build screenshots/index.html: before | after side by side, PNGs inlined as
// data: URIs (no external assets). Usage: node make-index.mjs <screenshots-dir>
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const dir = process.argv[2];
if (!dir) throw new Error("usage: node make-index.mjs <dir>");
const uri = (p) => (existsSync(p) ? `data:image/png;base64,${readFileSync(p).toString("base64")}` : null);
const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const after = readdirSync(join(dir, "after")).filter((f) => f.endsWith(".png")).sort();
const order = (f) => ["home", "tool", "palette", "rail"].findIndex((k) => f.startsWith(k));
after.sort((a, b) => order(a) - order(b) || a.localeCompare(b));
const rows = after.map((f) => {
  const b = uri(join(dir, "before", f));
  const a = uri(join(dir, "after", f));
  const cell = (src, label) => (src ? `<figure><img src="${src}" alt="${esc(label)}"><figcaption>${esc(label)}</figcaption></figure>` : `<figure class="na"><p>n/a (new in M4.5)</p><figcaption>${esc(label)}</figcaption></figure>`);
  return `<section><h2>${esc(f.replace(/\.png$/, ""))}</h2><div class="pair">${cell(b, `before (0ace10b): ${f}`)}${cell(a, `after (working tree): ${f}`)}</div></section>`;
});
writeFileSync(join(dir, "index.html"), `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>M4.5 before / after</title>
<style>body{font:14px system-ui,sans-serif;margin:24px;background:#f7f7f8;color:#18181b}h1{font-size:20px}h2{font-size:15px;margin:24px 0 8px}
.pair{display:grid;grid-template-columns:1fr 1fr;gap:16px;align-items:start}figure{margin:0;background:#fff;border:1px solid #e4e4e7;border-radius:8px;padding:8px}
img{max-width:100%;height:auto;display:block;border:1px solid #e4e4e7}figcaption{color:#55555f;font-size:12px;margin-top:6px}.na p{color:#55555f;padding:24px;text-align:center}</style></head>
<body><h1>M4.5 "calm neutral" redesign: before (0ace10b) vs after (working tree)</h1><p>Chromium, 1280x800 and 320x640, light and dark. Palette and collapsed rail are after-only.</p>
${rows.join("\n")}</body></html>\n`);
console.log(`index.html: ${after.length} rows`);
