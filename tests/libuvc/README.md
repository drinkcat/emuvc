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
  `RESULT frames=.. corrupt=.. gaps=.. max_gap=.. fps=..` (exit 0 ok, 1 failed, 2 setup/usage error).
  `--controls` exercises brightness/AE mode; `--list` lists devices; `--fps-tol PCT` checks the rate;
  `--max-gaps N` tolerates N skipped frames (`--allow-gaps`: any number).
* `configs/*.toml` are emulator configs for the fault/quirk cases. `info` cases
  (zero-max-payload) only report an outcome and never fail the suite. `EXPECT_RC=N` makes a
  `pass` case expect exit N, for checked failures (stall-get-def: libuvc must report PIPE).

## API coverage (`api_test.c`) and `scripts/libuvc-coverage.sh`

`api_test [-d VID:PID] [-n INDEX] [-m MS] [-l] MODE...` exercises libuvc's public API
(modes: `info controls convert poll still multi status negotiate wrap`, or `all`). It discovers
what the device offers through libuvc itself (formats, units, extension units, still
patterns, streaming interfaces), so richer emulated devices raise coverage without code
changes. Calls a device cannot serve are counted as "soft"; failures are crashes/hangs
(45 s SIGALRM watchdog), inconsistent data, or an API accepting what it must reject.
`-l` (lenient) counts failed expectations as soft, for fault-injection configs. Each mode
prints `RESULT mode=.. ok=.. soft=.. fail=..`. Cases: `api-*` in `libuvc-test.sh`.

    scripts/libuvc-coverage.sh [--functions] [--uncovered FILE.c] [--markdown FILE] [--html DIR] [-k PATTERN] [-v]

builds an instrumented libuvc (`target/libuvc-cov`), clears stale `.gcda`, runs the suite
and prints the gcov summary (`scripts/gcov-summary.py`), also when the suite fails; the exit
status is the suite's. `--markdown FILE` appends the suite report and the coverage table (CI
uses the job summary), `--html DIR` writes annotated sources with gcovr.

`api_test` includes libuvc's internal header (stream interfaces, claim, probe helpers) so it
needs the libuvc source tree, as `libuvc-test.sh` already does.
