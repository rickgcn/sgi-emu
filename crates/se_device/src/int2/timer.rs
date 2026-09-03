use se_core::bus::BusFault;
use se_core::time::VirtualDuration;

const ATTOSECONDS_PER_TICK: u128 = 1_000_000_000_000;
const COUNTER_ZERO_ENCODING: u32 = 1 << 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CounterId {
    Zero,
    One,
    Two,
}

impl CounterId {
    const fn index(self) -> usize {
        match self {
            Self::Zero => 0,
            Self::One => 1,
            Self::Two => 2,
        }
    }

    const fn interrupt_index(self) -> Option<usize> {
        match self {
            Self::Zero => Some(0),
            Self::One => Some(1),
            Self::Two => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct TimerOutputs {
    pub(super) counter_0: bool,
    pub(super) counter_1: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProgrammableTimer {
    counters: [Counter; 3],
    deferred_outputs: [bool; 2],
    oscillator_phase: u128,
}

impl ProgrammableTimer {
    pub(super) const fn new() -> Self {
        Self {
            counters: [Counter::new(), Counter::new(), Counter::new()],
            deferred_outputs: [false; 2],
            oscillator_phase: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new();
    }

    pub(super) fn write_control(&mut self, value: u8) -> Result<(), BusFault> {
        let command = ControlCommand::decode(value)?;
        match command {
            ControlCommand::Latch(counter) => self.counters[counter.index()].latch(),
            ControlCommand::Configure { counter, mode } => {
                self.counters[counter.index()].configure(mode);
                self.clear_deferred_output(counter);
                Ok(())
            }
            ControlCommand::Quiesce(counter) => {
                self.counters[counter.index()].quiesce();
                self.clear_deferred_output(counter);
                Ok(())
            }
        }
    }

    pub(super) fn write_counter(&mut self, counter: CounterId, value: u8) -> Result<(), BusFault> {
        self.counters[counter.index()].write(value)
    }

    pub(super) fn read_counter(&mut self, counter: CounterId) -> Result<u8, BusFault> {
        self.counters[counter.index()].read()
    }

    pub(super) fn debug_read_counter(&self, counter: CounterId) -> Result<u8, BusFault> {
        self.counters[counter.index()].peek()
    }

    pub(super) fn advance_time(&mut self, elapsed: VirtualDuration) -> TimerOutputs {
        let elapsed_attoseconds = elapsed.as_attoseconds();
        let elapsed_ticks = elapsed_attoseconds / ATTOSECONDS_PER_TICK;
        let partial_attoseconds = elapsed_attoseconds % ATTOSECONDS_PER_TICK;
        let combined_phase = self.oscillator_phase + partial_attoseconds;
        let ticks = elapsed_ticks + combined_phase / ATTOSECONDS_PER_TICK;
        self.oscillator_phase = combined_phase % ATTOSECONDS_PER_TICK;

        let counter_2_outputs = self.counters[CounterId::Two.index()].advance(ticks);
        let counter_0_output = self.counters[CounterId::Zero.index()]
            .advance_with_deferred_output(counter_2_outputs, &mut self.deferred_outputs[0]);
        let counter_1_output = self.counters[CounterId::One.index()]
            .advance_with_deferred_output(counter_2_outputs, &mut self.deferred_outputs[1]);

        TimerOutputs {
            counter_0: counter_0_output,
            counter_1: counter_1_output,
        }
    }

    pub(super) fn time_until_output(&self, counter: CounterId) -> Option<VirtualDuration> {
        if counter == CounterId::Two {
            return None;
        }

        let (counter_2_reload, counter_2_remaining) =
            self.counters[CounterId::Two.index()].rate_generator_state()?;
        let interrupt_index = counter
            .interrupt_index()
            .expect("counter two does not produce an INT2 timer interrupt");
        let ticks = if self.deferred_outputs[interrupt_index] {
            u128::from(counter_2_remaining)
        } else {
            let (_, downstream_remaining) =
                self.counters[counter.index()].rate_generator_state()?;
            u128::from(counter_2_remaining)
                + u128::from(downstream_remaining) * u128::from(counter_2_reload)
        };
        let attoseconds = ticks * ATTOSECONDS_PER_TICK - self.oscillator_phase;
        Some(VirtualDuration::from_attoseconds(attoseconds))
    }

    fn clear_deferred_output(&mut self, counter: CounterId) {
        if let Some(index) = counter.interrupt_index() {
            self.deferred_outputs[index] = false;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Counter {
    state: CounterState,
    latch: Option<LatchedCount>,
}

impl Counter {
    const fn new() -> Self {
        Self {
            state: CounterState::Unconfigured,
            latch: None,
        }
    }

    fn configure(&mut self, mode: CounterMode) {
        self.state = CounterState::AwaitingCount {
            mode,
            low_byte: None,
        };
        self.latch = None;
    }

    fn quiesce(&mut self) {
        self.state = CounterState::Quiescent;
        self.latch = None;
    }

    fn write(&mut self, value: u8) -> Result<(), BusFault> {
        let CounterState::AwaitingCount { mode, low_byte } = self.state else {
            return Err(BusFault::UnsupportedAccess);
        };

        let Some(low_byte) = low_byte else {
            self.state = CounterState::AwaitingCount {
                mode,
                low_byte: Some(value),
            };
            return Ok(());
        };

        let encoded = u16::from_le_bytes([low_byte, value]);
        let reload = if encoded == 0 {
            COUNTER_ZERO_ENCODING
        } else {
            u32::from(encoded)
        };
        if mode == CounterMode::Mode2 && reload == 1 {
            return Err(BusFault::UnsupportedAccess);
        }

        self.state = match mode {
            CounterMode::Mode1 => CounterState::WaitingForGate { reload },
            CounterMode::Mode2 => CounterState::RateGenerator {
                reload,
                remaining: reload,
            },
        };
        self.latch = None;
        Ok(())
    }

    fn latch(&mut self) -> Result<(), BusFault> {
        let value = match self.state {
            CounterState::WaitingForGate { reload } => reload,
            CounterState::RateGenerator { remaining, .. } => remaining,
            CounterState::Unconfigured
            | CounterState::AwaitingCount { .. }
            | CounterState::Quiescent => return Err(BusFault::UnsupportedAccess),
        };

        if self.latch.is_none() {
            self.latch = Some(LatchedCount {
                value: encode_counter(value),
                read_high: false,
            });
        }
        Ok(())
    }

    fn read(&mut self) -> Result<u8, BusFault> {
        let value = self.peek()?;
        let latch = self
            .latch
            .as_mut()
            .expect("a successful counter peek must have a latch");
        if latch.read_high {
            self.latch = None;
        } else {
            latch.read_high = true;
        }
        Ok(value)
    }

    fn peek(&self) -> Result<u8, BusFault> {
        let latch = self.latch.ok_or(BusFault::UnsupportedAccess)?;
        let bytes = latch.value.to_le_bytes();
        Ok(bytes[usize::from(latch.read_high)])
    }

    fn advance(&mut self, input_pulses: u128) -> u128 {
        let CounterState::RateGenerator { reload, remaining } = self.state else {
            return 0;
        };
        if input_pulses < u128::from(remaining) {
            self.state = CounterState::RateGenerator {
                reload,
                remaining: remaining
                    - u32::try_from(input_pulses)
                        .expect("input pulses below a counter value must fit in u32"),
            };
            return 0;
        }

        let reload_wide = u128::from(reload);
        let after_first_output = input_pulses - u128::from(remaining);
        let output_count = 1 + after_first_output / reload_wide;
        let position = after_first_output % reload_wide;
        let next_remaining = if position == 0 {
            reload
        } else {
            u32::try_from(reload_wide - position)
                .expect("counter remainder must fit in its reload value")
        };
        self.state = CounterState::RateGenerator {
            reload,
            remaining: next_remaining,
        };
        output_count
    }

    fn advance_with_deferred_output(&mut self, input_pulses: u128, output_due: &mut bool) -> bool {
        if input_pulses == 0 {
            return false;
        }

        let mut output = *output_due;
        *output_due = false;
        if let Some((reload, remaining)) = self.rate_generator_state()
            && input_pulses >= u128::from(remaining)
        {
            let after_first_terminal = input_pulses - u128::from(remaining);
            output |= after_first_terminal != 0;
            *output_due = after_first_terminal.is_multiple_of(u128::from(reload));
        }
        let _ = self.advance(input_pulses);
        output
    }

    const fn rate_generator_state(&self) -> Option<(u32, u32)> {
        match self.state {
            CounterState::RateGenerator { reload, remaining } => Some((reload, remaining)),
            CounterState::Unconfigured
            | CounterState::AwaitingCount { .. }
            | CounterState::WaitingForGate { .. }
            | CounterState::Quiescent => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CounterState {
    Unconfigured,
    AwaitingCount {
        mode: CounterMode,
        low_byte: Option<u8>,
    },
    WaitingForGate {
        reload: u32,
    },
    RateGenerator {
        reload: u32,
        remaining: u32,
    },
    Quiescent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CounterMode {
    Mode1,
    Mode2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LatchedCount {
    value: u16,
    read_high: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlCommand {
    Latch(CounterId),
    Configure {
        counter: CounterId,
        mode: CounterMode,
    },
    Quiesce(CounterId),
}

impl ControlCommand {
    fn decode(value: u8) -> Result<Self, BusFault> {
        let counter = match value >> 6 {
            0 => CounterId::Zero,
            1 => CounterId::One,
            2 => CounterId::Two,
            _ => return Err(BusFault::UnsupportedAccess),
        };
        let read_write = (value >> 4) & 3;
        if read_write == 0 {
            return Ok(Self::Latch(counter));
        }
        if read_write != 3 || value & 1 != 0 {
            return Err(BusFault::UnsupportedAccess);
        }

        match (value >> 1) & 7 {
            1 => Ok(Self::Configure {
                counter,
                mode: CounterMode::Mode1,
            }),
            2 => Ok(Self::Configure {
                counter,
                mode: CounterMode::Mode2,
            }),
            4 => Ok(Self::Quiesce(counter)),
            _ => Err(BusFault::UnsupportedAccess),
        }
    }
}

fn encode_counter(value: u32) -> u16 {
    if value == COUNTER_ZERO_ENCODING {
        0
    } else {
        u16::try_from(value).expect("counter values below 65536 must fit in u16")
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::BusFault;
    use se_core::time::VirtualDuration;

    use super::{ATTOSECONDS_PER_TICK, CounterId, ProgrammableTimer, TimerOutputs};

    fn configure(timer: &mut ProgrammableTimer, control: u8, counter: CounterId, reload: u16) {
        timer.write_control(control).unwrap();
        let [low, high] = reload.to_le_bytes();
        timer.write_counter(counter, low).unwrap();
        timer.write_counter(counter, high).unwrap();
    }

    fn latch(timer: &mut ProgrammableTimer, control: u8, counter: CounterId) -> u16 {
        timer.write_control(control).unwrap();
        let low = timer.read_counter(counter).unwrap();
        let high = timer.read_counter(counter).unwrap();
        u16::from_le_bytes([low, high])
    }

    #[test]
    fn control_decode_accepts_the_supported_subset() {
        let mut timer = ProgrammableTimer::new();

        for control in [0x32, 0x34, 0x38, 0x72, 0x74, 0x78, 0xb2, 0xb4, 0xb8] {
            assert_eq!(timer.write_control(control), Ok(()));
        }
        for control in [0xc0, 0x10, 0x20, 0x30, 0x31, 0x36, 0x3a, 0x3c, 0x3e] {
            assert_eq!(
                timer.write_control(control),
                Err(BusFault::UnsupportedAccess)
            );
        }
    }

    #[test]
    fn invalid_control_is_atomic() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0xb4, CounterId::Two, 0x1234);
        timer.advance_time(VirtualDuration::from_attoseconds(ATTOSECONDS_PER_TICK));
        let before = timer.clone();

        assert_eq!(timer.write_control(0xb6), Err(BusFault::UnsupportedAccess));
        assert_eq!(timer, before);
    }

    #[test]
    fn mode_2_starts_only_after_a_valid_high_byte() {
        let mut timer = ProgrammableTimer::new();
        timer.write_control(0xb4).unwrap();
        timer.write_counter(CounterId::Two, 1).unwrap();

        assert_eq!(timer.time_until_output(CounterId::One), None);
        assert_eq!(
            timer.write_counter(CounterId::Two, 0),
            Err(BusFault::UnsupportedAccess)
        );
        timer.write_counter(CounterId::Two, 1).unwrap();

        assert_eq!(latch(&mut timer, 0x8f, CounterId::Two), 0x0101);
    }

    #[test]
    fn zero_reload_is_65536_and_reads_back_as_zero() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0xb4, CounterId::Two, 0);

        assert_eq!(latch(&mut timer, 0x80, CounterId::Two), 0);
        timer.advance_time(VirtualDuration::from_attoseconds(ATTOSECONDS_PER_TICK));
        assert_eq!(latch(&mut timer, 0x80, CounterId::Two), u16::MAX);
    }

    #[test]
    fn latch_is_stable_and_repeated_latch_keeps_the_old_snapshot() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0xb4, CounterId::Two, 0x1234);
        timer.write_control(0x80).unwrap();
        timer.advance_time(VirtualDuration::from_attoseconds(4 * ATTOSECONDS_PER_TICK));
        timer.write_control(0x8f).unwrap();

        assert_eq!(timer.debug_read_counter(CounterId::Two), Ok(0x34));
        assert_eq!(timer.read_counter(CounterId::Two), Ok(0x34));
        assert_eq!(timer.debug_read_counter(CounterId::Two), Ok(0x12));
        assert_eq!(timer.read_counter(CounterId::Two), Ok(0x12));
        assert_eq!(
            timer.read_counter(CounterId::Two),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(latch(&mut timer, 0x80, CounterId::Two), 0x1230);
    }

    #[test]
    fn valid_control_discards_partial_writes_and_unread_latches() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0xb4, CounterId::Two, 0x1234);
        timer.write_control(0x80).unwrap();

        timer.write_control(0xb4).unwrap();
        assert_eq!(
            timer.debug_read_counter(CounterId::Two),
            Err(BusFault::UnsupportedAccess)
        );
        timer.write_counter(CounterId::Two, 0x34).unwrap();
        timer.write_control(0xb4).unwrap();
        timer.write_counter(CounterId::Two, 0x12).unwrap();
        timer.write_counter(CounterId::Two, 0).unwrap();

        assert_eq!(latch(&mut timer, 0x80, CounterId::Two), 0x0012);
    }

    #[test]
    fn latch_rejects_unconfigured_and_partial_counters() {
        let mut timer = ProgrammableTimer::new();

        assert_eq!(timer.write_control(0x80), Err(BusFault::UnsupportedAccess));
        timer.write_control(0xb4).unwrap();
        timer.write_counter(CounterId::Two, 0x34).unwrap();
        assert_eq!(timer.write_control(0x80), Err(BusFault::UnsupportedAccess));
    }

    #[test]
    fn mode_1_waits_for_gate_and_mode_4_quiesces() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0x72, CounterId::One, 0x4321);
        configure(&mut timer, 0xb4, CounterId::Two, 2);

        assert_eq!(latch(&mut timer, 0x40, CounterId::One), 0x4321);
        assert_eq!(
            timer.advance_time(VirtualDuration::from_attoseconds(10 * ATTOSECONDS_PER_TICK)),
            TimerOutputs::default()
        );

        timer.write_control(0x78).unwrap();
        assert_eq!(
            timer.write_counter(CounterId::One, 1),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(timer.write_control(0x40), Err(BusFault::UnsupportedAccess));
    }

    #[test]
    fn downstream_terminal_counts_emit_on_the_next_counter_2_output() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0xb4, CounterId::Two, 3);
        configure(&mut timer, 0x34, CounterId::Zero, 2);
        configure(&mut timer, 0x74, CounterId::One, 2);

        assert_eq!(
            timer.advance_time(VirtualDuration::from_attoseconds(5 * ATTOSECONDS_PER_TICK)),
            TimerOutputs::default()
        );
        assert_eq!(
            timer.advance_time(VirtualDuration::from_attoseconds(ATTOSECONDS_PER_TICK)),
            TimerOutputs::default()
        );
        assert_eq!(
            timer.time_until_output(CounterId::One),
            Some(VirtualDuration::from_attoseconds(3 * ATTOSECONDS_PER_TICK))
        );
        assert_eq!(
            timer.advance_time(VirtualDuration::from_attoseconds(3 * ATTOSECONDS_PER_TICK)),
            TimerOutputs {
                counter_0: true,
                counter_1: true,
            }
        );
    }

    #[test]
    fn oscillator_phase_runs_while_counter_2_is_stopped() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0x74, CounterId::One, 2);
        timer.advance_time(VirtualDuration::from_attoseconds(ATTOSECONDS_PER_TICK - 1));
        configure(&mut timer, 0xb4, CounterId::Two, 2);

        assert_eq!(
            timer.time_until_output(CounterId::One),
            Some(VirtualDuration::from_attoseconds(
                5 * ATTOSECONDS_PER_TICK + 1
            ))
        );
        assert_eq!(
            timer.advance_time(VirtualDuration::from_attoseconds(1)),
            TimerOutputs::default()
        );
    }

    #[test]
    fn batched_and_fragmented_advances_reach_the_same_state() {
        let mut batched = ProgrammableTimer::new();
        configure(&mut batched, 0xb4, CounterId::Two, 7);
        configure(&mut batched, 0x34, CounterId::Zero, 5);
        configure(&mut batched, 0x74, CounterId::One, 11);
        let mut fragmented = batched.clone();

        let batched_outputs = batched.advance_time(VirtualDuration::from_attoseconds(
            1_003 * ATTOSECONDS_PER_TICK,
        ));
        let mut fragmented_outputs = TimerOutputs::default();
        for ticks in [101_u128, 17, 503, 382] {
            let outputs = fragmented.advance_time(VirtualDuration::from_attoseconds(
                ticks * ATTOSECONDS_PER_TICK,
            ));
            fragmented_outputs.counter_0 |= outputs.counter_0;
            fragmented_outputs.counter_1 |= outputs.counter_1;
        }

        assert_eq!(fragmented, batched);
        assert_eq!(fragmented_outputs, batched_outputs);
    }

    #[test]
    fn ide_counter_1_sequence_emits_one_counter_2_period_after_terminal_count() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0xb4, CounterId::Two, 1_000);
        configure(&mut timer, 0x74, CounterId::One, 2_000);
        let two_seconds = 2_000_000 * ATTOSECONDS_PER_TICK;
        let counter_2_period = 1_000 * ATTOSECONDS_PER_TICK;
        let first_interrupt = two_seconds + counter_2_period;

        assert_eq!(
            timer.time_until_output(CounterId::One),
            Some(VirtualDuration::from_attoseconds(first_interrupt))
        );
        assert_eq!(
            timer.advance_time(VirtualDuration::from_attoseconds(two_seconds)),
            TimerOutputs::default()
        );
        assert_eq!(
            timer.advance_time(VirtualDuration::from_attoseconds(counter_2_period - 1)),
            TimerOutputs::default()
        );
        assert_eq!(
            timer.advance_time(VirtualDuration::from_attoseconds(1)),
            TimerOutputs {
                counter_0: false,
                counter_1: true,
            }
        );
    }

    #[test]
    fn stopped_paths_have_no_deadline_and_running_paths_choose_exact_boundaries() {
        let mut timer = ProgrammableTimer::new();
        configure(&mut timer, 0x74, CounterId::One, 2);
        assert_eq!(timer.time_until_output(CounterId::One), None);

        configure(&mut timer, 0xb4, CounterId::Two, 3);
        timer.advance_time(VirtualDuration::from_attoseconds(ATTOSECONDS_PER_TICK / 2));
        assert_eq!(
            timer.time_until_output(CounterId::One),
            Some(VirtualDuration::from_attoseconds(
                9 * ATTOSECONDS_PER_TICK - ATTOSECONDS_PER_TICK / 2
            ))
        );
    }

    #[test]
    fn configure_and_quiesce_clear_a_deferred_output() {
        for control in [0x74, 0x78] {
            let mut timer = ProgrammableTimer::new();
            configure(&mut timer, 0xb4, CounterId::Two, 3);
            configure(&mut timer, 0x74, CounterId::One, 2);
            assert_eq!(
                timer.advance_time(VirtualDuration::from_attoseconds(6 * ATTOSECONDS_PER_TICK)),
                TimerOutputs::default()
            );

            timer.write_control(control).unwrap();
            assert_eq!(
                timer.advance_time(VirtualDuration::from_attoseconds(3 * ATTOSECONDS_PER_TICK)),
                TimerOutputs::default()
            );
        }
    }
}
