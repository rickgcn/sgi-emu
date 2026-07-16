use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ip32FastCodeTimeline {
    SystemFlash {
        clock: FractionalClockProjection,
        fixed_ticks_per_fetch: u64,
        fetch_limit: u64,
    },
    Sdram {
        clock: FractionalClockProjection,
        fetch_limit: u64,
    },
}

pub(super) struct Ip32FastMemoryContext {
    start_time_ticks: u64,
    available_ticks: u64,
    cpu_clock: FractionalClockProjection,
    sysad_clock: FractionalClockProjection,
    cmi_clock: FractionalClockProjection,
    mace_ust: se_device::chipset::mace::peripheral::MaceUstProjection,
    code_timeline: Option<Ip32FastCodeTimeline>,
    full_budget_admitted: bool,
    attempts: u64,
    completed: u64,
    cmi_completed: u64,
    last_transaction_fetch: u64,
    last_cmi_transaction_fetch: u64,
    last_delivery_ticks: u64,
    last_cmi_delivery_ticks: u64,
}

impl Ip32FastMemoryContext {
    #[allow(clippy::too_many_arguments)]
    pub(super) const fn new(
        start_time_ticks: u64,
        available_ticks: u64,
        cpu_clock: FractionalClockProjection,
        sysad_clock: FractionalClockProjection,
        cmi_clock: FractionalClockProjection,
        mace_ust: se_device::chipset::mace::peripheral::MaceUstProjection,
        full_budget_admitted: bool,
    ) -> Self {
        Self {
            start_time_ticks,
            available_ticks,
            cpu_clock,
            sysad_clock,
            cmi_clock,
            mace_ust,
            code_timeline: None,
            full_budget_admitted,
            attempts: 0,
            completed: 0,
            cmi_completed: 0,
            last_transaction_fetch: 0,
            last_cmi_transaction_fetch: 0,
            last_delivery_ticks: 0,
            last_cmi_delivery_ticks: 0,
        }
    }

    pub(super) fn configure_code_fetch_timeline(
        &mut self,
        source: Ip32StableCodeSource,
        auxiliary_clock: FractionalClockProjection,
        fixed_ticks_per_fetch: u64,
        fetch_limit: u64,
    ) -> bool {
        if self.code_timeline.is_some()
            || fetch_limit == 0
            || fetch_limit > 256
            || auxiliary_clock.frequency_hz() <= 1
            || auxiliary_clock.timebase_hz() <= 1
            || auxiliary_clock.elapsed(512).is_none()
        {
            return false;
        }
        self.code_timeline = Some(match source {
            Ip32StableCodeSource::SystemFlash if auxiliary_clock == self.cmi_clock => {
                Ip32FastCodeTimeline::SystemFlash {
                    clock: auxiliary_clock,
                    fixed_ticks_per_fetch,
                    fetch_limit,
                }
            }
            Ip32StableCodeSource::Sdram if fixed_ticks_per_fetch == 0 => {
                Ip32FastCodeTimeline::Sdram {
                    clock: auxiliary_clock,
                    fetch_limit,
                }
            }
            _ => return false,
        });
        true
    }

    pub(super) const fn cpu_clock(&self) -> FractionalClockProjection {
        self.cpu_clock
    }

    pub(super) const fn bus_clock(&self) -> FractionalClockProjection {
        self.sysad_clock
    }

    pub(super) const fn cmi_clock(&self) -> FractionalClockProjection {
        self.cmi_clock
    }

    pub(super) const fn code_fetch_active(&self) -> bool {
        self.code_timeline.is_some()
    }

    pub(super) const fn code_fetch_shares_cmi(&self) -> bool {
        matches!(
            self.code_timeline,
            Some(Ip32FastCodeTimeline::SystemFlash { .. })
        )
    }

    pub(super) const fn code_aux_clock(&self) -> FractionalClockProjection {
        match self.code_timeline {
            Some(Ip32FastCodeTimeline::SystemFlash { clock, .. })
            | Some(Ip32FastCodeTimeline::Sdram { clock, .. }) => clock,
            None => self.cmi_clock,
        }
    }

