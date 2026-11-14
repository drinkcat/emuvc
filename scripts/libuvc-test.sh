#!/usr/bin/env bash
# End-to-end libuvc tests against the emuvc libusb shim. See tests/libuvc/README.md.
# usage: scripts/libuvc-test.sh [-k PATTERN] [-v]
#   Tests upstream libuvc master, fetched into target/libuvc-src on every run.
#   LIBUVC_REPO / LIBUVC_REF select another repository or branch, tag or ref
#   (e.g. LIBUVC_REF=pull/336/head); LIBUVC_SRC=DIR tests a local checkout as is.
#   LIBUVC_TEST_MARKDOWN=FILE appends a Markdown report (e.g. $GITHUB_STEP_SUMMARY), titled
#   with LIBUVC_TEST_NAME if set.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# LIBUVC_BUILD / LIBUVC_CMAKE_FLAGS select another libuvc build (e.g. with
# coverage instrumentation, see scripts/libuvc-coverage.sh). LIBUVC_TEST_CFLAGS are
# extra flags for the test programs, LIBUVC_PRELOAD libraries to load before the shim
# (both used by scripts/libuvc-asan.sh).
BUILD="${LIBUVC_BUILD:-$ROOT/target/libuvc-build}"
OUT="$BUILD/tests"
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

# ---- libuvc source ----
if [ -n "${LIBUVC_SRC:-}" ]; then
  LIBUVC_DESC="local tree $LIBUVC_SRC"
  LIBUVC_COMMIT="not a git checkout"
  [ ! -e "$LIBUVC_SRC/.git" ] || LIBUVC_COMMIT="$(git -C "$LIBUVC_SRC" log -1 --format='%h %s')"
else
  LIBUVC_SRC="$ROOT/target/libuvc-src"
  LIBUVC_REPO="${LIBUVC_REPO:-https://github.com/libuvc/libuvc}"
  LIBUVC_REF="${LIBUVC_REF:-master}"
  LIBUVC_DESC="$LIBUVC_REPO $LIBUVC_REF"
  # Always name the clone's repository: with a damaged target/libuvc-src/.git, `git -C`
  # would fall back to the enclosing emuvc repository and check libuvc out over it.
  lgit() { git --git-dir="$LIBUVC_SRC/.git" --work-tree="$LIBUVC_SRC" "$@"; }
  lgit rev-parse --git-dir >/dev/null 2>&1 || { rm -rf "$LIBUVC_SRC"; git init -q "$LIBUVC_SRC"; }
  if lgit fetch -q --depth 1 "$LIBUVC_REPO" "$LIBUVC_REF"; then
    lgit checkout -q --force --detach FETCH_HEAD
  elif lgit rev-parse -q --verify HEAD >/dev/null; then
    echo "warning: cannot fetch $LIBUVC_DESC, testing the libuvc fetched last" >&2
    LIBUVC_DESC="$LIBUVC_DESC (fetch failed, older copy)"
  else
    echo "cannot fetch $LIBUVC_DESC" >&2; exit 2
  fi
  LIBUVC_COMMIT="$(lgit log -1 --format='%h %s')"
fi
LIBUVC_SRC="$(cd "$LIBUVC_SRC" && pwd)"
echo "libuvc: $LIBUVC_DESC at $LIBUVC_COMMIT"

