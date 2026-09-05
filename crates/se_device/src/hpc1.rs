//! Silicon Graphics HPC1.5 register front end.

use se_core::bus::{BusError, DeviceAddr};
use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
use serde::{Deserialize, Serialize};

const ETHERNET_CURRENT_TRANSMIT_BUFFER_POINTER: u64 = 0x000c;
const ETHERNET_NEXT_TRANSMIT_DESCRIPTOR_POINTER: u64 = 0x0010;
const ETHERNET_TRANSMIT_BYTE_COUNT: u64 = 0x0014;
const ETHERNET_TRANSMIT_FIFO_POINTER: u64 = 0x0018;
const ETHERNET_TRANSMIT_FIFO: u64 = 0x001c;
const ETHERNET_CURRENT_TRANSMIT_DESCRIPTOR_POINTER: u64 = 0x0020;
const ETHERNET_CURRENT_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER: u64 = 0x0024;
const ETHERNET_PREVIOUS_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER: u64 = 0x0028;
const ETHERNET_TIMER: u64 = 0x002c;
const ETHERNET_TRANSMIT_STATUS: u64 = 0x0034;
const ETHERNET_RECEIVE_STATUS: u64 = 0x0038;
const ETHERNET_RESET: u64 = 0x003c;
const ETHERNET_RECEIVE_BYTE_COUNT: u64 = 0x0048;
const ETHERNET_CURRENT_RECEIVE_BUFFER_POINTER: u64 = 0x004c;
const ETHERNET_NEXT_RECEIVE_DESCRIPTOR_POINTER: u64 = 0x0050;
const ETHERNET_CURRENT_RECEIVE_DESCRIPTOR_POINTER: u64 = 0x0054;
const ETHERNET_RECEIVE_FIFO_POINTER: u64 = 0x0058;
const ETHERNET_RECEIVE_FIFO: u64 = 0x005c;

const SCSI_BYTE_COUNT: u64 = 0x0088;
const SCSI_CURRENT_BUFFER_POINTER: u64 = 0x008c;
const SCSI_NEXT_DESCRIPTOR_POINTER: u64 = 0x0090;
const SCSI_CONTROL: u64 = 0x0094;
const SCSI_FIFO_POINTER: u64 = 0x0098;
const SCSI_FIFO: u64 = 0x009c;

const PARALLEL_BYTE_COUNT: u64 = 0x00a8;
const PARALLEL_CURRENT_BUFFER_POINTER: u64 = 0x00ac;
const PARALLEL_NEXT_DESCRIPTOR_POINTER: u64 = 0x00b0;
const PARALLEL_CONTROL: u64 = 0x00b4;
const PARALLEL_FIFO_POINTER: u64 = 0x00b8;
const PARALLEL_FIFO: u64 = 0x00bc;

const INTERNAL_REGISTERS_END: u64 = 0x00c0;
const ENDIAN_CONTROL: u64 = 0x00c0;
const FREE_RUNNING_COUNTER: u64 = 0x0194;
const DSP_INTERRUPT_STATUS: u64 = 0x01a0;
const DSP_INTERRUPT_MASK: u64 = 0x01a4;
const MISCELLANEOUS_CONTROL: u64 = 0x01b0;
const REGISTER_BYTES: u64 = 4;

const REVISION: u8 = 0x40;
const WRITABLE_ENDIAN_BITS: u8 = 0x1f;

const ETHERNET_CURRENT_BUFFER_POINTER_MASK: u32 = 0x8fff_ffff;
const ETHERNET_DESCRIPTOR_POINTER_MASK: u32 = 0x0fff_ffff;
const ETHERNET_TRANSMIT_BYTE_COUNT_MASK: u32 = 0x8000_9fff;
const ETHERNET_RECEIVE_BYTE_COUNT_MASK: u32 = 0x0000_01ff;
const ETHERNET_RESET_CHANNEL: u32 = 0x01;
const ETHERNET_TRANSMIT_STATUS_BITS: u32 = 0x00ff_0000;
const ETHERNET_RECEIVE_STATUS_BITS: u32 = 0x0000_ff00;
const ETHERNET_CONTROL_BITS: u32 = 0x0f;
const ETHERNET_TIMER_COUNT_MASK: u32 = 0x00ff_fff0;
const ETHERNET_TIMER_EXPIRED: u32 = 0x0100_0000;
const ETHERNET_TIMER_COUNT_SHIFT: u32 = 4;

const SCSI_RESET: u8 = 0x01;
const SCSI_FLUSH: u8 = 0x02;
const SCSI_TO_MEMORY: u8 = 0x10;
const SCSI_START_DMA: u8 = 0x80;
const WRITABLE_SCSI_BITS: u8 = SCSI_RESET | SCSI_FLUSH | SCSI_TO_MEMORY | SCSI_START_DMA;
const SCSI_BYTE_COUNT_MASK: u16 = 0x1fff;
const SCSI_ADDRESS_MASK: u32 = 0x0fff_ffff;
const SCSI_CURRENT_BUFFER_POINTER_MASK: u32 = 0x8fff_ffff;
const SCSI_DESCRIPTOR_END: u32 = 1 << 31;

const PARALLEL_BYTE_COUNT_MASK: u32 = 0x0000_01ff;
const PARALLEL_CURRENT_BUFFER_POINTER_MASK: u32 = 0x8fff_ffff;
const PARALLEL_DESCRIPTOR_POINTER_MASK: u32 = 0x0fff_ffff;
const PARALLEL_RESET: u8 = 0x01;
const PARALLEL_CLEAR_INTERRUPT: u8 = 0x02;
const PARALLEL_TO_MEMORY: u8 = 0x10;
const WRITABLE_PARALLEL_BITS: u8 = 0xfd;

const DSP_INTERRUPT_BITS: u8 = 0x07;

const FREE_RUNNING_COUNTER_FREQUENCY: u128 = 33_000_000;
const FREE_RUNNING_COUNTER_MODULUS: u128 = 1 << 24;
const FIFO_ENTRIES: usize = 16;

/// One currently available HPC1 SCSI DMA window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScsiDmaWindow {
    buffer_address: u32,
    byte_count: u16,
    to_memory: bool,
}

impl ScsiDmaWindow {
    /// Returns the physical buffer address.
    #[must_use]
    pub const fn buffer_address(&self) -> u32 {
        self.buffer_address
    }

    /// Returns the number of bytes available in this descriptor.
    #[must_use]
    pub const fn byte_count(&self) -> u16 {
        self.byte_count
    }

