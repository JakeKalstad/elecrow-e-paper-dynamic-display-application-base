#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../firmware"
if [[ -f ../.env ]]; then set -a; source ../.env; set +a; fi
source "$HOME/export-esp.sh"
cargo build --release
espflash flash --partition-table partitions.csv --port "${EPAPER_PORT:-/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0}" target/xtensa-esp32s3-espidf/release/crowpanel-firmware
