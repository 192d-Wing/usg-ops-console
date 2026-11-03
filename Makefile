.PHONY: check fmt lint test web web-gallery check-no-gallery wasm-test gateway run deny audit image

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

# Optional: headless browser tests for console-ui (needs wasm-pack + Firefox).
wasm-test:
	wasm-pack test --headless --firefox crates/console-ui

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
