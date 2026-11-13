#!/usr/bin/env bash
# End-to-end libuvc tests against the emuvc libusb shim. See tests/libuvc/README.md.
# usage: scripts/libuvc-test.sh [-k PATTERN] [-v]
#   Tests upstream libuvc master, fetched into target/libuvc-src on every run.
#   LIBUVC_REPO / LIBUVC_REF select another repository or branch, tag or ref
#   (e.g. LIBUVC_REF=pull/336/head); LIBUVC_SRC=DIR tests a local checkout as is.
#   LIBUVC_TEST_MARKDOWN=FILE appends a Markdown report (e.g. $GITHUB_STEP_SUMMARY).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# LIBUVC_BUILD / LIBUVC_CMAKE_FLAGS select another libuvc build (e.g. with
# coverage instrumentation, see scripts/libuvc-coverage.sh).
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
# shellcheck disable=SC2046
cc -g -O1 -Wall -Wextra -o "$OUT/stream_test" "$ROOT/tests/libuvc/stream_test.c" \
  -I"$LIBUVC_SRC/include" -I"$BUILD/include" -L"$BUILD" -luvc -Wl,-rpath,"$BUILD" \
  $(pkg-config --cflags --libs libjpeg libusb-1.0) -lpthread
# api_test uses libuvc internals (libuvc_internal.h) and libusb directly
# shellcheck disable=SC2046
cc -g -O1 -Wall -Wextra -o "$OUT/api_test" "$ROOT/tests/libuvc/api_test.c" \
  -I"$LIBUVC_SRC/include" -I"$BUILD/include" -L"$BUILD" -luvc -Wl,-rpath,"$BUILD" \
  $(pkg-config --cflags --libs libjpeg libusb-1.0) -lpthread

# ---- runner ----
PASS=0; FAIL=0; INFO=0
FAILED=(); NOTABLE=() # "name rc" of failed cases, and of informational cases that did not exit 0
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
  env LD_PRELOAD="$SHIM" EMUVC_LOG="${EMUVC_LOG:-emuvc_uvc::stream=info}" "${envs[@]}" timeout 60 "$OUT/${TEST_BIN:-stream_test}" "$@" >"$TMP" 2>&1 || rc=$?
  local secs=$((SECONDS - t0)) ok=1
  [ $rc -eq 0 ] || ok=0
  if [ -n "${CHECK_RE:-}" ] && ! grep -Eq "$CHECK_RE" "$TMP"; then ok=0; echo "(missing output /$CHECK_RE/)" >>"$TMP"; fi
  local summary; summary=$({ grep -E '^RESULT|^devices:' "$TMP" || true; } | tr '\n' ' ')
  if [ "$expect" = info ]; then
    INFO=$((INFO + 1)); echo "INFO $name rc=$rc (${secs}s) $summary"
    [ $rc -eq 0 ] || NOTABLE+=("$name $rc")
  elif [ $ok = 1 ]; then
    PASS=$((PASS + 1)); echo "PASS $name (${secs}s) $summary"
  else
    FAIL=$((FAIL + 1)); echo "FAIL $name rc=$rc (${secs}s) $summary"
    FAILED+=("$name $rc")
  fi
  if [ $VERBOSE = 1 ]; then sed 's/^/    | /' "$TMP"
  elif [ "$expect" = pass ] && [ $ok = 0 ]; then tail -15 "$TMP" | sed 's/^/    | /'
  elif [ "$expect" = info ] && [ "${INFO_LINES:-4}" != 0 ]; then { grep -vE '^RESULT' "$TMP" || true; } | { if [ "$name" = stall-get-def ]; then grep -E 'FAIL|GET_DEF'; else cat; fi; } | tail -${INFO_LINES:-4} | sed 's/^/    | /'; fi
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

# Issue reproductions: libuvc is expected to fail on some of these, so every mode runs in its own
# process and only the outcome is reported (rc=139/136 = libuvc crashed, a finding, never a hang).
API_MODES="info controls convert poll still multi status negotiate wrap"
run_api_each() { # NAME [VAR=val ...] -- (modes run one by one)
  local name=$1; shift
  for m in $API_MODES; do
    INFO_LINES=0 TEST_BIN=api_test run_case "$name/$m" info "$@" -- -l "$m"
  done
}
for c in "$CFG"/issue-*.toml; do
  run_api_each "api-$(basename "$c" .toml)" EMUVC_CONFIG="$c" $V
done

# Shim-level libusb failures (EMUVC_LIBUSB_FAIL): libuvc's error paths must neither crash nor hang.
# FAULT[:mode] ; default mode is every mode in one process (lenient).
for f in get_config_descriptor=io get_active_config_descriptor=io open=access \
         claim_interface=busy detach_kernel_driver=io kernel_driver_active=io set_interface_alt_setting=io \
         clear_halt=io submit_transfer=no_mem@3 alloc_transfer=no_mem@5 alloc_transfer=no_mem \
         control_transfer=pipe@5 control_transfer=pipe@20-60 control_transfer=io@3 \
         get_string_descriptor_ascii=io init_context=no_mem set_configuration=io wrap_sys_device=io; do
  CHECK_RE='^RESULT mode=' run_api "api-shimfail-${f//[^a-z_0-9=-]/_}" $D $V "EMUVC_LIBUSB_FAIL=$f" -- -l all
done
# finding: uvc_print_diag ignores a uvc_get_device_descriptor failure and prints an uninitialised pointer
run_api_each api-shimfail-get_device_descriptor=io $D $V EMUVC_LIBUSB_FAIL=get_device_descriptor=io

echo "summary: $PASS passed, $FAIL failed, $INFO informational"

# ---- Markdown report ----
# Exit status: 1 failed check, 2 setup error, 3 api_test watchdog (hang), 124 timeout, 128+N signal N.
status() {
  case $1 in
    3) echo "3 (hang, watchdog)" ;; 124) echo "124 (timeout)" ;; 134) echo "134 (SIGABRT)" ;;
    136) echo "136 (SIGFPE)" ;; 139) echo "139 (SIGSEGV)" ;; *) echo "$1" ;;
  esac
}
table() { # "name rc"...
  echo "| case | exit status |"; echo "|---|---|"
  local e; for e in "$@"; do echo "| \`${e% *}\` | $(status "${e##* }") |"; done
}
if [ -n "${LIBUVC_TEST_MARKDOWN:-}" ]; then
  {
    echo "### libuvc suite: $PASS passed, $FAIL failed, $INFO informational"
    echo
    echo "libuvc: $LIBUVC_DESC at \`$LIBUVC_COMMIT\`"
    echo
    if [ $FAIL -gt 0 ]; then echo "Failed cases:"; echo; table "${FAILED[@]}"; echo; fi
    if [ ${#NOTABLE[@]} -gt 0 ]; then
      echo "<details><summary>${#NOTABLE[@]} informational cases did not exit 0 (libuvc bugs and fault scenarios)</summary>"
      echo; table "${NOTABLE[@]}"; echo; echo "</details>"; echo
    fi
  } >> "$LIBUVC_TEST_MARKDOWN"
fi
[ $FAIL -eq 0 ]
