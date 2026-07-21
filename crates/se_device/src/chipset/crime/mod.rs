//! SGI CRIME 1.1 chipset model.
//!
//! CRIME is a device on the processor SysAD domain and a controller on its
//! memory, CMI, and CGI domains. The buses remain distinct components so that
//! routing, arbitration, ordering, and delay stay in [`se_core::role::BusRole`]
//! implementations rather than the chipset component.

mod clock;
pub mod config;
pub mod iou;
pub mod memory;
pub mod piu;
pub mod protocol;
pub mod registers;
pub mod render;
pub mod trace;

use core::fmt;
use std::collections::{BTreeSet, VecDeque};

use se_core::component::{
    Component, ComponentId, ComponentStateError, validate_component_state_id,
};
use se_core::role::{BusControllerRole, BusDeviceRole};
use se_core::scheduler::SimTime;
use se_core::tracing::{
    OwnedTraceEvent, OwnedTraceField, OwnedTraceFields, OwnedTraceValue, TraceInterest, TraceLevel,
};

use self::clock::{CrimeClock, CrimeClockState};
use self::config::{CrimeAccessPolicy, CrimeConfig, CrimeConfigError};
use self::piu::{CRIME_MASTER_FREQUENCY_HZ, CrimePiu, CrimePiuState, PiuEffect};
use self::protocol::{
    CRIME_IRQ_OUTPUT, CrimeAction, CrimeBusError, CrimeCgiCompletion, CrimeCgiTransaction,
    CrimeCmiCompletion, CrimeCmiTransaction, CrimeCompletionPayload, CrimeCpuSignal, CrimeData,
    CrimeDmaRequest, CrimeEvent, CrimeInterruptPost, CrimeLinkDeviceResponse, CrimeLinkOperation,
    CrimeMemoryBankSelect, CrimeMemoryClient, CrimeMemoryCompletion, CrimeMemoryFault,
    CrimeMemoryInhibitReason, CrimeMemoryOutcome, CrimeMemoryTransaction, CrimePioRequest,
    CrimePoll, CrimeSdramSignal, CrimeSysAdCompletion, CrimeSysAdRequest, CrimeSysAdRoute,
    CrimeTransactionId, CrimeTransfer, CrimeTransferView,
};
use self::render::{
    CrimeRender, CrimeRenderError, CrimeRenderState, RenderAccessError, RenderInterruptEffect,
    RenderMemoryDestination, RenderMemoryRequest, RenderNotice, RenderProgress, RenderWriteError,
};
use crate::bus::irq::{IrqSource, IrqTransaction};
use crate::common::pending::InlineMap8;

const LOW_MEMORY_END: u64 = 0x1000_0000;
const FRAMEBUFFER_START: u64 = 0x1000_0000;
const FRAMEBUFFER_END: u64 = 0x1200_0000;
const DEPTH_START: u64 = 0x1200_0000;
const DEPTH_END: u64 = 0x1400_0000;
const GBE_START: u64 = 0x1600_0000;
const GBE_END: u64 = 0x1700_0000;
const VICE_START: u64 = 0x1700_0000;
const VICE_END: u64 = 0x1800_0000;
const MACE_LOW_START: u64 = 0x1800_0000;
const MACE_LOW_END: u64 = 0x2000_0000;
const LINEAR_MEMORY_START: u64 = 0x4000_0000;
const LINEAR_MEMORY_END: u64 = 0x8000_0000;
const NO_ECC_MEMORY_START: u64 = 0x8000_0000;
const NO_ECC_MEMORY_END: u64 = 0xc000_0000;
const PCI_HIGH_START: u64 = 0x1_0000_0000;
const PCI_HIGH_END: u64 = 0x3_0000_0000;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum PendingMemoryOrigin {
    SysAd {
        sysad_id: CrimeTransactionId,
        address: u64,
    },
    CmiDma {
        link_id: CrimeTransactionId,
    },
    CgiDma {
        link_id: CrimeTransactionId,
    },
    Render,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct PendingLink {
    sysad_id: CrimeTransactionId,
    address: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct PendingRenderWrite {
    sysad_id: CrimeTransactionId,
    address: u64,
    transfer: CrimeTransfer,
}

type PendingMemoryTable = InlineMap8<CrimeTransactionId, PendingMemoryOrigin>;
type PendingLinkTable = InlineMap8<CrimeTransactionId, PendingLink>;
type CrimeSysAdResult = Result<CrimeCompletionPayload, CrimeBusError>;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum RenderAccessResult {
    Complete(Result<CrimeCompletionPayload, CrimeBusError>),
    Deferred,
}

/// Terminal CRIME protocol or configuration error.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeError {
    /// Chipset construction input is invalid.
    Configuration(CrimeConfigError),

    /// The machine timing ABI has no ticks.
    InvalidTimebase,

    /// A second SysAD request arrived while another processor request was outstanding.
    SysAdBusy {
        /// Outstanding SysAD transaction.
        transaction_id: CrimeTransactionId,
    },

    /// CRIME exhausted its transaction identifier space.
    TransactionIdOverflow,

    /// A completion did not correspond to pending controller work.
    UnexpectedCompletion {
        /// Completion identifier.
        transaction_id: CrimeTransactionId,
    },

    /// The Rendering Engine rejected or could not complete a submitted job.
    Render(CrimeRenderError),
}

impl fmt::Display for CrimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => write!(f, "invalid CRIME configuration: {error}"),
            Self::InvalidTimebase => write!(f, "CRIME machine timebase is zero"),
            Self::SysAdBusy { transaction_id } => {
                write!(
                    f,
                    "CRIME SysAD request {transaction_id} is still outstanding"
                )
            }
            Self::TransactionIdOverflow => write!(f, "CRIME transaction identifier overflow"),
            Self::UnexpectedCompletion { transaction_id } => {
                write!(f, "unexpected CRIME completion {transaction_id}")
            }
            Self::Render(error) => write!(f, "CRIME Rendering Engine failed: {error}"),
        }
    }
}

impl std::error::Error for CrimeError {}

/// SGI CRIME 1.1 chipset component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Crime {
    id: ComponentId,
    name: String,
    config: CrimeConfig,
    timebase_hz: u64,
    memory_target: ComponentId,
    mace_target: ComponentId,
    gbe_target: ComponentId,
    piu: CrimePiu,
    render: CrimeRender,
    render_clock: CrimeClock,
    memory_control: u64,
    bank_control: [u16; 8],
    refresh_counter: u16,
    memory_error_status: u32,
    memory_error_address: u64,
    memory_ecc_syndrome: u32,
    memory_ecc_check: u32,
    memory_ecc_replacement: u8,
    next_transaction_id: u128,
    pending_sysad: Option<CrimeTransactionId>,
    pending_render_write: Option<PendingRenderWrite>,
    pending_memory: PendingMemoryTable,
    pending_cmi: PendingLinkTable,
    pending_cgi: PendingLinkTable,
    cancelled_memory: BTreeSet<CrimeTransactionId>,
    cancelled_cmi: BTreeSet<CrimeTransactionId>,
    cancelled_cgi: BTreeSet<CrimeTransactionId>,
    actions: VecDeque<CrimeAction>,
    trace_interest: TraceInterest,
    terminal_error: Option<CrimeError>,
    current_time: SimTime,
}

/// Immutable register image used by a proven synchronous SysAD read batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeSynchronousReadSnapshot {
    timebase_hz: u64,
    piu: CrimePiu,
    memory_control: u64,
    bank_control: [u16; 8],
    refresh_counter: u16,
    memory_error_status: u32,
    memory_error_address: u64,
    memory_ecc_syndrome: u32,
    memory_ecc_check: u32,
    memory_ecc_replacement: u8,
}

/// Affine CRIME TIMER model captured for one synchronous read batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CrimeSynchronousTimerProjection {
    /// Physical address of the TIMER register.
    pub physical_address: u64,

    /// Timer value programmed at the projection origin.
    pub base: u32,

    /// Simulated time at which `base` was programmed.
    pub base_time: SimTime,

    /// CRIME master frequency driving the timer.
    pub frequency_hz: u64,

    /// Simulated machine timebase frequency.
    pub timebase_hz: u64,
}

impl CrimeSynchronousReadSnapshot {
    /// Completes a defined, aligned control-register read at an exact delivery time.
    pub fn read(&self, address: u64, length: u16, delivery_time: SimTime) -> Option<CrimeData> {
        control_register_read_data(address, length, |address| {
            self.piu
                .read(address, delivery_time, self.timebase_hz)
                .or_else(|| self.read_memory_register(address))
        })
    }

    /// Returns the exact affine TIMER model represented by this snapshot.
    pub fn timer_projection(&self) -> CrimeSynchronousTimerProjection {
        let (base, base_time) = self.piu.timer_projection();
        CrimeSynchronousTimerProjection {
            physical_address: registers::TIMER,
            base,
            base_time,
            frequency_hz: CRIME_MASTER_FREQUENCY_HZ,
            timebase_hz: self.timebase_hz,
        }
    }

    fn read_memory_register(&self, address: u64) -> Option<u64> {
        if let Some(bank) = registers::memory_bank_index(address) {
            return Some(u64::from(self.bank_control[bank]));
        }
        match address {
            registers::MEMORY_CONTROL => Some(self.memory_control),
            registers::MEMORY_REFRESH_COUNTER => Some(u64::from(self.refresh_counter)),
            registers::MEMORY_ERROR_STATUS => Some(u64::from(self.memory_error_status)),
            registers::MEMORY_ERROR_ADDRESS => Some(self.memory_error_address),
            registers::MEMORY_ECC_SYNDROME => Some(u64::from(self.memory_ecc_syndrome)),
            registers::MEMORY_ECC_CHECK => Some(u64::from(self.memory_ecc_check)),
            registers::MEMORY_ECC_REPLACEMENT => Some(u64::from(self.memory_ecc_replacement)),
            _ => None,
        }
    }
}

