.PHONY: check fmt lint test web web-gallery web-run check-no-gallery wasm-test gateway run deny audit image e2e e2e-install e2e-typecheck up down down-clean compose-config test-integration ports-free dev-secrets tools-registries hello hello-web hello-server bench-proxy e2e-linux

check: fmt lint test web web-gallery check-no-gallery deny

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy -p console-shell --target wasm32-unknown-unknown -- -D warnings
	cargo clippy -p console-shell --target wasm32-unknown-unknown --features gallery -- -D warnings
	cargo clippy -p console-ui --target wasm32-unknown-unknown --all-targets -- -D warnings

test:
	cargo test --workspace

# Release bundle: gallery feature OFF (CM-7).
web:
	cd crates/console-shell && trunk build --release

# Dev bundle with the component gallery at /gallery. Separate output dir so
# the default dist/ never holds a gallery build (CM-7).
web-gallery:
	cd crates/console-shell && trunk build --release --features gallery --dist dist-gallery

# NIST 800-53: CM-7 - evidence that the release bundle excludes the gallery.
check-no-gallery: web
	@ls crates/console-shell/dist/*.wasm >/dev/null 2>&1 || { echo "no release wasm in dist to check"; exit 1; }
	@if grep -l "Component gallery" crates/console-shell/dist/*.wasm >/dev/null 2>&1; then \
		echo "release dist contains gallery code (CM-7)"; exit 1; \
	else echo "release dist is gallery-free"; fi

# Headless browser tests for console-ui (needs wasm-pack + the browser and
# its driver). POAM-003. Override with WASM_BROWSER=chrome (or safari).
WASM_BROWSER ?= firefox
wasm-test:
	wasm-pack test --headless --$(WASM_BROWSER) crates/console-ui
	wasm-pack test --headless --$(WASM_BROWSER) crates/console-shell
	wasm-pack test --headless --$(WASM_BROWSER) crates/console-sdk --features web

# Local stack (deploy/compose): postgres + keycloak + Caddy TLS proxy. DEV ONLY.
# CI and Linux docker: COMPOSE="docker compose" COMPOSE_FILES="-f deploy/compose/compose.yaml -f deploy/compose/compose.hostnet.yaml"
# The --env-file is added below so it applies to every COMPOSE_FILES value.
COMPOSE ?= podman compose
# Ports: single source deploy/compose/ports.env (also read by Playwright).
# Environment values override the file (`?=`; compose also prefers env over --env-file).
PORTS_ENV := deploy/compose/ports.env
PORT_VARS := $(shell sed -n 's/^\(CONSOLE_[A-Z0-9_]*\)=.*/\1/p' $(PORTS_ENV))
$(foreach v,$(PORT_VARS),$(eval $(v) ?= $(shell sed -n 's/^$(v)=//p' $(PORTS_ENV))))
export $(PORT_VARS)
COMPOSE_FILES ?= -f deploy/compose/compose.yaml
override COMPOSE_FILES := --env-file $(PORTS_ENV) $(COMPOSE_FILES)
GATEWAY_ENV := deploy/compose/gateway.env.example

# Fails fast (before compose) if a TLS port is already bound on the host.
up: ports-free dev-secrets tools-registries
	$(COMPOSE) $(COMPOSE_FILES) up -d --wait postgres keycloak tls-proxy

# Every Caddy-published TLS port (CONSOLE_TLS_PORT_* in $(PORTS_ENV)).
TLS_PORTS := $(foreach v,$(filter CONSOLE_TLS_PORT_%,$(PORT_VARS)),$($(v)))
ports-free:
	@busy=""; for p in $(TLS_PORTS); do \
		if lsof -nP -iTCP:$$p -sTCP:LISTEN 2>/dev/null | grep -qv -e '^COMMAND' -e '^$$'; then \
			if ! $(COMPOSE) $(COMPOSE_FILES) ps --format '{{.Names}}' 2>/dev/null | grep -q tls-proxy; then busy="$$busy $$p"; fi; fi; done; \
	if [ -n "$$busy" ]; then echo "host port(s) in use:$$busy. Override in env (see $(PORTS_ENV)). A stale podman gvproxy forward clears with: podman machine stop && podman machine start"; exit 1; fi

down:
	$(COMPOSE) $(COMPOSE_FILES) down

