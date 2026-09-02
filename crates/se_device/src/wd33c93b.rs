//! Western Digital WD33C93B indirect-register and command model.

use se_core::bus::{BusFault, DeviceAddr};

const ADDRESS_PORT: u64 = 0;
const DATA_PORT: u64 = 4;

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
const SELECT_AND_TRANSFER: u8 = 0x08;
const SELECT_AND_TRANSFER_WITH_ATN: u8 = 0x09;
const RESET_COMPLETION_STATUS: u8 = 0x00;
const ADVANCED_RESET_COMPLETION_STATUS: u8 = 0x01;
const SELECT_AND_TRANSFER_COMPLETION_STATUS: u8 = 0x16;
const SELECTION_TIMEOUT_STATUS: u8 = 0x42;
const SELECT_AND_TRANSFER_PHASE: u8 = 0x60;
const ADVANCED_FEATURES: u8 = 1 << 3;
const SOURCE_ID_PRESERVED_BITS: u8 = 0x0f;
const TRANSFER_COUNT_MASK: u32 = 0x00ff_ffff;

/// A stable Select-And-Transfer request produced by the WD33C93B.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
pub struct Wd33c93b {
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
    pending_request: Option<SelectAndTransferRequest>,
    software_reset_completed: bool,
}

impl Wd33c93b {
    /// Creates a controller after hardware reset completion.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
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
    }

    /// Reads one byte from the indirect interface.
    ///
    /// Reading SCSI Status acknowledges the current interrupt and advances the
    /// selector to Command.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Address => data[0] = self.auxiliary_status(),
            Port::Data => {
                let register = self.selected_register;
                data[0] = self.read_selected_register(register);
                if register == SCSI_STATUS {
                    self.interrupt_pending = false;
                    self.selected_register = COMMAND;
                } else if selector_advances(register) {
                    self.selected_register = register.wrapping_add(1) & 0x1f;
                }
            }
        }
        Ok(())
    }

    /// Reads one byte without acknowledging interrupts or advancing the selector.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Address => data[0] = self.auxiliary_status(),
            Port::Data => data[0] = self.read_selected_register(self.selected_register),
        }
        Ok(())
    }

    /// Writes one byte to the indirect interface.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Address => self.selected_register = data[0] & 0x1f,
            Port::Data => {
                let register = self.selected_register;
                self.write_selected_register(register, data[0]);
                if selector_advances(register) {
                    self.selected_register = register.wrapping_add(1) & 0x1f;
                }
            }
        }
        Ok(())
    }

    /// Returns and clears one pending Select-And-Transfer request.
    pub fn take_select_and_transfer_request(&mut self) -> Option<SelectAndTransferRequest> {
        self.pending_request.take()
    }

    /// Subtracts bytes accepted by the initiator from the transfer residual.
    ///
    /// Returns `false` without modifying state when `byte_count` exceeds the
    /// residual.
    pub fn consume_transfer_bytes(&mut self, byte_count: u32) -> bool {
        if byte_count > self.transfer_count {
            return false;
        }
        self.transfer_count -= byte_count;
        true
    }

    /// Completes the active command with a target status byte.
    pub fn finish_select_and_transfer(&mut self, target_status: u8) {
        self.target_lun = target_status;
        self.command_phase = SELECT_AND_TRANSFER_PHASE;
        self.scsi_status = SELECT_AND_TRANSFER_COMPLETION_STATUS;
        self.command_in_progress = false;
        self.interrupt_pending = true;
        self.pending_request = None;
    }

    /// Completes selection without finding the requested target.
    pub fn finish_selection_timeout(&mut self) {
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
            value |= COMMAND_IN_PROGRESS | BUSY;
        }
        value
    }

    const fn read_selected_register(&self, register: u8) -> u8 {
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
            _ => {}
        }
        self.transfer_count &= TRANSFER_COUNT_MASK;
    }

    fn execute_command(&mut self, command: u8) {
        self.command = command;
        match command {
            SOFTWARE_RESET => self.software_reset(),
            SELECT_AND_TRANSFER | SELECT_AND_TRANSFER_WITH_ATN => {
                self.command_in_progress = true;
                self.interrupt_pending = false;
                self.software_reset_completed = false;
                self.pending_request = Some(SelectAndTransferRequest {
                    destination_id: self.destination_id & 0x07,
                    lun: self.target_lun & 0x07,
                    transfer_count: self.transfer_count,
                    cdb: self.cdb,
                    cdb_length: cdb_length(self.cdb[0]),
                });
            }
            _ => {}
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
    }
}

