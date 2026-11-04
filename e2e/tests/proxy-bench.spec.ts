import { createHash, createPrivateKey, createPublicKey, randomBytes, sign } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import http from "node:http";
import { expect, test } from "@playwright/test";
import { ORIGINS } from "../playwright.config";
import { toolSessionFor } from "./tool-helpers";

// Plan §4.12 proxy overhead benchmark (PERF). Opt-in: E2E_BENCH=1
// (`make bench-proxy`). Measures GET /t/hello/api/ping:
//   direct   hello server over loopback http with a locally signed assertion
//            (DEV key, same kid as the gateway) - the upstream's own cost
//   gateway  main gateway over loopback http, Host = hello tool origin -
//            session join + authz + header rebuild + ES256 sign + proxy
// overhead = gateway - direct, at c=1, c=32 and c=128, n=5000 each (after warm-up).
// No TLS on either path so Caddy is not part of the number.
// Fix round 1: PERF-003 adds c=128 (the main gateway logs session.lookup
// acquire_us at debug when E2E_BENCH=1; see playwright.config.ts) and PERF-002
// adds a navigation case (GET /t/hello/, HTML, audited per navigation) and a
// state-changing POST case (/api/counter, audit_writes = true) at c=1/32.
const N = Number(process.env.E2E_BENCH_N ?? 5000);
const KEY = "../deploy/compose/secrets/assertion.key";

function b64u(b: Buffer | string) {
  return Buffer.from(b).toString("base64url");
}

function assertionSigner(iss: string, jwks: { keys: { kid: string; x: string }[] }) {
  const key = createPrivateKey(readFileSync(KEY));
  const jwk = createPublicKey(key).export({ format: "jwk" }) as { crv: string; x: string; y: string };
  // The gateway's kid for this key (RFC 7638 thumbprint as fallback).
  const kid =
    jwks.keys.find((k) => k.x === jwk.x)?.kid ??
    b64u(createHash("sha256").update(`{"crv":"${jwk.crv}","kty":"EC","x":"${jwk.x}","y":"${jwk.y}"}`).digest());
  let cached = { tok: "", exp: 0 };
  return () => {
    const now = Math.floor(Date.now() / 1000);
    if (cached.exp - now > 20) return cached.tok;
    const h = b64u(JSON.stringify({ alg: "ES256", typ: "console-assertion+jwt", kid }));
    const c = b64u(
      JSON.stringify({ iss, aud: "hello", sub: "bench", sid: "bench", jti: b64u(randomBytes(16)), iat: now, nbf: now, exp: now + 60, roles: ["hello:user"] }),
    );
    const sig = sign("sha256", Buffer.from(`${h}.${c}`), { key, dsaEncoding: "ieee-p1363" });
    cached = { tok: `${h}.${c}.${b64u(sig)}`, exp: now + 60 };
    return cached.tok;
  };
}

async function load(url: string, headers: () => Record<string, string>, n: number, c: number, method = "GET") {
  const agent = new http.Agent({ keepAlive: true, maxSockets: c });
  const lat: number[] = [];
  let errors = 0;
  let next = 0;
  const one = () =>
    new Promise<void>((resolve) => {
      const t0 = process.hrtime.bigint();
      const req = http.request(url, { method, agent, headers: headers() }, (res) => {
        res.resume();
        res.on("end", () => {
          if (res.statusCode !== 200) errors++;
          lat.push(Number(process.hrtime.bigint() - t0) / 1e6);
          resolve();
        });
      });
      req.on("error", () => {
        errors++;
        resolve();
      });
      req.end();
    });
  const worker = async () => {
    while (next < n) {
      next++;
      await one();
    }
  };
  const t0 = Date.now();
  await Promise.all(Array.from({ length: c }, worker));
  const wall = Date.now() - t0;
  agent.destroy();
  lat.sort((a, b) => a - b);
  const q = (p: number) => lat[Math.min(lat.length - 1, Math.floor(p * lat.length))]!;
  return { n, c, errors, p50: +q(0.5).toFixed(3), p99: +q(0.99).toFixed(3), rps: Math.round((n * 1000) / wall) };
}