/// Serializable dynamic state of the CRIME chipset.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeState {
    id: ComponentId,
    config: CrimeConfig,
    timebase_hz: u64,
    memory_target: ComponentId,
    mace_target: ComponentId,
    gbe_target: ComponentId,
    piu: CrimePiuState,
    render: CrimeRenderState,
    render_clock: CrimeClockState,
    memory_control: u64,
    bank_control: [u16; 8],
    refresh_counter: u16,
    memory_error_status: u32,
    memory_error_address: u64,
    memory_ecc_syndrome: u32,
    memory_ecc_check: u32,
    memory_ecc_replacement: u8,
    next_transaction_id: u128,
    pending_sysad: Option<CrimeTransactionId>,
    pending_render_write: Option<PendingRenderWrite>,
    pending_memory: PendingMemoryTable,
    pending_cmi: PendingLinkTable,
    pending_cgi: PendingLinkTable,
    cancelled_memory: BTreeSet<CrimeTransactionId>,
    cancelled_cmi: BTreeSet<CrimeTransactionId>,
    cancelled_cgi: BTreeSet<CrimeTransactionId>,
    terminal_error: Option<CrimeError>,
    current_time: SimTime,
}

impl Crime {
    /// Creates a CRIME 1.1 chipset with machine-supplied endpoint identities.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        config: CrimeConfig,
        timebase_hz: u64,
        memory_target: ComponentId,
        mace_target: ComponentId,
        gbe_target: ComponentId,
    ) -> Result<Self, CrimeError> {
        config.validate().map_err(CrimeError::Configuration)?;
        if timebase_hz == 0 {
            return Err(CrimeError::InvalidTimebase);
        }
        Ok(Self {
            id,
            name: name.into(),
            config,
            timebase_hz,
            memory_target,
            mace_target,
            gbe_target,
            piu: CrimePiu::new(),
            render: CrimeRender::new(),
            render_clock: CrimeClock::new(timebase_hz),
            memory_control: 0,
            bank_control: reset_bank_control(config.memory),
            refresh_counter: 0,
            memory_error_status: 0,
            memory_error_address: 0,
            memory_ecc_syndrome: 0,
            memory_ecc_check: 0,
            memory_ecc_replacement: 0,
            next_transaction_id: 0,
            pending_sysad: None,
            pending_render_write: None,
            pending_memory: PendingMemoryTable::new(),
            pending_cmi: PendingLinkTable::new(),
            pending_cgi: PendingLinkTable::new(),
            cancelled_memory: BTreeSet::new(),
            cancelled_cmi: BTreeSet::new(),
            cancelled_cgi: BTreeSet::new(),
            actions: VecDeque::new(),
            trace_interest: TraceInterest::All,
            terminal_error: None,
            current_time: SimTime::ZERO,
        })
    }

    /// Returns the immutable construction input.
    pub const fn config(&self) -> CrimeConfig {
        self.config
    }

    /// Captures CRIME's dynamic hardware state.
    pub fn save_state(&self) -> CrimeState {
        CrimeState {
            id: self.id,
            config: self.config,
            timebase_hz: self.timebase_hz,
            memory_target: self.memory_target,
            mace_target: self.mace_target,
            gbe_target: self.gbe_target,
            piu: self.piu.save_state(),
            render: self.render.save_state(),
            render_clock: self.render_clock.save_state(),
            memory_control: self.memory_control,
            bank_control: self.bank_control,
            refresh_counter: self.refresh_counter,
            memory_error_status: self.memory_error_status,
            memory_error_address: self.memory_error_address,
            memory_ecc_syndrome: self.memory_ecc_syndrome,
            memory_ecc_check: self.memory_ecc_check,
            memory_ecc_replacement: self.memory_ecc_replacement,
            next_transaction_id: self.next_transaction_id,
            pending_sysad: self.pending_sysad,
            pending_render_write: self.pending_render_write.clone(),
            pending_memory: self.pending_memory.clone(),
            pending_cmi: self.pending_cmi.clone(),
            pending_cgi: self.pending_cgi.clone(),
            cancelled_memory: self.cancelled_memory.clone(),
            cancelled_cmi: self.cancelled_cmi.clone(),
            cancelled_cgi: self.cancelled_cgi.clone(),
            terminal_error: self.terminal_error.clone(),
            current_time: self.current_time,
        }
    }

    /// Restores dynamic state after validating configuration and transaction invariants.
    pub fn restore_state(&mut self, state: CrimeState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        for (matches, field) in [
            (self.config == state.config, "config"),
            (self.timebase_hz == state.timebase_hz, "timebase_hz"),
            (self.memory_target == state.memory_target, "memory_target"),
            (self.mace_target == state.mace_target, "mace_target"),
            (self.gbe_target == state.gbe_target, "gbe_target"),
        ] {
            if !matches {
                return Err(ComponentStateError::ConfigurationMismatch {
                    component: self.id,
                    field,
                });
            }
        }
        self.render_clock
            .validate_state(self.id, state.render_clock)?;
        let restored_piu = CrimePiu::from_state(state.piu);
        if let Err(invariant) = restored_piu.validate_state(state.current_time) {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant,
            });
        }
        let restored_render = CrimeRender::from_state(state.render);
        if let Err(invariant) = restored_render.validate_state() {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant,
            });
        }
        if state.memory_control & !0x3 != 0
            || state
                .bank_control
                .iter()
                .any(|value| value & !registers::MEMORY_BANK_CONTROL_MASK != 0)
            || state.refresh_counter & !0x03ff != 0
            || state.memory_error_status & !registers::MEMORY_ERROR_STATUS_MASK != 0
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "CRIME memory registers must use implemented bit encodings",
            });
        }
        if state.pending_memory.len() > 8
            || state.pending_cmi.len() > 8
            || state.pending_cgi.len() > 8
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "CRIME pending transaction tables must fit their fixed capacity",
            });
        }
        let memory_ids: BTreeSet<_> = state.pending_memory.iter().map(|(id, _)| *id).collect();
        let cmi_ids: BTreeSet<_> = state.pending_cmi.iter().map(|(id, _)| *id).collect();
        let cgi_ids: BTreeSet<_> = state.pending_cgi.iter().map(|(id, _)| *id).collect();
        if memory_ids.len() != state.pending_memory.len()
            || cmi_ids.len() != state.pending_cmi.len()
            || cgi_ids.len() != state.pending_cgi.len()
            || memory_ids
                .iter()
                .any(|id| state.cancelled_memory.contains(id))
            || cmi_ids.iter().any(|id| state.cancelled_cmi.contains(id))
            || cgi_ids.iter().any(|id| state.cancelled_cgi.contains(id))
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "CRIME pending and cancelled transaction identifiers must be unique",
            });
        }
        let all_internal_ids = memory_ids
            .iter()
            .chain(cmi_ids.iter())
            .chain(cgi_ids.iter())
            .chain(state.cancelled_memory.iter())
            .chain(state.cancelled_cmi.iter())
            .chain(state.cancelled_cgi.iter())
            .copied()
            .collect::<Vec<_>>();
        let unique_internal_ids: BTreeSet<_> = all_internal_ids.iter().copied().collect();
        if unique_internal_ids.len() != all_internal_ids.len()
            || all_internal_ids
                .iter()
                .any(|id| id.get() >= state.next_transaction_id)
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "CRIME transaction identifiers must be unique and precede the allocation cursor",
            });
        }
        let mut sysad_references = Vec::new();
        for (_, origin) in state.pending_memory.iter() {
            if let PendingMemoryOrigin::SysAd { sysad_id, .. } = origin {
                sysad_references.push(*sysad_id);
            }
        }
        sysad_references.extend(
            state
                .pending_cmi
                .iter()
                .map(|(_, pending)| pending.sysad_id),
        );
        sysad_references.extend(
            state
                .pending_cgi
                .iter()
                .map(|(_, pending)| pending.sysad_id),
        );
        if let Some(pending) = &state.pending_render_write {
            sysad_references.push(pending.sysad_id);
            let CrimeTransferView::Write { data, byte_enable } = pending.transfer.view() else {
                return Err(ComponentStateError::InvalidState {
                    component: self.id,
                    invariant: "deferred CRIME render access must be a write",
                });
            };
            let Ok(size) = u8::try_from(data.len()) else {
                return Err(ComponentStateError::InvalidState {
                    component: self.id,
                    invariant: "deferred CRIME render write size must fit the register ABI",
                });
            };
            if data.len() != byte_enable.len()
                || byte_enable.iter().any(|enabled| !enabled)
                || decode_register_data(data).is_none()
                || !CrimeRender::validate_deferred_write(pending.address, size)
            {
                return Err(ComponentStateError::InvalidState {
                    component: self.id,
                    invariant: "deferred CRIME render write must remain a valid buffered access",
                });
            }
        }
        if match state.pending_sysad {
            Some(id) => sysad_references.len() != 1 || sysad_references[0] != id,
            None => !sysad_references.is_empty(),
        } {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "CRIME pending SysAD state must have exactly one matching operation",
            });
        }
        let render_pending_count = state
            .pending_memory
            .iter()
            .filter(|(_, origin)| matches!(origin, PendingMemoryOrigin::Render))
            .count();
        if render_pending_count != usize::from(restored_render.has_pending_memory()) {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "CRIME render memory state must match the transaction table",
            });
        }

        let mut restored = self.clone();
        restored.piu = restored_piu;
        restored.render = restored_render;
        restored.render_clock.restore_state(state.render_clock);
        restored.memory_control = state.memory_control;
        restored.bank_control = state.bank_control;
        restored.refresh_counter = state.refresh_counter;
        restored.memory_error_status = state.memory_error_status;
        restored.memory_error_address = state.memory_error_address;
        restored.memory_ecc_syndrome = state.memory_ecc_syndrome;
        restored.memory_ecc_check = state.memory_ecc_check;
        restored.memory_ecc_replacement = state.memory_ecc_replacement;
        restored.next_transaction_id = state.next_transaction_id;
        restored.pending_sysad = state.pending_sysad;
        restored.pending_render_write = state.pending_render_write;
        restored.pending_memory = state.pending_memory;
        restored.pending_cmi = state.pending_cmi;
        restored.pending_cgi = state.pending_cgi;
        restored.cancelled_memory = state.cancelled_memory;
        restored.cancelled_cmi = state.cancelled_cmi;
        restored.cancelled_cgi = state.cancelled_cgi;
        restored.terminal_error = state.terminal_error;
        restored.current_time = state.current_time;
        restored.actions.clear();
        *self = restored;
        Ok(())
    }

    /// Observes machine time before accepting an untimed peer-link request.
    pub fn observe_time(&mut self, now: SimTime) {
        self.current_time = now;
    }

    /// Returns whether an idle stable PROM fetch can bypass CRIME.
    pub fn stable_cpu_fetch_ready(&self) -> bool {
        self.pending_sysad.is_none()
            && self.pending_memory.is_empty()
            && self.pending_cmi.is_empty()
            && self.pending_cgi.is_empty()
            && self.actions.is_empty()
            && self.terminal_error.is_none()
    }

    /// Classifies a CPU request without changing chipset state.
    pub fn classify_sysad_route(address: u64, transfer: &CrimeTransfer) -> CrimeSysAdRoute {
        let size = transfer.length();
        if decode_memory(address, size).is_some() {
            CrimeSysAdRoute::Memory
        } else if (registers::CRIME_BASE..registers::CRIME_RENDER_BASE).contains(&address) {
            CrimeSysAdRoute::SynchronousInternalRegister
        } else if (registers::CRIME_RENDER_BASE..registers::CRIME_REGISTER_END).contains(&address) {
            CrimeSysAdRoute::RenderRegister
        } else if in_window(address, size, GBE_START, GBE_END) {
            CrimeSysAdRoute::Cgi
        } else if in_window(address, size, MACE_LOW_START, MACE_LOW_END)
            || in_window(address, size, PCI_HIGH_START, PCI_HIGH_END)
        {
            CrimeSysAdRoute::Cmi
        } else {
            CrimeSysAdRoute::Unsupported
        }
    }

    /// Previews the exact memory-domain request produced by an idle CPU access.
    pub fn preview_synchronous_memory_request(
        &self,
        request: &CrimeSysAdRequest,
    ) -> Option<CrimeMemoryTransaction> {
        if !self.stable_cpu_fetch_ready()
            || Self::classify_sysad_route(request.address, &request.transfer)
                != CrimeSysAdRoute::Memory
            || self.next_transaction_id.checked_add(1).is_none()
        {
            return None;
        }
        let (memory_address, no_ecc) = decode_memory(request.address, request.transfer.length())?;
        Some(CrimeMemoryTransaction {
            id: CrimeTransactionId::new(self.next_transaction_id),
            time: request.time,
            controller: self.id,
            client: CrimeMemoryClient::Cpu,
            address: memory_address,
            bank_select: CrimeMemoryBankSelect::Decode,
            no_ecc,
            transfer: request.transfer.clone(),
        })
    }

    /// Returns whether an idle, defined control-register read can complete synchronously.
    pub fn synchronous_sysad_read_ready(&self, address: u64, transfer: &CrimeTransfer) -> bool {
        self.stable_cpu_fetch_ready()
            && Self::classify_sysad_route(address, transfer)
                == CrimeSysAdRoute::SynchronousInternalRegister
            && self
                .control_register_read_data(address, transfer, self.current_time)
                .is_some()
    }

    /// Captures the immutable register image needed by direct read batches.
    pub fn synchronous_read_snapshot(&self) -> Option<CrimeSynchronousReadSnapshot> {
        self.stable_cpu_fetch_ready()
            .then(|| CrimeSynchronousReadSnapshot {
                timebase_hz: self.timebase_hz,
                piu: self.piu.clone(),
                memory_control: self.memory_control,
                bank_control: self.bank_control,
                refresh_counter: self.refresh_counter,
                memory_error_status: self.memory_error_status,
                memory_error_address: self.memory_error_address,
                memory_ecc_syndrome: self.memory_ecc_syndrome,
                memory_ecc_check: self.memory_ecc_check,
                memory_ecc_replacement: self.memory_ecc_replacement,
            })
    }

    /// Commits the time observed by a proven batch of side-effect-free reads.
    pub fn commit_synchronous_sysad_reads(
        &mut self,
        reads: u64,
        last_delivery_time: SimTime,
    ) -> bool {
        if reads == 0 {
            return true;
        }
        if !self.stable_cpu_fetch_ready() {
            return false;
        }
        self.current_time = self.current_time.max(last_delivery_time);
        true
    }

    /// Accounts for CMI transaction IDs consumed by proven synchronous reads.
    pub fn commit_synchronous_cmi_reads(&mut self, reads: u64) -> Result<bool, CrimeError> {
        if reads == 0 {
            return Ok(true);
        }
        if !self.stable_cpu_fetch_ready() {
            return Ok(false);
        }
        self.next_transaction_id = self
            .next_transaction_id
            .checked_add(u128::from(reads))
            .ok_or(CrimeError::TransactionIdOverflow)?;
        Ok(true)
    }

    /// Returns whether a synchronous CMI batch can allocate all transaction IDs.
    pub fn synchronous_cmi_reads_ready(&self, reads: u64) -> bool {
        self.stable_cpu_fetch_ready()
            && self
                .next_transaction_id
                .checked_add(u128::from(reads))
                .is_some()
    }

    /// Completes a previously planned synchronous control-register read.
    ///
    /// The caller must retain exclusive ownership of the idle SysAD path from
    /// planning through commit.
    pub fn commit_synchronous_sysad_read(
        &mut self,
        request: &CrimeSysAdRequest,
    ) -> CrimeSysAdCompletion {
        debug_assert!(self.synchronous_sysad_read_ready(request.address, &request.transfer));
        self.current_time = request.time;
        CrimeSysAdCompletion {
            id: request.id,
            result: Ok(CrimeCompletionPayload::ReadData(
                self.control_register_read_data(request.address, &request.transfer, request.time)
                    .expect("a validated synchronous CRIME read must remain defined"),
            )),
        }
    }

    /// Accounts for bypassed stable CPU requests at the last SysAD delivery time.
    pub fn account_stable_cpu_fetches(
        &mut self,
        fetches: usize,
        last_delivery_time: SimTime,
    ) -> Result<bool, CrimeError> {
        if fetches == 0 {
            return Ok(true);
        }
        if !self.stable_cpu_fetch_ready() {
            return Ok(false);
        }
        let fetches = u128::try_from(fetches).map_err(|_| CrimeError::TransactionIdOverflow)?;
        self.next_transaction_id = self
            .next_transaction_id
            .checked_add(fetches)
            .ok_or(CrimeError::TransactionIdOverflow)?;
        self.current_time = self.current_time.max(last_delivery_time);
        Ok(true)
    }

    /// Returns whether a stable CPU batch can allocate all downstream IDs.
    pub fn stable_cpu_fetches_ready(&self, fetches: usize) -> bool {
        self.stable_cpu_fetch_ready()
            && u128::try_from(fetches)
                .ok()
                .and_then(|fetches| self.next_transaction_id.checked_add(fetches))
                .is_some()
    }

    /// Updates the coarse trace interest supplied by the machine runtime.
    pub fn set_trace_interest(&mut self, interest: TraceInterest) {
        self.trace_interest = interest;
    }

    /// Restores power-on state and requests SDRAM clearing.
    pub fn power_on(&mut self, now: SimTime) {
        self.reset_logic(now);
        self.actions
            .push_back(CrimeAction::SignalMemory(CrimeSdramSignal::PowerOn));
        self.push_memory_configuration();
    }

    /// Restores hard-reset logic while preserving SDRAM contents.
    pub fn hard_reset(&mut self, now: SimTime) {
        self.reset_logic(now);
        self.actions
            .push_back(CrimeAction::SignalMemory(CrimeSdramSignal::HardReset));
        self.push_memory_configuration();
    }

    /// Cancels the processor-side transaction while preserving chipset state.
    pub fn warm_reset(&mut self) {
        self.pending_sysad = None;
        self.pending_render_write = None;
        let cancelled_memory = self
            .pending_memory
            .iter()
            .filter_map(|(id, origin)| {
                matches!(origin, PendingMemoryOrigin::SysAd { .. }).then_some(*id)
            })
            .collect::<Vec<_>>();
        for id in cancelled_memory {
            self.pending_memory.remove(&id);
            self.cancelled_memory.insert(id);
        }
        self.cancelled_cmi.extend(self.pending_cmi.drain_keys());
        self.cancelled_cgi.extend(self.pending_cgi.drain_keys());
        self.actions
            .retain(|action| !matches!(action, CrimeAction::CompleteSysAd(_)));
    }

    /// Handles one owned CRIME event.
    pub fn handle_event(&mut self, now: SimTime, event: CrimeEvent) {
        self.current_time = now;
        match event {
            CrimeEvent::Watchdog { epoch, stage } => {
                let effects = self.piu.handle_watchdog(epoch, stage, self.timebase_hz);
                self.apply_piu_effects(effects);
            }
            CrimeEvent::RenderStep { epoch } if epoch == self.render.epoch() => {
                match self.render.step() {
                    Ok(progress) => {
                        if let Err(error) = self.apply_render_progress(progress) {
                            self.latch_error(error);
                        } else if let Err(error) = self.retry_pending_render_write() {
                            self.latch_error(error);
                        }
                    }
                    Err(error) => self.latch_render_error(error),
                }
            }
            CrimeEvent::RenderStep { .. } => {}
        }
    }

    /// Polls one pending chipset action.
    pub fn poll(&mut self) -> Result<CrimePoll, CrimeError> {
        if let Some(action) = self.actions.pop_front() {
            return Ok(CrimePoll::Action(action));
        }
        if let Some(error) = &self.terminal_error {
            return Err(error.clone());
        }
        Ok(CrimePoll::Idle)
    }

    fn reset_logic(&mut self, now: SimTime) {
        self.piu.power_on(now);
        let render_interrupts = self.render.reset();
        self.render_clock.reset();
        self.memory_control = 0;
        self.bank_control = reset_bank_control(self.config.memory);
        self.refresh_counter = 0;
        self.memory_error_status = 0;
        self.memory_error_address = 0;
        self.memory_ecc_syndrome = 0;
        self.memory_ecc_check = 0;
        self.memory_ecc_replacement = 0;
        self.pending_sysad = None;
        self.pending_render_write = None;
        self.pending_memory.clear();
        self.pending_cmi.clear();
        self.pending_cgi.clear();
        self.cancelled_memory.clear();
        self.cancelled_cmi.clear();
        self.cancelled_cgi.clear();
        self.actions.clear();
        self.terminal_error = None;
        self.current_time = now;
        self.apply_render_interrupts(render_interrupts);
    }

    fn push_memory_configuration(&mut self) {
        for (bank, value) in self.bank_control.into_iter().enumerate() {
            self.actions.push_back(CrimeAction::SignalMemory(
                CrimeSdramSignal::SetBankControl {
                    bank: bank as u8,
                    value,
                },
            ));
        }
        self.push_ecc_control();
    }

    fn push_trace<F>(&mut self, build: F)
    where
        F: FnOnce() -> OwnedTraceEvent,
    {
        if self.trace_interest != TraceInterest::None {
            self.actions
                .push_back(CrimeAction::Trace(Box::new(build())));
        }
    }

    fn accept_sysad(&mut self, request: CrimeSysAdRequest) -> Result<(), CrimeError> {
        if let Some(transaction_id) = self.pending_sysad {
            return Err(CrimeError::SysAdBusy { transaction_id });
        }
        let CrimeSysAdRequest {
            id: sysad_id,
            time,
            address,
            transfer,
        } = request;
        self.current_time = time;
        let size = transfer.length();
        self.push_trace(|| OwnedTraceEvent {
            level: TraceLevel::Trace,
            target: trace::PIU_TARGET.into(),
            event: "sysad_request".into(),
            fields: [
                OwnedTraceField {
                    key: "physical_address".into(),
                    value: OwnedTraceValue::Hex64(address),
                },
                OwnedTraceField {
                    key: "size".into(),
                    value: OwnedTraceValue::U64(size as u64),
                },
            ]
            .into(),
        });

        if !matches!(size, 1 | 2 | 4 | 8)
            || matches!(
                transfer.view(),
                CrimeTransferView::Write { data, byte_enable }
                    if data.len() != byte_enable.len()
            )
        {
            self.record_cpu_error(address);
            self.finish_sysad(sysad_id, Err(CrimeBusError::Access));
            return Ok(());
        }

        if let Some((memory_address, no_ecc)) = decode_memory(address, size) {
            let id = self.allocate_transaction_id()?;
            self.pending_sysad = Some(sysad_id);
            self.pending_memory
                .insert(id, PendingMemoryOrigin::SysAd { sysad_id, address });
            self.actions
                .push_back(CrimeAction::StartMemory(CrimeMemoryTransaction {
                    id,
                    time,
                    controller: self.id,
                    client: CrimeMemoryClient::Cpu,
                    address: memory_address,
                    bank_select: CrimeMemoryBankSelect::Decode,
                    no_ecc,
                    transfer,
                }));
            return Ok(());
        }
        if (registers::CRIME_BASE..registers::CRIME_REGISTER_END).contains(&address) {
            self.access_internal_register(sysad_id, address, transfer, time)?;
            return Ok(());
        }
        if in_window(address, size, GBE_START, GBE_END) {
            let id = self.allocate_transaction_id()?;
            self.pending_sysad = Some(sysad_id);
            self.pending_cgi
                .insert(id, PendingLink { sysad_id, address });
            self.actions
                .push_back(CrimeAction::StartCgi(CrimeCgiTransaction {
                    id,
                    controller: self.id,
                    target: self.gbe_target,
                    operation: CrimeLinkOperation::Pio(CrimePioRequest { address, transfer }),
                }));
            return Ok(());
        }
        if in_window(address, size, MACE_LOW_START, MACE_LOW_END)
            || in_window(address, size, PCI_HIGH_START, PCI_HIGH_END)
        {
            let id = self.allocate_transaction_id()?;
            self.pending_sysad = Some(sysad_id);
            self.pending_cmi
                .insert(id, PendingLink { sysad_id, address });
            self.actions
                .push_back(CrimeAction::StartCmi(CrimeCmiTransaction {
                    id,
                    controller: self.id,
                    target: self.mace_target,
                    operation: CrimeLinkOperation::Pio(CrimePioRequest { address, transfer }),
                }));
            return Ok(());
        }
        let _vice = in_window(address, size, VICE_START, VICE_END);
        self.complete_unsupported(sysad_id, &transfer, address);
        Ok(())
    }

    fn access_internal_register(
        &mut self,
        sysad_id: CrimeTransactionId,
        address: u64,
        transfer: CrimeTransfer,
        now: SimTime,
    ) -> Result<(), CrimeError> {
        let size = transfer.length();
        let completion = if address < registers::CRIME_RENDER_BASE {
            match transfer.view() {
                CrimeTransferView::Read { .. } if size == 4 && address & 3 == 0 => {
                    self.access_control_register_word(address, now)
                }
                _ if size == 8 && address & 7 == 0 => {
                    self.access_control_register(address, &transfer, now)
                }
                _ => Err(CrimeBusError::Access),
            }
        } else {
            match self.access_render_register(sysad_id, address, &transfer)? {
                RenderAccessResult::Complete(completion) => completion,
                RenderAccessResult::Deferred => return Ok(()),
            }
        };
        self.complete_internal_register_access(sysad_id, address, &transfer, completion);
        Ok(())
    }

    fn complete_internal_register_access(
        &mut self,
        sysad_id: CrimeTransactionId,
        address: u64,
        transfer: &CrimeTransfer,
        completion: CrimeSysAdResult,
    ) {
        let size = transfer.length();
        let target = if address < registers::CRIME_BASE + 0x0200 {
            trace::PIU_TARGET
        } else if address < registers::CRIME_RENDER_BASE {
            trace::MIU_TARGET
        } else {
            trace::RENDER_TARGET
        };
        self.push_trace(|| OwnedTraceEvent {
            level: if completion.is_err() {
                TraceLevel::Warn
            } else {
                TraceLevel::Trace
            },
            target: target.into(),
            event: "register_access".into(),
            fields: [
                OwnedTraceField {
                    key: "physical_address".into(),
                    value: OwnedTraceValue::Hex64(address),
                },
                OwnedTraceField {
                    key: "size".into(),
                    value: OwnedTraceValue::U64(size as u64),
                },
                OwnedTraceField {
                    key: "operation".into(),
                    value: OwnedTraceValue::String(
                        (match transfer.view() {
                            CrimeTransferView::Read { .. } => "read",
                            CrimeTransferView::Write { .. } => "write",
                        })
                        .into(),
                    ),
                },
                OwnedTraceField {
                    key: "bus_error".into(),
                    value: OwnedTraceValue::Bool(completion.is_err()),
                },
            ]
            .into(),
        });
        if completion.is_err() {
            self.record_cpu_error(address);
        }
        self.finish_sysad(sysad_id, completion);
    }

    fn access_control_register(
        &mut self,
        address: u64,
        transfer: &CrimeTransfer,
        now: SimTime,
    ) -> CrimeSysAdResult {
        match transfer.view() {
            CrimeTransferView::Read { .. } => self
                .control_register_read_data(address, transfer, now)
                .map(CrimeCompletionPayload::ReadData)
                .map(Ok)
                .unwrap_or_else(|| self.unsupported_read_result(transfer.length())),
            CrimeTransferView::Write { data, byte_enable } => {
                if data.len() != 8
                    || byte_enable.len() != 8
                    || byte_enable.iter().any(|enabled| !enabled)
                {
                    return Err(CrimeBusError::Access);
                }
                let value = u64::from_be_bytes(data.try_into().expect("length was checked"));
                let result = self.piu.write(address, value, now, self.timebase_hz);
                if result.handled {
                    self.apply_piu_effects(result.effects);
                    Ok(CrimeCompletionPayload::WriteComplete)
                } else if self.write_memory_register(address, value) {
                    Ok(CrimeCompletionPayload::WriteComplete)
                } else {
                    self.unsupported_write_result()
                }
            }
        }
    }

    fn control_register_read_data(
        &self,
        address: u64,
        transfer: &CrimeTransfer,
        now: SimTime,
    ) -> Option<CrimeData> {
        let CrimeTransferView::Read { length } = transfer.view() else {
            return None;
        };
        control_register_read_data(address, length, |address| {
            self.read_control_register(address, now)
        })
    }

    fn access_control_register_word(&self, address: u64, now: SimTime) -> CrimeSysAdResult {
        self.control_register_word_read_data(address, now)
            .map(CrimeCompletionPayload::ReadData)
            .map(Ok)
            .unwrap_or_else(|| self.unsupported_read_result(4))
    }

    fn control_register_word_read_data(
        &self,
        address: u64,
        now: SimTime,
    ) -> Option<self::protocol::CrimeData> {
        control_register_word_read_data(address, |register_address| {
            self.read_control_register(register_address, now)
        })
    }

    fn read_control_register(&self, address: u64, now: SimTime) -> Option<u64> {
        self.piu
            .read(address, now, self.timebase_hz)
            .or_else(|| self.read_memory_register(address))
    }

    fn access_render_register(
        &mut self,
        sysad_id: CrimeTransactionId,
        address: u64,
        transfer: &CrimeTransfer,
    ) -> Result<RenderAccessResult, CrimeError> {
        let Ok(size) = u8::try_from(transfer.length()) else {
            return Ok(RenderAccessResult::Complete(Err(CrimeBusError::Access)));
        };
        if !matches!(size, 4 | 8) {
            return Ok(RenderAccessResult::Complete(Err(CrimeBusError::Access)));
        }
        let result = match transfer.view() {
            CrimeTransferView::Read { .. } => match self.render.read(address, size) {
                Ok(value) => RenderAccessResult::Complete(Ok(CrimeCompletionPayload::ReadData(
                    encode_register_data(value, size),
                ))),
                Err(RenderAccessError::Access) => {
                    RenderAccessResult::Complete(Err(CrimeBusError::Access))
                }
                Err(RenderAccessError::Unsupported) => {
                    RenderAccessResult::Complete(self.unsupported_read_result(usize::from(size)))
                }
            },
            CrimeTransferView::Write { data, byte_enable } => {
                if byte_enable.len() != data.len() || byte_enable.iter().any(|enabled| !enabled) {
                    return Ok(RenderAccessResult::Complete(Err(CrimeBusError::Access)));
                }
                let Some(value) = decode_register_data(data) else {
                    return Ok(RenderAccessResult::Complete(Err(CrimeBusError::Access)));
                };
                match self.render.write(address, size, value) {
                    Ok(progress) => {
                        self.trace_render_register_write(address, size, value);
                        self.apply_render_progress(progress)?;
                        RenderAccessResult::Complete(Ok(CrimeCompletionPayload::WriteComplete))
                    }
                    Err(RenderWriteError::InterfaceFull) => {
                        self.pending_sysad = Some(sysad_id);
                        self.pending_render_write = Some(PendingRenderWrite {
                            sysad_id,
                            address,
                            transfer: transfer.clone(),
                        });
                        RenderAccessResult::Deferred
                    }
                    Err(RenderWriteError::Access(RenderAccessError::Access)) => {
                        RenderAccessResult::Complete(Err(CrimeBusError::Access))
                    }
                    Err(RenderWriteError::Access(RenderAccessError::Unsupported)) => {
                        RenderAccessResult::Complete(self.unsupported_write_result())
                    }
                }
            }
        };
        Ok(result)
    }

    fn retry_pending_render_write(&mut self) -> Result<(), CrimeError> {
        if !self.render.has_interface_space() {
            return Ok(());
        }
        let Some(pending) = self.pending_render_write.take() else {
            return Ok(());
        };
        let CrimeTransferView::Write { data, .. } = pending.transfer.view() else {
            unreachable!("only RE writes can be deferred")
        };
        let size = u8::try_from(data.len()).expect("validated RE writes fit the size ABI");
        let value = decode_register_data(data).expect("deferred RE write was validated");
        let progress =
            self.render
                .write(pending.address, size, value)
                .map_err(|error| match error {
                    RenderWriteError::InterfaceFull => {
                        unreachable!("the RE interface space was checked before retry")
                    }
                    RenderWriteError::Access(_) => {
                        unreachable!("the deferred RE write was validated before retry")
                    }
                })?;
        self.trace_render_register_write(pending.address, size, value);
        self.apply_render_progress(progress)?;
        self.complete_internal_register_access(
            pending.sysad_id,
            pending.address,
            &pending.transfer,
            Ok(CrimeCompletionPayload::WriteComplete),
        );
        Ok(())
    }

    fn apply_render_progress(&mut self, progress: RenderProgress) -> Result<(), CrimeError> {
        self.apply_render_interrupts(progress.interrupts);
        for notice in progress.notices {
            self.trace_render_notice(notice);
        }
        if let Some(request) = progress.memory_request {
            self.start_render_memory(request)?;
        }
        if progress.schedule_step {
            self.actions.push_back(CrimeAction::Schedule {
                delay: self.render_clock.next_cycle(),
                event: CrimeEvent::RenderStep {
                    epoch: self.render.epoch(),
                },
            });
        }
        Ok(())
    }

    fn apply_render_interrupts(&mut self, effects: Vec<RenderInterruptEffect>) {
        for effect in effects {
            let piu_effect = self.piu.set_hardware_level(effect.mask, effect.asserted);
            self.push_piu_effect(piu_effect);
        }
    }

    fn start_render_memory(&mut self, request: RenderMemoryRequest) -> Result<(), CrimeError> {
        let id = self.allocate_transaction_id()?;
        self.pending_memory.insert(id, PendingMemoryOrigin::Render);
        let operation = match request.transfer.view() {
            CrimeTransferView::Read { .. } => "read",
            CrimeTransferView::Write { .. } => "write",
        };
        let destination = match request.destination {
            RenderMemoryDestination::Mte => "mte",
            RenderMemoryDestination::Pixel => "pixel",
        };
        if let CrimeMemoryBankSelect::Inhibited { reason } = request.bank_select {
            self.push_trace(|| OwnedTraceEvent {
                level: TraceLevel::Debug,
                target: trace::RENDER_TARGET.into(),
                event: "bank_select_inhibited".into(),
                fields: [
                    OwnedTraceField {
                        key: "reason".into(),
                        value: OwnedTraceValue::String(
                            (match reason {
                                CrimeMemoryInhibitReason::InvalidRenderTlb => "invalid_render_tlb",
                            })
                            .into(),
                        ),
                    },
                    OwnedTraceField {
                        key: "physical_address".into(),
                        value: OwnedTraceValue::Hex64(request.physical_address),
                    },
                    OwnedTraceField {
                        key: "operation".into(),
                        value: OwnedTraceValue::String(operation.into()),
                    },
                    OwnedTraceField {
                        key: "destination".into(),
                        value: OwnedTraceValue::String(destination.into()),
                    },
                ]
                .into(),
            });
        }
        self.actions
            .push_back(CrimeAction::StartMemory(CrimeMemoryTransaction {
                id,
                time: self.current_time,
                controller: self.id,
                client: CrimeMemoryClient::Render,
                address: request.physical_address,
                bank_select: request.bank_select,
                no_ecc: request.no_ecc,
                transfer: request.transfer,
            }));
        Ok(())
    }

    fn trace_render_register_write(&mut self, address: u64, size: u8, value: u64) {
        self.push_trace(|| OwnedTraceEvent {
            level: TraceLevel::Trace,
            target: trace::RENDER_TARGET.into(),
            event: "register_write".into(),
            fields: [
                OwnedTraceField {
                    key: "physical_address".into(),
                    value: OwnedTraceValue::Hex64(address),
                },
                OwnedTraceField {
                    key: "size".into(),
                    value: OwnedTraceValue::U64(u64::from(size)),
                },
                OwnedTraceField {
                    key: "value".into(),
                    value: OwnedTraceValue::Hex64(value),
                },
                OwnedTraceField {
                    key: "commit".into(),
                    value: OwnedTraceValue::Bool(
                        (registers::CRIME_RENDER_BASE + 0x2800
                            ..registers::CRIME_RENDER_BASE + 0x2a00)
                            .contains(&address)
                            || (registers::CRIME_RENDER_BASE + 0x3800
                                ..registers::CRIME_RENDER_BASE + 0x3880)
                                .contains(&address),
                    ),
                },
            ]
            .into(),
        });
    }

    fn trace_render_notice(&mut self, notice: RenderNotice) {
        let (event, fields): (&'static str, OwnedTraceFields) = match notice {
            RenderNotice::RegisterRetired(write) => (
                "register_retired",
                [
                    OwnedTraceField {
                        key: "physical_address".into(),
                        value: OwnedTraceValue::Hex64(write.address),
                    },
                    OwnedTraceField {
                        key: "commit".into(),
                        value: OwnedTraceValue::Bool(write.commit),
                    },
                ]
                .into(),
            ),
            RenderNotice::JobCommitted { start, end } => (
                "job_commit",
                [
                    OwnedTraceField {
                        key: "start".into(),
                        value: OwnedTraceValue::Hex64(u64::from(start)),
                    },
                    OwnedTraceField {
                        key: "end".into(),
                        value: OwnedTraceValue::Hex64(u64::from(end)),
                    },
                ]
                .into(),
            ),
            RenderNotice::SemanticFallback {
                domain,
                command,
                provenance,
            } => (
                "semantic_fallback",
                [
                    OwnedTraceField {
                        key: "domain".into(),
                        value: OwnedTraceValue::String(
                            match domain {
                                RenderMemoryDestination::Mte => "mte",
                                RenderMemoryDestination::Pixel => "pixel",
                            }
                            .into(),
                        ),
                    },
                    OwnedTraceField {
                        key: "command".into(),
                        value: OwnedTraceValue::Hex64(u64::from(command)),
                    },
                    OwnedTraceField {
                        key: "algorithm_mask".into(),
                        value: OwnedTraceValue::Hex64(u64::from(provenance.algorithm_mask())),
                    },
                    OwnedTraceField {
                        key: "algorithms".into(),
                        value: OwnedTraceValue::String(provenance.algorithm_names().into()),
                    },
                ]
                .into(),
            ),
            RenderNotice::PixelCommandDecoded {
                primitive,
                draw_mode,
                feature_bits,
                violation_count,
            } => (
                "pixel_command_decoded",
                [
                    OwnedTraceField {
                        key: "primitive".into(),
                        value: OwnedTraceValue::Hex64(u64::from(primitive)),
                    },
                    OwnedTraceField {
                        key: "draw_mode".into(),
                        value: OwnedTraceValue::Hex64(u64::from(draw_mode)),
                    },
                    OwnedTraceField {
                        key: "feature_bits".into(),
                        value: OwnedTraceValue::Hex64(u64::from(feature_bits)),
                    },
                    OwnedTraceField {
                        key: "violation_count".into(),
                        value: OwnedTraceValue::U64(u64::from(violation_count)),
                    },
                ]
                .into(),
            ),
            RenderNotice::PixelCommandCommitted {
                primitive,
                x0,
                y0,
                x1,
                y1,
            } => (
                "pixel_command_commit",
                [
                    OwnedTraceField {
                        key: "primitive".into(),
                        value: OwnedTraceValue::Hex64(u64::from(primitive)),
                    },
                    OwnedTraceField {
                        key: "x0".into(),
                        value: OwnedTraceValue::U64(u64::from(x0)),
                    },
                    OwnedTraceField {
                        key: "y0".into(),
                        value: OwnedTraceValue::U64(u64::from(y0)),
                    },
                    OwnedTraceField {
                        key: "x1".into(),
                        value: OwnedTraceValue::U64(u64::from(x1)),
                    },
                    OwnedTraceField {
                        key: "y1".into(),
                        value: OwnedTraceValue::U64(u64::from(y1)),
                    },
                ]
                .into(),
            ),
            RenderNotice::RasterBatch {
                x,
                y,
                candidates,
                enabled,
            } => (
                "raster_batch",
                [
                    OwnedTraceField {
                        key: "x".into(),
                        value: OwnedTraceValue::U64(u64::from(x)),
                    },
                    OwnedTraceField {
                        key: "y".into(),
                        value: OwnedTraceValue::U64(u64::from(y)),
                    },
                    OwnedTraceField {
                        key: "candidates".into(),
                        value: OwnedTraceValue::U64(u64::from(candidates)),
                    },
                    OwnedTraceField {
                        key: "enabled".into(),
                        value: OwnedTraceValue::U64(u64::from(enabled)),
                    },
                ]
                .into(),
            ),
            RenderNotice::FragmentStage {
                x,
                y,
                stage,
                iteration,
                read_modify_write,
            } => (
                "fragment_stage",
                [
                    OwnedTraceField {
                        key: "x".into(),
                        value: OwnedTraceValue::U64(u64::from(x)),
                    },
                    OwnedTraceField {
                        key: "y".into(),
                        value: OwnedTraceValue::U64(u64::from(y)),
                    },
                    OwnedTraceField {
                        key: "stage".into(),
                        value: OwnedTraceValue::String(stage.trace_name().into()),
                    },
                    OwnedTraceField {
                        key: "stage_code".into(),
                        value: OwnedTraceValue::U64(u64::from(stage.code())),
                    },
                    OwnedTraceField {
                        key: "iteration".into(),
                        value: OwnedTraceValue::U64(u64::from(iteration)),
                    },
                    OwnedTraceField {
                        key: "read_modify_write".into(),
                        value: OwnedTraceValue::Bool(read_modify_write),
                    },
                ]
                .into(),
            ),
            RenderNotice::FramebufferWordLayout {
                logical_lane,
                physical_lane,
                bytes_per_pixel,
            } => (
                "framebuffer_word_layout",
                [
                    OwnedTraceField {
                        key: "logical_lane".into(),
                        value: OwnedTraceValue::U64(u64::from(logical_lane)),
                    },
                    OwnedTraceField {
                        key: "physical_lane".into(),
                        value: OwnedTraceValue::U64(u64::from(physical_lane)),
                    },
                    OwnedTraceField {
                        key: "bytes_per_pixel".into(),
                        value: OwnedTraceValue::U64(u64::from(bytes_per_pixel)),
                    },
                ]
                .into(),
            ),
            RenderNotice::StippleMask {
                pattern,
                index,
                candidates,
                enabled_mask,
            } => (
                "stipple_mask",
                [
                    OwnedTraceField {
                        key: "pattern".into(),
                        value: OwnedTraceValue::Hex64(u64::from(pattern)),
                    },
                    OwnedTraceField {
                        key: "index".into(),
                        value: OwnedTraceValue::U64(u64::from(index)),
                    },
                    OwnedTraceField {
                        key: "candidates".into(),
                        value: OwnedTraceValue::U64(u64::from(candidates)),
                    },
                    OwnedTraceField {
                        key: "enabled_mask".into(),
                        value: OwnedTraceValue::Hex64(u64::from(enabled_mask)),
                    },
                ]
                .into(),
            ),
            RenderNotice::MemoryChunk {
                destination,
                virtual_address,
                physical_address,
                length,
            } => (
                match destination {
                    RenderMemoryDestination::Mte => "mte_chunk",
                    RenderMemoryDestination::Pixel => "pixel_chunk",
                },
                [
                    OwnedTraceField {
                        key: "virtual_address".into(),
                        value: OwnedTraceValue::Hex64(u64::from(virtual_address)),
                    },
                    OwnedTraceField {
                        key: "physical_address".into(),
                        value: OwnedTraceValue::Hex64(physical_address),
                    },
                    OwnedTraceField {
                        key: "length".into(),
                        value: OwnedTraceValue::U64(u64::from(length)),
                    },
                ]
                .into(),
            ),
            RenderNotice::MemoryCompleted {
                destination,
                virtual_address,
                physical_address,
                length,
            } => (
                match destination {
                    RenderMemoryDestination::Mte => "memory_complete",
                    RenderMemoryDestination::Pixel => "pixel_memory_complete",
                },
                [
                    OwnedTraceField {
                        key: "virtual_address".into(),
                        value: OwnedTraceValue::Hex64(u64::from(virtual_address)),
                    },
                    OwnedTraceField {
                        key: "physical_address".into(),
                        value: OwnedTraceValue::Hex64(physical_address),
                    },
                    OwnedTraceField {
                        key: "length".into(),
                        value: OwnedTraceValue::U64(u64::from(length)),
                    },
                ]
                .into(),
            ),
            RenderNotice::TlbTranslation {
                virtual_address,
                raw_entry,
                valid,
                alias_address,
                physical_address,
            } => (
                "tlb_translate",
                [
                    OwnedTraceField {
                        key: "virtual_address".into(),
                        value: OwnedTraceValue::Hex64(u64::from(virtual_address)),
                    },
                    OwnedTraceField {
                        key: "raw_entry".into(),
                        value: OwnedTraceValue::Hex64(u64::from(raw_entry)),
                    },
                    OwnedTraceField {
                        key: "valid".into(),
                        value: OwnedTraceValue::Bool(valid),
                    },
                    OwnedTraceField {
                        key: "alias_address".into(),
                        value: OwnedTraceValue::Hex64(alias_address),
                    },
                    OwnedTraceField {
                        key: "physical_address".into(),
                        value: OwnedTraceValue::Hex64(physical_address),
                    },
                ]
                .into(),
            ),
            RenderNotice::JobCompleted { start, end, reason } => (
                "job_complete",
                [
                    OwnedTraceField {
                        key: "start".into(),
                        value: OwnedTraceValue::Hex64(u64::from(start)),
                    },
                    OwnedTraceField {
                        key: "end".into(),
                        value: OwnedTraceValue::Hex64(u64::from(end)),
                    },
                    OwnedTraceField {
                        key: "reason".into(),
                        value: OwnedTraceValue::String(reason.trace_name().into()),
                    },
                    OwnedTraceField {
                        key: "reason_code".into(),
                        value: OwnedTraceValue::U64(u64::from(reason.code())),
                    },
                ]
                .into(),
            ),
            RenderNotice::PixelCommandCompleted {
                primitive,
                x0,
                y0,
                x1,
                y1,
                reason,
            } => (
                "pixel_command_complete",
                [
                    OwnedTraceField {
                        key: "primitive".into(),
                        value: OwnedTraceValue::Hex64(u64::from(primitive)),
                    },
                    OwnedTraceField {
                        key: "x0".into(),
                        value: OwnedTraceValue::U64(u64::from(x0)),
                    },
                    OwnedTraceField {
                        key: "y0".into(),
                        value: OwnedTraceValue::U64(u64::from(y0)),
                    },
                    OwnedTraceField {
                        key: "x1".into(),
                        value: OwnedTraceValue::U64(u64::from(x1)),
                    },
                    OwnedTraceField {
                        key: "y1".into(),
                        value: OwnedTraceValue::U64(u64::from(y1)),
                    },
                    OwnedTraceField {
                        key: "reason".into(),
                        value: OwnedTraceValue::String(reason.trace_name().into()),
                    },
                    OwnedTraceField {
                        key: "reason_code".into(),
                        value: OwnedTraceValue::U64(u64::from(reason.code())),
                    },
                ]
                .into(),
            ),
        };
        self.push_trace(|| OwnedTraceEvent {
            level: TraceLevel::Debug,
            target: trace::RENDER_TARGET.into(),
            event: event.into(),
            fields,
        });
    }

    fn read_memory_register(&self, address: u64) -> Option<u64> {
        if let Some(bank) = registers::memory_bank_index(address) {
            return Some(u64::from(self.bank_control[bank]));
        }
        match address {
            registers::MEMORY_CONTROL => Some(self.memory_control),
            registers::MEMORY_REFRESH_COUNTER => Some(u64::from(self.refresh_counter)),
            registers::MEMORY_ERROR_STATUS => Some(u64::from(self.memory_error_status)),
            registers::MEMORY_ERROR_ADDRESS => Some(self.memory_error_address),
            registers::MEMORY_ECC_SYNDROME => Some(u64::from(self.memory_ecc_syndrome)),
            registers::MEMORY_ECC_CHECK => Some(u64::from(self.memory_ecc_check)),
            registers::MEMORY_ECC_REPLACEMENT => Some(u64::from(self.memory_ecc_replacement)),
            _ => None,
        }
    }

    fn write_memory_register(&mut self, address: u64, value: u64) -> bool {
        if let Some(bank) = registers::memory_bank_index(address) {
            let value = value as u16 & registers::MEMORY_BANK_CONTROL_MASK;
            self.bank_control[bank] = value;
            self.actions.push_back(CrimeAction::SignalMemory(
                CrimeSdramSignal::SetBankControl {
                    bank: bank as u8,
                    value,
                },
            ));
            return true;
        }
        match address {
            registers::MEMORY_CONTROL => {
                self.memory_control = value & 0x3;
                self.push_ecc_control();
            }
            registers::MEMORY_REFRESH_COUNTER => self.refresh_counter = value as u16 & 0x03ff,
            registers::MEMORY_ERROR_STATUS => {
                self.memory_error_status = value as u32 & registers::MEMORY_ERROR_STATUS_MASK;
                if self.memory_error_status == 0 {
                    let effect = self
                        .piu
                        .set_hardware_level(registers::INTERRUPT_MEMORY_ERROR, false);
                    self.push_piu_effect(effect);
                }
            }
            registers::MEMORY_ERROR_ADDRESS
            | registers::MEMORY_ECC_SYNDROME
            | registers::MEMORY_ECC_CHECK => {}
            registers::MEMORY_ECC_REPLACEMENT => {
                self.memory_ecc_replacement = value as u8;
                self.push_ecc_control();
            }
            _ => return false,
        }
        true
    }

    fn push_ecc_control(&mut self) {
        self.actions
            .push_back(CrimeAction::SignalMemory(CrimeSdramSignal::SetEccControl {
                enabled: self.memory_control & 1 != 0,
                use_replacement: self.memory_control & 2 != 0,
                replacement: self.memory_ecc_replacement,
            }));
    }

    fn complete_memory(&mut self, completion: CrimeMemoryCompletion) -> Result<(), CrimeError> {
        if self.cancelled_memory.remove(&completion.id) {
            return Ok(());
        }
        let Some(origin) = self.pending_memory.remove(&completion.id) else {
            return Err(CrimeError::UnexpectedCompletion {
                transaction_id: completion.id,
            });
        };
        if let Ok(outcome) = &completion.result {
            self.record_memory_diagnostic(&origin, outcome);
        }
        match origin {
            PendingMemoryOrigin::SysAd { sysad_id, address } => {
                let result = sysad_memory_completion(completion.result);
                if result.is_err() {
                    self.record_cpu_error(address);
                }
                self.finish_sysad(sysad_id, result);
            }
            PendingMemoryOrigin::CmiDma { link_id } => {
                let (result, memory_fault) = link_memory_completion(completion.result);
                self.actions
                    .push_back(CrimeAction::CompleteCmiDevice(CrimeCmiCompletion {
                        id: link_id,
                        result,
                        memory_fault,
                    }));
            }
            PendingMemoryOrigin::CgiDma { link_id } => {
                let (result, memory_fault) = link_memory_completion(completion.result);
                self.actions
                    .push_back(CrimeAction::CompleteCgiDevice(CrimeCgiCompletion {
                        id: link_id,
                        result,
                        memory_fault,
                    }));
            }
            PendingMemoryOrigin::Render => {
                let progress = self
                    .render
                    .complete_memory(completion.result)
                    .map_err(CrimeError::Render)?;
                self.apply_render_progress(progress)?;
            }
        }
        Ok(())
    }

    fn complete_cmi(&mut self, completion: CrimeCmiCompletion) -> Result<(), CrimeError> {
        if self.cancelled_cmi.remove(&completion.id) {
            return Ok(());
        }
        let Some(pending) = self.pending_cmi.remove(&completion.id) else {
            return Err(CrimeError::UnexpectedCompletion {
                transaction_id: completion.id,
            });
        };
        let result = completion.result;
        if result.is_err() {
            self.record_cpu_error(pending.address);
        }
        self.finish_sysad(pending.sysad_id, result);
        Ok(())
    }

    fn complete_cgi(&mut self, completion: CrimeCgiCompletion) -> Result<(), CrimeError> {
        if self.cancelled_cgi.remove(&completion.id) {
            return Ok(());
        }
        let Some(pending) = self.pending_cgi.remove(&completion.id) else {
            return Err(CrimeError::UnexpectedCompletion {
                transaction_id: completion.id,
            });
        };
        let result = completion.result;
        if result.is_err() {
            self.record_cpu_error(pending.address);
        }
        self.finish_sysad(pending.sysad_id, result);
        Ok(())
    }

    fn accept_dma(
        &mut self,
        link_id: CrimeTransactionId,
        request: CrimeDmaRequest,
        client: CrimeMemoryClient,
        cmi: bool,
    ) -> Result<(), CrimeError> {
        let id = self.allocate_transaction_id()?;
        self.pending_memory.insert(
            id,
            if cmi {
                PendingMemoryOrigin::CmiDma { link_id }
            } else {
                PendingMemoryOrigin::CgiDma { link_id }
            },
        );
        self.actions
            .push_back(CrimeAction::StartMemory(CrimeMemoryTransaction {
                id,
                time: self.current_time,
                controller: self.id,
                client,
                address: request.address,
                bank_select: CrimeMemoryBankSelect::Decode,
                no_ecc: false,
                transfer: request.transfer,
            }));
        Ok(())
    }

    fn accept_interrupt(&mut self, post: CrimeInterruptPost) -> Result<(), CrimeBusError> {
        if post.interrupt_bit >= 32 {
            return Err(CrimeBusError::Access);
        }
        let effect = self
            .piu
            .set_hardware_level(1_u32 << post.interrupt_bit, post.asserted);
        self.push_piu_effect(effect);
        Ok(())
    }

    fn record_memory_diagnostic(
        &mut self,
        origin: &PendingMemoryOrigin,
        outcome: &CrimeMemoryOutcome,
    ) {
        let Some(diagnostic) = outcome.diagnostic() else {
            return;
        };
        let hard = outcome.fault == Some(CrimeMemoryFault::UncorrectableEcc);
        let address_error = outcome.fault == Some(CrimeMemoryFault::Address);
        let soft = diagnostic.corrected;
        let mut status = match origin {
            PendingMemoryOrigin::SysAd { .. } => registers::MEMORY_ERROR_CPU_ACCESS,
            PendingMemoryOrigin::CmiDma { .. } => 1 << 7,
            PendingMemoryOrigin::CgiDma { .. } => 1 << 16,
            PendingMemoryOrigin::Render => 1 << 15,
        };
        if hard || soft {
            status |= if soft {
                registers::MEMORY_ERROR_SOFT
            } else {
                registers::MEMORY_ERROR_HARD
            };
            status |= if diagnostic.read_modify_write {
                registers::MEMORY_ERROR_ECC_RMW
            } else {
                registers::MEMORY_ERROR_ECC_READ
            };
        } else if diagnostic.read_modify_write {
            status |= registers::MEMORY_ERROR_INVALID_RMW;
        } else if diagnostic.write {
            status |= registers::MEMORY_ERROR_INVALID_WRITE;
        } else {
            status |= registers::MEMORY_ERROR_INVALID_READ;
        }
        let existing_hard = self.memory_error_status & registers::MEMORY_ERROR_HARD != 0;
        let existing_soft = self.memory_error_status & registers::MEMORY_ERROR_SOFT != 0;
        let existing_address = self.memory_error_status
            & (registers::MEMORY_ERROR_INVALID_READ
                | registers::MEMORY_ERROR_INVALID_WRITE
                | registers::MEMORY_ERROR_INVALID_RMW)
            != 0;
        let capture = if hard {
            if existing_hard {
                status |= registers::MEMORY_ERROR_MULTIPLE;
                false
            } else {
                true
            }
        } else if soft {
            !existing_hard && !existing_soft
        } else if address_error {
            !existing_hard && !existing_soft && !existing_address
        } else {
            false
        };
        if capture {
            self.memory_error_address = diagnostic.address & 0x3fff_ffff;
            self.memory_ecc_syndrome = diagnostic.syndrome;
            self.memory_ecc_check = diagnostic.check;
        }
        self.memory_error_status |= status & registers::MEMORY_ERROR_STATUS_MASK;
        let effect = self
            .piu
            .set_hardware_level(registers::INTERRUPT_MEMORY_ERROR, true);
        self.push_piu_effect(effect);
    }

    fn record_cpu_error(&mut self, address: u64) {
        let effect = self
            .piu
            .record_cpu_error(address, registers::CPU_ERROR_ILLEGAL_ADDRESS);
        self.push_piu_effect(effect);
    }

    fn apply_piu_effects(&mut self, effects: Vec<PiuEffect>) {
        for effect in effects {
            self.apply_piu_effect(effect);
        }
    }

    fn push_piu_effect(&mut self, effect: Option<PiuEffect>) {
        if let Some(effect) = effect {
            self.apply_piu_effect(effect);
        }
    }

    fn apply_piu_effect(&mut self, effect: PiuEffect) {
        match effect {
            PiuEffect::InterruptOutput(asserted) => {
                self.actions.push_back(CrimeAction::SetIrq(IrqTransaction {
                    source: IrqSource {
                        component: self.id,
                        output: CRIME_IRQ_OUTPUT,
                    },
                    asserted,
                }))
            }
            PiuEffect::ArmWatchdog {
                epoch,
                stage,
                delay,
            } => self.actions.push_back(CrimeAction::Schedule {
                delay,
                event: CrimeEvent::Watchdog { epoch, stage },
            }),
            PiuEffect::WarmReset => self
                .actions
                .push_back(CrimeAction::SignalCpu(CrimeCpuSignal::WarmReset)),
            PiuEffect::HardReset => self
                .actions
                .push_back(CrimeAction::SignalCpu(CrimeCpuSignal::HardReset)),
        }
    }

    fn complete_unsupported(
        &mut self,
        sysad_id: CrimeTransactionId,
        transfer: &CrimeTransfer,
        address: u64,
    ) {
        let result = match self.config.unimplemented_access_policy {
            CrimeAccessPolicy::Strict => Err(CrimeBusError::Address),
            CrimeAccessPolicy::Permissive => match transfer.view() {
                CrimeTransferView::Read { .. } => Ok(CrimeCompletionPayload::ReadData(
                    CrimeData::zeroed(transfer.length()),
                )),
                CrimeTransferView::Write { .. } => Ok(CrimeCompletionPayload::WriteComplete),
            },
        };
        if result.is_err() {
            self.record_cpu_error(address);
        }
        self.finish_sysad(sysad_id, result);
    }

    fn unsupported_read_result(&self, length: usize) -> CrimeSysAdResult {
        match self.config.unimplemented_access_policy {
            CrimeAccessPolicy::Strict => Err(CrimeBusError::Unsupported),
            CrimeAccessPolicy::Permissive => {
                Ok(CrimeCompletionPayload::ReadData(CrimeData::zeroed(length)))
            }
        }
    }

    fn unsupported_write_result(&self) -> CrimeSysAdResult {
        match self.config.unimplemented_access_policy {
            CrimeAccessPolicy::Strict => Err(CrimeBusError::Unsupported),
            CrimeAccessPolicy::Permissive => Ok(CrimeCompletionPayload::WriteComplete),
        }
    }

    fn finish_sysad(&mut self, sysad_id: CrimeTransactionId, result: CrimeSysAdResult) {
        if self.pending_sysad == Some(sysad_id) {
            self.pending_sysad = None;
        }
        self.actions
            .push_back(CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
                id: sysad_id,
                result,
            }));
    }

    fn allocate_transaction_id(&mut self) -> Result<CrimeTransactionId, CrimeError> {
        let id = CrimeTransactionId::new(self.next_transaction_id);
        self.next_transaction_id = self
            .next_transaction_id
            .checked_add(1)
            .ok_or(CrimeError::TransactionIdOverflow)?;
        Ok(id)
    }

    fn latch_error(&mut self, error: CrimeError) {
        if self.terminal_error.is_none() {
            self.terminal_error = Some(error);
        }
    }

    fn latch_render_error(&mut self, error: CrimeRenderError) {
        let fields: OwnedTraceFields = match &error {
            CrimeRenderError::InvalidPixelCommand {
                trigger_address,
                primitive,
                draw_mode,
                source_buffer_mode,
                destination_buffer_mode,
                feature_bits,
                violations,
            } => [
                OwnedTraceField {
                    key: "kind".into(),
                    value: OwnedTraceValue::String("invalid_pixel_command".into()),
                },
                OwnedTraceField {
                    key: "trigger_address".into(),
                    value: OwnedTraceValue::Hex64(*trigger_address),
                },
                OwnedTraceField {
                    key: "primitive".into(),
                    value: OwnedTraceValue::Hex64(u64::from(*primitive)),
                },
                OwnedTraceField {
                    key: "draw_mode".into(),
                    value: OwnedTraceValue::Hex64(u64::from(*draw_mode)),
                },
                OwnedTraceField {
                    key: "source_buffer_mode".into(),
                    value: OwnedTraceValue::Hex64(u64::from(*source_buffer_mode)),
                },
                OwnedTraceField {
                    key: "destination_buffer_mode".into(),
                    value: OwnedTraceValue::Hex64(u64::from(*destination_buffer_mode)),
                },
                OwnedTraceField {
                    key: "feature_bits".into(),
                    value: OwnedTraceValue::Hex64(u64::from(*feature_bits)),
                },
                OwnedTraceField {
                    key: "violation_count".into(),
                    value: OwnedTraceValue::U64(violations.len() as u64),
                },
                OwnedTraceField {
                    key: "violations".into(),
                    value: OwnedTraceValue::String(format!("{violations:?}").into()),
                },
            ]
            .into(),
            CrimeRenderError::InvalidMteJob { mode, field } => [
                OwnedTraceField {
                    key: "kind".into(),
                    value: OwnedTraceValue::String("invalid_mte_job".into()),
                },
                OwnedTraceField {
                    key: "mode".into(),
                    value: OwnedTraceValue::Hex64(u64::from(*mode)),
                },
                OwnedTraceField {
                    key: "field".into(),
                    value: OwnedTraceValue::String(format!("{field:?}").into()),
                },
            ]
            .into(),
            CrimeRenderError::InvalidMteRange { start, end } => [
                OwnedTraceField {
                    key: "kind".into(),
                    value: OwnedTraceValue::String("invalid_mte_range".into()),
                },
                OwnedTraceField {
                    key: "start".into(),
                    value: OwnedTraceValue::Hex64(u64::from(*start)),
                },
                OwnedTraceField {
                    key: "end".into(),
                    value: OwnedTraceValue::Hex64(u64::from(*end)),
                },
            ]
            .into(),
            CrimeRenderError::UnexpectedMemoryCompletion => [OwnedTraceField {
                key: "kind".into(),
                value: OwnedTraceValue::String("unexpected_memory_completion".into()),
            }]
            .into(),
            CrimeRenderError::UnexpectedMemoryPayload => [OwnedTraceField {
                key: "kind".into(),
                value: OwnedTraceValue::String("unexpected_memory_payload".into()),
            }]
            .into(),
            CrimeRenderError::MemoryTransport(error) => [
                OwnedTraceField {
                    key: "kind".into(),
                    value: OwnedTraceValue::String("memory_transport".into()),
                },
                OwnedTraceField {
                    key: "transport_error".into(),
                    value: OwnedTraceValue::String(
                        (match error {
                            CrimeBusError::Address => "address",
                            CrimeBusError::Access => "access",
                            CrimeBusError::Unsupported => "unsupported",
                            CrimeBusError::Timeout => "timeout",
                        })
                        .into(),
                    ),
                },
            ]
            .into(),
        };
        self.push_trace(|| OwnedTraceEvent {
            level: TraceLevel::Error,
            target: trace::RENDER_TARGET.into(),
            event: "render_error".into(),
            fields,
        });
        self.latch_error(CrimeError::Render(error));
    }
}

