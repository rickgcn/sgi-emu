//! Western Digital WD33C93B indirect-register and command model.
//!
//! The host access layout uses two four-byte windows, with the connected byte
//! at offsets 2 and 6 (bits 15:8 of each big-endian word). This is the adopted
//! host layout, not an intrinsic property of the eight-bit WD chip interface.
//! The machine chooses the physical mapping. Indigo IP12 header byte addresses
//! and NetBSD word accesses provide the layout evidence; the WD33C93A manual
//! describes the underlying indirect access and interrupt rules.
//!
//! Aligned byte, halfword, and word transactions access at most one chip port.
//! Unconnected lanes read zero and ignore writes without a chip access. This
//! convention and rejection of other transaction shapes are model choices,
//! not verified Indigo hardware responses. Undefined chip registers retain
//! their all-ones data value in the connected lane.
//!
//! Initiator commands follow the common WD33C93A command and status tables
//! (sections 6.2.16, 6.2.19 and 7). A one-byte host staging buffer applies
//! backpressure between scheduler events; it does not model FIFO throughput or
//! synchronous REQ/ACK timing. Command acceptance clears CIP independently of
//! BSY. Selection completion and the first information-phase request are
//! separated by status acknowledgement. Message In leaves ACK asserted.
//! The machine supplies the input-clock frequency at construction. This device
//! converts controller timeouts to virtual durations; board DMA and event
//! scheduling remain outside this device.

use se_core::bus::{BusError, DeviceAddr};
use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
use serde::{Deserialize, Serialize};

use crate::scsi::{
    ScsiBus, ScsiBusError, ScsiDataDirection, ScsiPhase, ScsiStatus, ScsiTransferResult,
};

const ADDRESS_PORT: u64 = 2;
const DATA_PORT: u64 = 6;
const REGISTER_BYTES: u64 = 4;
const HOST_WINDOW_BYTES: u64 = 8;

const OWN_ID: u8 = 0x00;
const CONTROL: u8 = 0x01;
const TIMEOUT_PERIOD: u8 = 0x02;
const CDB_START: u8 = 0x03;
const CDB_END: u8 = 0x0e;
const TARGET_LUN: u8 = 0x0f;
const COMMAND_PHASE: u8 = 0x10;
const SYNCHRONOUS_TRANSFER: u8 = 0x11;
const TRANSFER_COUNT_MSB: u8 = 0x12;
const TRANSFER_COUNT_MID: u8 = 0x13;
const TRANSFER_COUNT_LSB: u8 = 0x14;
const DESTINATION_ID: u8 = 0x15;
const SOURCE_ID: u8 = 0x16;
const SCSI_STATUS: u8 = 0x17;
const COMMAND: u8 = 0x18;
const DATA: u8 = 0x19;
const AUXILIARY_STATUS: u8 = 0x1f;

const INTERRUPT_PENDING: u8 = 0x80;
const BUSY: u8 = 0x20;
const COMMAND_IN_PROGRESS: u8 = 0x10;
const SOFTWARE_RESET: u8 = 0x00;
const SELECT_AND_TRANSFER: u8 = 0x09;
const SELECT_AND_TRANSFER_WITH_ATN: u8 = 0x08;
const RESET_COMPLETION_STATUS: u8 = 0x00;
const ADVANCED_RESET_COMPLETION_STATUS: u8 = 0x01;
const SELECT_AND_TRANSFER_COMPLETION_STATUS: u8 = 0x16;
const UNEXPECTED_DATA_OUT_STATUS: u8 = 0x48;
const UNEXPECTED_DATA_IN_STATUS: u8 = 0x49;
const SELECTION_TIMEOUT_STATUS: u8 = 0x42;
const DATA_TRANSFER_PHASE: u8 = 0x46;
const SELECT_AND_TRANSFER_PHASE: u8 = 0x60;
const ADVANCED_FEATURES: u8 = 1 << 3;
const SOURCE_ID_PRESERVED_BITS: u8 = 0x0f;
const TRANSFER_COUNT_MASK: u32 = 0x00ff_ffff;

/// One latched controller operation to be serviced by the machine scheduler.
/// The machine forwards it without interpreting SCSI phases or messages.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WdRequest(RequestKind);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum RequestKind {
    Combined(SelectAndTransferRequest),
    Select { target_id: u8, attention: bool },
    Transfer,
    Pump,
    Output(u8),
    Phase,
    Acknowledge,
    Attention,
    Abort,
    Disconnect,
    Timeout,
}

/// Board work remaining after servicing a controller operation.
pub enum WdWork {
    /// No board-side transfer or timer is required.
    Idle,
    /// Connect the current payload transfer to the board's DMA engine.
    Dma {
        /// Direction requested by the target.
        direction: ScsiDataDirection,
        /// Maximum bytes accepted by this controller command.
        byte_count: u32,
    },
    /// Service the pending timeout after this virtual duration.
    /// `None` means automatic selection timeout is disabled.
    SelectionWait(Option<VirtualDuration>),
}

/// A stable Select-And-Transfer request produced by the WD33C93B.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SelectAndTransferRequest {
    destination_id: u8,
    lun: u8,
    transfer_count: u32,
    cdb: [u8; 12],
    cdb_length: u8,
}

impl SelectAndTransferRequest {
    /// Returns the selected target identifier.
    #[must_use]
    pub const fn destination_id(&self) -> u8 {
        self.destination_id
    }

    /// Returns the selected logical unit number.
    #[must_use]
    pub const fn lun(&self) -> u8 {
        self.lun
    }

    /// Returns the programmed transfer count.
    #[must_use]
    pub const fn transfer_count(&self) -> u32 {
        self.transfer_count
    }

