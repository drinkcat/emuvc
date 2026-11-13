/* End-to-end libuvc test client for emuvc. See README.md. */
#define _GNU_SOURCE
#include <libuvc/libuvc.h>
#include <jpeglib.h>
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <time.h>

/* ---- frame stamp (see crates/emuvc-video/src/stamp.rs doc) ---- */
#define CELLS 42
static uint8_t crc8(const uint8_t *d, int n) {
  uint8_t crc = 0;
  for (int i = 0; i < n; i++) {
    crc ^= d[i];
    for (int b = 0; b < 8; b++) crc = (crc & 0x80) ? (uint8_t)((crc << 1) ^ 0x07) : (uint8_t)(crc << 1);
  }
  return crc;
}

/* luma: rows of `stride` bytes, at least band rows available. */
static int decode_luma(const uint8_t *luma, int w, int h, int stride, uint32_t *out) {
  int band = (h / 10 > 8 ? h / 10 : 8) & ~1;
  if (band > h) band = h;
  if (w < CELLS || band <= 0) return 0;
  int y0 = band / 4, y1 = band - band / 4;
  int bits[CELLS];
  for (int i = 0; i < CELLS; i++) {
    int cx0 = i * w / CELLS, cx1 = (i + 1) * w / CELLS;
    int q = (cx1 - cx0) / 4;
    int x0 = cx0 + q, x1 = cx1 - q;
    if (x1 < x0 + q + 1 - q) x1 = x0 + 1;
    if (x1 < cx0 + q + 1) x1 = cx0 + q + 1;
    long sum = 0;
    for (int y = y0; y < y1; y++)
      for (int x = x0; x < x1; x++) sum += luma[(size_t)y * stride + x];
    long n = (long)(y1 - y0) * (x1 - x0);
    bits[i] = sum >= 128 * n;
  }
  if (!bits[0] || bits[41]) return 0;
  uint64_t payload = 0;
  for (int i = 1; i <= 40; i++) payload = (payload << 1) | (uint64_t)bits[i];
  uint32_t v = (uint32_t)(payload >> 8);
  uint8_t be[4] = {v >> 24, v >> 16, v >> 8, v};
  if ((uint8_t)payload != crc8(be, 4)) return 0;
  *out = v;
  return 1;
}

struct jerr { struct jpeg_error_mgr pub; jmp_buf jb; };
static void jerr_exit(j_common_ptr c) { longjmp(((struct jerr *)c->err)->jb, 1); }
static void jerr_msg(j_common_ptr c, int l) { (void)c; (void)l; }

/* libjpeg-turbo supplies the default Huffman tables if the stream has no DHT. */
static int decode_mjpeg(const uint8_t *data, size_t len, int w, int h, uint32_t *out) {
  struct jpeg_decompress_struct c;
  struct jerr je;
  int ok = 0;
  uint8_t *luma = NULL;
  c.err = jpeg_std_error(&je.pub);
  je.pub.error_exit = jerr_exit;
  je.pub.emit_message = jerr_msg;
  if (setjmp(je.jb)) { jpeg_destroy_decompress(&c); free(luma); return 0; }
  jpeg_create_decompress(&c);
  jpeg_mem_src(&c, data, len);
  jpeg_read_header(&c, TRUE);
  if ((int)c.image_width != w || (int)c.image_height != h) { jpeg_destroy_decompress(&c); return 0; }
  c.out_color_space = JCS_GRAYSCALE;
  jpeg_start_decompress(&c);
  int band = (h / 10 > 8 ? h / 10 : 8) & ~1;
  if (band > h) band = h;
  luma = malloc((size_t)w * band);
  for (int y = 0; y < band; y++) {
    JSAMPROW row = luma + (size_t)y * w;
    jpeg_read_scanlines(&c, &row, 1);
  }
  ok = decode_luma(luma, w, h, w, out);
  free(luma);
  jpeg_abort_decompress(&c);
  jpeg_destroy_decompress(&c);
  return ok;
}

