.PHONY: check fmt lint test web web-gallery check-no-gallery wasm-test gateway run deny audit image e2e e2e-install e2e-typecheck

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

# E2E: Playwright + axe against console-gateway serving dist-gallery on
# 127.0.0.1:8081, plus a release `dist` smoke run (dist-* projects) on
# 127.0.0.1:8082; both gateways are started/stopped by Playwright's webServer.
# Not part of `make check`. Browsers: chromium, firefox, webkit (run
# `make e2e-install` once).
E2E_ARGS ?=
e2e: web web-gallery gateway
	cd e2e && npm ci && npx tsc --noEmit -p . && npx playwright test $(E2E_ARGS)

e2e-install:
	cd e2e && npm ci && npx playwright install chromium firefox webkit

e2e-typecheck:
	cd e2e && npm ci && npx tsc --noEmit -p .

gateway:
	cargo build --release -p console-gateway

run: web-gallery
	CONSOLE_LISTEN=127.0.0.1:8080 CONSOLE_DIST=crates/console-shell/dist-gallery cargo run -p console-gateway

deny:
	cargo deny check

audit:
	cargo audit

image:
	podman build -f deploy/Containerfile -t usg-ops-console:dev .