    /// Returns the command descriptor block.
    #[must_use]
    pub fn cdb(&self) -> &[u8] {
        &self.cdb[..usize::from(self.cdb_length)]
    }
}

/// The software-visible WD33C93B state used by the IP12 machine.
#[derive(Clone, Deserialize, Serialize)]
pub struct Wd33c93b {
    clock_hz: u64,
    selected_register: u8,
    own_id: u8,
    control: u8,
    timeout_period: u8,
    cdb: [u8; 12],
    target_lun: u8,
    command_phase: u8,
    synchronous_transfer: u8,
    transfer_count: u32,
    destination_id: u8,
    source_id: u8,
    scsi_status: u8,
    command: u8,
    command_in_progress: bool,
    interrupt_pending: bool,
    pending_request: Option<WdRequest>,
    software_reset_completed: bool,
    busy: bool,
    combined: bool,
    attention: bool,
    ack_asserted: bool,
    phase_after_status: bool,
    pio_phase: Option<ScsiPhase>,
    pio_remaining: u32,
    single_byte: bool,
    input_byte: Option<u8>,
    output_ready: bool,
    aborting: bool,
}

impl Wd33c93b {
    /// Creates a controller after hardware reset completion with the supplied
    /// input-clock frequency in hertz.
    ///
    /// # Panics
    ///
    /// Panics if `clock_hz` is zero.
    #[must_use]
    pub const fn new(clock_hz: u64) -> Self {
        assert!(clock_hz != 0);
        Self {
            clock_hz,
            selected_register: 0,
            own_id: 0,
            control: 0,
            timeout_period: 0,
            cdb: [0; 12],
            target_lun: 0,
            command_phase: 0,
            synchronous_transfer: 0,
            transfer_count: 0,
            destination_id: 0,
            source_id: 0,
            scsi_status: RESET_COMPLETION_STATUS,
            command: 0,
            command_in_progress: false,
            interrupt_pending: true,
            pending_request: None,
            software_reset_completed: false,
            busy: false,
            combined: false,
            attention: false,
            ack_asserted: false,
            phase_after_status: false,
            pio_phase: None,
            pio_remaining: 0,
            single_byte: false,
            input_byte: None,
            output_ready: false,
            aborting: false,
        }
    }

    /// Applies a hardware reset while preserving the documented programming
    /// registers.
    pub fn reset(&mut self) {
        self.selected_register = 0;
        self.own_id = 0;
        self.source_id &= SOURCE_ID_PRESERVED_BITS;
        self.scsi_status = RESET_COMPLETION_STATUS;
        self.command_in_progress = false;
        self.interrupt_pending = true;
        self.pending_request = None;
        self.software_reset_completed = false;
        self.clear_transfer_state();
    }

