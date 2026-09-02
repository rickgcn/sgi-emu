//! National Semiconductor DP8573A real-time clock.

use std::error::Error;
use std::fmt;

use se_core::bus::{BusFault, DeviceAddr};
use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

const REGISTER_COUNT: usize = 32;
const BANKED_REGISTER_COUNT: usize = 4;

const MAIN_STATUS: usize = 0x00;
const PERIODIC_FLAG: usize = 0x03;
const TIME_SAVE_CONTROL: usize = 0x04;
const HUNDREDTHS: usize = 0x05;
const SECONDS: usize = 0x06;
const MINUTES: usize = 0x07;
const HOURS: usize = 0x08;
const DAY_OF_MONTH: usize = 0x09;
const MONTH: usize = 0x0a;
const YEAR: usize = 0x0b;
const DAY_OF_WEEK: usize = 0x0e;
const SECONDS_COMPARE: usize = 0x13;
const DAY_OF_WEEK_COMPARE: usize = 0x18;
const SECONDS_SAVE: usize = 0x19;
const MONTH_SAVE: usize = 0x1d;
const PAGE_ONE_RAM: usize = 0x1e;

const REAL_TIME_MODE: usize = 0;
const OUTPUT_MODE: usize = 1;
const INTERRUPT_CONTROL_0: usize = 2;
const INTERRUPT_CONTROL_1: usize = 3;

const MAIN_RAM_AND_SELECT: u8 = 0xf0;
const REGISTER_SELECT: u8 = 1 << 6;
const ALARM_PENDING: u8 = 1 << 3;
const PERIODIC_PENDING: u8 = 1 << 2;

const TIME_SAVE_ENABLE: u8 = 1 << 7;
const HOUR_MODE_12: u8 = 1 << 2;
const CLOCK_START: u8 = 1 << 3;

const PERIODIC_MINUTE: u8 = 1 << 0;
const PERIODIC_TEN_SECONDS: u8 = 1 << 1;
const PERIODIC_SECOND: u8 = 1 << 2;
const PERIODIC_HUNDRED_MILLISECONDS: u8 = 1 << 3;
const PERIODIC_TEN_MILLISECONDS: u8 = 1 << 4;
const PERIODIC_MILLISECOND: u8 = 1 << 5;
const PERIODIC_EVENT_BITS: u8 = 0x3f;
const OSCILLATOR_FAILED: u8 = 1 << 6;
const TEST_ENABLE: u8 = 1 << 7;

const ALARM_COMPARE_BITS: u8 = 0x3f;
const ALARM_INTERRUPT_ENABLE: u8 = 1 << 6;

const ATTOSECONDS_PER_MILLISECOND: u128 = ATTOSECONDS_PER_SECOND / 1_000;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
const CALENDAR_CYCLE_DAYS: u64 = 700 * 365 + 175;

/// An invalid persistent DP8573A state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dp8573aStateError {
    /// The saved prescaler phase is outside one millisecond.
    InvalidPrescalerPhase {
        /// Invalid phase in attoseconds.
        phase_attoseconds: u64,
    },
    /// The saved millisecond position is outside one hundredth of a second.
    InvalidMillisecondPosition {
        /// Invalid position in milliseconds.
        millisecond: u8,
    },
}

impl fmt::Display for Dp8573aStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrescalerPhase { phase_attoseconds } => write!(
                formatter,
                "invalid DP8573A prescaler phase: {phase_attoseconds} attoseconds"
            ),
            Self::InvalidMillisecondPosition { millisecond } => write!(
                formatter,
                "invalid DP8573A millisecond position: {millisecond}"
            ),
        }
    }
}

impl Error for Dp8573aStateError {}

/// State retained while an emulated DP8573A continues on battery power.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dp8573aBatteryState {
    registers: [u8; REGISTER_COUNT],
    alternate_control_registers: [u8; BANKED_REGISTER_COUNT],
    prescaler_phase_attoseconds: u64,
    millisecond_within_hundredth: u8,
    oscillator_failed: bool,
    single_supply: bool,
    alarm_match_active: bool,
}

impl Dp8573aBatteryState {
    /// Creates a validated persistent RTC state.
    ///
    /// # Errors
    ///
    /// Returns [`Dp8573aStateError`] when an internal phase is outside its
    /// valid range.
    #[allow(
        clippy::too_many_arguments,
        reason = "the fields mirror one fixed RTC state"
    )]
    pub fn new(
        registers: [u8; REGISTER_COUNT],
        alternate_control_registers: [u8; BANKED_REGISTER_COUNT],
        prescaler_phase_attoseconds: u64,
        millisecond_within_hundredth: u8,
        oscillator_failed: bool,
        single_supply: bool,
        alarm_match_active: bool,
    ) -> Result<Self, Dp8573aStateError> {
        if u128::from(prescaler_phase_attoseconds) >= ATTOSECONDS_PER_MILLISECOND {
            return Err(Dp8573aStateError::InvalidPrescalerPhase {
                phase_attoseconds: prescaler_phase_attoseconds,
            });
        }
        if millisecond_within_hundredth >= 10 {
            return Err(Dp8573aStateError::InvalidMillisecondPosition {
                millisecond: millisecond_within_hundredth,
            });
        }
        Ok(Self {
            registers,
            alternate_control_registers,
            prescaler_phase_attoseconds,
            millisecond_within_hundredth,
            oscillator_failed,
            single_supply,
            alarm_match_active,
        })
    }

    /// Returns the main register storage.
    #[must_use]
    pub const fn registers(&self) -> &[u8; REGISTER_COUNT] {
        &self.registers
    }

    /// Returns the alternate control register storage.
    #[must_use]
    pub const fn alternate_control_registers(&self) -> &[u8; BANKED_REGISTER_COUNT] {
        &self.alternate_control_registers
    }

    /// Returns the sub-millisecond prescaler phase in attoseconds.
    #[must_use]
    pub const fn prescaler_phase_attoseconds(&self) -> u64 {
        self.prescaler_phase_attoseconds
    }

    /// Returns the millisecond position within the current hundredth.
    #[must_use]
    pub const fn millisecond_within_hundredth(&self) -> u8 {
        self.millisecond_within_hundredth
    }

    /// Reports whether the oscillator-failed flag is set.
    #[must_use]
    pub const fn oscillator_failed(&self) -> bool {
        self.oscillator_failed
    }

    /// Reports whether single-supply operation is selected.
    #[must_use]
    pub const fn single_supply(&self) -> bool {
        self.single_supply
    }

    /// Reports whether the alarm comparison is currently active.
    #[must_use]
    pub const fn alarm_match_active(&self) -> bool {
        self.alarm_match_active
    }
}

