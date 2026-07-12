//! Dallas DS1687 real-time clock and battery-backed NVRAM.

use core::fmt;
use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;
use se_core::scheduler::SimTime;

use crate::bus::irq::{IrqOutput, IrqSource, IrqTransaction};
use crate::bus::isa::{
    IsaBusError, IsaCompletion, IsaCompletionPayload, IsaDeviceResponse, IsaTransaction,
    IsaTransfer,
};

/// DS1687 interrupt output.
pub const DS1687_IRQ_OUTPUT: IrqOutput = IrqOutput::new(0);

const REGISTER_SECONDS: usize = 0x00;
const REGISTER_SECONDS_ALARM: usize = 0x01;
const REGISTER_MINUTES: usize = 0x02;
const REGISTER_MINUTES_ALARM: usize = 0x03;
const REGISTER_HOURS: usize = 0x04;
const REGISTER_HOURS_ALARM: usize = 0x05;
const REGISTER_WEEKDAY: usize = 0x06;
const REGISTER_DAY: usize = 0x07;
const REGISTER_MONTH: usize = 0x08;
const REGISTER_YEAR: usize = 0x09;
const REGISTER_A: usize = 0x0a;
const REGISTER_B: usize = 0x0b;
const REGISTER_C: usize = 0x0c;
const REGISTER_D: usize = 0x0d;
const REGISTER_CENTURY: usize = 0x48;

const REGISTER_B_SET: u8 = 1 << 7;
const REGISTER_B_PERIODIC_INTERRUPT: u8 = 1 << 6;
const REGISTER_B_ALARM_INTERRUPT: u8 = 1 << 5;
const REGISTER_B_UPDATE_INTERRUPT: u8 = 1 << 4;
const REGISTER_B_BINARY: u8 = 1 << 2;
const REGISTER_B_24_HOUR: u8 = 1 << 1;
const REGISTER_C_IRQ: u8 = 1 << 7;
const REGISTER_C_PERIODIC: u8 = 1 << 6;
const REGISTER_C_ALARM: u8 = 1 << 5;
const REGISTER_C_UPDATE: u8 = 1 << 4;

/// Complete deterministic DS1687 construction input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ds1687Config {
    /// Initial UTC time as Unix seconds.
    pub initial_unix_seconds: i64,
    /// Initial 256-byte register and NVRAM image.
    pub nvram: Vec<u8>,
}

impl Default for Ds1687Config {
    fn default() -> Self {
        Self {
            initial_unix_seconds: 946_684_800,
            nvram: vec![0; 256],
        }
    }
}

/// DS1687 construction error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ds1687Error {
    InvalidNvramSize { size: usize },
    InvalidTimebase,
}

impl fmt::Display for Ds1687Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNvramSize { size } => {
                write!(formatter, "invalid DS1687 NVRAM size: {size} bytes")
            }
            Self::InvalidTimebase => formatter.write_str("DS1687 timebase must be nonzero"),
        }
    }
}

impl std::error::Error for Ds1687Error {}

/// Observable action emitted by the RTC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ds1687Action {
    /// Drives the alarm/periodic/update IRQ line.
    SetIrq(IrqTransaction),
    /// No action is pending.
    Idle,
}

/// DS1687 RTC/NVRAM device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ds1687 {
    id: ComponentId,
    name: String,
    timebase_hz: u64,
    initial_unix_seconds: i64,
    base_time: SimTime,
    registers: [u8; 256],
    last_observed_second: i64,
    irq_asserted: bool,
    actions: VecDeque<Ds1687Action>,
}