#[derive(Clone, Copy)]
enum Port {
    Address,
    Data,
}

fn decode_port(address: DeviceAddr, length: usize) -> Result<Port, BusFault> {
    if length != 1 {
        return Err(BusFault::UnsupportedAccess);
    }

    match address.get() {
        ADDRESS_PORT => Ok(Port::Address),
        DATA_PORT => Ok(Port::Data),
        _ => Err(BusFault::Unmapped),
    }
}

const fn selector_advances(register: u8) -> bool {
    !matches!(register, COMMAND | DATA | AUXILIARY_STATUS)
}

const fn cdb_length(opcode: u8) -> u8 {
    match opcode >> 5 {
        0 => 6,
        1 => 10,
        5 => 12,
        _ => 6,
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{
        ADDRESS_PORT, AUXILIARY_STATUS, BUSY, CDB_START, COMMAND, COMMAND_IN_PROGRESS, CONTROL,
        DATA_PORT, DESTINATION_ID, INTERRUPT_PENDING, OWN_ID, SCSI_STATUS, SELECT_AND_TRANSFER,
        SOURCE_ID, TARGET_LUN, TIMEOUT_PERIOD, TRANSFER_COUNT_MSB, Wd33c93b,
    };

    fn read_port(scsi: &mut Wd33c93b, port: u64) -> Result<u8, BusFault> {
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
        let mut scsi = Wd33c93b::new();

        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(INTERRUPT_PENDING));
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[0xe2]).unwrap();
        assert_eq!(scsi.selected_register, TIMEOUT_PERIOD);
    }

    #[test]
    fn select_and_transfer_latches_a_stable_request() {
        let mut scsi = Wd33c93b::new();
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
        let mut scsi = Wd33c93b::new();
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
    fn selection_timeout_retains_the_full_transfer_count() {
        let mut scsi = Wd33c93b::new();
        write_register(&mut scsi, TRANSFER_COUNT_MSB + 2, 8);
        write_register(&mut scsi, COMMAND, SELECT_AND_TRANSFER);
        scsi.finish_selection_timeout();

        assert_eq!(read_register(&mut scsi, TRANSFER_COUNT_MSB + 2), 8);
        assert_eq!(read_register(&mut scsi, SCSI_STATUS), 0x42);
    }

    #[test]
    fn software_and_hardware_reset_preserve_different_registers() {
        let mut scsi = Wd33c93b::new();
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
        let mut scsi = Wd33c93b::new();
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
        let mut scsi = Wd33c93b::new();
        assert_eq!(read_register(&mut scsi, 0x1a), 0xff);
    }

    #[test]
    fn special_registers_do_not_auto_increment() {
        let mut scsi = Wd33c93b::new();

        for register in [COMMAND, 0x19, AUXILIARY_STATUS] {
            scsi.write(DeviceAddr::new(ADDRESS_PORT), &[register])
                .unwrap();
            scsi.write(DeviceAddr::new(DATA_PORT), &[1]).unwrap();
            assert_eq!(scsi.selected_register, register);
        }
    }

    #[test]
    fn rejects_invalid_ports_and_widths_atomically() {
        let mut scsi = Wd33c93b::new();
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[TIMEOUT_PERIOD])
            .unwrap();

        assert_eq!(
            scsi.write(DeviceAddr::new(DATA_PORT), &[1, 2]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            scsi.write(DeviceAddr::new(2), &[1]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(scsi.timeout_period, 0);
        assert_eq!(scsi.selected_register, TIMEOUT_PERIOD);
    }
}
