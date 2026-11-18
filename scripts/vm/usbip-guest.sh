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

# uvc_close() with the status transfer submitted (libuvc PR #335). Releasing the VideoControl interface kills
# the transfer (usbfs), libusb reports it completed, and libuvc's callback submits it again, to an interface
# it no longer holds: usbfs claims it implicitly, warning "did not claim interface 0 before use" (or fails
# with EBUSY if uvcvideo has it back). The libusb shim reports that as "libusb misuse". The kernel and the
# shim must agree on the same libuvc's close, whether libuvc still does it or not.
step "libuvc PR #335: the kernel and the libusb shim agree on uvc_close()"
claims() { dmesg | grep -c "did not claim interface"; }
claims0=$(claims)
check "$STREAM_TEST" -d 30c9:0052 -f yuyv -s 640x480 -r 30 -n 5 -t 20
kernel=$(( $(claims) > claims0 ))
dmesg | grep "did not claim interface" | tail -n 1
shim=$(LD_PRELOAD="$ROOT/target/debug/libemuvc_libusb.so" EMUVC_DEVICE=luxvisions-30c9-0052 EMUVC_CLOCK=virtual \
  "$STREAM_TEST" -d 30c9:0052 -f yuyv -s 640x480 -r 30 -n 5 -t 20 2>&1 | grep -c "libusb misuse")
[ "$shim" -gt 0 ] && shim=1
echo "  the kernel saw libuvc resubmit: $kernel, the shim: $shim"
check test "$kernel" = "$shim"