    pub(super) const fn code_fetch_fixed_ticks(&self) -> u64 {
        match self.code_timeline {
            Some(Ip32FastCodeTimeline::SystemFlash {
                fixed_ticks_per_fetch,
                ..
            }) => fixed_ticks_per_fetch,
            Some(Ip32FastCodeTimeline::Sdram { .. }) | None => 0,
        }
    }

    pub(super) const fn code_fetch_limit(&self) -> u64 {
        match self.code_timeline {
            Some(Ip32FastCodeTimeline::SystemFlash { fetch_limit, .. })
            | Some(Ip32FastCodeTimeline::Sdram { fetch_limit, .. }) => fetch_limit,
            None => 0,
        }
    }

    pub(super) const fn start_time_ticks(&self) -> u64 {
        self.start_time_ticks
    }

    pub(super) const fn available_ticks(&self) -> u64 {
        self.available_ticks
    }

    pub(super) const fn full_budget_admitted(&self) -> bool {
        self.full_budget_admitted
    }

    pub(super) fn limit_available_ticks(&mut self, available_ticks: u64) {
        if available_ticks < self.available_ticks {
            self.available_ticks = available_ticks;
            self.full_budget_admitted = false;
        }
    }

    pub(super) fn record_attempt(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
    }

    pub(super) fn record_completion(&mut self, delivery_ticks: u64, fetches: u64, uses_cmi: bool) {
        self.completed = self.completed.saturating_add(1);
        self.last_delivery_ticks = delivery_ticks;
        self.last_transaction_fetch = fetches;
        if uses_cmi {
            self.cmi_completed = self.cmi_completed.saturating_add(1);
            self.last_cmi_delivery_ticks = delivery_ticks;
            self.last_cmi_transaction_fetch = fetches;
        }
    }

    pub(super) const fn attempts(&self) -> u64 {
        self.attempts
    }

    pub(super) const fn completed(&self) -> u64 {
        self.completed
    }

    pub(super) const fn cmi_completed(&self) -> u64 {
        self.cmi_completed
    }

    pub(super) const fn last_delivery_ticks(&self) -> u64 {
        self.last_delivery_ticks
    }

    pub(super) const fn last_cmi_delivery_ticks(&self) -> u64 {
        self.last_cmi_delivery_ticks
    }

    pub(super) const fn last_transaction_fetch(&self) -> u64 {
        self.last_transaction_fetch
    }

    pub(super) const fn last_cmi_transaction_fetch(&self) -> u64 {
        self.last_cmi_transaction_fetch
    }
}

pub(super) struct Ip32FastMemoryRuntime {
    pub(super) context: Ip32FastMemoryContext,
    pub(super) crime: CrimeSynchronousReadSnapshot,
}

impl Ip32FastMemoryRuntime {
    pub(super) fn elapsed_ticks(&self, code_fetches: u64) -> Option<u64> {
        let code_fetches = if self.context.code_fetch_active() {
            code_fetches
        } else {
            0
        };
        let sysad = self
            .context
            .bus_clock()
            .elapsed(
                self.context
                    .completed()
                    .checked_add(code_fetches)?
                    .checked_mul(2)?,
            )
            .map(SimDuration::get)?;
        let shared_fetches = if self.context.code_fetch_shares_cmi() {
            code_fetches
        } else {
            0
        };
        let cmi = self
            .context
            .cmi_clock()
            .elapsed(
                self.context
                    .cmi_completed()
                    .checked_add(shared_fetches)?
                    .checked_mul(2)?,
            )
            .map(SimDuration::get)?;
        let auxiliary = if self.context.code_fetch_active() && !self.context.code_fetch_shares_cmi()
        {
            self.context
                .code_aux_clock()
                .elapsed(code_fetches.checked_mul(2)?)
                .map(SimDuration::get)?
        } else {
            0
        };
        let fixed = self
            .context
            .code_fetch_fixed_ticks()
            .checked_mul(code_fetches)?;
        sysad
            .checked_add(cmi)?
            .checked_add(auxiliary)?
            .checked_add(fixed)
    }

