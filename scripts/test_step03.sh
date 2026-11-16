#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo test --manifest-path tools/pmm-extractor/Cargo.toml --target x86_64-unknown-linux-gnu
cargo run --manifest-path tools/pmm-extractor/Cargo.toml --target x86_64-unknown-linux-gnu -- audit-xml majora_code/mm/assets/xml
cargo run --manifest-path tools/pmm-extractor/Cargo.toml --target x86_64-unknown-linux-gnu -- build-synthetic-fixture --out target/test_synthetic.pak