# DEV ONLY tool-assertion signing key (ES256 / P-256, PKCS#8 PEM), generated
# locally on first `make up`; never committed (secrets/ is git-ignored except
# README). Real keys come from a Secret mount (CONSOLE_ASSERTION_KEY_FILE).
DEV_SECRETS := deploy/compose/secrets
ASSERTION_KEY := $(DEV_SECRETS)/assertion.key
# DEV ONLY key for the per-tool session pseudonym (assertion `sid` =
# HMAC(key, session_ref || tool_id); SEC-005/NIST-004). Separate from the
# signing key; required by the gateway outside dev mode. 64 base64 chars, which
# the gateway decodes to 48 random bytes (it also accepts raw binary >= 32 B).
TOOL_SID_KEY := $(DEV_SECRETS)/tool-session-hmac.key
dev-secrets: $(ASSERTION_KEY) $(TOOL_SID_KEY)
$(ASSERTION_KEY):
	@umask 077 && openssl ecparam -name prime256v1 -genkey -noout \
		| openssl pkcs8 -topk8 -nocrypt -out $@ && chmod 600 $@ \
		&& echo "generated DEV ONLY assertion key $@"
$(TOOL_SID_KEY):
	@umask 077 && openssl rand -base64 48 | tr -d "\n" > $@ && chmod 600 $@ \
		&& echo "generated DEV ONLY tool session HMAC key $@"

# Render deploy/compose/tools/registry.*.toml (${CONSOLE_*} port placeholders)
# into target/e2e-tools/. Playwright renders its own copy the same way.
TOOLS_OUT := target/e2e-tools
tools-registries:
	@mkdir -p $(TOOLS_OUT) && for f in deploy/compose/tools/registry.*.toml; do \
		perl -pe 's/\$$\{(CONSOLE_[A-Z0-9_]+)\}/exists $$ENV{$$1} ? $$ENV{$$1} : die "unset $$1\n"/ge' $$f > $(TOOLS_OUT)/$$(basename $$f) || exit 1; \
	done

down-clean:
	$(COMPOSE) $(COMPOSE_FILES) down -v

compose-config:
	$(COMPOSE) $(COMPOSE_FILES) config >/dev/null && echo "compose config ok"

# Integration tests against compose postgres (superuser so #[sqlx::test] can
# create per-test databases). DEV ONLY credentials.
CONSOLE_TEST_DATABASE_URL ?= postgres://postgres:dev-only-postgres@127.0.0.1:5432/postgres?sslmode=disable
test-integration:
	CONSOLE_TEST_DATABASE_URL='$(CONSOLE_TEST_DATABASE_URL)' cargo test -p console-gateway --locked -- --nocapture

# E2E: Playwright + axe through the compose Keycloak (requires `make up`).
# Playwright's webServer starts host gateways (main/gallery, release dist
# smoke, short idle, acr-mismatch) on CONSOLE_GW_PORT_* that browsers reach
# only via Caddy TLS on CONSOLE_TLS_PORT_* ($(PORTS_ENV); WebKit drops
# __Host-/Secure cookies on http://localhost). Gateway config: $(GATEWAY_ENV).
# Not part of `make check`. Browsers: chromium, firefox, webkit (run
# `make e2e-install` once).
E2E_ARGS ?=
e2e: web web-gallery gateway hello up
	cd e2e && npm ci && npx tsc --noEmit -p . && npx playwright test $(E2E_ARGS)

# Linux engines (chromium, firefox, webkit) from the digest-pinned Playwright
# image, like CI (2 workers). Browsers run in the container as a Playwright
# run-server; the tests, gateways and DB helpers run on this host.
PLAYWRIGHT_IMAGE := mcr.microsoft.com/playwright:v1.64.1-noble@sha256:2f05f8a734509a7b3f26a912639488ee08f58a7b292d14d62271bcff9038768f
E2E_LINUX_ARGS ?= --project=chromium --project=firefox --project=webkit --project=idle-chromium --project=idle-firefox --project=idle-webkit --project=dist-chromium --project=dist-firefox --project=dist-webkit --project=ka503-chromium --project=ka503-firefox --project=ka503-webkit
e2e-linux: web web-gallery gateway hello up
	podman run -d --rm --replace --name usg-ops-console-pw -p 127.0.0.1:3000:3000 --init --ipc=host $(PLAYWRIGHT_IMAGE) \
		/bin/sh -c 'cd /tmp && npx -y playwright@1.64.1 run-server --port 3000 --host 0.0.0.0'
	@for i in $$(seq 1 90); do curl -s -o /dev/null http://127.0.0.1:3000/ && break; sleep 1; done
	cd e2e && npm ci && E2E_CONNECT_WS=ws://127.0.0.1:3000/ npx playwright test --workers=2 $(E2E_LINUX_ARGS); rc=$$?; \
		podman stop usg-ops-console-pw >/dev/null 2>&1; exit $$rc

