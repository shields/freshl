# Copyright © 2026 Michael Shields
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

.PHONY: build install test lint fmt coverage run clean

build:
	cargo build --release

install:
	cargo install --path .

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
