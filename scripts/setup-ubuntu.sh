#!/usr/bin/env bash
set -euo pipefail
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev python3 python3-venv python3-pip cmake ninja-build git curl libclang-dev clang libudev-dev fonts-dejavu-core unzip
if ! command -v cargo >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/crowpanel-rustup.sh
  sh /tmp/crowpanel-rustup.sh -y
fi
source "$HOME/.cargo/env"
cargo install espup --locked
cargo install espflash --locked
cargo install ldproxy --locked
espup install
printf '\nSetup complete. Run: source "$HOME/export-esp.sh"\n'