# ---- build ----
# A build directory belongs to one source tree: start over when the source changes.
if [ -f "$BUILD/CMakeCache.txt" ] && ! grep -qxF "CMAKE_HOME_DIRECTORY:INTERNAL=$LIBUVC_SRC" "$BUILD/CMakeCache.txt"; then
  case $BUILD in
    "$ROOT"/target/*) rm -rf "$BUILD" ;;
    *) echo "$BUILD was configured for another libuvc source tree" >&2; exit 2 ;;
  esac
fi
(cd "$ROOT" && cargo build -q -p emuvc-libusb)
# shellcheck disable=SC2086
cmake -S "$LIBUVC_SRC" -B "$BUILD" -DCMAKE_BUILD_TYPE=Debug ${LIBUVC_CMAKE_FLAGS:-} >"$ROOT/target/libuvc-build.log" 2>&1
cmake --build "$BUILD" -j"$(nproc)" >>"$ROOT/target/libuvc-build.log" 2>&1 \
  || { tail -20 "$ROOT/target/libuvc-build.log"; exit 2; }
mkdir -p "$OUT"
# shellcheck disable=SC2046,SC2086
cc -g -O1 -Wall -Wextra ${LIBUVC_TEST_CFLAGS:-} -o "$OUT/stream_test" "$ROOT/tests/libuvc/stream_test.c" \
  -I"$LIBUVC_SRC/include" -I"$BUILD/include" -L"$BUILD" -luvc -Wl,-rpath,"$BUILD" \
  $(pkg-config --cflags --libs libjpeg libusb-1.0) -lpthread
# api_test uses libuvc internals (libuvc_internal.h) and libusb directly
# shellcheck disable=SC2046,SC2086
cc -g -O1 -Wall -Wextra ${LIBUVC_TEST_CFLAGS:-} -o "$OUT/api_test" "$ROOT/tests/libuvc/api_test.c" \
  -I"$LIBUVC_SRC/include" -I"$BUILD/include" -L"$BUILD" -luvc -Wl,-rpath,"$BUILD" \
  $(pkg-config --cflags --libs libjpeg libusb-1.0) -lpthread

# ---- runner ----
PASS=0; FAIL=0; XFAIL=0
FAILED=() # "name rc" of failed cases
BUGS=(); FIXED=() # "name|reference" of known libuvc bugs: still failing / passing now
TMP="$(mktemp)"; trap 'rm -f "$TMP"' EXIT

# run_case NAME EXPECT [VAR=val ...] -- stream_test args...
#   EXPECT: pass   must exit 0 (EXPECT_RC=N: exit N, an expected, checked failure) and print a line
#                  matching CHECK_RE (grep -E on the output) if set.
#           robust a misbehaving device or an injected fault: failing cleanly (exit 1) is as good as
#                  working. Crashes, hangs, sanitizer reports and setup errors fail. CHECK_RE applies.
#           bug    a known libuvc bug, BUG="reference: what is wrong". The case checks the correct
#                  behaviour like a pass case. Failing cleanly is XFAIL (still broken, the suite
#                  passes); passing is XPASS (fixed upstream: make it a pass case). Crashes, hangs
#                  and sanitizer reports fail, unless BUG_CRASH=1 (the bug is the crash).
run_case() {
  local name=$1 expect=$2; shift 2
  local envs=()
  while [ "$1" != "--" ]; do envs+=("$1"); shift; done
  shift
  [[ -n "$PATTERN" && ! "$name" =~ $PATTERN ]] && return 0
  local t0=$SECONDS rc=0
  # Emulator-side frame stats ("stop streaming ... frames_dropped") help tell
  # device drops from host drops when a case fails.
  # timeout stays outside LD_PRELOAD: only the test process gets the shim (and libasan).
  timeout 60 env LD_PRELOAD="${LIBUVC_PRELOAD:+$LIBUVC_PRELOAD:}$SHIM" EMUVC_LOG="${EMUVC_LOG:-emuvc_uvc::stream=info}" "${envs[@]}" "$OUT/${TEST_BIN:-stream_test}" "$@" >"$TMP" 2>&1 || rc=$?
  local secs=$((SECONDS - t0)) rc_ok=0 re_ok=1 clean=0 verdict=FAIL
  [ $rc -eq "${EXPECT_RC:-0}" ] && rc_ok=1
  if [ -n "${CHECK_RE:-}" ] && ! grep -Eq "$CHECK_RE" "$TMP"; then re_ok=0; echo "(missing output /$CHECK_RE/)" >>"$TMP"; fi
  case $rc in 0|1) clean=1 ;; esac
  case $expect in
    pass) [ $rc_ok$re_ok = 11 ] && verdict=PASS ;;
    robust) [ $clean$re_ok = 11 ] && verdict=PASS ;;
    bug)
      if [ $rc_ok$re_ok = 11 ]; then verdict=XPASS
      elif [ $clean = 1 ] || [ -n "${BUG_CRASH:-}" ]; then verdict=XFAIL; fi ;;
    *) echo "run_case $name: bad EXPECT $expect" >&2; exit 2 ;;
  esac
  local summary; summary=$({ grep -E '^RESULT|^devices:' "$TMP" || true; } | tr '\n' ' ')
  case $verdict in
    PASS) PASS=$((PASS + 1)); echo "PASS $name (${secs}s) $summary" ;;
    XPASS) PASS=$((PASS + 1)); FIXED+=("$name|$BUG"); echo "XPASS $name (${secs}s) [fixed? $BUG] $summary" ;;
    XFAIL) XFAIL=$((XFAIL + 1)); BUGS+=("$name|$BUG"); echo "XFAIL $name rc=$rc (${secs}s) [$BUG]" ;;
    FAIL) FAIL=$((FAIL + 1)); FAILED+=("$name $rc"); echo "FAIL $name rc=$rc (${secs}s) $summary" ;;
  esac
  if [ $VERBOSE = 1 ]; then sed 's/^/    | /' "$TMP"
  elif [ $verdict = FAIL ]; then tail -15 "$TMP" | sed 's/^/    | /'
  elif [ $verdict = XFAIL ]; then { grep -E '^ *FAIL|^uvc_' "$TMP" || true; } | head -3 | sed 's/^/    | /'; fi
  CHECK_RE=""; EXPECT_RC=""; BUG=""; BUG_CRASH=""; return 0
}
CHECK_RE=""; EXPECT_RC=""; BUG=""; BUG_CRASH=""

D=EMUVC_DEVICE=luxvisions-30c9-0052
V=EMUVC_CLOCK=virtual

run_case mjpeg-1080p pass $D -- -f mjpeg -s 1920x1080 -r 30 -n 60 -t 15 --fps-tol 10
run_case yuyv-vga pass $D -- -f yuyv -s 640x480 -r 30 -n 60 -t 15
run_case yuyv-1080p pass $D -- -f yuyv -s 1920x1080 -r 5 -n 10 -t 20
run_case virtual-clock pass $D $V -- -f mjpeg -s 640x480 -r 30 -n 300 -t 9
# A consumer slower than the frame rate: libuvc overwrites frames its callback thread has not taken
# yet. Those must count as skipped, not as gaps (which stay at 0 allowed).
CHECK_RE='skipped=[1-9]' run_case slow-callback pass $D -- -f mjpeg -s 640x480 -r 30 -n 20 -t 15 --cb-sleep 70
run_case controls pass $D -- --controls
CHECK_RE='devices: 2' run_case two-devices pass EMUVC_DEVICE=luxvisions-30c9-0052,luxvisions-30c9-0052 -- --list
run_case iso-errors pass EMUVC_CONFIG="$CFG/iso-errors.toml" -- -f mjpeg -s 640x480 -r 30 -n 60 -t 20 --allow-gaps --allow-corrupt
# The device commits dwMaxPayloadTransferSize 0: libuvc must not crash (it divided by zero before
# libuvc#340) and falls back to the first alt setting with bandwidth. That alt is too small for the
# stream, so frames arrive with gaps; only check that it streams.
CHECK_RE='start streaming .* 192 bytes/payload' run_case zero-max-payload pass EMUVC_CONFIG="$CFG/zero-max-payload.toml" -- -f yuyv -s 640x480 -r 30 -n 10 -t 20 --allow-gaps
run_case no-eof pass EMUVC_CONFIG="$CFG/no-eof.toml" -- -f yuyv -s 640x480 -r 30 -n 30 -t 15
# GET_DEF(brightness) stalls: libuvc must report PIPE for it (a failed check, so stream_test exits 1).
EXPECT_RC=1 CHECK_RE='FAIL brightness GET_DEF = .*\(Pipe\)' run_case stall-get-def pass EMUVC_CONFIG="$CFG/stall-get-def.toml" -- --controls

# api_test: public API coverage (see tests/libuvc/api_test.c); modes adapt to the device.
# run_api NAME [VAR=val ...] -- api_test args...  (failure = crash/hang/inconsistent data)
run_api() { local name=$1; shift; TEST_BIN=api_test run_case "$name" pass "$@"; }
for m in info controls convert poll still multi status negotiate; do
  CHECK_RE="^RESULT mode=$m " run_api "api-$m" $D -- -m 500 "$m"
done
CHECK_RE='^RESULT mode=info ' run_api api-info-two-devices EMUVC_DEVICE=luxvisions-30c9-0052,luxvisions-30c9-0052 -- info
CHECK_RE='^RESULT mode=poll ' run_api api-poll-virtual-clock $D $V -- poll
CHECK_RE='^RESULT mode=poll ' run_api api-poll-no-eof EMUVC_CONFIG="$CFG/no-eof.toml" -- poll
CHECK_RE='^RESULT mode=convert ' run_api api-convert-no-dht EMUVC_CONFIG="$CFG/no-dht.toml" -- convert

# Quirky-but-working devices: the API must behave as on the default device (strict).
for c in header-6 header-long idle-zlp probe-len-26 probe-no-frame-size startup-delay no-fid-toggle; do
  CHECK_RE='^RESULT mode=poll ' run_api "api-poll-$c" EMUVC_CONFIG="$CFG/$c.toml" -- poll negotiate
done
# Misbehaving devices: -l counts failed expectations as soft; crashes and hangs still fail.
for c in err-bit drop-truncate header-2 header-lie probe-len-34-strict ignore-format iso-faults disconnect bogus-header bulk-faults; do
  CHECK_RE='^RESULT mode=negotiate ' run_api "api-fault-$c" EMUVC_CONFIG="$CFG/$c.toml" -- -l poll still status negotiate
done

# Synthetic devices (every control/unit/format, bulk/SuperSpeed/UVC 1.0/two streams/...): all modes, strict.
for c in "$CFG"/synthetic-*.toml; do
  n=$(basename "$c" .toml)
  CHECK_RE='^RESULT mode=wrap ' run_api "api-$n" EMUVC_CONFIG="$c" $V -- -m 1500 all
done

# Malformed descriptors (configs/desc-*.toml, emuvc_devices::patch) the host must cope with: all modes, strict.
for c in desc-unknown-vs-subtype desc-color-before-frames desc-num-frames-over desc-duplicate-frame; do
  CHECK_RE='^RESULT mode=wrap ' run_api "api-$c" EMUVC_CONFIG="$CFG/$c.toml" $V -- -m 1500 all
done

# Issue reproductions (configs/issue-*.toml) libuvc handles now: all modes, strict.
for c in issue-193-zero-packet-alt issue-276-get-max-zero-payload issue-282-payload-changes issue-336-step-zero; do
  CHECK_RE='^RESULT mode=wrap ' run_api "api-$c" EMUVC_CONFIG="$CFG/$c.toml" $V -- -m 1500 all
done

# Devices too broken to stream, injected faults (robust): libuvc may fail, but cleanly. Every mode
# runs in its own process, so that a crash in one doesn't hide the others.
API_MODES="info controls convert poll still multi status negotiate wrap"
run_api_robust() { # NAME [VAR=val ...]
  local name=$1; shift
  for m in $API_MODES; do
    CHECK_RE="^RESULT mode=$m " TEST_BIN=api_test run_case "$name/$m" robust "$@" -- "$m"
  done
}
# every probe/commit stalls; the device rejects the probe length its UVC version requires; the first
# frames are truncated (libuvc hands them to the callback as they are)
for c in issue-1-probe-stall issue-strict-probe-len issue-garbage-first-frames; do
  run_api_robust "api-$c" EMUVC_CONFIG="$CFG/$c.toml" $V
done

# Known libuvc bugs (bug): the expected behaviour, XFAIL until libuvc is fixed.
run_api_bug() { # NAME [VAR=val ...]: all modes, strict
  local name=$1; shift
  CHECK_RE='^RESULT mode=wrap ' TEST_BIN=api_test run_case "$name" bug "$@" -- -m 1500 all
}
BUG="libuvc#348: uvc_open rejects an encoding unit (unknown VC subtype)" \
  run_api_bug api-issue-encoding-unit EMUVC_CONFIG="$CFG/issue-encoding-unit.toml" $V
BUG="one frame descriptor's interval list overruns its bLength: libuvc rejects the whole device" \
  run_api_bug api-desc-interval-count-over EMUVC_CONFIG="$CFG/desc-interval-count-over.toml" $V

# Shim-level libusb failures (EMUVC_LIBUSB_FAIL): libuvc's error paths must neither crash nor hang.
# FAULT[:mode] ; default mode is every mode in one process (lenient).
for f in get_config_descriptor=io get_active_config_descriptor=io open=access \
         claim_interface=busy detach_kernel_driver=io kernel_driver_active=io set_interface_alt_setting=io \
         clear_halt=io submit_transfer=no_mem@3 alloc_transfer=no_mem@5 alloc_transfer=no_mem \
         control_transfer=pipe@5 control_transfer=pipe@20-60 control_transfer=io@3 \
         get_string_descriptor_ascii=io init_context=no_mem set_configuration=io wrap_sys_device=io; do
  CHECK_RE='^RESULT mode=' run_api "api-shimfail-${f//[^a-z_0-9=-]/_}" $D $V "EMUVC_LIBUSB_FAIL=$f" -- -l all
done
# No device descriptor: libuvc finds no device. Mode by mode, as uvc_print_diag used to print an
# uninitialised pointer here.
run_api_robust api-shimfail-get_device_descriptor=io $D $V EMUVC_LIBUSB_FAIL=get_device_descriptor=io

echo "summary: $PASS passed, $FAIL failed, $XFAIL known libuvc bugs (XFAIL), ${#FIXED[@]} fixed upstream (XPASS)"
if [ ${#BUGS[@]} -gt 0 ]; then
  echo "known libuvc bugs:"; printf '  %s\n' "${BUGS[@]/|/: }"
fi
if [ ${#FIXED[@]} -gt 0 ]; then
  echo "fixed upstream? (XPASS: make these pass cases)"; printf '  %s\n' "${FIXED[@]/|/: }"
fi

# ---- Markdown report ----
# Exit status: 1 failed check, 2 setup error, 3 api_test watchdog (hang), 86 sanitizer error
# (scripts/libuvc-asan.sh), 124 timeout, 128+N signal N.
status() {
  case $1 in
    3) echo "3 (hang, watchdog)" ;; 86) echo "86 (sanitizer)" ;; 124) echo "124 (timeout)" ;; 134) echo "134 (SIGABRT)" ;;
    136) echo "136 (SIGFPE)" ;; 139) echo "139 (SIGSEGV)" ;; *) echo "$1" ;;
  esac
}
table() { # "name rc"...
  echo "| case | exit status |"; echo "|---|---|"
  local e; for e in "$@"; do echo "| \`${e% *}\` | $(status "${e##* }") |"; done
}
if [ -n "${LIBUVC_TEST_MARKDOWN:-}" ]; then
  {
    echo "### libuvc suite${LIBUVC_TEST_NAME:+ ($LIBUVC_TEST_NAME)}: $PASS passed, $FAIL failed, $XFAIL known libuvc bugs, ${#FIXED[@]} fixed upstream"
    echo
    echo "libuvc: $LIBUVC_DESC at \`$LIBUVC_COMMIT\`"
    echo
    if [ $FAIL -gt 0 ]; then echo "Failed cases:"; echo; table "${FAILED[@]}"; echo; fi
    bugs() { # "name|reference"...
      echo "| case | bug |"; echo "|---|---|"
      local e; for e in "$@"; do echo "| \`${e%%|*}\` | ${e#*|} |"; done
    }
    if [ ${#FIXED[@]} -gt 0 ]; then
      echo "Known libuvc bugs that pass now (XPASS, fixed upstream? make them pass cases):"
      echo; bugs "${FIXED[@]}"; echo
    fi
    if [ ${#BUGS[@]} -gt 0 ]; then
      echo "Known libuvc bugs, still failing (XFAIL):"; echo; bugs "${BUGS[@]}"; echo
    fi
  } >> "$LIBUVC_TEST_MARKDOWN"
fi
[ $FAIL -eq 0 ]
