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
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use se_core::component::{Component, ComponentId};
use se_core::role::{BusControllerRole, BusDeviceRole};
use se_core::scheduler::SimTime;
use se_core::tracing::TraceLevel;

use crate::bus::irq::{IrqSource, IrqTransaction};
use crate::cpu::execution::protocol::{ExecutionCompletion, ExecutionTransactionId};
use crate::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};

use self::clock::CrimeClock;
use self::config::{CrimeAccessPolicy, CrimeConfig, CrimeConfigError};
use self::piu::{CrimePiu, PiuEffect};
use self::protocol::{
    CRIME_IRQ_OUTPUT, CrimeAction, CrimeBusError, CrimeCgiCompletion, CrimeCgiTransaction,
    CrimeCmiCompletion, CrimeCmiTransaction, CrimeCompletionPayload, CrimeCpuSignal,
    CrimeDmaRequest, CrimeEvent, CrimeInterruptPost, CrimeLinkDeviceResponse, CrimeLinkOperation,
    CrimeMemoryClient, CrimeMemoryCompletion, CrimeMemoryFault, CrimeMemoryOutcome,
    CrimeMemoryTransaction, CrimePioRequest, CrimePoll, CrimeSdramSignal, CrimeSysAdRequest,
    CrimeTraceEvent, CrimeTraceField, CrimeTraceValue, CrimeTransactionId, CrimeTransfer,
};
use self::render::{
    CrimeRender, CrimeRenderError, RenderInterruptEffect, RenderMemoryWrite, RenderNotice,
    RenderProgress, RenderWriteError,
};

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

#[derive(Clone, Debug, Eq, PartialEq)]
enum PendingMemoryOrigin {
    SysAd {
        execution_id: ExecutionTransactionId,
        request: Mips4ExecutionTransaction,
    },
    CmiDma {
        link_id: CrimeTransactionId,
    },
    CgiDma {
        link_id: CrimeTransactionId,
    },
    Render,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingLink {
    execution_id: ExecutionTransactionId,
    request: Mips4ExecutionTransaction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingRenderWrite {
    execution_id: ExecutionTransactionId,
    transaction: Mips4ExecutionTransaction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenderAccessResult {
    Complete(Mips4ExecutionCompletion),
    Deferred,
}

/// Terminal CRIME protocol or configuration error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrimeError {
    /// Chipset construction input is invalid.
    Configuration(CrimeConfigError),

    /// The machine timing ABI has no ticks.
    InvalidTimebase,

    /// A second SysAD request arrived while the R5000 request was outstanding.
    SysAdBusy {
        /// Outstanding CPU transaction.
        transaction_id: ExecutionTransactionId,
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
    pending_sysad: Option<ExecutionTransactionId>,
    pending_render_write: Option<PendingRenderWrite>,
    pending_memory: BTreeMap<CrimeTransactionId, PendingMemoryOrigin>,
    pending_cmi: BTreeMap<CrimeTransactionId, PendingLink>,
    pending_cgi: BTreeMap<CrimeTransactionId, PendingLink>,
    cancelled_memory: BTreeSet<CrimeTransactionId>,
    cancelled_cmi: BTreeSet<CrimeTransactionId>,
    cancelled_cgi: BTreeSet<CrimeTransactionId>,
    actions: VecDeque<CrimeAction>,
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
            pending_memory: BTreeMap::new(),
            pending_cmi: BTreeMap::new(),
            pending_cgi: BTreeMap::new(),
            cancelled_memory: BTreeSet::new(),
            cancelled_cmi: BTreeSet::new(),
            cancelled_cgi: BTreeSet::new(),
            actions: VecDeque::new(),
            terminal_error: None,
            current_time: SimTime::ZERO,
        })
    }

    /// Returns the immutable construction input.
    pub const fn config(&self) -> CrimeConfig {
        self.config
    }