test("proxy overhead p50/p99: gateway vs direct hello /api/ping", async ({ request, browserName }) => {
  test.skip(process.env.E2E_BENCH !== "1" || browserName !== "chromium", "opt-in benchmark (E2E_BENCH=1, chromium project)");
  test.setTimeout(600_000);
  const { sid } = await toolSessionFor(request, "hello");
  const toolHost = new URL(ORIGINS.hello).host;
  const jwks = await (await request.get(`${ORIGINS.mainDirect}/.well-known/jwks.json`)).json();
  const signer = assertionSigner(ORIGINS.main, jwks);
  const direct = `${ORIGINS.helloDirect}/t/hello/api/ping`;
  const viaGw = `${ORIGINS.mainDirect}/t/hello/api/ping`;
  const dh = () => ({ "x-console-assertion": signer() });
  // Caddy-equivalent forwarding headers; the gateway trusts loopback as proxy.
  const gh = () => ({ Host: toolHost, Cookie: `__Host-tool_sid=${sid}`, "X-Forwarded-Proto": "https", "X-Forwarded-For": "198.18.0.9", "Sec-Fetch-Mode": "cors" });
  // Correctness first, then warm-up.
  expect((await load(direct, dh, 50, 4)).errors, "direct errors").toBe(0);
  expect((await load(viaGw, gh, 50, 4)).errors, "gateway errors").toBe(0);
  const out: Record<string, unknown> = { n: N, at: new Date().toISOString() };
  const pair = async (key: string, dUrl: string, dH: () => Record<string, string>, gUrl: string, gH: () => Record<string, string>, c: number, method = "GET") => {
    const d = await load(dUrl, dH, N, c, method);
    const g = await load(gUrl, gH, N, c, method);
    out[key] = { direct: d, gateway: g, overhead_p50_ms: +(g.p50 - d.p50).toFixed(3), overhead_p99_ms: +(g.p99 - d.p99).toFixed(3) };
    return { d, g };
  };
  for (const c of [1, 32, 128]) {
    const { d, g } = await pair(`c${c}`, direct, dh, viaGw, gh, c);
    // c=128 exceeds the per-tool upstream concurrency cap if one is set: record
    // 503s as data rather than failing (PERF-003 is a measurement).
    if (c <= 32) expect(d.errors + g.errors, `errors at c=${c}`).toBe(0);
  }
  // PERF-002: navigation (HTML document, tool.access audit row per request).
  const nav = { Accept: "text/html", "Sec-Fetch-Mode": "navigate", "Sec-Fetch-Dest": "iframe", "Sec-Fetch-Site": "same-site" };
  for (const c of [1, 32]) {
    const { d, g } = await pair(`nav_c${c}`, `${ORIGINS.helloDirect}/t/hello/`, () => ({ ...dh(), ...nav }), `${ORIGINS.mainDirect}/t/hello/`, () => ({ ...gh(), ...nav }), c);
    expect(d.errors + g.errors, `navigation errors at c=${c}`).toBe(0);
  }
  // PERF-002: state-changing POST with audit_writes (tool.request audit row per request).
  const post = { "x-hello-csrf": "1", "Content-Length": "0" };
  for (const c of [1, 32]) {
    const { d, g } = await pair(`post_c${c}`, `${ORIGINS.helloDirect}/t/hello/api/counter`, () => ({ ...dh(), ...post }), `${ORIGINS.mainDirect}/t/hello/api/counter`, () => ({ ...gh(), ...post }), c, "POST");
    expect(d.errors + g.errors, `POST errors at c=${c}`).toBe(0);
  }
  console.log(`PROXY-BENCH ${JSON.stringify(out)}`);
  writeFileSync("test-results/proxy-bench.json", JSON.stringify(out, null, 2));
});