/// The software-visible state of a DP8573A under normal power.
pub struct Dp8573a {
    registers: [u8; REGISTER_COUNT],
    alternate_control_registers: [u8; BANKED_REGISTER_COUNT],
    prescaler_phase_attoseconds: u128,
    millisecond_within_hundredth: u8,
    oscillator_failed: bool,
    #[allow(
        dead_code,
        reason = "the selected supply mode has no further effect under normal power"
    )]
    single_supply: bool,
    alarm_match_active: bool,
}

impl Dp8573a {
    /// Creates an RTC in its deterministic power-on state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            registers: [0; REGISTER_COUNT],
            alternate_control_registers: [0; BANKED_REGISTER_COUNT],
            prescaler_phase_attoseconds: 0,
            millisecond_within_hundredth: 0,
            oscillator_failed: true,
            single_supply: false,
            alarm_match_active: false,
        }
    }

    /// Returns the RTC state retained across application sessions.
    #[must_use]
    pub fn battery_state(&self) -> Dp8573aBatteryState {
        Dp8573aBatteryState {
            registers: self.registers,
            alternate_control_registers: self.alternate_control_registers,
            prescaler_phase_attoseconds: u64::try_from(self.prescaler_phase_attoseconds)
                .expect("a valid DP8573A prescaler phase fits in u64"),
            millisecond_within_hundredth: self.millisecond_within_hundredth,
            oscillator_failed: self.oscillator_failed,
            single_supply: self.single_supply,
            alarm_match_active: self.alarm_match_active,
        }
    }

    /// Restores retained RTC state and advances a running clock by elapsed
    /// offline milliseconds.
    pub fn restore_battery_state(&mut self, state: Dp8573aBatteryState, offline_milliseconds: u64) {
        self.registers = state.registers;
        self.alternate_control_registers = state.alternate_control_registers;
        self.prescaler_phase_attoseconds = u128::from(state.prescaler_phase_attoseconds);
        self.millisecond_within_hundredth = state.millisecond_within_hundredth;
        self.oscillator_failed = state.oscillator_failed;
        self.single_supply = state.single_supply;
        self.alarm_match_active = state.alarm_match_active;
        self.advance_offline_milliseconds(offline_milliseconds);
    }

    /// Reads one RTC transaction and applies read-to-clear behavior.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] unless the transaction is a
    /// byte on the low byte lane or a complete aligned word.
    pub fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let register = decode_register(address, data.len())?;
        let value = self.read_register(register);
        write_transaction_value(value, data);
        if register == PERIODIC_FLAG && !self.alternate_bank_selected() {
            self.registers[PERIODIC_FLAG] &= !PERIODIC_EVENT_BITS;
        }
        Ok(())
    }

    /// Reads one RTC transaction without changing device state.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] unless the transaction is a
    /// byte on the low byte lane or a complete aligned word.
    pub fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        let register = decode_register(address, data.len())?;
        write_transaction_value(self.read_register(register), data);
        Ok(())
    }

    /// Writes one RTC transaction.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::UnsupportedAccess`] unless the transaction is a
    /// byte on the low byte lane or a complete aligned word.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        let register = decode_register(address, data.len())?;
        self.write_register(register, data[data.len() - 1]);
        Ok(())
    }

    /// Advances the running clock by guest virtual time.
    pub fn advance_time(&mut self, elapsed: VirtualDuration) {
        if self.alternate_control_registers[REAL_TIME_MODE] & CLOCK_START == 0 {
            return;
        }

        let elapsed_attoseconds = elapsed.as_attoseconds();
        let elapsed_milliseconds = elapsed_attoseconds / ATTOSECONDS_PER_MILLISECOND;
        let elapsed_remainder = elapsed_attoseconds % ATTOSECONDS_PER_MILLISECOND;
        let phase = self.prescaler_phase_attoseconds + elapsed_remainder;
        let milliseconds = elapsed_milliseconds + phase / ATTOSECONDS_PER_MILLISECOND;
        self.prescaler_phase_attoseconds = phase % ATTOSECONDS_PER_MILLISECOND;

        for _ in 0..milliseconds {
            self.advance_millisecond();
        }
    }

    /// Returns the virtual duration until the next millisecond event.
    #[must_use]
    pub fn time_until_event(&self) -> Option<VirtualDuration> {
        if self.alternate_control_registers[REAL_TIME_MODE] & CLOCK_START == 0 {
            return None;
        }

        Some(VirtualDuration::from_attoseconds(
            ATTOSECONDS_PER_MILLISECOND - self.prescaler_phase_attoseconds,
        ))
    }

    fn advance_offline_milliseconds(&mut self, milliseconds: u64) {
        if milliseconds == 0 || self.alternate_control_registers[REAL_TIME_MODE] & CLOCK_START == 0
        {
            return;
        }

        let Some(clock) = ClockState::decode(self) else {
            return;
        };
        let total_milliseconds =
            u128::from(self.millisecond_within_hundredth) + u128::from(milliseconds);
        let hundredth_steps = total_milliseconds / 10;
        self.millisecond_within_hundredth = (total_milliseconds % 10) as u8;

        let mut events = PERIODIC_MILLISECOND;
        if hundredth_steps == 0 {
            self.raise_periodic_events(events);
            return;
        }
        events |= PERIODIC_TEN_MILLISECONDS;
        if crosses_cyclic_target(u128::from(clock.hundredths), hundredth_steps, 10) {
            events |= PERIODIC_HUNDRED_MILLISECONDS;
        }

        let total_hundredths = u128::from(clock.hundredths) + hundredth_steps;
        let second_steps = u64::try_from(total_hundredths / 100)
            .expect("offline milliseconds produce a u64 number of seconds");
        self.set_bcd_counter(HUNDREDTHS, 0xff, (total_hundredths % 100) as u8);
        if second_steps != 0 {
            events |= PERIODIC_SECOND;
            if crosses_cyclic_target(u128::from(clock.seconds), u128::from(second_steps), 10) {
                events |= PERIODIC_TEN_SECONDS;
            }
            if crosses_cyclic_target(u128::from(clock.seconds), u128::from(second_steps), 60) {
                events |= PERIODIC_MINUTE;
            }

            let alarm_edge = self.alarm_edge_during(&clock, second_steps);
            self.advance_clock_seconds(&clock, second_steps);
            if self.time_save_enabled() {
                self.copy_all_time_save_registers();
            }
            let alarm_matches = self.current_alarm_matches();
            if alarm_edge {
                self.registers[MAIN_STATUS] |= ALARM_PENDING;
            }
            self.alarm_match_active = alarm_matches;
        }
        self.raise_periodic_events(events);
    }

    /// Reports whether the logical interrupt output is asserted.
    #[must_use]
    pub fn interrupt_asserted(&self) -> bool {
        self.registers[MAIN_STATUS] & PERIODIC_PENDING != 0
            || self.registers[MAIN_STATUS] & ALARM_PENDING != 0
                && self.alternate_control_registers[INTERRUPT_CONTROL_1] & ALARM_INTERRUPT_ENABLE
                    != 0
    }

    fn alternate_bank_selected(&self) -> bool {
        self.registers[MAIN_STATUS] & REGISTER_SELECT != 0
    }

    fn read_register(&self, register: usize) -> u8 {
        match register {
            MAIN_STATUS => {
                (self.registers[MAIN_STATUS] & 0xfc) | u8::from(self.interrupt_asserted())
            }
            0x01 | 0x02 if !self.alternate_bank_selected() => 0,
            0x01..=0x04 if self.alternate_bank_selected() => {
                self.alternate_control_registers[register - 1]
            }
            PERIODIC_FLAG => {
                (self.registers[PERIODIC_FLAG] & !OSCILLATOR_FAILED)
                    | if self.oscillator_failed {
                        OSCILLATOR_FAILED
                    } else {
                        0
                    }
            }
            0x0f..=0x12 => 0,
            PAGE_ONE_RAM if !self.alternate_bank_selected() => 0,
            _ => self.registers[register],
        }
    }

    fn write_register(&mut self, register: usize, value: u8) {
        match register {
            MAIN_STATUS => self.write_main_status(value),
            0x01..=0x04 if self.alternate_bank_selected() => {
                self.write_alternate_control(register - 1, value);
            }
            0x01 | 0x02 => {}
            PERIODIC_FLAG => {
                self.single_supply = value & OSCILLATOR_FAILED != 0;
                self.registers[PERIODIC_FLAG] =
                    (self.registers[PERIODIC_FLAG] & PERIODIC_EVENT_BITS) | (value & TEST_ENABLE);
            }
            TIME_SAVE_CONTROL => self.write_time_save_control(value),
            HUNDREDTHS..=YEAR | DAY_OF_WEEK => self.write_counter(register, value),
            0x0d => self.registers[register] = value & 0x03,
            0x0f..=0x12 => {}
            SECONDS_COMPARE..=DAY_OF_WEEK_COMPARE => {
                self.registers[register] = value;
                self.recompute_alarm_match();
            }
            SECONDS_SAVE..=MONTH_SAVE => {
                self.registers[register] = value;
                if self.time_save_enabled() {
                    self.copy_time_save_register(register);
                }
            }
            PAGE_ONE_RAM if !self.alternate_bank_selected() => {}
            _ => self.registers[register] = value,
        }
    }

    fn write_main_status(&mut self, value: u8) {
        let pending = self.registers[MAIN_STATUS]
            & (ALARM_PENDING | PERIODIC_PENDING)
            & !(value & (ALARM_PENDING | PERIODIC_PENDING));
        self.registers[MAIN_STATUS] = (value & MAIN_RAM_AND_SELECT) | pending;
    }

    fn write_alternate_control(&mut self, register: usize, value: u8) {
        match register {
            REAL_TIME_MODE => {
                let was_running =
                    self.alternate_control_registers[REAL_TIME_MODE] & CLOCK_START != 0;
                let is_running = value & CLOCK_START != 0;
                self.alternate_control_registers[REAL_TIME_MODE] = value;
                if was_running && !is_running {
                    self.prescaler_phase_attoseconds = 0;
                    self.millisecond_within_hundredth = 0;
                }
                if is_running {
                    self.oscillator_failed = false;
                }
                if self.time_save_enabled() {
                    self.copy_all_time_save_registers();
                }
                self.recompute_alarm_match();
            }
            OUTPUT_MODE | INTERRUPT_CONTROL_0 => {
                self.alternate_control_registers[register] = value;
            }
            INTERRUPT_CONTROL_1 => {
                self.alternate_control_registers[register] = value;
                self.recompute_alarm_match();
            }
            _ => unreachable!(),
        }
    }

    fn write_time_save_control(&mut self, value: u8) {
        let was_enabled = self.time_save_enabled();
        self.registers[TIME_SAVE_CONTROL] = value;
        if was_enabled || self.time_save_enabled() {
            self.copy_all_time_save_registers();
        }
    }

    fn write_counter(&mut self, register: usize, value: u8) {
        self.registers[register] = value;
        if self.time_save_enabled() && (SECONDS..=MONTH).contains(&register) {
            self.copy_time_save(register - SECONDS + SECONDS_SAVE, register);
        }
        if (SECONDS..=MONTH).contains(&register) || register == DAY_OF_WEEK {
            self.recompute_alarm_match();
        }
    }

    fn advance_millisecond(&mut self) {
        let mut events = PERIODIC_MILLISECOND;
        self.millisecond_within_hundredth += 1;
        if self.millisecond_within_hundredth == 10 {
            self.millisecond_within_hundredth = 0;
            events |= PERIODIC_TEN_MILLISECONDS;
            events |= self.advance_hundredth();
        }
        self.raise_periodic_events(events);
    }

    fn advance_hundredth(&mut self) -> u8 {
        let Some((value, wrapped)) = self.increment_bcd_register(HUNDREDTHS, 0xff, 0, 99) else {
            return 0;
        };

        let mut events = 0;
        if value % 10 == 0 {
            events |= PERIODIC_HUNDRED_MILLISECONDS;
        }
        if !wrapped {
            return events;
        }

        events |= PERIODIC_SECOND;
        let Some((seconds, minute_carry)) = self.increment_bcd_register(SECONDS, 0x7f, 0, 59)
        else {
            self.finish_clock_change();
            return events;
        };
        if seconds % 10 == 0 {
            events |= PERIODIC_TEN_SECONDS;
        }
        if minute_carry {
            events |= PERIODIC_MINUTE;
            if self
                .increment_bcd_register(MINUTES, 0x7f, 0, 59)
                .is_some_and(|(_, carry)| carry)
            {
                self.advance_hour();
            }
        }
        self.finish_clock_change();
        events
    }

    fn finish_clock_change(&mut self) {
        if self.time_save_enabled() {
            self.copy_all_time_save_registers();
        }
        self.recompute_alarm_match();
    }

    fn advance_hour(&mut self) {
        if self.alternate_control_registers[REAL_TIME_MODE] & HOUR_MODE_12 != 0 {
            self.advance_12_hour_counter();
        } else if self
            .increment_bcd_register(HOURS, 0x3f, 0, 23)
            .is_some_and(|(_, carry)| carry)
        {
            self.advance_day();
        }
    }

    fn advance_12_hour_counter(&mut self) {
        let raw = self.registers[HOURS];
        let Some(hour) = decode_bcd(raw & 0x1f).filter(|hour| (1..=12).contains(hour)) else {
            return;
        };
        let was_pm = raw & 0x80 != 0;
        let (next_hour, next_pm, day_carry) = match hour {
            11 => (12, !was_pm, was_pm),
            12 => (1, was_pm, false),
            _ => (hour + 1, was_pm, false),
        };
        self.registers[HOURS] =
            (raw & !0x9f) | encode_bcd(next_hour) | if next_pm { 0x80 } else { 0 };
        if day_carry {
            self.advance_day();
        }
    }

    fn advance_day(&mut self) {
        let maximum_day = self.days_in_current_month();
        let Some((_, month_carry)) =
            self.increment_bcd_register(DAY_OF_MONTH, 0x3f, 1, maximum_day)
        else {
            return;
        };

        let _ = self.increment_bcd_register(DAY_OF_WEEK, 0x07, 1, 7);
        if !month_carry {
            return;
        }
        if !self
            .increment_bcd_register(MONTH, 0x1f, 1, 12)
            .is_some_and(|(_, carry)| carry)
        {
            return;
        }

        let _ = self.increment_bcd_register(YEAR, 0xff, 0, 99);
        let real_time_mode = &mut self.alternate_control_registers[REAL_TIME_MODE];
        let next_leap_year = ((*real_time_mode & 0x03) + 1) & 0x03;
        *real_time_mode = (*real_time_mode & !0x03) | next_leap_year;
    }

    fn days_in_current_month(&self) -> u8 {
        match decode_bcd(self.registers[MONTH] & 0x1f) {
            Some(2) if self.alternate_control_registers[REAL_TIME_MODE] & 0x03 == 0 => 29,
            Some(2) => 28,
            Some(4 | 6 | 9 | 11) => 30,
            _ => 31,
        }
    }

    fn increment_bcd_register(
        &mut self,
        register: usize,
        mask: u8,
        minimum: u8,
        maximum: u8,
    ) -> Option<(u8, bool)> {
        let raw = self.registers[register];
        let value =
            decode_bcd(raw & mask).filter(|value| *value >= minimum && *value <= maximum)?;
        let wrapped = value == maximum;
        let next = if wrapped { minimum } else { value + 1 };
        self.registers[register] = (raw & !mask) | (encode_bcd(next) & mask);
        Some((next, wrapped))
    }

    fn raise_periodic_events(&mut self, events: u8) {
        self.registers[PERIODIC_FLAG] |= events;
        if events & self.alternate_control_registers[INTERRUPT_CONTROL_0] != 0 {
            self.registers[MAIN_STATUS] |= PERIODIC_PENDING;
        }
    }

    fn time_save_enabled(&self) -> bool {
        self.registers[TIME_SAVE_CONTROL] & TIME_SAVE_ENABLE != 0
    }

    fn copy_all_time_save_registers(&mut self) {
        for source in SECONDS..=MONTH {
            self.copy_time_save(source - SECONDS + SECONDS_SAVE, source);
        }
    }

    fn copy_time_save_register(&mut self, destination: usize) {
        self.copy_time_save(destination, destination - SECONDS_SAVE + SECONDS);
    }

    fn copy_time_save(&mut self, destination: usize, source: usize) {
        let mask = self.counter_mask(source);
        self.registers[destination] =
            (self.registers[destination] & !mask) | (self.registers[source] & mask);
    }

    fn counter_mask(&self, register: usize) -> u8 {
        match register {
            SECONDS | MINUTES => 0x7f,
            HOURS if self.alternate_control_registers[REAL_TIME_MODE] & HOUR_MODE_12 != 0 => 0x9f,
            HOURS | DAY_OF_MONTH => 0x3f,
            MONTH => 0x1f,
            DAY_OF_WEEK => 0x07,
            _ => 0xff,
        }
    }

    fn set_bcd_counter(&mut self, register: usize, mask: u8, value: u8) {
        self.registers[register] = (self.registers[register] & !mask) | (encode_bcd(value) & mask);
    }

    fn advance_clock_seconds(&mut self, clock: &ClockState, seconds: u64) {
        let total_seconds = u128::from(clock.seconds_of_day()) + u128::from(seconds);
        let day_steps = u64::try_from(total_seconds / u128::from(SECONDS_PER_DAY))
            .expect("offline seconds produce a u64 number of days");
        let seconds_of_day = (total_seconds % u128::from(SECONDS_PER_DAY)) as u32;
        let hour = (seconds_of_day / 3_600) as u8;
        let minute = ((seconds_of_day / 60) % 60) as u8;
        let second = (seconds_of_day % 60) as u8;

        self.set_bcd_counter(SECONDS, 0x7f, second);
        self.set_bcd_counter(MINUTES, 0x7f, minute);
        if self.alternate_control_registers[REAL_TIME_MODE] & HOUR_MODE_12 != 0 {
            let (hour_12, afternoon) = match hour {
                0 => (12, false),
                1..=11 => (hour, false),
                12 => (12, true),
                _ => (hour - 12, true),
            };
            self.registers[HOURS] = (self.registers[HOURS] & !0x9f)
                | encode_bcd(hour_12)
                | if afternoon { 0x80 } else { 0 };
        } else {
            self.set_bcd_counter(HOURS, 0x3f, hour);
        }

        for _ in 0..day_steps % CALENDAR_CYCLE_DAYS {
            self.advance_day();
        }
    }

    fn alarm_edge_during(&self, clock: &ClockState, seconds: u64) -> bool {
        if seconds == 0
            || self.alternate_control_registers[INTERRUPT_CONTROL_1] & ALARM_COMPARE_BITS == 0
        {
            return false;
        }

        let first = if self.alarm_match_active {
            let first_change = self.seconds_until_selected_alarm_field_changes(clock);
            if seconds <= first_change {
                return false;
            }
            first_change + 1
        } else {
            1
        };
        self.alarm_matches_in_range(clock, first, seconds)
    }

    fn seconds_until_selected_alarm_field_changes(&self, clock: &ClockState) -> u64 {
        let enables = self.alternate_control_registers[INTERRUPT_CONTROL_1];
        if enables & (1 << 0) != 0 {
            1
        } else if enables & (1 << 1) != 0 {
            u64::from(60 - clock.seconds)
        } else if enables & (1 << 2) != 0 {
            u64::from(60 - clock.minutes) * 60 - u64::from(clock.seconds)
        } else if enables & ((1 << 3) | (1 << 5)) != 0 {
            SECONDS_PER_DAY - u64::from(clock.seconds_of_day())
        } else {
            let days = u64::from(
                days_in_month(clock.date.month, clock.date.leap_year) - clock.date.day_of_month + 1,
            );
            days * SECONDS_PER_DAY - u64::from(clock.seconds_of_day())
        }
    }

    fn alarm_matches_in_range(&self, clock: &ClockState, first: u64, last: u64) -> bool {
        let first_absolute = u64::from(clock.seconds_of_day()) + first;
        let last_absolute = u64::from(clock.seconds_of_day()) + last;
        let first_day = first_absolute / SECONDS_PER_DAY;
        let last_day = last_absolute / SECONDS_PER_DAY;
        let limited_last_day = last_day.min(first_day + CALENDAR_CYCLE_DAYS);
        let mut date = clock.date;
        for _ in 0..first_day % CALENDAR_CYCLE_DAYS {
            date.advance();
        }

        for day in first_day..=limited_last_day {
            let lower = if day == first_day {
                (first_absolute % SECONDS_PER_DAY) as u32
            } else {
                0
            };
            let upper = if day == last_day {
                (last_absolute % SECONDS_PER_DAY) as u32
            } else {
                (SECONDS_PER_DAY - 1) as u32
            };
            if self.alarm_date_matches(date) && self.alarm_time_matches_in_range(lower, upper) {
                return true;
            }
            date.advance();
        }
        false
    }

    fn alarm_date_matches(&self, date: CalendarDate) -> bool {
        let enables = self.alternate_control_registers[INTERRUPT_CONTROL_1];
        [
            (3, date.day_of_month, 0x3f),
            (4, date.month, 0x1f),
            (5, date.day_of_week, 0x07),
        ]
        .into_iter()
        .all(|(bit, value, mask)| {
            enables & (1 << bit) == 0
                || encode_bcd(value) & mask == self.registers[SECONDS_COMPARE + bit] & mask
        })
    }

    fn alarm_time_matches_in_range(&self, lower: u32, upper: u32) -> bool {
        if lower > upper || !self.alarm_time_fields_valid() {
            return false;
        }
        if lower == 0 && upper == (SECONDS_PER_DAY - 1) as u32 {
            return true;
        }
        (lower..=upper).any(|seconds_of_day| self.alarm_time_matches(seconds_of_day))
    }

    fn alarm_time_fields_valid(&self) -> bool {
        let enables = self.alternate_control_registers[INTERRUPT_CONTROL_1];
        let second =
            decode_bcd(self.registers[SECONDS_COMPARE] & 0x7f).is_some_and(|value| value <= 59);
        let minute =
            decode_bcd(self.registers[SECONDS_COMPARE + 1] & 0x7f).is_some_and(|value| value <= 59);
        let hour = decode_hour(
            self.registers[SECONDS_COMPARE + 2],
            self.alternate_control_registers[REAL_TIME_MODE] & HOUR_MODE_12 != 0,
        )
        .is_some();
        (enables & (1 << 0) == 0 || second)
            && (enables & (1 << 1) == 0 || minute)
            && (enables & (1 << 2) == 0 || hour)
    }

    fn alarm_time_matches(&self, seconds_of_day: u32) -> bool {
        let enables = self.alternate_control_registers[INTERRUPT_CONTROL_1];
        let hour = (seconds_of_day / 3_600) as u8;
        let minute = ((seconds_of_day / 60) % 60) as u8;
        let second = (seconds_of_day % 60) as u8;
        let hour_raw = encode_hour(
            hour,
            self.alternate_control_registers[REAL_TIME_MODE] & HOUR_MODE_12 != 0,
        );
        [
            (0, encode_bcd(second), 0x7f),
            (1, encode_bcd(minute), 0x7f),
            (2, hour_raw, self.counter_mask(HOURS)),
        ]
        .into_iter()
        .all(|(bit, value, mask)| {
            enables & (1 << bit) == 0
                || value & mask == self.registers[SECONDS_COMPARE + bit] & mask
        })
    }

    fn current_alarm_matches(&self) -> bool {
        let enables = self.alternate_control_registers[INTERRUPT_CONTROL_1] & ALARM_COMPARE_BITS;
        enables != 0
            && (0..6).all(|offset| {
                let enable = 1 << offset;
                if enables & enable == 0 {
                    return true;
                }
                let counter = if offset == 5 {
                    DAY_OF_WEEK
                } else {
                    SECONDS + offset
                };
                let compare = SECONDS_COMPARE + offset;
                let mask = self.counter_mask(counter);
                self.registers[counter] & mask == self.registers[compare] & mask
            })
    }

    fn recompute_alarm_match(&mut self) {
        let matches = self.current_alarm_matches();
        if matches && !self.alarm_match_active {
            self.registers[MAIN_STATUS] |= ALARM_PENDING;
        }
        self.alarm_match_active = matches;
    }
}

