# libuvc end-to-end tests

Runs libuvc (built out of tree, never modified) against the emuvc libusb shim
(`LD_PRELOAD=target/debug/libemuvc_libusb.so`); no real USB hardware is touched.

    scripts/libuvc-test.sh [-k PATTERN] [-v]

* Tests upstream libuvc master, fetched into `target/libuvc-src` on every run (offline: the copy
  fetched last). `LIBUVC_REPO` / `LIBUVC_REF` select another repository or branch, tag or ref
  (e.g. `LIBUVC_REF=pull/336/head`); `LIBUVC_SRC=DIR` tests a local checkout as is.
  Needs cmake, a C compiler, libjpeg, libusb-1.0 headers.
* Every case has one of three expected outcomes:
  * `pass`: must work. `FAIL` is a regression, in emuvc or in libuvc.
  * `robust`: a device too broken to work, or an injected fault. libuvc may fail, but only
    cleanly. A crash, hang or sanitizer report is a `FAIL`.
  * `bug`: a known libuvc bug, with a reference (`BUG=`). The case checks the correct behaviour.
    Today it reports `XFAIL` and doesn't fail the suite. Once libuvc gets it right, it reports
    `XPASS`, and the case should become `pass`.

  The summary ends with the list of known libuvc bugs: what should be fixed in libuvc. In GitHub
  Actions, every XPASS and FAIL is also an annotation on the run and the pull request.
* `LIBUVC_TEST_MARKDOWN=FILE` appends a Markdown report: counts, failed cases, and the known
  libuvc bugs (still failing, or passing now).
* `stream_test.c` is the client: it streams, decodes the frame stamp
  (`crates/emuvc-video/src/stamp.rs`) and reports
  `RESULT frames=.. corrupt=.. gaps=.. max_gap=.. skipped=.. fps=..` (exit 0 ok, 1 failed, 2 setup/usage error).
  A gap is a frame lost before libuvc assembled it (device, transport, payload parsing); `skipped`
  counts frames libuvc assembled but overwrote before its callback thread ran (host scheduling,
  reported only). `frame->sequence` tells the two apart.
  `--controls` exercises brightness/AE mode; `--list` lists devices; `--fps-tol PCT` checks the rate;
  `--cb-sleep MS` makes the callback slow;
  `--max-gaps N` tolerates N gaps (`--allow-gaps`: any number).
* `configs/*.toml` are emulator configs for the fault/quirk cases. `EXPECT_RC=N` makes a
  `pass` case expect exit N, for checked failures (stall-get-def: libuvc must report PIPE).
  `issue-*.toml` reproduce libuvc issues: libuvc handles some now (`pass`), some devices are
  too broken to stream (`robust`), and the rest are known bugs (`bug`).
  `desc-*.toml` patch the synthetic camera's descriptors (`[[device.patch]]`) with a few structural
  errors a fuzzer rarely produces (unknown subtype, reordering, wrong counts, a duplicate frame);
  the device keeps working from its original descriptors.
  `pts-first-only.toml` and `clock-wrap-jitter.toml` exercise payload header timestamps
  (`uvc.clock`). The first shortens the header after a frame's first payload. The second makes the
  PTS/SCR clock wrap, drift, jitter and step back. libuvc parses PTS/SCR without using them, and
  uvcvideo's hardware timestamps can't be checked over USB/IP: with `hwtimestamps=1`, even a
  well-behaved clock yields backwards buffer timestamps there.

## API coverage (`api_test.c`) and `scripts/libuvc-coverage.sh`

`api_test [-d VID:PID] [-n INDEX] [-m MS] [-l] MODE...` exercises libuvc's public API
(modes: `info controls convert poll still multi status async negotiate wrap`, or `all`). It discovers
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
`async` sets every asynchronous camera terminal or processing unit control (GET_INFO D4) and
expects the UVC 1.5 2.4.4 flow. The SET_CUR is accepted at once, and GET_CUR/SET_CUR on that
control STALL with "not ready" until the Control Change interrupt arrives. That interrupt carries
either the new value or a failure code. `configs/synthetic-async.toml` makes brightness slow and
contrast fail (`[[device.control]]`). `configs/status-edge.toml` sends malformed and reserved
status packets, then well-formed ones that must still reach the callbacks.
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
it. A sanitizer report, leaks included, makes a case exit 86, which fails it whatever its
expected outcome (unless `BUG_CRASH=1` marks a known bug as such a crash). This is a separate build from the coverage one. Both sets of flags work together,
but a case stopped by a sanitizer never writes its `.gcda`, and the ASan slowdown does not
belong in the coverage run. Leak stacks stop at the shim's frames unless
`ASAN_OPTIONS=fast_unwind_on_malloc=0` is set.
