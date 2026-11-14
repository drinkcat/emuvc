# Test gaps (draft, 2026-11-14)

What emuvc and the libuvc suite don't cover yet. Sources: libuvc coverage (gcov), every
libuvc/libuvc and drinkcat/libuvc issue and PR, the kernel's uvcvideo driver, the UVC 1.5
spec set, and Microsoft's UVC documentation. Effort: S = config or <50 lines, M = new knob or
feature, L = large.

## libuvc coverage
`scripts/libuvc-coverage.sh`, libuvc master 117df3f:

| | lines | functions | branches | (without utlist macros) | conditions |
|---|---:|---:|---:|---:|---:|
| before | 93.7% | 99.0% | 79.7% | 81.9% | 78.9% |
| now | 96.5% | 99.0% | 85.0% | 87.9% | 84.4% |

What closed the gap: descriptor validation cases (`desc-validation.toml`, the `truncate` patch),
typed setters run with the auto modes off, the Digital Window control, still capture on non
method 2 cameras, unknown format GUIDs, header/probe quirks, interrupt timeouts, per-transfer-type
`submit_transfer_*` failures. Left:
- Needs emulator work: iSight payloads (`stream.c` `is_isight`), TIS 199e:8101/8102 and other
  vendor-class interfaces (standard-descriptor patches), a VS output header (video output
  interface), frame data past dwMaxVideoFrameSize (an overrun quirk).
- Unreachable through libusb: a descriptor overrunning the VC/VS descriptors (libusb drops or
  absorbs it first); unused functions (`uvc_already_open`, `uvc_num_devices`).
- Not worth it: malloc failures, utlist macro internals, switch defaults.

## libuvc findings
Reproduced (known-bug cases in `scripts/libuvc-test.sh`: XFAIL until libuvc is fixed):
- `api-desc-composite-input`: any input terminal shorter than 15 bytes rejects the device
  (`uvc_parse_vc_input_terminal` checks the length before the type), so a valid 8-byte composite
  or S-Video input terminal makes the whole camera unusable.
- `api-desc-bcduvc-0101`: bcdUVC other than 0x0100/0x010a/0x0110/0x0150 makes `uvc_open` fail with
  NOT_SUPPORTED. The kernel accepts any value.
- `api-probe-zero-index`: a probe reply with bFormatIndex/bFrameIndex 0 (Fuji) fails negotiation;
  uvcvideo keeps the format it asked for.
- `api-max-payload-huge`: dwMaxPayloadTransferSize 0xffff0400 (Chicony) leaves no alternate setting
  to pick; uvcvideo masks it.
- `api-exit-open`: `uvc_exit()` closes the devices still open with `DL_FOREACH` while `uvc_close()`
  frees each one: heap-use-after-free (ASan). `DL_FOREACH_SAFE` fixes it.

To confirm (from reading code and the other sources):
- EOF in a header-only payload is ignored (EOF is checked only when data_len > 0), so frames are
  delayed or lost (uvcvideo `uvc_video_decode_end` handles it). Needs an `eof_payload` quirk (S).