    fn transaction_timeline(
        &self,
        retired_boundaries: u64,
        uses_cmi: bool,
    ) -> Option<(u64, u64, u64)> {
        let cpu = self
            .context
            .cpu_clock()
            .elapsed(retired_boundaries)
            .map(SimDuration::get)?;
        let code_fetches = self
            .context
            .code_fetch_active()
            .then(|| retired_boundaries.checked_add(1))
            .flatten()
            .unwrap_or(0);
        let sysad_prefix = code_fetches.checked_add(self.context.completed())?;
        let sysad_request = self
            .context
            .bus_clock()
            .elapsed(sysad_prefix.checked_mul(2)?.checked_add(1)?)
            .map(SimDuration::get)?;
        let sysad_completion = self
            .context
            .bus_clock()
            .elapsed(sysad_prefix.checked_mul(2)?.checked_add(2)?)
            .map(SimDuration::get)?;
        let shared_fetches = if self.context.code_fetch_shares_cmi() {
            code_fetches
        } else {
            0
        };
        let cmi_prefix = shared_fetches.checked_add(self.context.cmi_completed())?;
        let cmi_request = self
            .context
            .cmi_clock()
            .elapsed(
                cmi_prefix
                    .checked_mul(2)?
                    .checked_add(u64::from(uses_cmi))?,
            )
            .map(SimDuration::get)?;
        let cmi_completion = self
            .context
            .cmi_clock()
            .elapsed(
                cmi_prefix
                    .checked_mul(2)?
                    .checked_add(u64::from(uses_cmi).checked_mul(2)?)?,
            )
            .map(SimDuration::get)?;
        let auxiliary = if self.context.code_fetch_active() && !self.context.code_fetch_shares_cmi()
        {
            self.context
                .code_aux_clock()
                .elapsed(code_fetches.checked_mul(2)?)
                .map(SimDuration::get)?
        } else {
            0
        };
        let fixed = self
            .context
            .code_fetch_fixed_ticks()
            .checked_mul(code_fetches)?;
        let common = cpu.checked_add(auxiliary)?.checked_add(fixed)?;
        let delivery = common
            .checked_add(sysad_request)?
            .checked_add(cmi_request)?;
        let completion = common
            .checked_add(sysad_completion)?
            .checked_add(cmi_completion)?;
        Some((delivery, completion, code_fetches))
    }

    fn combined_elapsed_ticks(
        &self,
        retirements: u64,
        completed: u64,
        cmi_completed: u64,
    ) -> Option<u64> {
        let code_fetches = if self.context.code_fetch_active() {
            retirements
        } else {
            0
        };
        let shared_fetches = if self.context.code_fetch_shares_cmi() {
            code_fetches
        } else {
            0
        };
        let cpu = self
            .context
            .cpu_clock()
            .elapsed(retirements)
            .map(SimDuration::get)?;
        let sysad = self
            .context
            .bus_clock()
            .elapsed(code_fetches.checked_add(completed)?.checked_mul(2)?)
            .map(SimDuration::get)?;
        let cmi = self
            .context
            .cmi_clock()
            .elapsed(shared_fetches.checked_add(cmi_completed)?.checked_mul(2)?)
            .map(SimDuration::get)?;
        let auxiliary = if self.context.code_fetch_active() && !self.context.code_fetch_shares_cmi()
        {
            self.context
                .code_aux_clock()
                .elapsed(code_fetches.checked_mul(2)?)
                .map(SimDuration::get)?
        } else {
            0
        };
        let fixed = self
            .context
            .code_fetch_fixed_ticks()
            .checked_mul(code_fetches)?;
        cpu.checked_add(sysad)?
            .checked_add(cmi)?
            .checked_add(auxiliary)?
            .checked_add(fixed)
    }

    fn combined_retirement_limit(
        &self,
        retired: u64,
        completed: u64,
        cmi_completed: u64,
    ) -> Option<u64> {
        let mut lower = retired;
        let mut upper = self.context.code_fetch_limit();
        if self.context.full_budget_admitted() {
            return Some(upper);
        }
        if self.combined_elapsed_ticks(upper, completed, cmi_completed)?
            < self.context.available_ticks()
        {
            return Some(upper);
        }
        while lower < upper {
            let candidate = lower + (upper - lower).div_ceil(2);
            if self.combined_elapsed_ticks(candidate, completed, cmi_completed)?
                < self.context.available_ticks()
            {
                lower = candidate;
            } else {
                upper = candidate - 1;
            }
        }
        Some(lower)
    }

