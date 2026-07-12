//! SGI MACE 2.0 I/O ASIC.
//!
//! MACE is a device and controller on the CRIME CMI link and a controller for
//! the PCI, ISA, I2C, and external media communication domains. Chip-private
//! register banks, DMA engines, FIFOs, and media pipelines remain owned by this
//! component.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use se_core::component::{Component, ComponentId};
use se_core::role::{BusControllerRole, BusDeviceRole};
use se_core::scheduler::SimTime;
use se_core::tracing::TraceLevel;

use crate::bus::i2c::{I2cCompletion, I2cRate, I2cTransaction};
use crate::bus::irq::{IrqDelivery, IrqInput};
use crate::bus::isa::{
    IsaBusError, IsaCompletion, IsaCompletionPayload, IsaTransaction, IsaTransactionId, IsaTransfer,
};
use crate::bus::media::{MediaPayload, MediaPort, MediaTransaction};
use crate::bus::pci::{PciCommand, PciCompletion, PciStatus, PciTransaction};
use crate::chipset::crime::protocol::{
    CrimeBusError, CrimeCmiCompletion, CrimeCmiTransaction, CrimeCompletionPayload,
    CrimeInterruptPost, CrimeLinkDeviceResponse, CrimeLinkOperation, CrimePioRequest,
    CrimeTransactionId, CrimeTransfer,
};

use self::audio::MaceAudio;
use self::config::MaceConfig;
use self::ethernet::MaceEthernet;
use self::interrupt::{MaceInterruptController, MaceInterruptGroup};
use self::pci::MacePci;
use self::peripheral::{I2cPort, IsaController, MaceTimers, Ps2Port};
use self::protocol::{
    MaceAction, MaceEvent, MacePoll, MaceTraceEvent, MaceTraceField, MaceTraceValue, MaceWiring,
};
use self::system::{MaceAddressTarget, MaceExternalIsaTarget};
use self::video::VideoChannel;

pub mod audio;
pub mod config;
pub mod ethernet;
pub mod interrupt;
pub mod pci;
pub mod peripheral;
pub mod protocol;
pub mod registers;
pub mod system;
pub mod trace;
pub mod video;

/// External RTC IRQ input on MACE.
pub const MACE_IRQ_RTC: IrqInput = IrqInput::new(0);
/// First external UART IRQ input on MACE.
pub const MACE_IRQ_SERIAL0: IrqInput = IrqInput::new(1);
/// Second external UART IRQ input on MACE.
pub const MACE_IRQ_SERIAL1: IrqInput = IrqInput::new(2);
/// External parallel-port IRQ input on MACE.
pub const MACE_IRQ_PARALLEL: IrqInput = IrqInput::new(3);
/// First PCI IRQ input on MACE.
pub const MACE_IRQ_PCI0: IrqInput = IrqInput::new(8);

/// MACE model error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaceError {
    InvalidTimebase,
    TransactionIdOverflow,
    UnexpectedCmiCompletion(CrimeTransactionId),
    UnexpectedIsaCompletion(IsaTransactionId),
    UnsupportedIrqInput(IrqInput),
    HostPortFull(MediaPort),
}

impl fmt::Display for MaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimebase => formatter.write_str("MACE timebase must be nonzero"),
            Self::TransactionIdOverflow => {
                formatter.write_str("MACE transaction identifier overflow")
            }
            Self::UnexpectedCmiCompletion(id) => {
                write!(formatter, "unexpected MACE CMI completion {id}")
            }
            Self::UnexpectedIsaCompletion(id) => {
                write!(formatter, "unexpected MACE ISA completion {}", id.get())
            }
            Self::UnsupportedIrqInput(input) => {
                write!(formatter, "unsupported MACE IRQ input {}", input.get())
            }
            Self::HostPortFull(port) => write!(formatter, "MACE host port is full: {port:?}"),
        }
    }
}

impl std::error::Error for MaceError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingIsa {
    cmi_id: CrimeTransactionId,
}

/// MACE 2.0 component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mace {
    id: ComponentId,
    name: String,
    config: MaceConfig,
    wiring: MaceWiring,
    timebase_hz: u64,
    now: SimTime,
    epoch: u64,
    next_transaction_id: u128,
    actions: VecDeque<MaceAction>,
    pending_isa: BTreeMap<IsaTransactionId, PendingIsa>,
    pending_cmi: BTreeSet<CrimeTransactionId>,
    terminal_error: Option<MaceError>,
    interrupts: MaceInterruptController,
    timers: MaceTimers,
    isa: IsaController,
    ps2: [Ps2Port; 2],
    i2c: [I2cPort; 2],
    audio: MaceAudio,
    ethernet: MaceEthernet,
    video: [VideoChannel; 3],
    pci: MacePci,
    host_inputs: VecDeque<MediaTransaction>,
    host_outputs: VecDeque<MediaTransaction>,
}

