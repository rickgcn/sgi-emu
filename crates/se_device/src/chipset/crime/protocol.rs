//! Owned protocols used by CRIME and its communication domains.

use core::fmt;

use se_core::component::ComponentId;
use se_core::scheduler::{SimDuration, SimTime};
use se_core::tracing::TraceLevel;

use crate::bus::irq::{IrqOutput, IrqTransaction};
use crate::cpu::execution::protocol::{ExecutionCompletion, ExecutionTransaction};
use crate::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};

/// CRIME's single processor interrupt output.
pub const CRIME_IRQ_OUTPUT: IrqOutput = IrqOutput::new(0);

/// Identifier correlating one CRIME transaction and completion.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeSysAdRequest {
    /// Simulated delivery time.
    pub time: SimTime,

    /// Original correlated CPU transaction.
    pub transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
}

/// Origin of a request competing for CRIME memory service.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

/// Byte-oriented operation transported through a CRIME bus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrimeTransfer {
    /// Reads a contiguous range of bytes.
    Read {
        /// Requested byte count.
        length: u16,
    },

    /// Writes selected bytes from a contiguous payload.
    Write {
        /// Physical byte-lane data in ascending address order.
        data: Vec<u8>,

        /// One bit per payload byte; set bits enable their byte lanes.
        byte_enable: Vec<bool>,
    },
}

impl CrimeTransfer {
    /// Returns the transfer length in bytes.
    pub fn length(&self) -> usize {
        match self {
            Self::Read { length } => usize::from(*length),
            Self::Write { data, .. } => data.len(),
        }
    }
}

/// Reason an otherwise valid memory request must not assert SDRAM bank selects.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CrimeMemoryInhibitReason {
    /// The Rendering Engine generated a request from an invalid TLB entry.
    InvalidRenderTlb,
}

/// Bank-selection behavior attached to one CRIME memory request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrimeCompletionPayload {
    /// Read data in ascending physical byte-address order.
    ReadData(Vec<u8>),

    /// A write completed.
    WriteComplete,
}

/// Error reported by a CRIME bus or target.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CrimeMemoryFault {
    /// No programmed bank-control register selected the request address.
    Address,

    /// Read data contained an ECC error that could not be corrected.
    UncorrectableEcc,
}

/// Data and diagnostic state produced by one CRIME memory operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeMemoryOutcome {
    /// Data or write acknowledgement returned to the requesting client.
    pub payload: CrimeCompletionPayload,

    /// Hardware-visible fault reported independently of bus transport.
    pub fault: Option<CrimeMemoryFault>,

    /// ECC or address information captured while servicing the request.
    pub diagnostic: Option<CrimeMemoryDiagnostic>,
}

/// Completion returned through the CRIME memory domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeMemoryCompletion {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Completion result. Hardware memory faults are carried by the outcome.
    pub result: Result<CrimeMemoryOutcome, CrimeBusError>,
}

/// Software-visible memory diagnostic associated with a completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimePioRequest {
    /// Peer-local byte address.
    pub address: u64,

    /// Data transfer.
    pub transfer: CrimeTransfer,
}

/// DMA request emitted by a peer device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeDmaRequest {
    /// Main-memory byte address.
    pub address: u64,

    /// Data transfer.
    pub transfer: CrimeTransfer,
}

/// Interrupt transaction posted by a peer device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CrimeInterruptPost {
    /// CRIME hardware-interrupt bit to assert or deassert.
    pub interrupt_bit: u8,

    /// New level.
    pub asserted: bool,
}

/// Operation transported through a CRIME peer link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrimeLinkOperation {
    /// CPU-programmed I/O.
    Pio(CrimePioRequest),

    /// Peer-initiated main-memory access.
    Dma(CrimeDmaRequest),

    /// Peer-initiated interrupt update.
    InterruptPost(CrimeInterruptPost),
}

/// CMI transaction between CRIME and MACE.
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeCmiCompletion {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Completion result.
    pub result: Result<CrimeCompletionPayload, CrimeBusError>,

    /// Memory fault associated with a peer-initiated DMA operation.
    pub memory_fault: Option<CrimeMemoryFault>,
}

/// CGI transaction between CRIME and GBE.
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeCgiCompletion {
    /// Correlation identifier.
    pub id: CrimeTransactionId,

    /// Completion result.
    pub result: Result<CrimeCompletionPayload, CrimeBusError>,

    /// Memory fault associated with a peer-initiated DMA operation.
    pub memory_fault: Option<CrimeMemoryFault>,
}

/// Immediate response from a link device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrimeLinkDeviceResponse<C> {
    /// The target completed synchronously.
    Complete(C),

    /// The target retained the request and will complete through a later action.
    Deferred,
}

/// Result of submitting a transaction to a stateful CRIME bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrimeBusDisposition {
    /// The bus was already scheduled and only queued the transaction.
    Queued,

    /// The bus transitioned from idle and needs a service event.
    QueuedAndNeedsService {
        /// Delay before the first arbitration opportunity.
        delay: SimDuration,
    },
}

/// Action emitted by a stateful bus.
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrimeCpuSignal {
    /// Requests an R5000 warm reset.
    WarmReset,

    /// Requests a board hard reset.
    HardReset,
}

/// Owned trace value generated by CRIME.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrimeTraceValue {
    /// Boolean value.
    Bool(bool),
    /// Unsigned integer.
    U64(u64),
    /// Hexadecimal unsigned integer.
    Hex64(u64),
    /// Stable protocol string.
    String(&'static str),
}

/// Ordered trace field generated by CRIME.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeTraceField {
    /// Stable field name.
    pub key: &'static str,

    /// Field value.
    pub value: CrimeTraceValue,
}

/// Structured trace event generated by CRIME.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeTraceEvent {
    /// Importance level.
    pub level: TraceLevel,

    /// Stable trace target.
    pub target: &'static str,

    /// Stable event name.
    pub event: &'static str,

    /// Ordered fields.
    pub fields: Vec<CrimeTraceField>,
}

/// Action emitted while polling CRIME.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    CompleteSysAd(ExecutionCompletion<Mips4ExecutionCompletion>),

    /// Drives CRIME's interrupt output through the attached IRQ bus.
    SetIrq(IrqTransaction),

    /// Delivers an external CPU signal.
    SignalCpu(CrimeCpuSignal),

    /// Delivers a control signal to the SDRAM component.
    SignalMemory(CrimeSdramSignal),

    /// Emits a structured trace fact.
    Trace(CrimeTraceEvent),
}

/// Result of polling CRIME.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrimePoll {
    /// One pending action.
    Action(CrimeAction),

    /// No action is pending.
    Idle,
}
