#!/bin/sh
# Runs as root inside the throwaway VM (scripts/vm-usbip-test.sh): the real
# kernel (vhci-hcd, uvcvideo, usbfs) and the real libusb against `emuvc usbip`.
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
EMUVC="$ROOT/target/debug/emuvc"
STREAM_TEST="$ROOT/target/libuvc-build/tests/stream_test"
fail=0
step() { echo; echo "== $*"; }
check() {
  if "$@"; then echo "PASS: $*"; else echo "FAIL (rc=$?): $*"; fail=1; fi
}

modprobe vhci-hcd && modprobe uvcvideo || exit 2
EMUVC_LOG="${EMUVC_LOG:-info}" "$EMUVC" usbip --listen 127.0.0.1:3240 >/tmp/usbip.log 2>&1 &
for i in 1 2 3 4 5 6 7 8 9 10; do
  "$EMUVC" usbip-attach --server 127.0.0.1:3240 1-1 && break
  sleep 0.2
done
for i in $(seq 100); do [ -e /dev/video0 ] && break; sleep 0.1; done

step "kernel enumeration"
check lsusb -d 30c9:0052
dmesg | grep -E 'vhci|usb [0-9]+-[0-9]|uvcvideo' | tail -15

step "uvcvideo (V4L2)"
v4l2-ctl --list-devices
check v4l2-ctl -d /dev/video0 --set-fmt-video=width=640,height=480,pixelformat=YUYV \
  --stream-mmap=4 --stream-count=30 --stream-to=/tmp/frames.yuyv
check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/frames.yuyv
check v4l2-ctl -d /dev/video0 -c brightness=200
v4l2-ctl -d /dev/video0 -C brightness

step "libuvc through the real libusb (detaches uvcvideo)"
check "$STREAM_TEST" -f yuyv -s 640x480 -r 30 -n 30 -t 20
check "$STREAM_TEST" -f mjpeg -s 1280x720 -r 30 -n 30 -t 20
check "$STREAM_TEST" --controls

step "SuperSpeed synthetic camera (vhci SuperSpeed port)"
EMUVC_LOG="${EMUVC_LOG:-info}" "$EMUVC" usbip --listen 127.0.0.1:3241 \
  -c "$ROOT/tests/libuvc/configs/synthetic-iso-ss.toml" >/tmp/usbip-ss.log 2>&1 &
for i in 1 2 3 4 5 6 7 8 9 10; do
  "$EMUVC" usbip-attach --server 127.0.0.1:3241 1-1 && break
  sleep 0.2
done
ssdev=
for i in $(seq 100); do
  for n in /sys/class/video4linux/video*; do
    grep -q Synthetic "$n/name" 2>/dev/null && [ -z "$ssdev" ] && ssdev=/dev/$(basename "$n")
  done
  [ -n "$ssdev" ] && break
  sleep 0.1
done
lsusb -t | grep -i -B1 -A2 "5000M" | head -8
check test -n "$ssdev"
check v4l2-ctl -d "$ssdev" --set-fmt-video=width=640,height=480,pixelformat=YUYV \
  --stream-mmap=4 --stream-count=30 --stream-to=/tmp/ss.yuyv
check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/ss.yuyv
tail -n 4 /tmp/usbip-ss.log

step "emuvc log (tail)"
tail -n 12 /tmp/usbip.log
dmesg | grep -iE 'error|fail|warn' | grep -v -iE 'acpi|firmware|pci|i8042|rtc' | tail -10
echo
[ $fail = 0 ] && echo "ALL PASSED" || echo "SOME CHECKS FAILED"
exit $fail
