#!/usr/bin/env bash
# The libuvc suite with libuvc and the test programs built with AddressSanitizer and
# UndefinedBehaviorSanitizer. A sanitizer report makes the case exit 86: a failure for
# `pass` cases, reported like a crash for `info` ones. The emuvc shim is not instrumented.
# usage: scripts/libuvc-asan.sh [libuvc-test.sh args...]
#   ASAN_OPTIONS / UBSAN_OPTIONS are appended to the defaults below (later
#   options win), e.g. ASAN_OPTIONS=detect_leaks=0, or fast_unwind_on_malloc=0 for leak stacks
#   that go through the (Rust) shim.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SAN="-fsanitize=address,undefined -fno-omit-frame-pointer"
export LIBUVC_BUILD="${LIBUVC_BUILD:-$ROOT/target/libuvc-asan}"
# CMake reads CFLAGS only when it creates the cache (see libuvc-coverage.sh).
export CFLAGS="$SAN"
grep -qxF "CMAKE_C_FLAGS:STRING=$CFLAGS" "$LIBUVC_BUILD/CMakeCache.txt" 2>/dev/null \
  || rm -f "$LIBUVC_BUILD/CMakeCache.txt"
export LIBUVC_CMAKE_FLAGS="-DCMAKE_SHARED_LINKER_FLAGS=-fsanitize=address,undefined -DBUILD_EXAMPLE=OFF"
export LIBUVC_TEST_CFLAGS="$SAN"
# The ASan runtime must come first in the library list, before the preloaded shim.
LIBUVC_PRELOAD="$(${CC:-cc} -print-file-name=libasan.so)"
[ -f "$LIBUVC_PRELOAD" ] || { echo "libasan.so not found (install the compiler's ASan runtime)" >&2; exit 2; }
export LIBUVC_PRELOAD
# Leaks count as sanitizer errors too.
export ASAN_OPTIONS="exitcode=86:detect_leaks=1${ASAN_OPTIONS:+:$ASAN_OPTIONS}"
export UBSAN_OPTIONS="halt_on_error=1:exitcode=86:print_stacktrace=1${UBSAN_OPTIONS:+:$UBSAN_OPTIONS}"
export LIBUVC_TEST_NAME="ASan+UBSan"
exec "$ROOT/scripts/libuvc-test.sh" "$@"