    pub(super) fn last_delivery_time(&self) -> Option<SimTime> {
        SimTime::new(self.context.start_time_ticks())
            .checked_add(SimDuration::new(self.context.last_delivery_ticks()))
    }

    pub(super) fn last_cmi_delivery_time(&self) -> Option<SimTime> {
        SimTime::new(self.context.start_time_ticks())
            .checked_add(SimDuration::new(self.context.last_cmi_delivery_ticks()))
    }

    pub(super) fn last_code_delivery_time(
        &self,
        code_fetches: u64,
        auxiliary_delivery: bool,
    ) -> Option<SimTime> {
        if !self.context.code_fetch_active() || code_fetches == 0 {
            return None;
        }
        let previous_fetches = code_fetches.checked_sub(1)?;
        let data_before = self.context.completed().checked_sub(u64::from(
            self.context.last_transaction_fetch() == code_fetches,
        ))?;
        let cmi_before = self.context.cmi_completed().checked_sub(u64::from(
            self.context.last_cmi_transaction_fetch() == code_fetches,
        ))?;
        let cpu = self
            .context
            .cpu_clock()
            .elapsed(previous_fetches)
            .map(SimDuration::get)?;
        let sysad = self
            .context
            .bus_clock()
            .elapsed(
                previous_fetches
                    .checked_add(data_before)?
                    .checked_mul(2)?
                    .checked_add(1)?,
            )
            .map(SimDuration::get)?;
        let auxiliary = if self.context.code_fetch_shares_cmi() {
            self.context
                .cmi_clock()
                .elapsed(
                    previous_fetches
                        .checked_add(cmi_before)?
                        .checked_mul(2)?
                        .checked_add(u64::from(auxiliary_delivery))?,
                )
                .map(SimDuration::get)?
        } else {
            let code = self
                .context
                .code_aux_clock()
                .elapsed(
                    previous_fetches
                        .checked_mul(2)?
                        .checked_add(u64::from(auxiliary_delivery))?,
                )
                .map(SimDuration::get)?;
            let cmi = self
                .context
                .cmi_clock()
                .elapsed(cmi_before.checked_mul(2)?)
                .map(SimDuration::get)?;
            code.checked_add(cmi)?
        };
        let fixed = self
            .context
            .code_fetch_fixed_ticks()
            .checked_mul(previous_fetches)?;
        let ticks = cpu
            .checked_add(sysad)?
            .checked_add(auxiliary)?
            .checked_add(fixed)?;
        SimTime::new(self.context.start_time_ticks()).checked_add(SimDuration::new(ticks))
    }

    fn retirement_limit(
        &self,
        request: Mips4FastMemoryReadRequest,
        completion_ticks: u64,
        uses_cmi: bool,
    ) -> Option<u64> {
        if self.context.code_fetch_active() {
            return self.combined_retirement_limit(
                request.retired_boundaries(),
                self.context.completed().checked_add(1)?,
                self.context
                    .cmi_completed()
                    .checked_add(u64::from(uses_cmi))?,
            );
        }
        let cpu_ticks = self
            .context
            .cpu_clock()
            .elapsed(request.retired_boundaries())?
            .get();
        let cpu_tick_limit = self
            .context
            .available_ticks()
            .checked_sub(completion_ticks.saturating_sub(cpu_ticks))?
            .checked_sub(1)?;
        cpu_tick_limit
            .checked_add(1)
            .and_then(|ticks| {
                self.context
                    .cpu_clock()
                    .cycles_until_elapsed_at_least(ticks)
            })
            .map(|cycles| cycles.saturating_sub(1))
    }

    fn mace_ust_value(&self, delivery_time: SimTime, size: u32) -> Option<u64> {
        let elapsed = delivery_time
            .get()
            .saturating_sub(self.context.mace_ust.base_time.get());
        let increments = u128::from(elapsed)
            .saturating_mul(u128::from(self.context.mace_ust.frequency_hz))
            / u128::from(self.context.mace_ust.timebase_hz);
        let value = self.context.mace_ust.base.wrapping_add(increments as u32);
        match size {
            4 => {
                let bytes = value.to_be_bytes();
                Some(u64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0,
                ]))
            }
            8 => Some(u64::from(value).swap_bytes()),
            _ => None,
        }
    }
}

