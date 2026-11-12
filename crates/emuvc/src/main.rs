//! `emuvc`: command line front-end.
//!
//! - `emuvc models` — list device models.
//! - `emuvc descriptors [MODEL]` — show (or `--raw` dump) a model's descriptors.
//! - `emuvc selftest [MODEL]` — stream every format/frame in-process and check frames.
//! - `emuvc usbip [--listen ADDR] [-d MODEL].. [-c CONFIG]` — export devices over USB/IP.
//! - `emuvc usbip-attach [--server ADDR] BUSID` — attach an exported device to vhci-hcd (root).
//! - `emuvc run [-d MODEL].. [-c CONFIG] -- CMD..` — run a program with the libusb shim preloaded.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use emuvc_devices::{BusConfig, DEFAULT_MODEL, DeviceConfig, MODELS};
use emuvc_usb::desc::{self, ConfigView, dtype};
use emuvc_usb::{ClockMode, Host, HostConfig};
use emuvc_uvc::host::capture;
use emuvc_uvc::topology::{FormatKind, Transfer, find_video_control_interfaces, parse_function};
use emuvc_video::{PixelFormat, stamp};

#[derive(Parser)]
#[command(name = "emuvc", version, about = "USB Video Class camera emulator")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List device models.
    Models,
    /// Print a model's descriptors as a host enumerates them.
    Descriptors {
        #[arg(default_value = DEFAULT_MODEL)]
        model: String,
        /// Write a raw dump (device + configuration descriptors, the sysfs
        /// `descriptors` format) usable with `generic:FILE`.
        #[arg(long)]
        raw: Option<PathBuf>,
    },
    /// Stream every format/frame of a device in-process (virtual clock) and
    /// check frame sizes and stamps.
    Selftest {
        #[arg(default_value = DEFAULT_MODEL)]
        model: String,
        /// Bus configuration file instead of a model name.
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Frames per format/frame.
        #[arg(short, default_value_t = 3)]
        n: usize,
    },
    /// Export emulated devices over USB/IP (server).
    Usbip {
        /// Address to listen on (loopback by default: the server is unauthenticated).
        #[arg(long, default_value = "127.0.0.1:3240")]
        listen: String,
        /// Device model (repeat for several; `generic:FILE` for a dump).
        #[arg(short, long)]
        device: Vec<String>,
        /// Bus configuration file (TOML).
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// realtime or virtual.
        #[arg(long)]
        clock: Option<String>,
    },
    /// Attach a USB/IP exported device to the local vhci-hcd (like `usbip attach`; needs root).
    UsbipAttach {
        #[arg(long, default_value = "127.0.0.1:3240")]
        server: String,
        /// Bus id of the device, e.g. 1-1.
        busid: String,
    },
    /// Check the stamps of frames captured from an emulated camera (e.g.
    /// `v4l2-ctl --stream-to=FILE`): a raw file of back-to-back frames.
    CheckFrames {
        /// yuyv, uyvy, nv12 or grey.
        #[arg(short, long)]
        format: String,
        /// WIDTHxHEIGHT.
        #[arg(short, long)]
        size: String,
        file: PathBuf,
    },
    /// Run a program against emulated devices (LD_PRELOADs the libusb shim).
    Run {
        /// Device model (repeat for several; `generic:FILE` for a dump).
        #[arg(short, long)]
        device: Vec<String>,
        /// Bus configuration file (TOML).
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// realtime or virtual.
        #[arg(long)]
        clock: Option<String>,
        /// Path to libemuvc_libusb.so (default: next to this binary).
        #[arg(long)]
        shim: Option<PathBuf>,
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::new().filter_or("EMUVC_LOG", "warn")).init();
    let r = match Cli::parse().cmd {
        Cmd::Models => {
            for m in MODELS {
                println!("{:24} {}", m.name, m.description);
            }
            Ok(())
        }
        Cmd::Descriptors { model, raw } => descriptors(&model, raw),
        Cmd::Selftest { model, config, n } => selftest(&model, config, n),
        Cmd::Usbip {
            listen,
            device,
            config,
            clock,
        } => usbip(&listen, device, config, clock),
        Cmd::UsbipAttach { server, busid } => emuvc_usbip::attach_vhci(server.as_str(), &busid)
            .map(|port| println!("attached {busid} to vhci port {port}"))
            .map_err(|e| format!("attach {busid}: {e}")),
        Cmd::CheckFrames { format, size, file } => check_frames(&format, &size, &file),
        Cmd::Run {
            device,
            config,
            clock,
            shim,
            command,
        } => run(device, config, clock, shim, command),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("emuvc: {e}");
            ExitCode::FAILURE
        }
    }
}

