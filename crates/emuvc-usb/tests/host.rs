//! Host engine tests against a small synthetic device.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use emuvc_usb::desc::{ConfigBuilder, DeviceDescriptor, EndpointDescriptor, InterfaceDescriptor};
use emuvc_usb::fault::{Action, FaultInjector, FaultRule, Match};
use emuvc_usb::*;

/// Interface 0: alt 0 = bulk IN 0x82 + interrupt IN 0x83; alt 1 adds iso IN 0x81.
struct TestFn {
    pending_irq: bool,
    bulk_count: u32,
}

impl Function for TestFn {
    fn control(&mut self, _cx: &mut Ctx, setup: &SetupPacket, data: &[u8]) -> ControlReply {
        match (setup.request_type, setup.request) {
            (0xc1, 0x01) => ControlReply::data(vec![0x42, 0x43]),
            (0x41, 0x02) => {
                self.pending_irq = data == [1];
                ControlReply::ack()
            }
            (0xc1, 0x03) => ControlReply::data(vec![1]).with_latency_us(50_000),
            _ => ControlReply::stall(),
        }
    }

    fn data_in(&mut self, _cx: &mut Ctx, ep: u8, _max: usize) -> InReply {
        match ep {
            0x82 => {
                self.bulk_count += 1;
                InReply::Data(self.bulk_count.to_le_bytes().to_vec())
            }
            0x83 if self.pending_irq => {
                self.pending_irq = false;
                InReply::Data(vec![0xaa])
            }
            0x83 => InReply::Nak,
            _ => InReply::Error(UsbError::Stall),
        }
    }

    fn iso_in(&mut self, cx: &mut Ctx, _ep: u8, max_len: usize) -> IsoReply {
        let mut v = cx.now().to_le_bytes().to_vec();
        v.truncate(max_len);
        IsoReply::data(v)
    }
}

fn test_device() -> CompositeDevice {
    let device = DeviceDescriptor {
        bcd_usb: 0x0200,
        class: 0xff,
        subclass: 0,
        protocol: 0,
        max_packet_size0: 64,
        vendor_id: 0x1234,
        product_id: 0x5678,
        bcd_device: 0x0100,
        i_manufacturer: 1,
        i_product: 2,
        i_serial: 0,
        num_configurations: 1,
    };
    let mut cfg = ConfigBuilder::new(1);
    let iface = |alt, n| InterfaceDescriptor {
        number: 0,
        alt,
        num_endpoints: n,
        class: 0xff,
        subclass: 0,
        protocol: 0,
        i_interface: 0,
    };
    let bulk = EndpointDescriptor {
        address: 0x82,
        attributes: 2,
        max_packet_size: 512,
        interval: 0,
    };
    let irq = EndpointDescriptor {
        address: 0x83,
        attributes: 3,
        max_packet_size: 8,
        interval: 4,
    };
    cfg.interface(&iface(0, 2)).endpoint(&bulk).endpoint(&irq);
    cfg.interface(&iface(1, 3)).endpoint(&bulk).endpoint(&irq);
    cfg.endpoint(&EndpointDescriptor {
        address: 0x81,
        attributes: 5,
        max_packet_size: 1024,
        interval: 1,
    });
    let mut desc = Descriptors {
        device: device.to_bytes(),
        configs: vec![cfg.build()],
        ..Default::default()
    };
    desc.strings.insert(1, "emuvc".into());
    desc.strings.insert(2, "Test device".into());
    let mut dev = CompositeDevice::new("test", Speed::High, desc);
    dev.add_function(
        &[0],
        Box::new(TestFn {
            pending_irq: false,
            bulk_count: 0,
        }),
    );
    dev
}

fn virtual_host() -> Host {
    Host::new(HostConfig {
        clock: ClockMode::Virtual,
        ..Default::default()
    })
}

fn set_alt(host: &Host, dev: DeviceId, alt: u16) {
    host.control(dev, SetupPacket::new(0x01, 0x0b, alt, 0, 0), &[], None)
        .unwrap();
}

#[test]
fn enumeration_and_control() {
    let host = virtual_host();
    let info = host.attach(Box::new(test_device())).unwrap();
    assert_eq!(info.descriptor().unwrap().vendor_id, 0x1234);
    assert_eq!(info.product.as_deref(), Some("Test device"));
    assert_eq!(info.manufacturer.as_deref(), Some("emuvc"));
    assert_eq!(info.active_config(), 1);
    assert!(info.endpoint(0x82).is_some());
    assert!(
        info.endpoint(0x81).is_none(),
        "iso endpoint only exists in alt 1"
    );

    let r = host.control(info.id, SetupPacket::new(0xc1, 0x01, 0, 0, 1), &[], None);
    assert_eq!(r, Ok(vec![0x42]), "IN data is truncated to wLength");
    let r = host.control(info.id, SetupPacket::new(0xc1, 0x7f, 0, 0, 1), &[], None);
    assert_eq!(r, Err(UsbError::Stall));
    // Interface number must exist.
    let r = host.control(info.id, SetupPacket::new(0x01, 0x0b, 0, 5, 0), &[], None);
    assert_eq!(r, Err(UsbError::Stall));
}

