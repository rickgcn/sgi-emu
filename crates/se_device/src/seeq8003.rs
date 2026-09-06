//! SEEQ 8003 register, frame, and virtual link behavior.

use se_core::bus::{BusError, DeviceAddr};
use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
use serde::{Deserialize, Serialize};

const REGISTER_BYTES: u64 = 4;
const REGISTER_COUNT: u64 = 8;
const RECEIVE_COMMAND_SLOT: usize = 6;
const TRANSMIT_COMMAND_SLOT: usize = 7;
const BANK_SELECT_MASK: u8 = 0x60;
const OLD_DEVICE_STATUS: u8 = 0x80;

/// Maximum frame allocation accepted by the device, excluding the wire FCS.
///
/// This is a resource bound, not an Ethernet MTU. Accepted frames larger than
/// the guest's receive buffer can still cause a DMA receive buffer overflow.
const MAX_FRAME_BYTES: usize = 16_384;
const BYTE_TIME: u128 = ATTOSECONDS_PER_SECOND * 8 / 10_000_000;
const INTERFRAME_GAP: u128 = BYTE_TIME * 12;

/// A frame and its controller status presented to the attached DMA engine.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReceivedFrame {
    /// Ethernet bytes beginning at the destination address, without the FCS.
    pub bytes: Vec<u8>,
    /// SEEQ receive status before its OLD acknowledgement.
    pub status: u8,
    /// Whether the status enabled a receive interrupt.
    pub interrupt: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct WireFrame {
    bytes: Vec<u8>,
    remaining: u128,
    errors: u8,
}

/// The software-visible SEEQ 8003 state used by the IP12 machine.
///
/// SGI's shared driver writes 80C03 bank-select values even on IP12. The
/// diagnostic bank latches are retained for that access pattern, without
/// enabling hash filtering or programmable packet gaps on the original 8003.
/// MAC and bank latches clear on reset as an explicit compatibility convention;
/// IRIX reloads the station address before enabling reception.
#[derive(Clone, Deserialize, Serialize)]
pub struct Seeq8003 {
    station_address: [u8; 6],
    multicast_low: [u8; 6],
    multicast_high: [u8; 2],
    inter_packet_gap: u8,
    control: u8,
    receive_command: u8,
    transmit_command: u8,
    receive_status: u8,
    transmit_status: u8,
    receive_interrupt: bool,
    transmit_interrupt: bool,
    receiving: Option<WireFrame>,
    transmitting: Option<WireFrame>,
    received: Option<ReceivedFrame>,
    transmitted: Option<Vec<u8>>,
    receive_gap: u128,
    transmit_gap: u128,
}

