//! Verifies that the `#[repr(C)]` mirrors in `emuvc_libusb::ffi` match the installed
//! `libusb.h` (size, alignment and every field offset), by compiling and running a small C
//! program against the system header.

use emuvc_libusb::ffi::*;
use std::collections::BTreeMap;
use std::mem::{align_of, size_of};
use std::process::Command;

struct Entry {
    label: &'static str,
    /// C type expression usable in sizeof/_Alignof/offsetof.
    c_type: &'static str,
    size: usize,
    align: usize,
    fields: Vec<(&'static str, usize)>,
}

macro_rules! entry {
    ($label:literal, $c:literal, $t:ty { $($f:ident),* $(,)? }) => {
        Entry {
            label: $label,
            c_type: $c,
            size: size_of::<$t>(),
            align: align_of::<$t>(),
            fields: vec![$((stringify!($f), core::mem::offset_of!($t, $f))),*],
        }
    };
}

fn c_field(name: &str) -> &str {
    // Rust keywords are spelled with a trailing underscore (`type_`, `str_`).
    name.strip_suffix('_').unwrap_or(name)
}

fn entries() -> Vec<Entry> {
    vec![
        entry!(
            "device_descriptor",
            "struct libusb_device_descriptor",
            libusb_device_descriptor {
                bLength,
                bDescriptorType,
                bcdUSB,
                bDeviceClass,
                bDeviceSubClass,
                bDeviceProtocol,
                bMaxPacketSize0,
                idVendor,
                idProduct,
                bcdDevice,
                iManufacturer,
                iProduct,
                iSerialNumber,
                bNumConfigurations
            }
        ),
        entry!(
            "endpoint_descriptor",
            "struct libusb_endpoint_descriptor",
            libusb_endpoint_descriptor {
                bLength,
                bDescriptorType,
                bEndpointAddress,
                bmAttributes,
                wMaxPacketSize,
                bInterval,
                bRefresh,
                bSynchAddress,
                extra,
                extra_length
            }
        ),
        entry!(
            "interface_association_descriptor",
            "struct libusb_interface_association_descriptor",
            libusb_interface_association_descriptor {
                bLength,
                bDescriptorType,
                bFirstInterface,
                bInterfaceCount,
                bFunctionClass,
                bFunctionSubClass,
                bFunctionProtocol,
                iFunction
            }
        ),
        entry!(
            "interface_association_descriptor_array",
            "struct libusb_interface_association_descriptor_array",
            libusb_interface_association_descriptor_array { iad, length }
        ),
        entry!(
            "interface_descriptor",
            "struct libusb_interface_descriptor",
            libusb_interface_descriptor {
                bLength,
                bDescriptorType,
                bInterfaceNumber,
                bAlternateSetting,
                bNumEndpoints,
                bInterfaceClass,
                bInterfaceSubClass,
                bInterfaceProtocol,
                iInterface,
                endpoint,
                extra,
                extra_length
            }
        ),
        entry!(
            "interface",
            "struct libusb_interface",
            libusb_interface {
                altsetting,
                num_altsetting
            }
        ),
        entry!(
            "config_descriptor",
            "struct libusb_config_descriptor",
            libusb_config_descriptor {
                bLength,
                bDescriptorType,
                wTotalLength,
                bNumInterfaces,
                bConfigurationValue,
                iConfiguration,
                bmAttributes,
                MaxPower,
                interface,
                extra,
                extra_length
            }
        ),
        entry!(
            "ss_endpoint_companion_descriptor",
            "struct libusb_ss_endpoint_companion_descriptor",
            libusb_ss_endpoint_companion_descriptor {
                bLength,
                bDescriptorType,
                bMaxBurst,
                bmAttributes,
                wBytesPerInterval
            }
        ),
        entry!(
            "bos_dev_capability_descriptor",
            "struct libusb_bos_dev_capability_descriptor",
            libusb_bos_dev_capability_descriptor {
                bLength,
                bDescriptorType,
                bDevCapabilityType,
                dev_capability_data
            }
        ),
        entry!(
            "bos_descriptor",
            "struct libusb_bos_descriptor",
            libusb_bos_descriptor {
                bLength,
                bDescriptorType,
                wTotalLength,
                bNumDeviceCaps,
                dev_capability
            }
        ),
        entry!(
            "usb_2_0_extension_descriptor",
            "struct libusb_usb_2_0_extension_descriptor",
            libusb_usb_2_0_extension_descriptor {
                bLength,
                bDescriptorType,
                bDevCapabilityType,
                bmAttributes
            }
        ),
        entry!(
            "ss_usb_device_capability_descriptor",
            "struct libusb_ss_usb_device_capability_descriptor",
            libusb_ss_usb_device_capability_descriptor {
                bLength,
                bDescriptorType,
                bDevCapabilityType,
                bmAttributes,
                wSpeedSupported,
                bFunctionalitySupport,
                bU1DevExitLat,
                bU2DevExitLat
            }
        ),
        entry!(
            "ssplus_sublink_attribute",
            "struct libusb_ssplus_sublink_attribute",
            libusb_ssplus_sublink_attribute {
                ssid,
                exponent,
                type_,
                direction,
                protocol,
                mantissa
            }
        ),
        entry!(
            "ssplus_usb_device_capability_descriptor",
            "struct libusb_ssplus_usb_device_capability_descriptor",
            libusb_ssplus_usb_device_capability_descriptor {
                numSublinkSpeedAttributes,
                numSublinkSpeedIDs,
                ssid,
                minRxLaneCount,
                minTxLaneCount,
                sublinkSpeedAttributes
            }
        ),
        entry!(
            "container_id_descriptor",
            "struct libusb_container_id_descriptor",
            libusb_container_id_descriptor {
                bLength,
                bDescriptorType,
                bDevCapabilityType,
                bReserved,
                ContainerID
            }
        ),
        entry!(
            "platform_descriptor",
            "struct libusb_platform_descriptor",
            libusb_platform_descriptor {
                bLength,
                bDescriptorType,
                bDevCapabilityType,
                bReserved,
                PlatformCapabilityUUID,
                CapabilityData
            }
        ),
        entry!(
            "control_setup",
            "struct libusb_control_setup",
            libusb_control_setup {
                bmRequestType,
                bRequest,
                wValue,
                wIndex,
                wLength
            }
        ),
        entry!(
            "version",
            "struct libusb_version",
            libusb_version {
                major,
                minor,
                micro,
                nano,
                rc,
                describe
            }
        ),
        entry!(
            "iso_packet_descriptor",
            "struct libusb_iso_packet_descriptor",
            libusb_iso_packet_descriptor {
                length,
                actual_length,
                status
            }
        ),
        entry!(
            "transfer",
            "struct libusb_transfer",
            libusb_transfer {
                dev_handle,
                flags,
                endpoint,
                type_,
                timeout,
                status,
                length,
                actual_length,
                callback,
                user_data,
                buffer,
                num_iso_packets,
                iso_packet_desc
            }
        ),
        entry!(
            "init_option",
            "struct libusb_init_option",
            libusb_init_option { option, value }
        ),
        entry!(
            "init_option.value",
            "__typeof__(((struct libusb_init_option *)0)->value)",
            libusb_init_option_value { ival, log_cbval }
        ),
        entry!(
            "pollfd",
            "struct libusb_pollfd",
            libusb_pollfd { fd, events }
        ),
        // scalar typedefs / enums / callbacks
        Entry {
            label: "enum libusb_transfer_status",
            c_type: "enum libusb_transfer_status",
            size: size_of::<c_int_t>(),
            align: align_of::<c_int_t>(),
            fields: vec![],
        },
        Entry {
            label: "enum libusb_error",
            c_type: "enum libusb_error",
            size: size_of::<c_int_t>(),
            align: align_of::<c_int_t>(),
            fields: vec![],
        },
        Entry {
            label: "enum libusb_option",
            c_type: "enum libusb_option",
            size: size_of::<c_int_t>(),
            align: align_of::<c_int_t>(),
            fields: vec![],
        },
        Entry {
            label: "enum libusb_log_level",
            c_type: "enum libusb_log_level",
            size: size_of::<c_int_t>(),
            align: align_of::<c_int_t>(),
            fields: vec![],
        },
        Entry {
            label: "libusb_transfer_cb_fn",
            c_type: "libusb_transfer_cb_fn",
            size: size_of::<libusb_transfer_cb_fn>(),
            align: align_of::<libusb_transfer_cb_fn>(),
            fields: vec![],
        },
        Entry {
            label: "libusb_log_cb",
            c_type: "libusb_log_cb",
            size: size_of::<libusb_log_cb>(),
            align: align_of::<libusb_log_cb>(),
            fields: vec![],
        },
        Entry {
            label: "libusb_pollfd_added_cb",
            c_type: "libusb_pollfd_added_cb",
            size: size_of::<libusb_pollfd_added_cb>(),
            align: align_of::<libusb_pollfd_added_cb>(),
            fields: vec![],
        },
        Entry {
            label: "libusb_pollfd_removed_cb",
            c_type: "libusb_pollfd_removed_cb",
            size: size_of::<libusb_pollfd_removed_cb>(),
            align: align_of::<libusb_pollfd_removed_cb>(),
            fields: vec![],
        },
        Entry {
            label: "libusb_hotplug_callback_fn",
            c_type: "libusb_hotplug_callback_fn",
            size: size_of::<libusb_hotplug_callback_fn>(),
            align: align_of::<libusb_hotplug_callback_fn>(),
            fields: vec![],
        },
        Entry {
            label: "libusb_hotplug_callback_handle",
            c_type: "libusb_hotplug_callback_handle",
            size: size_of::<libusb_hotplug_callback_handle>(),
            align: align_of::<libusb_hotplug_callback_handle>(),
            fields: vec![],
        },
        Entry {
            label: "libusb_hotplug_event",
            c_type: "libusb_hotplug_event",
            size: size_of::<libusb_hotplug_event>(),
            align: align_of::<libusb_hotplug_event>(),
            fields: vec![],
        },
    ]
}

#[allow(non_camel_case_types)]
type c_int_t = core::ffi::c_int;

fn tool_ok(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn layout_matches_libusb_h() {
    if !tool_ok("cc", &["--version"]) || !tool_ok("pkg-config", &["--version"]) {
        eprintln!("SKIPPED: cc or pkg-config not available");
        return;
    }
    let cflags = Command::new("pkg-config")
        .args(["--cflags", "libusb-1.0"])
        .output();
    let cflags = match cflags {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => {
            eprintln!("SKIPPED: pkg-config cannot find libusb-1.0");
            return;
        }
    };

    let entries = entries();
    let mut c = String::from(
        "#include <stdio.h>\n#include <stddef.h>\n#include <libusb.h>\nint main(void) {\n",
    );
    for e in &entries {
        c += &format!(
            "  printf(\"%s size %zu\\n%s align %zu\\n\", \"{l}\", sizeof({t}), \"{l}\", _Alignof({t}));\n",
            l = e.label,
            t = e.c_type
        );
        for (f, _) in &e.fields {
            c += &format!(
                "  printf(\"%s.%s offset %zu\\n\", \"{l}\", \"{f}\", offsetof({t}, {cf}));\n",
                l = e.label,
                t = e.c_type,
                f = f,
                cf = c_field(f)
            );
        }
    }
    c += "  return 0;\n}\n";

    let dir = std::env::temp_dir().join(format!("emuvc-libusb-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("layout.c");
    let exe = dir.join("layout");
    std::fs::write(&src, &c).unwrap();

    let mut cmd = Command::new("cc");
    cmd.args(cflags.split_whitespace())
        .arg("-std=gnu11")
        .arg("-o")
        .arg(&exe)
        .arg(&src);
    let out = cmd.output().expect("run cc");
    assert!(
        out.status.success(),
        "cc failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = Command::new(&exe).output().expect("run layout program");
    assert!(out.status.success());
    let _ = std::fs::remove_dir_all(&dir);

    let mut c_vals: BTreeMap<String, usize> = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.rsplitn(2, ' ');
        let v: usize = it.next().unwrap().parse().unwrap();
        c_vals.insert(it.next().unwrap().to_string(), v);
    }

    let mut rust_vals: BTreeMap<String, usize> = BTreeMap::new();
    for e in &entries {
        rust_vals.insert(format!("{} size", e.label), e.size);
        rust_vals.insert(format!("{} align", e.label), e.align);
        for (f, off) in &e.fields {
            rust_vals.insert(format!("{}.{} offset", e.label, f), *off);
        }
    }
    assert_eq!(c_vals.len(), rust_vals.len());
    let mut bad = vec![];
    for (k, v) in &rust_vals {
        // Rust field names carry a trailing '_' for keywords; C names do not.
        let ck = k.replace("type_ ", "type ").replace("str_ ", "str ");
        let cv = c_vals.get(k).or_else(|| c_vals.get(&ck));
        if cv != Some(v) {
            bad.push(format!("{k}: rust={v} c={cv:?}"));
        }
    }
    assert!(bad.is_empty(), "layout mismatches:\n{}", bad.join("\n"));
}
