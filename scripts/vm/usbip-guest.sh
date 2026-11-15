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
# Over USB/IP, frames can arrive in bursts. libuvc only hands its callback the newest
# completed frame, so one that completes while stream_test is still decoding the last
# one is skipped (seen at MJPEG 1280x720 on CI runners): allow a couple.
check "$STREAM_TEST" -f yuyv -s 640x480 -r 30 -n 30 -t 20 --max-gaps 2
check "$STREAM_TEST" -f mjpeg -s 1280x720 -r 30 -n 30 -t 20 --max-gaps 2
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

# Logitech C930e: the kernel takes configuration index 0, UVC 1.0 on the real camera. The -uvc15
# variant only has the UVC 1.5 configuration (encoding unit, H.264 interface). uvcvideo must stream
# YUYV either way.
port=3242
for model in logitech-046d-0843 logitech-046d-0843-uvc15; do
  step "$model"
  before=$(grep -l C930e /sys/class/video4linux/video*/name 2>/dev/null || true)
  EMUVC_LOG="${EMUVC_LOG:-info}" "$EMUVC" usbip --listen 127.0.0.1:$port -d $model \
    >/tmp/usbip-$model.log 2>&1 &
  for i in 1 2 3 4 5 6 7 8 9 10; do
    "$EMUVC" usbip-attach --server 127.0.0.1:$port 1-1 && break
    sleep 0.2
  done
  port=$((port + 1))
  cdev=
  for i in $(seq 100); do
    for n in /sys/class/video4linux/video*; do
      grep -q C930e "$n/name" 2>/dev/null && ! echo "$before" | grep -qx "$n/name" &&
        [ -z "$cdev" ] && cdev=/dev/$(basename "$n")
    done
    [ -n "$cdev" ] && break
    sleep 0.1
  done
  check test -n "$cdev"
  grep -l 0843 /sys/bus/usb/devices/*/idProduct | while read -r f; do
    echo "$(dirname "$f"): bConfigurationValue $(cat "$(dirname "$f")/bConfigurationValue")"
  done
  v4l2-ctl -d "$cdev" --list-formats
  dmesg | grep -i "C930e\|0843" | tail -n 4
  check v4l2-ctl -d "$cdev" --set-fmt-video=width=640,height=480,pixelformat=YUYV \
    --stream-mmap=4 --stream-count=30 --stream-to=/tmp/c930e.yuyv
  check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/c930e.yuyv
done

# uvcvideo as the reference for the libuvc cases that say it copes (tests/libuvc/configs).
step "Luxvisions IR function (libuvc: api-second-function)"
# uvcvideo binds both functions: video devices hang off the IR function's VideoControl interface (2).
check sh -c 'for n in /sys/class/video4linux/*; do readlink -f "$n/device"; done | grep -q "1-1:1\.2$"'

# serve CONFIG PATTERN: serves a configuration on the next port and attaches it; $dev is its first new
# video device whose name matches PATTERN (the capture one: metadata devices come after it), $srv the
# server's PID (killing it detaches the device and frees the vhci port).
port=3250
serve() {
  cfg=$1 pat=$2
  before=$(ls /sys/class/video4linux)
  EMUVC_LOG="${EMUVC_LOG:-info}" "$EMUVC" usbip --listen 127.0.0.1:$port \
    -c "$ROOT/tests/libuvc/configs/$cfg.toml" >/tmp/usbip-$cfg.log 2>&1 &
  srv=$!
  for i in 1 2 3 4 5 6 7 8 9 10; do
    "$EMUVC" usbip-attach --server 127.0.0.1:$port 1-1 && break
    sleep 0.2
  done
  port=$((port + 1))
  dev=
  for i in $(seq 100); do
    for n in $(ls /sys/class/video4linux | sort -V); do
      echo "$before" | grep -qx "$n" && continue
      grep -q "$pat" "/sys/class/video4linux/$n/name" 2>/dev/null && [ -z "$dev" ] && dev=/dev/$n
    done
    [ -n "$dev" ] && break
    sleep 0.1
  done
  check test -n "$dev"
}
unserve() { kill "$srv" 2>/dev/null; wait "$srv" 2>/dev/null; sleep 0.5; }
# yuyv CONFIG PATTERN: 30 YUYV 640x480 frames through uvcvideo, stamps checked (gaps are allowed).
yuyv() {
  step "uvcvideo: $1"
  serve "$1" "$2"
  [ -n "$dev" ] && check timeout 30 v4l2-ctl -d "$dev" --set-fmt-video=width=640,height=480,pixelformat=YUYV \
    --stream-mmap=4 --stream-count=30 --stream-to=/tmp/$1.yuyv && check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/$1.yuyv
  dmesg | grep -i "uvcvideo" | tail -n 2
  unserve
}
yuyv need-set-interface "Integrated RGB"    # needs SET_INTERFACE before class requests
yuyv probe-zero-index "Integrated RGB"      # probe reply with format/frame index 0
yuyv max-payload-huge "Integrated RGB"      # dwMaxPayloadTransferSize 0xffff0400
yuyv desc-composite-input Synthetic         # an 8-byte composite input terminal
yuyv desc-bcduvc-0101 Synthetic             # bcdUVC 0x0101
yuyv isight Synthetic                       # the built-in iSight's packet format
yuyv quickcam-vendor-class Synthetic        # vendor-specific interface classes, by VID/PID
# ERR on one payload: uvcvideo drops the frame with nodrop=0, the others are intact.
echo 0 >/sys/module/uvcvideo/parameters/nodrop 2>/dev/null
yuyv err-payload "Integrated RGB"
echo 1 >/sys/module/uvcvideo/parameters/nodrop 2>/dev/null
# Overlong frames: uvcvideo flags them (V4L2_BUF_FLAG_ERROR, which v4l2-ctl --verbose prints).
step "uvcvideo: frame-overrun"
serve frame-overrun "Integrated RGB"
[ -n "$dev" ] && check sh -c "timeout 30 v4l2-ctl -d $dev --set-fmt-video=width=640,height=480,pixelformat=YUYV \
  --stream-mmap=4 --stream-count=5 --verbose 2>&1 | grep -q error"
unserve
# EOF only in header-only payloads, FID never toggling: MJPEG frames must still end. uvcvideo syncs on FID
# toggles unless the device has UVC_QUIRK_STREAM_NO_FID (0x10), which takes the quirks parameter here.
step "uvcvideo: eof-payload-no-fid"
q=$(cat /sys/module/uvcvideo/parameters/quirks 2>/dev/null)
echo 16 >/sys/module/uvcvideo/parameters/quirks 2>/dev/null
serve eof-payload-no-fid "Integrated RGB"
[ -n "$dev" ] && check timeout 30 v4l2-ctl -d "$dev" --set-fmt-video=width=640,height=480,pixelformat=MJPG \
  --stream-mmap=4 --stream-count=30 --stream-to=/tmp/eof.mjpg
[ -n "$q" ] && echo "$q" >/sys/module/uvcvideo/parameters/quirks 2>/dev/null
unserve

step "emuvc log (tail)"
tail -n 12 /tmp/usbip.log
dmesg | grep -iE 'error|fail|warn' | grep -v -iE 'acpi|firmware|pci|i8042|rtc' | tail -10
echo
[ $fail = 0 ] && echo "ALL PASSED" || echo "SOME CHECKS FAILED"
exit $fail