    /// Reads one transaction from the device-local host register windows.
    ///
    /// Reading SCSI Status acknowledges the current interrupt and advances the
    /// selector to Command. Each transaction accesses at most one byte port.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for invalid lengths or address
    /// overflow, or [`BusError::UnimplementedAccess`] for unsupported offsets,
    /// widths, alignment, or transactions crossing a register window.
    pub fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let access = decode_access(address, data.len())?;
        data.fill(0);
        if let Some(lane) = access.lane {
            data[lane] = self.read_port(access.port);
        }
        Ok(())
    }

    /// Reads a host register window without changing selector or interrupt state.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for invalid lengths or address
    /// overflow, or [`BusError::UnimplementedAccess`] for unsupported offsets,
    /// widths, alignment, or transactions crossing a register window.
    pub fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let access = decode_access(address, data.len())?;
        data.fill(0);
        if let Some(lane) = access.lane {
            data[lane] = self.peek_port(access.port);
        }
        Ok(())
    }

    /// Writes only the connected byte in a device-local host register window.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for invalid lengths or address
    /// overflow, or [`BusError::UnimplementedAccess`] for unsupported offsets,
    /// widths, alignment, or transactions crossing a register window.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let access = decode_access(address, data.len())?;
        if let Some(lane) = access.lane {
            // Valid chip commands outside the supported initiator subset must
            // not be silently accepted or reported as illegal hardware opcodes.
            if matches!(access.port, Port::Data)
                && self.selected_register == COMMAND
                && !matches!(data[lane], 0..=4 | 6..=9 | 0x20 | 0xa0)
            {
                return Err(BusError::UnimplementedAccess);
            }
            self.write_port(access.port, data[lane]);
        }
        Ok(())
    }

    /// Performs one eight-bit chip read and applies its register side effects.
    fn read_port(&mut self, port: Port) -> u8 {
        let value = self.peek_port(port);
        if matches!(port, Port::Data) {
            let register = self.selected_register;
            if register == SCSI_STATUS {
                self.interrupt_pending = false;
                self.selected_register = COMMAND;
                if self.phase_after_status {
                    self.phase_after_status = false;
                    self.pending_request = Some(WdRequest(RequestKind::Phase));
                }
            } else if register == DATA && self.input_byte.take().is_some() {
                self.pending_request = Some(WdRequest(RequestKind::Pump));
            } else if selector_advances(register) {
                self.selected_register = register.wrapping_add(1) & 0x1f;
            }
        }
        value
    }

    /// Observes one eight-bit chip port without strobing the selected register.
    fn peek_port(&self, port: Port) -> u8 {
        match port {
            Port::Address => self.auxiliary_status(),
            Port::Data => self.read_selected_register(self.selected_register),
        }
    }

    /// Performs one eight-bit chip write and applies its register side effects.
    fn write_port(&mut self, port: Port, value: u8) {
        match port {
            Port::Address => self.selected_register = value & 0x1f,
            Port::Data => {
                let register = self.selected_register;
                self.write_selected_register(register, value);
                if selector_advances(register) {
                    self.selected_register = register.wrapping_add(1) & 0x1f;
                }
            }
        }
    }

    /// Returns and clears one pending controller operation.
    pub fn take_request(&mut self) -> Option<WdRequest> {
        self.pending_request.take()
    }

    #[cfg(test)]
    fn take_select_and_transfer_request(&mut self) -> Option<SelectAndTransferRequest> {
        match self.take_request()?.0 {
            RequestKind::Combined(request) => Some(request),
            _ => None,
        }
    }

    /// Advances the controller against a connected functional SCSI bus.
    ///
    /// The caller supplies board wiring and schedules returned virtual durations;
    /// this method owns command interpretation and status generation.
    ///
    /// # Errors
    /// Returns [`ScsiBusError`] if controller and bus operations are inconsistent.
    pub fn service_request(
        &mut self,
        request: WdRequest,
        bus: &mut ScsiBus,
    ) -> Result<WdWork, ScsiBusError> {
        self.command_in_progress = false;
        match request.0 {
            RequestKind::Select {
                target_id,
                attention,
            } => {
                if bus.phase().is_some() {
                    self.raise_status(0x40);
                    return Ok(WdWork::Idle);
                }
                if !bus.select(target_id, attention)? {
                    return Ok(self.wait_for_selection());
                }
                self.raise_status(0x11);
                self.phase_after_status = true;
            }
            RequestKind::Combined(request) => return self.service_combined(request, bus),
            RequestKind::Transfer => {
                let Some(phase) = bus.phase() else {
                    self.raise_status(0x40);
                    return Ok(WdWork::Idle);
                };
                self.pio_phase = Some(phase);
                self.pio_remaining = if self.single_byte {
                    1
                } else {
                    self.transfer_count
                };
                return self.pump(bus);
            }
            RequestKind::Pump => return self.pump(bus),
            RequestKind::Output(value) => {
                let last = self.pio_remaining == 1;
                let consumed = bus.write_information(value, last)?;
                if self.pio_phase == Some(ScsiPhase::MessageOut) && last {
                    self.attention = false;
                }
                if consumed {
                    self.advance_information_byte();
                }
                return self.pump(bus);
            }
            RequestKind::Phase => self.report_phase(bus, 0x88),
            RequestKind::Acknowledge => {
                if self.ack_asserted {
                    self.ack_asserted = false;
                    bus.acknowledge_message(self.attention);
                    self.report_phase(bus, 0x88);
                }
            }
            RequestKind::Attention => {
                if bus.phase().is_none() {
                    self.raise_status(0x40);
                } else if !self.ack_asserted {
                    bus.assert_attention();
                    if self.interrupt_pending {
                        self.phase_after_status = true;
                    } else if self.busy {
                        return self.pump(bus);
                    } else {
                        self.report_phase(bus, 0x88);
                    }
                }
            }
            RequestKind::Abort => {
                if self.input_byte.is_some() {
                    self.aborting = true;
                } else {
                    self.abort_transfer(bus);
                }
            }
            RequestKind::Disconnect => {
                bus.cancel_transaction();
                self.clear_transfer_state();
            }
            RequestKind::Timeout => self.finish_selection_timeout(),
        }
        Ok(WdWork::Idle)
    }

    fn wait_for_selection(&mut self) -> WdWork {
        self.busy = true;
        self.pending_request = Some(WdRequest(RequestKind::Timeout));
        // WD33C93A section 6.2.5: T(ms) = register * 80 / CLK(MHz).
        WdWork::SelectionWait((self.timeout_period != 0).then(|| {
            let clocks = u128::from(self.timeout_period) * 80_000;
            VirtualDuration::from_attoseconds(
                (clocks * ATTOSECONDS_PER_SECOND).div_ceil(u128::from(self.clock_hz)),
            )
        }))
    }

    fn service_combined(
        &mut self,
        request: SelectAndTransferRequest,
        bus: &mut ScsiBus,
    ) -> Result<WdWork, ScsiBusError> {
        if bus.phase().is_none() {
            if !bus.select(request.destination_id, self.attention)? {
                return Ok(self.wait_for_selection());
            }
            self.command_phase = 0x10;
            if self.attention {
                bus.write_information(0x80 | request.lun, true)?;
                self.attention = false;
                self.command_phase = 0x20;
            }
            for &byte in request.cdb() {
                bus.write_information(byte, false)?;
            }
        } else if bus.connected_address().map(|address| address.0) != Some(request.destination_id)
            || !matches!(self.command_phase, 0x45 | DATA_TRANSFER_PHASE)
        {
            self.raise_status(0x40);
            return Ok(WdWork::Idle);
        }
        self.pio_phase = bus.phase();
        self.pio_remaining = self.transfer_count;
        self.single_byte = false;
        self.pump(bus)
    }

    fn pump(&mut self, bus: &mut ScsiBus) -> Result<WdWork, ScsiBusError> {
        if self.aborting && self.input_byte.is_none() {
            self.abort_transfer(bus);
            return Ok(WdWork::Idle);
        }
        if self.combined && bus.phase() == Some(ScsiPhase::Status) && self.input_byte.is_none() {
            let status = bus.read_information()?.ok_or(ScsiBusError::InvalidPhase)?;
            let message = bus.read_information()?;
            if message != Some(0) {
                return Err(ScsiBusError::InvalidPhase);
            }
            bus.acknowledge_message(false);
            self.finish_select_and_transfer(status);
            return Ok(WdWork::Idle);
        }
        let Some(phase) = self.pio_phase else {
            return Ok(WdWork::Idle);
        };
        if self.input_byte.is_some() {
            return Ok(WdWork::Idle);
        }
        if self.pio_remaining == 0 || bus.phase() != Some(phase) {
            if !self.combined && phase == ScsiPhase::MessageIn && self.pio_remaining == 0 {
                self.ack_asserted = true;
                self.raise_status(0x20);
            } else if self.combined {
                self.command_phase = DATA_TRANSFER_PHASE;
                self.report_phase(bus, 0x48);
            } else {
                self.report_phase(bus, if self.pio_remaining == 0 { 0x18 } else { 0x48 });
            }
            self.pio_phase = None;
            return Ok(WdWork::Idle);
        }
        if matches!(phase, ScsiPhase::DataIn | ScsiPhase::DataOut) && self.control & 0xe0 != 0 {
            return Ok(WdWork::Dma {
                direction: if phase.input() {
                    ScsiDataDirection::In
                } else {
                    ScsiDataDirection::Out
                },
                byte_count: self.pio_remaining,
            });
        }
        if phase.input() {
            self.input_byte = bus.read_information()?;
            if self.input_byte.is_some() {
                self.advance_information_byte();
            } else {
                return self.pump(bus);
            }
        } else {
            self.output_ready = true;
        }
        Ok(WdWork::Idle)
    }

    fn advance_information_byte(&mut self) {
        self.pio_remaining -= 1;
        if self.single_byte {
            self.transfer_count = 0;
        } else {
            self.transfer_count -= 1;
        }
    }

    fn report_phase(&mut self, bus: &ScsiBus, class: u8) {
        self.raise_status(bus.phase().map_or(0x85, |phase| class | phase.bits()));
    }

    fn raise_status(&mut self, status: u8) {
        self.scsi_status = status;
        self.command_in_progress = false;
        self.busy = false;
        self.output_ready = false;
        self.interrupt_pending = true;
    }

    fn abort_transfer(&mut self, bus: &ScsiBus) {
        self.pio_phase = None;
        self.aborting = false;
        self.raise_status(bus.phase().map_or(0x22, |phase| 0x28 | phase.bits()));
    }

    fn clear_transfer_state(&mut self) {
        self.busy = false;
        self.combined = false;
        self.attention = false;
        self.ack_asserted = false;
        self.phase_after_status = false;
        self.pio_phase = None;
        self.pio_remaining = 0;
        self.single_byte = false;
        self.input_byte = None;
        self.output_ready = false;
        self.aborting = false;
    }

    /// Completes a board DMA window without executing another target command.
    ///
    /// # Errors
    /// Returns [`ScsiBusError`] if the bus cannot advance to the next phase.
    pub fn finish_dma(
        &mut self,
        bus: &mut ScsiBus,
        status: Option<ScsiStatus>,
    ) -> Result<(), ScsiBusError> {
        self.pio_remaining = self.transfer_count;
        if let Some(status) = status {
            bus.observe_transfer_result(ScsiTransferResult::Complete {
                transferred: 0,
                status,
            });
        }
        let _ = self.pump(bus)?;
        Ok(())
    }

    /// Reports an active DMA payload waiting for the board's request acknowledgement.
    #[must_use]
    pub fn dma_pending(&self) -> bool {
        self.busy
            && self.control & 0xe0 != 0
            && matches!(self.pio_phase, Some(ScsiPhase::DataIn | ScsiPhase::DataOut))
            && self.pio_remaining != 0
    }

    /// Returns the remaining bytes in an active controller transfer.
    #[must_use]
    pub const fn remaining_transfer_bytes(&self) -> u32 {
        if self.single_byte {
            self.pio_remaining
        } else {
            self.transfer_count
        }
    }

    /// Subtracts bytes accepted by the initiator from the transfer residual.
    ///
    /// Returns `false` without modifying state when `byte_count` exceeds the
    /// residual.
    pub fn consume_transfer_bytes(&mut self, byte_count: u32) -> bool {
        if self.single_byte {
            if byte_count != 1 || self.pio_remaining != 1 {
                return false;
            }
            self.transfer_count = 0;
            self.pio_remaining = 0;
            return true;
        }
        if byte_count > self.transfer_count {
            return false;
        }
        self.transfer_count -= byte_count;
        true
    }

    /// Completes the active command with a target status byte.
    pub fn finish_select_and_transfer(&mut self, target_status: u8) {
        self.clear_transfer_state();
        self.target_lun = target_status;
        self.command_phase = SELECT_AND_TRANSFER_PHASE;
        self.scsi_status = SELECT_AND_TRANSFER_COMPLETION_STATUS;
        self.command_in_progress = false;
        self.interrupt_pending = true;
        self.pending_request = None;
    }

    /// Signals that a Data In target has more bytes after the programmed
    /// transfer count reaches zero.
    pub fn request_data_in_continuation(&mut self) {
        self.request_data_continuation(UNEXPECTED_DATA_IN_STATUS);
    }

    /// Signals that a Data Out target expects more bytes after the programmed
    /// transfer count reaches zero.
    pub fn request_data_out_continuation(&mut self) {
        self.request_data_continuation(UNEXPECTED_DATA_OUT_STATUS);
    }

    fn request_data_continuation(&mut self, status: u8) {
        self.busy = false;
        self.command_phase = DATA_TRANSFER_PHASE;
        self.scsi_status = status;
        self.command_in_progress = false;
        self.interrupt_pending = true;
        self.pending_request = None;
    }

    /// Completes selection without finding the requested target.
    pub fn finish_selection_timeout(&mut self) {
        self.clear_transfer_state();
        self.command_phase = 0;
        self.scsi_status = SELECTION_TIMEOUT_STATUS;
        self.command_in_progress = false;
        self.interrupt_pending = true;
        self.pending_request = None;
    }

    /// Reports the controller interrupt output level.
    #[must_use]
    pub const fn interrupt_asserted(&self) -> bool {
        self.interrupt_pending
    }

    /// Returns and clears notification of a completed software reset command.
    pub fn take_reset_completion(&mut self) -> bool {
        let completed = self.software_reset_completed;
        self.software_reset_completed = false;
        completed
    }

    const fn auxiliary_status(&self) -> u8 {
        let mut value = 0;
        if self.interrupt_pending {
            value |= INTERRUPT_PENDING;
        }
        if self.command_in_progress {
            value |= COMMAND_IN_PROGRESS;
        }
        if self.busy {
            value |= BUSY;
        }
        if self.input_byte.is_some() || self.output_ready {
            value |= 1;
        }
        value
    }

    fn read_selected_register(&self, register: u8) -> u8 {
        match register {
            OWN_ID => self.own_id,
            CONTROL => self.control,
            TIMEOUT_PERIOD => self.timeout_period,
            CDB_START..=CDB_END => self.cdb[(register - CDB_START) as usize],
            TARGET_LUN => self.target_lun,
            COMMAND_PHASE => self.command_phase,
            SYNCHRONOUS_TRANSFER => self.synchronous_transfer,
            TRANSFER_COUNT_MSB => (self.transfer_count >> 16) as u8,
            TRANSFER_COUNT_MID => (self.transfer_count >> 8) as u8,
            TRANSFER_COUNT_LSB => self.transfer_count as u8,
            DESTINATION_ID => self.destination_id,
            SOURCE_ID => self.source_id,
            SCSI_STATUS => self.scsi_status,
            COMMAND => self.command,
            DATA => self.input_byte.unwrap_or(0xff),
            AUXILIARY_STATUS => self.auxiliary_status(),
            _ => 0xff,
        }
    }

    fn write_selected_register(&mut self, register: u8, value: u8) {
        match register {
            OWN_ID => self.own_id = value,
            CONTROL => self.control = value,
            TIMEOUT_PERIOD => self.timeout_period = value,
            CDB_START..=CDB_END => self.cdb[(register - CDB_START) as usize] = value,
            TARGET_LUN => self.target_lun = value,
            COMMAND_PHASE => self.command_phase = value,
            SYNCHRONOUS_TRANSFER => self.synchronous_transfer = value,
            TRANSFER_COUNT_MSB => {
                self.transfer_count =
                    (self.transfer_count & 0x0000_ffff) | (u32::from(value) << 16);
            }
            TRANSFER_COUNT_MID => {
                self.transfer_count = (self.transfer_count & 0x00ff_00ff) | (u32::from(value) << 8);
            }
            TRANSFER_COUNT_LSB => {
                self.transfer_count = (self.transfer_count & 0x00ff_ff00) | u32::from(value);
            }
            DESTINATION_ID => self.destination_id = value,
            SOURCE_ID => self.source_id = value,
            COMMAND => self.execute_command(value),
            DATA if self.output_ready => {
                self.output_ready = false;
                self.pending_request = Some(WdRequest(RequestKind::Output(value)));
            }
            _ => {}
        }
        self.transfer_count &= TRANSFER_COUNT_MASK;
    }

    fn execute_command(&mut self, command: u8) {
        self.command = command;
        self.pending_request = None;
        self.phase_after_status = false;
        match command {
            SOFTWARE_RESET => self.software_reset(),
            SELECT_AND_TRANSFER | SELECT_AND_TRANSFER_WITH_ATN => {
                self.command_in_progress = true;
                self.busy = true;
                self.combined = true;
                self.attention = command == SELECT_AND_TRANSFER_WITH_ATN;
                self.interrupt_pending = false;
                self.software_reset_completed = false;
                self.pending_request =
                    Some(WdRequest(RequestKind::Combined(SelectAndTransferRequest {
                        destination_id: self.destination_id & 0x07,
                        lun: self.target_lun & 0x07,
                        transfer_count: self.transfer_count,
                        cdb: self.cdb,
                        cdb_length: cdb_length(self.cdb[0]),
                    })));
            }
            0x06 | 0x07 => {
                self.clear_transfer_state();
                self.command_in_progress = true;
                self.busy = true;
                self.interrupt_pending = false;
                self.attention = command == 6;
                self.pending_request = Some(WdRequest(RequestKind::Select {
                    target_id: self.destination_id & 7,
                    attention: self.attention,
                }));
            }
            0x20 | 0xa0 => {
                self.combined = false;
                self.busy = true;
                self.command_in_progress = true;
                self.interrupt_pending = false;
                self.input_byte = None;
                self.output_ready = false;
                self.single_byte = command & 0x80 != 0 || self.transfer_count == 0;
                self.pending_request = Some(WdRequest(RequestKind::Transfer));
            }
            1 => self.pending_request = Some(WdRequest(RequestKind::Abort)),
            2 => {
                self.attention = true;
                self.pending_request = Some(WdRequest(RequestKind::Attention));
            }
            3 => self.pending_request = Some(WdRequest(RequestKind::Acknowledge)),
            4 => self.pending_request = Some(WdRequest(RequestKind::Disconnect)),
            _ => unreachable!("host command writes are validated before changing state"),
        }
    }

    fn software_reset(&mut self) {
        let advanced = self.own_id & ADVANCED_FEATURES != 0;
        self.control = 0;
        self.timeout_period = 0;
        self.cdb = [0; 12];
        self.target_lun = 0;
        self.command_phase = 0;
        self.synchronous_transfer = 0;
        self.transfer_count = 0;
        self.destination_id = 0;
        self.source_id = 0;
        self.scsi_status = if advanced {
            ADVANCED_RESET_COMPLETION_STATUS
        } else {
            RESET_COMPLETION_STATUS
        };
        self.command = SOFTWARE_RESET;
        self.command_in_progress = false;
        self.interrupt_pending = true;
        self.pending_request = None;
        self.software_reset_completed = true;
        self.clear_transfer_state();
    }
}