impl Mace {
    /// Creates a MACE 2.0 ASIC.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        config: MaceConfig,
        wiring: MaceWiring,
        timebase_hz: u64,
    ) -> Result<Self, MaceError> {
        if timebase_hz == 0 {
            return Err(MaceError::InvalidTimebase);
        }
        Ok(Self {
            id,
            name: name.into(),
            config,
            wiring,
            timebase_hz,
            now: SimTime::ZERO,
            epoch: 0,
            next_transaction_id: 0,
            actions: VecDeque::new(),
            pending_isa: BTreeMap::new(),
            pending_cmi: BTreeSet::new(),
            terminal_error: None,
            interrupts: MaceInterruptController::new(),
            timers: MaceTimers::new(timebase_hz),
            isa: IsaController::new(),
            ps2: [Ps2Port::new(), Ps2Port::new()],
            i2c: [I2cPort::new(), I2cPort::new()],
            audio: MaceAudio::new(),
            ethernet: MaceEthernet::new(),
            video: [
                VideoChannel::new(false),
                VideoChannel::new(false),
                VideoChannel::new(true),
            ],
            pci: MacePci::new(),
            host_inputs: VecDeque::new(),
            host_outputs: VecDeque::new(),
        })
    }

    /// Observes the current simulation time before accepting work.
    pub fn observe_time(&mut self, now: SimTime) {
        self.now = now;
    }

    /// Applies power-on reset to volatile MACE state.
    pub fn power_on(&mut self, now: SimTime) {
        self.epoch = self.epoch.wrapping_add(1);
        self.now = now;
        self.next_transaction_id = 0;
        self.actions.clear();
        self.pending_isa.clear();
        self.pending_cmi.clear();
        self.terminal_error = None;
        self.interrupts.reset();
        self.timers.power_on(now);
        self.isa.reset();
        self.ps2 = [Ps2Port::new(), Ps2Port::new()];
        self.i2c = [I2cPort::new(), I2cPort::new()];
        self.audio.reset();
        self.ethernet.reset();
        for channel in &mut self.video {
            channel.reset();
        }
        self.pci.reset();
        self.host_inputs.clear();
        self.host_outputs.clear();
    }

    /// Cancels in-flight transactions and resets volatile logic.
    pub fn hard_reset(&mut self, now: SimTime) {
        self.power_on(now);
    }

    /// Handles a scheduled MACE transition.
    pub fn handle_event(&mut self, now: SimTime, event: MaceEvent) {
        self.now = now;
        let event_epoch = match event {
            MaceEvent::TimerCompare { epoch, .. }
            | MaceEvent::Ps2Transmit { epoch, .. }
            | MaceEvent::I2cComplete { epoch, .. }
            | MaceEvent::DmaStep { epoch, .. }
            | MaceEvent::VideoLine { epoch, .. }
            | MaceEvent::EthernetStep { epoch } => epoch,
        };
        if event_epoch != self.epoch {
            return;
        }
        match event {
            MaceEvent::TimerCompare { timer, .. } if timer < 3 => {
                if self.timers.fire_compare(now, timer as usize) {
                    self.interrupts.set_peripheral_source(13 + timer, true);
                    self.push_interrupt_posts();
                }
            }
            MaceEvent::Ps2Transmit { port, .. } if port < 2 => {
                if let Some(byte) = self.ps2[port as usize].take_transmit() {
                    let target = if port == 0 {
                        self.wiring.external_links.keyboard
                    } else {
                        self.wiring.external_links.mouse
                    };
                    self.actions
                        .push_back(MaceAction::StartExternal(MediaTransaction {
                            source: self.id,
                            target,
                            port: if port == 0 {
                                MediaPort::Keyboard
                            } else {
                                MediaPort::Mouse
                            },
                            payload: MediaPayload::Bytes(vec![byte]),
                        }));
                }
                self.update_ps2_interrupts();
            }
            MaceEvent::I2cComplete { port, .. } if port < 2 => {
                self.i2c[port as usize].complete(false, false)
            }
            MaceEvent::DmaStep { .. }
            | MaceEvent::VideoLine { .. }
            | MaceEvent::EthernetStep { .. }
            | MaceEvent::TimerCompare { .. }
            | MaceEvent::Ps2Transmit { .. }
            | MaceEvent::I2cComplete { .. } => {}
        }
    }

    /// Polls one MACE action or terminal error.
    pub fn poll(&mut self) -> Result<MacePoll, MaceError> {
        if let Some(error) = self.terminal_error.take() {
            return Err(error);
        }
        Ok(self
            .actions
            .pop_front()
            .map(MacePoll::Action)
            .unwrap_or(MacePoll::Idle))
    }

    /// Accepts one host-neutral media input.
    pub fn accept_host_input(&mut self, transaction: MediaTransaction) -> Result<(), MaceError> {
        let capacity = match transaction.port {
            MediaPort::Ethernet => self.config.ports.ethernet_frames,
            MediaPort::AudioInput | MediaPort::AudioOutput1 | MediaPort::AudioOutput2 => {
                self.config.ports.audio_sample_pairs
            }
            MediaPort::VideoInputAb | MediaPort::VideoInputCd | MediaPort::VideoOutput => {
                self.config.ports.video_fields
            }
            _ => self.config.ports.byte_stream_bytes,
        };
        if self.host_inputs.len() >= capacity {
            return Err(MaceError::HostPortFull(transaction.port));
        }
        match (&transaction.port, &transaction.payload) {
            (MediaPort::Keyboard, MediaPayload::Bytes(bytes))
            | (MediaPort::Mouse, MediaPayload::Bytes(bytes)) => {
                let index = usize::from(transaction.port == MediaPort::Mouse);
                if let Some(&byte) = bytes.first() {
                    self.ps2[index].receive(byte, false, false);
                    self.update_ps2_interrupts();
                }
            }
            (MediaPort::Ethernet, MediaPayload::Ethernet(frame))
                if self.ethernet.accepts(&frame.data) =>
            {
                self.ethernet.interrupt_status |= 1 << 5;
                self.interrupts
                    .set_group(MaceInterruptGroup::Ethernet, true);
                self.push_interrupt_posts();
            }
            _ => {}
        }
        let port = transaction.port;
        self.host_inputs.push_back(transaction);
        self.actions.push_back(MaceAction::Trace(MaceTraceEvent {
            level: TraceLevel::Debug,
            target: trace::MEDIA,
            event: "host_input",
            fields: vec![MaceTraceField {
                key: "port",
                value: MaceTraceValue::String(media_port_name(port)),
            }],
        }));
        Ok(())
    }

    /// Polls one host-neutral output.
    pub fn poll_host_output(&mut self) -> Option<MediaTransaction> {
        self.host_outputs.pop_front()
    }

    /// Returns the number of queued host inputs for one external port.
    pub fn host_input_len(&self, port: MediaPort) -> usize {
        self.host_inputs
            .iter()
            .filter(|input| input.port == port)
            .count()
    }

    /// Records one ordered output delivered to a host-neutral endpoint.
    pub fn record_host_output(&mut self, transaction: MediaTransaction) -> Result<(), MaceError> {
        let capacity = match transaction.port {
            MediaPort::Ethernet => self.config.ports.ethernet_frames,
            MediaPort::AudioInput | MediaPort::AudioOutput1 | MediaPort::AudioOutput2 => {
                self.config.ports.audio_sample_pairs
            }
            MediaPort::VideoInputAb | MediaPort::VideoInputCd | MediaPort::VideoOutput => {
                self.config.ports.video_fields
            }
            _ => self.config.ports.byte_stream_bytes,
        };
        if self.host_outputs.len() >= capacity {
            return Err(MaceError::HostPortFull(transaction.port));
        }
        self.host_outputs.push_back(transaction);
        Ok(())
    }

    /// Returns immutable timer state for diagnostics.
    pub const fn timers(&self) -> &MaceTimers {
        &self.timers
    }
    /// Returns the current peripheral interrupt status.
    pub const fn peripheral_interrupt_status(&self) -> u32 {
        self.interrupts.peripheral_status()
    }

    fn accept_cmi(
        &mut self,
        transaction: CrimeCmiTransaction,
    ) -> CrimeLinkDeviceResponse<CrimeCmiCompletion> {
        let id = transaction.id;
        let CrimeLinkOperation::Pio(request) = transaction.operation else {
            return CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
                id,
                result: Err(CrimeBusError::Unsupported),
                memory_fault: None,
            });
        };
        let Some(resolution) = system::resolve(request.address, request.transfer.length()) else {
            return complete_cmi_error(id, CrimeBusError::Address);
        };
        self.actions.push_back(MaceAction::Trace(MaceTraceEvent {
            level: TraceLevel::Trace,
            target: trace::CMI,
            event: "pio",
            fields: vec![
                MaceTraceField {
                    key: "address",
                    value: MaceTraceValue::Hex64(request.address),
                },
                MaceTraceField {
                    key: "width",
                    value: MaceTraceValue::U64(request.transfer.length() as u64),
                },
                MaceTraceField {
                    key: "write",
                    value: MaceTraceValue::Bool(matches!(
                        &request.transfer,
                        CrimeTransfer::Write { .. }
                    )),
                },
            ],
        }));
        match resolution.target {
            MaceAddressTarget::SystemFlash => self.start_isa(
                id,
                self.wiring.prom,
                resolution.offset as u32,
                request.transfer,
            ),
            MaceAddressTarget::ExternalIsa => {
                let Some(external) =
                    system::resolve_external_isa(resolution.offset, request.transfer.length())
                else {
                    return complete_cmi_error(id, CrimeBusError::Address);
                };
                let target = match external.target {
                    MaceExternalIsaTarget::Parallel => self.wiring.parallel,
                    MaceExternalIsaTarget::Serial1 => self.wiring.serial[0],
                    MaceExternalIsaTarget::Serial2 => self.wiring.serial[1],
                    MaceExternalIsaTarget::Rtc => self.wiring.rtc,
                };
                self.start_isa(id, target, external.register, request.transfer)
            }
            MaceAddressTarget::PciRegisters if (0x0cfc..0x0d00).contains(&resolution.offset) => {
                self.start_pci_config(id, resolution.offset, request)
            }
            MaceAddressTarget::PciIo
            | MaceAddressTarget::PciMemory
            | MaceAddressTarget::PciConfiguration => {
                self.start_pci(id, resolution.target, resolution.offset, request)
            }
            MaceAddressTarget::Future => complete_cmi_error(id, CrimeBusError::Unsupported),
            target => CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
                id,
                result: self.access_internal(target, resolution.offset, request.transfer),
                memory_fault: None,
            }),
        }
    }

    fn start_isa(
        &mut self,
        cmi_id: CrimeTransactionId,
        target: ComponentId,
        address: u32,
        transfer: CrimeTransfer,
    ) -> CrimeLinkDeviceResponse<CrimeCmiCompletion> {
        let Some(id) = self.allocate_isa_id() else {
            return complete_cmi_error(cmi_id, CrimeBusError::Unsupported);
        };
        self.pending_isa.insert(id, PendingIsa { cmi_id });
        self.actions.push_back(MaceAction::StartIsa(IsaTransaction {
            id,
            time: self.now,
            controller: self.id,
            target,
            address,
            transfer: to_isa_transfer(transfer),
        }));
        CrimeLinkDeviceResponse::Deferred
    }

    fn start_pci(
        &mut self,
        cmi_id: CrimeTransactionId,
        target: MaceAddressTarget,
        address: u64,
        request: CrimePioRequest,
    ) -> CrimeLinkDeviceResponse<CrimeCmiCompletion> {
        let command = match (&request.transfer, target) {
            (CrimeTransfer::Read { .. }, MaceAddressTarget::PciIo) => PciCommand::IoRead,
            (CrimeTransfer::Write { .. }, MaceAddressTarget::PciIo) => PciCommand::IoWrite,
            (CrimeTransfer::Read { .. }, MaceAddressTarget::PciMemory) => PciCommand::MemoryRead,
            (CrimeTransfer::Write { .. }, MaceAddressTarget::PciMemory) => PciCommand::MemoryWrite,
            (CrimeTransfer::Read { .. }, MaceAddressTarget::PciConfiguration) => {
                PciCommand::ConfigurationRead
            }
            (CrimeTransfer::Write { .. }, MaceAddressTarget::PciConfiguration) => {
                PciCommand::ConfigurationWrite
            }
            _ => return complete_cmi_error(cmi_id, CrimeBusError::Address),
        };
        let (mut data, mut byte_enable) = crime_transfer_parts(&request.transfer);
        data.reverse();
        byte_enable.reverse();
        self.actions.push_back(MaceAction::StartPci(PciTransaction {
            id: cmi_id.get(),
            controller: self.id,
            target: self.wiring.pci_devices[0],
            command,
            address,
            configuration: None,
            data,
            byte_enable,
        }));
        self.pending_cmi.insert(cmi_id);
        CrimeLinkDeviceResponse::Deferred
    }

    fn start_pci_config(
        &mut self,
        cmi_id: CrimeTransactionId,
        offset: u64,
        request: CrimePioRequest,
    ) -> CrimeLinkDeviceResponse<CrimeCmiCompletion> {
        let bus = ((self.pci.config_address >> 16) & 0xff) as u8;
        let devfn = ((self.pci.config_address >> 8) & 0xff) as u8;
        let device = devfn >> 3;
        let function = devfn & 7;
        let register = (self.pci.config_address & 0xfc) as u8;
        let Some(&target) = self.wiring.pci_devices.get(usize::from(device)) else {
            return complete_cmi_error(cmi_id, CrimeBusError::Address);
        };
        let command = if matches!(&request.transfer, CrimeTransfer::Read { .. }) {
            PciCommand::ConfigurationRead
        } else {
            PciCommand::ConfigurationWrite
        };
        let (mut data, mut byte_enable) = crime_transfer_parts(&request.transfer);
        data.reverse();
        byte_enable.reverse();
        self.actions.push_back(MaceAction::StartPci(PciTransaction {
            id: cmi_id.get(),
            controller: self.id,
            target,
            command,
            address: offset - 0x0cfc,
            configuration: Some(crate::bus::pci::PciConfigurationAddress {
                bus,
                device,
                function,
                register,
            }),
            data,
            byte_enable,
        }));
        self.pending_cmi.insert(cmi_id);
        CrimeLinkDeviceResponse::Deferred
    }

    fn access_internal(
        &mut self,
        target: MaceAddressTarget,
        offset: u64,
        transfer: CrimeTransfer,
    ) -> Result<CrimeCompletionPayload, CrimeBusError> {
        let width = transfer.length();
        let pci = target == MaceAddressTarget::PciRegisters;
        if pci {
            if !(1..=4).contains(&width) {
                return Err(CrimeBusError::Access);
            }
        } else if !matches!(width, 4 | 8) || offset & (width as u64 - 1) != 0 {
            return Err(CrimeBusError::Access);
        }
        let aligned_offset = if pci { offset } else { offset & !7 };
        let lane = if pci { 0 } else { (offset & 7) as usize };
        match transfer {
            CrimeTransfer::Read { .. } => {
                let value = self.read_internal(target, aligned_offset, width)?;
                let encoded = encode_value(value, if pci { width } else { 8 });
                Ok(CrimeCompletionPayload::ReadData(
                    encoded[lane..lane + width].to_vec(),
                ))
            }
            CrimeTransfer::Write { data, byte_enable } => {
                if data.len() != byte_enable.len() || byte_enable.iter().any(|enabled| !enabled) {
                    return Err(CrimeBusError::Access);
                }
                let value = if pci || width == 8 {
                    decode_value(&data)
                } else {
                    let current = self.read_internal(target, aligned_offset, 8)?;
                    let shift = (8 - lane - width) * 8;
                    let mask = ((1_u64 << (width * 8)) - 1) << shift;
                    current & !mask | (decode_value(&data) << shift)
                };
                self.write_internal(target, aligned_offset, if pci { width } else { 8 }, value)?;
                Ok(CrimeCompletionPayload::WriteComplete)
            }
        }
    }

    fn read_internal(
        &mut self,
        target: MaceAddressTarget,
        offset: u64,
        width: usize,
    ) -> Result<u64, CrimeBusError> {
        match target {
            MaceAddressTarget::PciRegisters => self.read_pci(offset, width),
            MaceAddressTarget::VideoInput1 => {
                self.video[0].read(offset).ok_or(CrimeBusError::Address)
            }
            MaceAddressTarget::VideoInput2 => {
                self.video[1].read(offset).ok_or(CrimeBusError::Address)
            }
            MaceAddressTarget::VideoOutput => {
                self.video[2].read(offset).ok_or(CrimeBusError::Address)
            }
            MaceAddressTarget::Ethernet => self.read_ethernet(offset),
            MaceAddressTarget::Peripheral => self.read_peripheral(offset),
            _ => Err(CrimeBusError::Unsupported),
        }
    }

    fn write_internal(
        &mut self,
        target: MaceAddressTarget,
        offset: u64,
        width: usize,
        value: u64,
    ) -> Result<(), CrimeBusError> {
        let result = match target {
            MaceAddressTarget::PciRegisters => return self.write_pci(offset, width, value),
            MaceAddressTarget::VideoInput1 => self.video[0].write(offset, value),
            MaceAddressTarget::VideoInput2 => self.video[1].write(offset, value),
            MaceAddressTarget::VideoOutput => self.video[2].write(offset, value),
            MaceAddressTarget::Ethernet => return self.write_ethernet(offset, value),
            MaceAddressTarget::Peripheral => return self.write_peripheral(offset, value),
            _ => false,
        };
        if result {
            Ok(())
        } else {
            Err(CrimeBusError::Address)
        }
    }

    fn read_pci(&self, offset: u64, width: usize) -> Result<u64, CrimeBusError> {
        let value = match offset {
            0x0000 => self.pci.error_address,
            0x0004 => self.pci.error_flags,
            0x0008 => self.pci.control,
            0x000c => registers::MACE_REVISION,
            0x0cf8 => self.pci.config_address,
            0x0cfc => u32::MAX,
            _ if width == 4 => u32::MAX,
            _ => return Err(CrimeBusError::Access),
        };
        Ok(u64::from(value))
    }

    fn write_pci(&mut self, offset: u64, width: usize, value: u64) -> Result<(), CrimeBusError> {
        if width != 4 && offset != 0x0cfc {
            self.pci.error_flags |= pci::error::ILLEGAL_TRANSACTION;
            return Err(CrimeBusError::Access);
        }
        match offset {
            0x0004 => self.pci.write_error_flags(value as u32),
            0x0008 => self.pci.control = value as u32,
            0x000c => self.pci.flush_prefetch(),
            0x0cf8 => self.pci.config_address = value as u32,
            0x0cfc => {}
            0x0000 => {}
            _ => {}
        }
        self.interrupts
            .set_group(MaceInterruptGroup::PciError, self.pci.error_interrupt());
        self.push_interrupt_posts();
        Ok(())
    }

    fn read_ethernet(&self, offset: u64) -> Result<u64, CrimeBusError> {
        let value = match offset {
            0x00 => u64::from(self.ethernet.mac_control),
            0x08 => u64::from(self.ethernet.interrupt_status),
            0x10 => u64::from(self.ethernet.dma_control),
            0x18 => u64::from(self.ethernet.interrupt_delay),
            0x30 | 0x38 => u64::from(self.ethernet.tx_info),
            0x40 | 0x48 | 0x50 => self.ethernet.rx_clusters.len() as u64,
            0x58 => self.ethernet.last_tx_vector,
            0xa0 => bytes_to_u64(&self.ethernet.station_address),
            0xa8 => bytes_to_u64(&self.ethernet.secondary_address),
            0xb0 => self.ethernet.multicast_filter,
            0xb8 => u64::from(self.ethernet.tx_ring_base),
            _ => return Err(CrimeBusError::Address),
        };
        Ok(value)
    }

    fn write_ethernet(&mut self, offset: u64, value: u64) -> Result<(), CrimeBusError> {
        match offset {
            0x00 => {
                self.ethernet.mac_control =
                    value as u32 & 0x1fff_ffff | ethernet::MAC_IMPLEMENTATION_REVISION << 29;
                if value & 1 != 0 {
                    self.ethernet.reset();
                }
            }
            0x08 => self.ethernet.clear_interrupts(value as u32),
            0x10 => self.ethernet.dma_control = value as u16,
            0x18 => self.ethernet.interrupt_delay = value as u8 & 0x3f,
            0x20 => {
                self.ethernet.interrupt_status =
                    self.ethernet.interrupt_status & !0x1f | value as u32 & 0x1f
            }
            0x28 => {
                self.ethernet.interrupt_status =
                    self.ethernet.interrupt_status & !0xe0 | value as u32 & 0xe0
            }
            0x58 => self.ethernet.interrupt_status |= value as u32 & 0xff,
            0x78 => {}
            0xa0 => self.ethernet.station_address = u64_to_six_bytes(value),
            0xa8 => self.ethernet.secondary_address = u64_to_six_bytes(value),
            0xb0 => self.ethernet.multicast_filter = value,
            0xb8 => self.ethernet.tx_ring_base = value as u32 & 0xffff_e000,
            0x100..=0x1f8 if offset & 7 == 0 => {
                if !self
                    .ethernet
                    .push_receive_cluster(value as u32 & 0xffff_f000)
                {
                    return Err(CrimeBusError::Access);
                }
            }
            _ => return Err(CrimeBusError::Address),
        }
        self.interrupts
            .set_group(MaceInterruptGroup::Ethernet, self.ethernet.interrupt());
        self.push_interrupt_posts();
        Ok(())
    }

    fn read_peripheral(&mut self, offset: u64) -> Result<u64, CrimeBusError> {
        match offset {
            0x10000 => Ok(u64::from(self.isa.ring_base_reset)),
            0x10008 => Ok(u64::from(self.isa.misc)),
            0x10010 => Ok(u64::from(self.interrupts.peripheral_status())),
            0x10018 => Ok(u64::from(self.interrupts.peripheral_mask())),
            0x12000..=0x13fff => Ok(bytes_to_u64(
                &self.isa.dp_ram[(offset as usize - 0x12000)..(offset as usize - 0x12000 + 8)],
            )),
            0x14000 => Ok(self.isa.parallel.context[0]),
            0x14008 => Ok(self.isa.parallel.context[1]),
            0x14010 => Ok(u64::from(self.isa.parallel.control)),
            0x14018 => Ok(u64::from(self.isa.parallel.diagnostic)),
            0x18000..=0x1c038 => self.read_serial_dma(offset),
            0x20000..=0x20038 => self.read_ps2(offset - 0x20000),
            0x30000..=0x30038 => self.read_i2c(offset - 0x30000),
            0x40000 => Ok(u64::from(self.timers.ust(self.now))),
            0x40008 => Ok(u64::from(self.timers.compare(0))),
            0x40010 => Ok(u64::from(self.timers.compare(1))),
            0x40018 => Ok(u64::from(self.timers.compare(2))),
            0x40020..=0x40048 if (offset - 0x40020).is_multiple_of(8) => {
                Ok(self.timers.media_pair(((offset - 0x40020) / 8) as usize))
            }
            0x00000..=0x00078 => self.read_audio(offset),
            _ => Err(CrimeBusError::Address),
        }
    }

    fn write_peripheral(&mut self, offset: u64, value: u64) -> Result<(), CrimeBusError> {
        match offset {
            0x10000 => self.isa.ring_base_reset = value as u32 & 0xffff_8001,
            0x10008 => self.isa.misc = value as u16 & 0x01fd,
            0x10010 => self.interrupts.clear_edge_sources(value as u32),
            0x10018 => self.interrupts.set_peripheral_mask(value as u32),
            0x12000..=0x13ff8 if offset & 7 == 0 => {
                if self.isa.dp_ram_write_enabled() {
                    let index = offset as usize - 0x12000;
                    self.isa.dp_ram[index..index + 8].copy_from_slice(&value.to_be_bytes());
                }
            }
            0x14000 => self.isa.parallel.context[0] = value,
            0x14008 => self.isa.parallel.context[1] = value,
            0x14010 => self.isa.parallel.write_control(value as u8),
            0x14018 => {}
            0x18000..=0x1c038 => self.write_serial_dma(offset, value as u16)?,
            0x20000..=0x20038 => self.write_ps2(offset - 0x20000, value as u8)?,
            0x30000..=0x30038 => self.write_i2c(offset - 0x30000, value as u8)?,
            0x40000 => self.timers.write_ust(self.now, value as u32),
            0x40008 | 0x40010 | 0x40018 => {
                let index = ((offset - 0x40008) / 8) as usize;
                self.timers.write_compare(index, value as u32);
                self.interrupts
                    .set_peripheral_source(13 + index as u8, false);
                self.actions.push_back(MaceAction::Schedule {
                    delay: self.timers.compare_delay(self.now, index),
                    event: MaceEvent::TimerCompare {
                        epoch: self.epoch,
                        timer: index as u8,
                    },
                });
            }
            0x40020..=0x40048 if (offset - 0x40020).is_multiple_of(8) => self
                .timers
                .write_media_pair(((offset - 0x40020) / 8) as usize, value),
            0x00000..=0x00078 => self.write_audio(offset, value)?,
            _ => return Err(CrimeBusError::Address),
        }
        self.push_interrupt_posts();
        Ok(())
    }

    fn read_serial_dma(&self, offset: u64) -> Result<u64, CrimeBusError> {
        let port = usize::from(offset >= 0x1c000);
        let local = offset - if port == 0 { 0x18000 } else { 0x1c000 };
        let direction = usize::from(local & 0x20 != 0);
        let register = local & 0x1f;
        let channel = &self.isa.serial_dma[port * 2 + direction];
        Ok(u64::from(match register {
            0x00 => channel.control,
            0x08 => channel.read_pointer,
            0x10 => channel.write_pointer,
            0x18 => channel.depth(),
            _ => return Err(CrimeBusError::Address),
        }))
    }

    fn write_serial_dma(&mut self, offset: u64, value: u16) -> Result<(), CrimeBusError> {
        let port = usize::from(offset >= 0x1c000);
        let local = offset - if port == 0 { 0x18000 } else { 0x1c000 };
        let direction = usize::from(local & 0x20 != 0);
        let register = local & 0x1f;
        let channel = &mut self.isa.serial_dma[port * 2 + direction];
        match register {
            0x00 => channel.write_control(value),
            0x08 => channel.read_pointer = value & 0x0fe0,
            0x10 => channel.write_pointer = value & 0x0fe0,
            0x18 => {}
            _ => return Err(CrimeBusError::Address),
        }
        Ok(())
    }

    fn read_audio(&self, offset: u64) -> Result<u64, CrimeBusError> {
        match offset {
            0x00 => Ok(u64::from(self.audio.control)),
            0x08 => Ok(u64::from(self.audio.codec_control)),
            0x10 => Ok(u64::from(self.audio.codec_mask)),
            0x18 => Ok(u64::from(self.audio.codec_status)),
            0x20..=0x78 => {
                let channel = ((offset - 0x20) / 0x20) as usize;
                let register = (offset - 0x20) % 0x20;
                let channel = self
                    .audio
                    .channels
                    .get(channel)
                    .ok_or(CrimeBusError::Address)?;
                Ok(u64::from(match register {
                    0x00 => channel.control,
                    0x08 => channel.read_pointer,
                    0x10 => channel.write_pointer,
                    0x18 => channel.depth(),
                    _ => return Err(CrimeBusError::Address),
                }))
            }
            _ => Err(CrimeBusError::Address),
        }
    }

    fn write_audio(&mut self, offset: u64, value: u64) -> Result<(), CrimeBusError> {
        match offset {
            0x00 => self.audio.control = value as u32 & 0x01ff_ffff,
            0x08 => self.audio.codec_control = value as u32 & 0x00ff_ffff,
            0x10 => self.audio.codec_mask = value as u16,
            0x18 => {}
            0x20..=0x78 => {
                let channel = ((offset - 0x20) / 0x20) as usize;
                let register = (offset - 0x20) % 0x20;
                let channel = self
                    .audio
                    .channels
                    .get_mut(channel)
                    .ok_or(CrimeBusError::Address)?;
                match register {
                    0x00 => channel.set_control(value as u16),
                    0x08 => channel.read_pointer = value as u16 & 0x0fe0,
                    0x10 => channel.write_pointer = value as u16 & 0x0fe0,
                    0x18 => {}
                    _ => return Err(CrimeBusError::Address),
                }
            }
            _ => return Err(CrimeBusError::Address),
        }
        for (index, channel) in self.audio.channels.iter().enumerate() {
            self.interrupts
                .set_peripheral_source(2 + index as u8 * 2, channel.threshold_interrupt());
        }
        self.interrupts
            .set_peripheral_source(0, self.audio.codec_interrupt());
        Ok(())
    }

    fn read_ps2(&mut self, offset: u64) -> Result<u64, CrimeBusError> {
        let port = usize::from(offset >= 0x20);
        match offset & 0x1f {
            0x08 => Ok(u64::from(self.ps2[port].read_receive())),
            0x10 => Ok(u64::from(self.ps2[port].control())),
            0x18 => Ok(u64::from(self.ps2[port].status())),
            _ => Err(CrimeBusError::Address),
        }
    }

    fn write_ps2(&mut self, offset: u64, value: u8) -> Result<(), CrimeBusError> {
        let port = usize::from(offset >= 0x20);
        match offset & 0x1f {
            0x00 => {
                self.ps2[port].write_transmit(value);
                self.actions.push_back(MaceAction::Schedule {
                    delay: se_core::scheduler::SimDuration::new(self.timebase_hz / 10_000),
                    event: MaceEvent::Ps2Transmit {
                        epoch: self.epoch,
                        port: port as u8,
                    },
                });
            }
            0x10 => self.ps2[port].set_control(value),
            _ => return Err(CrimeBusError::Address),
        }
        self.update_ps2_interrupts();
        Ok(())
    }

    fn read_i2c(&self, offset: u64) -> Result<u64, CrimeBusError> {
        let port = usize::from(offset >= 0x20);
        match offset & 0x1f {
            0x00 | 0x08 => Ok(u64::from(self.i2c[port].config)),
            0x10 => Ok(u64::from(self.i2c[port].control)),
            0x18 => Ok(u64::from(self.i2c[port].data)),
            _ => Err(CrimeBusError::Address),
        }
    }

    fn write_i2c(&mut self, offset: u64, value: u8) -> Result<(), CrimeBusError> {
        let port = usize::from(offset >= 0x20);
        match offset & 0x1f {
            0x00 | 0x08 => {
                self.i2c[port].config = value & 0x3f;
                if value & 1 != 0 {
                    self.i2c[port].reset();
                }
            }
            0x10 => self.i2c[port].control = self.i2c[port].control & 0xb8 | value & 7,
            0x18 => {
                self.i2c[port].data = value;
                self.i2c[port].begin();
                self.actions.push_back(MaceAction::StartI2c(I2cTransaction {
                    id: self.next_transaction_id,
                    controller: self.id,
                    target: self.wiring.external_links.i2c[port],
                    address: value >> 1,
                    read_length: u16::from(self.i2c[port].control & 2 != 0),
                    write_data: if self.i2c[port].control & 2 == 0 {
                        vec![value]
                    } else {
                        vec![]
                    },
                    rate: if self.i2c[port].fast() {
                        I2cRate::Fast400Khz
                    } else {
                        I2cRate::Standard100Khz
                    },
                }));
                self.next_transaction_id = self
                    .next_transaction_id
                    .checked_add(1)
                    .ok_or(CrimeBusError::Unsupported)?;
            }
            _ => return Err(CrimeBusError::Address),
        }
        Ok(())
    }

    fn update_ps2_interrupts(&mut self) {
        self.interrupts
            .set_peripheral_source(9, self.ps2[0].interrupt());
        self.interrupts
            .set_peripheral_source(10, self.ps2[0].status() & 0x10 != 0);
        self.interrupts
            .set_peripheral_source(11, self.ps2[1].interrupt());
        self.interrupts
            .set_peripheral_source(12, self.ps2[1].status() & 0x10 != 0);
        self.push_interrupt_posts();
    }

    fn push_interrupt_posts(&mut self) {
        for (bit, asserted) in self.interrupts.take_changed_posts() {
            let Some(id) = self.allocate_cmi_id() else {
                self.terminal_error = Some(MaceError::TransactionIdOverflow);
                return;
            };
            self.pending_cmi.insert(id);
            self.actions.push_back(MaceAction::Trace(MaceTraceEvent {
                level: TraceLevel::Debug,
                target: trace::INTERRUPT,
                event: "post",
                fields: vec![
                    MaceTraceField {
                        key: "slot",
                        value: MaceTraceValue::U64(u64::from(bit)),
                    },
                    MaceTraceField {
                        key: "asserted",
                        value: MaceTraceValue::Bool(asserted),
                    },
                ],
            }));
            self.actions
                .push_back(MaceAction::StartCmi(CrimeCmiTransaction {
                    id,
                    controller: self.id,
                    target: self.wiring.crime,
                    operation: CrimeLinkOperation::InterruptPost(CrimeInterruptPost {
                        interrupt_bit: bit,
                        asserted,
                    }),
                }));
        }
    }

    fn allocate_isa_id(&mut self) -> Option<IsaTransactionId> {
        let id = IsaTransactionId::new(self.next_transaction_id);
        self.next_transaction_id = self.next_transaction_id.checked_add(1)?;
        Some(id)
    }

    fn allocate_cmi_id(&mut self) -> Option<CrimeTransactionId> {
        let id = CrimeTransactionId::new(self.next_transaction_id);
        self.next_transaction_id = self.next_transaction_id.checked_add(1)?;
        Some(id)
    }

    fn complete_cmi(&mut self, completion: CrimeCmiCompletion) {
        if !self.pending_cmi.remove(&completion.id) {
            self.terminal_error = Some(MaceError::UnexpectedCmiCompletion(completion.id));
        }
    }

    fn complete_isa(&mut self, completion: IsaCompletion) {
        let Some(pending) = self.pending_isa.remove(&completion.id) else {
            self.terminal_error = Some(MaceError::UnexpectedIsaCompletion(completion.id));
            return;
        };
        self.actions
            .push_back(MaceAction::CompleteCmiDevice(CrimeCmiCompletion {
                id: pending.cmi_id,
                result: from_isa_result(completion.result),
                memory_fault: None,
            }));
    }
}

