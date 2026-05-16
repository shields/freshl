.PHONY: build test lint fmt coverage run clean

build:
	cargo build --release

test:
	cargo test --all-targets

lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	bunx prettier --check '**/*.md'

fmt:
	cargo fmt
	bunx prettier --write '**/*.md'

# cargo-llvm-cov auto-sets cfg(coverage_nightly) on nightly; passing --cfg
# explicitly is rejected. Do not add --cfg coverage_nightly here.
coverage:
	cargo +nightly llvm-cov --fail-under-lines 100

run:
	cargo run --release --

clean:
	cargo clean
