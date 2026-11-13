#!/usr/bin/env bash
# libuvc code coverage under the emuvc shim: builds an instrumented libuvc, runs the
# whole libuvc-test.sh suite and prints a gcov summary.
# usage: scripts/libuvc-coverage.sh [--functions] [--uncovered FILE.c] [-k PATTERN] [-v]
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SUMMARY_ARGS=(); TEST_ARGS=()
while [ $# -gt 0 ]; do
  case $1 in
    --functions) SUMMARY_ARGS+=(--functions) ;;
    --uncovered) SUMMARY_ARGS+=(--uncovered "${2:?--uncovered needs FILE.c}"); shift ;;
    -k) TEST_ARGS+=(-k "${2:?-k needs PATTERN}"); shift ;;
    -v) TEST_ARGS+=(-v) ;;
    *) echo "usage: $0 [--functions] [--uncovered FILE.c] [-k PATTERN] [-v]" >&2; exit 2 ;;
  esac
  shift
done
export LIBUVC_BUILD="${LIBUVC_BUILD:-$ROOT/target/libuvc-cov}"
export LIBUVC_CMAKE_FLAGS="-DCMAKE_C_FLAGS=--coverage -DCMAKE_SHARED_LINKER_FLAGS=--coverage -DCMAKE_EXE_LINKER_FLAGS=--coverage -DBUILD_EXAMPLE=OFF"
OBJ="$LIBUVC_BUILD/CMakeFiles/uvc.dir/src"
find "$LIBUVC_BUILD" -name '*.gcda' -delete 2>/dev/null
"$ROOT/scripts/libuvc-test.sh" "${TEST_ARGS[@]}"
rc=$?
echo "suite exit status: $rc"
python3 -I "$ROOT/scripts/gcov-summary.py" "$OBJ" "${SUMMARY_ARGS[@]}"
exit 0
