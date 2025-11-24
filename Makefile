all: target/release/libcuckoo-clock.rlib

target/release/libcuckoo-clock.rlib:
	cargo build --release

.PHONY: dev
dev:
	cargo build

.PHONY: fmt
fmt:
	cargo fmt

.PHONY: lint
lint:
	cargo clippy

.PHONY: test
test:
	cargo test

.PHONY: bench
bench:
	cargo bench

.PHONY: fuzz-filter
fuzz-filter:
	cargo +nightly fuzz run filter

clean:
	cargo clean
