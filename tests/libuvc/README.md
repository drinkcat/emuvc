# libuvc end-to-end tests

Runs libuvc (built out of tree, never modified) against the emuvc libusb shim
(`LD_PRELOAD=target/debug/libemuvc_libusb.so`); no real USB hardware is touched.

    scripts/libuvc-test.sh [-k PATTERN] [-v]

* `LIBUVC_SRC` selects the libuvc checkout (default `../libuvc`, else a shallow
  clone into `target/libuvc-src`). Needs cmake, a C compiler, libjpeg, libusb-1.0 headers.
* `stream_test.c` is the client: it streams, decodes the frame stamp
  (`crates/emuvc-video/src/stamp.rs`) and reports
  `RESULT frames=.. corrupt=.. gaps=.. max_gap=.. fps=..` (exit 0 ok, 1 failed, 2 setup/usage error).
  `--controls` exercises brightness/AE mode; `--list` lists devices; `--fps-tol PCT` checks the rate.
* `configs/*.toml` are emulator configs for the fault/quirk cases. `info` cases
  (zero-max-payload, stall-get-def) only report an outcome and never fail the suite.

## API coverage (`api_test.c`) and `scripts/libuvc-coverage.sh`

`api_test [-d VID:PID] [-n INDEX] [-m MS] [-l] MODE...` exercises libuvc's public API
(modes: `info controls convert poll still multi status negotiate wrap`, or `all`). It discovers
what the device offers through libuvc itself (formats, units, extension units, still
patterns, streaming interfaces), so richer emulated devices raise coverage without code
changes. Calls a device cannot serve are counted as "soft"; failures are crashes/hangs
(45 s SIGALRM watchdog), inconsistent data, or an API accepting what it must reject.
`-l` (lenient) counts failed expectations as soft, for fault-injection configs. Each mode
prints `RESULT mode=.. ok=.. soft=.. fail=..`. Cases: `api-*` in `libuvc-test.sh`.

    scripts/libuvc-coverage.sh [--functions] [--uncovered FILE.c] [-k PATTERN] [-v]

builds an instrumented libuvc (`target/libuvc-cov`), clears stale `.gcda`, runs the suite
and prints the gcov summary (`scripts/gcov-summary.py`); the suite's exit status is
reported but the summary is printed regardless.

`api_test` includes libuvc's internal header (stream interfaces, claim, probe helpers) so it
needs the libuvc source tree, as `libuvc-test.sh` already does.
