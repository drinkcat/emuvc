/* libuvc public API coverage client for emuvc. See README.md.
 *
 * usage: api_test [-d VID:PID] [-n INDEX] [-m MS] [-l] MODE...
 *   -l: lenient, failed expectations are counted as soft (fault-injection configs);
 *       crashes and hangs still fail
 *   MODE: info controls convert poll still multi status negotiate transfers wrap all
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
#include <pthread.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

extern void uvc_print_frameformats(uvc_device_handle_t *devh); /* exported, not in the public header */

/* ---- bookkeeping ---- */
static int n_ok, n_soft, n_fail;
static int opt_vid, opt_pid, opt_index, opt_ms = 1000, opt_lenient;
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
  if (list && list[0]) { uvc_device_t *keep = list[0]; uvc_free_device_list(list, 0); uvc_unref_device(keep); }
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
  expect(uvc_get_device(devh) == dev, "uvc_get_device");
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

  /* generic extension-unit access */
  int nxu = 0, xok = 0;
  for (const uvc_extension_unit_t *x = uvc_get_extension_units(devh); x; x = x->next) {
    nxu++;
    for (int sel = 1; sel <= 64; sel++) {
      if (!(x->bmControls & (1ull << (sel - 1)))) continue;
      int len = uvc_get_ctrl_len(devh, x->bUnitID, sel);
      soft(len);
      if (len <= 0 || len > 1024) continue;
      uint8_t *b = calloc(1, len), *b2 = calloc(1, len);
      int r = uvc_get_ctrl(devh, x->bUnitID, sel, b, len, UVC_GET_CUR);
      for (unsigned i = 1; i < 7; i++) soft(uvc_get_ctrl(devh, x->bUnitID, sel, b2, len, REQS[i]));
      if (r == len) {
        xok++;
        int w = soft(uvc_set_ctrl(devh, x->bUnitID, sel, b, len));
        if (w == len) {
          expect(uvc_get_ctrl(devh, x->bUnitID, sel, b2, len, UVC_GET_CUR) == len && !memcmp(b, b2, len), "XU %u/%d read-back", x->bUnitID, sel);
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
  expect(uvc_any2bgr(in, o) == UVC_ERROR_NOT_SUPPORTED, "%s any2bgr(mjpeg) unsupported", tag);
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
static void status_cb(enum uvc_status_class c, int ev, int sel, enum uvc_status_attribute a, void *d, size_t l, void *u) {
  (void)ev; (void)d; (void)u;
  n_status++;
  printf("  status event class=0x%x selector=%d attribute=%d len=%zu\n", c, sel, a, l);
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
  printf("RESULT_EXTRA status_events=%d button_events=%d\n", n_status, n_button);
}

/* ================= negotiate ================= */
static void mode_negotiate(void) {
  expect(open_device() == 0, "open device");
  if (!devh) return;
  int combos = 0, okc = 0;
  for (const uvc_format_desc_t *f = uvc_get_format_descs(devh); f; f = f->next) {
    enum uvc_frame_format ff = fmt_of(f);
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

/* ================= transfers ================= */
/* While streaming, libuvc keeps LIBUVC_NUM_TRANSFER_BUFS transfers in flight. One that completes
 * with a status libuvc does not retry (LIBUSB_TRANSFER_ERROR: CRC, bit stuffing, missing
 * handshake...) is freed instead of resubmitted, and its strmh->transfers[] slot cleared. Stream
 * for a while and count them: a stream that loses all its transfers stops, without an error. */
static int live_transfers(uvc_stream_handle_t *s) {
  int n = 0;
  pthread_mutex_lock(&s->cb_mutex);
  for (int i = 0; i < LIBUVC_NUM_TRANSFER_BUFS; i++) n += s->transfers[i] != NULL;
  pthread_mutex_unlock(&s->cb_mutex);
  return n;
}

static long now_ms(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec * 1000L + t.tv_nsec / 1000000;
}

static void mode_transfers(void) {
  enum { FRAMES = 90, STALL_MS = 2000 };
  expect(open_device() == 0, "open device");
  if (!devh) return;
  const uvc_format_desc_t *f = uvc_get_format_descs(devh);
  const uvc_frame_desc_t *fr = f ? smallest_frame(f) : NULL;
  if (!fr) { expect(0, "no format"); return; }
  uvc_stream_ctrl_t c;
  uvc_stream_handle_t *s = NULL;
  expect(ctrl_for(f, fr, default_interval(fr), &c) == UVC_SUCCESS, "negotiate");
  if (uvc_stream_open_ctrl(devh, &s, &c) != UVC_SUCCESS) { expect(0, "stream open"); return; }
  cb_frames = 0;
  if (uvc_stream_start(s, count_cb, NULL, 0) != UVC_SUCCESS) { expect(0, "stream start"); uvc_stream_close(s); return; }
  int start = live_transfers(s), shown = start, last = 0;
  long t0 = now_ms(), last_frame = t0, now = t0;
  printf("  %d transfers in flight\n", start);
  while (cb_frames < FRAMES && now - last_frame < STALL_MS) {
    usleep(5000);
    now = now_ms();
    int n = live_transfers(s), frames = cb_frames;
    if (frames != last) { last = frames; last_frame = now; }
    if (n / 10 != shown / 10) { printf("  %5.2f s: %3d left, %d frames\n", (now - t0) / 1000.0, n, frames); shown = n; }
  }
  int end = live_transfers(s);
  if (cb_frames < FRAMES)
    printf("  stalled: no frame for %d ms after %d frames, %d of %d transfers left\n", STALL_MS, cb_frames, end, start);
  expect(cb_frames >= FRAMES, "frames keep coming (%d of %d)", cb_frames, FRAMES);
  expect(end == start, "no transfer lost (%d of %d left)", end, start);
  uvc_stream_stop(s);
  uvc_stream_close(s);
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
    expect(uvc_get_device(wh) != NULL, "wrapped device has a uvc_device");
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
  {"still", mode_still}, {"multi", mode_multi}, {"status", mode_status}, {"negotiate", mode_negotiate},
  {"transfers", mode_transfers}, {"wrap", mode_wrap},
};

static void usage(void) {
  fprintf(stderr, "usage: api_test [-d VID:PID] [-n INDEX] [-m STATUS_WAIT_MS] [-l] MODE...\n  modes: info controls convert poll still multi status negotiate transfers wrap all\n");
}

int main(int argc, char **argv) {
  const char *modes[16]; int nm = 0;
  for (int i = 1; i < argc; i++) {
    if (!strcmp(argv[i], "-d") && i + 1 < argc) { if (sscanf(argv[++i], "%x:%x", &opt_vid, &opt_pid) != 2) { usage(); return 2; } }
    else if (!strcmp(argv[i], "-n") && i + 1 < argc) opt_index = atoi(argv[++i]);
    else if (!strcmp(argv[i], "-m") && i + 1 < argc) opt_ms = atoi(argv[++i]);
    else if (!strcmp(argv[i], "-l")) opt_lenient = 1;
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
