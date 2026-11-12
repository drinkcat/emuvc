#!/usr/bin/env bash
# Tests the USB/IP transport against the real Linux kernel (vhci-hcd,
# uvcvideo, usbfs + the real libusb) inside a throwaway VM, so no test module
# is ever loaded on the host kernel. See docs/VM.md.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
cargo build -q -p emuvc
# Build libuvc + stream_test (runs no case).
[ -x target/libuvc-test/stream_test ] || scripts/libuvc-test.sh -k '^$' >/dev/null
exec scripts/vm-run.sh -t 180 -- "$ROOT/scripts/vm/usbip-guest.sh"