impl Mips4FastMemoryRuntime for Ip32FastMemoryRuntime {
    fn read(&mut self, request: Mips4FastMemoryReadRequest) -> Mips4FastMemoryReadResult {
        const CANDIDATE_RANGE: core::ops::Range<u64> =
            se_device::chipset::crime::registers::CRIME_BASE
                ..se_device::chipset::mace::registers::PRIMARY_END;

        if !request.is_uncached_data_load()
            || !CANDIDATE_RANGE.contains(&request.physical_address())
        {
            return Mips4FastMemoryReadResult::Unavailable;
        }
        self.context.record_attempt();
        let crime_timer = (se_device::chipset::crime::registers::TIMER
            ..se_device::chipset::crime::registers::TIMER + 8)
            .contains(&request.physical_address());
        let mace_ust = match request.size() {
            4 => {
                request.physical_address()
                    == se_device::chipset::mace::registers::UST.saturating_add(4)
            }
            8 => request.physical_address() == se_device::chipset::mace::registers::UST,
            _ => false,
        };
        if !crime_timer && !mace_ust {
            return Mips4FastMemoryReadResult::Unavailable;
        }
        let uses_cmi = mace_ust;
        let size = match request.size() {
            1 => Mips4ExecutionTransferSize::Byte,
            2 => Mips4ExecutionTransferSize::Halfword,
            4 => Mips4ExecutionTransferSize::Word,
            8 => Mips4ExecutionTransferSize::Doubleword,
            _ => return Mips4FastMemoryReadResult::InternalError,
        };
        let Some((delivery_ticks, completion_ticks, code_fetches)) =
            self.transaction_timeline(request.retired_boundaries(), uses_cmi)
        else {
            return Mips4FastMemoryReadResult::InternalError;
        };
        if completion_ticks >= self.context.available_ticks() {
            return if self.context.completed() == 0 && request.retired_boundaries() == 0 {
                Mips4FastMemoryReadResult::Unavailable
            } else {
                Mips4FastMemoryReadResult::TimelineExhausted
            };
        }
        let Some(retirement_limit) = self.retirement_limit(request, completion_ticks, uses_cmi)
        else {
            return Mips4FastMemoryReadResult::InternalError;
        };
        if retirement_limit <= request.retired_boundaries() {
            return if self.context.completed() == 0 && request.retired_boundaries() == 0 {
                Mips4FastMemoryReadResult::Unavailable
            } else {
                Mips4FastMemoryReadResult::TimelineExhausted
            };
        }
        let Some(delivery_time) = SimTime::new(self.context.start_time_ticks())
            .checked_add(SimDuration::new(delivery_ticks))
        else {
            return Mips4FastMemoryReadResult::InternalError;
        };
        let value = if mace_ust {
            let Some(value) = self.mace_ust_value(delivery_time, request.size()) else {
                return Mips4FastMemoryReadResult::Unavailable;
            };
            value
        } else {
            let transaction = Mips4ExecutionTransaction::Read {
                physical_address: request.physical_address(),
                size,
                kind: Mips4ExecutionAccessKind::DataLoad,
                access_type: se_device::cpu::mips4::cache::Mips4MemoryAccessType::Uncached,
            };
            let Some(Mips4ExecutionCompletion::ReadData(value)) =
                self.crime.read(transaction, delivery_time)
            else {
                return Mips4FastMemoryReadResult::Unavailable;
            };
            value
        };
        self.context
            .record_completion(delivery_ticks, code_fetches, uses_cmi);
        Mips4FastMemoryReadResult::Complete {
            value,
            retirement_limit,
        }
    }

    fn completed_transactions(&self) -> u64 {
        self.context.completed()
    }
}

