#!/usr/bin/env bash
# Tests the USB/IP transport against the real Linux kernel (vhci-hcd,
# uvcvideo, usbfs + the real libusb) inside a throwaway VM, so no test module
# is ever loaded on the host kernel. See docs/VM.md.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
# The CLI, and the libusb shim (the guest compares libuvc's close through it and through the kernel).
cargo build -q -p emuvc -p emuvc-libusb
# Build libuvc + stream_test (runs no case).
[ -x target/libuvc-build/tests/stream_test ] || scripts/libuvc-test.sh -k '^$' >/dev/null
exec scripts/vm-run.sh -t 180 -- "$ROOT/scripts/vm/usbip-guest.sh"
