use se_core::bus::{BusFault, DeviceAddr, PhysAddr, PhysicalBus};
use se_device::rom::Rom;

use super::PROM_BYTES;

const PROM_BASE: u64 = 0x1fc0_0000;
const PROM_END: u64 = PROM_BASE + PROM_BYTES as u64;

pub(super) struct Ip12Bus {
    prom: Rom,
}

impl Ip12Bus {
    pub(super) const fn new(prom: Rom) -> Self {
        Self { prom }
    }

    fn prom_address(address: PhysAddr, length: usize) -> Result<DeviceAddr, BusFault> {
        if !(1..=4).contains(&length) {
            return Err(BusFault::UnsupportedAccess);
        }

        let start = address.get();
        let length = u64::try_from(length).map_err(|_| BusFault::UnsupportedAccess)?;
        let end = start.checked_add(length).ok_or(BusFault::Unmapped)?;
        if start < PROM_BASE || end > PROM_END {
            return Err(BusFault::Unmapped);
        }

        Ok(DeviceAddr::new(start - PROM_BASE))
    }

    pub(super) fn debug_read(&self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        self.prom
            .read(Self::prom_address(address, data.len())?, data)
    }
}

impl PhysicalBus for Ip12Bus {
    fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        self.prom
            .read(Self::prom_address(address, data.len())?, data)
    }

    fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
        self.prom
            .write(Self::prom_address(address, data.len())?, data)
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
    use se_device::rom::Rom;

    use super::{Ip12Bus, PROM_BASE, PROM_BYTES, PROM_END};

    fn bus() -> Ip12Bus {
        let bytes = (0..PROM_BYTES).map(|index| index as u8).collect();
        Ip12Bus::new(Rom::new(bytes))
    }

    #[test]
    fn reads_the_start_and_end_of_the_prom_window() {
        let mut bus = bus();
        let mut first = [0; 4];
        let mut last = [0; 4];

        assert_eq!(bus.read(PhysAddr::new(PROM_BASE), &mut first), Ok(()));
        assert_eq!(bus.read(PhysAddr::new(PROM_END - 4), &mut last), Ok(()));
        assert_eq!(first, [0, 1, 2, 3]);
        assert_eq!(last, [0xfc, 0xfd, 0xfe, 0xff]);
    }

    #[test]
    fn rejects_unmapped_and_crossing_transactions() {
        let mut bus = bus();

        for address in [PROM_BASE - 1, PROM_END, 0x1fff_ffff, u64::MAX] {
            assert_eq!(
                bus.read(PhysAddr::new(address), &mut [0; 1]),
                Err(BusFault::Unmapped)
            );
        }
        assert_eq!(
            bus.read(PhysAddr::new(PROM_END - 2), &mut [0; 4]),
            Err(BusFault::Unmapped)
        );
    }

    #[test]
    fn rejects_unsupported_widths_before_address_decode() {
        let mut bus = bus();

        assert_eq!(
            bus.read(PhysAddr::new(0), &mut []),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            bus.read(PhysAddr::new(0), &mut [0; 5]),
            Err(BusFault::UnsupportedAccess)
        );
    }

    #[test]
    fn rejects_prom_writes() {
        let mut bus = bus();

        assert_eq!(
            bus.write(PhysAddr::new(PROM_BASE), &[1, 2, 3, 4]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            bus.write(PhysAddr::new(PROM_END), &[1]),
            Err(BusFault::Unmapped)
        );
    }
}
