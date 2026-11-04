.PHONY: check fmt lint test web web-gallery check-no-gallery wasm-test gateway run deny audit image e2e e2e-install e2e-typecheck up down down-clean compose-config test-integration ports-free

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

# Local stack (deploy/compose): postgres + keycloak + Caddy TLS proxy. DEV ONLY.
# CI and Linux docker: COMPOSE="docker compose" COMPOSE_FILES="-f deploy/compose/compose.yaml -f deploy/compose/compose.hostnet.yaml"
# The --env-file is added below so it applies to every COMPOSE_FILES value.
COMPOSE ?= podman compose
# Ports: single source deploy/compose/ports.env (also read by Playwright).
# Environment values override the file (`?=`; compose also prefers env over --env-file).
PORTS_ENV := deploy/compose/ports.env
PORT_VARS := $(shell sed -n 's/^\(CONSOLE_[A-Z_]*\)=.*/\1/p' $(PORTS_ENV))
$(foreach v,$(PORT_VARS),$(eval $(v) ?= $(shell sed -n 's/^$(v)=//p' $(PORTS_ENV))))
export CONSOLE_TLS_PORT_RUN CONSOLE_GW_PORT_RUN CONSOLE_TLS_PORT_MAIN CONSOLE_GW_PORT_MAIN \
	CONSOLE_TLS_PORT_DIST CONSOLE_GW_PORT_DIST CONSOLE_TLS_PORT_IDLE CONSOLE_GW_PORT_IDLE \
	CONSOLE_TLS_PORT_ACR CONSOLE_GW_PORT_ACR CONSOLE_GW_PORT_RL CONSOLE_GW_PORT_RL_UNTRUSTED
COMPOSE_FILES ?= -f deploy/compose/compose.yaml
override COMPOSE_FILES := --env-file $(PORTS_ENV) $(COMPOSE_FILES)
GATEWAY_ENV := deploy/compose/gateway.env.example

# Fails fast (before compose) if a TLS port is already bound on the host.
up: ports-free
	$(COMPOSE) $(COMPOSE_FILES) up -d --wait postgres keycloak tls-proxy

ports-free:
	@busy=""; for p in $(CONSOLE_TLS_PORT_RUN) $(CONSOLE_TLS_PORT_MAIN) $(CONSOLE_TLS_PORT_DIST) $(CONSOLE_TLS_PORT_IDLE) $(CONSOLE_TLS_PORT_ACR); do \
		if lsof -nP -iTCP:$$p -sTCP:LISTEN 2>/dev/null | grep -qv -e '^COMMAND' -e '^$$'; then \
			if ! $(COMPOSE) $(COMPOSE_FILES) ps --format '{{.Names}}' 2>/dev/null | grep -q tls-proxy; then busy="$$busy $$p"; fi; fi; done; \
	if [ -n "$$busy" ]; then echo "host port(s) in use:$$busy. Override in env (see $(PORTS_ENV)). A stale podman gvproxy forward clears with: podman machine stop && podman machine start"; exit 1; fi

down:
	$(COMPOSE) $(COMPOSE_FILES) down

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
e2e: web web-gallery gateway up
	cd e2e && npm ci && npx tsc --noEmit -p . && npx playwright test $(E2E_ARGS)

e2e-install:
	cd e2e && npm ci && npx playwright install chromium firefox webkit

e2e-typecheck:
	cd e2e && npm ci && npx tsc --noEmit -p .

gateway:
	cargo build --release -p console-gateway

# Gateway on the host against the compose stack; browse
# https://localhost:$(CONSOLE_TLS_PORT_RUN) (Caddy internal CA).
# Sign in as console_user / dev-only-console_user.
run: web-gallery up
	set -a && . ./$(GATEWAY_ENV) && set +a && \
	CONSOLE_LISTEN=127.0.0.1:$(CONSOLE_GW_PORT_RUN) CONSOLE_PUBLIC_URL=https://localhost:$(CONSOLE_TLS_PORT_RUN) \
	cargo run -p console-gateway

deny:
	cargo deny check

audit:
	cargo audit

image:
	podman build -f deploy/Containerfile -t usg-ops-console:dev .
