//! Owned protocols used by CRIME and its communication domains.

use core::fmt;
use core::ops::{Deref, DerefMut};

use se_core::component::ComponentId;
use se_core::scheduler::{SimDuration, SimTime};
use se_core::tracing::OwnedTraceEvent;

use crate::bus::irq::{IrqOutput, IrqTransaction};
use crate::bus::transfer::{
    CompactByteEnable, CompactByteEnableView, CompactData, CompactTransfer, CompactTransferView,
};

/// CRIME's single processor interrupt output.
pub const CRIME_IRQ_OUTPUT: IrqOutput = IrqOutput::new(0);

/// Identifier correlating one CRIME transaction and completion.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct CrimeTransactionId(u128);

impl CrimeTransactionId {
    /// Creates an identifier from its raw value.
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Returns the raw identifier.
    pub const fn get(self) -> u128 {
        self.0
    }
}

impl fmt::Display for CrimeTransactionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "crime-transaction:{}", self.0)
    }
}

/// CPU request delivered to CRIME by the SysAD bus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeSysAdRequest {
    /// Correlation identifier owned by the SysAD domain.
    pub id: CrimeTransactionId,

    /// Simulated delivery time.
    pub time: SimTime,

    /// Physical byte address.
    pub address: u64,

    /// Physical byte transfer.
    pub transfer: CrimeTransfer,
}

/// Origin of a request competing for CRIME memory service.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum CrimeMemoryClient {
    /// GBE display traffic.
    Gbe,
    /// MACE I/O traffic.
    Mace,
    /// VICE traffic.
    Vice,
    /// CRIME rendering-engine traffic.
    Render,
    /// CPU traffic forwarded by the processor interface.
    Cpu,
}

/// Owned byte payload optimized for CRIME's common subblock transfers.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeData(CompactData);

impl CrimeData {
    /// Creates zero-filled data of the requested length.
    pub fn zeroed(length: usize) -> Self {
        Self(CompactData::zeroed(length))
    }

    /// Returns whether the payload spilled beyond inline storage.
    pub fn spilled(&self) -> bool {
        self.0.spilled()
    }

    pub(crate) fn from_compact(value: CompactData) -> Self {
        Self(value)
    }
}

impl Deref for CrimeData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CrimeData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AsRef<[u8]> for CrimeData {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl From<Vec<u8>> for CrimeData {
    fn from(value: Vec<u8>) -> Self {
        Self(value.into())
    }
}

impl<const N: usize> From<[u8; N]> for CrimeData {
    fn from(value: [u8; N]) -> Self {
        Self(value.into_iter().collect())
    }
}

impl FromIterator<u8> for CrimeData {
    fn from_iter<T: IntoIterator<Item = u8>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl PartialEq<Vec<u8>> for CrimeData {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_ref() == other.as_slice()
    }
}

/// Owned byte-enable payload optimized for CRIME's common subblock transfers.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeByteEnable(CompactByteEnable);

impl CrimeByteEnable {
    /// Creates a payload with every lane enabled.
    pub fn enabled(length: usize) -> Self {
        Self(CompactByteEnable::enabled(length))
    }

    /// Returns whether the payload spilled beyond inline storage.
    pub fn spilled(&self) -> bool {
        self.0.spilled()
    }
}

impl CrimeByteEnable {
    /// Returns the number of represented byte lanes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether no byte lanes are represented.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns one enable bit, or `None` when the lane is out of range.
    pub fn is_enabled(&self, index: usize) -> Option<bool> {
        self.0.is_enabled(index)
    }

    /// Iterates over enable bits in ascending lane order.
    pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        self.0.iter()
    }
}

impl From<Vec<bool>> for CrimeByteEnable {
    fn from(value: Vec<bool>) -> Self {
        Self(value.into_iter().collect())
    }
}

impl<const N: usize> From<[bool; N]> for CrimeByteEnable {
    fn from(value: [bool; N]) -> Self {
        Self(value.into_iter().collect())
    }
}