impl Component for Mace {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {
        self.power_on(SimTime::ZERO);
    }
}

impl BusDeviceRole<CrimeCmiTransaction> for Mace {
    type Response = CrimeLinkDeviceResponse<CrimeCmiCompletion>;
    fn accept(&mut self, transaction: CrimeCmiTransaction) -> Self::Response {
        self.accept_cmi(transaction)
    }
}

impl BusControllerRole<CrimeCmiCompletion> for Mace {
    fn complete(&mut self, completion: CrimeCmiCompletion) {
        self.complete_cmi(completion);
    }
}

impl BusControllerRole<IsaCompletion> for Mace {
    fn complete(&mut self, completion: IsaCompletion) {
        self.complete_isa(completion);
    }
}

impl BusControllerRole<PciCompletion> for Mace {
    fn complete(&mut self, completion: PciCompletion) {
        let id = CrimeTransactionId::new(completion.id);
        if !self.pending_cmi.remove(&id) {
            self.terminal_error = Some(MaceError::UnexpectedCmiCompletion(id));
            return;
        }
        let result = match completion.status {
            PciStatus::Complete => {
                if completion.data.is_empty() {
                    Ok(CrimeCompletionPayload::WriteComplete)
                } else {
                    let mut data = completion.data;
                    data.reverse();
                    Ok(CrimeCompletionPayload::ReadData(data))
                }
            }
            PciStatus::Retry => Err(CrimeBusError::Timeout),
            PciStatus::MasterAbort | PciStatus::TargetAbort | PciStatus::ParityError => {
                Err(CrimeBusError::Access)
            }
        };
        self.actions
            .push_back(MaceAction::CompleteCmiDevice(CrimeCmiCompletion {
                id,
                result,
                memory_fault: None,
            }));
    }
}