impl Component for Crime {
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

impl BusDeviceRole<CrimeSysAdRequest> for Crime {
    type Response = Result<(), CrimeError>;

    fn accept(&mut self, request: CrimeSysAdRequest) -> Self::Response {
        self.accept_sysad(request)
    }
}

impl BusControllerRole<CrimeMemoryCompletion> for Crime {
    fn complete(&mut self, completion: CrimeMemoryCompletion) {
        if let Err(error) = self.complete_memory(completion) {
            match error {
                CrimeError::Render(error) => self.latch_render_error(error),
                error => self.latch_error(error),
            }
        }
    }
}

impl BusControllerRole<CrimeCmiCompletion> for Crime {
    fn complete(&mut self, completion: CrimeCmiCompletion) {
        if let Err(error) = self.complete_cmi(completion) {
            self.latch_error(error);
        }
    }
}

impl BusControllerRole<CrimeCgiCompletion> for Crime {
    fn complete(&mut self, completion: CrimeCgiCompletion) {
        if let Err(error) = self.complete_cgi(completion) {
            self.latch_error(error);
        }
    }
}

impl BusDeviceRole<CrimeCmiTransaction> for Crime {
    type Response = CrimeLinkDeviceResponse<CrimeCmiCompletion>;

    fn accept(&mut self, transaction: CrimeCmiTransaction) -> Self::Response {
        match transaction.operation {
            CrimeLinkOperation::Dma(request) => {
                if let Err(error) =
                    self.accept_dma(transaction.id, request, CrimeMemoryClient::Mace, true)
                {
                    self.latch_error(error);
                }
                CrimeLinkDeviceResponse::Deferred
            }
            CrimeLinkOperation::InterruptPost(post) => {
                CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
                    id: transaction.id,
                    result: self
                        .accept_interrupt(post)
                        .map(|()| CrimeCompletionPayload::WriteComplete),
                    memory_fault: None,
                })
            }
            CrimeLinkOperation::Pio(_) => CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
                id: transaction.id,
                result: Err(CrimeBusError::Unsupported),
                memory_fault: None,
            }),
        }
    }
}