impl FromIterator<bool> for CrimeByteEnable {
    fn from_iter<T: IntoIterator<Item = bool>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl PartialEq<Vec<bool>> for CrimeByteEnable {
    fn eq(&self, other: &Vec<bool>) -> bool {
        self.iter().eq(other.iter().copied())
    }
}

/// Byte-oriented operation transported through a CRIME bus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeTransfer(CompactTransfer);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Borrowed byte-enable view for a CRIME write transfer.
pub struct CrimeByteEnableView<'a>(CompactByteEnableView<'a>);

impl<'a> CrimeByteEnableView<'a> {
    /// Returns the number of represented byte lanes.
    pub fn len(self) -> usize {
        self.0.len()
    }

    /// Returns whether no byte lanes are represented.
    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Returns one enable bit, or `None` when the lane is out of range.
    pub fn is_enabled(self, index: usize) -> Option<bool> {
        self.0.is_enabled(index)
    }

    /// Iterates over enable bits in ascending lane order.
    pub fn iter(self) -> impl Iterator<Item = bool> + 'a {
        self.0.iter()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Borrowed view of a CRIME byte transfer.
pub enum CrimeTransferView<'a> {
    /// Read request with a byte length.
    Read {
        /// Requested byte count.
        length: u16,
    },
    /// Write request with independent data and byte-enable lengths.
    Write {
        /// Bytes in ascending address order.
        data: &'a [u8],
        /// Per-byte write enables.
        byte_enable: CrimeByteEnableView<'a>,
    },
}

impl CrimeTransfer {
    /// Creates a read transfer without write-side storage.
    pub const fn read(length: u16) -> Self {
        Self(CompactTransfer::read(length))
    }

    /// Creates a write transfer while preserving independent payload lengths.
    pub fn write(data: CrimeData, byte_enable: CrimeByteEnable) -> Self {
        Self(CompactTransfer::write(data.0, byte_enable.0))
    }

    /// Returns the transfer length in bytes.
    pub fn length(&self) -> usize {
        self.0.length()
    }

    /// Borrows the strongly typed transfer contents.
    pub fn view(&self) -> CrimeTransferView<'_> {
        match self.0.view() {
            CompactTransferView::Read { length } => CrimeTransferView::Read { length },
            CompactTransferView::Write { data, byte_enable } => CrimeTransferView::Write {
                data,
                byte_enable: CrimeByteEnableView(byte_enable),
            },
        }
    }

    pub(crate) fn into_compact(self) -> CompactTransfer {
        self.0
    }
}

/// Reason an otherwise valid memory request must not assert SDRAM bank selects.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeMemoryInhibitReason {
    /// The Rendering Engine generated a request from an invalid TLB entry.
    InvalidRenderTlb,
}

/// Bank-selection behavior attached to one CRIME memory request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeMemoryBankSelect {
    /// Decode the request through the programmable MIU bank controls.
    Decode,

    /// Preserve arbitration and completion ordering without selecting SDRAM.
    Inhibited {
        /// Hardware reason for suppressing all external bank selects.
        reason: CrimeMemoryInhibitReason,
    },
}

/// Request routed through the CRIME memory domain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeMemoryTransaction {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Simulated submission time used for lazy refresh accounting.
    pub time: SimTime,

    /// Component that receives the eventual completion.
    pub controller: ComponentId,

    /// Arbitration client.
    pub client: CrimeMemoryClient,

    /// CRIME memory-domain byte address.
    pub address: u64,

    /// Whether the MIU may decode and assert an external SDRAM bank select.
    pub bank_select: CrimeMemoryBankSelect,

    /// Whether read-side ECC checking is bypassed.
    pub no_ecc: bool,

    /// Data transfer.
    pub transfer: CrimeTransfer,
}

/// Successful data returned by a CRIME bus target.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeCompletionPayload {
    /// Read data in ascending physical byte-address order.
    ReadData(CrimeData),

    /// A write completed.
    WriteComplete,
}

/// Error reported by a CRIME bus or target.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeBusError {
    /// No target decoded the requested address.
    Address,

    /// Transfer width or alignment is invalid.
    Access,

    /// Target is mapped but unsupported under the selected policy.
    Unsupported,

    /// A correlated transaction timed out.
    Timeout,
}

