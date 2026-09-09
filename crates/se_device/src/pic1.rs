//! Silicon Graphics PIC1 reset, GIO configuration, and graphics-DMA register front end.

use se_core::bus::{BusError, DeviceAddr, PhysAddr};
use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
use serde::{Deserialize, Serialize};

const CPU_CONTROL: u64 = 0x0000;
const RESET_CONFIGURATION: u64 = 0x0004;
const SYSTEM_ID: u64 = 0x0008;
const MEMORY_CONFIGURATION_0: u64 = 0x1_0000;
const MEMORY_CONFIGURATION_1: u64 = 0x1_0004;
const PARITY_ERROR: u64 = 0x1_0200;
const CPU_ERROR_ADDRESS: u64 = 0x1_0204;
const GIO_ERROR_ADDRESS: u64 = 0x1_0208;
const CLEAR_ERROR: u64 = 0x1_0210;
const GIO_SLOT_CONFIGURATION_0: u64 = 0x2_0000;
const GIO_SLOT_CONFIGURATION_1: u64 = 0x2_0004;
const GIO_BURST: u64 = 0x2_0008;
const GIO_DELAY: u64 = 0x2_000c;
const THREE_WAY_MASK: u64 = 0x8_0008;
const THREE_WAY_SUBSTITUTION: u64 = 0x8_000c;
const DESCRIPTOR_ARRAY_BASE: u64 = 0xa_0000;
const GRAPHICS_BUFFER_ADDRESS: u64 = 0xa_0004;
const GRAPHICS_BUFFER_LENGTH: u64 = 0xa_0008;
const GRAPHICS_DESTINATION_ADDRESS: u64 = 0xa_000c;
const GRAPHICS_STRIDE: u64 = 0xa_0010;
const GRAPHICS_START: u64 = 0xa_0100;

const REGISTER_BYTES: u64 = 4;
const SYSTEM_INITIALIZE: u16 = 1 << 9;
const GRAPHICS_DMA_INTERRUPT_ENABLE: u16 = 1 << 4;
const GRAPHICS_DMA_SYNC_ENABLE: u16 = 1 << 5;
const GRAPHICS_DMA_ERROR: u16 = 1 << 2;
const DMA_IDLE: u16 = 1 << 3;
const FLOATING_POINT_ABSENT: u16 = 1;
const MEMORY_DESCRIPTOR_MASK: u16 = 0x0f3f;
const GIO_SLOT_CONFIGURATION_MASK: u8 = 0x03;
const THREE_WAY_VALUE_MASK: u32 = 0x1fff_ffff;
const DESCRIPTOR_ADDRESS_MASK: u32 = 0x0fff_ffff;

const GRAPHICS_DMA_CLOCK_HZ: u128 = 33_000_000;
const GRAPHICS_DMA_CYCLE_ATTOSECONDS: u128 = ATTOSECONDS_PER_SECOND.div_ceil(GRAPHICS_DMA_CLOCK_HZ);
const GRAPHICS_DMA_DESCRIPTOR_CYCLES: u128 = 5;
const GRAPHICS_DMA_DESCRIPTOR_BYTES: u32 = 20;
const GRAPHICS_DMA_PAGE_BYTES: u32 = 4096;

const GRAPHICS_DMA_STRIDE_INCREMENT: u32 = 1 << 31;
const GRAPHICS_DMA_FULL_PAGE: u32 = 1 << 30;
const GRAPHICS_DMA_GRAPHICS_STRIDE_SHIFT: u32 = 16;
const GRAPHICS_DMA_LAST_DESCRIPTOR: u32 = 1 << 15;
const GRAPHICS_DMA_LINE_INCREMENT: u32 = 1 << 14;
const GRAPHICS_DMA_MODE_SHIFT: u32 = 12;
const GRAPHICS_DMA_MODE_MASK: u32 = 0x03;
const GRAPHICS_DMA_WIDTH_MASK: u32 = 0x0fff;
const GRAPHICS_DMA_STRIDE_MASK: u32 = 0x0fff;

/// Direction of one PIC1 graphics DMA line.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GraphicsDmaDirection {
    /// Transfer bytes from local memory to a GIO device.
    MemoryToGio,
    /// Transfer bytes from a GIO device to local memory.
    GioToMemory,
}

/// One bounded board operation requested by the PIC1 graphics DMA channel.
///
/// PIC1 owns descriptor interpretation and channel progress. The containing
/// machine supplies local-memory and GIO transactions, then reports their
/// result through the matching completion method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicsDmaRequest {
    /// Fetch one five-word descriptor from local memory.
    ReadDescriptor {
        /// Physical descriptor address.
        address: u32,
    },
    /// Transfer one complete descriptor line between local memory and GIO.
    TransferLine {
        /// Transfer direction.
        direction: GraphicsDmaDirection,
        /// Physical local-memory address of the first byte.
        memory_address: u32,
        /// Physical GIO address asserted for the burst.
        gio_address: u32,
        /// Number of bytes transferred as complete 32-bit GIO words.
        ///
        /// This is the descriptor width rounded up to a word boundary. A
        /// zero-width line remains an empty transfer.
        length: usize,
    },
}

#[derive(Clone, Copy, Deserialize, Serialize)]
enum GraphicsDmaStage {
    Idle,
    WaitingForSync,
    Descriptor,
    Line,
}

/// Graphics-channel registers and active descriptor progress.
#[derive(Clone, Deserialize, Serialize)]
struct GraphicsDmaChannel {
    buffer_address: u32,
    buffer_length: u32,
    destination_address: u32,
    stride: u32,
    stage: GraphicsDmaStage,
    delay_attoseconds: u128,
    elapsed_credit_attoseconds: u128,
    descriptor_address: u32,
    line_memory_address: u32,
    line_gio_address: u32,
    line_width: u16,
    line_transfer_bytes: u16,
    memory_line_increment: u32,
    gio_line_increment: u32,
    lines_remaining: u32,
    next_descriptor_address: u32,
    last_descriptor: bool,
    direction: GraphicsDmaDirection,
    error: bool,
}

impl GraphicsDmaChannel {
    const fn new() -> Self {
        Self {
            buffer_address: 0,
            buffer_length: 0,
            destination_address: 0,
            stride: 0,
            stage: GraphicsDmaStage::Idle,
            delay_attoseconds: 0,
            elapsed_credit_attoseconds: 0,
            descriptor_address: 0,
            line_memory_address: 0,
            line_gio_address: 0,
            line_width: 0,
            line_transfer_bytes: 0,
            memory_line_increment: 0,
            gio_line_increment: 0,
            lines_remaining: 0,
            next_descriptor_address: 0,
            last_descriptor: false,
            direction: GraphicsDmaDirection::MemoryToGio,
            error: false,
        }
    }

    const fn idle(&self) -> bool {
        matches!(self.stage, GraphicsDmaStage::Idle)
    }

    fn start(&mut self, descriptor_address: u32, sync_enabled: bool, sync_asserted: bool) {
        if !self.idle() {
            return;
        }
        self.error = false;
        self.elapsed_credit_attoseconds = 0;
        self.descriptor_address = descriptor_address;
        if sync_enabled && !sync_asserted {
            self.stage = GraphicsDmaStage::WaitingForSync;
            self.delay_attoseconds = 0;
        } else {
            self.arm_descriptor_fetch();
        }
    }

    fn release_sync_wait(&mut self, sync_enabled: bool, sync_asserted: bool) {
        if matches!(self.stage, GraphicsDmaStage::WaitingForSync)
            && (!sync_enabled || sync_asserted)
        {
            self.arm_descriptor_fetch();
        }
    }

    fn arm_descriptor_fetch(&mut self) {
        self.stage = GraphicsDmaStage::Descriptor;
        self.set_delay(GRAPHICS_DMA_CYCLE_ATTOSECONDS * GRAPHICS_DMA_DESCRIPTOR_CYCLES);
    }

