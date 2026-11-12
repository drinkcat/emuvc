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