impl BusControllerRole<I2cCompletion> for Mace {
    fn complete(&mut self, completion: I2cCompletion) {
        let (id, acknowledged, bus_error) = match completion {
            I2cCompletion::Ack { id, data } => {
                if let Some(value) = data.first() {
                    self.i2c[0].data = *value;
                }
                (id, true, false)
            }
            I2cCompletion::Nack { id } => (id, false, false),
            I2cCompletion::ArbitrationLost { id } | I2cCompletion::BusError { id } => {
                (id, false, true)
            }
        };
        let _ = id;
        self.i2c[0].complete(acknowledged, bus_error);
    }
}

impl BusDeviceRole<IrqDelivery> for Mace {
    type Response = Result<(), MaceError>;
    fn accept(&mut self, delivery: IrqDelivery) -> Self::Response {
        match delivery.input.get() {
            0 => self.interrupts.set_peripheral_source(8, delivery.asserted),
            1 => self.interrupts.set_peripheral_source(20, delivery.asserted),
            2 => self.interrupts.set_peripheral_source(26, delivery.asserted),
            3 => self.interrupts.set_peripheral_source(16, delivery.asserted),
            8..=15 => {
                let input = delivery.input.get() - 8;
                self.interrupts.set_group(
                    match input {
                        0 => MaceInterruptGroup::Pci0,
                        1 => MaceInterruptGroup::Pci1,
                        2 => MaceInterruptGroup::Pci2,
                        3 => MaceInterruptGroup::Pci3,
                        4 => MaceInterruptGroup::Pci4,
                        5 => MaceInterruptGroup::Pci5,
                        6 => MaceInterruptGroup::Pci6,
                        _ => MaceInterruptGroup::Pci7,
                    },
                    delivery.asserted && self.pci.pci_irq_enabled(input as u8),
                );
            }
            _ => return Err(MaceError::UnsupportedIrqInput(delivery.input)),
        }
        self.push_interrupt_posts();
        Ok(())
    }
}