pub(super) fn configure_fast_memory_code_timeline(
    registry: &ComponentRegistry,
    control: &MachineControl,
    runtime: &mut Ip32FastMemoryRuntime,
    window: &Mips4CodeWindow,
) -> Result<(), Ip32MachineDispatchError> {
    let source = Ip32StableCodeSource::from_guard(window.guard())?;
    if source == Ip32StableCodeSource::Sdram
        && let Some(deadline) = registry
            .get_resolved(control.slots.memory)?
            .stable_fetch_refresh_deadline()
    {
        runtime.context.limit_available_ticks(
            deadline
                .get()
                .saturating_sub(runtime.context.start_time_ticks()),
        );
    }
    let (auxiliary_clock, fixed_ticks_per_fetch) = match source {
        Ip32StableCodeSource::SystemFlash => (
            registry
                .get_resolved(control.slots.cmi)?
                .stable_fetch_clock()
                .ok_or_else(|| {
                    Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                        "stable PROM code lost its CMI clock before execution".to_owned(),
                    ))
                })?,
            registry
                .get_resolved(control.slots.isa)?
                .stable_fetch_delay()
                .ok_or_else(|| {
                    Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                        "stable PROM code lost its ISA delay before execution".to_owned(),
                    ))
                })?
                .get(),
        ),
        Ip32StableCodeSource::Sdram => (
            registry
                .get_resolved(control.slots.memory)?
                .stable_fetch_clock()
                .ok_or_else(|| {
                    Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                        "stable RAM code lost its memory clock before execution".to_owned(),
                    ))
                })?,
            0,
        ),
    };
    if !runtime.context.configure_code_fetch_timeline(
        source,
        auxiliary_clock,
        fixed_ticks_per_fetch,
        window.fetch_count() as u64,
    ) {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "stable code and fast-memory timelines were incompatible".to_owned(),
        )));
    }
    Ok(())
}

pub(super) fn plan_fast_memory_runtime<S>(
    registry: &ComponentRegistry,
    context: &Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    tracing_disabled: bool,
) -> Result<Option<Ip32FastMemoryRuntime>, Ip32MachineDispatchError>
where
    S: TraceSink,
{
    if !tracing_disabled || context.stop_requested() {
        return Ok(None);
    }
    let Some(sysad_clock) = registry
        .get_resolved(control.slots.sysad)?
        .stable_fetch_clock()
    else {
        return Ok(None);
    };
    let Some(cmi_clock) = registry
        .get_resolved(control.slots.cmi)?
        .stable_fetch_clock()
    else {
        return Ok(None);
    };
    let Some(mace_ust) = registry
        .get_resolved(control.slots.mace)?
        .synchronous_ust_projection()
    else {
        return Ok(None);
    };
    let Some(crime) = registry
        .get_resolved(control.slots.crime)?
        .synchronous_read_snapshot()
    else {
        return Ok(None);
    };
    let limit = context
        .next_event_time()
        .map_or(context.deadline(), |event| event.min(context.deadline()));
    let external_available_ticks = limit.get().saturating_sub(context.now().get());
    if external_available_ticks == 0 {
        return Ok(None);
    }
    let cpu_clock = control.cpu_clock.projection();
    let maximum_boundaries = (control.cpu_continuation_quantum.min(256)) as u64;
    let maximum_shared_bus_cycles = maximum_boundaries.saturating_mul(4);
    let maximum_code_bus_cycles = maximum_boundaries.saturating_mul(2);
    let memory_clock = registry
        .get_resolved(control.slots.memory)?
        .stable_fetch_clock();
    let isa_delay = registry
        .get_resolved(control.slots.isa)?
        .stable_fetch_delay()
        .map_or(0, SimDuration::get);
    let fast_horizon_ticks = cpu_clock
        .elapsed(maximum_boundaries)
        .and_then(|cpu| {
            sysad_clock
                .elapsed(maximum_shared_bus_cycles)
                .and_then(|bus| cpu.get().checked_add(bus.get()))
        })
        .and_then(|ticks| {
            cmi_clock
                .elapsed(maximum_shared_bus_cycles)
                .and_then(|cmi| ticks.checked_add(cmi.get()))
        })
        .and_then(|ticks| match memory_clock {
            Some(memory) => memory
                .elapsed(maximum_code_bus_cycles)
                .and_then(|memory| ticks.checked_add(memory.get())),
            None => Some(ticks),
        })
        .and_then(|ticks| {
            isa_delay
                .checked_mul(maximum_boundaries)
                .and_then(|isa| ticks.checked_add(isa))
        })
        .and_then(|ticks| ticks.checked_add(1))
        .unwrap_or(external_available_ticks);
    let available_ticks = external_available_ticks.min(fast_horizon_ticks);
    let fast_context = Ip32FastMemoryContext::new(
        context.now().get(),
        available_ticks,
        cpu_clock,
        sysad_clock,
        cmi_clock,
        mace_ust,
        fast_horizon_ticks <= external_available_ticks,
    );
    Ok(Some(Ip32FastMemoryRuntime {
        context: fast_context,
        crime,
    }))
}