#[derive(Clone, Copy)]
struct ClockState {
    hundredths: u8,
    seconds: u8,
    minutes: u8,
    hour: u8,
    date: CalendarDate,
}

impl ClockState {
    fn decode(rtc: &Dp8573a) -> Option<Self> {
        let hundredths = decode_bcd(rtc.registers[HUNDREDTHS]).filter(|value| *value <= 99)?;
        let seconds = decode_bcd(rtc.registers[SECONDS] & 0x7f).filter(|value| *value <= 59)?;
        let minutes = decode_bcd(rtc.registers[MINUTES] & 0x7f).filter(|value| *value <= 59)?;
        let hour_mode_12 = rtc.alternate_control_registers[REAL_TIME_MODE] & HOUR_MODE_12 != 0;
        let hour = decode_hour(rtc.registers[HOURS], hour_mode_12)?;
        let month =
            decode_bcd(rtc.registers[MONTH] & 0x1f).filter(|value| (1..=12).contains(value))?;
        let year = decode_bcd(rtc.registers[YEAR])?;
        let leap_year = rtc.alternate_control_registers[REAL_TIME_MODE] & 0x03;
        let maximum_day = days_in_month(month, leap_year);
        let day_of_month = decode_bcd(rtc.registers[DAY_OF_MONTH] & 0x3f)
            .filter(|value| (1..=maximum_day).contains(value))?;
        let day_of_week = decode_bcd(rtc.registers[DAY_OF_WEEK] & 0x07)
            .filter(|value| (1..=7).contains(value))?;
        Some(Self {
            hundredths,
            seconds,
            minutes,
            hour,
            date: CalendarDate {
                day_of_month,
                month,
                year,
                day_of_week,
                leap_year,
            },
        })
    }

