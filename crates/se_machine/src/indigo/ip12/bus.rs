use se_core::bus::{BusFault, DeviceAddr, PhysAddr, PhysicalBus};
use se_device::hpc1::Hpc1;
use se_device::int2::Int2;
use se_device::pic1::Pic1;
use se_device::rom::Rom;

use super::PROM_BYTES;

const PIC1_BASE: u64 = 0x1fa0_0000;
const PIC1_END: u64 = 0x1fab_0000;
const HPC1_BASE: u64 = 0x1fb8_0000;
const HPC1_END: u64 = 0x1fb8_0200;
const INT2_BASE: u64 = 0x1fb8_01c0;
const INT2_END: u64 = 0x1fb8_01d0;
const PROM_BASE: u64 = 0x1fc0_0000;
const PROM_END: u64 = PROM_BASE + PROM_BYTES as u64;

pub(super) struct Ip12Bus {
    pic1: Pic1,
    hpc1: Hpc1,
    int2: Int2,
    prom: Rom,
}

impl Ip12Bus {
    pub(super) const fn new(pic1: Pic1, hpc1: Hpc1, int2: Int2, prom: Rom) -> Self {
        Self {
            pic1,
            hpc1,
            int2,
            prom,
        }
    }

    pub(super) fn reset(&mut self) {
        self.pic1.reset();
        self.hpc1.reset();
        self.int2.reset();
    }

    pub(super) fn take_system_reset_request(&mut self) -> bool {
        self.pic1.take_system_reset_request()
    }

    pub(super) fn debug_read(&self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.read(address, data),
            Target::Hpc1(address) => self.hpc1.read(address, data),
            Target::Int2(address) => self.int2.read(address, data),
            Target::Prom(address) => self.prom.read(address, data),
        }
    }
}

impl PhysicalBus for Ip12Bus {
    fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.read(address, data),
            Target::Hpc1(address) => self.hpc1.read(address, data),
            Target::Int2(address) => self.int2.read(address, data),
            Target::Prom(address) => self.prom.read(address, data),
        }
    }

    fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.write(address, data),
            Target::Hpc1(address) => self.hpc1.write(address, data),
            Target::Int2(address) => self.int2.write(address, data),
            Target::Prom(address) => self.prom.write(address, data),
        }
    }
}

enum Target {
    Pic1(DeviceAddr),
    Hpc1(DeviceAddr),
    Int2(DeviceAddr),
    Prom(DeviceAddr),
}

fn route(address: PhysAddr, length: usize) -> Result<Target, BusFault> {
    if !(1..=4).contains(&length) {
        return Err(BusFault::UnsupportedAccess);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusFault::UnsupportedAccess)?;
    let end = start.checked_add(length).ok_or(BusFault::Unmapped)?;

    if contains(start, end, INT2_BASE, INT2_END) {
        return Ok(Target::Int2(DeviceAddr::new(start - INT2_BASE)));
    }
    if overlaps(start, end, INT2_BASE, INT2_END) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, HPC1_BASE, HPC1_END) {
        return Ok(Target::Hpc1(DeviceAddr::new(start - HPC1_BASE)));
    }
    if contains(start, end, PIC1_BASE, PIC1_END) {
        return Ok(Target::Pic1(DeviceAddr::new(start - PIC1_BASE)));
    }
    if contains(start, end, PROM_BASE, PROM_END) {
        return Ok(Target::Prom(DeviceAddr::new(start - PROM_BASE)));
    }

    Err(BusFault::Unmapped)
}

const fn contains(start: u64, end: u64, range_start: u64, range_end: u64) -> bool {
    start >= range_start && end <= range_end
}

const fn overlaps(start: u64, end: u64, range_start: u64, range_end: u64) -> bool {
    start < range_end && end > range_start
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
    use se_device::hpc1::Hpc1;
    use se_device::int2::Int2;
    use se_device::pic1::Pic1;
    use se_device::rom::Rom;

    use super::{HPC1_BASE, INT2_BASE, Ip12Bus, PIC1_BASE, PROM_BASE, PROM_BYTES, PROM_END};

    fn bus() -> Ip12Bus {
        let bytes = (0..PROM_BYTES).map(|index| index as u8).collect();
        Ip12Bus::new(
            Pic1::new(0xf7, 2, true),
            Hpc1::new(),
            Int2::new(),
            Rom::new(bytes),
        )
    }

    fn read_word(bus: &mut Ip12Bus, address: u64) -> Result<u32, BusFault> {
        let mut bytes = [0; 4];
        bus.read(PhysAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    #[test]
    fn routes_pic1_hpc1_int2_and_prom_transactions() {
        let mut bus = bus();

        assert_eq!(read_word(&mut bus, PIC1_BASE + 4), Ok(0xf7));
        assert_eq!(read_word(&mut bus, HPC1_BASE + 0xc0), Ok(0x40));
        assert_eq!(bus.write(PhysAddr::new(INT2_BASE + 7), &[0xa5]), Ok(()));
        assert_eq!(read_word(&mut bus, INT2_BASE + 4), Ok(0xa5));

        let mut prom = [0; 4];
        assert_eq!(bus.read(PhysAddr::new(PROM_BASE), &mut prom), Ok(()));
        assert_eq!(prom, [0, 1, 2, 3]);
    }

    #[test]
    fn int2_decode_takes_priority_inside_the_hpc1_aperture() {
        let mut bus = bus();

        assert_eq!(bus.write(PhysAddr::new(INT2_BASE + 7), &[0x5a]), Ok(()));
        assert_eq!(read_word(&mut bus, INT2_BASE + 4), Ok(0x5a));
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
        assert_eq!(
            bus.write(PhysAddr::new(INT2_BASE - 1), &[0xaa, 0xbb]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            bus.read(PhysAddr::new(PIC1_BASE + 0x100), &mut [0; 1]),
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

    #[test]
    fn debug_reads_use_the_same_routing_without_consuming_reset_requests() {
        let mut bus = bus();
        bus.write(PhysAddr::new(PIC1_BASE), &0x0000_0200_u32.to_be_bytes())
            .unwrap();

        let mut system_id = [0; 4];
        assert_eq!(
            bus.debug_read(PhysAddr::new(PIC1_BASE + 8), &mut system_id),
            Ok(())
        );
        assert_eq!(u32::from_be_bytes(system_id), 0x88);
        assert!(bus.take_system_reset_request());
    }

    #[test]
    fn reset_restores_devices_without_changing_prom() {
        let mut bus = bus();
        bus.write(
            PhysAddr::new(PIC1_BASE + 0xa_0000),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(PhysAddr::new(HPC1_BASE + 0xc3), &[0x1f]).unwrap();
        bus.write(PhysAddr::new(INT2_BASE + 7), &[0xa5]).unwrap();

        bus.reset();

        assert_eq!(read_word(&mut bus, PIC1_BASE + 0xa_0000), Ok(0));
        assert_eq!(read_word(&mut bus, HPC1_BASE + 0xc0), Ok(0x40));
        assert_eq!(read_word(&mut bus, INT2_BASE + 4), Ok(0));
        let mut prom = [0; 4];
        assert_eq!(bus.read(PhysAddr::new(PROM_BASE), &mut prom), Ok(()));
        assert_eq!(prom, [0, 1, 2, 3]);
    }
}
