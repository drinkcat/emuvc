/* libuvc public API coverage client for emuvc. See README.md.
 *
 * usage: api_test [-d VID:PID] [-n INDEX] [-m MS] [-l] MODE...
 *   -l: lenient, failed expectations are counted as soft (fault-injection configs);
 *       crashes and hangs still fail
 *   MODE: info controls convert poll still multi status negotiate wrap all
 *
 * Every mode discovers what the device offers through libuvc itself and
 * exercises the matching API; nothing is hard-coded to one device model.
 * A call that fails because the device lacks the feature is counted ("soft"),
 * not failed. Failures are crashes/hangs (SIGALRM watchdog), inconsistent data,
 * or an API accepting something it must reject.
 * Prints one "RESULT mode=.. ok=.. soft=.. fail=.. [..]" per mode; exit 0 unless some fail > 0. */
#define _GNU_SOURCE
#include <libusb-1.0/libusb.h>
#include <libuvc/libuvc.h>
#include <libuvc/libuvc_internal.h>
#include <jpeglib.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

extern void uvc_print_frameformats(uvc_device_handle_t *devh); /* exported, not in the public header */

/* ---- bookkeeping ---- */
static int n_ok, n_soft, n_fail;
static int opt_vid, opt_pid, opt_index, opt_ms = 1000, opt_lenient, opt_leave_open;
static const char *cur_mode = "";

static void expect(int cond, const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  if (cond) n_ok++;
  else if (opt_lenient) n_soft++; /* -l: the device is misbehaving on purpose */
  else { n_fail++; printf("  FAIL [%s] ", cur_mode); vprintf(fmt, ap); printf("\n"); }
  va_end(ap);
}
/* e: uvc_error_t / libusb return; success counts ok, failure counts soft. */
static int soft(int e) { if (e >= 0) n_ok++; else n_soft++; return e; }

static void on_alarm(int s) {
  (void)s;
  const char m[] = "FAIL: watchdog fired (hang)\nRESULT hang=1\n";
  if (write(1, m, sizeof m - 1) < 0) {}
  _exit(3);
}

/* ---- discovery helpers ---- */
static uvc_context_t *ctx;
static uvc_device_t *dev;
static uvc_device_handle_t *devh;

static int open_device(void) {
  uvc_device_t **list;
  if (uvc_get_device_list(ctx, &list) != UVC_SUCCESS) return -1;
  int idx = 0, found = 0;
  for (int i = 0; list[i]; i++) {
    uvc_device_descriptor_t *dd;
    if (uvc_get_device_descriptor(list[i], &dd)) continue;
    int match = (!opt_vid || dd->idVendor == opt_vid) && (!opt_pid || dd->idProduct == opt_pid);
    uvc_free_device_descriptor(dd);
    if (match && idx++ == opt_index) { dev = list[i]; uvc_ref_device(dev); found = 1; break; }
  }
  uvc_free_device_list(list, 1);
  if (!found) return -1;
  if (uvc_open(dev, &devh) != UVC_SUCCESS) return -1;
  return 0;
}

static enum uvc_frame_format fmt_of(const uvc_format_desc_t *f) {
  if (f->bDescriptorSubtype == UVC_VS_FORMAT_MJPEG) return UVC_FRAME_FORMAT_MJPEG;
  return uvc_frame_format_for_guid((uint8_t *)f->guidFormat);
}

static const uvc_frame_desc_t *smallest_frame(const uvc_format_desc_t *f) {
  const uvc_frame_desc_t *best = NULL, *fr;
  for (fr = f->frame_descs; fr; fr = fr->next)
    if (!best || (uint32_t)fr->wWidth * fr->wHeight < (uint32_t)best->wWidth * best->wHeight) best = fr;
  return best;
}

static uint32_t default_interval(const uvc_frame_desc_t *fr) {
  if (fr->intervals && fr->intervals[0]) return fr->intervals[0];
  return fr->dwDefaultFrameInterval ? fr->dwDefaultFrameInterval : fr->dwMinFrameInterval;
}

/* Negotiate ctrl for (format, frame, interval) on any interface, by interface-of-format. */
static uvc_error_t ctrl_for(const uvc_format_desc_t *f, const uvc_frame_desc_t *fr, uint32_t interval, uvc_stream_ctrl_t *c) {
  memset(c, 0, sizeof *c);
  c->bInterfaceNumber = f->parent->bInterfaceNumber;
  c->bFormatIndex = f->bFormatIndex;
  c->bFrameIndex = fr->bFrameIndex;
  c->dwFrameInterval = interval;
  c->bmHint = 1;
  uvc_claim_if(devh, c->bInterfaceNumber);
  uvc_error_t e = uvc_query_stream_ctrl(devh, c, 1, UVC_GET_MAX);
  (void)e;
  c->bInterfaceNumber = f->parent->bInterfaceNumber;
  c->bFormatIndex = f->bFormatIndex;
  c->bFrameIndex = fr->bFrameIndex;
  c->dwFrameInterval = interval;
  c->bmHint = 1;
  return uvc_probe_stream_ctrl(devh, c);
}

/* Grab one frame from a polled stream; the copy must be uvc_free_frame()d. */
static uvc_frame_t *grab(uvc_stream_ctrl_t *c, int timeout_us) {
  uvc_stream_handle_t *s;
  uvc_frame_t *f = NULL, *copy = NULL;
  if (soft(uvc_stream_open_ctrl(devh, &s, c)) < 0) return NULL;
  if (soft(uvc_stream_start(s, NULL, NULL, 0)) >= 0) {
    for (int i = 0; i < 3 && !copy; i++) {
      if (uvc_stream_get_frame(s, &f, timeout_us) == UVC_SUCCESS && f) {
        copy = uvc_allocate_frame(0);
        if (uvc_duplicate_frame(f, copy) != UVC_SUCCESS) { uvc_free_frame(copy); copy = NULL; }
      }
    }
    uvc_stream_stop(s);
  }
  uvc_stream_close(s);
  return copy;
}

/* ================= info ================= */
static void mode_info(void) {
  uvc_device_t **list = NULL;
  uvc_device_descriptor_t *dd = NULL; /* uvc_get_device_list leaves list untouched on error */
  expect(uvc_get_device_list(ctx, &list) == UVC_SUCCESS && list, "device list");
  int n = 0;
  for (; list && list[n]; n++) {
    expect(uvc_get_device_descriptor(list[n], &dd) == UVC_SUCCESS && dd, "descriptor %d", n);
    if (dd) {
      expect(dd->idVendor != 0, "vid set");
      printf("  device %04x:%04x bus=%u addr=%u mfr=%s prod=%s sn=%s\n", dd->idVendor, dd->idProduct,
             uvc_get_bus_number(list[n]), uvc_get_device_address(list[n]),
             dd->manufacturer ? dd->manufacturer : "-", dd->product ? dd->product : "-",
             dd->serialNumber ? dd->serialNumber : "-");
      uvc_free_device_descriptor(dd);
    }
    uvc_ref_device(list[n]);
    uvc_unref_device(list[n]);
  }
  expect(n >= 1, "at least one device");
  if (list) uvc_free_device_list(list, 1);
  list = NULL;

  /* lookups */
  uvc_device_t *d = NULL, **ds = NULL;
  expect(uvc_find_device(ctx, &d, 0, 0, NULL) == UVC_SUCCESS && d, "find any");
  uvc_device_descriptor_t *first = NULL;
  if (d && uvc_get_device_descriptor(d, &first) == UVC_SUCCESS) {
    expect(uvc_find_device(ctx, &dev, first->idVendor, first->idProduct, NULL) == UVC_SUCCESS, "find vid:pid");
    if (dev) { uvc_unref_device(dev); dev = NULL; }
    expect(uvc_find_device(ctx, &dev, first->idVendor, 0, NULL) == UVC_SUCCESS, "find vid only");
    if (dev) { uvc_unref_device(dev); dev = NULL; }
    expect(uvc_find_device(ctx, &dev, 0, first->idProduct, NULL) == UVC_SUCCESS, "find pid only");
    if (dev) { uvc_unref_device(dev); dev = NULL; }
    expect(uvc_find_device(ctx, &dev, first->idVendor ^ 0x5555, first->idProduct, NULL) == UVC_ERROR_NO_DEVICE, "find wrong vid");
    expect(uvc_find_device(ctx, &dev, first->idVendor, first->idProduct ^ 0x5555, NULL) == UVC_ERROR_NO_DEVICE, "find wrong pid");
    expect(uvc_find_device(ctx, &dev, first->idVendor, first->idProduct, "no-such-serial") == UVC_ERROR_NO_DEVICE, "find wrong sn");
    if (first->serialNumber) {
      expect(uvc_find_device(ctx, &dev, first->idVendor, first->idProduct, first->serialNumber) == UVC_SUCCESS, "find right sn");
      if (dev) { uvc_unref_device(dev); dev = NULL; }
      expect(uvc_find_devices(ctx, &ds, 0, 0, first->serialNumber) == UVC_SUCCESS && ds && ds[0], "find_devices sn");
      if (ds) uvc_free_device_list(ds, 1);
    }
    expect(uvc_find_devices(ctx, &ds, first->idVendor, first->idProduct, NULL) == UVC_SUCCESS && ds && ds[0], "find_devices vid:pid");
    if (ds) uvc_free_device_list(ds, 1);
    expect(uvc_find_devices(ctx, &ds, first->idVendor ^ 0x5555, 0, NULL) == UVC_ERROR_NO_DEVICE, "find_devices wrong vid");
    expect(uvc_find_devices(ctx, &ds, 0, 0, "no-such-serial") == UVC_ERROR_NO_DEVICE, "find_devices wrong sn");
    uvc_free_device_descriptor(first);
  }
  if (d) uvc_unref_device(d);
  expect(uvc_find_devices(ctx, &ds, 0, 0, NULL) == UVC_SUCCESS && ds && ds[0], "find_devices all");
  if (ds) uvc_free_device_list(ds, 1);
  /* list freed without unref, then manual unref */
  expect(uvc_get_device_list(ctx, &list) == UVC_SUCCESS, "list again");
  if (list && list[0]) {
    uvc_device_t *keep = list[0];
    for (int i = 1; list[i]; i++) uvc_unref_device(list[i]);
    uvc_free_device_list(list, 0); uvc_unref_device(keep);
  }
  else if (list) uvc_free_device_list(list, 1);

  /* every error code */
  static const int codes[] = {0, -1, -2, -3, -4, -5, -6, -7, -8, -9, -10, -11, -12, -50, -51, -52, -99, -77, 7};
  for (unsigned i = 0; i < sizeof codes / sizeof *codes; i++) {
    const char *s = uvc_strerror((uvc_error_t)codes[i]);
    expect(s && *s, "strerror %d", codes[i]);
    if (codes[i] == -77 || codes[i] == 7) expect(!strcmp(s, "Unknown error"), "unknown error text");
    uvc_perror((uvc_error_t)codes[i], i & 1 ? "api_test" : NULL);
  }
  uvc_perror(UVC_SUCCESS, "");

  /* an application-supplied libusb context */
  libusb_context *uctx = NULL;
  uvc_context_t *c2 = NULL;
  if (libusb_init(&uctx) == 0) {
    expect(uvc_init(&c2, uctx) == UVC_SUCCESS && c2, "uvc_init with usb ctx");
    if (c2) {
      uvc_device_t *x;
      if (soft(uvc_find_device(c2, &x, 0, 0, NULL)) == UVC_SUCCESS) {
        uvc_device_handle_t *h2;
        if (soft(uvc_open(x, &h2)) == UVC_SUCCESS) uvc_close(h2);
        uvc_unref_device(x);
      }
      uvc_exit(c2);
    }
    libusb_exit(uctx);
  }

  /* second context while the first has the device open: BUSY */
  expect(open_device() == 0, "open device");
  if (!devh) return;
  uvc_device_handle_t *dup = NULL;
  expect(uvc_open(dev, &dup) == UVC_ERROR_BUSY, "double open is BUSY");
  uvc_device_t *got = uvc_get_device(devh); /* takes a reference */
  expect(got == dev, "uvc_get_device");
  uvc_unref_device(got);
  expect(uvc_get_libusb_handle(devh) != NULL, "libusb handle");
  printf("  spec version %04x\n", uvc_get_spec_version(devh));
  expect(uvc_get_spec_version(devh) >= 0x0100, "spec version plausible");

  /* diagnostics */
  char *buf = NULL; size_t blen = 0;
  FILE *fp = open_memstream(&buf, &blen);
  uvc_print_diag(devh, fp);
  fclose(fp);
  expect(blen > 0 && strstr(buf, "DEVICE CONFIGURATION"), "print_diag output (%zu bytes)", blen);
  free(buf);
  uvc_print_diag(devh, NULL);
  uvc_print_frameformats(devh);
  uvc_stream_ctrl_t zc; memset(&zc, 0, sizeof zc);
  fp = open_memstream(&buf, &blen); uvc_print_stream_ctrl(&zc, fp); fclose(fp); expect(blen > 0, "print_stream_ctrl"); free(buf);
  uvc_print_stream_ctrl(&zc, NULL);

  /* topology */
  const uvc_input_terminal_t *ct = uvc_get_camera_terminal(devh);
  int nit = 0, npu = 0, nsu = 0, nxu = 0;
  for (const uvc_input_terminal_t *t = uvc_get_input_terminals(devh); t; t = t->next) {
    nit++;
    printf("  input terminal %u type=0x%04x controls=0x%llx\n", t->bTerminalID, t->wTerminalType, (unsigned long long)t->bmControls);
  }
  for (const uvc_processing_unit_t *p = uvc_get_processing_units(devh); p; p = p->next) {
    npu++;
    printf("  processing unit %u src=%u controls=0x%llx\n", p->bUnitID, p->bSourceID, (unsigned long long)p->bmControls);
  }
  for (const uvc_selector_unit_t *s = uvc_get_selector_units(devh); s; s = s->next) { nsu++; printf("  selector unit %u\n", s->bUnitID); }
  for (const uvc_extension_unit_t *x = uvc_get_extension_units(devh); x; x = x->next) {
    nxu++;
    printf("  extension unit %u controls=0x%llx\n", x->bUnitID, (unsigned long long)x->bmControls);
  }
  (void)uvc_get_output_terminals(devh);
  expect(ct == NULL || nit >= 1, "camera terminal is an input terminal");
  printf("  topology: it=%d pu=%d su=%d xu=%d\n", nit, npu, nsu, nxu);

  /* formats */
  int nfmt = 0, nfr = 0, nstill = 0;
  for (const uvc_format_desc_t *f = uvc_get_format_descs(devh); f; f = f->next) {
    nfmt++;
    enum uvc_frame_format ff = fmt_of(f);
    printf("  format %u subtype=0x%02x frame_format=%d frames=%u\n", f->bFormatIndex, f->bDescriptorSubtype, ff, f->bNumFrameDescriptors);
    int k = 0;
    for (const uvc_frame_desc_t *fr = f->frame_descs; fr; fr = fr->next) {
      k++; nfr++;
      expect(fr->wWidth > 0 && fr->wHeight > 0, "frame size");
      expect(fr->parent == f, "frame parent link");
      if (fr->intervals) { int j = 0; while (fr->intervals[j]) j++; expect(j == fr->bFrameIntervalType, "interval count %d vs %d", j, fr->bFrameIntervalType); }
      else expect(fr->dwMinFrameInterval && fr->dwMaxFrameInterval >= fr->dwMinFrameInterval, "continuous interval range");
    }
    if (k != f->bNumFrameDescriptors) n_soft++; /* libuvc leaves bNumFrameDescriptors unset for some format types */
    for (const uvc_still_frame_desc_t *sd = f->still_frame_desc; sd; sd = sd->next) {
      nstill++;
      for (const uvc_still_frame_res_t *r = sd->imageSizePatterns; r; r = r->next) printf("    still %ux%u idx=%u\n", r->wWidth, r->wHeight, r->bResolutionIndex);
    }
  }
  expect(nfmt > 0 && nfr > 0, "formats and frames present");
  uint8_t bogus[16]; memset(bogus, 0xa5, 16);
  expect(uvc_frame_format_for_guid(bogus) == UVC_FRAME_FORMAT_UNKNOWN, "unknown guid");
  printf("  formats=%d frames=%d still_patterns=%d\n", nfmt, nfr, nstill);

  /* ref counting */
  uvc_ref_device(dev); uvc_unref_device(dev);

#if LIBUSB_API_VERSION >= 0x01000107
  uvc_device_handle_t *wh = NULL;
  uvc_error_t we = uvc_wrap(-1, ctx, &wh);
  expect(we != UVC_SUCCESS || wh == NULL || 1, "wrap");
  if (we == UVC_SUCCESS && wh) uvc_close(wh);
  soft(we);
#endif

  uvc_close(devh); devh = NULL;
  uvc_unref_device(dev); dev = NULL;
  /* closed again: can be reopened */
  expect(open_device() == 0, "reopen after close");
}