impl Ds1687 {
    /// Creates an RTC with deterministic time and NVRAM contents.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        timebase_hz: u64,
        config: Ds1687Config,
    ) -> Result<Self, Ds1687Error> {
        if timebase_hz == 0 {
            return Err(Ds1687Error::InvalidTimebase);
        }
        let registers: [u8; 256] = config
            .nvram
            .try_into()
            .map_err(|value: Vec<u8>| Ds1687Error::InvalidNvramSize { size: value.len() })?;
        let mut rtc = Self {
            id,
            name: name.into(),
            timebase_hz,
            initial_unix_seconds: config.initial_unix_seconds,
            base_time: SimTime::ZERO,
            registers,
            last_observed_second: config.initial_unix_seconds,
            irq_asserted: false,
            actions: VecDeque::new(),
        };
        rtc.registers[REGISTER_A] &= 0x7f;
        rtc.registers[REGISTER_D] |= 0x80;
        Ok(rtc)
    }

    /// Starts the deterministic RTC epoch without altering NVRAM.
    pub fn power_on(&mut self, now: SimTime) {
        self.base_time = now;
        self.last_observed_second = self.initial_unix_seconds;
        self.registers[REGISTER_C] = 0;
        self.irq_asserted = false;
        self.actions.clear();
    }

    /// Preserves time and NVRAM while resetting interrupt control state.
    pub fn hard_reset(&mut self, now: SimTime) {
        let current = self.unix_seconds(now);
        self.initial_unix_seconds = current;
        self.base_time = now;
        self.last_observed_second = current;
        self.registers[REGISTER_A] &= 0x7f;
        self.registers[REGISTER_B] = 0;
        self.registers[REGISTER_C] = 0;
        self.set_irq(false);
    }

    /// Advances lazy time-derived flags to the supplied time.
    pub fn observe_time(&mut self, now: SimTime) {
        let second = self.unix_seconds(now);
        if second != self.last_observed_second && self.registers[REGISTER_B] & REGISTER_B_SET == 0 {
            self.last_observed_second = second;
            self.registers[REGISTER_C] |= REGISTER_C_UPDATE;
            if self.alarm_matches(second) {
                self.registers[REGISTER_C] |= REGISTER_C_ALARM;
            }
            self.update_irq();
        }
    }

    /// Polls one pending action.
    pub fn poll(&mut self) -> Ds1687Action {
        self.actions.pop_front().unwrap_or(Ds1687Action::Idle)
    }

    /// Returns the complete battery-backed image.
    pub fn nvram_snapshot(&self) -> &[u8; 256] {
        &self.registers
    }

    fn read_register(&mut self, address: usize, now: SimTime) -> Result<u8, IsaBusError> {
        if address >= self.registers.len() {
            return Err(IsaBusError::Address);
        }
        self.observe_time(now);
        let binary = self.registers[REGISTER_B] & REGISTER_B_BINARY != 0;
        let twenty_four = self.registers[REGISTER_B] & REGISTER_B_24_HOUR != 0;
        let fields = DateTimeFields::from_unix_seconds(self.unix_seconds(now));
        let value = match address {
            REGISTER_SECONDS => encode(fields.second, binary),
            REGISTER_MINUTES => encode(fields.minute, binary),
            REGISTER_HOURS => encode_hour(fields.hour, binary, twenty_four),
            REGISTER_WEEKDAY => encode(fields.weekday, binary),
            REGISTER_DAY => encode(fields.day, binary),
            REGISTER_MONTH => encode(fields.month, binary),
            REGISTER_YEAR => encode((fields.year.rem_euclid(100)) as u8, binary),
            REGISTER_CENTURY => encode((fields.year.div_euclid(100)) as u8, binary),
            REGISTER_A => self.registers[address],
            REGISTER_C => {
                let value = self.registers[address];
                self.registers[address] = 0;
                self.update_irq();
                value
            }
            REGISTER_D => self.registers[address] | 0x80,
            _ => self.registers[address],
        };
        Ok(value)
    }

    fn write_register(
        &mut self,
        address: usize,
        value: u8,
        now: SimTime,
    ) -> Result<(), IsaBusError> {
        if address >= self.registers.len() {
            return Err(IsaBusError::Address);
        }
        self.observe_time(now);
        match address {
            REGISTER_C | REGISTER_D => {}
            REGISTER_A => self.registers[address] = value & 0x7f,
            REGISTER_B => {
                self.registers[address] = value;
                self.update_irq();
            }
            REGISTER_SECONDS | REGISTER_MINUTES | REGISTER_HOURS | REGISTER_WEEKDAY
            | REGISTER_DAY | REGISTER_MONTH | REGISTER_YEAR | REGISTER_CENTURY => {
                self.registers[address] = value;
                if self.registers[REGISTER_B] & REGISTER_B_SET == 0 {
                    self.rebase_from_clock_registers(now);
                }
            }
            _ => self.registers[address] = value,
        }
        Ok(())
    }

    fn unix_seconds(&self, now: SimTime) -> i64 {
        let elapsed_ticks = now.get().saturating_sub(self.base_time.get());
        self.initial_unix_seconds
            .saturating_add((elapsed_ticks / self.timebase_hz) as i64)
    }

    fn rebase_from_clock_registers(&mut self, now: SimTime) {
        let binary = self.registers[REGISTER_B] & REGISTER_B_BINARY != 0;
        let twenty_four = self.registers[REGISTER_B] & REGISTER_B_24_HOUR != 0;
        let current = DateTimeFields::from_unix_seconds(self.unix_seconds(now));
        let fields = DateTimeFields {
            year: i32::from(decode(self.registers[REGISTER_CENTURY], binary)) * 100
                + i32::from(decode(self.registers[REGISTER_YEAR], binary)),
            month: decode(self.registers[REGISTER_MONTH], binary).clamp(1, 12),
            day: decode(self.registers[REGISTER_DAY], binary).clamp(1, 31),
            weekday: current.weekday,
            hour: decode_hour(self.registers[REGISTER_HOURS], binary, twenty_four),
            minute: decode(self.registers[REGISTER_MINUTES], binary).min(59),
            second: decode(self.registers[REGISTER_SECONDS], binary).min(59),
        };
        self.initial_unix_seconds = fields.to_unix_seconds();
        self.base_time = now;
        self.last_observed_second = self.initial_unix_seconds;
    }

    fn alarm_matches(&self, second: i64) -> bool {
        let fields = DateTimeFields::from_unix_seconds(second);
        let binary = self.registers[REGISTER_B] & REGISTER_B_BINARY != 0;
        let twenty_four = self.registers[REGISTER_B] & REGISTER_B_24_HOUR != 0;
        alarm_field_matches(
            self.registers[REGISTER_SECONDS_ALARM],
            fields.second,
            binary,
        ) && alarm_field_matches(
            self.registers[REGISTER_MINUTES_ALARM],
            fields.minute,
            binary,
        ) && (self.registers[REGISTER_HOURS_ALARM] & 0xc0 == 0xc0
            || decode_hour(self.registers[REGISTER_HOURS_ALARM], binary, twenty_four)
                == fields.hour)
    }

    fn update_irq(&mut self) {
        let control = self.registers[REGISTER_B];
        let flags = self.registers[REGISTER_C];
        let asserted = flags & REGISTER_C_PERIODIC != 0
            && control & REGISTER_B_PERIODIC_INTERRUPT != 0
            || flags & REGISTER_C_ALARM != 0 && control & REGISTER_B_ALARM_INTERRUPT != 0
            || flags & REGISTER_C_UPDATE != 0 && control & REGISTER_B_UPDATE_INTERRUPT != 0;
        if asserted {
            self.registers[REGISTER_C] |= REGISTER_C_IRQ;
        } else {
            self.registers[REGISTER_C] &= !REGISTER_C_IRQ;
        }
        self.set_irq(asserted);
    }

    fn set_irq(&mut self, asserted: bool) {
        if self.irq_asserted == asserted {
            return;
        }
        self.irq_asserted = asserted;
        self.actions.push_back(Ds1687Action::SetIrq(IrqTransaction {
            source: IrqSource {
                component: self.id,
                output: DS1687_IRQ_OUTPUT,
            },
            asserted,
        }));
    }
}