    /// Observes machine time before accepting an untimed peer-link request.
    pub fn observe_time(&mut self, now: SimTime) {
        self.current_time = now;
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
        self.cancelled_cmi
            .extend(core::mem::take(&mut self.pending_cmi).into_keys());
        self.cancelled_cgi
            .extend(core::mem::take(&mut self.pending_cgi).into_keys());
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

    fn accept_sysad(&mut self, request: CrimeSysAdRequest) -> Result<(), CrimeError> {
        if let Some(transaction_id) = self.pending_sysad {
            return Err(CrimeError::SysAdBusy { transaction_id });
        }
        self.current_time = request.time;
        let execution_id = request.transaction.id;
        let transaction = request.transaction.payload;
        let (address, size) = transaction_shape(transaction);
        self.actions.push_back(CrimeAction::Trace(CrimeTraceEvent {
            level: TraceLevel::Trace,
            target: trace::PIU_TARGET,
            event: "sysad_request",
            fields: vec![
                CrimeTraceField {
                    key: "physical_address",
                    value: CrimeTraceValue::Hex64(address),
                },
                CrimeTraceField {
                    key: "size",
                    value: CrimeTraceValue::U64(u64::from(size)),
                },
            ],
        }));

        if let Some((memory_address, no_ecc)) = decode_memory(address, size) {
            let id = self.allocate_transaction_id()?;
            self.pending_sysad = Some(execution_id);
            self.pending_memory.insert(
                id,
                PendingMemoryOrigin::SysAd {
                    execution_id,
                    request: transaction,
                },
            );
            self.actions
                .push_back(CrimeAction::StartMemory(CrimeMemoryTransaction {
                    id,
                    time: request.time,
                    controller: self.id,
                    client: CrimeMemoryClient::Cpu,
                    address: memory_address,
                    no_ecc,
                    transfer: transfer_from_cpu(transaction),
                }));
            return Ok(());
        }
        if (registers::CRIME_BASE..registers::CRIME_REGISTER_END).contains(&address) {
            self.access_internal_register(execution_id, transaction, request.time)?;
            return Ok(());
        }
        if in_window(address, size, GBE_START, GBE_END) {
            let id = self.allocate_transaction_id()?;
            self.pending_sysad = Some(execution_id);
            self.pending_cgi.insert(
                id,
                PendingLink {
                    execution_id,
                    request: transaction,
                },
            );
            self.actions
                .push_back(CrimeAction::StartCgi(CrimeCgiTransaction {
                    id,
                    controller: self.id,
                    target: self.gbe_target,
                    operation: CrimeLinkOperation::Pio(CrimePioRequest {
                        address,
                        transfer: transfer_from_cpu(transaction),
                    }),
                }));
            return Ok(());
        }
        if in_window(address, size, MACE_LOW_START, MACE_LOW_END)
            || in_window(address, size, PCI_HIGH_START, PCI_HIGH_END)
        {
            let id = self.allocate_transaction_id()?;
            self.pending_sysad = Some(execution_id);
            self.pending_cmi.insert(
                id,
                PendingLink {
                    execution_id,
                    request: transaction,
                },
            );
            self.actions
                .push_back(CrimeAction::StartCmi(CrimeCmiTransaction {
                    id,
                    controller: self.id,
                    target: self.mace_target,
                    operation: CrimeLinkOperation::Pio(CrimePioRequest {
                        address,
                        transfer: transfer_from_cpu(transaction),
                    }),
                }));
            return Ok(());
        }
        let _vice = in_window(address, size, VICE_START, VICE_END);
        self.complete_unsupported(execution_id, transaction, address);
        Ok(())
    }

    fn access_internal_register(
        &mut self,
        execution_id: ExecutionTransactionId,
        transaction: Mips4ExecutionTransaction,
        now: SimTime,
    ) -> Result<(), CrimeError> {
        let (address, size) = transaction_shape(transaction);
        let completion = if address < registers::CRIME_RENDER_BASE {
            if size != 8 || address & 7 != 0 {
                Mips4ExecutionCompletion::BusError
            } else {
                self.access_control_register(transaction, now)
            }
        } else {
            match self.access_render_register(execution_id, transaction)? {
                RenderAccessResult::Complete(completion) => completion,
                RenderAccessResult::Deferred => return Ok(()),
            }
        };
        self.complete_internal_register_access(execution_id, transaction, completion);
        Ok(())
    }

    fn complete_internal_register_access(
        &mut self,
        execution_id: ExecutionTransactionId,
        transaction: Mips4ExecutionTransaction,
        completion: Mips4ExecutionCompletion,
    ) {
        let (address, size) = transaction_shape(transaction);
        let target = if address < registers::CRIME_BASE + 0x0200 {
            trace::PIU_TARGET
        } else if address < registers::CRIME_RENDER_BASE {
            trace::MIU_TARGET
        } else {
            trace::RENDER_TARGET
        };
        self.actions.push_back(CrimeAction::Trace(CrimeTraceEvent {
            level: if matches!(completion, Mips4ExecutionCompletion::BusError) {
                TraceLevel::Warn
            } else {
                TraceLevel::Trace
            },
            target,
            event: "register_access",
            fields: vec![
                CrimeTraceField {
                    key: "physical_address",
                    value: CrimeTraceValue::Hex64(address),
                },
                CrimeTraceField {
                    key: "size",
                    value: CrimeTraceValue::U64(u64::from(size)),
                },
                CrimeTraceField {
                    key: "operation",
                    value: CrimeTraceValue::String(match transaction {
                        Mips4ExecutionTransaction::Read { .. } => "read",
                        Mips4ExecutionTransaction::Write { .. } => "write",
                    }),
                },
                CrimeTraceField {
                    key: "bus_error",
                    value: CrimeTraceValue::Bool(matches!(
                        completion,
                        Mips4ExecutionCompletion::BusError
                    )),
                },
            ],
        }));
        if matches!(completion, Mips4ExecutionCompletion::BusError) {
            self.record_cpu_error(address);
        }
        self.finish_sysad(execution_id, completion);
    }

    fn access_control_register(
        &mut self,
        transaction: Mips4ExecutionTransaction,
        now: SimTime,
    ) -> Mips4ExecutionCompletion {
        let (address, _) = transaction_shape(transaction);
        match transaction {
            Mips4ExecutionTransaction::Read { .. } => {
                let value = self
                    .piu
                    .read(address, now, self.timebase_hz)
                    .or_else(|| self.read_memory_register(address));
                match value {
                    Some(value) => Mips4ExecutionCompletion::ReadData(value.swap_bytes()),
                    None => self.unsupported_read_completion(),
                }
            }
            Mips4ExecutionTransaction::Write {
                data, byte_enable, ..
            } => {
                if byte_enable != 0xff {
                    return Mips4ExecutionCompletion::BusError;
                }
                let value = data.swap_bytes();
                let result = self.piu.write(address, value, now, self.timebase_hz);
                if result.handled {
                    self.apply_piu_effects(result.effects);
                    Mips4ExecutionCompletion::WriteComplete
                } else if self.write_memory_register(address, value) {
                    Mips4ExecutionCompletion::WriteComplete
                } else {
                    self.unsupported_write_completion()
                }
            }
        }
    }

    fn access_render_register(
        &mut self,
        execution_id: ExecutionTransactionId,
        transaction: Mips4ExecutionTransaction,
    ) -> Result<RenderAccessResult, CrimeError> {
        let (address, size) = transaction_shape(transaction);
        let result = match transaction {
            Mips4ExecutionTransaction::Read { .. } => match self.render.read(address, size) {
                Some(value) => RenderAccessResult::Complete(Mips4ExecutionCompletion::ReadData(
                    encode_big_endian(value, size),
                )),
                None => RenderAccessResult::Complete(self.unsupported_read_completion()),
            },
            Mips4ExecutionTransaction::Write {
                data, byte_enable, ..
            } => {
                let expected_enable = ((1_u16 << size) - 1) as u8;
                if byte_enable != expected_enable {
                    return Ok(RenderAccessResult::Complete(
                        Mips4ExecutionCompletion::BusError,
                    ));
                }
                let value = decode_big_endian(data, size);
                match self.render.write(address, size, value) {
                    Ok(progress) => {
                        self.trace_render_register_write(address, size, value);
                        self.apply_render_progress(progress)?;
                        RenderAccessResult::Complete(Mips4ExecutionCompletion::WriteComplete)
                    }
                    Err(RenderWriteError::InterfaceFull) => {
                        self.pending_sysad = Some(execution_id);
                        self.pending_render_write = Some(PendingRenderWrite {
                            execution_id,
                            transaction,
                        });
                        RenderAccessResult::Deferred
                    }
                    Err(RenderWriteError::UndefinedRegister) => {
                        RenderAccessResult::Complete(self.unsupported_write_completion())
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
        let (address, size) = transaction_shape(pending.transaction);
        let Mips4ExecutionTransaction::Write { data, .. } = pending.transaction else {
            unreachable!("only RE writes can be deferred")
        };
        let value = decode_big_endian(data, size);
        let progress = self
            .render
            .write(address, size, value)
            .map_err(|error| match error {
                RenderWriteError::InterfaceFull => {
                    unreachable!("the RE interface space was checked before retry")
                }
                RenderWriteError::UndefinedRegister => {
                    unreachable!("the deferred RE write was validated before retry")
                }
            })?;
        self.trace_render_register_write(address, size, value);
        self.apply_render_progress(progress)?;
        self.complete_internal_register_access(
            pending.execution_id,
            pending.transaction,
            Mips4ExecutionCompletion::WriteComplete,
        );
        Ok(())
    }

    fn apply_render_progress(&mut self, progress: RenderProgress) -> Result<(), CrimeError> {
        self.apply_render_interrupts(progress.interrupts);
        for notice in progress.notices {
            self.trace_render_notice(notice);
        }
        if let Some(write) = progress.memory_write {
            self.start_render_memory(write)?;
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

    fn start_render_memory(&mut self, write: RenderMemoryWrite) -> Result<(), CrimeError> {
        let id = self.allocate_transaction_id()?;
        self.pending_memory.insert(id, PendingMemoryOrigin::Render);
        self.actions
            .push_back(CrimeAction::StartMemory(CrimeMemoryTransaction {
                id,
                time: self.current_time,
                controller: self.id,
                client: CrimeMemoryClient::Render,
                address: write.physical_address,
                no_ecc: false,
                transfer: CrimeTransfer::Write {
                    data: write.data,
                    byte_enable: write.byte_enable,
                },
            }));
        Ok(())
    }

    fn trace_render_register_write(&mut self, address: u64, size: u8, value: u64) {
        self.actions.push_back(CrimeAction::Trace(CrimeTraceEvent {
            level: TraceLevel::Trace,
            target: trace::RENDER_TARGET,
            event: "register_write",
            fields: vec![
                CrimeTraceField {
                    key: "physical_address",
                    value: CrimeTraceValue::Hex64(address),
                },
                CrimeTraceField {
                    key: "size",
                    value: CrimeTraceValue::U64(u64::from(size)),
                },
                CrimeTraceField {
                    key: "value",
                    value: CrimeTraceValue::Hex64(value),
                },
                CrimeTraceField {
                    key: "commit",
                    value: CrimeTraceValue::Bool(
                        (registers::CRIME_RENDER_BASE + 0x3800
                            ..=registers::CRIME_RENDER_BASE + 0x3878)
                            .contains(&address),
                    ),
                },
            ],
        }));
    }

    fn trace_render_notice(&mut self, notice: RenderNotice) {
        let (event, fields) = match notice {
            RenderNotice::RegisterRetired(write) => (
                "register_retired",
                vec![
                    CrimeTraceField {
                        key: "physical_address",
                        value: CrimeTraceValue::Hex64(write.address),
                    },
                    CrimeTraceField {
                        key: "commit",
                        value: CrimeTraceValue::Bool(write.commit),
                    },
                ],
            ),
            RenderNotice::JobCommitted { start, end } => (
                "job_commit",
                vec![
                    CrimeTraceField {
                        key: "start",
                        value: CrimeTraceValue::Hex64(u64::from(start)),
                    },
                    CrimeTraceField {
                        key: "end",
                        value: CrimeTraceValue::Hex64(u64::from(end)),
                    },
                ],
            ),
            RenderNotice::MemoryChunk {
                virtual_address,
                physical_address,
                length,
            } => (
                "mte_chunk",
                vec![
                    CrimeTraceField {
                        key: "virtual_address",
                        value: CrimeTraceValue::Hex64(u64::from(virtual_address)),
                    },
                    CrimeTraceField {
                        key: "physical_address",
                        value: CrimeTraceValue::Hex64(physical_address),
                    },
                    CrimeTraceField {
                        key: "length",
                        value: CrimeTraceValue::U64(u64::from(length)),
                    },
                ],
            ),
            RenderNotice::JobCompleted { start, end } => (
                "job_complete",
                vec![
                    CrimeTraceField {
                        key: "start",
                        value: CrimeTraceValue::Hex64(u64::from(start)),
                    },
                    CrimeTraceField {
                        key: "end",
                        value: CrimeTraceValue::Hex64(u64::from(end)),
                    },
                ],
            ),
        };
        self.actions.push_back(CrimeAction::Trace(CrimeTraceEvent {
            level: TraceLevel::Debug,
            target: trace::RENDER_TARGET,
            event,
            fields,
        }));
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
            PendingMemoryOrigin::SysAd {
                execution_id,
                request,
            } => {
                let payload = cpu_memory_completion(completion.result);
                if matches!(payload, Mips4ExecutionCompletion::BusError) {
                    self.record_cpu_error(transaction_shape(request).0);
                }
                self.finish_sysad(execution_id, payload);
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
        let payload = cpu_link_completion(completion.result);
        if matches!(payload, Mips4ExecutionCompletion::BusError) {
            self.record_cpu_error(transaction_shape(pending.request).0);
        }
        self.finish_sysad(pending.execution_id, payload);
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
        let payload = cpu_link_completion(completion.result);
        if matches!(payload, Mips4ExecutionCompletion::BusError) {
            self.record_cpu_error(transaction_shape(pending.request).0);
        }
        self.finish_sysad(pending.execution_id, payload);
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
        let Some(diagnostic) = outcome.diagnostic else {
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
        execution_id: ExecutionTransactionId,
        request: Mips4ExecutionTransaction,
        address: u64,
    ) {
        let completion = match self.config.unimplemented_access_policy {
            CrimeAccessPolicy::Strict => Mips4ExecutionCompletion::BusError,
            CrimeAccessPolicy::Permissive => match request {
                Mips4ExecutionTransaction::Read { .. } => Mips4ExecutionCompletion::ReadData(0),
                Mips4ExecutionTransaction::Write { .. } => Mips4ExecutionCompletion::WriteComplete,
            },
        };
        if matches!(completion, Mips4ExecutionCompletion::BusError) {
            self.record_cpu_error(address);
        }
        self.finish_sysad(execution_id, completion);
    }

    fn unsupported_read_completion(&self) -> Mips4ExecutionCompletion {
        match self.config.unimplemented_access_policy {
            CrimeAccessPolicy::Strict => Mips4ExecutionCompletion::BusError,
            CrimeAccessPolicy::Permissive => Mips4ExecutionCompletion::ReadData(0),
        }
    }

    fn unsupported_write_completion(&self) -> Mips4ExecutionCompletion {
        match self.config.unimplemented_access_policy {
            CrimeAccessPolicy::Strict => Mips4ExecutionCompletion::BusError,
            CrimeAccessPolicy::Permissive => Mips4ExecutionCompletion::WriteComplete,
        }
    }

    fn finish_sysad(
        &mut self,
        execution_id: ExecutionTransactionId,
        payload: Mips4ExecutionCompletion,
    ) {
        if self.pending_sysad == Some(execution_id) {
            self.pending_sysad = None;
        }
        self.actions
            .push_back(CrimeAction::CompleteSysAd(ExecutionCompletion {
                id: execution_id,
                payload,
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
        let fields = match &error {
            CrimeRenderError::UnsupportedMteJob {
                mode,
                byte_mask,
                foreground,
            } => vec![
                CrimeTraceField {
                    key: "kind",
                    value: CrimeTraceValue::String("unsupported_mte_job"),
                },
                CrimeTraceField {
                    key: "mode",
                    value: CrimeTraceValue::Hex64(u64::from(*mode)),
                },
                CrimeTraceField {
                    key: "byte_mask",
                    value: CrimeTraceValue::Hex64(u64::from(*byte_mask)),
                },
                CrimeTraceField {
                    key: "foreground",
                    value: CrimeTraceValue::Hex64(u64::from(*foreground)),
                },
            ],
            CrimeRenderError::InvalidMteRange { start, end } => vec![
                CrimeTraceField {
                    key: "kind",
                    value: CrimeTraceValue::String("invalid_mte_range"),
                },
                CrimeTraceField {
                    key: "start",
                    value: CrimeTraceValue::Hex64(u64::from(*start)),
                },
                CrimeTraceField {
                    key: "end",
                    value: CrimeTraceValue::Hex64(u64::from(*end)),
                },
            ],
            CrimeRenderError::InvalidLinearTlb {
                virtual_address,
                entry,
            } => vec![
                CrimeTraceField {
                    key: "kind",
                    value: CrimeTraceValue::String("invalid_linear_tlb"),
                },
                CrimeTraceField {
                    key: "virtual_address",
                    value: CrimeTraceValue::Hex64(u64::from(*virtual_address)),
                },
                CrimeTraceField {
                    key: "entry",
                    value: CrimeTraceValue::Hex64(u64::from(*entry)),
                },
            ],
            CrimeRenderError::UnexpectedMemoryCompletion => vec![CrimeTraceField {
                key: "kind",
                value: CrimeTraceValue::String("unexpected_memory_completion"),
            }],
            CrimeRenderError::UnexpectedMemoryPayload => vec![CrimeTraceField {
                key: "kind",
                value: CrimeTraceValue::String("unexpected_memory_payload"),
            }],
            CrimeRenderError::MemoryTransport(error) => vec![
                CrimeTraceField {
                    key: "kind",
                    value: CrimeTraceValue::String("memory_transport"),
                },
                CrimeTraceField {
                    key: "transport_error",
                    value: CrimeTraceValue::String(match error {
                        CrimeBusError::Address => "address",
                        CrimeBusError::Access => "access",
                        CrimeBusError::Unsupported => "unsupported",
                        CrimeBusError::Timeout => "timeout",
                    }),
                },
            ],
        };
        self.actions.push_back(CrimeAction::Trace(CrimeTraceEvent {
            level: TraceLevel::Error,
            target: trace::RENDER_TARGET,
            event: "render_error",
            fields,
        }));
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

fn transaction_shape(transaction: Mips4ExecutionTransaction) -> (u64, u8) {
    match transaction {
        Mips4ExecutionTransaction::Read {
            physical_address,
            size,
            ..
        }
        | Mips4ExecutionTransaction::Write {
            physical_address,
            size,
            ..
        } => (physical_address, size.bytes()),
    }
}

fn transfer_from_cpu(transaction: Mips4ExecutionTransaction) -> CrimeTransfer {
    match transaction {
        Mips4ExecutionTransaction::Read { size, .. } => CrimeTransfer::Read {
            length: u16::from(size.bytes()),
        },
        Mips4ExecutionTransaction::Write {
            size,
            data,
            byte_enable,
            ..
        } => {
            let length = usize::from(size.bytes());
            CrimeTransfer::Write {
                data: data.to_le_bytes()[..length].to_vec(),
                byte_enable: (0..length)
                    .map(|lane| byte_enable & (1 << lane) != 0)
                    .collect(),
            }
        }
    }
}

fn cpu_link_completion(
    result: Result<CrimeCompletionPayload, CrimeBusError>,
) -> Mips4ExecutionCompletion {
    match result {
        Ok(CrimeCompletionPayload::ReadData(data)) if data.len() <= 8 => {
            let mut lanes = [0; 8];
            lanes[..data.len()].copy_from_slice(&data);
            Mips4ExecutionCompletion::ReadData(u64::from_le_bytes(lanes))
        }
        Ok(CrimeCompletionPayload::WriteComplete) => Mips4ExecutionCompletion::WriteComplete,
        Ok(CrimeCompletionPayload::ReadData(_)) | Err(_) => Mips4ExecutionCompletion::BusError,
    }
}

fn cpu_memory_completion(
    result: Result<CrimeMemoryOutcome, CrimeBusError>,
) -> Mips4ExecutionCompletion {
    match result {
        Ok(outcome) => cpu_link_completion(Ok(outcome.payload)),
        Err(error) => cpu_link_completion(Err(error)),
    }
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

fn decode_memory(address: u64, size: u8) -> Option<(u64, bool)> {
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

fn in_window(address: u64, size: u8, start: u64, end: u64) -> bool {
    address >= start
        && address < end
        && address
            .checked_add(u64::from(size))
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

fn encode_big_endian(value: u64, size: u8) -> u64 {
    match size {
        4 => u64::from_le_bytes([
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
            0,
            0,
            0,
            0,
        ]),
        8 => value.swap_bytes(),
        _ => 0,
    }
}

fn decode_big_endian(data: u64, size: u8) -> u64 {
    match size {
        4 => u64::from(u32::from_be_bytes(
            data.to_le_bytes()[..4].try_into().unwrap(),
        )),
        8 => data.swap_bytes(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