impl BusDeviceRole<CrimeCgiTransaction> for Crime {
    type Response = CrimeLinkDeviceResponse<CrimeCgiCompletion>;

    fn accept(&mut self, transaction: CrimeCgiTransaction) -> Self::Response {
        match transaction.operation {
            CrimeLinkOperation::Dma(request) => {
                if let Err(error) =
                    self.accept_dma(transaction.id, request, CrimeMemoryClient::Gbe, false)
                {
                    self.latch_error(error);
                }
                CrimeLinkDeviceResponse::Deferred
            }
            CrimeLinkOperation::InterruptPost(post) => {
                CrimeLinkDeviceResponse::Complete(CrimeCgiCompletion {
                    id: transaction.id,
                    result: self
                        .accept_interrupt(post)
                        .map(|()| CrimeCompletionPayload::WriteComplete),
                    memory_fault: None,
                })
            }
            CrimeLinkOperation::Pio(_) => CrimeLinkDeviceResponse::Complete(CrimeCgiCompletion {
                id: transaction.id,
                result: Err(CrimeBusError::Unsupported),
                memory_fault: None,
            }),
        }
    }
}

fn control_register_read_data(
    address: u64,
    length: u16,
    mut read: impl FnMut(u64) -> Option<u64>,
) -> Option<CrimeData> {
    if length == 4 && address & 3 == 0 {
        return control_register_word_read_data(address, read);
    }
    if length == 8 && address & 7 == 0 {
        return read(address).map(|value| value.to_be_bytes().into());
    }
    None
}