    fn seconds_of_day(self) -> u32 {
        u32::from(self.hour) * 3_600 + u32::from(self.minutes) * 60 + u32::from(self.seconds)
    }
}

#[derive(Clone, Copy)]
struct CalendarDate {
    day_of_month: u8,
    month: u8,
    year: u8,
    day_of_week: u8,
    leap_year: u8,
}

impl CalendarDate {
    fn advance(&mut self) {
        self.day_of_week = self.day_of_week % 7 + 1;
        let maximum_day = days_in_month(self.month, self.leap_year);
        if self.day_of_month < maximum_day {
            self.day_of_month += 1;
            return;
        }

        self.day_of_month = 1;
        if self.month < 12 {
            self.month += 1;
            return;
        }

        self.month = 1;
        self.year = (self.year + 1) % 100;
        self.leap_year = (self.leap_year + 1) & 0x03;
    }
}

const fn days_in_month(month: u8, leap_year: u8) -> u8 {
    match month {
        2 if leap_year == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn decode_hour(raw: u8, hour_mode_12: bool) -> Option<u8> {
    if !hour_mode_12 {
        return decode_bcd(raw & 0x3f).filter(|value| *value <= 23);
    }

    let hour = decode_bcd(raw & 0x1f).filter(|value| (1..=12).contains(value))?;
    let afternoon = raw & 0x80 != 0;
    Some(match (hour, afternoon) {
        (12, false) => 0,
        (12, true) => 12,
        (_, false) => hour,
        (_, true) => hour + 12,
    })
}

fn encode_hour(hour: u8, hour_mode_12: bool) -> u8 {
    if !hour_mode_12 {
        return encode_bcd(hour);
    }

    let (hour_12, afternoon) = match hour {
        0 => (12, false),
        1..=11 => (hour, false),
        12 => (12, true),
        _ => (hour - 12, true),
    };
    encode_bcd(hour_12) | if afternoon { 0x80 } else { 0 }
}

const fn crosses_cyclic_target(current: u128, steps: u128, period: u128) -> bool {
    steps != 0 && steps >= period - current % period
}

fn decode_bcd(value: u8) -> Option<u8> {
    let high = value >> 4;
    let low = value & 0x0f;
    (high <= 9 && low <= 9).then_some(high * 10 + low)
}

fn encode_bcd(value: u8) -> u8 {
    ((value / 10) << 4) | (value % 10)
}

fn write_transaction_value(value: u8, data: &mut [u8]) {
    match data.len() {
        1 => data[0] = value,
        4 => data.copy_from_slice(&u32::from(value).to_be_bytes()),
        _ => unreachable!(),
    }
}

fn decode_register(address: DeviceAddr, length: usize) -> Result<usize, BusFault> {
    let address = address.get();
    let register = match length {
        1 if address & 3 == 3 => address >> 2,
        4 if address & 3 == 0 => address >> 2,
        _ => return Err(BusFault::UnsupportedAccess),
    };
    usize::try_from(register)
        .ok()
        .filter(|register| *register < REGISTER_COUNT)
        .ok_or(BusFault::UnsupportedAccess)
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use super::{
        ALARM_PENDING, ATTOSECONDS_PER_MILLISECOND, CALENDAR_CYCLE_DAYS, CLOCK_START, Dp8573a,
        Dp8573aBatteryState, Dp8573aStateError, HOUR_MODE_12, PERIODIC_PENDING, TIME_SAVE_ENABLE,
    };

    const HOURS_COMPARE: usize = 0x15;
    const DAY_OF_MONTH_COMPARE: usize = 0x16;

    fn address(register: usize) -> DeviceAddr {
        DeviceAddr::new((register as u64) * 4 + 3)
    }

    fn read_register(rtc: &mut Dp8573a, register: usize) -> Result<u8, BusFault> {
        let mut value = [0xff];
        rtc.read(address(register), &mut value)?;
        Ok(value[0])
    }

    fn debug_read_register(rtc: &Dp8573a, register: usize) -> Result<u8, BusFault> {
        let mut value = [0xff];
        rtc.debug_read(address(register), &mut value)?;
        Ok(value[0])
    }

    fn write_register(rtc: &mut Dp8573a, register: usize, value: u8) {
        rtc.write(address(register), &[value]).unwrap();
    }

    fn select_alternate_bank(rtc: &mut Dp8573a) {
        write_register(rtc, 0x00, 0x40);
    }

    fn select_main_bank(rtc: &mut Dp8573a) {
        write_register(rtc, 0x00, 0x00);
    }

    fn advance_milliseconds(rtc: &mut Dp8573a, milliseconds: u128) {
        rtc.advance_time(VirtualDuration::from_attoseconds(
            ATTOSECONDS_PER_SECOND / 1_000 * milliseconds,
        ));
    }

    #[test]
    fn compare_ram_stores_independent_values() {
        let mut rtc = Dp8573a::new();

        write_register(&mut rtc, HOURS_COMPARE, 0xa5);
        write_register(&mut rtc, DAY_OF_MONTH_COMPARE, 0x5a);

        assert_eq!(read_register(&mut rtc, HOURS_COMPARE), Ok(0xa5));
        assert_eq!(read_register(&mut rtc, DAY_OF_MONTH_COMPARE), Ok(0x5a));
    }

    #[test]
    fn aligned_words_use_the_low_byte_lane() {
        let mut rtc = Dp8573a::new();

        rtc.write(DeviceAddr::new(0x64), &0x1234_56a5_u32.to_be_bytes())
            .unwrap();
        let mut word = [0xff; 4];
        rtc.read(DeviceAddr::new(0x64), &mut word).unwrap();

        assert_eq!(u32::from_be_bytes(word), 0xa5);
        assert_eq!(read_register(&mut rtc, 0x19), Ok(0xa5));
    }

    #[test]
    fn bank_selection_and_unimplemented_locations_follow_the_register_map() {
        let mut rtc = Dp8573a::new();

        write_register(&mut rtc, 0x01, 0x12);
        assert_eq!(read_register(&mut rtc, 0x01), Ok(0));
        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x01, 0x34);
        write_register(&mut rtc, 0x1e, 0x56);
        assert_eq!(read_register(&mut rtc, 0x01), Ok(0x34));
        assert_eq!(read_register(&mut rtc, 0x1e), Ok(0x56));
        select_main_bank(&mut rtc);
        assert_eq!(read_register(&mut rtc, 0x01), Ok(0));
        assert_eq!(read_register(&mut rtc, 0x1e), Ok(0));
        write_register(&mut rtc, 0x0f, 0xa5);
        assert_eq!(read_register(&mut rtc, 0x0f), Ok(0));
        write_register(&mut rtc, 0x0d, 0xff);
        assert_eq!(read_register(&mut rtc, 0x0d), Ok(0x03));
    }

    #[test]
    fn start_stop_and_prescaler_phase_are_software_visible() {
        let mut rtc = Dp8573a::new();

        assert_eq!(debug_read_register(&rtc, 0x03), Ok(0x40));
        advance_milliseconds(&mut rtc, 20);
        assert_eq!(debug_read_register(&rtc, 0x05), Ok(0));

        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x01, CLOCK_START);
        select_main_bank(&mut rtc);
        assert_eq!(debug_read_register(&rtc, 0x03), Ok(0));
        advance_milliseconds(&mut rtc, 9);
        assert_eq!(debug_read_register(&rtc, 0x05), Ok(0));

        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x01, 0);
        write_register(&mut rtc, 0x01, CLOCK_START);
        select_main_bank(&mut rtc);
        advance_milliseconds(&mut rtc, 1);
        assert_eq!(debug_read_register(&rtc, 0x05), Ok(0));
        advance_milliseconds(&mut rtc, 9);
        assert_eq!(debug_read_register(&rtc, 0x05), Ok(1));
    }

    #[test]
    fn calendar_rolls_over_in_24_hour_mode() {
        let mut rtc = Dp8573a::new();
        for (register, value) in [
            (0x05, 0x99),
            (0x06, 0x59),
            (0x07, 0x59),
            (0x08, 0x23),
            (0x09, 0x31),
            (0x0a, 0x12),
            (0x0b, 0x99),
            (0x0e, 0x07),
        ] {
            write_register(&mut rtc, register, value);
        }
        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x01, CLOCK_START);
        select_main_bank(&mut rtc);

        advance_milliseconds(&mut rtc, 10);

        for (register, expected) in [
            (0x05, 0x00),
            (0x06, 0x00),
            (0x07, 0x00),
            (0x08, 0x00),
            (0x09, 0x01),
            (0x0a, 0x01),
            (0x0b, 0x00),
            (0x0e, 0x01),
        ] {
            assert_eq!(debug_read_register(&rtc, register), Ok(expected));
        }
        select_alternate_bank(&mut rtc);
        assert_eq!(debug_read_register(&rtc, 0x01).unwrap() & 0x03, 1);
    }

    #[test]
    fn leap_year_and_12_hour_midnight_roll_over_correctly() {
        let mut rtc = Dp8573a::new();
        for (register, value) in [
            (0x05, 0x99),
            (0x06, 0x59),
            (0x07, 0x59),
            (0x08, 0x91),
            (0x09, 0x28),
            (0x0a, 0x02),
            (0x0b, 0x24),
            (0x0e, 0x03),
        ] {
            write_register(&mut rtc, register, value);
        }
        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x01, CLOCK_START | HOUR_MODE_12);
        select_main_bank(&mut rtc);

        advance_milliseconds(&mut rtc, 10);

        assert_eq!(debug_read_register(&rtc, 0x08), Ok(0x12));
        assert_eq!(debug_read_register(&rtc, 0x09), Ok(0x29));
        assert_eq!(debug_read_register(&rtc, 0x0e), Ok(0x04));
    }

    #[test]
    fn non_leap_february_rolls_over_to_march() {
        let mut rtc = Dp8573a::new();
        for (register, value) in [
            (0x05, 0x99),
            (0x06, 0x59),
            (0x07, 0x59),
            (0x08, 0x23),
            (0x09, 0x28),
            (0x0a, 0x02),
            (0x0b, 0x25),
            (0x0e, 0x04),
        ] {
            write_register(&mut rtc, register, value);
        }
        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x01, CLOCK_START | 0x01);
        select_main_bank(&mut rtc);

        advance_milliseconds(&mut rtc, 10);

        assert_eq!(debug_read_register(&rtc, 0x09), Ok(0x01));
        assert_eq!(debug_read_register(&rtc, 0x0a), Ok(0x03));
        assert_eq!(debug_read_register(&rtc, 0x0e), Ok(0x05));
    }

    #[test]
    fn time_save_follows_and_then_freezes_used_counter_bits() {
        let mut rtc = Dp8573a::new();
        for register in 0x19..=0x1d {
            write_register(&mut rtc, register, 0xff);
        }
        for (register, value) in [
            (0x06, 0x12),
            (0x07, 0x23),
            (0x08, 0x14),
            (0x09, 0x25),
            (0x0a, 0x06),
        ] {
            write_register(&mut rtc, register, value);
        }

        write_register(&mut rtc, 0x04, TIME_SAVE_ENABLE);
        assert_eq!(debug_read_register(&rtc, 0x19), Ok(0x92));
        assert_eq!(debug_read_register(&rtc, 0x1b), Ok(0xd4));
        write_register(&mut rtc, 0x06, 0x34);
        assert_eq!(debug_read_register(&rtc, 0x19), Ok(0xb4));
        write_register(&mut rtc, 0x04, 0);
        write_register(&mut rtc, 0x06, 0x45);
        assert_eq!(debug_read_register(&rtc, 0x19), Ok(0xb4));
    }

    #[test]
    fn periodic_flags_debug_reads_and_pending_bits_are_independent() {
        let mut rtc = Dp8573a::new();
        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x03, 0x20);
        write_register(&mut rtc, 0x01, CLOCK_START);
        select_main_bank(&mut rtc);

        advance_milliseconds(&mut rtc, 1);

        assert_eq!(debug_read_register(&rtc, 0x03), Ok(0x20));
        assert_eq!(debug_read_register(&rtc, 0x03), Ok(0x20));
        assert_eq!(read_register(&mut rtc, 0x03), Ok(0x20));
        assert_eq!(debug_read_register(&rtc, 0x03), Ok(0));
        assert_eq!(debug_read_register(&rtc, 0x00).unwrap() & 0x05, 0x05);
        write_register(&mut rtc, 0x00, PERIODIC_PENDING);
        assert_eq!(debug_read_register(&rtc, 0x00).unwrap() & 0x05, 0);
    }

    #[test]
    fn alarm_pending_uses_match_edges_and_external_enable_only_gates_output() {
        let mut rtc = Dp8573a::new();
        write_register(&mut rtc, 0x06, 0x00);
        write_register(&mut rtc, 0x13, 0x01);
        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x04, 0x41);
        write_register(&mut rtc, 0x01, CLOCK_START);
        select_main_bank(&mut rtc);

        advance_milliseconds(&mut rtc, 1_000);

        assert!(rtc.interrupt_asserted());
        assert_ne!(debug_read_register(&rtc, 0x00).unwrap() & ALARM_PENDING, 0);
        write_register(&mut rtc, 0x00, ALARM_PENDING | 0x40);
        assert!(!rtc.interrupt_asserted());
        write_register(&mut rtc, 0x06, 0x02);
        write_register(&mut rtc, 0x06, 0x01);
        assert!(rtc.interrupt_asserted());
        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x04, 0x01);
        assert!(!rtc.interrupt_asserted());
        select_main_bank(&mut rtc);
        assert_ne!(debug_read_register(&rtc, 0x00).unwrap() & ALARM_PENDING, 0);
    }

    #[test]
    fn battery_state_rejects_invalid_internal_phases() {
        assert!(matches!(
            Dp8573aBatteryState::new(
                [0; 32],
                [0; 4],
                ATTOSECONDS_PER_MILLISECOND as u64,
                0,
                false,
                false,
                false,
            ),
            Err(Dp8573aStateError::InvalidPrescalerPhase { .. })
        ));
        assert!(matches!(
            Dp8573aBatteryState::new([0; 32], [0; 4], 0, 10, false, false, false,),
            Err(Dp8573aStateError::InvalidMillisecondPosition { .. })
        ));
    }

    #[test]
    fn stopped_persistent_clock_does_not_advance_offline() {
        let mut rtc = Dp8573a::new();
        write_register(&mut rtc, 0x06, 0x12);
        let state = rtc.battery_state();

        let mut restored = Dp8573a::new();
        restored.restore_battery_state(state.clone(), 86_400_000);

        assert_eq!(restored.battery_state(), state);
    }

    #[test]
    fn bulk_offline_progression_matches_millisecond_progression() {
        let mut stepped = Dp8573a::new();
        for (register, value) in [
            (0x05, 0x42),
            (0x06, 0x58),
            (0x07, 0x59),
            (0x08, 0x23),
            (0x09, 0x28),
            (0x0a, 0x02),
            (0x0b, 0x24),
            (0x0e, 0x03),
            (0x13, 0x00),
            (0x04, TIME_SAVE_ENABLE),
        ] {
            write_register(&mut stepped, register, value);
        }
        select_alternate_bank(&mut stepped);
        write_register(&mut stepped, 0x03, 0x3f);
        write_register(&mut stepped, 0x04, 0x41);
        write_register(&mut stepped, 0x01, CLOCK_START);
        select_main_bank(&mut stepped);
        advance_milliseconds(&mut stepped, 7);
        let initial = stepped.battery_state();

        let mut bulk = Dp8573a::new();
        bulk.restore_battery_state(initial, 65_432);
        advance_milliseconds(&mut stepped, 65_432);

        assert_eq!(bulk.battery_state(), stepped.battery_state());
    }

    #[test]
    fn full_calendar_cycle_is_skipped_without_changing_the_date() {
        let mut rtc = Dp8573a::new();
        for (register, value) in [
            (0x05, 0x12),
            (0x06, 0x34),
            (0x07, 0x56),
            (0x08, 0x07),
            (0x09, 0x17),
            (0x0a, 0x08),
            (0x0b, 0x24),
            (0x0e, 0x05),
        ] {
            write_register(&mut rtc, register, value);
        }
        select_alternate_bank(&mut rtc);
        write_register(&mut rtc, 0x01, CLOCK_START);
        select_main_bank(&mut rtc);
        let initial = rtc.battery_state();

        rtc.restore_battery_state(initial, CALENDAR_CYCLE_DAYS * 24 * 60 * 60 * 1_000);

        for (register, expected) in [
            (0x05, 0x12),
            (0x06, 0x34),
            (0x07, 0x56),
            (0x08, 0x07),
            (0x09, 0x17),
            (0x0a, 0x08),
            (0x0b, 0x24),
            (0x0e, 0x05),
        ] {
            assert_eq!(debug_read_register(&rtc, register), Ok(expected));
        }
    }

    #[test]
    fn rejects_invalid_lanes_widths_and_registers_atomically() {
        let mut rtc = Dp8573a::new();
        write_register(&mut rtc, HOURS_COMPARE, 0xa5);

        assert_eq!(
            rtc.write(DeviceAddr::new(0x54), &[0]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            rtc.write(address(HOURS_COMPARE), &[1, 2]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            rtc.write(DeviceAddr::new(0x80), &0_u32.to_be_bytes()),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(read_register(&mut rtc, HOURS_COMPARE), Ok(0xa5));
    }
}
