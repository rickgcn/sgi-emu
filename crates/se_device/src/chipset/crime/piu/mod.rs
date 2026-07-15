//! CRIME 1.1 processor-interface registers, interrupts, and timers.

use se_core::scheduler::{SimDuration, SimTime};

use super::registers;

pub(super) const CRIME_MASTER_FREQUENCY_HZ: u64 = 66_666_500;
const WATCHDOG_THRESHOLD_CRIME_CYCLES: u64 = (1 << 19) * 64;

/// Side effect produced by a processor-interface register transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PiuEffect {
    /// The combined enabled interrupt output changed.
    InterruptOutput(bool),

    /// Arms one watchdog stage.
    ArmWatchdog {
        /// Watchdog epoch.
        epoch: u64,

        /// One for warm reset and two for hard reset.
        stage: u8,

        /// Simulated delay to the threshold.
        delay: SimDuration,
    },

    /// Requests an R5000 warm reset.
    WarmReset,

    /// Requests a board hard reset.
    HardReset,
}

/// Result of a processor-interface register write.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PiuWriteResult {
    /// Whether the address is a defined processor-interface register.
    pub handled: bool,

    /// Ordered hardware effects.
    pub effects: Vec<PiuEffect>,
}

impl PiuWriteResult {
    const fn unhandled() -> Self {
        Self {
            handled: false,
            effects: Vec::new(),
        }
    }

    const fn handled() -> Self {
        Self {
            handled: true,
            effects: Vec::new(),
        }
    }
}

/// CRIME 1.1 processor-interface state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimePiu {
    control: u64,
    interrupt_enable: u32,
    software_interrupt: u32,
    hardware_interrupt: u32,
    watchdog: u64,
    watchdog_epoch: u64,
    timer_base: u32,
    timer_base_time: SimTime,
    cpu_error_address: u64,
    cpu_error_status: u64,
    interrupt_output_asserted: bool,
}

impl CrimePiu {
    /// Creates reset CRIME 1.1 PIU state for an R5000 O2.
    pub const fn new() -> Self {
        Self {
            control: registers::CONTROL_BIG_ENDIAN,
            interrupt_enable: 0,
            software_interrupt: 0,
            hardware_interrupt: 0,
            watchdog: 0,
            watchdog_epoch: 0,
            timer_base: 0,
            timer_base_time: SimTime::ZERO,
            cpu_error_address: 0,
            cpu_error_status: 0,
            interrupt_output_asserted: false,
        }
    }

    /// Restores deterministic power-on state and advances epochs.
    pub fn power_on(&mut self, now: SimTime) {
        self.control = registers::CONTROL_BIG_ENDIAN;
        self.interrupt_enable = 0;
        self.software_interrupt = 0;
        self.hardware_interrupt = 0;
        self.watchdog = 0;
        self.watchdog_epoch = self.watchdog_epoch.wrapping_add(1);
        self.timer_base = 0;
        self.timer_base_time = now;
        self.cpu_error_address = 0;
        self.cpu_error_status = 0;
        self.interrupt_output_asserted = false;
    }

    /// Restores hard-reset PIU state without changing simulated time.
    pub fn hard_reset(&mut self, now: SimTime) {
        self.power_on(now);
    }

    /// Returns the combined interrupt status.
    pub const fn interrupt_status(&self) -> u32 {
        self.hardware_interrupt | self.software_interrupt
    }

    /// Returns whether enabled CRIME interrupts assert its processor output.
    pub const fn interrupt_output_asserted(&self) -> bool {
        self.interrupt_output_asserted
    }

    pub(super) const fn timer_projection(&self) -> (u32, SimTime) {
        (self.timer_base, self.timer_base_time)
    }

    /// Reads a defined PIU register.
    pub fn read(&self, address: u64, now: SimTime, timebase_hz: u64) -> Option<u64> {
        match address {
            registers::ID => Some(registers::ID_VALUE),
            registers::CONTROL => Some(self.control),
            registers::INTERRUPT_STATUS => Some(u64::from(self.interrupt_status())),
            registers::INTERRUPT_ENABLE => Some(u64::from(self.interrupt_enable)),
            registers::SOFTWARE_INTERRUPT => Some(u64::from(self.software_interrupt)),
            registers::HARDWARE_INTERRUPT => Some(u64::from(self.hardware_interrupt)),
            registers::WATCHDOG => Some(self.watchdog),
            registers::TIMER => Some(u64::from(self.timer(now, timebase_hz))),
            registers::CPU_ERROR_ADDRESS => Some(self.cpu_error_address),
            registers::CPU_ERROR_STATUS => Some(self.cpu_error_status),
            _ => None,
        }
    }

