#!/usr/bin/env node
// M5 (plan U6) LOCAL E2E ONLY: runs usg-dcim (finch + otter-go) as the `dcim`
// tool upstream of the MAIN E2E gateway. Started by e2e/playwright.config.ts
// only when DCIM_REPO points at a usg-dcim checkout (branch feat/console-embed);
// without DCIM_REPO nothing here runs and the suite is unchanged.
//
//   1. dcim Postgres (TimescaleDB, digest-pinned) in its own podman container on
//      127.0.0.1:${CONSOLE_PORT_DCIM_PG}; tmpfs data, separate from the console
//      compose Postgres (brief Q9).
//   2. otter-go-migrate up, db/dev/console_role_mappings.sql (DEV ONLY), and the
//      E2E fabrics below (slugs = console demo sites, plan Q1).
//   3. otter-go on 127.0.0.1:${CONSOLE_PORT_DCIM_API}, OTTER_AUTH_MODE=console,
//      JWKS from the main gateway (direct http), iss = console origin, required
//      acr = the dev password acr (DEV ONLY value).
//   4. finch `VITE_BASE=/t/dcim/ npm run build`, served by nginx (digest-pinned image; nginx is not
//      installed on the host) with finch's nginx.local.conf on
//      127.0.0.1:${CONSOLE_PORT_DCIM_UPSTREAM}.
//   5. Ready when GET <upstream>/api/v1/auth/methods answers through nginx ->
//      otter-go. SIGTERM/SIGINT/exit tear everything down.
//
// E2E-only addition to the rendered nginx config: /api/ requests' assertion
// header is written to target/e2e-dcim/log/assertions.log so
// dcim-security.spec.ts can replay a consumed assertion against otter-go
// directly (jti replay cache, plan Q8). Dev-only throwaway keys and a
// throwaway container; never use this config outside E2E.
//
// Env (set by playwright.config.ts from deploy/compose/ports.env):
//   DCIM_REPO, CONSOLE_PORT_DCIM_PG, CONSOLE_PORT_DCIM_API,
//   CONSOLE_PORT_DCIM_UPSTREAM, DCIM_CONSOLE_ISSUER, DCIM_CONSOLE_JWKS_URL,
//   DCIM_REQUIRED_ACR. Optional: DCIM_SKIP_FINCH_BUILD=1 (reuse finch dist/).

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { platform } from "node:os";
import { resolve } from "node:path";

// timescale/timescaledb:latest-pg16 (linux/arm64+amd64 index digest, resolved 2026-11-05).
// usg-dcim's own compose uses this tag; migration 00046 needs the extension.
export const PG_IMAGE =
  "docker.io/timescale/timescaledb:latest-pg16@sha256:ff69a69158396461c2aea07ca565791f68762953c265db9e6ef06b627500182a";
// nginx:1.29-alpine (multi-arch index digest, resolved 2026-11-05).
export const NGINX_IMAGE = "docker.io/library/nginx:1.29-alpine@sha256:7dd09a6c4f8cab9a2d2cb98fb39790f220e8bc2ea106b2cebde64b90405e0be8";

const PG_NAME = "usg-console-e2e-dcim-pg";
const NGINX_NAME = "usg-console-e2e-dcim-nginx";
/** DEV ONLY throwaway credentials for the tmpfs E2E database. */
const PG_USER = "dcim";
const PG_PASSWORD = "dev-only-dcim";
const PG_DB = "dcim";

/** E2E fabrics: slugs match the console demo sites (crates/console-shell/src/demo.rs). */
export const DCIM_FABRICS = [
  { id: "0d1e2f30-0000-4000-8000-0000000000a1", slug: "hq", name: "E2E HQ Fabric" },
  { id: "0d1e2f30-0000-4000-8000-0000000000a2", slug: "east", name: "E2E East Fabric" },
];

const REPO = resolve(import.meta.dirname, "../..");
const OUT = resolve(REPO, "target/e2e-dcim");
const LOG_DIR = resolve(OUT, "log");

function need(name) {
  const v = process.env[name];
  if (!v) throw new Error(`dcim-stack: ${name} is required`);
  return v;
}

const log = (...a) => console.log("[dcim-stack]", ...a);