#[test]
fn iso_slots_are_contiguous() {
    let host = virtual_host();
    let info = host.attach(Box::new(test_device())).unwrap();
    assert_eq!(
        host.submit(Urb::iso_in(info.id, 0x81, vec![1024; 8]), Box::new(|_| ()))
            .err(),
        Some(UsbError::Invalid)
    );
    set_alt(&host, info.id, 1);
    let (tx, rx) = mpsc::channel();
    for _ in 0..3 {
        let tx = tx.clone();
        host.submit(
            Urb::iso_in(info.id, 0x81, vec![1024; 8]),
            Box::new(move |c| tx.send(c).unwrap()),
        )
        .unwrap();
    }
    let mut slots = Vec::new();
    for _ in 0..3 {
        let c = rx.recv().unwrap();
        assert_eq!(c.status, Ok(()));
        assert_eq!(c.iso.len(), 8);
        for p in &c.iso {
            assert_eq!(p.status, Ok(()));
            slots.push(u64::from_le_bytes(p.data[..8].try_into().unwrap()));
        }
    }
    assert!(slots.windows(2).all(|w| w[1] == w[0] + 1), "{slots:?}");
}

#[test]
fn interrupt_naks_until_event() {
    let host = virtual_host();
    let info = host.attach(Box::new(test_device())).unwrap();
    let (tx, rx) = mpsc::channel();
    host.submit(
        Urb::interrupt_in(info.id, 0x83, 8),
        Box::new(move |c| tx.send(c).unwrap()),
    )
    .unwrap();
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "should NAK"
    );
    host.control(info.id, SetupPacket::new(0x41, 0x02, 0, 0, 1), &[1], None)
        .unwrap();
    let c = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("interrupt completes after the event");
    assert_eq!(c.data, vec![0xaa]);
}

#[test]
fn cancel_and_detach() {
    let host = virtual_host();
    let info = host.attach(Box::new(test_device())).unwrap();
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    let id = host
        .submit(
            Urb::interrupt_in(info.id, 0x83, 8),
            Box::new(move |c| tx.send(c).unwrap()),
        )
        .unwrap();
    host.submit(
        Urb::interrupt_in(info.id, 0x83, 8),
        Box::new(move |c| tx2.send(c).unwrap()),
    )
    .unwrap();
    host.cancel(id);
    let c = rx.recv().unwrap();
    assert_eq!((c.id, c.status), (id, Err(UsbError::Cancelled)));
    host.detach(info.id);
    assert_eq!(rx.recv().unwrap().status, Err(UsbError::NoDevice));
    assert!(host.devices().is_empty());
    assert_eq!(
        host.control(info.id, SetupPacket::new(0xc1, 0x01, 0, 0, 1), &[], None),
        Err(UsbError::NoDevice)
    );
}

#[test]
fn set_interface_flushes_iso() {
    let host = virtual_host();
    let info = host.attach(Box::new(test_device())).unwrap();
    set_alt(&host, info.id, 1);
    // Keep the engine busy far into the future, then switch back to alt 0.
    let (tx, rx) = mpsc::channel();
    host.submit(
        Urb::iso_in(info.id, 0x81, vec![1024; 8]).with_timeout(None),
        Box::new(move |c| tx.send(c).unwrap()),
    )
    .unwrap();
    set_alt(&host, info.id, 0);
    let c = rx.recv().unwrap();
    // Either it completed before the SET_INTERFACE was processed, or it was shut down.
    assert!(
        c.status == Ok(()) || c.status == Err(UsbError::Shutdown),
        "{:?}",
        c.status
    );
    assert!(info.endpoint(0x81).is_none());
}

#[test]
fn control_latency_and_timeout() {
    let host = virtual_host();
    let rules = vec![FaultRule {
        on: Match::Control {
            request_type: Some(0xc1),
            request: Some(0x01),
            value: None,
            index: None,
            selector: None,
            entity: None,
            interface: None,
        },
        action: Action::Timeout,
        skip: 0,
        count: Some(1),
        every: None,
        probability: None,
        after_ms: None,
        until_ms: None,
    }];
    let info = host
        .attach(Box::new(FaultInjector::new(test_device(), rules)))
        .unwrap();
    let t0 = host.now();
    let setup = SetupPacket::new(0xc1, 0x01, 0, 0, 2);
    assert_eq!(
        host.control(info.id, setup, &[], Some(Duration::from_millis(500))),
        Err(UsbError::Timeout)
    );
    assert!(
        host.now() - t0 >= 4000,
        "virtual time advanced by the timeout"
    );
    // count = 1: the next one goes through.
    assert_eq!(
        host.control(info.id, setup, &[], Some(Duration::from_millis(500))),
        Ok(vec![0x42, 0x43])
    );
    let t1 = host.now();
    assert_eq!(
        host.control(info.id, SetupPacket::new(0xc1, 0x03, 0, 0, 1), &[], None),
        Ok(vec![1])
    );
    assert!(host.now() - t1 >= 400, "50 ms latency");
}

#[test]
fn realtime_iso_pacing() {
    let host = Host::new(HostConfig::default());
    let info = host.attach(Box::new(test_device())).unwrap();
    set_alt(&host, info.id, 1);
    let start = Instant::now();
    // 160 microframes = 20 ms of bus time.
    let c = host.transfer(Urb::iso_in(info.id, 0x81, vec![1024; 160]));
    let el = start.elapsed();
    assert_eq!(c.iso.len(), 160);
    assert!(
        el >= Duration::from_millis(18) && el < Duration::from_millis(200),
        "{el:?}"
    );
}

#[test]
fn bulk_in() {
    let host = virtual_host();
    let info = host.attach(Box::new(test_device())).unwrap();
    for i in 1..=3u32 {
        let c = host.transfer(Urb::bulk_in(info.id, 0x82, 512));
        assert_eq!(c.data, i.to_le_bytes());
    }
    let mut urb = Urb::bulk_in(info.id, 0x82, 512);
    urb.short_not_ok = true;
    assert_eq!(host.transfer(urb).status, Err(UsbError::ShortPacket));
}