impl BusDeviceRole<MediaTransaction> for Mace {
    type Response = Result<(), MaceError>;
    fn accept(&mut self, transaction: MediaTransaction) -> Self::Response {
        self.accept_host_input(transaction)
    }
}

fn complete_cmi_error(
    id: CrimeTransactionId,
    error: CrimeBusError,
) -> CrimeLinkDeviceResponse<CrimeCmiCompletion> {
    CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
        id,
        result: Err(error),
        memory_fault: None,
    })
}

fn to_isa_transfer(transfer: CrimeTransfer) -> IsaTransfer {
    match transfer {
        CrimeTransfer::Read { length } => IsaTransfer::Read {
            length: length as u8,
        },
        CrimeTransfer::Write { data, byte_enable } => IsaTransfer::Write { data, byte_enable },
    }
}

fn from_isa_result(
    result: Result<IsaCompletionPayload, IsaBusError>,
) -> Result<CrimeCompletionPayload, CrimeBusError> {
    match result {
        Ok(IsaCompletionPayload::ReadData(data)) => Ok(CrimeCompletionPayload::ReadData(data)),
        Ok(IsaCompletionPayload::WriteComplete) => Ok(CrimeCompletionPayload::WriteComplete),
        Err(IsaBusError::Address) => Err(CrimeBusError::Address),
        Err(IsaBusError::Access) => Err(CrimeBusError::Access),
        Err(IsaBusError::ReadOnly | IsaBusError::Unsupported) => Err(CrimeBusError::Unsupported),
    }
}