function run(cmd, args, opts = {}) {
  log("$", cmd, args.join(" "));
  const r = spawnSync(cmd, args, { stdio: ["pipe", "inherit", "inherit"], ...opts });
  if (r.status !== 0) throw new Error(`${cmd} ${args[0] ?? ""} exited ${r.status}`);
  return r;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitFor(what, fn, timeoutMs) {
  const end = Date.now() + timeoutMs;
  for (;;) {
    try {
      if (await fn()) return;
    } catch {
      /* not yet */
    }
    if (Date.now() > end) throw new Error(`dcim-stack: timed out waiting for ${what}`);
    await sleep(500);
  }
}

let otter = null;
let torndown = false;
function teardown() {
  if (torndown) return;
  torndown = true;
  log("teardown");
  if (otter && otter.exitCode === null) otter.kill("SIGTERM");
  spawnSync("podman", ["rm", "-f", "-t", "2", NGINX_NAME, PG_NAME], { stdio: "ignore" });
}

async function main() {
  const dcim = resolve(need("DCIM_REPO"));
  const otterDir = `${dcim}/packages/otter-go`;
  const finchDir = `${dcim}/packages/finch`;
  if (!existsSync(otterDir) || !existsSync(`${finchDir}/nginx.local.conf`)) {
    throw new Error(`dcim-stack: ${dcim} is not a usg-dcim checkout with feat/console-embed (nginx.local.conf missing)`);
  }
  const pgPort = need("CONSOLE_PORT_DCIM_PG");
  const apiPort = need("CONSOLE_PORT_DCIM_API");
  const upPort = need("CONSOLE_PORT_DCIM_UPSTREAM");
  const issuer = need("DCIM_CONSOLE_ISSUER");
  const jwks = need("DCIM_CONSOLE_JWKS_URL");
  const acr = need("DCIM_REQUIRED_ACR");
  const darwin = platform() === "darwin";

  mkdirSync(`${OUT}/bin`, { recursive: true });
  // Playwright stops webServers with SIGKILL on the process group, so no
  // handler here runs: a detached watchdog removes the containers once this
  // process is gone (otter-go dies with the group).
  spawn(
    process.execPath,
    ["-e", `const p=${process.pid};setInterval(()=>{try{process.kill(p,0)}catch{require("node:child_process").spawnSync("podman",["rm","-f","-t","2","${NGINX_NAME}","${PG_NAME}"],{stdio:"ignore"});process.exit(0)}},1000)`],
    { detached: true, stdio: "ignore" },
  ).unref();
  rmSync(LOG_DIR, { recursive: true, force: true });
  mkdirSync(LOG_DIR, { recursive: true });
  spawnSync("podman", ["rm", "-f", "-t", "1", NGINX_NAME, PG_NAME], { stdio: "ignore" });

  // 1. Postgres (throwaway, tmpfs).
  run("podman", [
    "run", "-d", "--name", PG_NAME,
    "-p", `127.0.0.1:${pgPort}:5432`,
    "--tmpfs", "/var/lib/postgresql/data",
    "-e", `POSTGRES_USER=${PG_USER}`, "-e", `POSTGRES_PASSWORD=${PG_PASSWORD}`, "-e", `POSTGRES_DB=${PG_DB}`,
    PG_IMAGE,
  ]);

  // Build while Postgres starts.
  run("go", ["build", "-o", `${OUT}/bin/otter-go`, "./cmd/otter-go"], { cwd: otterDir });
  run("go", ["build", "-o", `${OUT}/bin/otter-go-migrate`, "./cmd/otter-go-migrate"], { cwd: otterDir });
  // Console build: finch is served under /t/dcim/ on the tool origin (proxy
  // contract), so its base path is set at build time (finch D1 fix).
  const builtForConsole = () =>
    existsSync(`${finchDir}/dist/index.html`) && readFileSync(`${finchDir}/dist/index.html`, "utf8").includes("/t/dcim/assets/");
  if (process.env.DCIM_SKIP_FINCH_BUILD !== "1" || !builtForConsole()) {
    if (!existsSync(`${finchDir}/node_modules`)) run("npm", ["ci"], { cwd: finchDir });
    run("npm", ["run", "build"], { cwd: finchDir, env: { ...process.env, VITE_BASE: "/t/dcim/" } });
  }
  if (!builtForConsole()) throw new Error("dcim-stack: finch dist/index.html does not reference /t/dcim/assets/ (VITE_BASE build)");

  const psql = (sql, file) =>
    run("podman", ["exec", "-i", PG_NAME, "psql", "-q", "-v", "ON_ERROR_STOP=1", "-U", PG_USER, "-d", PG_DB, ...(file ? [] : ["-c", sql])], {
      input: file ? readFileSync(file) : undefined,
    });
  await waitFor(
    "postgres",
    () => spawnSync("podman", ["exec", PG_NAME, "pg_isready", "-h", "127.0.0.1", "-U", PG_USER, "-d", PG_DB]).status === 0,
    90_000,
  );
  // The image's init restarts the server once; make sure a real query works.
  await waitFor("postgres query", () => spawnSync("podman", ["exec", PG_NAME, "psql", "-h", "127.0.0.1", "-U", PG_USER, "-d", PG_DB, "-c", "select 1"]).status === 0, 60_000);

  // 2. Migrations + dev seed.
  const dsn = `postgres://${PG_USER}:${PG_PASSWORD}@127.0.0.1:${pgPort}/${PG_DB}?sslmode=disable`;
  run(`${OUT}/bin/otter-go-migrate`, ["-cmd", "up"], { env: { ...process.env, DCIM_POSTGRES_DSN: dsn } });
  psql(null, `${otterDir}/db/dev/console_role_mappings.sql`);
  const values = DCIM_FABRICS.map((f) => `('${f.id}', '${f.name}', '${f.slug}', 'E2E fixture (dev only)')`).join(", ");
  psql(`INSERT INTO fabrics (id, name, slug, description) VALUES ${values} ON CONFLICT (slug) DO NOTHING;`);

  // 3. otter-go, console mode only (standalone login refused).
  otter = spawn(`${OUT}/bin/otter-go`, [], {
    stdio: ["ignore", "inherit", "inherit"],
    env: {
      ...process.env,
      OTTER_GO_ADDR: `127.0.0.1:${apiPort}`,
      DCIM_POSTGRES_DSN_RAW: dsn,
      DCIM_POSTGRES_DSN: dsn,
      OTTER_AUTH_MODE: "console",
      OTTER_CONSOLE_JWKS_URL: jwks,
      OTTER_CONSOLE_ISSUER: issuer,
      OTTER_CONSOLE_TOOL_ID: "dcim",
      OTTER_CONSOLE_REQUIRED_ACR: acr,
      // DEV ONLY: otter-go refuses a ":dev:" acr without this flag (NIST-007).
      OTTER_CONSOLE_DEV_ACR_ALLOWED: "true",
    },
  });
  otter.on("exit", (code) => {
    log(`otter-go exited (${code})`);
    if (!torndown) {
      teardown();
      process.exit(1);
    }
  });
  await waitFor("otter-go /healthz", async () => (await fetch(`http://127.0.0.1:${apiPort}/healthz`)).ok, 60_000);

  // 4. finch behind nginx (container). macOS podman runs in a VM, so publish
  // the port and reach host otter-go via host.containers.internal (gvproxy ->
  // host loopback); Linux uses the host network and the config unchanged.
  let conf = readFileSync(`${finchDir}/nginx.local.conf`, "utf8").replaceAll("FINCH_DIST", "/dist");
  if (darwin) {
    conf = conf
      .replace(`listen 127.0.0.1:${upPort};`, `listen 0.0.0.0:${upPort};`)
      .replace(`proxy_pass http://127.0.0.1:${apiPort};`, `proxy_pass http://host.containers.internal:${apiPort};`);
  }
  if (!conf.includes(`:${upPort};`) || !conf.includes(`:${apiPort};`)) {
    throw new Error("dcim-stack: nginx.local.conf ports do not match ports.env (CONSOLE_PORT_DCIM_UPSTREAM / _API)");
  }
  // E2E ONLY: record assertion headers for the replay test (see header).
  conf = conf
    .replace("http {", 'http {\n    log_format e2e_assertion "$http_x_console_assertion";')
    .replace("location /api/ {", "location /api/ {\n            access_log /e2e-log/assertions.log e2e_assertion;");
  writeFileSync(`${OUT}/nginx.conf`, conf);
  run("podman", [
    "run", "-d", "--name", NGINX_NAME,
    ...(darwin ? ["-p", `127.0.0.1:${upPort}:${upPort}`, "--add-host", "host.containers.internal:host-gateway"] : ["--network", "host"]),
    "-v", `${finchDir}/dist:/dist:ro`,
    "-v", `${OUT}/nginx.conf:/etc/nginx/nginx.conf:ro`,
    "-v", `${LOG_DIR}:/e2e-log`,
    NGINX_IMAGE,
  ]);
  await waitFor(
    "finch via nginx -> otter-go",
    async () => (await fetch(`http://127.0.0.1:${upPort}/api/v1/auth/methods`)).status < 500 &&
      (await fetch(`http://127.0.0.1:${upPort}/`)).ok,
    60_000,
  );
  log(`ready: finch http://127.0.0.1:${upPort}  otter-go http://127.0.0.1:${apiPort}  pg 127.0.0.1:${pgPort}`);
}

for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(sig, () => {
    teardown();
    process.exit(0);
  });
}
process.on("exit", teardown);

main().catch((e) => {
  console.error("[dcim-stack]", e instanceof Error ? e.message : e);
  teardown();
  process.exit(1);
});
// Stay alive until Playwright stops us.
setInterval(() => {}, 1 << 30);