# serve CONFIG|SPEC PATTERN: serves a configuration (tests/libuvc/configs/CONFIG.toml) or a device (a model,
# lsusb:DUMP...) on the next port and attaches it; $dev is its first new video device whose name matches
# PATTERN (the capture one: metadata devices come after it), $srv the server's PID (killing it detaches the
# device and frees the vhci port). The server logs to /tmp/usbip-NAME.log (NAME: CONFIG, or the spec's base
# name).
port=3250
serve() {
  cfg=$1 pat=$2
  src="-d $cfg"
  [ -f "$ROOT/tests/libuvc/configs/$cfg.toml" ] && src="-c $ROOT/tests/libuvc/configs/$cfg.toml"
  before=$(ls /sys/class/video4linux)
  # shellcheck disable=SC2086
  EMUVC_LOG="${EMUVC_LOG:-info}" "$EMUVC" usbip --listen 127.0.0.1:$port $src \
    >"/tmp/usbip-$(basename "$cfg" .txt).log" 2>&1 &
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
# stats_ok DEV: uvcvideo found no invalid payload header in the last stream of DEV's camera (the debugfs
# statistics of its streaming interfaces; prints those of the interfaces that streamed).
stats_ok() {
  usb=$(dirname "$(readlink -f "/sys/class/video4linux/${1#/dev/}/device")")
  set -- /sys/kernel/debug/usb/uvcvideo/"$(cat "$usb/busnum")-$(cat "$usb/devnum")"-*/stats
  [ -r "$1" ] || return 2
  for f; do
    grep -q '^frames: *0$' "$f" || echo "  $(grep -E '^(frames|packets|empty|errors|invalid):' "$f" | tr -s ' \n' ' ')"
  done
  ! grep -q '^invalid: *[1-9]' "$@"
}
# libuvc_stream VID:PID ARGS...: stream_test through libuvc and the real libusb (libuvc detaches uvcvideo from
# the camera, and attaches it back when it closes).
libuvc_stream() { check timeout 40 "$STREAM_TEST" -d "$@"; }
# yuyv_checks NAME [VID:PID [FPS]]: 30 YUYV 640x480 frames from $dev through uvcvideo, stamps checked (gaps
# are allowed); with VID:PID, then through libuvc and the real libusb (at FPS, 30 by default).
yuyv_checks() {
  [ -n "$dev" ] && check timeout 30 v4l2-ctl -d "$dev" --set-fmt-video=width=640,height=480,pixelformat=YUYV \
    --stream-mmap=4 --stream-count=30 --stream-to=/tmp/$1.yuyv && check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/$1.yuyv &&
    check stats_ok "$dev"
  [ -n "${2:-}" ] && libuvc_stream "$2" -f yuyv -s 640x480 -r "${3:-30}" -n 30 -t 20 --max-gaps 2
}
# yuyv CONFIG PATTERN [VID:PID]: yuyv_checks on a served configuration.
yuyv() {
  step "uvcvideo: $1"
  serve "$1" "$2"
  yuyv_checks "$1" "${3:-}"
  dmesg | grep -i "uvcvideo" | tail -n 2
  unserve
}
# camera DUMP PATTERN VID:PID FPS: yuyv_checks on a real camera, emulated from its lsusb -v dump (libuvc's
# cameras/, in tests/fixtures/lsusb/libuvc-cameras).
camera() {
  step "camera: $1"
  serve "lsusb:$ROOT/tests/fixtures/lsusb/libuvc-cameras/$1.txt" "$2"
  yuyv_checks "$1" "$3" "$4"
  dmesg | grep -i "uvcvideo" | tail -n 2
  unserve
}

step "SuperSpeed synthetic camera (vhci SuperSpeed port)"
serve synthetic-iso-ss Synthetic
lsusb -t | grep -i -B1 -A2 "5000M" | head -8
[ -n "$dev" ] && check timeout 30 v4l2-ctl -d "$dev" --set-fmt-video=width=640,height=480,pixelformat=YUYV \
  --stream-mmap=4 --stream-count=30 --stream-to=/tmp/ss.yuyv && check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/ss.yuyv
# libuvc sizes SuperSpeed packets from wBytesPerInterval (bursts: libuvc#270), so it picks the 4096-byte alt.
libuvc_stream 1209:0001 -f yuyv -s 640x480 -r 30 -n 30 -t 20 --max-gaps 2
check sh -c "grep 'start streaming' /tmp/usbip-synthetic-iso-ss.log | tail -n 1 | grep '4096 bytes/payload'"
unserve
# Logitech C930e: the kernel takes configuration index 0, UVC 1.0 on the real camera. The -uvc15 variant only
# has the UVC 1.5 configuration (encoding unit, H.264 interface). uvcvideo and libuvc must stream YUYV
# either way.
for model in logitech-046d-0843 logitech-046d-0843-uvc15; do
  step "$model"
  serve "$model" C930e
  grep -l 0843 /sys/bus/usb/devices/*/idProduct | while read -r f; do
    echo "$(dirname "$f"): bConfigurationValue $(cat "$(dirname "$f")/bConfigurationValue")"
  done
  [ -n "$dev" ] && v4l2-ctl -d "$dev" --list-formats
  dmesg | grep -i "C930e\|0843" | tail -n 4
  [ -n "$dev" ] && check timeout 30 v4l2-ctl -d "$dev" --set-fmt-video=width=640,height=480,pixelformat=YUYV \
    --stream-mmap=4 --stream-count=30 --stream-to=/tmp/c930e.yuyv && check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/c930e.yuyv
  libuvc_stream 046d:0843 -f yuyv -s 640x480 -r 30 -n 30 -t 20 --max-gaps 2
  unserve
done
# Real cameras from their lsusb -v dumps: the C920 (H.264 next to YUYV), the LifeCam Show (YUYV 640x480 at
# 15 fps only, interval 666667: libuvc finds it at 14 fps, libuvc#297) and the QuickCam Pro 9000 (a vendor
# descriptor among the VideoControl ones, no product string: uvcvideo names it by VID:PID).
camera logitech_hd_pro_920 C920 046d:082d 30
camera ms_lifecam_show LifeCam 045e:0729 14
camera quickcampro9000 046d:0809 046d:0809 30

# usbfs's memory limit, down to 1 MiB (16 by default), as the libusb shim emulates it (libuvc suite:
# usbfs-memory-1mb): 10 of libuvc's isochronous transfers of 96 KiB fit, the 11th fails with ENOMEM
# ("submiturb failed, errno=12"), and libuvc streams on the 10. The kernel must agree: libusb's debug log has
# every submit and completion, so the transfers in flight when the kernel refused one are known.
step "usbfs memory limit (libuvc: usbfs-memory-1mb)"
serve usbfs-memory-1mb Synthetic
lim=/sys/module/usbcore/parameters/usbfs_memory_mb
old=$(cat $lim)
echo 1 >$lim
LIBUSB_DEBUG=4 timeout 40 "$STREAM_TEST" -d 1209:0001 -f yuyv -s 640x480 -r 30 -n 60 -t 20 >/tmp/usbfs.log 2>&1
rc=$?
echo "$old" >$lim
grep -E "^RESULT|errno=12" /tmp/usbfs.log | head -n 3
check [ "$rc" = 0 ]
sed -n '1,/submiturb failed, errno=12/p' /tmp/usbfs.log >/tmp/usbfs-upto.log
sub=$(grep -c "need 1 urbs for new transfer with length 98304" /tmp/usbfs-upto.log)
done_=$(grep -c "handling completion status .* of iso urb" /tmp/usbfs-upto.log)
echo "isochronous transfers in flight when the kernel refused one: $((sub - 1 - done_))"
check [ "$((sub - 1 - done_))" = 10 ]
unserve

# uvcvideo as the reference for the libuvc cases that say it copes (tests/libuvc/configs).
step "Luxvisions IR function (libuvc: api-second-function)"
# uvcvideo binds both functions: video devices hang off the IR function's VideoControl interface (2).
check sh -c 'for n in /sys/class/video4linux/*; do readlink -f "$n/device"; done | grep -q "1-1:1\.2$"'

yuyv need-set-interface "Integrated RGB"    # needs SET_INTERFACE before class requests
yuyv probe-zero-index "Integrated RGB"      # probe reply with format/frame index 0
yuyv max-payload-huge "Integrated RGB"      # dwMaxPayloadTransferSize 0xffff0400
yuyv desc-composite-input Synthetic         # an 8-byte composite input terminal
yuyv desc-bcduvc-0101 Synthetic             # bcdUVC 0x0101
yuyv isight Synthetic 05ac:8501             # the built-in iSight's packet format
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

# Bulk streaming through uvcvideo, whose URBs (16 KiB) are smaller than the payloads (32 KiB).
yuyv synthetic-bulk-hs Synthetic 1209:0001
yuyv desc-cs-after-endpoint Synthetic       # VS descriptors after the bulk endpoint (Ecamm Pico iMage)
# A bulk camera that stops only on CLEAR_FEATURE(ENDPOINT_HALT) (libuvc#257): uvcvideo stops bulk streams
# that way, so a second capture works.
step "uvcvideo: bulk-stop-clear-halt"
serve bulk-stop-clear-halt Synthetic
for round in 1 2; do
  [ -n "$dev" ] && check timeout 30 v4l2-ctl -d "$dev" --set-fmt-video=width=640,height=480,pixelformat=YUYV \
    --stream-mmap=4 --stream-count=10 --stream-to=/tmp/bulk$round.yuyv &&
    check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/bulk$round.yuyv && check stats_ok "$dev"
done
grep -E "commit|streaming" /tmp/usbip-bulk-stop-clear-halt.log | tail -n 6
unserve

# Control Change interrupts (synthetic-events.toml: brightness 10 every 200 ms): uvcvideo turns them into
# V4L2 control events, so it vouches for the emulator's status packets. uvcvideo only reads the status
# endpoint while the camera is on (streaming, or during some control ioctls): a capture runs meanwhile.
step "uvcvideo: control events"
serve synthetic-events Synthetic
if [ -n "$dev" ]; then
  timeout 20 v4l2-ctl -d "$dev" --set-fmt-video=width=640,height=480,pixelformat=YUYV --stream-mmap=4 \
    --stream-count=300 >/dev/null 2>&1 &
  capture=$!
  check sh -c "timeout 10 v4l2-ctl -d $dev --wait-for-event=ctrl=brightness | grep 'value: 10 0xa'"
  kill "$capture" 2>/dev/null; wait "$capture" 2>/dev/null
fi
unserve

# Unplugged mid-stream (unplug.toml): the server closes the connection, as usbip-host does when its device
# goes away, and vhci-hcd reports the disconnect. Neither uvcvideo's client nor libuvc through the real
# libusb may hang then (libuvc#152, #53: stop and close after an unplug).
# Two streaming interfaces of one camera at once (synthetic-uvc11-2streams.toml): uvcvideo makes a capture
# node of each, and both must deliver intact frames while the other streams.
step "uvcvideo: two streams at once"
serve synthetic-uvc11-2streams Synthetic
cap1= cap2=
for n in $(ls /sys/class/video4linux | sort -V); do
  echo "$before" | grep -qx "$n" && continue
  v4l2-ctl -d "/dev/$n" -D 2>/dev/null | sed -n '/Device Caps/,$p' | grep -q "Video Capture" || continue
  if [ -z "$cap1" ]; then cap1=/dev/$n; elif [ -z "$cap2" ]; then cap2=/dev/$n; fi
done
check test -n "$cap2"
if [ -n "$cap2" ]; then
  timeout 30 v4l2-ctl -d "$cap2" --set-fmt-video=width=640,height=480,pixelformat=YUYV --stream-mmap=4 \
    --stream-count=60 --stream-to=/tmp/stream2.yuyv >/dev/null 2>&1 &
  second=$!
  check timeout 30 v4l2-ctl -d "$cap1" --set-fmt-video=width=640,height=480,pixelformat=YUYV --stream-mmap=4 \
    --stream-count=60 --stream-to=/tmp/stream1.yuyv
  check wait "$second"
  check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/stream1.yuyv
  check "$EMUVC" check-frames -f yuyv -s 640x480 /tmp/stream2.yuyv
  check stats_ok "$cap1"
fi
grep -E "start streaming|stop streaming" /tmp/usbip-synthetic-uvc11-2streams.log | tail -n 4
unserve

disconnects() { dmesg | grep -c "USB disconnect"; }
step "uvcvideo: unplugged while streaming"
unplugs=$(disconnects)
serve unplug Synthetic
[ -n "$dev" ] && check sh -c "timeout 30 v4l2-ctl -d $dev --set-fmt-video=width=640,height=480,pixelformat=YUYV \
  --stream-mmap=4 --stream-count=300 >/tmp/unplug.txt 2>&1; rc=\$?; tail -c 200 /tmp/unplug.txt; echo; \
  echo \"  v4l2-ctl: rc=\$rc\"; [ \$rc -ne 124 ]"
check test "$(disconnects)" -gt "$unplugs"
grep -E "fault|disconnected" /tmp/usbip-unplug.log | tail -n 2
unserve
step "libuvc through the real libusb: unplugged while streaming"
unplugs=$(disconnects)
serve unplug Synthetic
check sh -c "timeout 30 $STREAM_TEST -d 1209:0002 -f yuyv -s 640x480 -r 30 -n 300 -t 10; rc=\$?; \
  echo \"  stream_test: rc=\$rc\"; [ \$rc -eq 1 ] || [ \$rc -eq 2 ]"
check test "$(disconnects)" -gt "$unplugs"
grep -E "fault|disconnected" /tmp/usbip-unplug.log | tail -n 2
unserve

step "emuvc log (tail)"
tail -n 12 /tmp/usbip.log
dmesg | grep -iE 'error|fail|warn' | grep -v -iE 'acpi|firmware|pci|i8042|rtc' | tail -10
echo
[ $fail = 0 ] && echo "ALL PASSED" || echo "SOME CHECKS FAILED"
exit $fail