/// Hardware-visible fault reported by the CRIME memory controller.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeMemoryFault {
    /// No programmed bank-control register selected the request address.
    Address,

    /// Read data contained an ECC error that could not be corrected.
    UncorrectableEcc,
}

/// Data and diagnostic state produced by one CRIME memory operation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeMemoryOutcome {
    /// Data or write acknowledgement returned to the requesting client.
    pub payload: CrimeCompletionPayload,

    /// Hardware-visible fault reported independently of bus transport.
    pub fault: Option<CrimeMemoryFault>,

    /// ECC or address information captured while servicing the request.
    diagnostic: Option<Box<CrimeMemoryDiagnostic>>,
}

impl CrimeMemoryOutcome {
    /// Creates a memory outcome without exposing its cold-path storage.
    pub fn new(
        payload: CrimeCompletionPayload,
        fault: Option<CrimeMemoryFault>,
        diagnostic: Option<CrimeMemoryDiagnostic>,
    ) -> Self {
        Self {
            payload,
            fault,
            diagnostic: diagnostic.map(Box::new),
        }
    }

    /// Borrows the optional hardware diagnostic.
    pub fn diagnostic(&self) -> Option<&CrimeMemoryDiagnostic> {
        self.diagnostic.as_deref()
    }

    /// Removes and returns the optional hardware diagnostic.
    pub fn take_diagnostic(&mut self) -> Option<CrimeMemoryDiagnostic> {
        self.diagnostic.take().map(|diagnostic| *diagnostic)
    }

    /// Splits the outcome into its public protocol values.
    pub fn into_parts(
        self,
    ) -> (
        CrimeCompletionPayload,
        Option<CrimeMemoryFault>,
        Option<CrimeMemoryDiagnostic>,
    ) {
        (
            self.payload,
            self.fault,
            self.diagnostic.map(|value| *value),
        )
    }
}

/// Completion returned through the CRIME memory domain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeMemoryCompletion {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Completion result. Hardware memory faults are carried by the outcome.
    pub result: Result<CrimeMemoryOutcome, CrimeBusError>,
}

/// Completion returned to the SysAD domain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeSysAdCompletion {
    /// Correlation identifier copied from the request.
    pub id: CrimeTransactionId,

    /// CRIME transfer result.
    pub result: Result<CrimeCompletionPayload, CrimeBusError>,
}

/// Software-visible memory diagnostic associated with a completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeMemoryDiagnostic {
    /// Memory-domain address of the affected 256-bit word.
    pub address: u64,

    /// Four packed ECC syndrome bytes.
    pub syndrome: u32,

    /// Four packed regenerated ECC check bytes.
    pub check: u32,

    /// Whether hardware corrected the returned data.
    pub corrected: bool,

    /// Whether the failing memory operation was a write.
    pub write: bool,

    /// Whether the error occurred during read-modify-write.
    pub read_modify_write: bool,
}

/// CPU or Rendering Engine change sent to the SDRAM device.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeSdramSignal {
    /// Replaces one programmable bank-control register.
    SetBankControl {
        /// External bank index.
        bank: u8,

        /// Nine software-visible control bits.
        value: u16,
    },

    /// Updates read checking and replacement-check generation.
    SetEccControl {
        /// Whether read and RMW checking is enabled.
        enabled: bool,

        /// Whether writes use the replacement byte.
        use_replacement: bool,

        /// Replacement byte replicated across the four lanes.
        replacement: u8,
    },

    /// Clears all stored data and ECC to the power-on state.
    PowerOn,

    /// Preserves data while invalidating transient access state.
    HardReset,
}

/// CPU PIO request transported over CMI or CGI.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimePioRequest {
    /// Peer-local byte address.
    pub address: u64,

    /// Data transfer.
    pub transfer: CrimeTransfer,
}

/// DMA request emitted by a peer device.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeDmaRequest {
    /// Main-memory byte address.
    pub address: u64,

    /// Data transfer.
    pub transfer: CrimeTransfer,
}