- ERR on one mid-frame payload delivers a frame with a hole (needs `err_payload`, S).
- dwMaxPayloadTransferSize huge on bulk: libuvc mallocs 4x that size, config S.
- dwMaxVideoFrameSize 0 on frame-based formats gives empty frames (no dwMaxVideoFrameBufferSize), config S.
- Short control replies: the typed getters return the byte count and leave the out param unset (fault `truncate`, S).
- Typed getters/setters dereference NULL when the unit is absent (`uvc_get_camera_terminal(devh)->...`, #85), S.
- A zero in a discrete interval list ends the interval loop early (patch, S).
- Class-specific VS descriptors after the endpoints: libuvc reads only altsetting[0].extra (needs standard-descriptor patches, M).

## libuvc issues labelled `emuvc`
Retested against libuvc master (40136c5) on 2026-11-14:
- [#282](https://github.com/libuvc/libuvc/issues/282): a probe reply with a different
  dwMaxPayloadTransferSize, smaller (`issue-282-payload-changes`) or larger as reported (3072,
  `issue-282-larger-payload`), negotiates and streams since libuvc#292.
- [#276](https://github.com/libuvc/libuvc/issues/276): GET_MAX reporting dwMaxPayloadTransferSize 0
  (`issue-276-get-max-zero-payload`) works since libuvc#292. libuvc#320 isn't needed for either, and
  breaks `zero-max-payload` (a commit with 0 then falls back on the frame size, which no alternate
  setting fits, so the stream doesn't start).
- [#270](https://github.com/libuvc/libuvc/issues/270): libuvc has sized SuperSpeed endpoints from
  wBytesPerInterval since 2019 (86ad964), as uvcvideo does (`ss-burst`).
- [#267](https://github.com/libuvc/libuvc/issues/267): the ELP 32e4:4689 at 640x360/260 fps, built from
  the issue's diagnostics: polling uvc_stream_get_frame() doesn't time out (`api-issue-267-elp-260fps`).
  Open: with a callback, frame delivery collapses above ~240 fps (~30 fps delivered, the rest skipped
  by libuvc's hand-off) while the CPU isn't the limit. Not understood yet.
- [#242](https://github.com/libuvc/libuvc/issues/242): a compliant camera with the FT602's descriptors
  (0403:602a, GREY 1280x1024/30) negotiates. A probe reply that changes bFormatIndex/bFrameIndex
  gives exactly the reported `Invalid mode (-51)` (`api-probe-zero-index`).
- [#296](https://github.com/libuvc/libuvc/issues/296): both stop sequences from the report run clean
  under ASan on Linux (`api-poll` stops a stream while another thread waits in
  uvc_stream_get_frame). The crash in `usbi_mutex_lock` looks specific to libusb's Windows backend.
- [#230](https://github.com/libuvc/libuvc/issues/230): button presses reach uvc_set_button_callback()
  (`api-status-edge`, `api-status-timeout`).

## Backlog by theme
**Payloads**: MJPEG with no EOF and no FID toggle (`UVC_QUIRK_MJPEG_NO_EOF`, S); header length
0/1 (S); HLE 255 (S); frames longer than width x height, header-only frames, MJPEG without SOI
(#183 #184 #209 #182 #212, S-M); bulk payload > dwMaxPayloadTransferSize and the ZLP rule
(2.4.3.2.1; `bulk_payload_size` is unused, S); SuperSpeed payloads larger than one packet
(#270 #191 #263, S); iSight 05ac:8501 framing (M); stream error code control, VS stream-error
interrupts, dynamic frame rate/format (2.4.3.5-6, M).

**Probe/commit**: GET_RES on probe STALLs but the spec lists it (S); hard checks on probe contents
through `uvc_query_stream_ctrl` (S-M); SET_CUR(PROBE) stalls when dwMaxVideoFrameSize is 0
(PROBE_EXTRAFIELDS, S); device needs SET_INTERFACE(0) first (XBox Live Vision, S-M); bandwidth
retries with a lower wCompQuality (M); two intervals with the same integer fps (#297, S); still
larger than video (#256, PR #281, S); probe stalls leave the ctrl uninitialised (#242/#223, S).

**Descriptors**: vendor-class 0xff interfaces (QuickCam 046d:08c1-08c7, Oculus 2833:0211, TIS,
M); two VC+VS functions (stereo; #170 #265 #241 #302, PR #301, M); Luxvisions IR function
untested (S); multiple configurations (after #16); format with zero frames, missing VS input
header, VC header without VS interfaces (#172 #195 #300 #71, S-M); odd status endpoints (M);
synthetic knobs no config uses yet: still 0/1, no trigger, no status endpoint, no selector,
3-4 streams, `ProbeOverride.raw` (S each); H.264/VP8 descriptors and encoding unit controls
(after #16, L).

**Controls**: auto-update drift and Control Change interrupts for AE/AF/WB (S-M); relative
controls moving the absolute ones (S); error codes 0x03 (power), 0x05/0x06 asserted (S);
multi-field range checks, power line frequency 3 (1.5) (S); MSXU preset (GUID
DC953F0F-3226-4E4C-92C9-A04782F43BC8, selectors 1-0x10, metadata control for uvcvideo's UVCM,
S-M); cancelable controls (M).

**Host and transport (shim)**: iso submit ENOMEM above a size (usbfs limit, Android; #326 #299
#204, M); `release_interface` completing the status URB with 0 bytes (PR #335, M); stop/close
after unplug with cancel returning NO_DEVICE (#152 #53 #16 #146 #296, M); bulk stop with
CLEAR_HALT (#257, M); open/stream/close soak (#261 #227 #260 #293, S); disconnect during a
control SET, between find and open, at close (#255 #155 #283, S); status packets >16 bytes (S);
MS OS 2.0 descriptor set via vendor request (S-M).

**VM (uvcvideo as the oracle)**: run every config in the VM and assert the kernel's "UVC non
compliance" messages; pairs with `modprobe uvcvideo quirks=`; debugfs stats `invalid=0`; MJPEG,
bulk, UVC 1.0, two streams, metadata node, control events, disconnect/unbind.

**Infrastructure**: standard-descriptor patches (interface, endpoint, IAD, config: unlocks
several items above), a second configuration and a second function in `synthetic`, re-enumeration.