fn control_register_word_read_data(
    address: u64,
    mut read: impl FnMut(u64) -> Option<u64>,
) -> Option<CrimeData> {
    let value = read(address & !7)?;

    // SysAD obtains the complete aligned doubleword and selects the requested
    // big-endian word lane for a word transfer.
    let word = if address & 4 == 0 {
        (value >> 32) as u32
    } else {
        value as u32
    };
    Some(word.to_be_bytes().into())
}

fn sysad_memory_completion(result: Result<CrimeMemoryOutcome, CrimeBusError>) -> CrimeSysAdResult {
    result.map(|outcome| outcome.payload)
}

fn link_memory_completion(
    result: Result<CrimeMemoryOutcome, CrimeBusError>,
) -> (
    Result<CrimeCompletionPayload, CrimeBusError>,
    Option<CrimeMemoryFault>,
) {
    match result {
        Ok(outcome) => (Ok(outcome.payload), outcome.fault),
        Err(error) => (Err(error), None),
    }
}

fn decode_memory(address: u64, size: usize) -> Option<(u64, bool)> {
    if in_window(address, size, 0, LOW_MEMORY_END) {
        return Some((address, false));
    }
    if in_window(address, size, FRAMEBUFFER_START, FRAMEBUFFER_END) {
        return Some((address - FRAMEBUFFER_START, false));
    }
    if in_window(address, size, DEPTH_START, DEPTH_END) {
        return Some((address - DEPTH_START, false));
    }
    if in_window(address, size, LINEAR_MEMORY_START, LINEAR_MEMORY_END) {
        return Some((address - LINEAR_MEMORY_START, false));
    }
    if in_window(address, size, NO_ECC_MEMORY_START, NO_ECC_MEMORY_END) {
        return Some((address - NO_ECC_MEMORY_START, true));
    }
    None
}

