.PHONY: quality fmt-check clippy test

quality: fmt-check clippy test

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets

test:
	cargo test --workspace
