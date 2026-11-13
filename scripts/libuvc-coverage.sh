#!/usr/bin/env bash
# libuvc code coverage under the emuvc shim: builds an instrumented libuvc, runs the
# whole libuvc-test.sh suite and prints a gcov summary (also when the suite fails;
# the exit status is the suite's).
# usage: scripts/libuvc-coverage.sh [--functions] [--uncovered FILE.c] [--uncovered-branches FILE.c]
#                                   [--markdown FILE] [--html DIR] [-k PATTERN] [-v]
#   --markdown FILE  append suite results and the coverage table to FILE (e.g. $GITHUB_STEP_SUMMARY)
#   --html DIR       annotated-source HTML report (needs gcovr)
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SUMMARY_ARGS=(); TEST_ARGS=(); HTML=""
usage() { sed -n 's/^# \{0,1\}//; 5,10p' "$0" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case $1 in
    --functions) SUMMARY_ARGS+=(--functions) ;;
    --uncovered) SUMMARY_ARGS+=(--uncovered "${2:?--uncovered needs FILE.c}"); shift ;;
    --uncovered-branches) SUMMARY_ARGS+=(--uncovered-branches "${2:?--uncovered-branches needs FILE.c}"); shift ;;
    --markdown) export LIBUVC_TEST_MARKDOWN="${2:?--markdown needs FILE}"; SUMMARY_ARGS+=(--markdown "$2"); shift ;;
    --html) HTML="${2:?--html needs DIR}"; shift ;;
    -k) TEST_ARGS+=(-k "${2:?-k needs PATTERN}"); shift ;;
    -v) TEST_ARGS+=(-v) ;;
    *) usage ;;
  esac
  shift
done
[ -z "$HTML" ] || command -v gcovr >/dev/null || { echo "--html needs gcovr" >&2; exit 2; }
export LIBUVC_BUILD="${LIBUVC_BUILD:-$ROOT/target/libuvc-cov}"
# Condition (MC/DC) coverage too when the compiler has it (gcc 14+). The flags go through
# CFLAGS (LIBUVC_CMAKE_FLAGS is split on spaces), which CMake reads only when it creates
# the cache: start over when the cached flags differ.
export CFLAGS="--coverage"
echo 'int main(void) { return 0; }' | ${CC:-cc} -fcondition-coverage -fsyntax-only -x c - 2>/dev/null \
  && CFLAGS="--coverage -fcondition-coverage"
grep -qxF "CMAKE_C_FLAGS:STRING=$CFLAGS" "$LIBUVC_BUILD/CMakeCache.txt" 2>/dev/null \
  || rm -f "$LIBUVC_BUILD/CMakeCache.txt"
export LIBUVC_CMAKE_FLAGS="-DCMAKE_SHARED_LINKER_FLAGS=--coverage -DCMAKE_EXE_LINKER_FLAGS=--coverage -DBUILD_EXAMPLE=OFF"
OBJ="$LIBUVC_BUILD/CMakeFiles/uvc.dir/src"
find "$LIBUVC_BUILD" -name '*.gcda' -delete 2>/dev/null
"$ROOT/scripts/libuvc-test.sh" "${TEST_ARGS[@]}"
rc=$?
echo "suite exit status: $rc"
fail() { [ "$rc" -ne 0 ] || rc=2; } # keep the suite status if it failed
[ -d "$OBJ" ] || { fail; exit "$rc"; }
python3 -I "$ROOT/scripts/gcov-summary.py" "$OBJ" "${SUMMARY_ARGS[@]}" || fail
if [ -n "$HTML" ]; then
  SRC=$(sed -n 's/^CMAKE_HOME_DIRECTORY:INTERNAL=//p' "$LIBUVC_BUILD/CMakeCache.txt")
  mkdir -p "$HTML"
  gcovr --root "$SRC" --filter "$SRC/src/" --html-details "$HTML/index.html" \
    --html-title "libuvc coverage (emuvc suite)" "$LIBUVC_BUILD" || fail
  echo "HTML report: $HTML/index.html"
fi
exit "$rc"