/// Interrupt transaction posted by a peer device.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeInterruptPost {
    /// CRIME hardware-interrupt bit to assert or deassert.
    pub interrupt_bit: u8,

    /// New level.
    pub asserted: bool,
}

/// Operation transported through a CRIME peer link.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeLinkOperation {
    /// CPU-programmed I/O.
    Pio(CrimePioRequest),

    /// Peer-initiated main-memory access.
    Dma(CrimeDmaRequest),

    /// Peer-initiated interrupt update.
    InterruptPost(CrimeInterruptPost),
}

/// CMI transaction between CRIME and MACE.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeCmiTransaction {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Component receiving completion.
    pub controller: ComponentId,

    /// Component accepting the request.
    pub target: ComponentId,

    /// Protocol operation.
    pub operation: CrimeLinkOperation,
}

/// CMI completion.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeCmiCompletion {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Completion result.
    pub result: Result<CrimeCompletionPayload, CrimeBusError>,

    /// Memory fault associated with a peer-initiated DMA operation.
    pub memory_fault: Option<CrimeMemoryFault>,
}

/// CGI transaction between CRIME and GBE.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeCgiTransaction {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Component receiving completion.
    pub controller: ComponentId,

    /// Component accepting the request.
    pub target: ComponentId,

    /// Protocol operation.
    pub operation: CrimeLinkOperation,
}

/// CGI completion.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeCgiCompletion {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Completion result.
    pub result: Result<CrimeCompletionPayload, CrimeBusError>,

    /// Memory fault associated with a peer-initiated DMA operation.
    pub memory_fault: Option<CrimeMemoryFault>,
}

/// Immediate response from a link device.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeLinkDeviceResponse<C> {
    /// The target completed synchronously.
    Complete(C),

    /// The target retained the request and will complete through a later action.
    Deferred,
}

/// Result of submitting a transaction to a stateful CRIME bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeBusDisposition {
    /// The bus was already scheduled and only queued the transaction.
    Queued,

    /// The bus transitioned from idle and needs a service event.
    QueuedAndNeedsService {
        /// Delay before the first arbitration opportunity.
        delay: SimDuration,

        /// Reset epoch used by the corresponding service event.
        epoch: u64,
    },
}

/// Action emitted by a stateful bus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeBusAction<T, C> {
    /// Delivers one request to a bus device.
    Deliver {
        /// Destination component.
        target: ComponentId,

        /// Delivered request.
        transaction: T,
    },

    /// Returns a completion to a controller.
    Complete {
        /// Destination controller.
        controller: ComponentId,

        /// Delivered completion.
        completion: C,
    },

    /// Requests another service event.
    ScheduleService {
        /// Delay before service.
        delay: SimDuration,
    },

    /// No bus work is ready.
    Idle,
}

/// CRIME-internal scheduled event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeEvent {
    /// First or second CRIME 1.1 watchdog threshold.
    Watchdog {
        /// Epoch that armed the watchdog.
        epoch: u64,

        /// One for warm reset and two for hard reset.
        stage: u8,
    },

    /// Continues a bounded Rendering Engine job.
    RenderStep {
        /// Rendering reset epoch.
        epoch: u64,
    },
}

/// CPU-facing signal emitted by CRIME.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeCpuSignal {
    /// Requests an R5000 warm reset.
    WarmReset,

    /// Requests a board hard reset.
    HardReset,
}

/// Action emitted while polling CRIME.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum CrimeAction {
    /// Schedules an internal CRIME event.
    Schedule {
        /// Delivery delay.
        delay: SimDuration,

        /// Event payload.
        event: CrimeEvent,
    },

    /// Initiates a memory-domain transaction.
    StartMemory(CrimeMemoryTransaction),

    /// Initiates a CMI transaction.
    StartCmi(CrimeCmiTransaction),

    /// Initiates a CGI transaction.
    StartCgi(CrimeCgiTransaction),

    /// Completes a deferred request accepted from CMI.
    CompleteCmiDevice(CrimeCmiCompletion),

    /// Completes a deferred request accepted from CGI.
    CompleteCgiDevice(CrimeCgiCompletion),

    /// Completes the outstanding CPU transaction.
    CompleteSysAd(CrimeSysAdCompletion),

    /// Drives CRIME's interrupt output through the attached IRQ bus.
    SetIrq(IrqTransaction),

    /// Delivers an external CPU signal.
    SignalCpu(CrimeCpuSignal),

    /// Delivers a control signal to the SDRAM component.
    SignalMemory(CrimeSdramSignal),

    /// Emits a structured trace fact.
    Trace(Box<OwnedTraceEvent>),
}