fn crime_transfer_parts(transfer: &CrimeTransfer) -> (Vec<u8>, Vec<bool>) {
    match transfer {
        CrimeTransfer::Read { length } => (
            vec![0; usize::from(*length)],
            vec![true; usize::from(*length)],
        ),
        CrimeTransfer::Write { data, byte_enable } => (data.clone(), byte_enable.clone()),
    }
}

fn encode_value(value: u64, width: usize) -> Vec<u8> {
    value.to_be_bytes()[8 - width..].to_vec()
}
fn decode_value(data: &[u8]) -> u64 {
    data.iter()
        .fold(0, |value, &byte| value << 8 | u64::from(byte))
}
fn bytes_to_u64(data: &[u8]) -> u64 {
    data.iter()
        .take(8)
        .fold(0, |value, &byte| value << 8 | u64::from(byte))
}
fn u64_to_six_bytes(value: u64) -> [u8; 6] {
    value.to_be_bytes()[2..].try_into().expect("six bytes")
}

fn media_port_name(port: MediaPort) -> &'static str {
    match port {
        MediaPort::VideoInputAb => "video_input_ab",
        MediaPort::VideoInputCd => "video_input_cd",
        MediaPort::VideoOutput => "video_output",
        MediaPort::AudioInput => "audio_input",
        MediaPort::AudioOutput1 => "audio_output_1",
        MediaPort::AudioOutput2 => "audio_output_2",
        MediaPort::Ethernet => "ethernet",
        MediaPort::Keyboard => "keyboard",
        MediaPort::Mouse => "mouse",
        MediaPort::Serial0 => "serial_0",
        MediaPort::Serial1 => "serial_1",
        MediaPort::Parallel => "parallel",
    }
}

#[cfg(test)]
mod tests;