fn device_config(model: &str) -> DeviceConfig {
    match model.split_once(':') {
        Some(("generic", path)) => DeviceConfig {
            descriptors: Some(path.into()),
            ..DeviceConfig::model("generic")
        },
        _ => DeviceConfig::model(model),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn descriptors(model: &str, raw: Option<PathBuf>) -> Result<(), String> {
    let host = Host::new(HostConfig {
        clock: ClockMode::Virtual,
        ..Default::default()
    });
    let info = host
        .attach(emuvc_devices::build(&device_config(model))?)
        .map_err(|e| e.to_string())?;
    if let Some(path) = raw {
        let mut out = info.device_descriptor.clone();
        info.configs.iter().for_each(|c| out.extend_from_slice(c));
        return std::fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()));
    }
    println!("{} ({:?} speed)", info.name, info.speed);
    for (label, s) in [
        ("manufacturer", &info.manufacturer),
        ("product", &info.product),
        ("serial", &info.serial),
    ] {
        println!("  {label:13} {}", s.as_deref().unwrap_or("-"));
    }
    println!("device: {}", hex(&info.device_descriptor));
    for c in &info.configs {
        println!("configuration ({} bytes):", c.len());
        for item in desc::split(c) {
            let Ok((off, d)) = item else {
                println!("  (malformed)");
                break;
            };
            let name = match d[1] {
                dtype::CONFIG => "CONFIG",
                dtype::INTERFACE => "INTERFACE",
                dtype::ENDPOINT => "ENDPOINT",
                dtype::INTERFACE_ASSOCIATION => "IAD",
                dtype::CS_INTERFACE => "CS_INTERFACE",
                dtype::CS_ENDPOINT => "CS_ENDPOINT",
                _ => "?",
            };
            println!("  {off:04x} {name:12} {}", hex(d));
        }
    }
    if let Some(bos) = &info.bos {
        println!("bos: {}", hex(bos));
    }
    Ok(())
}

fn selftest(model: &str, config: Option<PathBuf>, n: usize) -> Result<(), String> {
    let cfg = match config {
        Some(p) => BusConfig::from_toml(
            &std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
        )?,
        None => BusConfig {
            devices: vec![device_config(model)],
            ..Default::default()
        },
    };
    let host = Host::new(HostConfig {
        clock: ClockMode::Virtual,
        seed: cfg.seed,
        ..Default::default()
    });
    let mut failures = 0;
    for d in &cfg.devices {
        let info = host
            .attach(emuvc_devices::build(d)?)
            .map_err(|e| format!("{}: enumeration failed: {e}", d.model))?;
        println!("{} ({:03}:{:03})", info.name, info.bus_number, info.address);
        let view = ConfigView::parse(&info.configs[0]).map_err(|e| e.to_string())?;
        for vc in find_video_control_interfaces(&view) {
            let f = parse_function(&view, vc, info.speed).map_err(|e| e.to_string())?;
            for vs in &f.streaming {
                for fmt in &vs.formats {
                    let pf = match &fmt.kind {
                        FormatKind::Uncompressed { guid, .. } => PixelFormat::from_guid(guid),
                        FormatKind::Mjpeg => Some(PixelFormat::Mjpeg),
                        _ => None,
                    };
                    for fr in &fmt.frames {
                        let label = format!(
                            "  VS{} format {} frame {:2} {:>6} {:4}x{:<4} {:5.1} fps",
                            vs.interface,
                            fmt.index,
                            fr.index,
                            pf.map_or("?".into(), |p| format!("{p:?}")),
                            fr.width,
                            fr.height,
                            1e7 / fr.default_interval.max(1) as f64
                        );
                        let Some(pf) = pf else {
                            println!("{label}: skipped (no frame generator)");
                            continue;
                        };
                        match capture(&host, info.id, vs, fmt.index, fr.index, n) {
                            Ok(c) => {
                                let expect = pf.frame_size(fr.width, fr.height);
                                let mut bad = 0;
                                let mut prev: Option<u32> = None;
                                let mut gaps = 0;
                                for (data, err) in &c.frames {
                                    let s = stamp::decode_frame(
                                        pf,
                                        data,
                                        fr.width as usize,
                                        fr.height as usize,
                                    );
                                    if *err
                                        || s.is_none()
                                        || expect.is_some_and(|e| e != data.len())
                                    {
                                        bad += 1;
                                    }
                                    if let (Some(p), Some(s)) = (prev, s) {
                                        gaps += (s != p + 1) as usize;
                                    }
                                    prev = s.or(prev);
                                }
                                let transfer = match vs.transfer {
                                    Transfer::Isochronous { .. } => format!("alt {:2}", c.alt),
                                    Transfer::Bulk { .. } => "bulk".into(),
                                };
                                let ok = bad == 0 && gaps == 0;
                                failures += !ok as usize;
                                println!(
                                    "{label}: {transfer}, {:5} B/payload, {} frames, {bad} bad, {gaps} gaps{}",
                                    c.commit.max_payload_transfer_size,
                                    c.frames.len(),
                                    if ok { "" } else { "  FAIL" }
                                );
                            }
                            Err(e) => {
                                failures += 1;
                                println!("{label}: FAIL ({e})");
                            }
                        }
                    }
                }
            }
        }
    }
    if failures > 0 {
        Err(format!("{failures} failure(s)"))
    } else {
        Ok(())
    }
}

