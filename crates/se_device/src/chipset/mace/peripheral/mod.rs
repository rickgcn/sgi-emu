//! MACE peripheral controller: ISA DMA control, PS/2, I2C, and timers.

use se_core::scheduler::{SimDuration, SimTime};

/// Affine MACE UST model captured for a synchronous read batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaceUstProjection {
    /// UST value at `base_time`.
    pub base: u32,

    /// Simulation-time origin of `base`.
    pub base_time: SimTime,

    /// Numerator frequency used by the affine counter.
    pub frequency_hz: u64,

    /// Denominator timebase used by the affine counter.
    pub timebase_hz: u64,
}

/// MACE 32-bit UST and three compare registers.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceTimers {
    timebase_hz: u64,
    base_time: SimTime,
    base_ust: u32,
    compare: [u32; 3],
    pending: [bool; 3],
    media: [(u32, u32); 6],
}

impl MaceTimers {
    pub const fn new(timebase_hz: u64) -> Self {
        Self {
            timebase_hz,
            base_time: SimTime::ZERO,
            base_ust: 0,
            compare: [0; 3],
            pending: [false; 3],
            media: [(0, 0); 6],
        }
    }

    pub fn power_on(&mut self, now: SimTime) {
        self.base_time = now;
        self.base_ust = 0;
        self.compare = [0; 3];
        self.pending = [false; 3];
        self.media = [(0, 0); 6];
    }

    pub fn ust(&self, now: SimTime) -> u32 {
        let elapsed = now.get().saturating_sub(self.base_time.get()) as u128;
        let increments =
            elapsed.saturating_mul(1_000_000_000) / (u128::from(self.timebase_hz) * 960);
        self.base_ust.wrapping_add(increments as u32)
    }

    /// Captures the exact affine UST model without observing a new time.
    pub fn ust_projection(&self) -> Option<MaceUstProjection> {
        Some(MaceUstProjection {
            base: self.base_ust,
            base_time: self.base_time,
            frequency_hz: 1_000_000_000,
            timebase_hz: self.timebase_hz.checked_mul(960)?,
        })
    }

    pub fn write_ust(&mut self, now: SimTime, value: u32) {
        self.base_time = now;
        self.base_ust = value;
    }

    pub fn compare(&self, index: usize) -> u32 {
        self.compare[index]
    }
    pub fn pending(&self, index: usize) -> bool {
        self.pending[index]
    }

    pub fn write_compare(&mut self, index: usize, value: u32) {
        self.compare[index] = value;
        self.pending[index] = false;
    }

    pub fn compare_delay(&self, now: SimTime, index: usize) -> SimDuration {
        let delta = self.compare[index].wrapping_sub(self.ust(now));
        let ticks = u128::from(delta) * u128::from(self.timebase_hz) * 960 / 1_000_000_000;
        SimDuration::new(ticks.max(1).min(u128::from(u64::MAX)) as u64)
    }

    pub fn fire_compare(&mut self, now: SimTime, index: usize) -> bool {
        if self.ust(now) == self.compare[index]
            || self.ust(now).wrapping_sub(self.compare[index]) < 0x8000_0000
        {
            self.pending[index] = true;
        }
        self.pending[index]
    }

    pub fn media_pair(&self, index: usize) -> u64 {
        let (ust, msc) = self.media[index];
        u64::from(ust) << 32 | u64::from(msc)
    }

    pub fn write_media_pair(&mut self, index: usize, value: u64) {
        self.media[index] = ((value >> 32) as u32, value as u32);
    }

    pub fn increment_media(&mut self, now: SimTime, index: usize) {
        self.media[index].0 = self.ust(now);
        self.media[index].1 = self.media[index].1.wrapping_add(1);
    }
}

/// One integrated PS/2 controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2Port {
    transmit: Option<u8>,
    receive: Option<u8>,
    control: u8,
    parity_error: bool,
    framing_error: bool,
    transmitting: bool,
}

impl Ps2Port {
    pub const fn new() -> Self {
        Self {
            transmit: None,
            receive: None,
            control: 0x10,
            parity_error: false,
            framing_error: false,
            transmitting: false,
        }
    }

    pub fn write_transmit(&mut self, value: u8) {
        self.transmit = Some(value);
        self.transmitting = self.control & 2 != 0;
    }
    pub fn take_transmit(&mut self) -> Option<u8> {
        self.transmitting = false;
        self.transmit.take()
    }
    pub fn receive(&mut self, value: u8, parity_error: bool, framing_error: bool) {
        self.receive = Some(value);
        self.parity_error = parity_error;
        self.framing_error = framing_error;
    }
    pub fn read_receive(&mut self) -> u16 {
        let status = self.status();
        let data = self.receive.take().unwrap_or(0);
        u16::from(status) << 8 | u16::from(data)
    }
    pub const fn control(&self) -> u8 {
        self.control
    }
    pub fn set_control(&mut self, value: u8) {
        self.control = value & 0x3f;
        if value & 0x20 != 0 {
            *self = Self::new();
        }
    }
    pub fn status(&self) -> u8 {
        u8::from(self.control & 0x10 != 0) << 1
            | u8::from(self.transmitting) << 2
            | u8::from(self.transmit.is_none()) << 3
            | u8::from(self.receive.is_some()) << 4
            | u8::from(self.parity_error) << 6
            | u8::from(self.framing_error) << 7
    }
    pub fn interrupt(&self) -> bool {
        self.control & 4 != 0 && self.transmit.is_none()
            || self.control & 8 != 0 && self.receive.is_some()
    }
}