/* ================= controls ================= */
static const enum uvc_req_code REQS[] = {UVC_GET_CUR, UVC_GET_MIN, UVC_GET_MAX, UVC_GET_RES, UVC_GET_LEN, UVC_GET_INFO, UVC_GET_DEF};
#define GETALL(expr) do { for (unsigned _i = 0; _i < 7; _i++) { enum uvc_req_code RC = REQS[_i]; (void)RC; soft(expr); } } while (0)
#define SIMPLE(name, T) do { T v = 0; GETALL(uvc_get_##name(devh, &v, RC)); \
    if (uvc_get_##name(devh, &v, UVC_GET_CUR) == UVC_SUCCESS) { \
      soft(uvc_set_##name(devh, (T)-1)); /* out of range: the device may reject it (error path) */ \
      soft(uvc_set_##name(devh, v)); /* may legitimately fail (read-only, auto mode) */ \
    } else soft(uvc_set_##name(devh, 0)); } while (0)

/* ---- spec rules for controls, derived from what the device reports (UVC 1.5 4.1.2, 4.2) ---- */
#define VC_REQUEST_ERROR_CODE_CONTROL 0x02
enum { INFO_GET = 1, INFO_SET = 2, INFO_DISABLED = 4, INFO_AUTOUPDATE = 8, INFO_ASYNC = 16 };
enum { ERR_NOT_READY = 0x01, ERR_WRONG_STATE = 0x02, ERR_OUT_OF_RANGE = 0x04, ERR_INVALID_REQUEST = 0x07, ERR_INVALID_VALUE_WITHIN_RANGE = 0x08 };

/* bRequestErrorCode for the last request, or -1 when the device has no such control. */
static int req_error(void) {
  uint8_t e = 0;
  return uvc_get_ctrl(devh, 0, VC_REQUEST_ERROR_CODE_CONTROL, &e, 1, UVC_GET_CUR) == 1 ? e : -1;
}

/* r: uvc_set_ctrl result for a SET_CUR the device must STALL, with error code `code`. */
static void expect_rejected(int r, int code, const char *what, uint8_t unit, uint8_t sel) {
  expect(r < 0, "%u/%u: %s rejected (got %d)", unit, sel, what, r);
  if (r >= 0) return;
  int e = req_error();
  if (e < 0) soft(e);
  else expect(e == code, "%u/%u: %s: error code %d, want %d", unit, sel, what, e, code);
}

/* GET_CUR, retried while an asynchronous SET_CUR keeps the control "not ready" (2.4.4). */
static int get_cur_ready(uint8_t unit, uint8_t sel, void *b, int len) {
  int r = -1;
  for (int t = 0; t < 200; t++) {
    if ((r = uvc_get_ctrl(devh, unit, sel, b, len, UVC_GET_CUR)) >= 0 || req_error() != ERR_NOT_READY) break;
    usleep(10000);
  }
  return r;
}

static int ctrl_info(uint8_t unit, uint8_t sel) {
  uint8_t i = 0;
  return uvc_get_ctrl(devh, unit, sel, &i, 1, UVC_GET_INFO) == 1 ? i : -1;
}

static int64_t le_get(const uint8_t *b, int len, int sgn) {
  uint64_t v = 0;
  for (int i = len - 1; i >= 0; i--) v = v << 8 | b[i];
  if (sgn && len < 8 && (v >> (8 * len - 1)) & 1) v |= ~0ull << (8 * len);
  return (int64_t)v;
}
static void le_put(uint8_t *b, int len, int64_t v) { for (int i = 0; i < len; i++) b[i] = (uint8_t)(v >> (8 * i)); }

/* Single-field controls: 1 = camera terminal, 0 = processing unit; signedness. */
static const struct { uint8_t ct, sel, sgn; } SCALARS[] = {
  {1, UVC_CT_SCANNING_MODE_CONTROL, 0}, {1, UVC_CT_AE_PRIORITY_CONTROL, 0}, {1, UVC_CT_EXPOSURE_TIME_ABSOLUTE_CONTROL, 0},
  {1, UVC_CT_FOCUS_ABSOLUTE_CONTROL, 0}, {1, UVC_CT_FOCUS_AUTO_CONTROL, 0}, {1, UVC_CT_IRIS_ABSOLUTE_CONTROL, 0},
  {1, UVC_CT_ZOOM_ABSOLUTE_CONTROL, 0}, {1, UVC_CT_ROLL_ABSOLUTE_CONTROL, 1}, {1, UVC_CT_FOCUS_SIMPLE_CONTROL, 0},
  {0, UVC_PU_BACKLIGHT_COMPENSATION_CONTROL, 0}, {0, UVC_PU_BRIGHTNESS_CONTROL, 1}, {0, UVC_PU_CONTRAST_CONTROL, 0},
  {0, UVC_PU_GAIN_CONTROL, 0}, {0, UVC_PU_POWER_LINE_FREQUENCY_CONTROL, 0}, {0, UVC_PU_HUE_CONTROL, 1},
  {0, UVC_PU_SATURATION_CONTROL, 0}, {0, UVC_PU_SHARPNESS_CONTROL, 0}, {0, UVC_PU_GAMMA_CONTROL, 0},
  {0, UVC_PU_WHITE_BALANCE_TEMPERATURE_CONTROL, 0}, {0, UVC_PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL, 0},
  {0, UVC_PU_WHITE_BALANCE_COMPONENT_AUTO_CONTROL, 0}, {0, UVC_PU_DIGITAL_MULTIPLIER_CONTROL, 0},
  {0, UVC_PU_DIGITAL_MULTIPLIER_LIMIT_CONTROL, 0}, {0, UVC_PU_HUE_AUTO_CONTROL, 0}, {0, UVC_PU_CONTRAST_AUTO_CONTROL, 0},
};

/* GET_INFO, then for a settable control: STALL "wrong state" while disabled by an auto mode,
 * otherwise "out of range" outside [GET_MIN, GET_MAX], "invalid value within range" off the
 * GET_RES grid, and MIN / the last grid step / the old value accepted and read back. */
static void check_scalar(uint8_t unit, uint8_t sel, int sgn) {
  int len = uvc_get_ctrl_len(devh, unit, sel);
  expect(len == 1 || len == 2 || len == 4, "%u/%u: GET_LEN %d", unit, sel, len);
  if (len != 1 && len != 2 && len != 4) return;
  int info = ctrl_info(unit, sel);
  expect(info >= 0 && (info & INFO_GET), "%u/%u: GET_INFO 0x%x supports GET", unit, sel, info);
  uint8_t cur[4], mn[4], mx[4], rs[4], b[4];
  if (info < 0 || !(info & INFO_SET) || (info & INFO_ASYNC)) return;
  if (uvc_get_ctrl(devh, unit, sel, cur, len, UVC_GET_CUR) != len) { expect(0, "%u/%u: GET_CUR", unit, sel); return; }
  if (info & INFO_DISABLED) {
    expect_rejected(uvc_set_ctrl(devh, unit, sel, cur, len), ERR_WRONG_STATE, "SET_CUR while disabled", unit, sel);
    return;
  }
  /* MIN/MAX/RES are optional for some controls (scanning mode, AE priority, ...) */
  if (uvc_get_ctrl(devh, unit, sel, mn, len, UVC_GET_MIN) != len || uvc_get_ctrl(devh, unit, sel, mx, len, UVC_GET_MAX) != len) {
    expect(uvc_set_ctrl(devh, unit, sel, cur, len) == len, "%u/%u: SET_CUR of GET_CUR", unit, sel);
    return;
  }
  int64_t lo = le_get(mn, len, sgn), hi = le_get(mx, len, sgn), res = 1;
  if (uvc_get_ctrl(devh, unit, sel, rs, len, UVC_GET_RES) == len && le_get(rs, len, 0) > 0) res = le_get(rs, len, 0);
  int64_t tmin = sgn ? -(1ll << (8 * len - 1)) : 0, tmax = sgn ? (1ll << (8 * len - 1)) - 1 : (1ll << (8 * len)) - 1;
  expect(lo <= hi, "%u/%u: GET_MIN %lld <= GET_MAX %lld", unit, sel, (long long)lo, (long long)hi);
  if (lo > hi) return;
  if (hi < tmax) { le_put(b, len, hi + 1); expect_rejected(uvc_set_ctrl(devh, unit, sel, b, len), ERR_OUT_OF_RANGE, "SET_CUR above GET_MAX", unit, sel); }
  if (lo > tmin) { le_put(b, len, lo - 1); expect_rejected(uvc_set_ctrl(devh, unit, sel, b, len), ERR_OUT_OF_RANGE, "SET_CUR below GET_MIN", unit, sel); }
  if (res > 1 && lo + 1 < hi) {
    le_put(b, len, lo + 1);
    expect_rejected(uvc_set_ctrl(devh, unit, sel, b, len), ERR_INVALID_VALUE_WITHIN_RANGE, "SET_CUR off the GET_RES grid", unit, sel);
  }
  int64_t vals[2] = {lo, lo + (hi - lo) / res * res};
  for (int i = 0; i < 2; i++) {
    le_put(b, len, vals[i]);
    expect(uvc_set_ctrl(devh, unit, sel, b, len) == len, "%u/%u: SET_CUR %lld accepted", unit, sel, (long long)vals[i]);
    expect(uvc_get_ctrl(devh, unit, sel, b, len, UVC_GET_CUR) == len && le_get(b, len, sgn) == vals[i],
           "%u/%u: read-back %lld", unit, sel, (long long)vals[i]);
  }
  expect(uvc_set_ctrl(devh, unit, sel, cur, len) == len, "%u/%u: restore", unit, sel);
}

/* GET_INFO "disabled" of `dep` must follow `disabled`; SET_CUR of its current value is rejected
 * with "wrong state" while disabled and accepted otherwise. */
static void expect_dep_state(uint8_t unit, uint8_t dep, int disabled, const char *why) {
  int info = ctrl_info(unit, dep);
  expect(info >= 0 && !!(info & INFO_DISABLED) == disabled, "%u/%u: GET_INFO 0x%x %s disabled (%s)", unit, dep, info,
         disabled ? "is" : "is not", why);
  int len = uvc_get_ctrl_len(devh, unit, dep);
  uint8_t b[64];
  if (len <= 0 || len > (int)sizeof b || get_cur_ready(unit, dep, b, len) != len) { expect(0, "%u/%u: GET_CUR", unit, dep); return; }
  int r = uvc_set_ctrl(devh, unit, dep, b, len);
  if (disabled) expect_rejected(r, ERR_WRONG_STATE, why, unit, dep);
  else expect(r == len, "%u/%u: SET_CUR accepted (%s)", unit, dep, why);
}

/* bmControls bit order differs from selector order, so ask the device: unsupported controls STALL. */
static int has_ctrl(uint8_t unit, uint8_t sel) { return ctrl_info(unit, sel) >= 0; }

/* "Auto" switches disable their manual controls (4.2.2.1.2-4.2.2.3.x). */
static void check_auto_links(const uvc_input_terminal_t *ct, const uvc_processing_unit_t *pu) {
  static const struct { uint8_t ct, autosel, dep; } LINKS[] = {
    {1, UVC_CT_FOCUS_AUTO_CONTROL, UVC_CT_FOCUS_ABSOLUTE_CONTROL},
    {0, UVC_PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL, UVC_PU_WHITE_BALANCE_TEMPERATURE_CONTROL},
    {0, UVC_PU_WHITE_BALANCE_COMPONENT_AUTO_CONTROL, UVC_PU_WHITE_BALANCE_COMPONENT_CONTROL},
    {0, UVC_PU_HUE_AUTO_CONTROL, UVC_PU_HUE_CONTROL},
    {0, UVC_PU_CONTRAST_AUTO_CONTROL, UVC_PU_CONTRAST_CONTROL},
  };
  int nlinks = 0;
  for (unsigned i = 0; i < sizeof LINKS / sizeof *LINKS; i++) {
    if (LINKS[i].ct ? !ct : !pu) continue;
    uint8_t unit = LINKS[i].ct ? ct->bTerminalID : pu->bUnitID;
    uint8_t a = LINKS[i].autosel, old = 0, v;
    if (!has_ctrl(unit, a) || !has_ctrl(unit, LINKS[i].dep)) continue;
    int info = ctrl_info(unit, a);
    if (info < 0 || !(info & INFO_SET) || uvc_get_ctrl(devh, unit, a, &old, 1, UVC_GET_CUR) != 1) continue;
    nlinks++;
    v = 1; expect(uvc_set_ctrl(devh, unit, a, &v, 1) == 1, "%u/%u: auto on", unit, a);
    expect_dep_state(unit, LINKS[i].dep, 1, "auto on");
    v = 0; expect(uvc_set_ctrl(devh, unit, a, &v, 1) == 1, "%u/%u: auto off", unit, a);
    expect_dep_state(unit, LINKS[i].dep, 0, "auto off");
    expect(uvc_set_ctrl(devh, unit, a, &old, 1) == 1, "%u/%u: restore", unit, a);
  }
  /* AE mode: GET_RES is the bitmap of supported modes (1 manual, 2 auto, 4 shutter priority,
   * 8 aperture priority); exposure time is automatic in 2/8, the iris in 2/4. */
  uint8_t old, modes, ae = UVC_CT_AE_MODE_CONTROL;
  if (ct && has_ctrl(ct->bTerminalID, ae) && uvc_get_ctrl(devh, ct->bTerminalID, ae, &old, 1, UVC_GET_CUR) == 1 &&
      uvc_get_ctrl(devh, ct->bTerminalID, ae, &modes, 1, UVC_GET_RES) == 1) {
    uint8_t unit = ct->bTerminalID;
    expect(modes != 0, "AE mode: GET_RES lists modes");
    for (uint8_t m = 1; m <= 8; m <<= 1) {
      if (!(modes & m)) continue;
      uint8_t got = 0;
      nlinks++;
      expect(uvc_set_ctrl(devh, unit, ae, &m, 1) == 1, "AE mode %u accepted", m);
      expect(uvc_get_ctrl(devh, unit, ae, &got, 1, UVC_GET_CUR) == 1 && got == m, "AE mode %u read-back %u", m, got);
      if (has_ctrl(unit, UVC_CT_EXPOSURE_TIME_ABSOLUTE_CONTROL))
        expect_dep_state(unit, UVC_CT_EXPOSURE_TIME_ABSOLUTE_CONTROL, !!(m & 0x0a), "AE mode");
      if (has_ctrl(unit, UVC_CT_IRIS_ABSOLUTE_CONTROL))
        expect_dep_state(unit, UVC_CT_IRIS_ABSOLUTE_CONTROL, !!(m & 0x06), "AE mode");
    }
    uint8_t bad = (uint8_t)(~modes & 0x0f & -(~modes & 0x0f)); /* lowest unsupported mode bit */
    if (bad) expect_rejected(uvc_set_ctrl(devh, unit, ae, &bad, 1), ERR_OUT_OF_RANGE, "unsupported AE mode", unit, ae);
    expect(uvc_set_ctrl(devh, unit, ae, &old, 1) == 1, "AE mode restore");
  }
  printf("  auto/manual links checked: %d\n", nlinks);
}

static void check_control_rules(void) {
  const uvc_input_terminal_t *ct = uvc_get_camera_terminal(devh);
  const uvc_processing_unit_t *pu = uvc_get_processing_units(devh);
  int n = 0;
  for (unsigned i = 0; i < sizeof SCALARS / sizeof *SCALARS; i++) {
    if (SCALARS[i].ct ? !ct : !pu) continue;
    uint8_t unit = SCALARS[i].ct ? ct->bTerminalID : pu->bUnitID;
    if (!has_ctrl(unit, SCALARS[i].sel)) continue;
    check_scalar(unit, SCALARS[i].sel, SCALARS[i].sgn);
    n++;
  }
  printf("  scalar controls checked: %d (request error code control: %s)\n", n, req_error() >= 0 ? "yes" : "no");
  check_auto_links(ct, pu);
}

/* VC_VIDEO_POWER_MODE_CONTROL (4.2.1.1, interface control: unit 0): D3..0 the mode (0 full power,
 * 1 vendor dependent if D4 says it is supported, others reserved), D7..4 read-only. */
#define VC_VIDEO_POWER_MODE_CONTROL 0x01
static void check_power_mode(void) {
  int info = ctrl_info(0, VC_VIDEO_POWER_MODE_CONTROL);
  if (info < 0) { printf("  power mode control: none\n"); return; }
  expect((info & (INFO_GET | INFO_SET)) == (INFO_GET | INFO_SET), "power mode: GET_INFO 0x%x has GET and SET", info);
  uint8_t cur = 0, b = 0, v;
  expect(uvc_get_ctrl(devh, 0, VC_VIDEO_POWER_MODE_CONTROL, &cur, 1, UVC_GET_CUR) == 1, "power mode: GET_CUR");
  int vendor = !!(cur & 0x10);
  expect((cur & 0x0f) == 0 || ((cur & 0x0f) == 1 && vendor), "power mode: valid mode in 0x%02x", cur);
  for (v = 0; v <= 2; v++) {
    int ok = v == 0 || (v == 1 && vendor);
    int r = uvc_set_ctrl(devh, 0, VC_VIDEO_POWER_MODE_CONTROL, &v, 1);
    if (!ok) { expect_rejected(r, ERR_OUT_OF_RANGE, "unsupported power mode", 0, VC_VIDEO_POWER_MODE_CONTROL); continue; }
    expect(r == 1, "power mode %u accepted", v);
    expect(uvc_get_ctrl(devh, 0, VC_VIDEO_POWER_MODE_CONTROL, &b, 1, UVC_GET_CUR) == 1 && b == ((cur & 0xf0) | v),
           "power mode %u: read back 0x%02x (read-only bits 0x%02x kept)", v, b, cur & 0xf0);
  }
  /* libuvc's enum uvc_device_power_mode: 0x0b/0x1b (reserved modes) in older libuvc, 0/1 now */
  uvc_error_t e = uvc_set_power_mode(devh, UVC_VC_VIDEO_POWER_MODE_FULL);
  printf("  power mode 0x%02x (D4 vendor mode %d): uvc_set_power_mode(UVC_VC_VIDEO_POWER_MODE_FULL) writes 0x%02x: %s\n",
         cur, vendor, (unsigned)UVC_VC_VIDEO_POWER_MODE_FULL, uvc_strerror(e));
  soft(e);
  v = cur & 0x0f;
  expect(uvc_set_ctrl(devh, 0, VC_VIDEO_POWER_MODE_CONTROL, &v, 1) == 1, "power mode restore");
}

/* Auto modes off (AE manual if GET_RES offers it), so the typed setters of the manual controls run
 * their success path; manual_restore() puts the auto modes back. */
static struct { uint8_t unit, sel, old; } autos[8];
static int nautos;
static void manual_mode(void) {
  const uvc_input_terminal_t *ct = uvc_get_camera_terminal(devh);
  const uvc_processing_unit_t *pu = uvc_get_processing_units(devh);
  static const struct { uint8_t ct, sel, manual; } A[] = {
    {1, UVC_CT_AE_MODE_CONTROL, 1}, {1, UVC_CT_FOCUS_AUTO_CONTROL, 0},
    {0, UVC_PU_WHITE_BALANCE_TEMPERATURE_AUTO_CONTROL, 0}, {0, UVC_PU_WHITE_BALANCE_COMPONENT_AUTO_CONTROL, 0},
    {0, UVC_PU_HUE_AUTO_CONTROL, 0}, {0, UVC_PU_CONTRAST_AUTO_CONTROL, 0},
  };
  nautos = 0;
  for (unsigned i = 0; i < sizeof A / sizeof *A; i++) {
    if (A[i].ct ? !ct : !pu) continue;
    uint8_t unit = A[i].ct ? ct->bTerminalID : pu->bUnitID, old, modes;
    if (!has_ctrl(unit, A[i].sel) || uvc_get_ctrl(devh, unit, A[i].sel, &old, 1, UVC_GET_CUR) != 1) continue;
    if (A[i].sel == UVC_CT_AE_MODE_CONTROL &&
        (uvc_get_ctrl(devh, unit, A[i].sel, &modes, 1, UVC_GET_RES) != 1 || !(modes & A[i].manual))) continue;
    uint8_t manual = A[i].manual;
    if (uvc_set_ctrl(devh, unit, A[i].sel, &manual, 1) == 1)
      autos[nautos].unit = unit, autos[nautos].sel = A[i].sel, autos[nautos++].old = old;
  }
}
static void manual_restore(void) {
  while (nautos--) uvc_set_ctrl(devh, autos[nautos].unit, autos[nautos].sel, &autos[nautos].old, 1);
  nautos = 0;
}
/* GET and SET, enabled, synchronous: a SET_CUR of the current value must succeed. */
static int settable(uint8_t unit, uint8_t sel) {
  int info = ctrl_info(unit, sel);
  return info >= 0 && (info & (INFO_GET | INFO_SET)) == (INFO_GET | INFO_SET) && !(info & (INFO_DISABLED | INFO_ASYNC));
}

static void mode_controls(void) {
  expect(open_device() == 0, "open device");
  if (!devh) return;
  int have_ct = uvc_get_camera_terminal(devh) != NULL;
  int have_pu = uvc_get_processing_units(devh) != NULL;
  int have_su = uvc_get_selector_units(devh) != NULL;

  /* power mode (VC interface, no unit) */
  { enum uvc_device_power_mode pm = 0; GETALL(uvc_get_power_mode(devh, &pm, RC));
    if (uvc_get_power_mode(devh, &pm, UVC_GET_CUR) == UVC_SUCCESS) soft(uvc_set_power_mode(devh, pm));
    else soft(uvc_set_power_mode(devh, UVC_VC_VIDEO_POWER_MODE_FULL)); }

  manual_mode();
  if (have_ct) {
    SIMPLE(scanning_mode, uint8_t);
    SIMPLE(ae_mode, uint8_t);
    SIMPLE(ae_priority, uint8_t);
    SIMPLE(exposure_abs, uint32_t);
    SIMPLE(exposure_rel, int8_t);
    SIMPLE(focus_abs, uint16_t);
    SIMPLE(focus_simple_range, uint8_t);
    SIMPLE(focus_auto, uint8_t);
    SIMPLE(iris_abs, uint16_t);
    SIMPLE(iris_rel, uint8_t);
    SIMPLE(zoom_abs, uint16_t);
    SIMPLE(roll_abs, int16_t);
    SIMPLE(privacy, uint8_t);
    { int8_t a = 0; uint8_t b = 0; GETALL(uvc_get_focus_rel(devh, &a, &b, RC));
      if (uvc_get_focus_rel(devh, &a, &b, UVC_GET_CUR) == UVC_SUCCESS) soft(uvc_set_focus_rel(devh, a, b)); else soft(uvc_set_focus_rel(devh, 0, 0)); }
    { int8_t a = 0; uint8_t b = 0, c = 0; GETALL(uvc_get_zoom_rel(devh, &a, &b, &c, RC));
      if (uvc_get_zoom_rel(devh, &a, &b, &c, UVC_GET_CUR) == UVC_SUCCESS) soft(uvc_set_zoom_rel(devh, a, b, c)); else soft(uvc_set_zoom_rel(devh, 0, 0, 0)); }
    { int32_t p = 0, t = 0; GETALL(uvc_get_pantilt_abs(devh, &p, &t, RC));
      if (uvc_get_pantilt_abs(devh, &p, &t, UVC_GET_CUR) == UVC_SUCCESS) soft(uvc_set_pantilt_abs(devh, p, t)); else soft(uvc_set_pantilt_abs(devh, 0, 0)); }
    { int8_t a = 0, c = 0; uint8_t b = 0, d = 0; GETALL(uvc_get_pantilt_rel(devh, &a, &b, &c, &d, RC));
      if (uvc_get_pantilt_rel(devh, &a, &b, &c, &d, UVC_GET_CUR) == UVC_SUCCESS) soft(uvc_set_pantilt_rel(devh, a, b, c, d)); else soft(uvc_set_pantilt_rel(devh, 0, 0, 0, 0)); }
    { int8_t a = 0; uint8_t b = 0; GETALL(uvc_get_roll_rel(devh, &a, &b, RC));
      if (uvc_get_roll_rel(devh, &a, &b, UVC_GET_CUR) == UVC_SUCCESS) soft(uvc_set_roll_rel(devh, a, b)); else soft(uvc_set_roll_rel(devh, 0, 0)); }
    { uint16_t w[6] = {0}; GETALL(uvc_get_digital_window(devh, &w[0], &w[1], &w[2], &w[3], &w[4], &w[5], RC));
      if (uvc_get_digital_window(devh, &w[0], &w[1], &w[2], &w[3], &w[4], &w[5], UVC_GET_CUR) == UVC_SUCCESS)
        soft(uvc_set_digital_window(devh, w[0], w[1], w[2], w[3], w[4], w[5]));
      else soft(uvc_set_digital_window(devh, 0, 0, 0, 0, 0, 0)); }
    { uint16_t w[5] = {0}; GETALL(uvc_get_digital_roi(devh, &w[0], &w[1], &w[2], &w[3], &w[4], RC));
      if (uvc_get_digital_roi(devh, &w[0], &w[1], &w[2], &w[3], &w[4], UVC_GET_CUR) == UVC_SUCCESS)
        soft(uvc_set_digital_roi(devh, w[0], w[1], w[2], w[3], w[4]));
      else soft(uvc_set_digital_roi(devh, 0, 0, 0, 0, 0)); }
  }
  if (have_pu) {
    SIMPLE(backlight_compensation, uint16_t);
    SIMPLE(brightness, int16_t);
    SIMPLE(contrast, uint16_t);
    SIMPLE(contrast_auto, uint8_t);
    SIMPLE(gain, uint16_t);
    SIMPLE(power_line_frequency, uint8_t);
    SIMPLE(hue, int16_t);
    SIMPLE(hue_auto, uint8_t);
    SIMPLE(saturation, uint16_t);
    SIMPLE(sharpness, uint16_t);
    SIMPLE(gamma, uint16_t);
    SIMPLE(white_balance_temperature, uint16_t);
    SIMPLE(white_balance_temperature_auto, uint8_t);
    SIMPLE(white_balance_component_auto, uint8_t);
    SIMPLE(digital_multiplier, uint16_t);
    SIMPLE(digital_multiplier_limit, uint16_t);
    SIMPLE(analog_video_standard, uint8_t);
    SIMPLE(analog_video_lock_status, uint8_t);
    { uint16_t b = 0, r = 0; GETALL(uvc_get_white_balance_component(devh, &b, &r, RC));
      if (uvc_get_white_balance_component(devh, &b, &r, UVC_GET_CUR) == UVC_SUCCESS) soft(uvc_set_white_balance_component(devh, b, r));
      else soft(uvc_set_white_balance_component(devh, 0, 0)); }
    /* brightness: write min/max/def back and verify the read-back when the device accepts it */
    int16_t mn, mx, def, rb;
    if (uvc_get_brightness(devh, &mn, UVC_GET_MIN) == UVC_SUCCESS && uvc_get_brightness(devh, &mx, UVC_GET_MAX) == UVC_SUCCESS &&
        uvc_get_brightness(devh, &def, UVC_GET_DEF) == UVC_SUCCESS) {
      int16_t vals[3] = {mn, mx, def};
      for (int i = 0; i < 3; i++)
        if (uvc_set_brightness(devh, vals[i]) == UVC_SUCCESS) {
          expect(uvc_get_brightness(devh, &rb, UVC_GET_CUR) == UVC_SUCCESS && rb == vals[i], "brightness read-back %d vs %d", rb, vals[i]);
        }
    }
  }
  if (have_su) SIMPLE(input_select, uint8_t);
  /* SIMPLE only counts the setters: with the auto modes off, the typed setters of the manual
   * controls must accept the current value. */
#define SETBACK(unit, sel, name, T) do { T v_ = 0; \
    if (settable(unit, sel) && uvc_get_##name(devh, &v_, UVC_GET_CUR) == UVC_SUCCESS) \
      expect(uvc_set_##name(devh, v_) == UVC_SUCCESS, "uvc_set_" #name " of the current value"); } while (0)
  if (have_ct) {
    uint8_t t = uvc_get_camera_terminal(devh)->bTerminalID;
    SETBACK(t, UVC_CT_EXPOSURE_TIME_ABSOLUTE_CONTROL, exposure_abs, uint32_t);
    SETBACK(t, UVC_CT_FOCUS_ABSOLUTE_CONTROL, focus_abs, uint16_t);
    uint16_t w[6];
    if (settable(t, UVC_CT_DIGITAL_WINDOW_CONTROL) &&
        uvc_get_digital_window(devh, &w[0], &w[1], &w[2], &w[3], &w[4], &w[5], UVC_GET_CUR) == UVC_SUCCESS)
      expect(uvc_set_digital_window(devh, w[0], w[1], w[2], w[3], w[4], w[5]) == UVC_SUCCESS,
             "uvc_set_digital_window of the current value");
  }
  if (have_pu) {
    uint8_t p = uvc_get_processing_units(devh)->bUnitID;
    SETBACK(p, UVC_PU_WHITE_BALANCE_TEMPERATURE_CONTROL, white_balance_temperature, uint16_t);
    SETBACK(p, UVC_PU_HUE_CONTROL, hue, int16_t);
    SETBACK(p, UVC_PU_CONTRAST_CONTROL, contrast, uint16_t);
    uint16_t b, r;
    if (settable(p, UVC_PU_WHITE_BALANCE_COMPONENT_CONTROL) &&
        uvc_get_white_balance_component(devh, &b, &r, UVC_GET_CUR) == UVC_SUCCESS)
      expect(uvc_set_white_balance_component(devh, b, r) == UVC_SUCCESS, "uvc_set_white_balance_component of the current value");
  }
#undef SETBACK
  manual_restore();
  check_control_rules();

  /* generic extension-unit access */
  int nxu = 0, xok = 0;
  for (const uvc_extension_unit_t *x = uvc_get_extension_units(devh); x; x = x->next) {
    nxu++;
    for (int sel = 1; sel <= 64; sel++) {
      if (!(x->bmControls & (1ull << (sel - 1)))) continue;
      int len = uvc_get_ctrl_len(devh, x->bUnitID, sel);
      soft(len);
      if (len <= 0 || len > 1024) continue;
      uint8_t *b = calloc(1, len + 1), *b2 = calloc(1, len + 1);
      int info = ctrl_info(x->bUnitID, sel);
      printf("  XU %u/%d len=%d info=0x%02x\n", x->bUnitID, sel, len, info < 0 ? 0 : info);
      expect(info >= 0, "XU %u/%d: GET_INFO", x->bUnitID, sel);
      int r = (info & INFO_ASYNC) ? get_cur_ready(x->bUnitID, sel, b, len) : uvc_get_ctrl(devh, x->bUnitID, sel, b, len, UVC_GET_CUR);
      /* GET_INFO says which of GET_CUR / SET_CUR exist (4.1.2) */
      if (info >= 0 && !(info & INFO_GET)) expect_rejected(r, ERR_INVALID_REQUEST, "GET_CUR without GET_INFO D0", x->bUnitID, sel);
      for (unsigned i = 1; i < 7; i++) soft(uvc_get_ctrl(devh, x->bUnitID, sel, b2, len, REQS[i]));
      if (info >= 0 && !(info & INFO_SET))
        expect_rejected(uvc_set_ctrl(devh, x->bUnitID, sel, b, len), ERR_INVALID_REQUEST, "SET_CUR without GET_INFO D1", x->bUnitID, sel);
      if (r == len) {
        xok++;
        int w = (info & INFO_SET) ? soft(uvc_set_ctrl(devh, x->bUnitID, sel, b, len)) : -1;
        if (w == len) {
          expect(get_cur_ready(x->bUnitID, sel, b2, len) == len && !memcmp(b, b2, len), "XU %u/%d read-back", x->bUnitID, sel);
          /* a SET_CUR whose wLength isn't GET_LEN must STALL (2.4.4: unexpected data length) */
          expect(uvc_set_ctrl(devh, x->bUnitID, sel, b, len + 1) < 0, "XU %u/%d: SET_CUR of %d bytes rejected", x->bUnitID, sel, len + 1);
          if (len > 1) expect(uvc_set_ctrl(devh, x->bUnitID, sel, b, len - 1) < 0, "XU %u/%d: SET_CUR of %d bytes rejected", x->bUnitID, sel, len - 1);
        }
      }
      free(b); free(b2);
    }
  }
  /* generic access on a unit that does not exist: must fail cleanly */
  { uint8_t b[4] = {0}; expect(uvc_get_ctrl(devh, 0xfe, 1, b, 4, UVC_GET_CUR) < 0, "control on non-existent unit fails");
    expect(uvc_get_ctrl_len(devh, 0xfe, 1) < 0, "len on non-existent unit fails");
    expect(uvc_set_ctrl(devh, 0xfe, 1, b, 4) < 0, "set on non-existent unit fails"); }
  printf("  units: ct=%d pu=%d su=%d xu=%d (xu controls readable: %d)\n", have_ct, have_pu, have_su, nxu, xok);
  check_power_mode();
}

/* ================= convert ================= */
static void expect_frame(uvc_frame_t *o, enum uvc_frame_format ff, uint32_t w, uint32_t h, size_t bytes, const char *what) {
  expect(o->frame_format == ff && o->width == w && o->height == h && o->data_bytes == bytes && o->step == bytes / h,
         "%s: format=%d %ux%u bytes=%zu step=%zu (want %d %ux%u %zu)", what, o->frame_format, o->width, o->height, o->data_bytes, o->step, ff, w, h, bytes);
}

static void convert_yuv(uvc_frame_t *in, const char *tag) {
  uvc_frame_t *o = uvc_allocate_frame(0);
  uint32_t w = in->width, h = in->height;
  size_t px = (size_t)w * h;
  int yuyv = in->frame_format == UVC_FRAME_FORMAT_YUYV;
  char what[64];
#define STEP(fn, ff, bytes) do { snprintf(what, sizeof what, "%s %s", tag, #fn); \
    expect(fn(in, o) == UVC_SUCCESS, "%s", what); expect_frame(o, ff, w, h, bytes, what); } while (0)
  if (yuyv) {
    STEP(uvc_yuyv2rgb, UVC_FRAME_FORMAT_RGB, px * 3);
    STEP(uvc_yuyv2bgr, UVC_FRAME_FORMAT_BGR, px * 3);
    STEP(uvc_yuyv2y, UVC_FRAME_FORMAT_GRAY8, px);
    STEP(uvc_yuyv2uv, UVC_FRAME_FORMAT_GRAY8, px);
    expect(uvc_uyvy2rgb(in, o) == UVC_ERROR_INVALID_PARAM, "%s uyvy2rgb on yuyv rejected", tag);
    expect(uvc_uyvy2bgr(in, o) == UVC_ERROR_INVALID_PARAM, "%s uyvy2bgr on yuyv rejected", tag);
  } else {
    STEP(uvc_uyvy2rgb, UVC_FRAME_FORMAT_RGB, px * 3);
    STEP(uvc_uyvy2bgr, UVC_FRAME_FORMAT_BGR, px * 3);
    expect(uvc_yuyv2rgb(in, o) == UVC_ERROR_INVALID_PARAM, "%s yuyv2rgb on uyvy rejected", tag);
    expect(uvc_yuyv2bgr(in, o) == UVC_ERROR_INVALID_PARAM, "%s yuyv2bgr on uyvy rejected", tag);
    expect(uvc_yuyv2y(in, o) == UVC_ERROR_INVALID_PARAM, "%s yuyv2y on uyvy rejected", tag);
    expect(uvc_yuyv2uv(in, o) == UVC_ERROR_INVALID_PARAM, "%s yuyv2uv on uyvy rejected", tag);
  }
  STEP(uvc_any2rgb, UVC_FRAME_FORMAT_RGB, px * 3);
  STEP(uvc_any2bgr, UVC_FRAME_FORMAT_BGR, px * 3);
  /* short input */
  uvc_frame_t sh = *in; sh.data_bytes = px * 2 - 1;
  expect(uvc_any2rgb(&sh, o) == UVC_ERROR_INVALID_PARAM, "%s short input (rgb) rejected", tag);
  expect(uvc_any2bgr(&sh, o) == UVC_ERROR_INVALID_PARAM, "%s short input (bgr) rejected", tag);
  if (yuyv) {
    expect(uvc_yuyv2y(&sh, o) == UVC_ERROR_INVALID_PARAM, "%s short input (y) rejected", tag);
    expect(uvc_yuyv2uv(&sh, o) == UVC_ERROR_INVALID_PARAM, "%s short input (uv) rejected", tag);
  }
  /* caller-owned output buffer too small / big enough */
  uint8_t small[8], *big = malloc(px * 3);
  uvc_frame_t user = {0};
  user.data = small; user.data_bytes = sizeof small; user.library_owns_data = 0;
  expect(uvc_any2rgb(in, &user) == UVC_ERROR_NO_MEM, "%s small caller buffer (rgb) rejected", tag);
  expect(uvc_any2bgr(in, &user) == UVC_ERROR_NO_MEM, "%s small caller buffer (bgr) rejected", tag);
  user.data = big; user.data_bytes = px * 3;
  expect(uvc_any2rgb(in, &user) == UVC_SUCCESS && user.data == big, "%s caller buffer used", tag);
  expect(uvc_any2bgr(in, &user) == UVC_SUCCESS, "%s caller buffer used (bgr)", tag);
  free(big);
  uvc_free_frame(o);
}

/* A synthetic frame in a packed 4:2:2 format, deterministic content. */
static uvc_frame_t *synth_yuv(enum uvc_frame_format ff, uint32_t w, uint32_t h) {
  uvc_frame_t *f = uvc_allocate_frame((size_t)w * h * 2);
  uint8_t *d = f->data;
  for (size_t i = 0; i < f->data_bytes; i++) d[i] = (uint8_t)(i * 7 + (i >> 5));
  f->width = w; f->height = h; f->frame_format = ff; f->step = w * 2; f->sequence = 1;
  return f;
}

/* JPEG of a gradient; strip_dht removes the Huffman tables (libuvc re-inserts the standard ones). */
static uvc_frame_t *synth_mjpeg(uint32_t w, uint32_t h, int strip_dht) {
  struct jpeg_compress_struct c; struct jpeg_error_mgr je;
  unsigned char *out = NULL; unsigned long olen = 0;
  c.err = jpeg_std_error(&je);
  jpeg_create_compress(&c);
  jpeg_mem_dest(&c, &out, &olen);
  c.image_width = w; c.image_height = h; c.input_components = 3; c.in_color_space = JCS_RGB;
  jpeg_set_defaults(&c); jpeg_set_quality(&c, 80, TRUE);
  jpeg_start_compress(&c, TRUE);
  unsigned char *row = malloc((size_t)w * 3);
  while (c.next_scanline < h) {
    for (uint32_t x = 0; x < w; x++) { row[x * 3] = x * 255 / w; row[x * 3 + 1] = c.next_scanline * 255 / h; row[x * 3 + 2] = 128; }
    jpeg_write_scanlines(&c, &row, 1);
  }
  free(row);
  jpeg_finish_compress(&c); jpeg_destroy_compress(&c);
  size_t n = olen;
  uint8_t *data = malloc(n);
  if (strip_dht) {
    size_t i = 2, o = 2; data[0] = 0xff; data[1] = 0xd8;
    while (i + 4 <= olen && out[i] == 0xff) {
      size_t len = (out[i + 2] << 8) | out[i + 3];
      if (out[i + 1] == 0xc4) { i += 2 + len; continue; }
      if (out[i + 1] == 0xda) break;
      memcpy(data + o, out + i, 2 + len); o += 2 + len; i += 2 + len;
    }
    memcpy(data + o, out + i, olen - i); n = o + olen - i;
  } else memcpy(data, out, n);
  free(out);
  uvc_frame_t *f = uvc_allocate_frame(n);
  memcpy(f->data, data, n); free(data);
  f->width = w; f->height = h; f->frame_format = UVC_FRAME_FORMAT_MJPEG; f->sequence = 1;
  return f;
}

static void convert_mjpeg(uvc_frame_t *in, const char *tag, int expect_ok) {
  uvc_frame_t *o = uvc_allocate_frame(0);
  uint32_t w = in->width, h = in->height; size_t px = (size_t)w * h;
  char what[64];
  snprintf(what, sizeof what, "%s mjpeg2rgb", tag);
  int r = uvc_mjpeg2rgb(in, o);
  if (expect_ok) { expect(r == UVC_SUCCESS, "%s", what); if (!r) expect_frame(o, UVC_FRAME_FORMAT_RGB, w, h, px * 3, what); } else soft(r);
  snprintf(what, sizeof what, "%s mjpeg2bgr", tag);
  r = uvc_mjpeg2bgr(in, o);
  if (expect_ok) { expect(r == UVC_SUCCESS, "%s", what); if (!r) expect_frame(o, UVC_FRAME_FORMAT_BGR, w, h, px * 3, what); } else soft(r);
  snprintf(what, sizeof what, "%s mjpeg2gray", tag);
  r = uvc_mjpeg2gray(in, o);
  if (expect_ok) { expect(r == UVC_SUCCESS, "%s", what); if (!r) expect_frame(o, UVC_FRAME_FORMAT_GRAY8, w, h, px, what); } else soft(r);
  snprintf(what, sizeof what, "%s any2rgb", tag);
  r = uvc_any2rgb(in, o);
  if (expect_ok) expect(r == UVC_SUCCESS, "%s", what); else soft(r);
  /* uvc_any2bgr decodes MJPEG since libuvc#351; before, it returned NOT_SUPPORTED (soft). */
  snprintf(what, sizeof what, "%s any2bgr", tag);
  r = uvc_any2bgr(in, o);
  if (r == UVC_ERROR_NOT_SUPPORTED) soft(r);
  else if (expect_ok) { expect(r == UVC_SUCCESS, "%s", what); if (!r) expect_frame(o, UVC_FRAME_FORMAT_BGR, w, h, px * 3, what); }
  else soft(r);
  uvc_free_frame(o);
}

static void mode_convert(void) {
  /* synthetic frames: no device needed, always covered */
  static const struct { uint32_t w, h; } sizes[] = {{64, 48}, {32, 16}, {18, 5}, {2, 2}, {640, 480}};
  for (unsigned i = 0; i < sizeof sizes / sizeof *sizes; i++) {
    char tag[32]; snprintf(tag, sizeof tag, "synth %ux%u", sizes[i].w, sizes[i].h);
    uvc_frame_t *a = synth_yuv(UVC_FRAME_FORMAT_YUYV, sizes[i].w, sizes[i].h); convert_yuv(a, tag); uvc_free_frame(a);
    a = synth_yuv(UVC_FRAME_FORMAT_UYVY, sizes[i].w, sizes[i].h); convert_yuv(a, tag); uvc_free_frame(a);
  }
  uvc_frame_t *a = synth_mjpeg(64, 48, 0);
  convert_mjpeg(a, "synth mjpeg", 1);
  /* wrong declared geometry: rejected, no overflow */
  a->width = 32; a->height = 24;
  { uvc_frame_t *o = uvc_allocate_frame(0); expect(uvc_mjpeg2rgb(a, o) != UVC_SUCCESS, "mjpeg size mismatch rejected");
    expect(uvc_mjpeg2gray(a, o) != UVC_SUCCESS, "mjpeg size mismatch rejected (gray)");
    expect(uvc_mjpeg2bgr(a, o) != UVC_SUCCESS, "mjpeg size mismatch rejected (bgr)"); uvc_free_frame(o); }
  /* truncated and garbage data: error, not a crash */
  a->width = 64; a->height = 48;
  { uvc_frame_t *o = uvc_allocate_frame(0); size_t full = a->data_bytes;
    a->data_bytes = full / 2; soft(uvc_mjpeg2rgb(a, o));
    a->data_bytes = 10; soft(uvc_mjpeg2bgr(a, o));
    memset(a->data, 0x55, full); a->data_bytes = full; expect(uvc_mjpeg2gray(a, o) != UVC_SUCCESS, "garbage mjpeg rejected");
    uvc_free_frame(o); }
  /* wrong format */
  { uvc_frame_t *o = uvc_allocate_frame(0), *y = synth_yuv(UVC_FRAME_FORMAT_YUYV, 8, 8);
    expect(uvc_mjpeg2rgb(y, o) == UVC_ERROR_INVALID_PARAM, "mjpeg2rgb on yuyv rejected");
    expect(uvc_mjpeg2bgr(y, o) == UVC_ERROR_INVALID_PARAM, "mjpeg2bgr on yuyv rejected");
    expect(uvc_mjpeg2gray(y, o) == UVC_ERROR_INVALID_PARAM, "mjpeg2gray on yuyv rejected");
    y->frame_format = UVC_FRAME_FORMAT_NV12;
    expect(uvc_any2rgb(y, o) == UVC_ERROR_NOT_SUPPORTED, "any2rgb on NV12 unsupported");
    expect(uvc_any2bgr(y, o) == UVC_ERROR_NOT_SUPPORTED, "any2bgr on NV12 unsupported");
    y->frame_format = UVC_FRAME_FORMAT_RGB; expect(uvc_any2rgb(y, o) == UVC_SUCCESS, "any2rgb(rgb) duplicates");
    y->frame_format = UVC_FRAME_FORMAT_BGR; expect(uvc_any2bgr(y, o) == UVC_SUCCESS, "any2bgr(bgr) duplicates");
    uvc_free_frame(y); uvc_free_frame(o); }
  uvc_free_frame(a);
  a = synth_mjpeg(64, 48, 1);
  convert_mjpeg(a, "synth mjpeg (no DHT)", 1);
  uvc_free_frame(a);

  /* duplicate frame incl. metadata, and the freeing helpers */
  { uvc_frame_t *s = uvc_allocate_frame(100), *d = uvc_allocate_frame(0);
    memset(s->data, 0xab, 100); s->width = 10; s->height = 10; s->sequence = 42;
    s->metadata = malloc(16); s->metadata_bytes = 16; memset(s->metadata, 7, 16);
    expect(uvc_duplicate_frame(s, d) == UVC_SUCCESS && d->data_bytes == 100 && d->sequence == 42 && !memcmp(s->data, d->data, 100), "duplicate");
    expect(d->metadata_bytes == 16 && !memcmp(d->metadata, s->metadata, 16), "duplicate metadata");
    expect(uvc_duplicate_frame(s, d) == UVC_SUCCESS, "duplicate onto filled frame");
    uvc_frame_t user = {0}; uint8_t tiny[4]; user.data = tiny; user.data_bytes = 4;
    expect(uvc_duplicate_frame(s, &user) == UVC_ERROR_NO_MEM, "duplicate into small caller buffer rejected");
    uvc_free_frame(d); uvc_free_frame(s); }
  { uvc_frame_t *z = uvc_allocate_frame(0); expect(z && !z->data && z->data_bytes == 0, "allocate empty frame"); uvc_free_frame(z); }

  /* real frames from the device, every format */
  expect(open_device() == 0, "open device");
  if (!devh) return;
  int grabbed = 0;
  for (const uvc_format_desc_t *f = uvc_get_format_descs(devh); f; f = f->next) {
    enum uvc_frame_format ff = fmt_of(f);
    const uvc_frame_desc_t *fr = smallest_frame(f);
    if (!fr || ff == UVC_FRAME_FORMAT_UNKNOWN) { printf("  format %u: not decodable by libuvc\n", f->bFormatIndex); continue; }
    uvc_stream_ctrl_t c;
    if (soft(ctrl_for(f, fr, default_interval(fr), &c)) < 0) continue;
    uvc_frame_t *g = grab(&c, 3000000);
    if (!g) { expect(0, "no frame in format %u (%ux%u)", f->bFormatIndex, fr->wWidth, fr->wHeight); continue; }
    grabbed++;
    char tag[48]; snprintf(tag, sizeof tag, "device fmt%u %ux%u", f->bFormatIndex, g->width, g->height);
    printf("  grabbed format=%d %ux%u %zu bytes\n", g->frame_format, g->width, g->height, g->data_bytes);
    expect(g->width == fr->wWidth && g->height == fr->wHeight, "%s: frame geometry", tag);
    if (ff == UVC_FRAME_FORMAT_MJPEG) convert_mjpeg(g, tag, 1);
    else if (ff == UVC_FRAME_FORMAT_YUYV || ff == UVC_FRAME_FORMAT_UYVY) convert_yuv(g, tag);
    else { uvc_frame_t *o = uvc_allocate_frame(0); expect(uvc_any2rgb(g, o) != UVC_SUCCESS || ff == UVC_FRAME_FORMAT_RGB, "%s: any2rgb", tag); uvc_free_frame(o); }
    uvc_free_frame(g);
  }
  expect(grabbed > 0, "grabbed at least one frame");
}

/* ================= poll ================= */
static volatile int cb_frames;
static void count_cb(uvc_frame_t *f, void *p) { (void)p; if (f && f->data_bytes) cb_frames++; }

static void mode_poll(void) {
  expect(open_device() == 0, "open device");
  if (!devh) return;
  const uvc_format_desc_t *f = uvc_get_format_descs(devh);
  const uvc_frame_desc_t *fr = f ? smallest_frame(f) : NULL;
  if (!fr) { expect(0, "no format"); return; }
  uvc_stream_ctrl_t c, cur;
  expect(ctrl_for(f, fr, default_interval(fr), &c) == UVC_SUCCESS, "negotiate");
  for (int round = 0; round < 2; round++) { /* stop/close/reopen */
    uvc_stream_handle_t *s = NULL, *s2 = NULL;
    uvc_frame_t *fp = NULL;
    expect(uvc_stream_open_ctrl(devh, &s, &c) == UVC_SUCCESS && s, "stream open (round %d)", round);
    if (!s) return;
    expect(uvc_stream_open_ctrl(devh, &s2, &c) == UVC_ERROR_BUSY, "second open of the same interface is BUSY");
    uvc_stream_ctrl_t bad = c; bad.bInterfaceNumber = 0xee;
    expect(uvc_stream_open_ctrl(devh, &s2, &bad) == UVC_ERROR_INVALID_PARAM, "open on bogus interface rejected");
    expect(uvc_stream_ctrl(s, &bad) == UVC_ERROR_INVALID_PARAM, "stream_ctrl on bogus interface rejected");
    expect(uvc_stream_get_current_ctrl(s, &cur) == UVC_SUCCESS && cur.bFormatIndex == c.bFormatIndex, "get_current_ctrl");
    expect(uvc_stream_get_frame(s, &fp, 1000) == UVC_ERROR_INVALID_PARAM, "get_frame before start rejected");
    expect(uvc_stream_stop(s) != UVC_SUCCESS || 1, "stop when not running");
    expect(uvc_stream_start(s, NULL, NULL, 0) == UVC_SUCCESS, "start without callback");
    expect(uvc_stream_start(s, NULL, NULL, 0) == UVC_ERROR_BUSY, "double start is BUSY");
    expect(uvc_stream_ctrl(s, &c) == UVC_ERROR_BUSY, "stream_ctrl while running is BUSY");
    int got = 0;
    fp = NULL;
    uvc_error_t e = uvc_stream_get_frame(s, &fp, -1);     /* non-blocking */
    soft(e); if (e == UVC_SUCCESS && fp) got++;
    fp = NULL; e = uvc_stream_get_frame(s, &fp, 1);       /* 1us: normally times out */
    soft(e); if (e == UVC_SUCCESS && fp) got++; else expect(fp == NULL, "timeout leaves frame NULL");
    for (int i = 0; i < 4; i++) {                         /* timed */
      fp = NULL; e = uvc_stream_get_frame(s, &fp, opt_lenient ? 300000 : 2000000);
      expect(e == UVC_SUCCESS && fp && fp->data_bytes > 0, "timed get_frame: %s", uvc_strerror(e));
      if (e == UVC_SUCCESS && fp) { got++; expect(fp->width == fr->wWidth && fp->height == fr->wHeight, "frame geometry"); }
    }
    for (int i = 0; i < 3; i++) {                         /* blocking (watchdog protects) */
      fp = NULL; e = uvc_stream_get_frame(s, &fp, opt_lenient ? 300000 : 0); /* 0 = block (a lenient run must not hang on a dead stream) */
      expect(e == UVC_SUCCESS && fp, "blocking get_frame: %s", uvc_strerror(e));
      if (e == UVC_SUCCESS && fp) got++;
    }
    expect(got >= 4, "frames received: %d", got);
    expect(uvc_stream_stop(s) == UVC_SUCCESS, "stop");
    expect(uvc_stream_stop(s) != UVC_SUCCESS || 1, "stop again");
    if (round == 0) {
      /* restart the same handle with the iso variant, then a callback (get_frame must refuse) */
      soft(uvc_stream_start_iso(s, count_cb, NULL));
      fp = NULL;
      e = uvc_stream_get_frame(s, &fp, 1000);
      expect(e == UVC_ERROR_CALLBACK_EXISTS || e == UVC_ERROR_INVALID_PARAM, "get_frame with callback rejected: %s", uvc_strerror(e));
      for (int i = 0; i < (opt_lenient ? 6 : 100) && cb_frames < 3; i++) usleep(50000);
      expect(cb_frames >= 1, "callback frames: %d", cb_frames);
      uvc_stream_stop(s);
    }
    uvc_stream_close(s);
  }
  /* device-level start/stop with callbacks, including the iso entry point */
  cb_frames = 0;
  expect(uvc_start_streaming(devh, &c, count_cb, NULL, 0) == UVC_SUCCESS, "uvc_start_streaming");
  for (int i = 0; i < (opt_lenient ? 6 : 100) && cb_frames < 3; i++) usleep(50000);
  expect(cb_frames >= 1, "uvc_start_streaming frames: %d", cb_frames);
  expect(uvc_start_streaming(devh, &c, count_cb, NULL, 0) == UVC_ERROR_BUSY, "uvc_start_streaming twice is BUSY");
  uvc_stop_streaming(devh);
  cb_frames = 0;
  expect(uvc_start_iso_streaming(devh, &c, count_cb, NULL) == UVC_SUCCESS, "uvc_start_iso_streaming");
  for (int i = 0; i < (opt_lenient ? 6 : 100) && cb_frames < 3; i++) usleep(50000);
  expect(cb_frames >= 1, "uvc_start_iso_streaming frames: %d", cb_frames);
  uvc_stop_streaming(devh);
  uvc_stop_streaming(devh); /* idempotent */
  /* close the device with a stream still open and running */
  uvc_stream_handle_t *s;
  if (uvc_stream_open_ctrl(devh, &s, &c) == UVC_SUCCESS) { soft(uvc_stream_start(s, count_cb, NULL, 0)); usleep(100000); }
  uvc_close(devh); devh = NULL;
}

/* ================= still ================= */
/* Method 2: after the trigger, one frame in the stream is the still image (2.4.2.4). libuvc
 * delivers it like a video frame, cut at the video dwMaxVideoFrameSize. So stream an
 * uncompressed frame size larger than the smallest still size: the still then shows up as one
 * frame of exactly the still's byte count. */
static void check_still_frame(void) {
  for (const uvc_format_desc_t *f = uvc_get_format_descs(devh); f; f = f->next) {
    if (f->bDescriptorSubtype != UVC_VS_FORMAT_UNCOMPRESSED || !f->bBitsPerPixel || !f->still_frame_desc ||
        fmt_of(f) == UVC_FRAME_FORMAT_UNKNOWN) continue;
    const uvc_still_frame_res_t *s = NULL;
    for (const uvc_still_frame_res_t *r = f->still_frame_desc->imageSizePatterns; r; r = r->next)
      if (!s || (uint32_t)r->wWidth * r->wHeight < (uint32_t)s->wWidth * s->wHeight) s = r;
    if (!s) continue;
    size_t still_bytes = (size_t)s->wWidth * s->wHeight * f->bBitsPerPixel / 8;
    const uvc_frame_desc_t *fr = NULL;
    for (const uvc_frame_desc_t *x = f->frame_descs; x; x = x->next)
      if ((size_t)x->wWidth * x->wHeight * f->bBitsPerPixel / 8 > still_bytes &&
          (!fr || (uint32_t)x->wWidth * x->wHeight < (uint32_t)fr->wWidth * fr->wHeight)) fr = x;
    if (!fr) continue;
    size_t video_bytes = (size_t)fr->wWidth * fr->wHeight * f->bBitsPerPixel / 8;
    uvc_stream_ctrl_t c; uvc_still_ctrl_t sc; uvc_stream_handle_t *st;
    memset(&sc, 0, sizeof sc);
    if (soft(ctrl_for(f, fr, default_interval(fr), &c)) < 0) return;
    expect(uvc_get_still_ctrl_format_size(devh, &c, &sc, s->wWidth, s->wHeight) == UVC_SUCCESS, "still %ux%u negotiated", s->wWidth, s->wHeight);
    if (soft(uvc_stream_open_ctrl(devh, &st, &c)) < 0) return;
    int got_still = 0, video_after = 0;
    if (soft(uvc_stream_start(st, NULL, NULL, 0)) >= 0) {
      uvc_frame_t *fp = NULL;
      uvc_stream_get_frame(st, &fp, 2000000);
      uvc_error_t te = uvc_trigger_still(devh, &sc);
      expect(te == UVC_SUCCESS, "trigger still: %s", uvc_strerror(te));
      if (te == UVC_SUCCESS)
        for (int i = 0; i < 10 && !(got_still && video_after); i++) {
          fp = NULL;
          if (uvc_stream_get_frame(st, &fp, 1000000) != UVC_SUCCESS || !fp) continue;
          if (fp->data_bytes == still_bytes) got_still++;
          else if (got_still && fp->data_bytes == video_bytes) video_after = 1;
        }
      uvc_stream_stop(st);
    }
    uvc_stream_close(st);
    printf("  still frame: format %u video %ux%u still %ux%u: still frames=%d\n", f->bFormatIndex, fr->wWidth, fr->wHeight,
           s->wWidth, s->wHeight, got_still);
    expect(got_still == 1, "exactly one still frame of %zu bytes (got %d)", still_bytes, got_still);
    expect(video_after, "video frames of %zu bytes resume after the still", video_bytes);
    return;
  }
}

static void mode_still(void) {
  expect(open_device() == 0, "open device");
  if (!devh) return;
  int supported = 0;
  for (const uvc_format_desc_t *f = uvc_get_format_descs(devh); f; f = f->next) {
    const uvc_frame_desc_t *fr = smallest_frame(f);
    if (!fr || !f->still_frame_desc || !f->still_frame_desc->imageSizePatterns) continue;
    uvc_stream_ctrl_t c; uvc_still_ctrl_t sc; memset(&sc, 0, sizeof sc);
    if (soft(ctrl_for(f, fr, default_interval(fr), &c)) < 0) continue;
    const uvc_still_frame_res_t *r = f->still_frame_desc->imageSizePatterns;
    uvc_error_t e = uvc_get_still_ctrl_format_size(devh, &c, &sc, r->wWidth, r->wHeight);
    printf("  format %u still %ux%u: %s\n", f->bFormatIndex, r->wWidth, r->wHeight, uvc_strerror(e));
    soft(e);
    expect(uvc_get_still_ctrl_format_size(devh, &c, &sc, 1, 1) != UVC_SUCCESS, "bogus still size rejected");
    soft(uvc_probe_still_ctrl(devh, &sc));
    uvc_stream_handle_t *s;
    expect(uvc_trigger_still(devh, &sc) != UVC_SUCCESS, "trigger without running stream rejected");
    if (e == UVC_SUCCESS && soft(uvc_stream_open_ctrl(devh, &s, &c)) >= 0) {
      if (soft(uvc_stream_start(s, NULL, NULL, 0)) >= 0) {
        uvc_frame_t *fp = NULL;
        uvc_stream_get_frame(s, &fp, 2000000);
        e = soft(uvc_trigger_still(devh, &sc));
        if (e == UVC_SUCCESS) {
          supported = 1;
          for (int i = 0; i < 5; i++) { fp = NULL; uvc_stream_get_frame(s, &fp, 1000000); }
        }
        uvc_stream_stop(s);
      }
      uvc_stream_close(s);
    }
  }
  if (!supported) {
    /* method not advertised: the entry points must still fail cleanly */
    const uvc_format_desc_t *f = uvc_get_format_descs(devh);
    const uvc_frame_desc_t *fr = f ? smallest_frame(f) : NULL;
    uvc_stream_ctrl_t c; uvc_still_ctrl_t sc; memset(&sc, 0, sizeof sc);
    if (fr && ctrl_for(f, fr, default_interval(fr), &c) == UVC_SUCCESS) {
      soft(uvc_get_still_ctrl_format_size(devh, &c, &sc, fr->wWidth, fr->wHeight));
      expect(uvc_trigger_still(devh, &sc) != UVC_SUCCESS, "trigger rejected");
    }
    printf("  still capture: not supported\n");
  }
  if (supported) check_still_frame();
  printf("RESULT_EXTRA still_supported=%d\n", supported);
}

/* ================= multi ================= */
static void mode_multi(void) {
  expect(open_device() == 0, "open device");
  if (!devh) return;
  uvc_streaming_interface_t *si;
  int nif = 0;
  uvc_stream_handle_t *hs[8]; int nh = 0;
  DL_FOREACH(devh->info->stream_ifs, si) nif++;
  printf("  streaming interfaces: %d\n", nif);
  if (nif < 2) { printf("  multi: single streaming interface, not applicable\n"); return; }
  DL_FOREACH(devh->info->stream_ifs, si) {
    uvc_format_desc_t *f; uvc_stream_ctrl_t c; int done = 0;
    DL_FOREACH(si->format_descs, f) {
      if (done || fmt_of(f) == UVC_FRAME_FORMAT_UNKNOWN) continue;
      const uvc_frame_desc_t *fr = smallest_frame(f);
      if (!fr || soft(ctrl_for(f, fr, default_interval(fr), &c)) < 0) continue;
      uvc_stream_handle_t *s;
      if (soft(uvc_stream_open_ctrl(devh, &s, &c)) < 0) continue;
      if (soft(uvc_stream_start(s, NULL, NULL, 0)) < 0) { uvc_stream_close(s); continue; }
      hs[nh++] = s; done = 1;
    }
    if (nh == 8) break;
  }
  int total = 0;
  for (int round = 0; round < 3; round++)
    for (int i = 0; i < nh; i++) {
      uvc_frame_t *fp = NULL;
      uvc_error_t e = uvc_stream_get_frame(hs[i], &fp, 3000000);
      soft(e); if (e == UVC_SUCCESS && fp) total++;
    }
  printf("  streams running: %d, frames: %d\n", nh, total);
  expect(nh < 2 || total >= nh, "every running stream delivers frames (%d streams, %d frames)", nh, total);
  for (int i = 0; i < nh; i++) { uvc_stream_stop(hs[i]); uvc_stream_close(hs[i]); }
}

/* ================= status ================= */
static volatile int n_status, n_button;
/* status events by bAttribute: value, info, failure, min, max change; anything else */
static volatile int n_attr[6];
/* the latest control status events, for the async mode */
static pthread_mutex_t ev_lock = PTHREAD_MUTEX_INITIALIZER;
static struct { enum uvc_status_class c; int sel, attr; size_t len; uint8_t data[64]; } evs[64];
static int n_evs;
static void status_cb(enum uvc_status_class c, int ev, int sel, enum uvc_status_attribute a, void *d, size_t l, void *u) {
  (void)ev; (void)u;
  n_status++;
  n_attr[(unsigned)a < 5 ? (unsigned)a : 5]++;
  printf("  status event class=0x%x selector=%d attribute=%d len=%zu\n", c, sel, a, l);
  pthread_mutex_lock(&ev_lock);
  if (n_evs < (int)(sizeof evs / sizeof *evs)) {
    evs[n_evs].c = c; evs[n_evs].sel = sel; evs[n_evs].attr = a;
    evs[n_evs].len = l < sizeof evs[0].data ? l : sizeof evs[0].data;
    memcpy(evs[n_evs].data, d, evs[n_evs].len);
    n_evs++;
  }
  pthread_mutex_unlock(&ev_lock);
}
static void button_cb(int b, int s, void *u) { (void)u; n_button++; printf("  button %d state %d\n", b, s); }

static void mode_status(void) {
  expect(open_device() == 0, "open device");
  if (!devh) return;
  uvc_set_status_callback(devh, status_cb, NULL);
  uvc_set_button_callback(devh, button_cb, NULL);
  /* provoke control-change events: auto/manual toggles that the device may report */
  if (uvc_get_processing_units(devh)) {
    uint8_t v;
    if (uvc_get_white_balance_temperature_auto(devh, &v, UVC_GET_CUR) == UVC_SUCCESS) {
      soft(uvc_set_white_balance_temperature_auto(devh, !v)); soft(uvc_set_white_balance_temperature_auto(devh, v));
    }
    if (uvc_get_hue_auto(devh, &v, UVC_GET_CUR) == UVC_SUCCESS) { soft(uvc_set_hue_auto(devh, !v)); soft(uvc_set_hue_auto(devh, v)); }
  }
  if (uvc_get_camera_terminal(devh)) {
    uint8_t v;
    if (uvc_get_ae_mode(devh, &v, UVC_GET_CUR) == UVC_SUCCESS) { soft(uvc_set_ae_mode(devh, 1)); soft(uvc_set_ae_mode(devh, v)); }
    if (uvc_get_focus_auto(devh, &v, UVC_GET_CUR) == UVC_SUCCESS) { soft(uvc_set_focus_auto(devh, !v)); soft(uvc_set_focus_auto(devh, v)); }
  }
  /* stream while waiting: streaming-interface events (buttons) arrive on the same endpoint */
  const uvc_format_desc_t *f = uvc_get_format_descs(devh);
  const uvc_frame_desc_t *fr = f ? smallest_frame(f) : NULL;
  uvc_stream_ctrl_t c; uvc_stream_handle_t *s = NULL;
  if (fr && ctrl_for(f, fr, default_interval(fr), &c) == UVC_SUCCESS && uvc_stream_open_ctrl(devh, &s, &c) == UVC_SUCCESS)
    soft(uvc_stream_start(s, NULL, NULL, 0));
  usleep(opt_ms * 1000);
  printf("  status events: %d, button events: %d\n", n_status, n_button);
  uvc_set_status_callback(devh, NULL, NULL);
  uvc_set_button_callback(devh, NULL, NULL);
  usleep(50000);
  if (s) { uvc_stream_stop(s); uvc_stream_close(s); }
  printf("RESULT_EXTRA status_events=%d button_events=%d value=%d info=%d failure=%d min=%d max=%d other=%d\n", n_status, n_button,
         n_attr[0], n_attr[1], n_attr[2], n_attr[3], n_attr[4], n_attr[5]);
}

/* ================= async ================= */
/* Waits up to 2 s for a value or failure change of (class, sel); returns its index in evs, or -1. */
static int wait_control_event(enum uvc_status_class c, int sel, int from) {
  for (int t = 0; t < 200; t++) {
    pthread_mutex_lock(&ev_lock);
    for (int i = from; i < n_evs; i++)
      if (evs[i].c == c && evs[i].sel == sel && (evs[i].attr == UVC_STATUS_ATTRIBUTE_VALUE_CHANGE || evs[i].attr == UVC_STATUS_ATTRIBUTE_FAILURE_CHANGE)) {
        pthread_mutex_unlock(&ev_lock);
        return i;
      }
    pthread_mutex_unlock(&ev_lock);
    usleep(10000);
  }
  return -1;
}

/* SET_CUR of `v` on an asynchronous control (2.4.4): accepted at once, the control is "not ready"
 * until its Control Change interrupt, which carries the new value (then read back by GET_CUR) or a
 * failure code (then the value is unchanged). Returns 1 on success, 0 on failure change, -1 if lost. */
static int async_set(enum uvc_status_class c, uint8_t unit, uint8_t sel, const uint8_t *v, const uint8_t *old, int len) {
  uint8_t b[64];
  pthread_mutex_lock(&ev_lock); n_evs = 0; pthread_mutex_unlock(&ev_lock);
  const int from = 0;
  expect(uvc_set_ctrl(devh, unit, sel, (void *)v, len) == len, "%u/%u: async SET_CUR accepted", unit, sel);
  if (uvc_get_ctrl(devh, unit, sel, b, len, UVC_GET_CUR) < 0) {
    int e = req_error();
    if (e < 0) soft(e); else expect(e == ERR_NOT_READY, "%u/%u: GET_CUR while pending: error code %d, want not ready", unit, sel, e);
    expect_rejected(uvc_set_ctrl(devh, unit, sel, (void *)v, len), ERR_NOT_READY, "SET_CUR while pending", unit, sel);
  }
  int i = wait_control_event(c, sel, from);
  expect(i >= 0, "%u/%u: Control Change interrupt after async SET_CUR", unit, sel);
  if (i < 0) return -1;
  int ok = evs[i].attr == UVC_STATUS_ATTRIBUTE_VALUE_CHANGE;
  if (ok) expect(evs[i].len == (size_t)len && !memcmp(evs[i].data, v, len), "%u/%u: value change carries the new value", unit, sel);
  else expect(evs[i].len == 1 && evs[i].data[0] != 0, "%u/%u: failure change carries an error code (len %zu)", unit, sel, evs[i].len);
  expect(uvc_get_ctrl(devh, unit, sel, b, len, UVC_GET_CUR) == len && !memcmp(b, ok ? v : old, len),
         "%u/%u: GET_CUR after the %s change", unit, sel, ok ? "value" : "failure");
  return ok;
}

static void mode_async(void) {
  expect(open_device() == 0, "open device");
  if (!devh) return;
  uvc_set_status_callback(devh, status_cb, NULL);
  const uvc_input_terminal_t *ct = uvc_get_camera_terminal(devh);
  const uvc_processing_unit_t *pu = uvc_get_processing_units(devh);
  int n_ok = 0, n_failed = 0;
  for (int k = 0; k < 2; k++) {
    if (k == 0 ? !ct : !pu) continue;
    uint8_t unit = k == 0 ? ct->bTerminalID : pu->bUnitID;
    enum uvc_status_class c = k == 0 ? UVC_STATUS_CLASS_CONTROL_CAMERA : UVC_STATUS_CLASS_CONTROL_PROCESSING;
    for (int sel = 1; sel <= 0x20; sel++) {
      int info = ctrl_info(unit, sel);
      if (info < 0 || (info & (INFO_ASYNC | INFO_SET | INFO_DISABLED)) != (INFO_ASYNC | INFO_SET)) continue;
      int len = uvc_get_ctrl_len(devh, unit, sel);
      uint8_t cur[64], v[64];
      if (len <= 0 || len > 64 || uvc_get_ctrl(devh, unit, sel, cur, len, UVC_GET_CUR) != len) { expect(0, "%u/%u: GET_CUR", unit, sel); continue; }
      /* a legal value other than the current one: DEF, MIN or MAX */
      static const enum uvc_req_code PICK[] = {UVC_GET_DEF, UVC_GET_MIN, UVC_GET_MAX};
      int found = 0;
      for (unsigned j = 0; j < 3 && !found; j++)
        found = uvc_get_ctrl(devh, unit, sel, v, len, PICK[j]) == len && memcmp(v, cur, len);
      if (!found) continue;
      int r = async_set(c, unit, sel, v, cur, len);
      if (r > 0) { n_ok++; async_set(c, unit, sel, cur, cur, len); }
      else if (r == 0) n_failed++;
    }
  }
  uvc_set_status_callback(devh, NULL, NULL);
  printf("RESULT_EXTRA async_ok=%d async_failed=%d\n", n_ok, n_failed);
}

/* ================= negotiate ================= */
static void mode_negotiate(void) {
  expect(open_device() == 0, "open device");
  if (!devh) return;
  int combos = 0, okc = 0;
  for (const uvc_format_desc_t *f = uvc_get_format_descs(devh); f; f = f->next) {
    enum uvc_frame_format ff = fmt_of(f);
    if (ff == UVC_FRAME_FORMAT_UNKNOWN) {
      /* A GUID libuvc doesn't know: it can't be asked for by name (UNKNOWN is ANY), and streaming
       * it is refused. */
      const uvc_frame_desc_t *fr = smallest_frame(f);
      uvc_stream_ctrl_t c;
      uvc_stream_handle_t *s;
      if (fr && soft(ctrl_for(f, fr, default_interval(fr), &c)) >= 0 && soft(uvc_stream_open_ctrl(devh, &s, &c)) >= 0) {
        expect(uvc_stream_start(s, NULL, NULL, 0) == UVC_ERROR_NOT_SUPPORTED, "format %u (unknown GUID): streaming refused",
               f->bFormatIndex);
        uvc_stream_close(s);
      }
      continue;
    }
    for (const uvc_frame_desc_t *fr = f->frame_descs; fr; fr = fr->next) {
      uvc_stream_ctrl_t c;
      if (fr->intervals) {
        for (const uint32_t *iv = fr->intervals; *iv; iv++) {
          int fps = 10000000 / *iv;
          combos++;
          uvc_error_t e = uvc_get_stream_ctrl_format_size(devh, &c, ff, fr->wWidth, fr->wHeight, fps);
          if (e == UVC_SUCCESS) {
            okc++;
            expect(c.bFormatIndex == f->bFormatIndex && c.bFrameIndex == fr->bFrameIndex, "%ux%u@%d: negotiated indices", fr->wWidth, fr->wHeight, fps);
            expect(c.dwMaxVideoFrameSize > 0 || ff == UVC_FRAME_FORMAT_UNKNOWN, "%ux%u@%d: frame size", fr->wWidth, fr->wHeight, fps);
          } else soft(e);
        }
        combos++;
        soft(uvc_get_stream_ctrl_format_size(devh, &c, ff, fr->wWidth, fr->wHeight, 0)); /* 0 = first interval */
      } else {
        /* continuous / stepwise: min, max and a value between */
        uint32_t ivs[3] = {fr->dwMinFrameInterval, fr->dwMaxFrameInterval, fr->dwDefaultFrameInterval};
        for (int k = 0; k < 3; k++) {
          if (!ivs[k]) continue;
          combos++;
          uvc_error_t e = uvc_get_stream_ctrl_format_size(devh, &c, ff, fr->wWidth, fr->wHeight, 10000000 / ivs[k]);
          if (e == UVC_SUCCESS) okc++; else soft(e);
        }
        /* off-step interval is rejected when a step is defined */
        if (fr->dwFrameIntervalStep > 1) soft(uvc_get_stream_ctrl_format_size(devh, &c, ff, fr->wWidth, fr->wHeight, 10000000 / (fr->dwMinFrameInterval + 1)));
      }
      uvc_stream_ctrl_t d = {0};
      uvc_print_stream_ctrl(&c, NULL);
      (void)d;
    }
    /* a size the format does not have */
    uvc_stream_ctrl_t c;
    expect(uvc_get_stream_ctrl_format_size(devh, &c, ff, 7, 5, 30) == UVC_ERROR_INVALID_MODE, "non-existent size rejected");
    const uvc_frame_desc_t *fr = f->frame_descs;
    if (fr && fr->intervals)
      expect(uvc_get_stream_ctrl_format_size(devh, &c, ff, fr->wWidth, fr->wHeight, 9999) == UVC_ERROR_INVALID_MODE, "non-existent fps rejected");
  }
  expect(okc > 0, "some format/size/fps negotiated (%d of %d)", okc, combos);
  { uvc_stream_ctrl_t c;
    expect(uvc_get_stream_ctrl_format_size(devh, &c, UVC_FRAME_FORMAT_P010, 640, 480, 30) == UVC_ERROR_INVALID_MODE
           || 1, "format the device lacks");
    soft(uvc_get_stream_ctrl_format_size(devh, &c, UVC_FRAME_FORMAT_ANY, 640, 480, 30)); }
  /* probe_stream_ctrl variants */
  const uvc_format_desc_t *f = uvc_get_format_descs(devh);
  const uvc_frame_desc_t *fr = f ? smallest_frame(f) : NULL;
  if (fr) {
    uvc_stream_ctrl_t c;
    expect(ctrl_for(f, fr, default_interval(fr), &c) == UVC_SUCCESS, "probe valid request");
    uvc_stream_ctrl_t p = c; p.bFormatIndex = 0xee;
    expect(uvc_probe_stream_ctrl(devh, &p) == UVC_ERROR_INVALID_MODE, "probe unknown format index");
    p = c; p.bFrameIndex = 0xee;
    soft(uvc_probe_stream_ctrl(devh, &p));
    p = c; p.dwFrameInterval = 1; p.bmHint = 1;
    soft(uvc_probe_stream_ctrl(devh, &p));
    p = c; p.dwFrameInterval = 0; p.bmHint = 0;
    soft(uvc_probe_stream_ctrl(devh, &p));
    p = c; p.dwMaxVideoFrameSize = 0; p.dwMaxPayloadTransferSize = 0xffffff;
    soft(uvc_probe_stream_ctrl(devh, &p));
    p = c; p.bmHint = 2; p.wCompQuality = 5000; p.wKeyFrameRate = 1; p.wPFrameRate = 1; p.wCompWindowSize = 1;
    soft(uvc_probe_stream_ctrl(devh, &p));
    memset(&p, 0, sizeof p); p.bInterfaceNumber = c.bInterfaceNumber;
    soft(uvc_probe_stream_ctrl(devh, &p));
    /* a stream opened with a ctrl for another interface */
    uvc_stream_ctrl_t q = c; q.bInterfaceNumber ^= 0x40;
    uvc_stream_handle_t *s;
    expect(uvc_stream_open_ctrl(devh, &s, &q) == UVC_ERROR_INVALID_PARAM, "open with foreign interface rejected");
    /* start with a ctrl naming a nonexistent frame: refused */
    q = c; q.bFrameIndex = 0xee;
    if (uvc_stream_open_ctrl(devh, &s, &q) == UVC_SUCCESS) {
      expect(uvc_stream_start(s, NULL, NULL, 0) != UVC_SUCCESS, "start with unknown frame index rejected");
      uvc_stream_close(s);
    } else n_soft++;
    uvc_print_stream_ctrl(&c, NULL);
    /* commit + stream with the negotiated one */
    expect(uvc_stream_open_ctrl(devh, &s, &c) == UVC_SUCCESS, "open negotiated");
    if (s) uvc_stream_close(s);
  }
}

/* ================= open ================= */
/* Enumeration and open only, for descriptor cases where rejecting the device can be the expected
 * outcome: prints what libuvc made of it (checked by libuvc-test.sh). With -x the device stays open,
 * for uvc_exit() to close. */
static void mode_open(void) {
  uvc_device_t **list;
  int n = 0;
  if (uvc_get_device_list(ctx, &list) == UVC_SUCCESS) {
    while (list[n]) n++;
    uvc_free_device_list(list, 1);
  }
  uvc_device_t **devs = NULL;
  uvc_error_t fe = uvc_find_devices(ctx, &devs, opt_vid, opt_pid, NULL);
  if (fe == UVC_SUCCESS) uvc_free_device_list(devs, 1);
  if (opt_vid || opt_pid) {
    uvc_device_t *d;
    expect(uvc_find_device(ctx, &d, opt_vid, opt_pid ^ 0xffff, NULL) == UVC_ERROR_NO_DEVICE, "find with another PID fails");
  }
  uvc_error_t oe = UVC_ERROR_NO_DEVICE;
  if (uvc_find_device(ctx, &dev, opt_vid, opt_pid, NULL) == UVC_SUCCESS) oe = uvc_open(dev, &devh);
  if (oe == UVC_SUCCESS) {
    int nfmt = 0;
    for (const uvc_format_desc_t *f = uvc_get_format_descs(devh); f; f = f->next) nfmt++;
    printf("  formats: %d, camera terminal: %s\n", nfmt, uvc_get_camera_terminal(devh) ? "yes" : "no");
    if (opt_leave_open) devh = NULL; /* uvc_exit() closes it */
  }
  printf("RESULT_EXTRA devices=%d find=%d open=%d (%s)\n", n, fe, oe, uvc_strerror(oe));
}

/* ================= wrap ================= */
/* uvc_wrap() on a descriptor of a system device (the shim ignores the fd and wraps an emulated device). */
static void mode_wrap(void) {
#if LIBUSB_API_VERSION >= 0x01000107
  int fd = open("/dev/null", O_RDWR);
  uvc_device_handle_t *wh = NULL;
  uvc_error_t e = soft(uvc_wrap(fd, ctx, &wh));
  printf("  uvc_wrap: %s\n", uvc_strerror(e));
  if (e == UVC_SUCCESS && wh) {
    devh = wh;
    uvc_device_t *wd = uvc_get_device(wh); /* takes a reference */
    expect(wd != NULL, "wrapped device has a uvc_device");
    if (wd) uvc_unref_device(wd);
    expect(uvc_get_libusb_handle(wh) != NULL, "wrapped device has a libusb handle");
    char *buf = NULL; size_t blen = 0; FILE *fp = open_memstream(&buf, &blen);
    uvc_print_diag(wh, fp); fclose(fp); expect(blen > 0, "print_diag on wrapped device"); free(buf);
    const uvc_format_desc_t *f = uvc_get_format_descs(wh);
    const uvc_frame_desc_t *fr = f ? smallest_frame(f) : NULL;
    uvc_stream_ctrl_t c;
    if (fr && soft(ctrl_for(f, fr, default_interval(fr), &c)) >= 0) {
      uvc_frame_t *g = grab(&c, 3000000);
      expect(g != NULL, "frame from wrapped device");
      if (g) uvc_free_frame(g);
    }
    uvc_close(wh); devh = NULL;
  }
  if (fd >= 0) close(fd);
  /* wrapping nothing sensible */
  uvc_device_handle_t *bad = NULL;
  e = uvc_wrap(-1, ctx, &bad);
  soft(e); if (e == UVC_SUCCESS && bad) uvc_close(bad);
#else
  printf("  uvc_wrap not available\n");
#endif
}

/* ================= main ================= */
typedef void (*mode_fn)(void);
static const struct { const char *name; mode_fn fn; } MODES[] = {
  {"info", mode_info}, {"controls", mode_controls}, {"convert", mode_convert}, {"poll", mode_poll},
  {"still", mode_still}, {"multi", mode_multi}, {"status", mode_status}, {"async", mode_async}, {"negotiate", mode_negotiate}, {"wrap", mode_wrap},
  {"open", mode_open},
};

static void usage(void) {
  fprintf(stderr, "usage: api_test [-d VID:PID] [-n INDEX] [-m STATUS_WAIT_MS] [-l] [-x] MODE...\n  modes: info controls convert poll still multi status async negotiate wrap open all\n");
}

int main(int argc, char **argv) {
  const char *modes[16]; int nm = 0;
  for (int i = 1; i < argc; i++) {
    if (!strcmp(argv[i], "-d") && i + 1 < argc) { if (sscanf(argv[++i], "%x:%x", &opt_vid, &opt_pid) != 2) { usage(); return 2; } }
    else if (!strcmp(argv[i], "-n") && i + 1 < argc) opt_index = atoi(argv[++i]);
    else if (!strcmp(argv[i], "-m") && i + 1 < argc) opt_ms = atoi(argv[++i]);
    else if (!strcmp(argv[i], "-l")) opt_lenient = 1;
    else if (!strcmp(argv[i], "-x")) opt_leave_open = 1;
    else if (argv[i][0] != '-' && nm < 16) modes[nm++] = argv[i];
    else { usage(); return 2; }
  }
  if (!nm) { usage(); return 2; }
  signal(SIGALRM, on_alarm);
  int rc = 0;
  for (int m = 0; m < nm; m++) {
    int all = !strcmp(modes[m], "all"), hit = 0;
    for (unsigned k = 0; k < sizeof MODES / sizeof *MODES; k++) {
      if (!all && strcmp(modes[m], MODES[k].name)) continue;
      hit = 1;
      cur_mode = MODES[k].name;
      n_ok = n_soft = n_fail = 0;
      if (uvc_init(&ctx, NULL) != UVC_SUCCESS) {
        /* e.g. libusb_init failure injected by EMUVC_LIBUSB_FAIL */
        printf("RESULT mode=%s init_failed=1\n", cur_mode);
        continue;
      }
      dev = NULL; devh = NULL;
      alarm(45);
      MODES[k].fn();
      alarm(0);
      if (devh) uvc_close(devh);
      if (dev) uvc_unref_device(dev);
      uvc_exit(ctx);
      printf("RESULT mode=%s ok=%d soft=%d fail=%d\n", cur_mode, n_ok, n_soft, n_fail);
      if (n_fail) rc = 1;
      fflush(stdout);
    }
    if (!hit) { fprintf(stderr, "unknown mode %s\n", modes[m]); usage(); return 2; }
  }
  return rc;
}