impl Seeq8003 {
    /// Creates a SEEQ 8003 in its reset state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            station_address: [0; 6],
            multicast_low: [0; 6],
            multicast_high: [0; 2],
            inter_packet_gap: 0,
            control: 0,
            receive_command: 0,
            transmit_command: 0,
            receive_status: OLD_DEVICE_STATUS,
            transmit_status: OLD_DEVICE_STATUS,
            receive_interrupt: false,
            transmit_interrupt: false,
            receiving: None,
            transmitting: None,
            received: None,
            transmitted: None,
            receive_gap: 0,
            transmit_gap: 0,
        }
    }

    /// Restores the mutable SEEQ 8003 reset state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError`] when the complete transaction does not fit one
    /// external register or the width is unsupported.
    pub fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        self.debug_read(address, data)?;
        let (slot, offset) = register_transaction(address, data.len())?;
        if offset + data.len() == 4 {
            match slot {
                RECEIVE_COMMAND_SLOT => {
                    self.acknowledge_receive();
                }
                TRANSMIT_COMMAND_SLOT => {
                    self.acknowledge_transmit();
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Reads registers without acknowledging controller status or interrupts.
    ///
    /// Undefined address reads retain the nonzero old-device probe convention
    /// used by the IP12 PROM front end. They are not MAC readback registers.
    /// TX status resets to OLD as specified by MD400024/C section 1-6 and used
    /// by the operating system's reset probe.
    ///
    /// # Errors
    /// Returns a bus error for an unsupported or uncontained transaction.
    pub fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let (slot, offset) = register_transaction(address, data.len())?;
        let value = match slot {
            0 => OLD_DEVICE_STATUS,
            1..=5 => 0,
            RECEIVE_COMMAND_SLOT => self.receive_status,
            TRANSMIT_COMMAND_SLOT => self.transmit_status,
            _ => unreachable!("validated SEEQ register slot"),
        };
        let bytes = u32::from(value).to_be_bytes();
        data.copy_from_slice(&bytes[offset..offset + data.len()]);
        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError`] when the complete transaction does not fit one
    /// external register or the width is unsupported.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let (slot, offset) = register_transaction(address, data.len())?;
        let low_lane = 3;
        if offset > low_lane || offset + data.len() <= low_lane {
            return Ok(());
        }
        let value = data[low_lane - offset];

        match slot {
            0..=5 => self.write_banked_register(slot, value),
            RECEIVE_COMMAND_SLOT => self.receive_command = value,
            TRANSMIT_COMMAND_SLOT => self.transmit_command = value,
            _ => unreachable!("validated SEEQ register slot"),
        }
        Ok(())
    }

    fn write_banked_register(&mut self, slot: usize, value: u8) {
        match self.transmit_command & BANK_SELECT_MASK {
            0x00 => self.station_address[slot] = value,
            0x20 => self.multicast_low[slot] = value,
            0x40 => match slot {
                0..=1 => self.multicast_high[slot] = value,
                2 => self.inter_packet_gap = value,
                3 => self.control = value,
                4..=5 => {}
                _ => unreachable!("validated banked SEEQ register slot"),
            },
            _ => {}
        }
    }

    /// Reports whether a frame can enter the virtual link now.
    ///
    /// Link spacing is independent of address matching, receive enable, and
    /// DMA availability. Rejected frames occupy the same wire time.
    #[must_use]
    pub fn receive_ready(&self) -> bool {
        self.receiving.is_none() && self.receive_gap == 0
    }

    /// Starts reception of a frame without preamble or FCS.
    ///
    /// `errors` supplies physical CRC/dribble/overflow indications in bits 0–2.
    /// A host network frame has no physical error indication and passes zero.
    /// Returns false only when the link is occupied or the allocation bound is
    /// exceeded; disabled or filtered reception still consumes the wire slot.
    pub fn receive(&mut self, bytes: &[u8], errors: u8) -> bool {
        if !self.receive_ready() || bytes.len() > MAX_FRAME_BYTES {
            return false;
        }
        self.receiving = Some(WireFrame {
            bytes: bytes.to_vec(),
            remaining: wire_time(bytes.len()),
            errors: errors & 7,
        });
        true
    }

    /// Reports whether the transmitter can accept another packet.
    #[must_use]
    pub fn transmit_ready(&self) -> bool {
        self.transmitting.is_none() && self.transmitted.is_none() && self.transmit_gap == 0
    }

    /// Starts transmission; completion depends only on virtual wire time.
    ///
    /// IRIX pads its TX descriptors to 60 bytes. The original 8003 model does
    /// not enable the 80C03 automatic-padding extension. The wire FCS consumes
    /// time but is excluded from the host frame and DMA byte count.
    pub fn transmit(&mut self, bytes: Vec<u8>) -> bool {
        if !self.transmit_ready() || bytes.len() > MAX_FRAME_BYTES {
            return false;
        }
        self.transmitting = Some(WireFrame {
            remaining: wire_time(bytes.len()),
            bytes,
            errors: 0,
        });
        true
    }

    /// Advances frame transmission, reception, and interframe gaps.
    pub fn advance_time(&mut self, elapsed: VirtualDuration) {
        let elapsed = elapsed.as_attoseconds();
        self.receive_gap = self.receive_gap.saturating_sub(elapsed);
        self.transmit_gap = self.transmit_gap.saturating_sub(elapsed);
        if let Some(frame) = advance_wire(&mut self.receiving, elapsed) {
            self.receive_gap =
                INTERFRAME_GAP.saturating_sub(elapsed.saturating_sub(frame.remaining));
            self.finish_receive(frame);
        }
        if let Some(frame) = advance_wire(&mut self.transmitting, elapsed) {
            self.transmit_gap =
                INTERFRAME_GAP.saturating_sub(elapsed.saturating_sub(frame.remaining));
            self.transmit_status = 0x08;
            self.transmit_interrupt = self.transmit_command & 0x08 != 0;
            self.transmitted = Some(frame.bytes);
        }
    }

    /// Returns the next software-visible link transition.
    #[must_use]
    pub fn time_until_event(&self) -> Option<VirtualDuration> {
        [
            self.receiving.as_ref().map(|f| f.remaining),
            self.transmitting.as_ref().map(|f| f.remaining),
            (self.receive_gap != 0).then_some(self.receive_gap),
            (self.transmit_gap != 0).then_some(self.transmit_gap),
        ]
        .into_iter()
        .flatten()
        .min()
        .map(VirtualDuration::from_attoseconds)
    }

    /// Takes a completed receive transfer without acknowledging its status.
    pub fn take_received_frame(&mut self) -> Option<ReceivedFrame> {
        self.received.take()
    }

    /// Takes a transmitted host frame without acknowledging TX status.
    pub fn take_transmitted_frame(&mut self) -> Option<Vec<u8>> {
        self.transmitted.take()
    }

    /// Reads and acknowledges the receive status through the DMA-side bus.
    pub fn acknowledge_receive(&mut self) -> u8 {
        let status = self.receive_status;
        self.receive_status |= OLD_DEVICE_STATUS;
        self.receive_interrupt = false;
        status
    }

    /// Reads and acknowledges the transmit status through the DMA-side bus.
    pub fn acknowledge_transmit(&mut self) -> u8 {
        let status = self.transmit_status;
        self.transmit_status |= OLD_DEVICE_STATUS;
        self.transmit_interrupt = false;
        status
    }

    /// Reports the independent receive interrupt reason.
    #[must_use]
    pub const fn receive_interrupt_asserted(&self) -> bool {
        self.receive_interrupt
    }

    /// Reports the independent transmit interrupt reason.
    #[must_use]
    pub const fn transmit_interrupt_asserted(&self) -> bool {
        self.transmit_interrupt
    }

    /// Captures an HPC transmit underflow as a controller error.
    pub fn transmit_underflow(&mut self) {
        self.transmitting = None;
        self.transmit_status = 1;
        self.transmit_interrupt = self.transmit_command & 1 != 0;
    }

    /// MD400024/C defines GOOD by the absence of CRC, short-frame and overflow
    /// errors; dribble alone neither clears GOOD nor asserts RxDC. Its receive
    /// command EOF condition excludes overflow. Keep that interrupt condition
    /// separate from the status-bit representation, whose overflow combination
    /// is not specified by the command description.
    fn finish_receive(&mut self, frame: WireFrame) {
        if !self.matches_address(&frame.bytes) || self.receive_status & OLD_DEVICE_STATUS == 0 {
            return;
        }
        let errors = frame.errors | if frame.bytes.len() < 60 { 8 } else { 0 };
        let status = 0x10 | errors | if errors & 0x0b == 0 { 0x20 } else { 0 };
        let interrupt_conditions = if errors & 1 != 0 {
            status & !0x10
        } else {
            status
        };
        let interrupt = interrupt_conditions & self.receive_command & 0x3f != 0;
        self.receive_status = status | if interrupt { 0 } else { OLD_DEVICE_STATUS };
        self.receive_interrupt = interrupt;
        if errors & 0x0b & !self.receive_command != 0 || self.received.is_some() {
            return;
        }
        self.received = Some(ReceivedFrame {
            bytes: frame.bytes,
            status,
            interrupt,
        });
    }

    fn matches_address(&self, bytes: &[u8]) -> bool {
        let Some(destination) = bytes.get(..6) else {
            return false;
        };
        match self.receive_command >> 6 {
            0 => false,
            1 => true,
            2 => destination == self.station_address || destination == [0xff; 6],
            _ => destination == self.station_address || destination[0] & 1 != 0,
        }
    }
}

fn wire_time(length: usize) -> u128 {
    (length as u128 + 8 + 4) * BYTE_TIME
}

fn advance_wire(wire: &mut Option<WireFrame>, elapsed: u128) -> Option<WireFrame> {
    let frame = wire.as_mut()?;
    if frame.remaining > elapsed {
        frame.remaining -= elapsed;
        None
    } else {
        wire.take()
    }
}

fn register_transaction(address: DeviceAddr, length: usize) -> Result<(usize, usize), BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusError::InvalidTransaction)?;
    let end = start
        .checked_add(length)
        .ok_or(BusError::InvalidTransaction)?;
    let register_end = REGISTER_COUNT * REGISTER_BYTES;
    if start >= register_end || end > register_end {
        return Err(BusError::HardwareFault);
    }
    if start / REGISTER_BYTES != (end - 1) / REGISTER_BYTES {
        return Err(BusError::UnimplementedAccess);
    }

    let slot =
        usize::try_from(start / REGISTER_BYTES).map_err(|_| BusError::UnimplementedAccess)?;
    let offset =
        usize::try_from(start % REGISTER_BYTES).map_err(|_| BusError::UnimplementedAccess)?;
    Ok((slot, offset))
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};

    use super::{OLD_DEVICE_STATUS, Seeq8003};

    fn write_word(seeq: &mut Seeq8003, slot: u64, value: u8) {
        seeq.write(DeviceAddr::new(slot * 4), &u32::from(value).to_be_bytes())
            .unwrap();
    }

    fn read_word(seeq: &Seeq8003, slot: u64) -> Result<u32, BusError> {
        let mut data = [0; 4];
        seeq.debug_read(DeviceAddr::new(slot * 4), &mut data)?;
        Ok(u32::from_be_bytes(data))
    }

    #[test]
    fn reset_selects_station_address_bank_and_clears_writable_state() {
        let mut seeq = Seeq8003::new();
        for (slot, value) in [0x08, 0x00, 0x69, 0x12, 0x34, 0x56].into_iter().enumerate() {
            write_word(&mut seeq, slot as u64, value);
        }
        write_word(&mut seeq, 6, 0x9f);
        write_word(&mut seeq, 7, 0x4f);

        seeq.reset();

        assert_eq!(seeq.station_address, [0; 6]);
        assert_eq!(seeq.multicast_low, [0; 6]);
        assert_eq!(seeq.multicast_high, [0; 2]);
        assert_eq!(seeq.inter_packet_gap, 0);
        assert_eq!(seeq.control, 0);
        assert_eq!(seeq.receive_command, 0);
        assert_eq!(seeq.transmit_command, 0);
    }

    #[test]
    fn transmit_command_selects_three_independent_write_banks() {
        let mut seeq = Seeq8003::new();

        for slot in 0..6 {
            write_word(&mut seeq, slot, 0x10 + slot as u8);
        }
        write_word(&mut seeq, 7, 0x20);
        for slot in 0..6 {
            write_word(&mut seeq, slot, 0x20 + slot as u8);
        }
        write_word(&mut seeq, 7, 0x40);
        for (slot, value) in [0x31, 0x32, 0x33, 0x34, 0x35, 0x36].into_iter().enumerate() {
            write_word(&mut seeq, slot as u64, value);
        }

        assert_eq!(seeq.station_address, [0x10, 0x11, 0x12, 0x13, 0x14, 0x15]);
        assert_eq!(seeq.multicast_low, [0x20, 0x21, 0x22, 0x23, 0x24, 0x25]);
        assert_eq!(seeq.multicast_high, [0x31, 0x32]);
        assert_eq!(seeq.inter_packet_gap, 0x33);
        assert_eq!(seeq.control, 0x34);
    }

    #[test]
    fn command_aliases_share_the_external_low_byte_lanes() {
        let mut seeq = Seeq8003::new();

        seeq.write(DeviceAddr::new(0x1b), &[0xa5]).unwrap();
        seeq.write(DeviceAddr::new(0x1f), &[0x24]).unwrap();

        assert_eq!(seeq.receive_command, 0xa5);
        assert_eq!(seeq.transmit_command, 0x24);
    }

    #[test]
    fn old_seeq_probe_and_status_use_the_low_byte_lane() {
        let mut seeq = Seeq8003::new();

        assert_eq!(read_word(&seeq, 0), Ok(u32::from(OLD_DEVICE_STATUS)));
        assert_eq!(read_word(&seeq, 6), Ok(u32::from(OLD_DEVICE_STATUS)));
        let mut high_lane = [0xff];
        seeq.read(DeviceAddr::new(0), &mut high_lane).unwrap();
        assert_eq!(high_lane, [0]);
        let mut low_lane = [0];
        seeq.read(DeviceAddr::new(3), &mut low_lane).unwrap();
        assert_eq!(low_lane, [OLD_DEVICE_STATUS]);
    }

    #[test]
    fn rejects_crossing_unmapped_and_unsupported_transactions() {
        let mut seeq = Seeq8003::new();

        assert_eq!(
            seeq.read(DeviceAddr::new(3), &mut [0; 2]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            seeq.read(DeviceAddr::new(0x20), &mut [0]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            seeq.write(DeviceAddr::new(0), &[]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            seeq.write(DeviceAddr::new(0x20), &[0]),
            Err(BusError::HardwareFault)
        );
    }

    fn frame(destination: [u8; 6]) -> Vec<u8> {
        let mut bytes = vec![0; 60];
        bytes[..6].copy_from_slice(&destination);
        bytes
    }

    #[test]
    fn dribble_preserves_good_and_does_not_require_its_interrupt_enable() {
        use se_core::time::VirtualDuration;

        for (command, errors, expected_status, interrupt, retained) in [
            (0x80, 0x04, 0x34, false, true),
            (0xa0, 0x04, 0x34, true, true),
            (0x84, 0x04, 0x34, true, true),
            (0x82, 0x06, 0x16, true, true),
            (0x84, 0x06, 0x16, true, false),
        ] {
            let mut seeq = Seeq8003::new();
            write_word(&mut seeq, 6, command);
            assert!(seeq.receive(&frame([0xff; 6]), errors));
            seeq.advance_time(VirtualDuration::from_attoseconds(super::wire_time(60)));
            assert_eq!(seeq.receive_status & 0x3f, expected_status);
            assert_eq!(seeq.receive_interrupt_asserted(), interrupt);
            assert_eq!(seeq.receive_status & OLD_DEVICE_STATUS == 0, interrupt);
            assert_eq!(seeq.take_received_frame().is_some(), retained);
        }
    }

    #[test]
    fn overflow_does_not_qualify_for_the_eof_interrupt_condition() {
        use se_core::time::VirtualDuration;

        for (command, interrupt, retained) in [(0x90, false, false), (0x91, true, true)] {
            let mut seeq = Seeq8003::new();
            write_word(&mut seeq, 6, command);
            assert!(seeq.receive(&frame([0xff; 6]), 1));
            seeq.advance_time(VirtualDuration::from_attoseconds(super::wire_time(60)));
            assert_ne!(seeq.receive_status & 1, 0);
            assert_eq!(seeq.receive_status & 0x20, 0);
            assert_eq!(seeq.receive_interrupt_asserted(), interrupt);
            assert_eq!(seeq.receive_status & OLD_DEVICE_STATUS == 0, interrupt);
            assert_eq!(seeq.take_received_frame().is_some(), retained);
        }
    }

    #[test]
    fn receive_filters_and_disabled_reception_keep_identical_link_spacing() {
        for command in [0, 0x70, 0xb0, 0xf0] {
            let mut seeq = Seeq8003::new();
            write_word(&mut seeq, 6, command);
            let bytes = frame([2, 1, 2, 3, 4, 5]);
            assert!(seeq.receive(&bytes, 0));
            assert!(!seeq.receive_ready());
            seeq.advance_time(se_core::time::VirtualDuration::from_attoseconds(
                super::wire_time(60),
            ));
            assert_eq!(seeq.take_received_frame().is_some(), command == 0x70);
            assert!(!seeq.receive_ready());
            seeq.advance_time(se_core::time::VirtualDuration::from_attoseconds(
                super::INTERFRAME_GAP,
            ));
            assert!(seeq.receive_ready());
        }
    }

    #[test]
    fn status_reads_acknowledge_only_the_selected_source_and_debug_reads_do_not() {
        let mut seeq = Seeq8003::new();
        write_word(&mut seeq, 6, 0xb0);
        write_word(&mut seeq, 7, 0x0f);
        let bytes = frame([0xff; 6]);
        assert!(seeq.receive(&bytes, 0));
        assert!(seeq.transmit(bytes));
        seeq.advance_time(se_core::time::VirtualDuration::from_attoseconds(
            super::wire_time(60),
        ));
        assert!(seeq.receive_interrupt_asserted());
        assert!(seeq.transmit_interrupt_asserted());
        assert_eq!(read_word(&seeq, 6), Ok(0x30));
        assert_eq!(read_word(&seeq, 7), Ok(8));
        assert!(seeq.receive_interrupt_asserted());
        seeq.read(DeviceAddr::new(0x1b), &mut [0]).unwrap();
        assert!(!seeq.receive_interrupt_asserted());
        assert!(seeq.transmit_interrupt_asserted());
        seeq.read(DeviceAddr::new(0x1f), &mut [0]).unwrap();
        assert!(!seeq.transmit_interrupt_asserted());
        assert_eq!(read_word(&seeq, 7), Ok(0x88));
    }

    #[test]
    fn frame_resource_bound_applies_to_receive_and_transmit_data() {
        for length in [16_384, 16_385] {
            let bytes = vec![0xff; length];
            let mut seeq = Seeq8003::new();
            assert_eq!(seeq.receive(&bytes, 0), length == 16_384);
            assert_eq!(seeq.transmit(bytes), length == 16_384);
        }
    }

    #[test]
    fn reset_cancels_inflight_frames_and_restores_old_status() {
        let mut seeq = Seeq8003::new();
        assert!(seeq.receive(&frame([0xff; 6]), 0));
        assert!(seeq.transmit(frame([0xff; 6])));
        seeq.reset();
        seeq.advance_time(se_core::time::VirtualDuration::from_attoseconds(
            super::wire_time(60),
        ));
        assert!(seeq.take_received_frame().is_none());
        assert!(seeq.take_transmitted_frame().is_none());
        assert_eq!(read_word(&seeq, 7), Ok(0x80));
    }
}