pub(super) fn commit_fast_memory_runtime<S>(
    registry: &mut ComponentRegistry,
    context: &mut Ip32DispatchContext<'_, '_, S>,
    control: &mut MachineControl,
    runtime: &Ip32FastMemoryRuntime,
    code_window: Option<&Mips4CodeWindow>,
    code_fetches: u64,
) -> Result<(), Ip32MachineDispatchError>
where
    S: TraceSink,
{
    control.fast_transaction_attempts = control
        .fast_transaction_attempts
        .saturating_add(runtime.context.attempts());
    control.fast_transaction_hits = control
        .fast_transaction_hits
        .saturating_add(runtime.context.completed());
    control.fast_transaction_fallbacks = control.fast_transaction_fallbacks.saturating_add(
        runtime
            .context
            .attempts()
            .saturating_sub(runtime.context.completed()),
    );
    if runtime.context.code_fetch_active() != code_window.is_some() {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "fast-memory and stable-code timelines lost their binding".to_owned(),
        )));
    }
    let code_fetches_usize = usize::try_from(code_fetches).map_err(|_| {
        Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "stable-code fetch count did not fit the host ABI".to_owned(),
        ))
    })?;
    if code_window.is_some_and(|window| code_fetches_usize > window.fetch_count()) {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "stable-code execution exceeded its planned fetch count".to_owned(),
        )));
    }
    let code_source = code_window
        .map(Mips4CodeWindow::guard)
        .map(Ip32StableCodeSource::from_guard)
        .transpose()?;
    let completed = runtime.context.completed();
    let total_sysad = completed.checked_add(code_fetches).ok_or_else(|| {
        Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "combined SysAD transaction count overflowed".to_owned(),
        ))
    })?;
    if total_sysad == 0 {
        return Ok(());
    }
    if !registry
        .get_resolved(control.slots.sysad)?
        .stable_fetch_ready()
        || !registry
            .get_resolved(control.slots.crime)?
            .stable_cpu_fetch_ready()
    {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "a fast-memory batch lost idle IP32 bus ownership".to_owned(),
        )));
    }
    let cmi_completed = runtime.context.cmi_completed();
    let (prom_fetches, ram_fetches) = match code_source {
        Some(Ip32StableCodeSource::SystemFlash) => (code_fetches_usize, 0),
        Some(Ip32StableCodeSource::Sdram) => (0, code_fetches_usize),
        None => (0, 0),
    };
    let cmi_transactions = cmi_completed
        .checked_add(prom_fetches as u64)
        .ok_or_else(|| {
            Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "combined CMI transaction count overflowed".to_owned(),
            ))
        })?;
    let crime_transaction_ids = code_fetches_usize
        .checked_add(usize::try_from(cmi_completed).map_err(|_| {
            Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "fast-memory CMI count did not fit the host ABI".to_owned(),
            ))
        })?)
        .ok_or_else(|| {
            Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "combined CRIME transaction ID count overflowed".to_owned(),
            ))
        })?;
    if !registry
        .get_resolved(control.slots.crime)?
        .stable_cpu_fetches_ready(crime_transaction_ids)
    {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "a fast-memory batch could not reserve its CRIME transaction IDs".to_owned(),
        )));
    }
    if cmi_transactions != 0
        && (!registry
            .get_resolved(control.slots.cmi)?
            .stable_fetch_ready()
            || !registry
                .get_resolved(control.slots.mace)?
                .stable_prom_fetch_ready()
            || !registry
                .get_resolved(control.slots.mace)?
                .stable_prom_fetches_ready(prom_fetches))
    {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "a fast-memory CMI batch lost idle link ownership".to_owned(),
        )));
    }
    if ram_fetches != 0
        && !registry
            .get_resolved(control.slots.memory)?
            .stable_fetch_ready()
    {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "a fast-memory batch lost idle SDRAM bus ownership".to_owned(),
        )));
    }
    if prom_fetches != 0
        && registry
            .get_resolved(control.slots.isa)?
            .stable_fetch_delay()
            .is_none()
    {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "a fast-memory batch lost idle ISA ownership".to_owned(),
        )));
    }
    let elapsed = SimDuration::new(runtime.elapsed_ticks(code_fetches).ok_or_else(|| {
        Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "a combined stable-code and fast-memory timeline overflowed".to_owned(),
        ))
    })?);
    let next_time = context
        .now()
        .checked_add(elapsed)
        .ok_or(SchedulerError::TimeOverflow {
            time: context.now(),
            duration: elapsed,
        })?;
    if !context.try_advance_to(next_time)? {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "a proven fast-memory batch crossed an event boundary".to_owned(),
        )));
    }
    registry
        .get_resolved_mut(control.slots.sysad)?
        .commit_direct_transactions(total_sysad);
    if cmi_transactions != 0 {
        registry
            .get_resolved_mut(control.slots.cmi)?
            .commit_stable_fetches(usize::try_from(cmi_transactions).map_err(|_| {
                Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                    "combined CMI transaction count did not fit the host ABI".to_owned(),
                ))
            })?);
    }
    if ram_fetches != 0 {
        registry
            .get_resolved_mut(control.slots.memory)?
            .commit_stable_fetches(ram_fetches);
    }
    if code_fetches != 0 {
        let last_code_delivery = runtime
            .last_code_delivery_time(code_fetches, false)
            .ok_or_else(|| {
                Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                    "a stable-code batch lost its final SysAD delivery time".to_owned(),
                ))
            })?;
        if !registry
            .get_resolved_mut(control.slots.crime)?
            .account_stable_cpu_fetches(code_fetches_usize, last_code_delivery)?
        {
            return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "a stable-code batch lost CRIME ownership during commit".to_owned(),
            )));
        }
    }
    if completed != 0 {
        let last_delivery = runtime.last_delivery_time().ok_or_else(|| {
            Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "a fast-memory batch lost its final delivery time".to_owned(),
            ))
        })?;
        if !registry
            .get_resolved_mut(control.slots.crime)?
            .commit_synchronous_sysad_reads(completed, last_delivery)
        {
            return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "a fast-memory batch lost CRIME ownership during commit".to_owned(),
            )));
        }
    }
    if cmi_completed != 0
        && !registry
            .get_resolved_mut(control.slots.crime)?
            .commit_synchronous_cmi_reads(cmi_completed)?
    {
        return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
            "a fast-memory CMI batch lost CRIME ownership during commit".to_owned(),
        )));
    }
    if prom_fetches != 0 {
        let last_code_delivery = runtime
            .last_code_delivery_time(code_fetches, true)
            .ok_or_else(|| {
                Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                    "a stable PROM batch lost its final CMI delivery time".to_owned(),
                ))
            })?;
        if !registry
            .get_resolved_mut(control.slots.mace)?
            .account_stable_prom_fetches(prom_fetches, last_code_delivery)
        {
            return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "a stable PROM batch lost MACE ownership during commit".to_owned(),
            )));
        }
    }
    if cmi_completed != 0 {
        let last_cmi_delivery = runtime.last_cmi_delivery_time().ok_or_else(|| {
            Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "a fast-memory CMI batch lost its final delivery time".to_owned(),
            ))
        })?;
        if !registry
            .get_resolved_mut(control.slots.mace)?
            .commit_synchronous_ust_reads(cmi_completed, last_cmi_delivery)
        {
            return Err(Ip32MachineDispatchError::Cpu(R5000CpuError::Block(
                "a fast-memory CMI batch lost MACE ownership during commit".to_owned(),
            )));
        }
    }
    control.sysad_transactions = control.sysad_transactions.saturating_add(total_sysad);
    control.cmi_transactions = control.cmi_transactions.saturating_add(cmi_transactions);
    control.memory_transactions = control
        .memory_transactions
        .saturating_add(ram_fetches as u64);
    if let Some(source) = code_source {
        control.jit_code_fetches.record(source, code_fetches);
    }
    Ok(())
}