static int decode_yuyv(const uint8_t *d, int w, int h, uint32_t *out) {
  int band = (h / 10 > 8 ? h / 10 : 8) & ~1;
  if (band > h) band = h;
  uint8_t *luma = malloc((size_t)w * band);
  for (int y = 0; y < band; y++)
    for (int x = 0; x < w; x++) luma[(size_t)y * w + x] = d[((size_t)y * w + x) * 2];
  int ok = decode_luma(luma, w, h, w, out);
  free(luma);
  return ok;
}

/* ---- shared state ---- */
static pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static struct {
  int mjpeg, w, h;
  long target, frames, corrupt, gaps, max_gap;
  int have_prev; uint32_t prev;
  double t_first, t_last;
  int done;
} S;

static double now(void) {
  struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec + ts.tv_nsec * 1e-9;
}

static void cb(uvc_frame_t *f, void *arg) {
  (void)arg;
  size_t exp = (size_t)S.w * S.h * 2;
  const uint8_t *d = f->data;
  uint32_t v = 0;
  int ok;
  if (S.mjpeg)
    ok = f->data_bytes >= 4 && d[0] == 0xFF && d[1] == 0xD8 && d[f->data_bytes - 2] == 0xFF &&
         d[f->data_bytes - 1] == 0xD9 && decode_mjpeg(d, f->data_bytes, S.w, S.h, &v);
  else
    ok = f->data_bytes == exp && (int)f->width == S.w && (int)f->height == S.h && decode_yuyv(d, S.w, S.h, &v);
  pthread_mutex_lock(&mu);
  if (!S.done) {
    if (!ok) {
      S.corrupt++;
    } else {
      double t = now();
      if (S.frames == 0) S.t_first = t;
      S.t_last = t;
      if (S.have_prev && v != S.prev + 1) {
        long gap = v > S.prev ? (long)(v - S.prev - 1) : 1;
        S.gaps++;
        if (gap > S.max_gap) S.max_gap = gap;
      }
      S.prev = v; S.have_prev = 1;
      S.frames++;
      if (S.frames >= S.target) { S.done = 1; pthread_cond_broadcast(&cv); }
    }
  }
  pthread_mutex_unlock(&mu);
}

/* ---- controls ---- */
static int cfails;
#define CHECK(cond, ...) do { if (cond) printf("  ok   "); else { printf("  FAIL "); cfails++; } printf(__VA_ARGS__); printf("\n"); } while (0)

static void test_controls(uvc_device_handle_t *h) {
  int16_t mn = 0, mx = 0, res = 0, def = 0, cur = 0, rb = 0;
  uvc_error_t e;
  e = uvc_get_brightness(h, &cur, UVC_GET_CUR); CHECK(e == UVC_SUCCESS, "brightness GET_CUR = %d (%s)", cur, uvc_strerror(e));
  e = uvc_get_brightness(h, &mn, UVC_GET_MIN);  CHECK(e == UVC_SUCCESS, "brightness GET_MIN = %d (%s)", mn, uvc_strerror(e));
  e = uvc_get_brightness(h, &mx, UVC_GET_MAX);  CHECK(e == UVC_SUCCESS, "brightness GET_MAX = %d (%s)", mx, uvc_strerror(e));
  e = uvc_get_brightness(h, &res, UVC_GET_RES); CHECK(e == UVC_SUCCESS, "brightness GET_RES = %d (%s)", res, uvc_strerror(e));
  e = uvc_get_brightness(h, &def, UVC_GET_DEF); CHECK(e == UVC_SUCCESS, "brightness GET_DEF = %d (%s)", def, uvc_strerror(e));
  CHECK(mn < mx, "min < max");
  CHECK(def >= mn && def <= mx, "default within range");
  int16_t vals[3] = {mn, mx, def};
  const char *names[3] = {"min", "max", "default"};
  for (int i = 0; i < 3; i++) {
    e = uvc_set_brightness(h, vals[i]);
    CHECK(e == UVC_SUCCESS, "set brightness %s (%d): %s", names[i], vals[i], uvc_strerror(e));
    rb = 12345;
    e = uvc_get_brightness(h, &rb, UVC_GET_CUR);
    CHECK(e == UVC_SUCCESS && rb == vals[i], "read back %d (expect %d)", rb, vals[i]);
  }
  if (mx < 32767) {
    e = uvc_set_brightness(h, (int16_t)(mx + 1));
    CHECK(e != UVC_SUCCESS, "set above max (%d) fails: %s", mx + 1, uvc_strerror(e));
  }
  if (mn > -32768) {
    e = uvc_set_brightness(h, (int16_t)(mn - 1));
    CHECK(e != UVC_SUCCESS, "set below min (%d) fails: %s", mn - 1, uvc_strerror(e));
  }
  uint8_t ae = 0xff;
  e = uvc_get_ae_mode(h, &ae, UVC_GET_CUR); CHECK(e == UVC_SUCCESS, "AE mode GET_CUR = %u (%s)", ae, uvc_strerror(e));
  uint8_t ae_def = 0;
  e = uvc_get_ae_mode(h, &ae_def, UVC_GET_DEF); CHECK(e == UVC_SUCCESS, "AE mode GET_DEF = %u (%s)", ae_def, uvc_strerror(e));
  static const uint8_t modes[] = {1, 8, 2, 4}; /* manual, aperture priority, auto, shutter priority */
  for (unsigned i = 0; i < sizeof modes; i++) {
    e = uvc_set_ae_mode(h, modes[i]);
    printf("  info set AE mode %u: %s\n", modes[i], uvc_strerror(e));
    if (e == UVC_SUCCESS) {
      uint8_t r = 0xff;
      e = uvc_get_ae_mode(h, &r, UVC_GET_CUR);
      CHECK(e == UVC_SUCCESS && r == modes[i], "AE mode read back %u (expect %u)", r, modes[i]);
    }
  }
}

