# libuvc end-to-end tests

Runs libuvc (built out of tree, never modified) against the emuvc libusb shim
(`LD_PRELOAD=target/debug/libemuvc_libusb.so`); no real USB hardware is touched.

    scripts/libuvc-test.sh [-k PATTERN] [-v]

* Tests upstream libuvc master, fetched into `target/libuvc-src` on every run (offline: the copy
  fetched last). `LIBUVC_REPO` / `LIBUVC_REF` select another repository or branch, tag or ref
  (e.g. `LIBUVC_REF=pull/336/head`); `LIBUVC_SRC=DIR` tests a local checkout as is.
  Needs cmake, a C compiler, libjpeg, libusb-1.0 headers.
* `LIBUVC_TEST_MARKDOWN=FILE` appends a Markdown report: counts, failed cases, and the
  informational cases that did not exit 0 (crashes, hangs: mostly libuvc bugs).
* `stream_test.c` is the client: it streams, decodes the frame stamp
  (`crates/emuvc-video/src/stamp.rs`) and reports
  `RESULT frames=.. corrupt=.. gaps=.. max_gap=.. skipped=.. fps=..` (exit 0 ok, 1 failed, 2 setup/usage error).
  A gap is a frame lost before libuvc assembled it (device, transport, payload parsing); `skipped`
  counts frames libuvc assembled but overwrote before its callback thread ran (host scheduling,
  reported only). `frame->sequence` tells the two apart.
  `--controls` exercises brightness/AE mode; `--list` lists devices; `--fps-tol PCT` checks the rate;
  `--cb-sleep MS` makes the callback slow;
  `--max-gaps N` tolerates N gaps (`--allow-gaps`: any number).
* `configs/*.toml` are emulator configs for the fault/quirk cases. `info` cases only report an
  outcome and never fail the suite. `EXPECT_RC=N` makes a
  `pass` case expect exit N, for checked failures (stall-get-def: libuvc must report PIPE).
  `desc-*.toml` patch the synthetic camera's descriptors (`[[device.patch]]`) with a few structural
  errors a fuzzer rarely produces (unknown subtype, reordering, wrong counts, a duplicate frame);
  the device keeps working from its original descriptors.

## API coverage (`api_test.c`) and `scripts/libuvc-coverage.sh`

`api_test [-d VID:PID] [-n INDEX] [-m MS] [-l] MODE...` exercises libuvc's public API
(modes: `info controls convert poll still multi status negotiate wrap`, or `all`). It discovers
what the device offers through libuvc itself (formats, units, extension units, still
patterns, streaming interfaces), so richer emulated devices raise coverage without code
changes. Calls a device cannot serve are counted as "soft"; failures are crashes/hangs
(45 s SIGALRM watchdog), inconsistent data, or an API accepting what it must reject.
`controls` also holds the device to the spec rules it advertises. For each single-value camera terminal
and processing unit control it checks GET_INFO and the STALL with the right Request Error Code:
out of range (0x04) outside GET_MIN/GET_MAX, invalid value within range (0x08) off the GET_RES
grid, and wrong state (0x02) while an auto mode disables the control. It also checks that auto
switches (focus, white balance, hue, contrast, every AE mode in GET_RES) disable and re-enable
their manual controls. `still` checks that a method 2 trigger yields exactly one frame of the
still size before video frames resume.
`-l` (lenient) counts failed expectations as soft, for fault-injection configs. Each mode
prints `RESULT mode=.. ok=.. soft=.. fail=..`. Cases: `api-*` in `libuvc-test.sh`.

    scripts/libuvc-coverage.sh [--functions] [--uncovered FILE.c] [--markdown FILE] [--html DIR] [-k PATTERN] [-v]

builds an instrumented libuvc (`target/libuvc-cov`), clears stale `.gcda`, runs the suite
and prints the gcov summary (`scripts/gcov-summary.py`), also when the suite fails; the exit
status is the suite's. `--markdown FILE` appends the suite report and the coverage table (CI
uses the job summary), `--html DIR` writes annotated sources with gcovr.

`api_test` includes libuvc's internal header (stream interfaces, claim, probe helpers) so it
needs the libuvc source tree, as `libuvc-test.sh` already does.

## Sanitizers (`scripts/libuvc-asan.sh`)

    scripts/libuvc-asan.sh [-k PATTERN] [-v]

runs the same suite with libuvc and the test programs built with ASan and UBSan
(`target/libuvc-asan`). The Rust shim is not instrumented, and libasan is preloaded ahead of
it. A sanitizer report, leaks included, makes a case exit 86: `pass` cases fail, `info` cases
list it as `86 (sanitizer)`. This is a separate build from the coverage one. Both sets of flags work together,
but a case stopped by a sanitizer never writes its `.gcda`, and the ASan slowdown does not
belong in the coverage run. Leak stacks stop at the shim's frames unless
`ASAN_OPTIONS=fast_unwind_on_malloc=0` is set.
