//! Minimal DFU 1.1 runtime-mode interface: answers status queries, accepts
//! (and ignores) DFU_DETACH. Enough for hosts that probe it.

use emuvc_usb::{ControlReply, Ctx, Function, SetupPacket};

const DFU_DETACH: u8 = 0x00;
const DFU_GETSTATUS: u8 = 0x03;
const DFU_GETSTATE: u8 = 0x05;
const APP_IDLE: u8 = 0;

pub struct DfuRuntime;

impl Function for DfuRuntime {
    fn control(&mut self, _cx: &mut Ctx, setup: &SetupPacket, _data: &[u8]) -> ControlReply {
        match (setup.request_type, setup.request) {
            (0x21, DFU_DETACH) => ControlReply::ack(),
            // bStatus OK, bwPollTimeout 0, bState appIDLE, iString 0.
            (0xa1, DFU_GETSTATUS) => ControlReply::data(vec![0, 0, 0, 0, APP_IDLE, 0]),
            (0xa1, DFU_GETSTATE) => ControlReply::data(vec![APP_IDLE]),
            _ => ControlReply::stall(),
        }
    }
}