impl Default for Ps2Port {
    fn default() -> Self {
        Self::new()
    }
}

/// One MACE I2C controller register set.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct I2cPort {
    pub config: u8,
    pub control: u8,
    pub data: u8,
}

impl I2cPort {
    pub const fn new() -> Self {
        Self {
            config: 0,
            control: 0,
            data: 0,
        }
    }
    pub fn reset(&mut self) {
        *self = Self::new();
    }
    pub const fn fast(&self) -> bool {
        self.config & 2 != 0
    }
    pub const fn busy(&self) -> bool {
        self.control & 0x10 != 0
    }
    pub fn begin(&mut self) {
        self.control |= 0x10;
        self.control &= !0xa0;
    }
    pub fn complete(&mut self, acknowledged: bool, bus_error: bool) {
        self.control &= !0x10;
        if !acknowledged {
            self.control |= 0x20;
        }
        if bus_error {
            self.control |= 0x80;
        }
    }
}

impl Default for I2cPort {
    fn default() -> Self {
        Self::new()
    }
}

/// ISA control, DP-RAM, and DMA ring base state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct IsaController {
    pub ring_base_reset: u32,
    misc: u16,
    nic_line_high: bool,
    pub dp_ram: Vec<u8>,
    pub parallel: ParallelDma,
    pub serial_dma: [PeripheralDmaChannel; 4],
}

impl IsaController {
    pub fn new() -> Self {
        Self {
            ring_base_reset: 1,
            misc: 1 << 5,
            nic_line_high: false,
            dp_ram: vec![0; 8192],
            parallel: ParallelDma::new(),
            serial_dma: [PeripheralDmaChannel::new(); 4],
        }
    }
    pub fn reset(&mut self) {
        self.ring_base_reset = 1;
        self.misc = 1 << 5;
        self.nic_line_high = false;
        self.dp_ram.fill(0);
        self.parallel.reset();
        self.serial_dma.fill(PeripheralDmaChannel::new());
    }
    pub const fn ring_base(&self) -> u32 {
        self.ring_base_reset & 0xffff_8000
    }
    pub const fn flash_write_enabled(&self) -> bool {
        self.misc & 1 != 0
    }
    pub const fn read_misc(&self) -> u16 {
        self.misc | (self.nic_line_high as u16) << 3
    }
    pub fn write_misc(&mut self, value: u16) {
        self.misc = value & 0x01f5;
    }
    pub const fn nic_drive_low(&self) -> bool {
        self.misc & (1 << 2) == 0
    }
    pub fn set_nic_line_low(&mut self, line_low: bool) {
        self.nic_line_high = !line_low;
    }
    pub const fn dp_ram_write_enabled(&self) -> bool {
        self.misc & (1 << 6) != 0
    }
}

/// One 4 KiB serial DMA ring channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PeripheralDmaChannel {
    pub control: u16,
    pub read_pointer: u16,
    pub write_pointer: u16,
}

impl PeripheralDmaChannel {
    pub const fn new() -> Self {
        Self {
            control: 1 << 10,
            read_pointer: 0,
            write_pointer: 0,
        }
    }
    pub fn write_control(&mut self, value: u16) {
        if value & (1 << 10) != 0 {
            *self = Self::new();
        } else {
            self.control = value & 0x06e0;
        }
    }
    pub fn depth(&self) -> u16 {
        self.write_pointer.wrapping_sub(self.read_pointer) & 0x0fe0
    }
}

impl Default for PeripheralDmaChannel {
    fn default() -> Self {
        Self::new()
    }
}

/// Parallel DMA double-buffered context state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ParallelDma {
    pub context: [u64; 2],
    pub control: u8,
    pub diagnostic: u16,
}

impl ParallelDma {
    pub const fn new() -> Self {
        Self {
            context: [0; 2],
            control: 1 << 2,
            diagnostic: 0,
        }
    }
    pub fn reset(&mut self) {
        *self = Self::new();
    }
    pub fn write_control(&mut self, value: u8) {
        if value & (1 << 2) != 0 {
            self.reset();
        } else {
            self.control = value & 0x1f;
        }
    }
}

impl Default for ParallelDma {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for IsaController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ust_advances_every_960_nanoseconds() {
        let timers = MaceTimers::new(1_000_000_000);
        assert_eq!(timers.ust(SimTime::new(9_600)), 10);
    }
    #[test]
    fn ps2_receive_read_clears_full_flag() {
        let mut ps2 = Ps2Port::new();
        ps2.receive(0xaa, false, false);
        assert_eq!(ps2.status() & 0x10, 0x10);
        assert_eq!(ps2.read_receive() & 0xff, 0xaa);
        assert_eq!(ps2.status() & 0x10, 0);
    }
}