    /// Writes a PIU register and returns ordered hardware effects.
    pub fn write(
        &mut self,
        address: u64,
        value: u64,
        now: SimTime,
        timebase_hz: u64,
    ) -> PiuWriteResult {
        let mut result = PiuWriteResult::handled();
        match address {
            registers::ID
            | registers::INTERRUPT_STATUS
            | registers::CPU_ERROR_ADDRESS
            | registers::CPU_RESERVED_WRITE_SINK => {}
            registers::CONTROL => {
                let old_watchdog_enabled = self.watchdog_enabled();
                let read_only = self.control & registers::CONTROL_BIG_ENDIAN;
                let reset_bits = registers::CONTROL_HARD_RESET | registers::CONTROL_SOFT_RESET;
                self.control = read_only | (value & registers::CONTROL_MASK & !reset_bits);

                if value & registers::CONTROL_SOFT_RESET != 0 {
                    result.effects.push(PiuEffect::WarmReset);
                }
                if value & registers::CONTROL_HARD_RESET != 0 {
                    result.effects.push(PiuEffect::HardReset);
                }

                let watchdog_enabled = self.watchdog_enabled();
                if old_watchdog_enabled != watchdog_enabled {
                    self.watchdog_epoch = self.watchdog_epoch.wrapping_add(1);
                    if watchdog_enabled {
                        result.effects.push(self.watchdog_arm(timebase_hz, 1));
                    }
                }
            }
            registers::INTERRUPT_ENABLE => {
                self.interrupt_enable = value as u32;
                self.push_interrupt_output_change(&mut result.effects);
            }
            registers::SOFTWARE_INTERRUPT => {
                self.software_interrupt = value as u32 & registers::SOFTWARE_INTERRUPT_MASK;
                self.push_interrupt_output_change(&mut result.effects);
            }
            registers::HARDWARE_INTERRUPT => {
                let read_only =
                    self.hardware_interrupt & !registers::HARDWARE_INTERRUPT_WRITABLE_MASK;
                self.hardware_interrupt =
                    read_only | (value as u32 & registers::HARDWARE_INTERRUPT_WRITABLE_MASK);
                self.push_interrupt_output_change(&mut result.effects);
            }
            registers::WATCHDOG => {
                self.watchdog = value
                    & (registers::WATCHDOG_POWER_ON_RESET
                        | registers::WATCHDOG_WARM_RESET
                        | registers::WATCHDOG_VALUE_MASK);
                self.watchdog_epoch = self.watchdog_epoch.wrapping_add(1);
                if self.watchdog_enabled() {
                    result.effects.push(self.watchdog_arm(timebase_hz, 1));
                }
            }
            registers::TIMER => {
                self.timer_base = value as u32;
                self.timer_base_time = now;
            }
            registers::CPU_ERROR_STATUS => {
                self.cpu_error_status = value & registers::CPU_ERROR_MASK;
                if self.cpu_error_status == 0 {
                    self.set_hardware_level(registers::INTERRUPT_CPU_ERROR, false);
                    self.push_interrupt_output_change(&mut result.effects);
                }
            }
            _ => return PiuWriteResult::unhandled(),
        }
        result
    }

    /// Updates one level-sensitive hardware source.
    pub fn set_hardware_level(&mut self, mask: u32, asserted: bool) -> Option<PiuEffect> {
        if asserted {
            self.hardware_interrupt |= mask;
        } else {
            self.hardware_interrupt &= !mask;
        }
        self.take_interrupt_output_change()
    }

    /// Latches one edge-sensitive hardware source.
    pub fn latch_hardware_edge(&mut self, mask: u32) -> Option<PiuEffect> {
        self.hardware_interrupt |= mask & registers::HARDWARE_INTERRUPT_WRITABLE_MASK;
        self.take_interrupt_output_change()
    }

    /// Records the first CPU interface error and asserts its interrupt.
    pub fn record_cpu_error(&mut self, address: u64, status: u64) -> Option<PiuEffect> {
        if self.cpu_error_status == 0 {
            self.cpu_error_address = (address >> 2) & 0xffff_ffff;
            self.cpu_error_status = status & registers::CPU_ERROR_MASK;
        }
        self.set_hardware_level(registers::INTERRUPT_CPU_ERROR, true)
    }

    /// Handles one watchdog event and ignores stale epochs.
    pub fn handle_watchdog(&mut self, epoch: u64, stage: u8, timebase_hz: u64) -> Vec<PiuEffect> {
        if epoch != self.watchdog_epoch || !self.watchdog_enabled() {
            return Vec::new();
        }
        match stage {
            1 => {
                self.watchdog |= registers::WATCHDOG_WARM_RESET;
                vec![PiuEffect::WarmReset, self.watchdog_arm(timebase_hz, 2)]
            }
            2 if self.watchdog & registers::WATCHDOG_WARM_RESET != 0 => {
                self.watchdog |= registers::WATCHDOG_POWER_ON_RESET;
                vec![PiuEffect::HardReset]
            }
            _ => Vec::new(),
        }
    }

    fn timer(&self, now: SimTime, timebase_hz: u64) -> u32 {
        let elapsed = now.get().saturating_sub(self.timer_base_time.get());
        let increments =
            u128::from(elapsed) * u128::from(CRIME_MASTER_FREQUENCY_HZ) / u128::from(timebase_hz);
        self.timer_base.wrapping_add(increments as u32)
    }

    const fn watchdog_enabled(&self) -> bool {
        self.control & registers::CONTROL_WATCHDOG_ENABLE != 0
    }

    fn watchdog_arm(&self, timebase_hz: u64, stage: u8) -> PiuEffect {
        let numerator = u128::from(WATCHDOG_THRESHOLD_CRIME_CYCLES) * u128::from(timebase_hz);
        let delay = numerator.div_ceil(u128::from(CRIME_MASTER_FREQUENCY_HZ));
        PiuEffect::ArmWatchdog {
            epoch: self.watchdog_epoch,
            stage,
            delay: SimDuration::new(delay as u64),
        }
    }

    fn push_interrupt_output_change(&mut self, effects: &mut Vec<PiuEffect>) {
        if let Some(effect) = self.take_interrupt_output_change() {
            effects.push(effect);
        }
    }

    fn take_interrupt_output_change(&mut self) -> Option<PiuEffect> {
        let asserted = self.interrupt_status() & self.interrupt_enable != 0;
        if asserted == self.interrupt_output_asserted {
            return None;
        }
        self.interrupt_output_asserted = asserted;
        Some(PiuEffect::InterruptOutput(asserted))
    }
}

impl Default for CrimePiu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