fn run(
    device: Vec<String>,
    config: Option<PathBuf>,
    clock: Option<String>,
    shim: Option<PathBuf>,
    command: Vec<String>,
) -> Result<(), String> {
    let shim = match shim.or_else(|| std::env::var_os("EMUVC_SHIM").map(PathBuf::from)) {
        Some(s) => s,
        None => std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name("libemuvc_libusb.so"),
    };
    if !shim.exists() {
        return Err(format!(
            "{} not found (cargo build -p emuvc-libusb, or pass --shim)",
            shim.display()
        ));
    }
    let shim = shim.canonicalize().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    let preload = match std::env::var("LD_PRELOAD") {
        Ok(p) if !p.is_empty() => format!("{}:{p}", shim.display()),
        _ => shim.display().to_string(),
    };
    cmd.env("LD_PRELOAD", preload);
    if !device.is_empty() {
        cmd.env("EMUVC_DEVICE", device.join(","));
    }
    if let Some(c) = config {
        cmd.env("EMUVC_CONFIG", c);
    }
    if let Some(c) = clock {
        cmd.env("EMUVC_CLOCK", c);
    }
    Err(format!("{}: {}", command[0], cmd.exec()))
}

fn usbip(
    listen: &str,
    device: Vec<String>,
    config: Option<PathBuf>,
    clock: Option<String>,
) -> Result<(), String> {
    let mut cfg = match config {
        Some(p) => BusConfig::from_toml(
            &std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
        )?,
        None => BusConfig::default(),
    };
    cfg.devices.extend(device.iter().map(|d| device_config(d)));
    if cfg.devices.is_empty() {
        cfg.devices.push(device_config(DEFAULT_MODEL));
    }
    match clock.as_deref() {
        None => {}
        Some("virtual") => cfg.clock = ClockMode::Virtual,
        Some("realtime") => cfg.clock = ClockMode::Realtime,
        Some(c) => return Err(format!("unknown clock {c:?} (realtime or virtual)")),
    }
    let host = std::sync::Arc::new(Host::new(HostConfig {
        clock: cfg.clock,
        seed: cfg.seed,
        ..Default::default()
    }));
    for d in &cfg.devices {
        let info = host
            .attach(emuvc_devices::build(d)?)
            .map_err(|e| format!("{}: enumeration failed: {e}", d.model))?;
        println!(
            "exporting {} as busid {}-{}",
            info.name, info.bus_number, info.port
        );
    }
    let listener = std::net::TcpListener::bind(listen).map_err(|e| format!("{listen}: {e}"))?;
    println!(
        "usbip server listening on {}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    emuvc_usbip::serve(host, listener).map_err(|e| e.to_string())
}

fn check_frames(format: &str, size: &str, file: &std::path::Path) -> Result<(), String> {
    let pf = match format {
        "yuyv" => PixelFormat::Yuyv,
        "uyvy" => PixelFormat::Uyvy,
        "nv12" => PixelFormat::Nv12,
        "grey" | "gray" => PixelFormat::Grey8,
        _ => return Err(format!("unsupported raw format {format:?}")),
    };
    let (w, h) = size
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)))
        .ok_or("bad size")?;
    let frame_len = pf.frame_size(w, h).unwrap();
    let data = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let (mut prev, mut bad, mut gaps) = (None::<u32>, 0, 0);
    for f in data.chunks_exact(frame_len) {
        match stamp::decode_frame(pf, f, w as usize, h as usize) {
            Some(s) => {
                gaps += prev.is_some_and(|p| s != p.wrapping_add(1)) as usize;
                prev = Some(s);
            }
            None => bad += 1,
        }
    }
    let n = data.len() / frame_len;
    println!(
        "RESULT frames={n} bad={bad} gaps={gaps} trailing_bytes={}",
        data.len() % frame_len
    );
    if n == 0 || bad > 0 || data.len() % frame_len != 0 {
        Err("frame check failed".into())
    } else {
        Ok(())
    }
}