    const fn time_until_event(&self) -> Option<VirtualDuration> {
        if matches!(
            self.stage,
            GraphicsDmaStage::Descriptor | GraphicsDmaStage::Line
        ) {
            Some(VirtualDuration::from_attoseconds(self.delay_attoseconds))
        } else {
            None
        }
    }

    fn advance_time(&mut self, elapsed: VirtualDuration) {
        if !matches!(
            self.stage,
            GraphicsDmaStage::Descriptor | GraphicsDmaStage::Line
        ) {
            return;
        }
        let elapsed = elapsed.as_attoseconds();
        if elapsed >= self.delay_attoseconds {
            self.elapsed_credit_attoseconds = self
                .elapsed_credit_attoseconds
                .saturating_add(elapsed - self.delay_attoseconds);
            self.delay_attoseconds = 0;
        } else {
            self.delay_attoseconds -= elapsed;
        }
    }

    fn next_request(&mut self) -> Option<GraphicsDmaRequest> {
        if self.delay_attoseconds != 0 {
            return None;
        }
        match self.stage {
            GraphicsDmaStage::Descriptor => {
                let page_offset = self.descriptor_address & (GRAPHICS_DMA_PAGE_BYTES - 1);
                if !self.descriptor_address.is_multiple_of(4)
                    || page_offset > GRAPHICS_DMA_PAGE_BYTES - GRAPHICS_DMA_DESCRIPTOR_BYTES
                {
                    self.finish(true);
                    return None;
                }
                Some(GraphicsDmaRequest::ReadDescriptor {
                    address: self.descriptor_address,
                })
            }
            GraphicsDmaStage::Line => {
                debug_assert_eq!(
                    u32::from(self.line_transfer_bytes),
                    gio_transfer_bytes(u32::from(self.line_width))
                );
                Some(GraphicsDmaRequest::TransferLine {
                    direction: self.direction,
                    memory_address: self.line_memory_address,
                    gio_address: self.line_gio_address,
                    length: usize::from(self.line_transfer_bytes),
                })
            }
            GraphicsDmaStage::Idle | GraphicsDmaStage::WaitingForSync => None,
        }
    }

    fn complete_descriptor(&mut self, descriptor: Option<[u8; 20]>) {
        debug_assert!(matches!(self.stage, GraphicsDmaStage::Descriptor));
        debug_assert_eq!(self.delay_attoseconds, 0);
        let Some(descriptor) = descriptor else {
            self.finish(true);
            return;
        };
        let words = std::array::from_fn(|index| {
            let start = index * 4;
            u32::from_be_bytes(
                descriptor[start..start + 4]
                    .try_into()
                    .expect("graphics descriptor words have a fixed width"),
            )
        });
        let [
            buffer_address,
            buffer_length,
            destination_address,
            stride,
            next_descriptor,
        ] = words;

        self.buffer_address = buffer_address;
        self.buffer_length = buffer_length;
        self.destination_address = destination_address;
        self.stride = stride;

        self.direction = match (buffer_length >> GRAPHICS_DMA_MODE_SHIFT) & GRAPHICS_DMA_MODE_MASK {
            0 => GraphicsDmaDirection::MemoryToGio,
            2 => GraphicsDmaDirection::GioToMemory,
            // Accumulation and the remaining undefined mode terminate with
            // DMAERR. This deterministic policy is not a claim about PIC1
            // silicon behavior for those unverified modes.
            _ => {
                self.finish(true);
                return;
            }
        };

        let line_width = if buffer_length & GRAPHICS_DMA_FULL_PAGE != 0 {
            GRAPHICS_DMA_PAGE_BYTES
        } else {
            buffer_length & GRAPHICS_DMA_WIDTH_MASK
        };
        let line_transfer_bytes = gio_transfer_bytes(line_width);
        let host_stride = sign_extend_12(stride >> GRAPHICS_DMA_GRAPHICS_STRIDE_SHIFT);
        let graphics_stride = sign_extend_12(buffer_length >> GRAPHICS_DMA_GRAPHICS_STRIDE_SHIFT);
        let memory_line_increment = (line_width as i32).wrapping_add(host_stride) as u32;
        let gio_line_increment = (if buffer_length & GRAPHICS_DMA_LINE_INCREMENT != 0 {
            line_width as i32
        } else {
            0
        })
        .wrapping_add(if buffer_length & GRAPHICS_DMA_STRIDE_INCREMENT != 0 {
            graphics_stride
        } else {
            0
        }) as u32;

        self.line_memory_address = buffer_address;
        self.line_gio_address = destination_address;
        self.line_width = line_width as u16;
        self.line_transfer_bytes = line_transfer_bytes as u16;
        self.memory_line_increment = memory_line_increment;
        self.gio_line_increment = gio_line_increment;
        self.lines_remaining = u32::from(stride as u16) + 1;
        self.next_descriptor_address = next_descriptor & DESCRIPTOR_ADDRESS_MASK;
        self.last_descriptor = buffer_length & GRAPHICS_DMA_LAST_DESCRIPTOR != 0;
        self.stage = GraphicsDmaStage::Line;
        self.set_delay(line_delay_attoseconds(line_transfer_bytes));
    }

    fn complete_line(&mut self, success: bool) {
        debug_assert!(matches!(self.stage, GraphicsDmaStage::Line));
        debug_assert_eq!(self.delay_attoseconds, 0);
        if !success {
            self.finish(true);
            return;
        }

        if self.lines_remaining > 1 {
            self.lines_remaining -= 1;
            self.line_memory_address = self
                .line_memory_address
                .wrapping_add(self.memory_line_increment);
            self.line_gio_address = self.line_gio_address.wrapping_add(self.gio_line_increment);
            self.set_delay(line_delay_attoseconds(u32::from(self.line_transfer_bytes)));
        } else if self.last_descriptor {
            self.finish(false);
        } else {
            self.descriptor_address = self.next_descriptor_address;
            self.arm_descriptor_fetch();
        }
    }

    fn finish(&mut self, error: bool) {
        self.stage = GraphicsDmaStage::Idle;
        self.delay_attoseconds = 0;
        self.error = error;
        self.elapsed_credit_attoseconds = 0;
    }

    /// Applies elapsed time left over after a scheduler call crossed a request
    /// boundary. This makes DMA progress independent of how the caller
    /// fragments an otherwise identical virtual-time interval.
    fn set_delay(&mut self, delay_attoseconds: u128) {
        let consumed = delay_attoseconds.min(self.elapsed_credit_attoseconds);
        self.delay_attoseconds = delay_attoseconds - consumed;
        self.elapsed_credit_attoseconds -= consumed;
    }
}

const fn gio_transfer_bytes(line_width: u32) -> u32 {
    line_width.div_ceil(4) * 4
}

const fn line_delay_attoseconds(transfer_bytes: u32) -> u128 {
    let words = transfer_bytes / 4;
    let words = if words == 0 { 1 } else { words };
    GRAPHICS_DMA_CYCLE_ATTOSECONDS * words as u128
}

const fn sign_extend_12(value: u32) -> i32 {
    ((value & GRAPHICS_DMA_STRIDE_MASK) << 20) as i32 >> 20
}

/// The software-visible PIC1 state needed by the IP12 reset path.
#[derive(Clone, Deserialize, Serialize)]
pub struct Pic1 {
    reset_configuration: u8,
    revision: u8,
    floating_point_present: bool,
    cpu_control: u16,
    memory_descriptors: [u16; 4],
    parity_error: u8,
    cpu_error_address: u32,
    gio_error_address: u32,
    address_error_pending: bool,
    gio_slot_configurations: [u8; 2],
    gio_burst: u8,
    gio_delay: u8,
    three_way_mask: u32,
    three_way_substitution: u32,
    descriptor_array_base: u32,
    graphics_dma: GraphicsDmaChannel,
    graphics_dma_sync_input: bool,
    system_reset_requested: bool,
}

