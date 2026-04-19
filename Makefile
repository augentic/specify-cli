.PHONY: build
build:
	cargo build --release
	cp target/release/specify .

.PHONY: test
test:
	cargo test --workspace

.PHONY: fmt
fmt:
	cargo fmt --all

.PHONY: clippy
clippy:
	cargo clippy --workspace --all-targets -- -D warnings