    /// Reports whether bytes move from the SCSI target into memory.
    #[must_use]
    pub const fn to_memory(&self) -> bool {
        self.to_memory
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct DiagnosticFifo {
    data: [u32; FIFO_ENTRIES],
    flags: [u8; FIFO_ENTRIES],
    read_index: usize,
    write_index: usize,
}

impl DiagnosticFifo {
    const fn new() -> Self {
        Self {
            data: [0; FIFO_ENTRIES],
            flags: [0; FIFO_ENTRIES],
            read_index: 0,
            write_index: 0,
        }
    }

    fn reset_pointers(&mut self) {
        self.read_index = 0;
        self.write_index = 0;
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct EthernetChannel {
    current_transmit_buffer_pointer: u32,
    next_transmit_descriptor_pointer: u32,
    transmit_byte_count: u32,
    current_transmit_descriptor_pointer: u32,
    current_packet_first_transmit_descriptor_pointer: u32,
    previous_packet_first_transmit_descriptor_pointer: u32,
    timer_count: u32,
    timer_expired: bool,
    transmit_status: u32,
    receive_status: u32,
    control: u32,
    receive_byte_count: u32,
    current_receive_buffer_pointer: u32,
    next_receive_descriptor_pointer: u32,
    current_receive_descriptor_pointer: u32,
    transmit_fifo: DiagnosticFifo,
    transmit_fifo_write_flag: u8,
    receive_fifo: DiagnosticFifo,
    receive_fifo_write_direction: bool,
    reset_output_pending: bool,
}

impl EthernetChannel {
    const fn new() -> Self {
        Self {
            current_transmit_buffer_pointer: 0,
            next_transmit_descriptor_pointer: 0,
            transmit_byte_count: 0,
            current_transmit_descriptor_pointer: 0,
            current_packet_first_transmit_descriptor_pointer: 0,
            previous_packet_first_transmit_descriptor_pointer: 0,
            timer_count: 0,
            timer_expired: false,
            transmit_status: 0,
            receive_status: 0,
            control: 0,
            receive_byte_count: 0,
            current_receive_buffer_pointer: 0,
            next_receive_descriptor_pointer: 0,
            current_receive_descriptor_pointer: 0,
            transmit_fifo: DiagnosticFifo::new(),
            transmit_fifo_write_flag: 0,
            receive_fifo: DiagnosticFifo::new(),
            receive_fifo_write_direction: false,
            reset_output_pending: false,
        }
    }

    fn reset_data_path(&mut self) {
        let control = self.control;
        *self = Self::new();
        self.control = control;
    }

    const fn timer(&self) -> u32 {
        (self.timer_count << ETHERNET_TIMER_COUNT_SHIFT)
            | if self.timer_expired {
                ETHERNET_TIMER_EXPIRED
            } else {
                0
            }
    }

    fn write_timer(&mut self, value: u32) {
        self.timer_count = (value & ETHERNET_TIMER_COUNT_MASK) >> ETHERNET_TIMER_COUNT_SHIFT;
        self.timer_expired = value & ETHERNET_TIMER_EXPIRED != 0;
    }

    fn advance_timer(&mut self, ticks: u128) {
        if self.timer_expired || self.timer_count == 0 {
            return;
        }

        if ticks >= u128::from(self.timer_count) {
            self.timer_count = 0;
            self.timer_expired = true;
        } else {
            self.timer_count -= u32::try_from(ticks).expect("timer ticks fit the active count");
        }
    }

    fn transmit_fifo_pointer(&self) -> u32 {
        u32::from(self.transmit_fifo.flags[self.transmit_fifo.read_index]) << 24
            | (self.transmit_fifo.read_index as u32) << 10
            | (self.transmit_fifo.write_index as u32) << 2
    }

    fn write_transmit_fifo_pointer(&mut self, value: u32) {
        self.transmit_fifo.write_index = fifo_index(value, 2);
        self.transmit_fifo.read_index = fifo_index(value, 10);
        self.transmit_fifo_write_flag = (value >> 24) as u8;
    }

    fn receive_fifo_pointer(&self) -> u32 {
        if self.receive_fifo_write_direction {
            0x8000 | (self.receive_fifo.write_index as u32) << 10
        } else {
            (self.receive_fifo.read_index as u32) << 2
        }
    }

    fn write_receive_fifo_pointer(&mut self, value: u32) {
        self.receive_fifo_write_direction = value & 0x8000 != 0;
        if self.receive_fifo_write_direction {
            self.receive_fifo.write_index = fifo_index(value, 10);
        } else {
            self.receive_fifo.read_index = fifo_index(value, 2);
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct ScsiChannel {
    byte_count: u16,
    current_buffer_address: u32,
    next_descriptor_pointer: u32,
    descriptor_end: bool,
    descriptor_loaded: bool,
    descriptor_fetch_pending: bool,
    control: u8,
    fifo: DiagnosticFifo,
    reset_output_pending: bool,
}

impl ScsiChannel {
    const fn new() -> Self {
        Self {
            byte_count: 0,
            current_buffer_address: 0,
            next_descriptor_pointer: 0,
            descriptor_end: false,
            descriptor_loaded: false,
            descriptor_fetch_pending: false,
            control: 0,
            fifo: DiagnosticFifo::new(),
            reset_output_pending: false,
        }
    }

    fn reset_data_path(&mut self) {
        let control = self.control;
        *self = Self::new();
        self.control = control;
    }

    fn reset_fifo_pointers(&mut self) {
        self.fifo.reset_pointers();
    }

    const fn current_buffer_pointer(&self) -> u32 {
        self.current_buffer_address
            | if self.descriptor_end {
                SCSI_DESCRIPTOR_END
            } else {
                0
            }
    }

    fn write_current_buffer_pointer(&mut self, offset: usize, data: &[u8]) {
        let value = masked_write(
            self.current_buffer_pointer(),
            offset,
            data,
            SCSI_CURRENT_BUFFER_POINTER_MASK,
        );
        self.current_buffer_address = value & SCSI_ADDRESS_MASK;
        self.descriptor_end = value & SCSI_DESCRIPTOR_END != 0;
    }

    fn fifo_pointer(&self) -> u32 {
        u32::from(self.fifo.flags[self.fifo.read_index]) << 28 | (self.fifo.read_index as u32) << 2
    }

    fn write_fifo_pointer(&mut self, value: u32) {
        let index = fifo_index(value, 2);
        self.fifo.read_index = index;
        self.fifo.write_index = index;
        if self.control & SCSI_TO_MEMORY == 0 {
            self.fifo.flags[index] = ((value >> 28) & 0x0f) as u8;
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct DspChannel {
    interrupt_status: u8,
    interrupt_mask: u8,
}

impl DspChannel {
    const fn new() -> Self {
        Self {
            interrupt_status: 0,
            interrupt_mask: 0,
        }
    }

    const fn interrupt_asserted(&self) -> bool {
        self.interrupt_status & self.interrupt_mask != 0
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct ParallelChannel {
    byte_count: u32,
    current_buffer_pointer: u32,
    next_descriptor_pointer: u32,
    control: u8,
    interrupt_pending: bool,
    fifo: DiagnosticFifo,
}

impl ParallelChannel {
    const fn new() -> Self {
        Self {
            byte_count: 0,
            current_buffer_pointer: 0,
            next_descriptor_pointer: 0,
            control: 0,
            interrupt_pending: false,
            fifo: DiagnosticFifo::new(),
        }
    }

    fn reset_data_path(&mut self) {
        let control = self.control;
        let interrupt_pending = self.interrupt_pending;
        *self = Self::new();
        self.control = control;
        self.interrupt_pending = interrupt_pending;
    }

    const fn control_value(&self) -> u8 {
        self.control
            | if self.interrupt_pending {
                PARALLEL_CLEAR_INTERRUPT
            } else {
                0
            }
    }

    fn fifo_pointer(&self) -> u32 {
        u32::from(self.fifo.flags[self.fifo.read_index]) << 28 | (self.fifo.read_index as u32) << 2
    }

    fn write_fifo_pointer(&mut self, value: u32) {
        let index = fifo_index(value, 2);
        self.fifo.read_index = index;
        self.fifo.write_index = index;
        if self.control & PARALLEL_TO_MEMORY == 0 {
            self.fifo.flags[index] = ((value >> 28) & 0x0f) as u8;
        }
    }
}

/// The complete functional state of the IP12 HPC1.5 device.
#[derive(Clone, Deserialize, Serialize)]
pub struct Hpc1 {
    ethernet: EthernetChannel,
    scsi: ScsiChannel,
    parallel: ParallelChannel,
    dsp: DspChannel,
    endian_control: u8,
    free_running_counter: u32,
    miscellaneous_control: u32,
    clock_phase: u128,
}

impl Hpc1 {
    /// Creates an HPC1.5 in its reset state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            ethernet: EthernetChannel::new(),
            scsi: ScsiChannel::new(),
            parallel: ParallelChannel::new(),
            dsp: DspChannel::new(),
            endian_control: REVISION,
            free_running_counter: 0,
            miscellaneous_control: 0,
            clock_phase: 0,
        }
    }

    /// Restores the mutable HPC1.5 reset state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Reports the masked CPU-side DSP interrupt output level.
    #[must_use]
    pub const fn dsp_interrupt_asserted(&self) -> bool {
        self.dsp.interrupt_asserted()
    }

    /// Reports the HPC1 parallel-channel interrupt output level.
    #[must_use]
    pub const fn parallel_interrupt_asserted(&self) -> bool {
        self.parallel.interrupt_pending
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, [`BusError::HardwareFault`] outside the modeled
    /// register windows, or [`BusError::UnimplementedAccess`] for unsupported
    /// accesses within those windows.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(offset) = register_offset(start, end, ETHERNET_CURRENT_TRANSMIT_BUFFER_POINTER)
        {
            read_register(self.ethernet.current_transmit_buffer_pointer, offset, data);
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_NEXT_TRANSMIT_DESCRIPTOR_POINTER)
        {
            read_register(self.ethernet.next_transmit_descriptor_pointer, offset, data);
        } else if let Some(offset) = register_offset(start, end, ETHERNET_TRANSMIT_BYTE_COUNT) {
            read_register(self.ethernet.transmit_byte_count, offset, data);
        } else if is_word(start, end, ETHERNET_TRANSMIT_FIFO_POINTER) {
            data.copy_from_slice(&self.ethernet.transmit_fifo_pointer().to_be_bytes());
        } else if is_word(start, end, ETHERNET_TRANSMIT_FIFO) {
            data.copy_from_slice(
                &self.ethernet.transmit_fifo.data[self.ethernet.transmit_fifo.read_index]
                    .to_be_bytes(),
            );
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_CURRENT_TRANSMIT_DESCRIPTOR_POINTER)
        {
            read_register(
                self.ethernet.current_transmit_descriptor_pointer,
                offset,
                data,
            );
        } else if let Some(offset) = register_offset(
            start,
            end,
            ETHERNET_CURRENT_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER,
        ) {
            read_register(
                self.ethernet
                    .current_packet_first_transmit_descriptor_pointer,
                offset,
                data,
            );
        } else if let Some(offset) = register_offset(
            start,
            end,
            ETHERNET_PREVIOUS_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER,
        ) {
            read_register(
                self.ethernet
                    .previous_packet_first_transmit_descriptor_pointer,
                offset,
                data,
            );
        } else if is_word(start, end, ETHERNET_TIMER) {
            data.copy_from_slice(&self.ethernet.timer().to_be_bytes());
        } else if let Some(offset) = register_offset(start, end, ETHERNET_TRANSMIT_STATUS) {
            read_register(self.ethernet.transmit_status, offset, data);
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RECEIVE_STATUS) {
            read_register(self.ethernet.receive_status, offset, data);
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RESET) {
            read_register(self.ethernet.control, offset, data);
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RECEIVE_BYTE_COUNT) {
            read_register(self.ethernet.receive_byte_count, offset, data);
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_CURRENT_RECEIVE_BUFFER_POINTER)
        {
            read_register(self.ethernet.current_receive_buffer_pointer, offset, data);
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_NEXT_RECEIVE_DESCRIPTOR_POINTER)
        {
            read_register(self.ethernet.next_receive_descriptor_pointer, offset, data);
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_CURRENT_RECEIVE_DESCRIPTOR_POINTER)
        {
            read_register(
                self.ethernet.current_receive_descriptor_pointer,
                offset,
                data,
            );
        } else if is_word(start, end, ETHERNET_RECEIVE_FIFO_POINTER) {
            data.copy_from_slice(&self.ethernet.receive_fifo_pointer().to_be_bytes());
        } else if is_word(start, end, ETHERNET_RECEIVE_FIFO) {
            data.copy_from_slice(
                &self.ethernet.receive_fifo.data[self.ethernet.receive_fifo.read_index]
                    .to_be_bytes(),
            );
        } else if let Some(offset) = register_offset(start, end, SCSI_BYTE_COUNT) {
            read_register(u32::from(self.scsi.byte_count), offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_CURRENT_BUFFER_POINTER) {
            read_register(self.scsi.current_buffer_pointer(), offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_NEXT_DESCRIPTOR_POINTER) {
            read_register(self.scsi.next_descriptor_pointer, offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_CONTROL) {
            read_register(u32::from(self.scsi.control), offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_FIFO_POINTER) {
            read_register(self.scsi.fifo_pointer(), offset, data);
        } else if is_word(start, end, SCSI_FIFO) {
            data.copy_from_slice(&self.scsi.fifo.data[self.scsi.fifo.read_index].to_be_bytes());
        } else if let Some(offset) = register_offset(start, end, PARALLEL_BYTE_COUNT) {
            read_register(self.parallel.byte_count, offset, data);
        } else if let Some(offset) = register_offset(start, end, PARALLEL_CURRENT_BUFFER_POINTER) {
            read_register(self.parallel.current_buffer_pointer, offset, data);
        } else if let Some(offset) = register_offset(start, end, PARALLEL_NEXT_DESCRIPTOR_POINTER) {
            read_register(self.parallel.next_descriptor_pointer, offset, data);
        } else if let Some(offset) = register_offset(start, end, PARALLEL_CONTROL) {
            read_register(u32::from(self.parallel.control_value()), offset, data);
        } else if is_word(start, end, PARALLEL_FIFO_POINTER) {
            data.copy_from_slice(&self.parallel.fifo_pointer().to_be_bytes());
        } else if is_word(start, end, PARALLEL_FIFO) {
            data.copy_from_slice(
                &self.parallel.fifo.data[self.parallel.fifo.read_index].to_be_bytes(),
            );
        } else if special_register_overlap(start, end) {
            return Err(BusError::UnimplementedAccess);
        } else if internal_register_transaction(start, end) {
            data.fill(0);
        } else if let Some(offset) = register_offset(start, end, ENDIAN_CONTROL) {
            read_register(u32::from(self.endian_control), offset, data);
        } else if is_word(start, end, FREE_RUNNING_COUNTER) {
            data.copy_from_slice(&self.free_running_counter.to_be_bytes());
        } else if is_word(start, end, DSP_INTERRUPT_STATUS) {
            data.copy_from_slice(&u32::from(self.dsp.interrupt_status).to_be_bytes());
        } else if is_word(start, end, DSP_INTERRUPT_MASK) {
            data.copy_from_slice(&u32::from(self.dsp.interrupt_mask).to_be_bytes());
        } else if is_word(start, end, MISCELLANEOUS_CONTROL) {
            data.copy_from_slice(&self.miscellaneous_control.to_be_bytes());
        } else if overlaps_register(start, end, FREE_RUNNING_COUNTER)
            || overlaps_register(start, end, DSP_INTERRUPT_STATUS)
            || overlaps_register(start, end, DSP_INTERRUPT_MASK)
            || overlaps_register(start, end, MISCELLANEOUS_CONTROL)
        {
            return Err(BusError::UnimplementedAccess);
        } else {
            return Err(BusError::HardwareFault);
        }

        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, [`BusError::HardwareFault`] outside the modeled
    /// register windows, or [`BusError::UnimplementedAccess`] for unsupported
    /// accesses within those windows.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(offset) = register_offset(start, end, ETHERNET_CURRENT_TRANSMIT_BUFFER_POINTER)
        {
            self.ethernet.current_transmit_buffer_pointer = masked_write(
                self.ethernet.current_transmit_buffer_pointer,
                offset,
                data,
                ETHERNET_CURRENT_BUFFER_POINTER_MASK,
            );
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_NEXT_TRANSMIT_DESCRIPTOR_POINTER)
        {
            self.ethernet.next_transmit_descriptor_pointer = masked_write(
                self.ethernet.next_transmit_descriptor_pointer,
                offset,
                data,
                ETHERNET_DESCRIPTOR_POINTER_MASK,
            );
        } else if let Some(offset) = register_offset(start, end, ETHERNET_TRANSMIT_BYTE_COUNT) {
            self.ethernet.transmit_byte_count = masked_write(
                self.ethernet.transmit_byte_count,
                offset,
                data,
                ETHERNET_TRANSMIT_BYTE_COUNT_MASK,
            );
        } else if is_word(start, end, ETHERNET_TRANSMIT_FIFO_POINTER) {
            self.ethernet
                .write_transmit_fifo_pointer(u32::from_be_bytes(data.try_into().unwrap()));
        } else if is_word(start, end, ETHERNET_TRANSMIT_FIFO) {
            self.ethernet.transmit_fifo.data[self.ethernet.transmit_fifo.write_index] =
                u32::from_be_bytes(data.try_into().unwrap());
            self.ethernet.transmit_fifo.flags[self.ethernet.transmit_fifo.write_index] =
                self.ethernet.transmit_fifo_write_flag;
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_CURRENT_TRANSMIT_DESCRIPTOR_POINTER)
        {
            self.ethernet.current_transmit_descriptor_pointer = masked_write(
                self.ethernet.current_transmit_descriptor_pointer,
                offset,
                data,
                ETHERNET_DESCRIPTOR_POINTER_MASK,
            );
        } else if let Some(offset) = register_offset(
            start,
            end,
            ETHERNET_CURRENT_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER,
        ) {
            self.ethernet
                .current_packet_first_transmit_descriptor_pointer = masked_write(
                self.ethernet
                    .current_packet_first_transmit_descriptor_pointer,
                offset,
                data,
                ETHERNET_DESCRIPTOR_POINTER_MASK,
            );
        } else if let Some(offset) = register_offset(
            start,
            end,
            ETHERNET_PREVIOUS_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER,
        ) {
            self.ethernet
                .previous_packet_first_transmit_descriptor_pointer = masked_write(
                self.ethernet
                    .previous_packet_first_transmit_descriptor_pointer,
                offset,
                data,
                ETHERNET_DESCRIPTOR_POINTER_MASK,
            );
        } else if is_word(start, end, ETHERNET_TIMER) {
            self.ethernet
                .write_timer(u32::from_be_bytes(data.try_into().unwrap()));
        } else if let Some(offset) = register_offset(start, end, ETHERNET_TRANSMIT_STATUS) {
            self.ethernet.transmit_status = masked_write(
                self.ethernet.transmit_status,
                offset,
                data,
                ETHERNET_TRANSMIT_STATUS_BITS,
            );
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RECEIVE_STATUS) {
            self.ethernet.receive_status = masked_write(
                self.ethernet.receive_status,
                offset,
                data,
                ETHERNET_RECEIVE_STATUS_BITS,
            );
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RESET) {
            let old_control = self.ethernet.control;
            self.ethernet.control = masked_write(old_control, offset, data, ETHERNET_CONTROL_BITS);
            if old_control & ETHERNET_RESET_CHANNEL == 0
                && self.ethernet.control & ETHERNET_RESET_CHANNEL != 0
            {
                self.ethernet.reset_data_path();
                self.ethernet.reset_output_pending = true;
            }
        } else if let Some(offset) = register_offset(start, end, ETHERNET_RECEIVE_BYTE_COUNT) {
            self.ethernet.receive_byte_count = masked_write(
                self.ethernet.receive_byte_count,
                offset,
                data,
                ETHERNET_RECEIVE_BYTE_COUNT_MASK,
            );
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_CURRENT_RECEIVE_BUFFER_POINTER)
        {
            self.ethernet.current_receive_buffer_pointer = masked_write(
                self.ethernet.current_receive_buffer_pointer,
                offset,
                data,
                ETHERNET_CURRENT_BUFFER_POINTER_MASK,
            );
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_NEXT_RECEIVE_DESCRIPTOR_POINTER)
        {
            self.ethernet.next_receive_descriptor_pointer = masked_write(
                self.ethernet.next_receive_descriptor_pointer,
                offset,
                data,
                ETHERNET_DESCRIPTOR_POINTER_MASK,
            );
        } else if let Some(offset) =
            register_offset(start, end, ETHERNET_CURRENT_RECEIVE_DESCRIPTOR_POINTER)
        {
            self.ethernet.current_receive_descriptor_pointer = masked_write(
                self.ethernet.current_receive_descriptor_pointer,
                offset,
                data,
                ETHERNET_DESCRIPTOR_POINTER_MASK,
            );
        } else if is_word(start, end, ETHERNET_RECEIVE_FIFO_POINTER) {
            self.ethernet
                .write_receive_fifo_pointer(u32::from_be_bytes(data.try_into().unwrap()));
        } else if is_word(start, end, ETHERNET_RECEIVE_FIFO) {
            self.ethernet.receive_fifo.data[self.ethernet.receive_fifo.write_index] =
                u32::from_be_bytes(data.try_into().unwrap());
        } else if let Some(offset) = register_offset(start, end, SCSI_BYTE_COUNT) {
            let value = write_register(u32::from(self.scsi.byte_count), offset, data);
            self.scsi.byte_count = value as u16 & SCSI_BYTE_COUNT_MASK;
        } else if let Some(offset) = register_offset(start, end, SCSI_CURRENT_BUFFER_POINTER) {
            self.scsi.write_current_buffer_pointer(offset, data);
        } else if let Some(offset) = register_offset(start, end, SCSI_NEXT_DESCRIPTOR_POINTER) {
            self.scsi.next_descriptor_pointer = masked_write(
                self.scsi.next_descriptor_pointer,
                offset,
                data,
                SCSI_ADDRESS_MASK,
            );
            self.scsi.descriptor_loaded = false;
            self.scsi.descriptor_fetch_pending = self.scsi.control & SCSI_START_DMA != 0;
        } else if let Some(offset) = register_offset(start, end, SCSI_CONTROL) {
            let old_value = self.scsi.control;
            let value = write_register(u32::from(old_value), offset, data) as u8;
            self.scsi.control = value & WRITABLE_SCSI_BITS;
            let reset_rising = old_value & SCSI_RESET == 0 && self.scsi.control & SCSI_RESET != 0;
            if reset_rising {
                self.scsi.reset_data_path();
                self.scsi.reset_output_pending = true;
            }

            if self.scsi.control & SCSI_RESET != 0 {
                self.scsi.control &= !SCSI_START_DMA;
                self.scsi.descriptor_fetch_pending = false;
            } else if self.scsi.control & SCSI_START_DMA == 0 {
                self.scsi.descriptor_fetch_pending = false;
            } else {
                if old_value & SCSI_START_DMA == 0 {
                    self.scsi.reset_fifo_pointers();
                    self.scsi.descriptor_fetch_pending = !self.scsi.descriptor_loaded;
                }
                if self.scsi.control & SCSI_FLUSH != 0 {
                    self.scsi.control &= !SCSI_START_DMA;
                    self.scsi.descriptor_fetch_pending = false;
                }
            }
        } else if is_word(start, end, SCSI_FIFO_POINTER) {
            self.scsi
                .write_fifo_pointer(u32::from_be_bytes(data.try_into().unwrap()));
        } else if is_word(start, end, SCSI_FIFO) {
            self.scsi.fifo.data[self.scsi.fifo.write_index] =
                u32::from_be_bytes(data.try_into().unwrap());
        } else if let Some(offset) = register_offset(start, end, PARALLEL_BYTE_COUNT) {
            self.parallel.byte_count = masked_write(
                self.parallel.byte_count,
                offset,
                data,
                PARALLEL_BYTE_COUNT_MASK,
            );
        } else if let Some(offset) = register_offset(start, end, PARALLEL_CURRENT_BUFFER_POINTER) {
            self.parallel.current_buffer_pointer = masked_write(
                self.parallel.current_buffer_pointer,
                offset,
                data,
                PARALLEL_CURRENT_BUFFER_POINTER_MASK,
            );
        } else if let Some(offset) = register_offset(start, end, PARALLEL_NEXT_DESCRIPTOR_POINTER) {
            self.parallel.next_descriptor_pointer = masked_write(
                self.parallel.next_descriptor_pointer,
                offset,
                data,
                PARALLEL_DESCRIPTOR_POINTER_MASK,
            );
        } else if let Some(offset) = register_offset(start, end, PARALLEL_CONTROL) {
            let old_control = self.parallel.control;
            let value = write_register(u32::from(old_control), offset, data) as u8;
            self.parallel.control = value & WRITABLE_PARALLEL_BITS;
            if old_control & PARALLEL_RESET == 0 && self.parallel.control & PARALLEL_RESET != 0 {
                self.parallel.reset_data_path();
            }
            if writes_low_byte_bit(offset, data, PARALLEL_CLEAR_INTERRUPT) {
                self.parallel.interrupt_pending = false;
            }
        } else if is_word(start, end, PARALLEL_FIFO_POINTER) {
            self.parallel
                .write_fifo_pointer(u32::from_be_bytes(data.try_into().unwrap()));
        } else if is_word(start, end, PARALLEL_FIFO) {
            self.parallel.fifo.data[self.parallel.fifo.write_index] =
                u32::from_be_bytes(data.try_into().unwrap());
        } else if special_register_overlap(start, end) {
            return Err(BusError::UnimplementedAccess);
        } else if internal_register_transaction(start, end) {
        } else if let Some(offset) = register_offset(start, end, ENDIAN_CONTROL) {
            let value = write_register(u32::from(self.endian_control), offset, data) as u8;
            self.endian_control = REVISION | (value & WRITABLE_ENDIAN_BITS);
        } else if overlaps_register(start, end, FREE_RUNNING_COUNTER) {
            return Err(BusError::UnimplementedAccess);
        } else if is_word(start, end, DSP_INTERRUPT_STATUS) {
            self.dsp.interrupt_status &=
                u32::from_be_bytes(data.try_into().unwrap()) as u8 & DSP_INTERRUPT_BITS;
        } else if is_word(start, end, DSP_INTERRUPT_MASK) {
            self.dsp.interrupt_mask =
                u32::from_be_bytes(data.try_into().unwrap()) as u8 & DSP_INTERRUPT_BITS;
        } else if overlaps_register(start, end, DSP_INTERRUPT_STATUS)
            || overlaps_register(start, end, DSP_INTERRUPT_MASK)
        {
            return Err(BusError::UnimplementedAccess);
        } else if is_word(start, end, MISCELLANEOUS_CONTROL) {
            self.miscellaneous_control = u32::from_be_bytes(data.try_into().unwrap());
        } else if overlaps_register(start, end, MISCELLANEOUS_CONTROL) {
            return Err(BusError::UnimplementedAccess);
        } else {
            return Err(BusError::HardwareFault);
        }

        Ok(())
    }

    /// Advances the 33 MHz HPC1 clock domain by guest virtual time.
    pub fn advance_time(&mut self, elapsed: VirtualDuration) {
        let attoseconds = elapsed.as_attoseconds();
        let whole_seconds = attoseconds / ATTOSECONDS_PER_SECOND;
        let partial_attoseconds = attoseconds % ATTOSECONDS_PER_SECOND;
        let scaled_partial =
            partial_attoseconds * FREE_RUNNING_COUNTER_FREQUENCY + self.clock_phase;
        let partial_ticks = scaled_partial / ATTOSECONDS_PER_SECOND;
        self.clock_phase = scaled_partial % ATTOSECONDS_PER_SECOND;

        let whole_ticks = (whole_seconds % FREE_RUNNING_COUNTER_MODULUS)
            * (FREE_RUNNING_COUNTER_FREQUENCY % FREE_RUNNING_COUNTER_MODULUS);
        let wrapping_ticks = (whole_ticks + partial_ticks % FREE_RUNNING_COUNTER_MODULUS)
            % FREE_RUNNING_COUNTER_MODULUS;
        self.free_running_counter = ((u128::from(self.free_running_counter) + wrapping_ticks)
            % FREE_RUNNING_COUNTER_MODULUS) as u32;

        let timer_ticks = if whole_seconds == 0 {
            partial_ticks
        } else {
            u128::MAX
        };
        self.ethernet.advance_timer(timer_ticks);
    }

    /// Returns and clears a pending reset request for the attached Ethernet controller.
    pub fn take_ethernet_reset_request(&mut self) -> bool {
        let requested = self.ethernet.reset_output_pending;
        self.ethernet.reset_output_pending = false;
        requested
    }

    /// Returns and clears a pending reset request for the attached SCSI controller.
    pub fn take_scsi_reset_request(&mut self) -> bool {
        let requested = self.scsi.reset_output_pending;
        self.scsi.reset_output_pending = false;
        requested
    }

    /// Returns and clears a pending SCSI descriptor fetch request.
    pub fn take_scsi_descriptor_fetch(&mut self) -> Option<u32> {
        if !self.scsi.descriptor_fetch_pending {
            return None;
        }
        self.scsi.descriptor_fetch_pending = false;
        Some(self.scsi.next_descriptor_pointer)
    }

    /// Loads one big-endian 12-byte SCSI DMA descriptor.
    pub fn load_scsi_descriptor(&mut self, descriptor: [u8; 12]) {
        let count = u32::from_be_bytes(descriptor[0..4].try_into().expect("fixed descriptor word"));
        let buffer =
            u32::from_be_bytes(descriptor[4..8].try_into().expect("fixed descriptor word"));
        let next = u32::from_be_bytes(descriptor[8..12].try_into().expect("fixed descriptor word"));
        self.scsi.byte_count = count as u16 & SCSI_BYTE_COUNT_MASK;
        self.scsi.current_buffer_address = buffer & SCSI_ADDRESS_MASK;
        self.scsi.next_descriptor_pointer = next & SCSI_ADDRESS_MASK;
        self.scsi.descriptor_end = buffer & SCSI_DESCRIPTOR_END != 0;
        self.scsi.descriptor_loaded = true;
        self.scsi.descriptor_fetch_pending = false;
    }

    /// Returns the active SCSI DMA buffer window.
    #[must_use]
    pub const fn scsi_dma_window(&self) -> Option<ScsiDmaWindow> {
        if self.scsi.control & SCSI_START_DMA == 0
            || !self.scsi.descriptor_loaded
            || self.scsi.byte_count == 0
        {
            return None;
        }
        Some(ScsiDmaWindow {
            buffer_address: self.scsi.current_buffer_address,
            byte_count: self.scsi.byte_count,
            to_memory: self.scsi.control & SCSI_TO_MEMORY != 0,
        })
    }

    /// Advances the active DMA cursor after one bulk transfer.
    ///
    /// Returns `false` without modifying state when `byte_count` exceeds the
    /// current descriptor.
    pub fn consume_scsi_dma_bytes(&mut self, byte_count: u16) -> bool {
        if byte_count > self.scsi.byte_count {
            return false;
        }
        self.scsi.byte_count -= byte_count;
        self.scsi.current_buffer_address = self
            .scsi
            .current_buffer_address
            .wrapping_add(u32::from(byte_count))
            & SCSI_ADDRESS_MASK;
        if self.scsi.byte_count == 0 {
            self.scsi.descriptor_loaded = false;
            if self.scsi.descriptor_end {
                self.scsi.control &= !SCSI_START_DMA;
            } else {
                self.scsi.descriptor_fetch_pending = true;
            }
        }
        true
    }

    /// Finishes a short target transfer while preserving descriptor residuals.
    pub fn finish_scsi_dma(&mut self) {
        self.scsi.control &= !SCSI_START_DMA;
        self.scsi.descriptor_fetch_pending = false;
    }

    /// Stops SCSI DMA after a descriptor, address, or protocol failure.
    pub fn stop_scsi_dma(&mut self) {
        self.scsi.control &= !SCSI_START_DMA;
        self.scsi.descriptor_fetch_pending = false;
    }

    #[cfg(test)]
    fn set_dsp_interrupt_status_for_test(&mut self, status: u8) {
        self.dsp.interrupt_status = status & DSP_INTERRUPT_BITS;
    }

    #[cfg(test)]
    fn set_parallel_interrupt_pending_for_test(&mut self, pending: bool) {
        self.parallel.interrupt_pending = pending;
    }
}

fn transaction_bounds(address: DeviceAddr, length: usize) -> Result<(u64, u64), BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusError::InvalidTransaction)?;
    let end = start
        .checked_add(length)
        .ok_or(BusError::InvalidTransaction)?;
    Ok((start, end))
}

fn register_offset(start: u64, end: u64, register: u64) -> Option<usize> {
    if !contains_register(start, end, register) {
        return None;
    }

    usize::try_from(start - register).ok()
}

const fn contains_register(start: u64, end: u64, register: u64) -> bool {
    start >= register && end <= register + REGISTER_BYTES
}

const fn overlaps_register(start: u64, end: u64, register: u64) -> bool {
    start < register + REGISTER_BYTES && end > register
}

const fn is_word(start: u64, end: u64, register: u64) -> bool {
    start == register && end == register + REGISTER_BYTES
}

fn internal_register_transaction(start: u64, end: u64) -> bool {
    start < INTERNAL_REGISTERS_END
        && end <= INTERNAL_REGISTERS_END
        && start / REGISTER_BYTES == (end - 1) / REGISTER_BYTES
}

fn special_register_overlap(start: u64, end: u64) -> bool {
    [
        ETHERNET_TRANSMIT_FIFO_POINTER,
        ETHERNET_TRANSMIT_FIFO,
        ETHERNET_TIMER,
        ETHERNET_RECEIVE_FIFO_POINTER,
        ETHERNET_RECEIVE_FIFO,
        SCSI_FIFO_POINTER,
        SCSI_FIFO,
        PARALLEL_FIFO_POINTER,
        PARALLEL_FIFO,
    ]
    .into_iter()
    .any(|register| overlaps_register(start, end, register))
}

fn read_register(value: u32, offset: usize, data: &mut [u8]) {
    data.copy_from_slice(&value.to_be_bytes()[offset..offset + data.len()]);
}

fn write_register(value: u32, offset: usize, data: &[u8]) -> u32 {
    let mut bytes = value.to_be_bytes();
    bytes[offset..offset + data.len()].copy_from_slice(data);
    u32::from_be_bytes(bytes)
}

fn masked_write(value: u32, offset: usize, data: &[u8], mask: u32) -> u32 {
    write_register(value, offset, data) & mask
}

fn writes_low_byte_bit(offset: usize, data: &[u8], bit: u8) -> bool {
    const LOW_BYTE_OFFSET: usize = 3;

    offset <= LOW_BYTE_OFFSET
        && LOW_BYTE_OFFSET < offset + data.len()
        && data[LOW_BYTE_OFFSET - offset] & bit != 0
}

fn fifo_index(value: u32, shift: u32) -> usize {
    ((value >> shift) & (FIFO_ENTRIES as u32 - 1)) as usize
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use super::{
        DSP_INTERRUPT_MASK, DSP_INTERRUPT_STATUS, ENDIAN_CONTROL,
        ETHERNET_CURRENT_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER,
        ETHERNET_CURRENT_RECEIVE_BUFFER_POINTER, ETHERNET_CURRENT_RECEIVE_DESCRIPTOR_POINTER,
        ETHERNET_CURRENT_TRANSMIT_BUFFER_POINTER, ETHERNET_CURRENT_TRANSMIT_DESCRIPTOR_POINTER,
        ETHERNET_NEXT_RECEIVE_DESCRIPTOR_POINTER, ETHERNET_NEXT_TRANSMIT_DESCRIPTOR_POINTER,
        ETHERNET_PREVIOUS_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER, ETHERNET_RECEIVE_BYTE_COUNT,
        ETHERNET_RECEIVE_FIFO, ETHERNET_RECEIVE_FIFO_POINTER, ETHERNET_RESET, ETHERNET_TIMER,
        ETHERNET_TIMER_COUNT_MASK, ETHERNET_TIMER_COUNT_SHIFT, ETHERNET_TIMER_EXPIRED,
        ETHERNET_TRANSMIT_BYTE_COUNT, ETHERNET_TRANSMIT_FIFO, ETHERNET_TRANSMIT_FIFO_POINTER,
        FIFO_ENTRIES, FREE_RUNNING_COUNTER, FREE_RUNNING_COUNTER_FREQUENCY,
        FREE_RUNNING_COUNTER_MODULUS, Hpc1, MISCELLANEOUS_CONTROL, PARALLEL_BYTE_COUNT,
        PARALLEL_CONTROL, PARALLEL_CURRENT_BUFFER_POINTER, PARALLEL_FIFO, PARALLEL_FIFO_POINTER,
        PARALLEL_NEXT_DESCRIPTOR_POINTER, SCSI_BYTE_COUNT, SCSI_CONTROL,
        SCSI_CURRENT_BUFFER_POINTER, SCSI_DESCRIPTOR_END, SCSI_FIFO, SCSI_FIFO_POINTER, SCSI_FLUSH,
        SCSI_NEXT_DESCRIPTOR_POINTER, SCSI_START_DMA, SCSI_TO_MEMORY,
    };

    fn read_word(hpc1: &Hpc1, address: u64) -> Result<u32, BusError> {
        let mut bytes = [0; 4];
        hpc1.read(DeviceAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn write_word(hpc1: &mut Hpc1, address: u64, value: u32) {
        hpc1.write(DeviceAddr::new(address), &value.to_be_bytes())
            .unwrap();
    }

    #[test]
    fn reset_values_match_the_ip12_front_end() {
        let hpc1 = Hpc1::new();

        assert_eq!(read_word(&hpc1, 0), Ok(0));
        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0));
        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x40));
        assert_eq!(read_word(&hpc1, FREE_RUNNING_COUNTER), Ok(0));
        assert_eq!(read_word(&hpc1, MISCELLANEOUS_CONTROL), Ok(0));
    }

    #[test]
    fn free_running_counter_accumulates_exact_fractional_ticks_and_wraps() {
        let mut hpc1 = Hpc1::new();
        let almost_one_tick = ATTOSECONDS_PER_SECOND / FREE_RUNNING_COUNTER_FREQUENCY;

        hpc1.advance_time(VirtualDuration::from_attoseconds(almost_one_tick));
        assert_eq!(read_word(&hpc1, FREE_RUNNING_COUNTER), Ok(0));
        hpc1.advance_time(VirtualDuration::from_attoseconds(1));
        assert_eq!(read_word(&hpc1, FREE_RUNNING_COUNTER), Ok(1));

        hpc1.reset();
        hpc1.advance_time(VirtualDuration::from_attoseconds(
            2 * ATTOSECONDS_PER_SECOND,
        ));
        assert_eq!(
            read_word(&hpc1, FREE_RUNNING_COUNTER),
            Ok((2 * FREE_RUNNING_COUNTER_FREQUENCY % FREE_RUNNING_COUNTER_MODULUS) as u32)
        );
    }

    #[test]
    fn free_running_counter_is_a_read_only_word() {
        let mut hpc1 = Hpc1::new();

        assert_eq!(
            hpc1.read(DeviceAddr::new(FREE_RUNNING_COUNTER + 3), &mut [0]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            hpc1.write(DeviceAddr::new(FREE_RUNNING_COUNTER), &0_u32.to_be_bytes()),
            Err(BusError::UnimplementedAccess)
        );
    }

    #[test]
    fn dsp_interrupt_status_is_write_zero_to_clear_and_masked_to_three_bits() {
        let mut hpc1 = Hpc1::new();
        hpc1.set_dsp_interrupt_status_for_test(u8::MAX);

        write_word(&mut hpc1, DSP_INTERRUPT_MASK, u32::MAX);
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_STATUS), Ok(7));
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_MASK), Ok(7));
        assert!(hpc1.dsp_interrupt_asserted());

        write_word(&mut hpc1, DSP_INTERRUPT_STATUS, 0xffff_fffd);
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_STATUS), Ok(5));
        write_word(&mut hpc1, DSP_INTERRUPT_STATUS, 0xffff_fffb);
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_STATUS), Ok(1));

        write_word(&mut hpc1, DSP_INTERRUPT_MASK, 2);
        assert!(!hpc1.dsp_interrupt_asserted());
        write_word(&mut hpc1, DSP_INTERRUPT_STATUS, 0);
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_STATUS), Ok(0));
    }

    #[test]
    fn dsp_interrupt_registers_require_complete_aligned_words() {
        let mut hpc1 = Hpc1::new();

        for address in [DSP_INTERRUPT_STATUS, DSP_INTERRUPT_MASK] {
            assert_eq!(
                hpc1.read(DeviceAddr::new(address + 3), &mut [0]),
                Err(BusError::UnimplementedAccess)
            );
            assert_eq!(
                hpc1.write(DeviceAddr::new(address), &[0; 2]),
                Err(BusError::UnimplementedAccess)
            );
        }
        assert_eq!(
            hpc1.write(DeviceAddr::new(DSP_INTERRUPT_STATUS + 3), &[0; 2]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_STATUS), Ok(0));
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_MASK), Ok(0));
    }

    #[test]
    fn parallel_control_keeps_pending_separate_and_clears_it_with_one() {
        let mut hpc1 = Hpc1::new();
        hpc1.set_parallel_interrupt_pending_for_test(true);

        assert_eq!(read_word(&hpc1, PARALLEL_CONTROL), Ok(2));
        assert!(hpc1.parallel_interrupt_asserted());
        write_word(&mut hpc1, PARALLEL_CONTROL, 0x10);
        assert_eq!(read_word(&hpc1, PARALLEL_CONTROL), Ok(0x12));
        assert!(hpc1.parallel_interrupt_asserted());

        hpc1.write(DeviceAddr::new(PARALLEL_CONTROL), &[0xff])
            .unwrap();
        assert_eq!(read_word(&hpc1, PARALLEL_CONTROL), Ok(0x12));
        assert!(hpc1.parallel_interrupt_asserted());

        hpc1.write(DeviceAddr::new(PARALLEL_CONTROL + 3), &[0x12])
            .unwrap();
        assert_eq!(read_word(&hpc1, PARALLEL_CONTROL), Ok(0x10));
        assert!(!hpc1.parallel_interrupt_asserted());
    }

    #[test]
    fn ide_register_masks_are_independent() {
        let registers = [
            (ETHERNET_CURRENT_TRANSMIT_BUFFER_POINTER, 0x8fff_ffff),
            (ETHERNET_NEXT_TRANSMIT_DESCRIPTOR_POINTER, 0x0fff_ffff),
            (ETHERNET_TRANSMIT_BYTE_COUNT, 0x8000_9fff),
            (ETHERNET_CURRENT_TRANSMIT_DESCRIPTOR_POINTER, 0x0fff_ffff),
            (
                ETHERNET_CURRENT_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER,
                0x0fff_ffff,
            ),
            (
                ETHERNET_PREVIOUS_PACKET_FIRST_TRANSMIT_DESCRIPTOR_POINTER,
                0x0fff_ffff,
            ),
            (ETHERNET_RECEIVE_BYTE_COUNT, 0x0000_01ff),
            (ETHERNET_CURRENT_RECEIVE_BUFFER_POINTER, 0x8fff_ffff),
            (ETHERNET_NEXT_RECEIVE_DESCRIPTOR_POINTER, 0x0fff_ffff),
            (ETHERNET_CURRENT_RECEIVE_DESCRIPTOR_POINTER, 0x0fff_ffff),
            (PARALLEL_BYTE_COUNT, 0x0000_01ff),
            (PARALLEL_CURRENT_BUFFER_POINTER, 0x8fff_ffff),
            (PARALLEL_NEXT_DESCRIPTOR_POINTER, 0x0fff_ffff),
        ];

        for (address, mask) in registers {
            let mut hpc1 = Hpc1::new();
            write_word(&mut hpc1, address, u32::MAX);
            assert_eq!(read_word(&hpc1, address), Ok(mask));
        }
    }

    #[test]
    fn ethernet_timer_uses_documented_count_and_expiration_fields() {
        let mut hpc1 = Hpc1::new();
        write_word(&mut hpc1, ETHERNET_TIMER, 0x00ff_ffff);
        assert_eq!(
            read_word(&hpc1, ETHERNET_TIMER),
            Ok(ETHERNET_TIMER_COUNT_MASK)
        );

        hpc1.advance_time(VirtualDuration::from_attoseconds(
            ATTOSECONDS_PER_SECOND / FREE_RUNNING_COUNTER_FREQUENCY + 1,
        ));
        assert_eq!(
            read_word(&hpc1, ETHERNET_TIMER),
            Ok(ETHERNET_TIMER_COUNT_MASK - (1 << ETHERNET_TIMER_COUNT_SHIFT))
        );
        hpc1.advance_time(VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND));
        assert_eq!(read_word(&hpc1, ETHERNET_TIMER), Ok(ETHERNET_TIMER_EXPIRED));

        write_word(&mut hpc1, ETHERNET_TIMER, 100 << ETHERNET_TIMER_COUNT_SHIFT);
        assert_eq!(
            read_word(&hpc1, ETHERNET_TIMER),
            Ok(100 << ETHERNET_TIMER_COUNT_SHIFT)
        );
        write_word(
            &mut hpc1,
            ETHERNET_TIMER,
            ETHERNET_TIMER_EXPIRED | 100 << ETHERNET_TIMER_COUNT_SHIFT,
        );
        hpc1.advance_time(VirtualDuration::from_attoseconds(
            ATTOSECONDS_PER_SECOND / FREE_RUNNING_COUNTER_FREQUENCY + 1,
        ));
        assert_eq!(
            read_word(&hpc1, ETHERNET_TIMER),
            Ok(ETHERNET_TIMER_EXPIRED | 100 << ETHERNET_TIMER_COUNT_SHIFT)
        );
    }

    #[test]
    fn ethernet_transmit_fifo_preserves_data_and_eight_bit_flags() {
        let mut hpc1 = Hpc1::new();
        let patterns = [
            0xaaaa_aaaa,
            !0xaaaa_aaaa,
            0xcccc_cccc,
            !0xcccc_cccc,
            0xf0f0_f0f0,
            !0xf0f0_f0f0,
            0xff00_ff00,
            !0xff00_ff00,
            0xffff_0000,
            !0xffff_0000,
            u32::MAX,
            0,
        ];

        for index in 0..FIFO_ENTRIES as u32 {
            let value = patterns[index as usize % patterns.len()];
            write_word(
                &mut hpc1,
                ETHERNET_TRANSMIT_FIFO_POINTER,
                (value & 0xff) << 24 | index << 10 | index << 2,
            );
            write_word(&mut hpc1, ETHERNET_TRANSMIT_FIFO, value);
        }

        for index in 0..FIFO_ENTRIES as u32 {
            let expected = patterns[index as usize % patterns.len()];
            write_word(
                &mut hpc1,
                ETHERNET_TRANSMIT_FIFO_POINTER,
                index << 10 | index << 2,
            );
            assert_eq!(read_word(&hpc1, ETHERNET_TRANSMIT_FIFO), Ok(expected));
            assert_eq!(
                read_word(&hpc1, ETHERNET_TRANSMIT_FIFO_POINTER).unwrap() >> 24,
                expected & 0xff
            );
        }
    }

    #[test]
    fn ethernet_receive_fifo_uses_direction_specific_indices() {
        let mut hpc1 = Hpc1::new();

        for index in 0..FIFO_ENTRIES as u32 {
            write_word(
                &mut hpc1,
                ETHERNET_RECEIVE_FIFO_POINTER,
                0x8000 | index << 10,
            );
            write_word(&mut hpc1, ETHERNET_RECEIVE_FIFO, 0xabcd_0000 | index);
        }
        for index in 0..FIFO_ENTRIES as u32 {
            write_word(&mut hpc1, ETHERNET_RECEIVE_FIFO_POINTER, index << 2);
            assert_eq!(
                read_word(&hpc1, ETHERNET_RECEIVE_FIFO),
                Ok(0xabcd_0000 | index)
            );
        }
    }

    #[test]
    fn scsi_and_parallel_fifos_preserve_data_and_four_bit_flags() {
        let cases = [
            (SCSI_CONTROL, SCSI_FIFO_POINTER, SCSI_FIFO),
            (PARALLEL_CONTROL, PARALLEL_FIFO_POINTER, PARALLEL_FIFO),
        ];

        for (control, pointer, fifo) in cases {
            let mut hpc1 = Hpc1::new();
            for index in 0..FIFO_ENTRIES as u32 {
                write_word(&mut hpc1, control, 0);
                write_word(&mut hpc1, pointer, (index & 0x0f) << 28 | index << 2);
                write_word(&mut hpc1, fifo, 0x5678_0000 | index);
            }
            for index in 0..FIFO_ENTRIES as u32 {
                write_word(&mut hpc1, control, 0x10);
                write_word(&mut hpc1, pointer, index << 2);
                assert_eq!(read_word(&hpc1, fifo), Ok(0x5678_0000 | index));
                assert_eq!(read_word(&hpc1, pointer).unwrap() >> 28, index);
            }
        }
    }

    #[test]
    fn ethernet_reset_is_edge_triggered_and_clears_only_its_channel() {
        let mut hpc1 = Hpc1::new();
        write_word(&mut hpc1, ETHERNET_TIMER, 9 << ETHERNET_TIMER_COUNT_SHIFT);
        write_word(&mut hpc1, PARALLEL_BYTE_COUNT, 7);

        write_word(&mut hpc1, ETHERNET_RESET, 1);
        assert!(hpc1.take_ethernet_reset_request());
        assert!(!hpc1.take_ethernet_reset_request());
        assert_eq!(read_word(&hpc1, ETHERNET_TIMER), Ok(0));
        assert_eq!(read_word(&hpc1, PARALLEL_BYTE_COUNT), Ok(7));
        assert_eq!(read_word(&hpc1, ETHERNET_RESET), Ok(1));

        write_word(&mut hpc1, ETHERNET_RESET, 1);
        assert!(!hpc1.take_ethernet_reset_request());
        write_word(&mut hpc1, ETHERNET_RESET, 0);
        write_word(&mut hpc1, ETHERNET_RESET, 1);
        assert!(hpc1.take_ethernet_reset_request());
    }

    #[test]
    fn endian_and_scsi_controls_use_big_endian_lanes_masks_and_reset_priority() {
        let mut hpc1 = Hpc1::new();

        hpc1.write(DeviceAddr::new(ENDIAN_CONTROL), &[0xff; 4])
            .unwrap();
        hpc1.write(DeviceAddr::new(SCSI_CONTROL), &[0xff; 4])
            .unwrap();

        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x5f));
        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0x13));
    }

    #[test]
    fn scsi_reset_requests_are_edge_triggered() {
        let mut hpc1 = Hpc1::new();

        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[1]).unwrap();
        assert!(hpc1.take_scsi_reset_request());
        assert!(!hpc1.take_scsi_reset_request());
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[1]).unwrap();
        assert!(!hpc1.take_scsi_reset_request());
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0]).unwrap();
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[1]).unwrap();
        assert!(hpc1.take_scsi_reset_request());
    }

    #[test]
    fn scsi_reset_clears_the_internal_data_path() {
        let mut hpc1 = Hpc1::new();
        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&32_u32.to_be_bytes());
        descriptor[4..8].copy_from_slice(&0x8010_0000_u32.to_be_bytes());
        descriptor[8..12].copy_from_slice(&0x0012_3400_u32.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);
        write_word(&mut hpc1, SCSI_CONTROL, u32::from(SCSI_START_DMA));
        write_word(&mut hpc1, SCSI_FIFO_POINTER, 5 << 2);
        write_word(&mut hpc1, SCSI_FIFO, 0x1234_5678);

        write_word(&mut hpc1, SCSI_CONTROL, 1);

        assert!(hpc1.take_scsi_reset_request());
        assert!(!hpc1.take_scsi_reset_request());
        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(1));
        assert_eq!(read_word(&hpc1, SCSI_BYTE_COUNT), Ok(0));
        assert_eq!(read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER), Ok(0));
        assert_eq!(read_word(&hpc1, SCSI_NEXT_DESCRIPTOR_POINTER), Ok(0));
        assert_eq!(read_word(&hpc1, SCSI_FIFO_POINTER), Ok(0));
        assert_eq!(read_word(&hpc1, SCSI_FIFO), Ok(0));
        assert!(hpc1.scsi_dma_window().is_none());
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), None);
    }

    #[test]
    fn scsi_start_resets_fifo_pointers_only_on_a_rising_edge() {
        let mut hpc1 = Hpc1::new();
        write_word(&mut hpc1, SCSI_FIFO_POINTER, 5 << 2);

        write_word(&mut hpc1, SCSI_CONTROL, u32::from(SCSI_START_DMA));
        assert_eq!(read_word(&hpc1, SCSI_FIFO_POINTER), Ok(0));

        write_word(&mut hpc1, SCSI_FIFO_POINTER, 7 << 2);
        write_word(&mut hpc1, SCSI_CONTROL, u32::from(SCSI_START_DMA));
        assert_eq!(read_word(&hpc1, SCSI_FIFO_POINTER), Ok(7 << 2));
    }

    #[test]
    fn scsi_descriptor_fetch_requires_an_active_dma_channel() {
        let mut hpc1 = Hpc1::new();
        write_word(&mut hpc1, SCSI_NEXT_DESCRIPTOR_POINTER, 0x0012_3400);
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), None);

        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x90])
            .unwrap();
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), Some(0x0012_3400));
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), None);
    }

    #[test]
    fn sash_reads_the_empty_scsi_channel_fifo_pointer_as_a_halfword() {
        let hpc1 = Hpc1::new();
        let mut channel_pointer = [0xff; 2];

        hpc1.read(DeviceAddr::new(SCSI_FIFO_POINTER), &mut channel_pointer)
            .unwrap();

        assert_eq!(channel_pointer, [0; 2]);
    }

    #[test]
    fn scsi_descriptor_walk_updates_the_bulk_dma_cursor() {
        let mut hpc1 = Hpc1::new();
        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&0x0000_0200_u32.to_be_bytes());
        descriptor[4..8].copy_from_slice(&0x0010_0000_u32.to_be_bytes());
        descriptor[8..12].copy_from_slice(&0x0012_3500_u32.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x90])
            .unwrap();

        let window = hpc1.scsi_dma_window().unwrap();
        assert_eq!(window.buffer_address(), 0x0010_0000);
        assert_eq!(window.byte_count(), 512);
        assert!(window.to_memory());
        assert!(hpc1.consume_scsi_dma_bytes(512));
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), Some(0x0012_3500));
    }

    #[test]
    fn end_of_chain_and_short_completion_clear_start_without_hiding_residuals() {
        let mut hpc1 = Hpc1::new();
        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&32_u32.to_be_bytes());
        descriptor[4..8].copy_from_slice(&0x8010_0000_u32.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x90])
            .unwrap();
        assert!(hpc1.consume_scsi_dma_bytes(8));
        hpc1.finish_scsi_dma();

        assert_eq!(read_word(&hpc1, SCSI_BYTE_COUNT), Ok(24));
        assert_eq!(
            read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER),
            Ok(0x8010_0008)
        );
        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0x10));

        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x90])
            .unwrap();
        assert!(hpc1.consume_scsi_dma_bytes(24));
        assert!(hpc1.scsi_dma_window().is_none());
        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0x10));
    }

    #[test]
    fn scsi_flush_stops_dma_without_hiding_descriptor_residuals() {
        let mut hpc1 = Hpc1::new();
        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&32_u32.to_be_bytes());
        descriptor[4..8].copy_from_slice(&0x8010_0000_u32.to_be_bytes());
        descriptor[8..12].copy_from_slice(&0x0012_3400_u32.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);
        write_word(
            &mut hpc1,
            SCSI_CONTROL,
            u32::from(SCSI_TO_MEMORY | SCSI_START_DMA),
        );
        assert!(hpc1.consume_scsi_dma_bytes(8));

        write_word(
            &mut hpc1,
            SCSI_CONTROL,
            u32::from(SCSI_TO_MEMORY | SCSI_FLUSH | SCSI_START_DMA),
        );

        assert_eq!(
            read_word(&hpc1, SCSI_CONTROL),
            Ok(u32::from(SCSI_TO_MEMORY | SCSI_FLUSH))
        );
        assert_eq!(read_word(&hpc1, SCSI_BYTE_COUNT), Ok(24));
        assert_eq!(
            read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER),
            Ok(0x8010_0008)
        );
        assert!(hpc1.scsi_dma_window().is_none());
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), None);

        write_word(
            &mut hpc1,
            SCSI_CONTROL,
            u32::from(SCSI_TO_MEMORY | SCSI_FLUSH | SCSI_START_DMA),
        );
        assert_eq!(
            read_word(&hpc1, SCSI_CONTROL),
            Ok(u32::from(SCSI_TO_MEMORY | SCSI_FLUSH))
        );
        write_word(&mut hpc1, SCSI_CONTROL, u32::from(SCSI_TO_MEMORY));
        write_word(
            &mut hpc1,
            SCSI_CONTROL,
            u32::from(SCSI_TO_MEMORY | SCSI_START_DMA),
        );
        assert!(hpc1.scsi_dma_window().is_some());
    }

    #[test]
    fn scsi_current_buffer_pointer_keeps_end_out_of_dma_addresses() {
        let mut hpc1 = Hpc1::new();
        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&2_u32.to_be_bytes());
        descriptor[4..8].copy_from_slice(&0x8010_0000_u32.to_be_bytes());
        descriptor[8..12].copy_from_slice(&0x0012_3400_u32.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);
        write_word(
            &mut hpc1,
            SCSI_CONTROL,
            u32::from(SCSI_TO_MEMORY | SCSI_START_DMA),
        );

        assert_eq!(
            read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER),
            Ok(SCSI_DESCRIPTOR_END | 0x0010_0000)
        );
        assert_eq!(
            hpc1.scsi_dma_window().unwrap().buffer_address(),
            0x0010_0000
        );
        assert!(hpc1.consume_scsi_dma_bytes(1));
        assert_eq!(
            read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER),
            Ok(SCSI_DESCRIPTOR_END | 0x0010_0001)
        );

        hpc1.write(DeviceAddr::new(SCSI_CURRENT_BUFFER_POINTER), &[0])
            .unwrap();
        assert_eq!(
            read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER),
            Ok(0x0010_0001)
        );
        assert!(hpc1.consume_scsi_dma_bytes(1));
        assert_eq!(hpc1.take_scsi_descriptor_fetch(), Some(0x0012_3400));
    }

    #[test]
    fn descriptor_fields_apply_operational_hardware_masks() {
        let mut hpc1 = Hpc1::new();
        let mut descriptor = [0; 12];
        descriptor[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        descriptor[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        descriptor[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        hpc1.load_scsi_descriptor(descriptor);

        assert_eq!(read_word(&hpc1, SCSI_BYTE_COUNT), Ok(0x1fff));
        assert_eq!(
            read_word(&hpc1, SCSI_CURRENT_BUFFER_POINTER),
            Ok(0x8fff_ffff)
        );
        assert_eq!(
            read_word(&hpc1, SCSI_NEXT_DESCRIPTOR_POINTER),
            Ok(0x0fff_ffff)
        );
    }

    #[test]
    fn reserved_internal_registers_respond_without_state() {
        let mut hpc1 = Hpc1::new();

        for address in [0, 4, 8, 0x40, 0x44, 0x60, 0x80, 0x84, 0xa0, 0xa4] {
            assert_eq!(read_word(&hpc1, address), Ok(0));
            write_word(&mut hpc1, address, u32::MAX);
            assert_eq!(read_word(&hpc1, address), Ok(0));
        }
    }

    #[test]
    fn miscellaneous_control_requires_a_complete_word() {
        let mut hpc1 = Hpc1::new();

        write_word(&mut hpc1, MISCELLANEOUS_CONTROL, 9);
        assert_eq!(read_word(&hpc1, MISCELLANEOUS_CONTROL), Ok(9));
        assert_eq!(
            hpc1.write(DeviceAddr::new(MISCELLANEOUS_CONTROL + 3), &[0]),
            Err(BusError::UnimplementedAccess)
        );
    }

    #[test]
    fn reset_clears_mutable_state_and_pending_outputs() {
        let mut hpc1 = Hpc1::new();
        write_word(&mut hpc1, ETHERNET_TIMER, 9 << ETHERNET_TIMER_COUNT_SHIFT);
        write_word(&mut hpc1, ETHERNET_RESET, 1);
        hpc1.write(DeviceAddr::new(SCSI_CONTROL + 3), &[0x93])
            .unwrap();
        write_word(&mut hpc1, SCSI_NEXT_DESCRIPTOR_POINTER, 0x0012_3400);
        hpc1.write(DeviceAddr::new(ENDIAN_CONTROL + 3), &[0x1f])
            .unwrap();
        hpc1.set_dsp_interrupt_status_for_test(7);
        write_word(&mut hpc1, DSP_INTERRUPT_MASK, 7);
        hpc1.set_parallel_interrupt_pending_for_test(true);
        write_word(&mut hpc1, MISCELLANEOUS_CONTROL, 9);
        hpc1.advance_time(VirtualDuration::from_attoseconds(
            ATTOSECONDS_PER_SECOND / FREE_RUNNING_COUNTER_FREQUENCY,
        ));

        hpc1.reset();

        assert_eq!(read_word(&hpc1, SCSI_CONTROL), Ok(0));
        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x40));
        assert_eq!(read_word(&hpc1, FREE_RUNNING_COUNTER), Ok(0));
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_STATUS), Ok(0));
        assert_eq!(read_word(&hpc1, DSP_INTERRUPT_MASK), Ok(0));
        assert!(!hpc1.dsp_interrupt_asserted());
        assert!(!hpc1.parallel_interrupt_asserted());
        assert_eq!(read_word(&hpc1, MISCELLANEOUS_CONTROL), Ok(0));
        assert!(!hpc1.take_ethernet_reset_request());
        assert!(!hpc1.take_scsi_reset_request());
        assert!(hpc1.take_scsi_descriptor_fetch().is_none());
    }

    #[test]
    fn rejects_invalid_unmapped_and_crossing_transactions_atomically() {
        let mut hpc1 = Hpc1::new();
        hpc1.write(DeviceAddr::new(ENDIAN_CONTROL + 3), &[0x12])
            .unwrap();

        assert_eq!(
            hpc1.write(DeviceAddr::new(ENDIAN_CONTROL), &[]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            hpc1.write(DeviceAddr::new(ETHERNET_TIMER + 3), &[0xaa, 0xbb]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            hpc1.read(DeviceAddr::new(0x0100), &mut [0]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(read_word(&hpc1, ENDIAN_CONTROL), Ok(0x52));
    }
}