#[derive(Clone, Copy)]
enum Port {
    Address,
    Data,
}

/// One validated host transaction and its optional connected byte.
struct PortAccess {
    port: Port,
    lane: Option<usize>,
}

fn decode_access(address: DeviceAddr, length: usize) -> Result<PortAccess, BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }

    let start = address.get();
    let length = length as u64;
    let end = start
        .checked_add(length)
        .ok_or(BusError::InvalidTransaction)?;
    if end > HOST_WINDOW_BYTES
        || start / REGISTER_BYTES != (end - 1) / REGISTER_BYTES
        || !matches!(length, 1 | 2 | 4)
        || !start.is_multiple_of(length)
    {
        return Err(BusError::UnimplementedAccess);
    }

    let (port, connected_byte) = if start < REGISTER_BYTES {
        (Port::Address, ADDRESS_PORT)
    } else {
        (Port::Data, DATA_PORT)
    };
    let lane = (start <= connected_byte && connected_byte < end)
        .then(|| (connected_byte - start) as usize);
    Ok(PortAccess { port, lane })
}

const fn selector_advances(register: u8) -> bool {
    !matches!(register, COMMAND | DATA | AUXILIARY_STATUS)
}

const fn cdb_length(opcode: u8) -> u8 {
    match opcode >> 5 {
        0 => 6,
        1 | 2 => 10,
        5 => 12,
        _ => 6,
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};

    use super::{
        ADDRESS_PORT, AUXILIARY_STATUS, BUSY, CDB_START, COMMAND, COMMAND_IN_PROGRESS,
        COMMAND_PHASE, CONTROL, DATA_PORT, DATA_TRANSFER_PHASE, DESTINATION_ID, INTERRUPT_PENDING,
        OWN_ID, SCSI_STATUS, SELECT_AND_TRANSFER, SOURCE_ID, TARGET_LUN, TIMEOUT_PERIOD,
        TRANSFER_COUNT_MSB, UNEXPECTED_DATA_IN_STATUS, UNEXPECTED_DATA_OUT_STATUS, Wd33c93b,
    };

    fn read_word(scsi: &mut Wd33c93b, offset: u64) -> Result<u32, BusError> {
        let mut bytes = [0; 4];
        scsi.read(DeviceAddr::new(offset), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn access_widths_share_the_connected_byte_and_ignore_other_write_bits() {
        let mut scsi = Wd33c93b::new(20_000_000);
        for (offset, selector, value) in [
            (2, vec![2], vec![0xa5]),
            (2, vec![2, 0xff], vec![0xa5, 0xff]),
            (0, vec![0xff, 0xff, 2, 0xff], vec![0xff, 0xff, 0xa5, 0xff]),
        ] {
            scsi.write(DeviceAddr::new(offset), &selector).unwrap();
            scsi.write(DeviceAddr::new(4 + offset), &value).unwrap();
            for (read_offset, expected) in [
                (2, vec![0xa5]),
                (2, vec![0xa5, 0]),
                (0, vec![0, 0, 0xa5, 0]),
            ] {
                scsi.write(DeviceAddr::new(ADDRESS_PORT), &[2]).unwrap();
                let mut actual = vec![0xff; expected.len()];
                scsi.read(DeviceAddr::new(4 + read_offset), &mut actual)
                    .unwrap();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn unconnected_lanes_do_not_access_the_selected_register() {
        let mut scsi = Wd33c93b::new(20_000_000);
        write_register(&mut scsi, 2, 0xa5);
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[2]).unwrap();
        for slot in [0, 4] {
            for (offset, length) in [(0, 1), (1, 1), (3, 1), (0, 2)] {
                let address = DeviceAddr::new(slot + offset);
                let mut bytes = vec![0xff; length];
                scsi.write(address, &bytes).unwrap();
                scsi.read(address, &mut bytes).unwrap();
                assert_eq!(bytes, vec![0; length]);
            }
        }
        assert_eq!(read_port(&mut scsi, DATA_PORT), Ok(0xa5));
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[0x17]).unwrap();
        assert_eq!(read_port(&mut scsi, 4), Ok(0));
        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(0x80));
    }

    #[test]
    fn invalid_accesses_preserve_output_and_device_state() {
        let mut scsi = Wd33c93b::new(20_000_000);
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[0x17]).unwrap();
        for (offset, length, error) in [
            (0, 0, BusError::InvalidTransaction),
            (0, 5, BusError::InvalidTransaction),
            (u64::MAX, 2, BusError::InvalidTransaction),
            (0, 3, BusError::UnimplementedAccess),
            (1, 2, BusError::UnimplementedAccess),
            (2, 4, BusError::UnimplementedAccess),
            (7, 2, BusError::UnimplementedAccess),
            (8, 1, BusError::UnimplementedAccess),
        ] {
            let mut bytes = vec![0xa5; length];
            let address = DeviceAddr::new(offset);
            assert_eq!(scsi.read(address, &mut bytes), Err(error));
            assert_eq!(bytes, vec![0xa5; length]);
            assert_eq!(scsi.debug_read(address, &mut bytes), Err(error));
            assert_eq!(bytes, vec![0xa5; length]);
            assert_eq!(scsi.write(address, &bytes), Err(error));
        }
        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(0x80));
        assert_eq!(read_port(&mut scsi, DATA_PORT), Ok(0));
        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(0));
    }

    #[test]
    fn undefined_chip_registers_keep_their_all_ones_value_in_the_connected_lane() {
        let mut scsi = Wd33c93b::new(20_000_000);
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[0x1a]).unwrap();
        assert_eq!(read_word(&mut scsi, 4), Ok(0xff00));
    }

    fn read_port(scsi: &mut Wd33c93b, port: u64) -> Result<u8, BusError> {
        let mut value = [0];
        scsi.read(DeviceAddr::new(port), &mut value)?;
        Ok(value[0])
    }

    fn write_register(scsi: &mut Wd33c93b, register: u8, value: u8) {
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[register])
            .unwrap();
        scsi.write(DeviceAddr::new(DATA_PORT), &[value]).unwrap();
    }

    fn read_register(scsi: &mut Wd33c93b, register: u8) -> u8 {
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[register])
            .unwrap();
        read_port(scsi, DATA_PORT).unwrap()
    }

    #[test]
    fn address_port_selects_low_five_bits_and_reads_auxiliary_status() {
        let mut scsi = Wd33c93b::new(20_000_000);

        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(INTERRUPT_PENDING));
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[0xe2]).unwrap();
        assert_eq!(scsi.selected_register, TIMEOUT_PERIOD);
    }

    #[test]
    fn select_and_transfer_latches_a_stable_request() {
        let mut scsi = Wd33c93b::new(20_000_000);
        write_register(&mut scsi, DESTINATION_ID, 1);
        write_register(&mut scsi, TARGET_LUN, 2);
        write_register(&mut scsi, TRANSFER_COUNT_MSB, 0x01);
        write_register(&mut scsi, TRANSFER_COUNT_MSB + 1, 0x23);
        write_register(&mut scsi, TRANSFER_COUNT_MSB + 2, 0x45);
        for (offset, value) in [0x28, 0, 0, 0, 4, 0, 0, 0, 2, 0].into_iter().enumerate() {
            write_register(&mut scsi, CDB_START + offset as u8, value);
        }

        write_register(&mut scsi, COMMAND, SELECT_AND_TRANSFER);

        assert_eq!(
            read_port(&mut scsi, ADDRESS_PORT),
            Ok(BUSY | COMMAND_IN_PROGRESS)
        );
        let request = scsi.take_select_and_transfer_request().unwrap();
        assert_eq!(request.destination_id(), 1);
        assert_eq!(request.lun(), 2);
        assert_eq!(request.transfer_count(), 0x01_2345);
        assert_eq!(request.cdb(), &[0x28, 0, 0, 0, 4, 0, 0, 0, 2, 0]);
        assert!(scsi.take_select_and_transfer_request().is_none());
    }

    #[test]
    fn completion_preserves_residual_and_status_read_acknowledges_interrupt() {
        let mut scsi = Wd33c93b::new(20_000_000);
        write_register(&mut scsi, TRANSFER_COUNT_MSB + 2, 16);
        write_register(&mut scsi, COMMAND, SELECT_AND_TRANSFER);
        assert!(scsi.consume_transfer_bytes(12));
        assert!(!scsi.consume_transfer_bytes(5));
        scsi.finish_select_and_transfer(2);

        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(INTERRUPT_PENDING));
        assert_eq!(read_register(&mut scsi, TARGET_LUN), 2);
        assert_eq!(read_register(&mut scsi, TRANSFER_COUNT_MSB + 2), 4);
        assert_eq!(read_register(&mut scsi, SCSI_STATUS), 0x16);
        assert!(!scsi.interrupt_asserted());
        assert_eq!(scsi.selected_register, COMMAND);
    }

    #[test]
    fn exhausted_data_in_count_requests_another_transfer_window() {
        let mut scsi = Wd33c93b::new(20_000_000);
        write_register(&mut scsi, TRANSFER_COUNT_MSB + 2, 8);
        write_register(&mut scsi, COMMAND, SELECT_AND_TRANSFER);
        assert!(scsi.consume_transfer_bytes(8));

        scsi.request_data_in_continuation();

        assert_eq!(read_register(&mut scsi, COMMAND_PHASE), DATA_TRANSFER_PHASE);
        assert_eq!(
            read_register(&mut scsi, SCSI_STATUS),
            UNEXPECTED_DATA_IN_STATUS
        );
        write_register(&mut scsi, TRANSFER_COUNT_MSB + 2, 4);
        write_register(&mut scsi, COMMAND, SELECT_AND_TRANSFER);
        let request = scsi.take_select_and_transfer_request().unwrap();
        assert_eq!(request.transfer_count(), 4);
        assert_eq!(
            read_port(&mut scsi, ADDRESS_PORT),
            Ok(BUSY | COMMAND_IN_PROGRESS)
        );
    }

    #[test]
    fn exhausted_data_out_count_requests_another_transfer_window() {
        let mut scsi = Wd33c93b::new(20_000_000);
        write_register(&mut scsi, TRANSFER_COUNT_MSB + 2, 8);
        write_register(&mut scsi, COMMAND, SELECT_AND_TRANSFER);
        assert!(scsi.consume_transfer_bytes(8));

        scsi.request_data_out_continuation();

        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(INTERRUPT_PENDING));
        assert_eq!(read_register(&mut scsi, COMMAND_PHASE), DATA_TRANSFER_PHASE);
        assert_eq!(
            read_register(&mut scsi, SCSI_STATUS),
            UNEXPECTED_DATA_OUT_STATUS
        );
        assert!(!scsi.interrupt_asserted());
        assert_eq!(read_register(&mut scsi, TRANSFER_COUNT_MSB + 2), 0);
    }

    #[test]
    fn selection_timeout_retains_the_full_transfer_count() {
        let mut scsi = Wd33c93b::new(20_000_000);
        write_register(&mut scsi, TRANSFER_COUNT_MSB + 2, 8);
        write_register(&mut scsi, COMMAND, SELECT_AND_TRANSFER);
        scsi.finish_selection_timeout();

        assert_eq!(read_register(&mut scsi, TRANSFER_COUNT_MSB + 2), 8);
        assert_eq!(read_register(&mut scsi, SCSI_STATUS), 0x42);
    }

    #[test]
    fn software_and_hardware_reset_preserve_different_registers() {
        let mut scsi = Wd33c93b::new(20_000_000);
        write_register(&mut scsi, CONTROL, 0x55);
        write_register(&mut scsi, TIMEOUT_PERIOD, 0xa5);
        write_register(&mut scsi, SOURCE_ID, 0xf3);

        scsi.reset();

        assert_eq!(read_register(&mut scsi, CONTROL), 0x55);
        assert_eq!(read_register(&mut scsi, TIMEOUT_PERIOD), 0xa5);
        assert_eq!(read_register(&mut scsi, SOURCE_ID), 0x03);
        assert!(!scsi.take_reset_completion());

        write_register(&mut scsi, OWN_ID, 0x08);
        write_register(&mut scsi, COMMAND, 0);
        assert_eq!(read_register(&mut scsi, CONTROL), 0);
        assert_eq!(read_register(&mut scsi, TIMEOUT_PERIOD), 0);
        assert_eq!(read_register(&mut scsi, SCSI_STATUS), 1);
        assert!(scsi.take_reset_completion());
        assert!(!scsi.take_reset_completion());
    }

    #[test]
    fn debug_status_read_has_no_side_effects() {
        let mut scsi = Wd33c93b::new(20_000_000);
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[SCSI_STATUS])
            .unwrap();
        let mut value = [0xff];

        assert_eq!(
            scsi.debug_read(DeviceAddr::new(DATA_PORT), &mut value),
            Ok(())
        );
        assert_eq!(value, [0]);
        assert!(scsi.interrupt_asserted());
        assert_eq!(scsi.selected_register, SCSI_STATUS);
    }

    #[test]
    fn undefined_registers_read_as_all_ones() {
        let mut scsi = Wd33c93b::new(20_000_000);
        assert_eq!(read_register(&mut scsi, 0x1a), 0xff);
    }

    #[test]
    fn special_registers_do_not_auto_increment() {
        let mut scsi = Wd33c93b::new(20_000_000);

        for register in [COMMAND, 0x19, AUXILIARY_STATUS] {
            scsi.write(DeviceAddr::new(ADDRESS_PORT), &[register])
                .unwrap();
            scsi.write(DeviceAddr::new(DATA_PORT), &[1]).unwrap();
            assert_eq!(scsi.selected_register, register);
        }
    }

    #[test]
    fn selection_timeout_uses_the_supplied_clock_across_resets() {
        for (clock_hz, milliseconds) in [(20_000_000, 252), (10_000_000, 504)] {
            let mut scsi = Wd33c93b::new(clock_hz);
            for _ in 0..2 {
                write_register(&mut scsi, TIMEOUT_PERIOD, 63);
                let super::WdWork::SelectionWait(Some(duration)) = scsi.wait_for_selection() else {
                    panic!("nonzero timeout must produce a virtual duration");
                };
                assert_eq!(
                    duration.as_attoseconds(),
                    milliseconds * 1_000_000_000_000_000
                );
                scsi.reset();
                write_register(&mut scsi, OWN_ID, 0x88);
                write_register(&mut scsi, COMMAND, 0);
                assert!(matches!(
                    scsi.wait_for_selection(),
                    super::WdWork::SelectionWait(None)
                ));
            }
        }
    }

    #[test]
    #[should_panic]
    fn rejects_zero_input_clock() {
        let _ = Wd33c93b::new(0);
    }

    #[test]
    fn unsupported_commands_are_model_errors_without_register_side_effects() {
        let mut scsi = Wd33c93b::new(20_000_000);
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[COMMAND])
            .unwrap();
        for command in [5, 0x0a, 0x18, 0x21, 0xff] {
            assert_eq!(
                scsi.write(DeviceAddr::new(DATA_PORT), &[command]),
                Err(BusError::UnimplementedAccess)
            );
            assert_eq!(scsi.command, 0);
            assert_eq!(scsi.selected_register, COMMAND);
            assert_eq!(scsi.auxiliary_status(), INTERRUPT_PENDING);
            assert!(scsi.take_request().is_none());
        }
    }

    #[test]
    fn rejects_invalid_ports_and_widths_atomically() {
        let mut scsi = Wd33c93b::new(20_000_000);
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[TIMEOUT_PERIOD])
            .unwrap();

        assert_eq!(
            scsi.write(DeviceAddr::new(DATA_PORT), &[1, 2, 3]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            scsi.write(DeviceAddr::new(8), &[1]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(scsi.timeout_period, 0);
        assert_eq!(scsi.selected_register, TIMEOUT_PERIOD);
    }
}
