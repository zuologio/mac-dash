#!/usr/bin/env bash
# Cross-compile the display binary for a Raspberry Pi Zero.
#
# ARMv6 (Zero W v1) is the default because it also runs on a Zero 2 W; pass
# --zero2 for a build tuned to the newer quad-core model.
set -euo pipefail

cd "$(dirname "$0")/.."

TARGET="arm-unknown-linux-musleabihf"   # ARMv6, statically linked
LABEL="Pi Zero / Zero W (ARMv6)"

if [[ "${1:-}" == "--zero2" ]]; then
    TARGET="armv7-unknown-linux-musleabihf"
    LABEL="Pi Zero 2 W (ARMv7)"
fi

echo "Building macdash-tui for ${LABEL}"
echo "  target: ${TARGET}"

if ! command -v cross >/dev/null 2>&1; then
    echo
    echo "The 'cross' tool is required (it runs the build in a container with"
    echo "the right ARMv6 toolchain; plain rustup targets ARMv7 and the"
    echo "resulting binary faults on a Zero W v1)."
    echo
    echo "  cargo install cross --git https://github.com/cross-rs/cross"
    echo
    exit 1
fi

if ! docker info >/dev/null 2>&1; then
    echo "Docker is not running — 'cross' needs it to provide the toolchain."
    exit 1
fi

cross build --release --target "${TARGET}" -p macdash-tui

BIN="target/${TARGET}/release/macdash-tui"
echo
echo "Built ${BIN}"
ls -lh "${BIN}" | awk '{print "  size: " $5}'
file "${BIN}" 2>/dev/null | sed 's/^/  /' || true
echo
echo "Next: deploy/install-pi.sh <pi-host>"