impl Pic1 {
    /// Creates a PIC1 with fixed board reset inputs.
    ///
    /// # Panics
    ///
    /// Panics when `revision` does not fit the three-bit SYSID revision field.
    #[must_use]
    pub const fn new(reset_configuration: u8, revision: u8, floating_point_present: bool) -> Self {
        assert!(revision <= 7, "PIC1 revision must fit in three bits");

        Self {
            reset_configuration,
            revision,
            floating_point_present,
            cpu_control: 0,
            memory_descriptors: [0; 4],
            parity_error: 0,
            cpu_error_address: 0,
            gio_error_address: 0,
            address_error_pending: false,
            gio_slot_configurations: [0; 2],
            gio_burst: 0,
            gio_delay: 0,
            three_way_mask: 0,
            three_way_substitution: 0,
            descriptor_array_base: 0,
            graphics_dma: GraphicsDmaChannel::new(),
            graphics_dma_sync_input: false,
            system_reset_requested: false,
        }
    }

    /// Restores the mutable PIC1 reset state.
    pub fn reset(&mut self) {
        self.cpu_control = 0;
        self.memory_descriptors = [0; 4];
        self.parity_error = 0;
        self.cpu_error_address = 0;
        self.gio_error_address = 0;
        self.address_error_pending = false;
        self.gio_slot_configurations = [0; 2];
        self.gio_burst = 0;
        self.gio_delay = 0;
        self.three_way_mask = 0;
        self.three_way_substitution = 0;
        self.descriptor_array_base = 0;
        self.graphics_dma = GraphicsDmaChannel::new();
        self.graphics_dma_sync_input = false;
        self.system_reset_requested = false;
    }

    /// Reads one fixed-width device-local transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, [`BusError::HardwareFault`] for transactions crossing
    /// ordinary register boundaries, or [`BusError::UnimplementedAccess`] when
    /// the register, width, or direction is not implemented.
    pub fn read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(index) = memory_configuration_index(start, end)? {
            let value = u32::from(self.memory_descriptors[index]) << 16
                | u32::from(self.memory_descriptors[index + 1]);
            data.copy_from_slice(&value.to_be_bytes());
            return Ok(());
        }

        if let Some(offset) = register_offset(start, end, CPU_CONTROL) {
            read_register(u32::from(self.cpu_control), offset, data);
        } else if let Some(offset) = register_offset(start, end, RESET_CONFIGURATION) {
            read_register(u32::from(self.reset_configuration), offset, data);
        } else if let Some(offset) = register_offset(start, end, SYSTEM_ID) {
            read_register(u32::from(self.system_id()), offset, data);
        } else if let Some(offset) = register_offset(start, end, PARITY_ERROR) {
            read_register(u32::from(self.parity_error), offset, data);
        } else if let Some(offset) = register_offset(start, end, CPU_ERROR_ADDRESS) {
            read_register(self.cpu_error_address, offset, data);
        } else if let Some(offset) = register_offset(start, end, GIO_ERROR_ADDRESS) {
            read_register(self.gio_error_address, offset, data);
        } else if register_offset(start, end, CLEAR_ERROR).is_some() {
            return Err(BusError::UnimplementedAccess);
        } else if let Some(offset) = register_offset(start, end, GIO_SLOT_CONFIGURATION_0) {
            read_register(u32::from(self.gio_slot_configurations[0]), offset, data);
        } else if let Some(offset) = register_offset(start, end, GIO_SLOT_CONFIGURATION_1) {
            read_register(u32::from(self.gio_slot_configurations[1]), offset, data);
        } else if let Some(offset) = register_offset(start, end, GIO_BURST) {
            read_register(u32::from(self.gio_burst), offset, data);
        } else if let Some(offset) = register_offset(start, end, GIO_DELAY) {
            read_register(u32::from(self.gio_delay), offset, data);
        } else if let Some(offset) = register_offset(start, end, THREE_WAY_MASK) {
            read_register(self.three_way_mask, offset, data);
        } else if let Some(offset) = register_offset(start, end, THREE_WAY_SUBSTITUTION) {
            read_register(self.three_way_substitution, offset, data);
        } else if let Some(offset) = register_offset(start, end, DESCRIPTOR_ARRAY_BASE) {
            read_register(self.descriptor_array_base, offset, data);
        } else if let Some(offset) = register_offset(start, end, GRAPHICS_BUFFER_ADDRESS) {
            read_register(self.graphics_dma.buffer_address, offset, data);
        } else if let Some(offset) = register_offset(start, end, GRAPHICS_BUFFER_LENGTH) {
            read_register(self.graphics_dma.buffer_length, offset, data);
        } else if let Some(offset) = register_offset(start, end, GRAPHICS_DESTINATION_ADDRESS) {
            read_register(self.graphics_dma.destination_address, offset, data);
        } else if let Some(offset) = register_offset(start, end, GRAPHICS_STRIDE) {
            read_register(self.graphics_dma.stride, offset, data);
        } else if register_offset(start, end, GRAPHICS_START).is_some() {
            return Err(BusError::UnimplementedAccess);
        } else if start / REGISTER_BYTES != (end - 1) / REGISTER_BYTES {
            return Err(BusError::HardwareFault);
        } else {
            return Err(BusError::UnimplementedAccess);
        }