/// Result of polling CRIME.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum CrimePoll {
    /// One pending action.
    Action(CrimeAction),

    /// No action is pending.
    Idle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_payloads_inline_eight_bytes_and_spill_larger_transfers() {
        for length in [0, 1, 4, 8, 9, 32, 33, 256, 512] {
            let data = CrimeData::zeroed(length);
            let enables = CrimeByteEnable::enabled(length);
            assert_eq!(data.len(), length);
            assert_eq!(enables.len(), length);
            assert_eq!(data.spilled(), length > 8);
            assert_eq!(enables.spilled(), length > 8);
        }
        for length in [0, 1, 4, 8, 9, 32, 33, 256, 512] {
            let data = CrimeData::from(vec![0; length]);
            assert_eq!(data.spilled(), length > 8);
        }
    }

    #[test]
    fn transfer_views_preserve_data_enable_lengths_and_bits() {
        for length in [0, 1, 4, 8, 9, 32, 33, 256, 512] {
            let data: CrimeData = (0..length).map(|index| index as u8).collect();
            let enables: CrimeByteEnable = (0..length + 1).map(|index| index % 3 != 0).collect();
            let transfer = CrimeTransfer::write(data, enables);
            let CrimeTransferView::Write { data, byte_enable } = transfer.view() else {
                panic!("write transfer changed variant");
            };
            assert_eq!(data.len(), length);
            assert_eq!(byte_enable.len(), length + 1);
            assert_eq!(
                byte_enable.iter().collect::<Vec<_>>(),
                (0..length + 1)
                    .map(|index| index % 3 != 0)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn compact_crime_protocols_meet_hot_path_size_limits() {
        assert!(core::mem::size_of::<CrimeSysAdRequest>() <= 64);
        assert!(core::mem::size_of::<CrimeSysAdCompletion>() <= 64);
        assert!(core::mem::size_of::<CrimeMemoryTransaction>() <= 64);
        assert!(core::mem::size_of::<CrimeMemoryCompletion>() <= 64);
        assert!(core::mem::size_of::<CrimeCmiTransaction>() <= 64);
        assert!(core::mem::size_of::<CrimeCgiTransaction>() <= 64);
        assert!(
            core::mem::size_of::<CrimeBusAction<CrimeMemoryTransaction, CrimeMemoryCompletion>>()
                <= 96
        );
    }

    #[test]
    fn memory_outcome_hides_and_recovers_cold_diagnostics() {
        let diagnostic = CrimeMemoryDiagnostic {
            address: 0x1234,
            syndrome: 1,
            check: 2,
            corrected: true,
            write: false,
            read_modify_write: false,
        };
        let mut outcome = CrimeMemoryOutcome::new(
            CrimeCompletionPayload::WriteComplete,
            None,
            Some(diagnostic),
        );
        assert_eq!(outcome.diagnostic(), Some(&diagnostic));
        assert_eq!(outcome.take_diagnostic(), Some(diagnostic));
        assert_eq!(outcome.diagnostic(), None);

        let outcome = CrimeMemoryOutcome::new(
            CrimeCompletionPayload::WriteComplete,
            Some(CrimeMemoryFault::Address),
            Some(diagnostic),
        );
        assert_eq!(
            outcome.into_parts(),
            (
                CrimeCompletionPayload::WriteComplete,
                Some(CrimeMemoryFault::Address),
                Some(diagnostic)
            )
        );
    }

    #[test]
    fn hardware_actions_are_not_sized_by_inline_trace_fields() {
        assert!(core::mem::size_of::<CrimeAction>() <= 96);
    }
}