static void usage(void) {
  fprintf(stderr, "usage: stream_test [-d VID:PID] [-f mjpeg|yuyv] [-s WxH] [-r FPS] [-n FRAMES] [-t TIMEOUT_S]\n"
                  "       [--allow-gaps] [--allow-corrupt] [--expect-open-fail] [--controls] [--list] [--fps-tol PCT]\n");
}

int main(int argc, char **argv) {
  int vid = 0, pid = 0, w = 640, h = 480, fps = 30, list = 0, controls = 0;
  int allow_gaps = 0, allow_corrupt = 0, expect_open_fail = 0;
  double timeout = 20, fps_tol = 0;
  const char *fmt = "mjpeg";
  S.target = 30;
  for (int i = 1; i < argc; i++) {
    const char *a = argv[i];
    #define NEXT() (++i < argc ? argv[i] : (usage(), exit(2), (char *)0))
    if (!strcmp(a, "-d")) { if (sscanf(NEXT(), "%x:%x", &vid, &pid) != 2) { usage(); return 2; } }
    else if (!strcmp(a, "-f")) fmt = NEXT();
    else if (!strcmp(a, "-s")) { if (sscanf(NEXT(), "%dx%d", &w, &h) != 2) { usage(); return 2; } }
    else if (!strcmp(a, "-r")) fps = atoi(NEXT());
    else if (!strcmp(a, "-n")) S.target = atol(NEXT());
    else if (!strcmp(a, "-t")) timeout = atof(NEXT());
    else if (!strcmp(a, "--fps-tol")) fps_tol = atof(NEXT());
    else if (!strcmp(a, "--allow-gaps")) allow_gaps = 1;
    else if (!strcmp(a, "--allow-corrupt")) allow_corrupt = 1;
    else if (!strcmp(a, "--expect-open-fail")) expect_open_fail = 1;
    else if (!strcmp(a, "--controls")) controls = 1;
    else if (!strcmp(a, "--list")) list = 1;
    else { usage(); return 2; }
  }
  if (!strcmp(fmt, "mjpeg")) S.mjpeg = 1;
  else if (strcmp(fmt, "yuyv")) { usage(); return 2; }
  S.w = w; S.h = h;

  uvc_context_t *ctx;
  uvc_error_t e = uvc_init(&ctx, NULL);
  if (e) { fprintf(stderr, "uvc_init: %s\n", uvc_strerror(e)); return 2; }

  if (list) {
    uvc_device_t **devs;
    e = uvc_get_device_list(ctx, &devs);
    if (e) { fprintf(stderr, "uvc_get_device_list: %s\n", uvc_strerror(e)); return 2; }
    int n = 0;
    for (int i = 0; devs[i]; i++) {
      uvc_device_descriptor_t *dd;
      if (uvc_get_device_descriptor(devs[i], &dd)) continue;
      printf("device %04x:%04x bus=%u addr=%u serial=%s\n", dd->idVendor, dd->idProduct,
             uvc_get_bus_number(devs[i]), uvc_get_device_address(devs[i]), dd->serialNumber ? dd->serialNumber : "(none)");
      uvc_free_device_descriptor(dd);
      n++;
    }
    printf("devices: %d\n", n);
    uvc_free_device_list(devs, 1);
    uvc_exit(ctx);
    return 0;
  }

  uvc_device_t *dev;
  e = uvc_find_device(ctx, &dev, vid, pid, NULL);
  if (e) {
    fprintf(stderr, "uvc_find_device: %s\n", uvc_strerror(e));
    printf("RESULT open_failed=1\n");
    return expect_open_fail ? 0 : 2;
  }
  uvc_device_handle_t *devh;
  e = uvc_open(dev, &devh);
  if (e) {
    fprintf(stderr, "uvc_open: %s\n", uvc_strerror(e));
    printf("RESULT open_failed=1\n");
    return expect_open_fail ? 0 : 1;
  }
  if (expect_open_fail) { fprintf(stderr, "open succeeded but --expect-open-fail given\n"); return 1; }

  int rc = 0;
  if (controls) {
    test_controls(devh);
    printf("RESULT control_failures=%d\n", cfails);
    rc = cfails ? 1 : 0;
    goto out;
  }

  uvc_stream_ctrl_t ctrl;
  e = uvc_get_stream_ctrl_format_size(devh, &ctrl, S.mjpeg ? UVC_FRAME_FORMAT_MJPEG : UVC_FRAME_FORMAT_YUYV, w, h, fps);
  if (e) { fprintf(stderr, "uvc_get_stream_ctrl_format_size: %s\n", uvc_strerror(e)); printf("RESULT stream_ctrl_failed=1\n"); rc = 2; goto out; }
  e = uvc_start_streaming(devh, &ctrl, cb, NULL, 0);
  if (e) { fprintf(stderr, "uvc_start_streaming: %s\n", uvc_strerror(e)); printf("RESULT start_failed=1\n"); rc = 1; goto out; }

  struct timespec ts; clock_gettime(CLOCK_REALTIME, &ts);
  ts.tv_sec += (time_t)timeout; ts.tv_nsec += (long)((timeout - (time_t)timeout) * 1e9);
  if (ts.tv_nsec >= 1000000000L) { ts.tv_sec++; ts.tv_nsec -= 1000000000L; }
  pthread_mutex_lock(&mu);
  while (!S.done)
    if (pthread_cond_timedwait(&cv, &mu, &ts)) break;
  int done = S.done;
  S.done = 1;
  long frames = S.frames, corrupt = S.corrupt, gaps = S.gaps, max_gap = S.max_gap;
  double dt = S.t_last - S.t_first;
  pthread_mutex_unlock(&mu);
  uvc_stop_streaming(devh);

  double mfps = (frames > 1 && dt > 0) ? (frames - 1) / dt : 0;
  printf("RESULT frames=%ld corrupt=%ld gaps=%ld max_gap=%ld fps=%.2f\n", frames, corrupt, gaps, max_gap, mfps);
  if (!done) { fprintf(stderr, "FAIL: timeout after %.0fs with %ld/%ld frames\n", timeout, frames, S.target); rc = 1; }
  if (corrupt && !allow_corrupt) { fprintf(stderr, "FAIL: %ld corrupt frames\n", corrupt); rc = 1; }
  if (gaps && !allow_gaps) { fprintf(stderr, "FAIL: %ld gaps (max %ld)\n", gaps, max_gap); rc = 1; }
  if (fps_tol > 0 && (mfps < fps * (1 - fps_tol / 100) || mfps > fps * (1 + fps_tol / 100))) {
    fprintf(stderr, "FAIL: fps %.2f not within %.0f%% of %d\n", mfps, fps_tol, fps); rc = 1;
  }
out:
  uvc_close(devh);
  uvc_unref_device(dev);
  uvc_exit(ctx);
  return rc;
}
