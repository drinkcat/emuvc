#!/usr/bin/env bash
# End-to-end libuvc tests against the emuvc libusb shim. See tests/libuvc/README.md.
# usage: scripts/libuvc-test.sh [-k PATTERN] [-v]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIBUVC_SRC="${LIBUVC_SRC:-$ROOT/../libuvc}"
BUILD="$ROOT/target/libuvc-build"
OUT="$ROOT/target/libuvc-test"
SHIM="$ROOT/target/debug/libemuvc_libusb.so"
CFG="$ROOT/tests/libuvc/configs"
PATTERN=""
VERBOSE=0

while getopts "k:vh" o; do
  case $o in
    k) PATTERN=$OPTARG ;;
    v) VERBOSE=1 ;;
    *) echo "usage: $0 [-k PATTERN] [-v]" >&2; exit 2 ;;
  esac
done

# ---- build ----
if [ ! -d "$LIBUVC_SRC" ]; then
  LIBUVC_SRC="$ROOT/target/libuvc-src"
  [ -d "$LIBUVC_SRC" ] || git clone --depth 1 https://github.com/libuvc/libuvc "$LIBUVC_SRC"
fi
(cd "$ROOT" && cargo build -q -p emuvc-libusb)
cmake -S "$LIBUVC_SRC" -B "$BUILD" -DCMAKE_BUILD_TYPE=Debug >"$ROOT/target/libuvc-build.log" 2>&1
cmake --build "$BUILD" -j"$(nproc)" >>"$ROOT/target/libuvc-build.log" 2>&1 \
  || { tail -20 "$ROOT/target/libuvc-build.log"; exit 2; }
mkdir -p "$OUT"
# shellcheck disable=SC2046
cc -g -O1 -Wall -Wextra -o "$OUT/stream_test" "$ROOT/tests/libuvc/stream_test.c" \
  -I"$LIBUVC_SRC/include" -I"$BUILD/include" -L"$BUILD" -luvc -Wl,-rpath,"$BUILD" \
  $(pkg-config --cflags --libs libjpeg libusb-1.0) -lpthread

# ---- runner ----
PASS=0; FAIL=0; INFO=0
TMP="$(mktemp)"; trap 'rm -f "$TMP"' EXIT

# run_case NAME EXPECT [VAR=val ...] -- stream_test args...
#   EXPECT: pass  = must exit 0;  info = report outcome only, never fails the suite
#   Extra checks on output can be done by the caller via CHECK_RE (grep -E on output).
run_case() {
  local name=$1 expect=$2; shift 2
  local envs=()
  while [ "$1" != "--" ]; do envs+=("$1"); shift; done
  shift
  [[ -n "$PATTERN" && ! "$name" =~ $PATTERN ]] && return 0
  local t0=$SECONDS rc=0
  # Emulator-side frame stats ("stop streaming ... frames_dropped") help tell
  # device drops from host drops when a case fails.
  env LD_PRELOAD="$SHIM" EMUVC_LOG="${EMUVC_LOG:-emuvc_uvc::stream=info}" "${envs[@]}" timeout 60 "$OUT/stream_test" "$@" >"$TMP" 2>&1 || rc=$?
  local secs=$((SECONDS - t0)) ok=1
  [ $rc -eq 0 ] || ok=0
  if [ -n "${CHECK_RE:-}" ] && ! grep -Eq "$CHECK_RE" "$TMP"; then ok=0; echo "(missing output /$CHECK_RE/)" >>"$TMP"; fi
  local summary; summary=$({ grep -E '^RESULT|^devices:' "$TMP" || true; } | tr '\n' ' ')
  if [ "$expect" = info ]; then
    INFO=$((INFO + 1)); echo "INFO $name rc=$rc (${secs}s) $summary"
  elif [ $ok = 1 ]; then
    PASS=$((PASS + 1)); echo "PASS $name (${secs}s) $summary"
  else
    FAIL=$((FAIL + 1)); echo "FAIL $name rc=$rc (${secs}s) $summary"
  fi
  if [ $VERBOSE = 1 ]; then sed 's/^/    | /' "$TMP"
  elif [ "$expect" = pass ] && [ $ok = 0 ]; then tail -15 "$TMP" | sed 's/^/    | /'
  elif [ "$expect" = info ]; then { grep -vE '^RESULT' "$TMP" || true; } | { if [ "$name" = stall-get-def ]; then grep -E 'FAIL|GET_DEF'; else cat; fi; } | tail -${INFO_LINES:-4} | sed 's/^/    | /'; fi
  CHECK_RE=""; return 0
}
CHECK_RE=""

D=EMUVC_DEVICE=luxvisions-30c9-0052
V=EMUVC_CLOCK=virtual

run_case mjpeg-1080p pass $D -- -f mjpeg -s 1920x1080 -r 30 -n 60 -t 15 --fps-tol 10
run_case yuyv-vga pass $D -- -f yuyv -s 640x480 -r 30 -n 60 -t 15
run_case yuyv-1080p pass $D -- -f yuyv -s 1920x1080 -r 5 -n 10 -t 20
run_case virtual-clock pass $D $V -- -f mjpeg -s 640x480 -r 30 -n 300 -t 9
run_case controls pass $D -- --controls
CHECK_RE='devices: 2' run_case two-devices pass EMUVC_DEVICE=luxvisions-30c9-0052,luxvisions-30c9-0052 -- --list
run_case iso-errors pass EMUVC_CONFIG="$CFG/iso-errors.toml" -- -f mjpeg -s 640x480 -r 30 -n 60 -t 20 --allow-gaps --allow-corrupt
run_case zero-max-payload info EMUVC_CONFIG="$CFG/zero-max-payload.toml" -- -f yuyv -s 640x480 -r 30 -n 30 -t 10
run_case no-eof pass EMUVC_CONFIG="$CFG/no-eof.toml" -- -f yuyv -s 640x480 -r 30 -n 30 -t 15
INFO_LINES=40 run_case stall-get-def info EMUVC_CONFIG="$CFG/stall-get-def.toml" -- --controls

echo "summary: $PASS passed, $FAIL failed, $INFO informational"
[ $FAIL -eq 0 ]
