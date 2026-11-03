.PHONY: check fmt lint test web gateway run deny audit image

check: fmt lint test web deny

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy -p console-shell --target wasm32-unknown-unknown -- -D warnings

test:
	cargo test --workspace

web:
	cd crates/console-shell && trunk build --release

gateway:
	cargo build --release -p console-gateway

run: web
	CONSOLE_LISTEN=127.0.0.1:8080 cargo run -p console-gateway

deny:
	cargo deny check

audit:
	cargo audit

image:
	podman build -f deploy/Containerfile -t usg-ops-console:dev .