        Ok(())
    }

    /// Writes one fixed-width device-local transaction.
    ///
    /// Writes to PARERR are accepted without changing state. Only CLRERR
    /// clears the latched errors. PARERR write completion is a compatibility
    /// assumption that has not been verified against hardware.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid length or
    /// address overflow, [`BusError::HardwareFault`] for transactions crossing
    /// ordinary register boundaries, or [`BusError::UnimplementedAccess`] when
    /// the register, width, or direction is not implemented.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
        let (start, end) = transaction_bounds(address, data.len())?;

        if let Some(index) = memory_configuration_index(start, end)? {
            let value =
                u32::from_be_bytes(data.try_into().map_err(|_| BusError::InvalidTransaction)?);
            self.memory_descriptors[index] = (value >> 16) as u16 & MEMORY_DESCRIPTOR_MASK;
            self.memory_descriptors[index + 1] = value as u16 & MEMORY_DESCRIPTOR_MASK;
            return Ok(());
        }

        if let Some(offset) = register_offset(start, end, CPU_CONTROL) {
            let value = write_register(u32::from(self.cpu_control), offset, data) as u16;
            if value & SYSTEM_INITIALIZE != 0 {
                self.system_reset_requested = true;
            }
            self.cpu_control = value & !SYSTEM_INITIALIZE;
            self.graphics_dma.release_sync_wait(
                self.cpu_control & GRAPHICS_DMA_SYNC_ENABLE != 0,
                self.graphics_dma_sync_input,
            );
        } else if register_offset(start, end, RESET_CONFIGURATION).is_some()
            || register_offset(start, end, SYSTEM_ID).is_some()
            || register_offset(start, end, CPU_ERROR_ADDRESS).is_some()
            || register_offset(start, end, GIO_ERROR_ADDRESS).is_some()
        {
            return Err(BusError::UnimplementedAccess);
        } else if register_offset(start, end, PARITY_ERROR).is_some() {
            return Ok(());
        } else if register_offset(start, end, CLEAR_ERROR).is_some() {
            self.parity_error = 0;
            self.address_error_pending = false;
        } else if let Some(offset) = register_offset(start, end, GIO_SLOT_CONFIGURATION_0) {
            self.gio_slot_configurations[0] =
                write_register(u32::from(self.gio_slot_configurations[0]), offset, data) as u8
                    & GIO_SLOT_CONFIGURATION_MASK;
        } else if let Some(offset) = register_offset(start, end, GIO_SLOT_CONFIGURATION_1) {
            self.gio_slot_configurations[1] =
                write_register(u32::from(self.gio_slot_configurations[1]), offset, data) as u8
                    & GIO_SLOT_CONFIGURATION_MASK;
        } else if let Some(offset) = register_offset(start, end, GIO_BURST) {
            self.gio_burst = write_register(u32::from(self.gio_burst), offset, data) as u8;
        } else if let Some(offset) = register_offset(start, end, GIO_DELAY) {
            self.gio_delay = write_register(u32::from(self.gio_delay), offset, data) as u8;
        } else if let Some(offset) = register_offset(start, end, THREE_WAY_MASK) {
            self.three_way_mask =
                write_register(self.three_way_mask, offset, data) & THREE_WAY_VALUE_MASK;
        } else if let Some(offset) = register_offset(start, end, THREE_WAY_SUBSTITUTION) {
            self.three_way_substitution =
                write_register(self.three_way_substitution, offset, data) & THREE_WAY_VALUE_MASK;
        } else if let Some(offset) = register_offset(start, end, DESCRIPTOR_ARRAY_BASE) {
            self.descriptor_array_base =
                write_register(self.descriptor_array_base, offset, data) & DESCRIPTOR_ADDRESS_MASK;
        } else if register_offset(start, end, GRAPHICS_BUFFER_ADDRESS).is_some()
            || register_offset(start, end, GRAPHICS_BUFFER_LENGTH).is_some()
            || register_offset(start, end, GRAPHICS_DESTINATION_ADDRESS).is_some()
            || register_offset(start, end, GRAPHICS_STRIDE).is_some()
        {
            return Err(BusError::UnimplementedAccess);
        } else if register_offset(start, end, GRAPHICS_START).is_some() {
            self.graphics_dma.start(
                self.descriptor_array_base,
                self.cpu_control & GRAPHICS_DMA_SYNC_ENABLE != 0,
                self.graphics_dma_sync_input,
            );
        } else if start / REGISTER_BYTES != (end - 1) / REGISTER_BYTES {
            return Err(BusError::HardwareFault);
        } else {
            return Err(BusError::UnimplementedAccess);
        }

        Ok(())
    }

    /// Decodes one physical transaction through the memory configuration.
    ///
    /// Returns the matching descriptor index and descriptor-relative byte
    /// address. Installed storage is a property of the containing machine and
    /// is not considered by this method.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] when `byte_len` is zero or
    /// the address range overflows.
    pub fn decode_memory(
        &self,
        address: PhysAddr,
        byte_len: usize,
    ) -> Result<Option<(usize, DeviceAddr)>, BusError> {
        if byte_len == 0 {
            return Err(BusError::InvalidTransaction);
        }

        let start = address.get();
        let length = u64::try_from(byte_len).map_err(|_| BusError::InvalidTransaction)?;
        let end = start
            .checked_add(length)
            .ok_or(BusError::InvalidTransaction)?;

        for (index, descriptor) in self.memory_descriptors.iter().copied().enumerate() {
            let Some(size) = memory_size(descriptor) else {
                continue;
            };
            let base = u64::from(descriptor & 0x003f) << 22;
            let window_end = base + size;
            if start >= base && end <= window_end {
                return Ok(Some((index, DeviceAddr::new(start - base))));
            }
        }

        Ok(None)
    }

    /// Records an asynchronous address error.
    pub fn report_address_error(&mut self) {
        self.address_error_pending = true;
    }

    /// Returns whether the PIC1 interrupt output is asserted.
    ///
    /// Address and parity errors assert independently of CPUCTRL. Graphics DMA
    /// completion is a level derived from GDE and the DMAIDLE status bit, so
    /// masking and later re-enabling GDE immediately changes the output.
    #[must_use]
    pub fn interrupt_asserted(&self) -> bool {
        self.address_error_pending
            || self.parity_error != 0
            || (self.cpu_control & GRAPHICS_DMA_INTERRUPT_ENABLE != 0 && self.graphics_dma.idle())
    }

    /// Transfers the board's graphics-DMA synchronization input into PIC1.
    ///
    /// GSE gates only the beginning of a DMA chain. Once an asserted input or
    /// disabled GSE releases the wait, later input changes do not pause work.
    pub fn set_graphics_dma_sync_input(&mut self, asserted: bool) {
        self.graphics_dma_sync_input = asserted;
        self.graphics_dma
            .release_sync_wait(self.cpu_control & GRAPHICS_DMA_SYNC_ENABLE != 0, asserted);
    }

    /// Returns the duration until the next graphics DMA completion boundary.
    ///
    /// Descriptor fetches take five cycles of the IP12 33 MHz graphics DMA
    /// clock. Each line transfers the descriptor width as complete 32-bit GIO
    /// words and takes one cycle per transferred word; an empty line is
    /// assigned one cycle. Durations are rounded up to whole attoseconds
    /// independently at each request boundary. Internal GIO arbitration,
    /// contention, and pipeline timing are not modeled.
    #[must_use]
    pub const fn graphics_dma_time_until_event(&self) -> Option<VirtualDuration> {
        self.graphics_dma.time_until_event()
    }

    /// Advances the PIC1 graphics DMA clock domain by guest virtual time.
    pub fn advance_time(&mut self, elapsed: VirtualDuration) {
        self.graphics_dma.advance_time(elapsed);
    }

    /// Returns the next bounded graphics DMA board operation.
    ///
    /// A malformed descriptor address terminates the channel with DMAERR at
    /// the descriptor deadline and therefore returns no request.
    pub fn next_graphics_dma_request(&mut self) -> Option<GraphicsDmaRequest> {
        self.graphics_dma.next_request()
    }

    /// Completes a five-word descriptor fetch.
    ///
    /// `None` records DMAERR. A successful fetch publishes all four
    /// software-visible channel registers atomically before validating the
    /// mode. Accumulation and undefined modes terminate with DMAERR as a
    /// deterministic emulator policy; that result is not asserted to match
    /// unverified PIC1 hardware behavior.
    pub fn complete_graphics_dma_descriptor(&mut self, descriptor: Option<[u8; 20]>) {
        self.graphics_dma.complete_descriptor(descriptor);
    }

    /// Completes one graphics DMA line transfer.
    ///
    /// Failure terminates the chain with DMAERR. Success advances to the next
    /// line or descriptor, or publishes DMAIDLE after the final line.
    pub fn complete_graphics_dma_line(&mut self, success: bool) {
        self.graphics_dma.complete_line(success);
    }

    /// Returns and clears a pending whole-system reset request.
    pub fn take_system_reset_request(&mut self) -> bool {
        let requested = self.system_reset_requested;
        self.system_reset_requested = false;
        requested
    }

    const fn system_id(&self) -> u16 {
        let floating_point = if self.floating_point_present {
            0
        } else {
            FLOATING_POINT_ABSENT
        };
        (self.revision as u16) << 6
            | if self.graphics_dma.idle() {
                DMA_IDLE
            } else {
                0
            }
            | if self.graphics_dma.error {
                GRAPHICS_DMA_ERROR
            } else {
                0
            }
            | floating_point
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

fn memory_configuration_index(start: u64, end: u64) -> Result<Option<usize>, BusError> {
    for (index, base) in [MEMORY_CONFIGURATION_0, MEMORY_CONFIGURATION_1]
        .into_iter()
        .enumerate()
    {
        let register_end = base + REGISTER_BYTES;
        if start == base && end == register_end {
            return Ok(Some(index * 2));
        }
        if start < register_end && end > base {
            return Err(BusError::UnimplementedAccess);
        }
    }

    Ok(None)
}

const fn memory_size(descriptor: u16) -> Option<u64> {
    match (descriptor >> 8) & 0x000f {
        0x0 => Some(4 * 1024 * 1024),
        0x1 => Some(8 * 1024 * 1024),
        0x3 => Some(16 * 1024 * 1024),
        0x7 => Some(32 * 1024 * 1024),
        0xf => Some(64 * 1024 * 1024),
        _ => None,
    }
}

fn register_offset(start: u64, end: u64, register: u64) -> Option<usize> {
    if start < register || end > register + REGISTER_BYTES {
        return None;
    }

    usize::try_from(start - register).ok()
}

fn read_register(value: u32, offset: usize, data: &mut [u8]) {
    data.copy_from_slice(&value.to_be_bytes()[offset..offset + data.len()]);
}

fn write_register(value: u32, offset: usize, data: &[u8]) -> u32 {
    let mut bytes = value.to_be_bytes();
    bytes[offset..offset + data.len()].copy_from_slice(data);
    u32::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr, PhysAddr};
    use se_core::time::VirtualDuration;

    use super::{
        CLEAR_ERROR, CPU_CONTROL, CPU_ERROR_ADDRESS, DESCRIPTOR_ARRAY_BASE, GIO_BURST, GIO_DELAY,
        GIO_ERROR_ADDRESS, GIO_SLOT_CONFIGURATION_0, GIO_SLOT_CONFIGURATION_1,
        GRAPHICS_BUFFER_ADDRESS, GRAPHICS_BUFFER_LENGTH, GRAPHICS_DESTINATION_ADDRESS,
        GRAPHICS_DMA_CYCLE_ATTOSECONDS, GRAPHICS_DMA_FULL_PAGE, GRAPHICS_DMA_INTERRUPT_ENABLE,
        GRAPHICS_DMA_LAST_DESCRIPTOR, GRAPHICS_DMA_LINE_INCREMENT, GRAPHICS_DMA_STRIDE_INCREMENT,
        GRAPHICS_DMA_SYNC_ENABLE, GRAPHICS_DMA_WIDTH_MASK, GRAPHICS_START, GRAPHICS_STRIDE,
        GraphicsDmaDirection, GraphicsDmaRequest, MEMORY_CONFIGURATION_0, MEMORY_CONFIGURATION_1,
        PARITY_ERROR, Pic1, RESET_CONFIGURATION, SYSTEM_ID, THREE_WAY_MASK, THREE_WAY_SUBSTITUTION,
    };

    fn pic1() -> Pic1 {
        Pic1::new(0xf7, 2, true)
    }

    fn read_word(pic1: &Pic1, address: u64) -> Result<u32, BusError> {
        let mut bytes = [0; 4];
        pic1.read(DeviceAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn descriptor(words: [u32; 5]) -> [u8; 20] {
        let mut bytes = [0; 20];
        for (destination, word) in bytes.chunks_exact_mut(4).zip(words) {
            destination.copy_from_slice(&word.to_be_bytes());
        }
        bytes
    }

    fn start_dma(pic1: &mut Pic1, descriptor_address: u32) {
        pic1.write(
            DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
            &descriptor_address.to_be_bytes(),
        )
        .unwrap();
        pic1.write(DeviceAddr::new(GRAPHICS_START), &[0]).unwrap();
    }

    fn reach_next_request(pic1: &mut Pic1) -> GraphicsDmaRequest {
        let delay = pic1.graphics_dma_time_until_event().unwrap();
        pic1.advance_time(delay);
        pic1.next_graphics_dma_request().unwrap()
    }

    #[test]
    fn reset_values_match_the_ip12_profile() {
        let pic1 = pic1();

        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0));
        assert_eq!(read_word(&pic1, RESET_CONFIGURATION), Ok(0xf7));
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x88));
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_0), Ok(0));
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_1), Ok(0));
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_0), Ok(0));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_1), Ok(0));
        assert_eq!(read_word(&pic1, GIO_BURST), Ok(0));
        assert_eq!(read_word(&pic1, GIO_DELAY), Ok(0));
        assert_eq!(read_word(&pic1, THREE_WAY_MASK), Ok(0));
        assert_eq!(read_word(&pic1, THREE_WAY_SUBSTITUTION), Ok(0));
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0));
    }

    #[test]
    fn system_id_marks_an_absent_floating_point_coprocessor() {
        let pic1 = Pic1::new(0xf7, 2, false);

        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x89));
    }

    #[test]
    #[should_panic(expected = "PIC1 revision must fit in three bits")]
    fn constructor_rejects_an_out_of_range_revision() {
        let _ = Pic1::new(0xf7, 8, true);
    }

    #[test]
    fn cpu_control_stores_bits_and_turns_system_initialize_into_a_request() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(CPU_CONTROL), &0x0000_0e01_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0x0000_0c01));
        assert!(pic1.take_system_reset_request());
        assert!(!pic1.take_system_reset_request());
    }

    #[test]
    fn cpu_control_uses_big_endian_lanes_and_ignores_high_word_lanes() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(CPU_CONTROL + 3), &[0x5a]),
            Ok(())
        );
        assert_eq!(pic1.write(DeviceAddr::new(CPU_CONTROL), &[0xff]), Ok(()));
        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0x5a));
    }

    #[test]
    fn descriptor_array_base_uses_big_endian_lanes() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(
                DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
                &0x0000_000f_u32.to_be_bytes()
            ),
            Ok(())
        );

        let mut first = [0xff];
        let mut last = [0];
        assert_eq!(
            pic1.read(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE), &mut first),
            Ok(())
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE + 3), &mut last),
            Ok(())
        );
        assert_eq!(first, [0]);
        assert_eq!(last, [0x0f]);
    }

    #[test]
    fn descriptor_array_base_masks_undefined_high_bits() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE), &[0xff; 4]),
            Ok(())
        );
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0x0fff_ffff));
    }

    #[test]
    fn gio_registers_use_independent_low_big_endian_lanes() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(GIO_BURST), &0xff00_0001_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(pic1.write(DeviceAddr::new(GIO_DELAY + 3), &[0xf2]), Ok(()));
        assert_eq!(pic1.write(DeviceAddr::new(GIO_DELAY), &[0xff]), Ok(()));

        assert_eq!(read_word(&pic1, GIO_BURST), Ok(1));
        assert_eq!(read_word(&pic1, GIO_DELAY), Ok(0xf2));
    }

    #[test]
    fn gio_slot_configurations_store_two_bits_independently() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(
                DeviceAddr::new(GIO_SLOT_CONFIGURATION_0),
                &0xffff_ffff_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(GIO_SLOT_CONFIGURATION_1 + 3), &[0x02]),
            Ok(())
        );

        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_0), Ok(0x03));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_1), Ok(0x02));
    }

    #[test]
    fn three_way_address_registers_store_29_bits_independently() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(DeviceAddr::new(THREE_WAY_MASK), &[0xff; 4]),
            Ok(())
        );
        assert_eq!(
            pic1.write(
                DeviceAddr::new(THREE_WAY_SUBSTITUTION),
                &0x1234_5678_u32.to_be_bytes()
            ),
            Ok(())
        );

        assert_eq!(read_word(&pic1, THREE_WAY_MASK), Ok(0x1fff_ffff));
        assert_eq!(read_word(&pic1, THREE_WAY_SUBSTITUTION), Ok(0x1234_5678));
    }

    #[test]
    fn ide_graphics_dma_channel_register_sequence_is_mapped() {
        const PATTERNS: [u32; 12] = [
            0xaaaa_aaaa,
            0x5555_5555,
            0xcccc_cccc,
            0x3333_3333,
            0xf0f0_f0f0,
            0x0f0f_0f0f,
            0xff00_ff00,
            0x00ff_00ff,
            0xffff_0000,
            0x0000_ffff,
            0xffff_ffff,
            0x0000_0000,
        ];

        let mut pic1 = pic1();
        for (address, mask, pattern_count) in [
            (DESCRIPTOR_ARRAY_BASE, 0x0fff_ffff, 12),
            (THREE_WAY_MASK, 0x0fff_ffff, 12),
            (THREE_WAY_SUBSTITUTION, 0x0fff_ffff, 12),
            (GIO_DELAY, 0x0000_00ff, 8),
            (GIO_BURST, 0x0000_00ff, 8),
            (GIO_SLOT_CONFIGURATION_1, 0x0000_0003, 4),
            (GIO_SLOT_CONFIGURATION_0, 0x0000_0003, 4),
        ] {
            for pattern in PATTERNS.into_iter().take(pattern_count) {
                let expected = pattern & mask;
                assert_eq!(
                    pic1.write(DeviceAddr::new(address), &expected.to_be_bytes()),
                    Ok(())
                );
                assert_eq!(read_word(&pic1, address), Ok(expected));
            }
        }
    }

    #[test]
    fn clear_error_is_a_write_only_strobe() {
        let mut pic1 = pic1();
        pic1.parity_error = 0xa5;
        pic1.cpu_error_address = 0x1234_5678;
        pic1.gio_error_address = 0x9abc_def0;
        pic1.report_address_error();

        assert!(pic1.interrupt_asserted());

        assert_eq!(
            pic1.read(DeviceAddr::new(CLEAR_ERROR), &mut [0; 1]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(CLEAR_ERROR + 1), &[0x12, 0x34]),
            Ok(())
        );
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0x1234_5678));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0x9abc_def0));
        assert!(!pic1.interrupt_asserted());
    }

    #[test]
    fn error_address_registers_use_big_endian_lanes() {
        let mut pic1 = pic1();
        pic1.cpu_error_address = 0x1234_5678;
        pic1.gio_error_address = 0x9abc_def0;

        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0x1234_5678));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0x9abc_def0));

        let mut cpu_first = [0];
        let mut cpu_last = [0];
        let mut gio_middle = [0; 2];
        assert_eq!(
            pic1.read(DeviceAddr::new(CPU_ERROR_ADDRESS), &mut cpu_first),
            Ok(())
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(CPU_ERROR_ADDRESS + 3), &mut cpu_last),
            Ok(())
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(GIO_ERROR_ADDRESS + 1), &mut gio_middle),
            Ok(())
        );
        assert_eq!(cpu_first, [0x12]);
        assert_eq!(cpu_last, [0x78]);
        assert_eq!(gio_middle, [0xbc, 0xde]);
    }

    #[test]
    fn prom_error_register_sequence_is_mapped() {
        let mut pic1 = pic1();

        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0));
        assert_eq!(
            pic1.write(DeviceAddr::new(CLEAR_ERROR), &0_u32.to_be_bytes()),
            Ok(())
        );
    }

    #[test]
    fn memory_configuration_requires_aligned_words_and_masks_reserved_bits() {
        let mut pic1 = pic1();

        assert_eq!(
            pic1.write(
                DeviceAddr::new(MEMORY_CONFIGURATION_0),
                &0xffff_ffff_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_0), Ok(0x0f3f_0f3f));
        assert_eq!(
            pic1.read(DeviceAddr::new(MEMORY_CONFIGURATION_0), &mut [0]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(MEMORY_CONFIGURATION_0 + 1), &[0; 4]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(MEMORY_CONFIGURATION_1 - 1), &[0; 2]),
            Err(BusError::UnimplementedAccess)
        );
    }

    #[test]
    fn memory_decoder_supports_every_documented_size() {
        let mut pic1 = pic1();

        for (size_code, byte_len) in [
            (0x0_u16, 4_u64 * 1024 * 1024),
            (0x1, 8 * 1024 * 1024),
            (0x3, 16 * 1024 * 1024),
            (0x7, 32 * 1024 * 1024),
            (0xf, 64 * 1024 * 1024),
        ] {
            let descriptor = size_code << 8 | 5;
            let value = u32::from(descriptor) << 16 | 0x023f;
            pic1.write(
                DeviceAddr::new(MEMORY_CONFIGURATION_0),
                &value.to_be_bytes(),
            )
            .unwrap();

            let base = 5_u64 << 22;
            assert_eq!(
                pic1.decode_memory(PhysAddr::new(base), 4),
                Ok(Some((0, DeviceAddr::new(0))))
            );
            assert_eq!(
                pic1.decode_memory(PhysAddr::new(base + byte_len - 4), 4),
                Ok(Some((0, DeviceAddr::new(byte_len - 4))))
            );
            assert_eq!(
                pic1.decode_memory(PhysAddr::new(base + byte_len), 1),
                Ok(None)
            );
        }
    }

    #[test]
    fn memory_decoder_rejects_undefined_sizes_and_crossing_transactions() {
        let mut pic1 = pic1();
        let base = 5_u64 << 22;

        for size_code in [0x2_u16, 0x4, 0x5, 0x6, 0x8, 0x9, 0xa, 0xb, 0xc, 0xd, 0xe] {
            let descriptor = size_code << 8 | 5;
            let value = u32::from(descriptor) << 16 | 0x023f;
            pic1.write(
                DeviceAddr::new(MEMORY_CONFIGURATION_0),
                &value.to_be_bytes(),
            )
            .unwrap();

            assert_eq!(pic1.decode_memory(PhysAddr::new(base), 4), Ok(None));
        }

        pic1.write(
            DeviceAddr::new(MEMORY_CONFIGURATION_0),
            &(u32::from(5_u16) << 16 | 0x023f).to_be_bytes(),
        )
        .unwrap();
        assert_eq!(
            pic1.decode_memory(PhysAddr::new(base + 4 * 1024 * 1024 - 2), 4),
            Ok(None)
        );
        assert_eq!(
            pic1.decode_memory(PhysAddr::new(base), 0),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            pic1.decode_memory(PhysAddr::new(base), 5),
            Ok(Some((0, DeviceAddr::new(0))))
        );
    }

    #[test]
    fn write_bus_errors_latch_until_clear_or_reset() {
        let mut pic1 = pic1();

        assert!(!pic1.interrupt_asserted());
        pic1.report_address_error();
        assert!(pic1.interrupt_asserted());

        pic1.reset();
        assert!(!pic1.interrupt_asserted());
    }

    #[test]
    fn parity_error_writes_preserve_latched_errors() {
        let mut pic1 = pic1();
        pic1.parity_error = 0xa5;
        pic1.cpu_error_address = 0x1234_5678;
        pic1.gio_error_address = 0x9abc_def0;
        pic1.report_address_error();

        for value in [0_u32, u32::MAX] {
            for (offset, length) in [(0, 4), (0, 1), (3, 1), (2, 2)] {
                assert_eq!(
                    pic1.write(
                        DeviceAddr::new(PARITY_ERROR + offset),
                        &value.to_be_bytes()[..length],
                    ),
                    Ok(())
                );
                assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0xa5));
                assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0x1234_5678));
                assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0x9abc_def0));
                assert!(pic1.interrupt_asserted());
            }
        }

        assert_eq!(
            pic1.write(DeviceAddr::new(PARITY_ERROR + 3), &[0; 2]),
            Err(BusError::HardwareFault)
        );
    }

    #[test]
    fn other_read_only_registers_reject_writes() {
        let mut pic1 = pic1();

        for address in [
            RESET_CONFIGURATION,
            SYSTEM_ID,
            CPU_ERROR_ADDRESS,
            GIO_ERROR_ADDRESS,
        ] {
            assert_eq!(
                pic1.write(DeviceAddr::new(address), &[0]),
                Err(BusError::UnimplementedAccess)
            );
        }
    }

    #[test]
    fn reset_clears_mutable_state_and_pending_requests() {
        let mut pic1 = pic1();
        pic1.parity_error = 0xff;
        pic1.cpu_error_address = 0x1234_5678;
        pic1.gio_error_address = 0x9abc_def0;
        pic1.report_address_error();
        pic1.write(DeviceAddr::new(CPU_CONTROL), &0x0000_0201_u32.to_be_bytes())
            .unwrap();
        pic1.write(
            DeviceAddr::new(MEMORY_CONFIGURATION_0),
            &0x0100_003f_u32.to_be_bytes(),
        )
        .unwrap();
        pic1.write(
            DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();
        pic1.write(DeviceAddr::new(GIO_BURST + 3), &[1]).unwrap();
        pic1.write(DeviceAddr::new(GIO_DELAY + 3), &[0xf2]).unwrap();
        pic1.write(DeviceAddr::new(GIO_SLOT_CONFIGURATION_0 + 3), &[3])
            .unwrap();
        pic1.write(DeviceAddr::new(GIO_SLOT_CONFIGURATION_1 + 3), &[2])
            .unwrap();
        pic1.write(DeviceAddr::new(THREE_WAY_MASK), &[0xff; 4])
            .unwrap();
        pic1.write(
            DeviceAddr::new(THREE_WAY_SUBSTITUTION),
            &0x1234_5678_u32.to_be_bytes(),
        )
        .unwrap();

        pic1.reset();

        assert_eq!(read_word(&pic1, CPU_CONTROL), Ok(0));
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_0), Ok(0));
        assert_eq!(read_word(&pic1, MEMORY_CONFIGURATION_1), Ok(0));
        assert_eq!(read_word(&pic1, PARITY_ERROR), Ok(0));
        assert_eq!(read_word(&pic1, CPU_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_ERROR_ADDRESS), Ok(0));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_0), Ok(0));
        assert_eq!(read_word(&pic1, GIO_SLOT_CONFIGURATION_1), Ok(0));
        assert_eq!(read_word(&pic1, GIO_BURST), Ok(0));
        assert_eq!(read_word(&pic1, GIO_DELAY), Ok(0));
        assert_eq!(read_word(&pic1, THREE_WAY_MASK), Ok(0));
        assert_eq!(read_word(&pic1, THREE_WAY_SUBSTITUTION), Ok(0));
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0));
        assert_eq!(read_word(&pic1, RESET_CONFIGURATION), Ok(0xf7));
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x88));
        assert!(!pic1.interrupt_asserted());
        assert!(!pic1.take_system_reset_request());
    }

    #[test]
    fn rejects_invalid_unmapped_and_crossing_transactions_atomically() {
        let mut pic1 = pic1();
        pic1.write(
            DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();

        assert_eq!(
            pic1.write(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE), &[]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            pic1.write(DeviceAddr::new(DESCRIPTOR_ARRAY_BASE + 3), &[1, 2]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(CPU_ERROR_ADDRESS + 3), &mut [0; 2]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            pic1.read(DeviceAddr::new(0x100), &mut [0; 1]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0x0123_4567));
    }

    #[test]
    fn graphics_dma_start_captures_dabr_and_fetches_after_five_cycles() {
        let mut pic1 = pic1();
        start_dma(&mut pic1, 0x1000);

        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x80));
        assert_eq!(
            pic1.graphics_dma_time_until_event(),
            Some(VirtualDuration::from_attoseconds(
                GRAPHICS_DMA_CYCLE_ATTOSECONDS * 5
            ))
        );
        pic1.write(
            DeviceAddr::new(DESCRIPTOR_ARRAY_BASE),
            &0x2000_u32.to_be_bytes(),
        )
        .unwrap();
        pic1.write(DeviceAddr::new(GRAPHICS_START + 3), &[0xff])
            .unwrap();
        pic1.advance_time(VirtualDuration::from_attoseconds(
            GRAPHICS_DMA_CYCLE_ATTOSECONDS * 5 - 1,
        ));
        assert_eq!(pic1.next_graphics_dma_request(), None);
        pic1.advance_time(VirtualDuration::from_attoseconds(1));

        assert_eq!(
            pic1.next_graphics_dma_request(),
            Some(GraphicsDmaRequest::ReadDescriptor { address: 0x1000 })
        );
        assert_eq!(read_word(&pic1, DESCRIPTOR_ARRAY_BASE), Ok(0x2000));
    }

    #[test]
    fn graphics_dma_register_directions_and_start_strobe_widths_are_fixed() {
        for register in [
            GRAPHICS_BUFFER_ADDRESS,
            GRAPHICS_BUFFER_LENGTH,
            GRAPHICS_DESTINATION_ADDRESS,
            GRAPHICS_STRIDE,
        ] {
            let mut pic1 = pic1();
            assert_eq!(read_word(&pic1, register), Ok(0));
            assert_eq!(
                pic1.write(DeviceAddr::new(register), &[0xff; 4]),
                Err(BusError::UnimplementedAccess)
            );
            assert_eq!(read_word(&pic1, register), Ok(0));
        }

        for length in 1..=4 {
            for offset in 0..=4 - length {
                let mut pic1 = pic1();
                let mut read_bytes = [0; 4];
                assert_eq!(
                    pic1.read(
                        DeviceAddr::new(GRAPHICS_START + offset as u64),
                        &mut read_bytes[..length]
                    ),
                    Err(BusError::UnimplementedAccess)
                );
                let write_bytes = [0xa5; 4];
                assert_eq!(
                    pic1.write(
                        DeviceAddr::new(GRAPHICS_START + offset as u64),
                        &write_bytes[..length]
                    ),
                    Ok(())
                );
                assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x80));
            }
        }
    }

    #[test]
    fn a_new_graphics_dma_start_clears_only_the_previous_dma_error() {
        let mut pic1 = pic1();
        start_dma(&mut pic1, 0x1001);
        let delay = pic1.graphics_dma_time_until_event().unwrap();
        pic1.advance_time(delay);
        assert_eq!(pic1.next_graphics_dma_request(), None);
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x8c));

        pic1.report_address_error();
        pic1.write(DeviceAddr::new(CLEAR_ERROR), &[0]).unwrap();
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x8c));

        start_dma(&mut pic1, 0x2000);
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x80));
        pic1.reset();
        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x88));
        assert_eq!(pic1.graphics_dma_time_until_event(), None);
    }

    #[test]
    fn graphics_dma_sync_gates_only_the_start_of_the_chain() {
        let mut pic1 = pic1();
        pic1.write(
            DeviceAddr::new(CPU_CONTROL),
            &u32::from(GRAPHICS_DMA_SYNC_ENABLE).to_be_bytes(),
        )
        .unwrap();
        start_dma(&mut pic1, 0x1000);
        assert_eq!(pic1.graphics_dma_time_until_event(), None);

        pic1.set_graphics_dma_sync_input(true);
        assert_eq!(
            reach_next_request(&mut pic1),
            GraphicsDmaRequest::ReadDescriptor { address: 0x1000 }
        );
        pic1.set_graphics_dma_sync_input(false);
        pic1.complete_graphics_dma_descriptor(Some(descriptor([
            0x4000,
            GRAPHICS_DMA_LAST_DESCRIPTOR | 4,
            0x1f3f_0870,
            0,
            0,
        ])));
        assert_eq!(
            reach_next_request(&mut pic1),
            GraphicsDmaRequest::TransferLine {
                direction: GraphicsDmaDirection::MemoryToGio,
                memory_address: 0x4000,
                gio_address: 0x1f3f_0870,
                length: 4,
            }
        );
    }

    #[test]
    fn clearing_gse_releases_a_waiting_graphics_dma() {
        let mut pic1 = pic1();
        pic1.write(
            DeviceAddr::new(CPU_CONTROL),
            &u32::from(GRAPHICS_DMA_SYNC_ENABLE).to_be_bytes(),
        )
        .unwrap();
        start_dma(&mut pic1, 0x1000);

        pic1.write(DeviceAddr::new(CPU_CONTROL), &0_u32.to_be_bytes())
            .unwrap();

        assert_eq!(
            reach_next_request(&mut pic1),
            GraphicsDmaRequest::ReadDescriptor { address: 0x1000 }
        );
    }

    #[test]
    fn descriptor_lines_apply_independent_signed_host_and_gio_strides() {
        let mut pic1 = pic1();
        start_dma(&mut pic1, 0x1000);
        assert!(matches!(
            reach_next_request(&mut pic1),
            GraphicsDmaRequest::ReadDescriptor { .. }
        ));
        let control = GRAPHICS_DMA_STRIDE_INCREMENT
            | (0x0ffc << 16)
            | GRAPHICS_DMA_LAST_DESCRIPTOR
            | GRAPHICS_DMA_LINE_INCREMENT
            | 15;
        pic1.complete_graphics_dma_descriptor(Some(descriptor([
            0x4000,
            control,
            0x1f3f_0870,
            (0x0ff8 << 16) | 1,
            0,
        ])));

        assert_eq!(
            reach_next_request(&mut pic1),
            GraphicsDmaRequest::TransferLine {
                direction: GraphicsDmaDirection::MemoryToGio,
                memory_address: 0x4000,
                gio_address: 0x1f3f_0870,
                length: 16,
            }
        );
        pic1.complete_graphics_dma_line(true);
        assert_eq!(
            reach_next_request(&mut pic1),
            GraphicsDmaRequest::TransferLine {
                direction: GraphicsDmaDirection::MemoryToGio,
                memory_address: 0x4007,
                gio_address: 0x1f3f_087b,
                length: 16,
            }
        );
        pic1.complete_graphics_dma_line(true);

        assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x88));
        assert_eq!(read_word(&pic1, GRAPHICS_BUFFER_ADDRESS), Ok(0x4000));
        assert_eq!(read_word(&pic1, GRAPHICS_BUFFER_LENGTH), Ok(control));
        assert_eq!(
            read_word(&pic1, GRAPHICS_DESTINATION_ADDRESS),
            Ok(0x1f3f_0870)
        );
        assert_eq!(read_word(&pic1, GRAPHICS_STRIDE), Ok((0x0ff8 << 16) | 1));
    }

    #[test]
    fn every_graphics_dma_width_issues_complete_gio_words() {
        for width in 0..=GRAPHICS_DMA_WIDTH_MASK {
            let expected_length = if width.is_multiple_of(4) {
                width
            } else {
                width + 4 - width % 4
            } as usize;
            let mut pic1 = pic1();
            start_dma(&mut pic1, 0x1000);
            let _ = reach_next_request(&mut pic1);
            pic1.complete_graphics_dma_descriptor(Some(descriptor([
                0x4000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | width,
                0x1f3f_0870,
                0,
                0,
            ])));

            assert_eq!(
                reach_next_request(&mut pic1),
                GraphicsDmaRequest::TransferLine {
                    direction: GraphicsDmaDirection::MemoryToGio,
                    memory_address: 0x4000,
                    gio_address: 0x1f3f_0870,
                    length: expected_length,
                },
                "WIDTH {width:#05x}"
            );
        }
    }

    #[test]
    fn full_page_and_gio_to_memory_modes_preserve_line_timing() {
        let mut pic1 = pic1();
        start_dma(&mut pic1, 0x1000);
        let _ = reach_next_request(&mut pic1);
        pic1.complete_graphics_dma_descriptor(Some(descriptor([
            0x4000,
            GRAPHICS_DMA_FULL_PAGE | GRAPHICS_DMA_LAST_DESCRIPTOR | 0x2000,
            0x1f3f_0870,
            0,
            0,
        ])));

        assert_eq!(
            pic1.graphics_dma_time_until_event(),
            Some(VirtualDuration::from_attoseconds(
                GRAPHICS_DMA_CYCLE_ATTOSECONDS * 1024
            ))
        );
        assert_eq!(
            reach_next_request(&mut pic1),
            GraphicsDmaRequest::TransferLine {
                direction: GraphicsDmaDirection::GioToMemory,
                memory_address: 0x4000,
                gio_address: 0x1f3f_0870,
                length: 4096,
            }
        );
    }

    #[test]
    fn descriptor_chains_fetch_the_next_entry_after_the_last_line() {
        let mut pic1 = pic1();
        start_dma(&mut pic1, 0x1000);
        let _ = reach_next_request(&mut pic1);
        pic1.complete_graphics_dma_descriptor(Some(descriptor([
            0x4000,
            4,
            0x1f3f_0870,
            0,
            0x2345_6000,
        ])));
        let _ = reach_next_request(&mut pic1);
        pic1.complete_graphics_dma_line(true);

        assert_eq!(
            reach_next_request(&mut pic1),
            GraphicsDmaRequest::ReadDescriptor {
                address: 0x0345_6000
            }
        );
    }

    #[test]
    fn malformed_and_unsupported_descriptors_end_with_dmaerr() {
        for descriptor_address in [0x1001, 0x1ff0] {
            let mut pic1 = pic1();
            start_dma(&mut pic1, descriptor_address);
            let delay = pic1.graphics_dma_time_until_event().unwrap();
            pic1.advance_time(delay);

            assert_eq!(pic1.next_graphics_dma_request(), None);
            assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x8c));
        }

        for mode in [1_u32, 3] {
            let mut pic1 = pic1();
            start_dma(&mut pic1, 0x1000);
            let _ = reach_next_request(&mut pic1);
            pic1.complete_graphics_dma_descriptor(Some(descriptor([
                0x4000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | (mode << 12) | 4,
                0x1f3f_0870,
                0,
                0,
            ])));

            assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x8c));
            assert_eq!(pic1.graphics_dma_time_until_event(), None);
        }
    }

    #[test]
    fn graphics_dma_interrupt_is_the_gde_and_idle_level() {
        let mut pic1 = pic1();
        pic1.write(
            DeviceAddr::new(CPU_CONTROL),
            &u32::from(GRAPHICS_DMA_INTERRUPT_ENABLE).to_be_bytes(),
        )
        .unwrap();
        assert!(pic1.interrupt_asserted());

        start_dma(&mut pic1, 0x1000);
        assert!(!pic1.interrupt_asserted());
        let _ = reach_next_request(&mut pic1);
        pic1.complete_graphics_dma_descriptor(None);
        assert!(pic1.interrupt_asserted());
        pic1.write(DeviceAddr::new(CPU_CONTROL), &0_u32.to_be_bytes())
            .unwrap();
        assert!(!pic1.interrupt_asserted());
        pic1.write(
            DeviceAddr::new(CPU_CONTROL),
            &u32::from(GRAPHICS_DMA_INTERRUPT_ENABLE).to_be_bytes(),
        )
        .unwrap();
        assert!(pic1.interrupt_asserted());
    }

    #[test]
    fn graphics_dma_progress_is_invariant_under_elapsed_fragmentation() {
        fn complete(intervals: &[u128]) -> Vec<GraphicsDmaRequest> {
            let mut pic1 = pic1();
            start_dma(&mut pic1, 0x1000);
            let mut requests = Vec::new();
            for elapsed in intervals {
                pic1.advance_time(VirtualDuration::from_attoseconds(*elapsed));
                while let Some(request) = pic1.next_graphics_dma_request() {
                    requests.push(request);
                    match request {
                        GraphicsDmaRequest::ReadDescriptor { .. } => {
                            pic1.complete_graphics_dma_descriptor(Some(descriptor([
                                0x4000,
                                GRAPHICS_DMA_LAST_DESCRIPTOR | 8,
                                0x1f3f_0870,
                                0,
                                0,
                            ])));
                        }
                        GraphicsDmaRequest::TransferLine { .. } => {
                            pic1.complete_graphics_dma_line(true);
                        }
                    }
                }
            }
            assert_eq!(read_word(&pic1, SYSTEM_ID), Ok(0x88));
            requests
        }

        let descriptor_time = GRAPHICS_DMA_CYCLE_ATTOSECONDS * 5;
        let line_time = GRAPHICS_DMA_CYCLE_ATTOSECONDS * 2;
        let whole = complete(&[descriptor_time + line_time]);
        let split = complete(&[descriptor_time - 1, 1, line_time - 1, 1]);

        assert_eq!(whole, split);
    }
}