impl Component for Ds1687 {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {
        self.hard_reset(SimTime::ZERO);
    }
}

impl BusDeviceRole<IsaTransaction> for Ds1687 {
    type Response = IsaDeviceResponse;

    fn accept(&mut self, transaction: IsaTransaction) -> Self::Response {
        let result = match transaction.transfer {
            IsaTransfer::Read { length: 1 } => self
                .read_register(transaction.address as usize, transaction.time)
                .map(|value| IsaCompletionPayload::ReadData(vec![value])),
            IsaTransfer::Write { data, byte_enable }
                if data.len() == 1 && byte_enable.as_slice() == [true] =>
            {
                self.write_register(transaction.address as usize, data[0], transaction.time)
                    .map(|()| IsaCompletionPayload::WriteComplete)
            }
            _ => Err(IsaBusError::Access),
        };
        IsaDeviceResponse::Complete(IsaCompletion {
            id: transaction.id,
            result,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DateTimeFields {
    year: i32,
    month: u8,
    day: u8,
    weekday: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

impl DateTimeFields {
    fn from_unix_seconds(seconds: i64) -> Self {
        let days = seconds.div_euclid(86_400);
        let day_seconds = seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            weekday: (days + 4).rem_euclid(7) as u8 + 1,
            hour: (day_seconds / 3_600) as u8,
            minute: (day_seconds % 3_600 / 60) as u8,
            second: (day_seconds % 60) as u8,
        }
    }

    fn to_unix_seconds(self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 86_400
            + i64::from(self.hour) * 3_600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
    }
}

fn encode(value: u8, binary: bool) -> u8 {
    if binary {
        value
    } else {
        ((value / 10) << 4) | (value % 10)
    }
}

fn decode(value: u8, binary: bool) -> u8 {
    if binary {
        value
    } else {
        (value >> 4) * 10 + (value & 0x0f)
    }
}

fn encode_hour(hour: u8, binary: bool, twenty_four: bool) -> u8 {
    if twenty_four {
        return encode(hour, binary);
    }
    let pm = hour >= 12;
    let hour = match hour % 12 {
        0 => 12,
        value => value,
    };
    encode(hour, binary) | if pm { 0x80 } else { 0 }
}

fn decode_hour(value: u8, binary: bool, twenty_four: bool) -> u8 {
    if twenty_four {
        return decode(value & 0x3f, binary).min(23);
    }
    let hour = decode(value & 0x7f, binary).clamp(1, 12) % 12;
    hour + if value & 0x80 != 0 { 12 } else { 0 }
}

fn alarm_field_matches(alarm: u8, value: u8, binary: bool) -> bool {
    alarm & 0xc0 == 0xc0 || decode(alarm & 0x7f, binary) == value
}

fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i32::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i32::from(day) - 1;
    let day_of_era = yoe * 365 + yoe / 4 - yoe / 100 + day_of_year;
    i64::from(era * 146_097 + day_of_era - 719_468)
}

fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_epoch_is_2000_01_01() {
        let rtc = Ds1687::new(
            ComponentId::new(1),
            "RTC",
            1_000_000_000,
            Ds1687Config::default(),
        )
        .unwrap();
        assert_eq!(
            DateTimeFields::from_unix_seconds(rtc.unix_seconds(SimTime::ZERO)),
            DateTimeFields {
                year: 2000,
                month: 1,
                day: 1,
                weekday: 7,
                hour: 0,
                minute: 0,
                second: 0
            }
        );
    }

    #[test]
    fn reads_bcd_time_and_preserves_nvram() {
        let mut rtc = Ds1687::new(
            ComponentId::new(1),
            "RTC",
            1_000_000_000,
            Ds1687Config::default(),
        )
        .unwrap();
        let read = |rtc: &mut Ds1687, address| match rtc.accept(IsaTransaction {
            id: crate::bus::isa::IsaTransactionId::new(address as u128),
            time: SimTime::ZERO,
            controller: ComponentId::new(2),
            target: ComponentId::new(1),
            address,
            transfer: IsaTransfer::Read { length: 1 },
        }) {
            IsaDeviceResponse::Complete(IsaCompletion {
                result: Ok(IsaCompletionPayload::ReadData(data)),
                ..
            }) => data[0],
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(read(&mut rtc, REGISTER_YEAR as u32), 0x00);
        assert_eq!(read(&mut rtc, REGISTER_CENTURY as u32), 0x20);
        rtc.registers[0x37] = 0x5a;
        rtc.hard_reset(SimTime::new(1_000_000_000));
        assert_eq!(rtc.registers[0x37], 0x5a);
    }

    #[test]
    fn calendar_conversion_round_trips() {
        for seconds in [0, 946_684_800, 1_704_067_199, -1] {
            let fields = DateTimeFields::from_unix_seconds(seconds);
            assert_eq!(fields.to_unix_seconds(), seconds);
        }
    }
}
