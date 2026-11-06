#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -f .env ]]; then set -a; source .env; set +a; fi
cargo build --release --manifest-path cli/Cargo.toml
exec cargo run --release --manifest-path web/Cargo.toml