e2e-install:
	cd e2e && npm ci && npx playwright install chromium firefox webkit

e2e-typecheck:
	cd e2e && npm ci && npx tsc --noEmit -p .

gateway:
	cargo build --release -p console-gateway

# examples/hello-tool: Yew web bundle (Trunk) + axum server (E2E target and
# template). Runs on the host like the gateways (upstream of the proxy).
hello: hello-web hello-server
hello-web:
	cd examples/hello-tool/web && trunk build --release
hello-server:
	cargo build --release -p hello-tool-server

# Proxy overhead (plan §4.12): gateway vs direct hello /api/ping at c=1, 32
# and 128, plus navigation and audited POST at c=1/32, n=5000
# (e2e/tests/proxy-bench.spec.ts; result in e2e/test-results/proxy-bench.json).
# In bench mode the main gateway logs session.lookup acquire_us at debug; the
# full log (gateway JSON lines included) is target/bench-proxy.log. Needs a
# freshly started main gateway (not one reused from another run).
bench-proxy: gateway hello up
	mkdir -p target && cd e2e && E2E_BENCH=1 npx playwright test tests/proxy-bench.spec.ts --project=chromium --reporter=list \
		> ../target/bench-proxy.log 2>&1; rc=$$?; tail -n 5 ../target/bench-proxy.log; exit $$rc

# Gateway on the host against the compose stack; browse
# https://localhost:$(CONSOLE_TLS_PORT_RUN) (Caddy internal CA).
# Sign in as console_user / dev-only-console_user.
# The hello tool is at https://localhost:$(CONSOLE_TLS_PORT_TOOL_RUN) (service
# menu); its server runs in the background and stops with the gateway.
# `make run` serves its own copy of the shell (dist-run). The gateway pins the
# CSP to index.html's inline-script hash at startup, so a rebuild of the
# shared dist-gallery by `make e2e`/`make check` while the stack runs would
# leave it serving a page its CSP blocks (blank screen). Restart `make run`
# to pick up new shell code.
web-run:
	cd crates/console-shell && trunk build --release --features gallery --dist dist-run

run: web-run hello up
	HELLO_LISTEN=127.0.0.1:$(CONSOLE_PORT_HELLO_UPSTREAM) HELLO_TOOL_IDS=hello HELLO_EVIL_ID= \
	CONSOLE_ISSUER=https://localhost:$(CONSOLE_TLS_PORT_RUN) CONSOLE_JWKS_URL=http://127.0.0.1:$(CONSOLE_GW_PORT_RUN)/.well-known/jwks.json \
	./target/release/hello-tool-server & hello=$$!; \
	trap 'kill $$hello 2>/dev/null' EXIT INT TERM; \
	set -a && . ./$(GATEWAY_ENV) && set +a && \
	CONSOLE_LISTEN=127.0.0.1:$(CONSOLE_GW_PORT_RUN) CONSOLE_PUBLIC_URL=https://localhost:$(CONSOLE_TLS_PORT_RUN) \
	CONSOLE_TOOLS_REGISTRY_FILE=$(TOOLS_OUT)/registry.run.toml CONSOLE_ASSERTION_KEY_FILE=$(ASSERTION_KEY) \
	CONSOLE_TOOL_SESSION_HMAC_KEY_FILE=$(TOOL_SID_KEY) CONSOLE_DIST=crates/console-shell/dist-run \
	cargo run -p console-gateway

deny:
	cargo deny check

audit:
	cargo audit

image:
	podman build -f deploy/Containerfile -t usg-ops-console:dev .

.PHONY: sdk-js-test
sdk-js-test:
	cd sdk-js && npm ci --ignore-scripts && npm run typecheck && npm test