/// Converts a Rendering Engine physical alias into the 30-bit MIU address domain.
///
/// RE TLB entries can name the CPU-visible linear-memory alias. Addresses outside
/// the CPU memory windows still reach the MIU request pipe, where programmable
/// bank controls determine whether any external bank is selected.
pub(super) fn normalize_render_memory_alias(address: u64) -> u64 {
    decode_memory(address, 1)
        .map(|(memory_address, _)| memory_address)
        .unwrap_or(address & 0x3fff_ffff)
}

fn in_window(address: u64, size: usize, start: u64, end: u64) -> bool {
    let size = u64::try_from(size).ok();
    address >= start
        && address < end
        && size
            .and_then(|size| address.checked_add(size))
            .is_some_and(|transfer_end| transfer_end <= end)
}

const fn reset_bank_control(memory: config::CrimeMemoryConfig) -> [u16; 8] {
    let mut controls = [0; 8];
    let mut index = 0;
    while index < controls.len() {
        controls[index] = index as u16;
        if let Some(bank) = memory.banks[index] {
            controls[index] |= bank.size.control_bit();
        }
        index += 1;
    }
    controls
}

fn encode_register_data(value: u64, size: u8) -> CrimeData {
    match size {
        4 => (value as u32).to_be_bytes().into(),
        8 => value.to_be_bytes().into(),
        _ => unreachable!("validated CRIME register sizes are four or eight bytes"),
    }
}

fn decode_register_data(data: &[u8]) -> Option<u64> {
    match data {
        [a, b, c, d] => Some(u64::from(u32::from_be_bytes([*a, *b, *c, *d]))),
        [a, b, c, d, e, f, g, h] => Some(u64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
