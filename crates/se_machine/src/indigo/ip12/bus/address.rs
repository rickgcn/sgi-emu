use se_core::bus::{BusFault, DeviceAddr, PhysAddr};

use super::super::PROM_BYTES;

pub(super) const LOCAL_MEMORY_END: u64 = 0x1000_0000;
pub(super) const GIO_BASE: u64 = 0x1f00_0000;
pub(super) const GIO_END: u64 = 0x1f40_0000;
pub(super) const PIC1_BASE: u64 = 0x1fa0_0000;
const PIC1_END: u64 = 0x1fab_0000;
const HPC1_BASE: u64 = 0x1fb8_0000;
pub(super) const HPC1_ETHERNET_STATUS_BASE: u64 = 0x1fb8_0034;
pub(super) const HPC1_ETHERNET_POINTER_BASE: u64 = 0x1fb8_0058;
pub(super) const HPC1_ETHERNET_FIFO_BASE: u64 = 0x1fb8_005c;
pub(super) const HPC1_SCSI_REGISTERS_BASE: u64 = 0x1fb8_0088;
#[cfg(test)]
pub(super) const HPC1_SCSI_CONTROL_BASE: u64 = 0x1fb8_0094;
pub(super) const HPC1_ENDIAN_CONTROL_BASE: u64 = 0x1fb8_00c0;
pub(super) const HPC1_MISCELLANEOUS_CONTROL_BASE: u64 = 0x1fb8_01b0;
const HPC1_ETHERNET_STATUS_END: u64 = 0x1fb8_0040;
const HPC1_ETHERNET_POINTER_END: u64 = 0x1fb8_005c;
const HPC1_ETHERNET_FIFO_END: u64 = 0x1fb8_0060;
const HPC1_SCSI_REGISTERS_END: u64 = 0x1fb8_009c;
const HPC1_ENDIAN_CONTROL_END: u64 = 0x1fb8_00c4;
const HPC1_MISCELLANEOUS_CONTROL_END: u64 = 0x1fb8_01b4;
pub(super) const SCSI_BASE: u64 = 0x1fb8_0122;
const SCSI_END: u64 = 0x1fb8_0127;
pub(super) const CPU_AUX_CONTROL: u64 = 0x1fb8_01bf;
pub(super) const INT2_BASE: u64 = 0x1fb8_01c0;
const INT2_END: u64 = 0x1fb8_0200;
pub(super) const SERIAL_0_BASE: u64 = 0x1fb8_0d00;
const SERIAL_0_END: u64 = 0x1fb8_0d10;
pub(super) const SERIAL_1_BASE: u64 = 0x1fb8_0d10;
const SERIAL_1_END: u64 = 0x1fb8_0d20;
pub(super) const SERIAL_2_BASE: u64 = 0x1fb8_0d20;
const SERIAL_2_END: u64 = 0x1fb8_0d30;
pub(super) const MDAC_BASE: u64 = 0x1fb8_0d33;
const MDAC_END: u64 = 0x1fb8_0d38;
pub(super) const RTC_BASE: u64 = 0x1fb8_0e00;
const RTC_END: u64 = 0x1fb8_0e80;
pub(super) const BOARD_REVISION_BASE: u64 = 0x1fbd_0000;
const BOARD_REVISION_END: u64 = 0x1fbd_0004;
pub(super) const DSP56001_BASE: u64 = 0x1fbe_0000;
pub(super) const DSP56001_END: u64 = 0x1fc0_0000;
pub(super) const PROM_BASE: u64 = 0x1fc0_0000;
pub(super) const PROM_END: u64 = PROM_BASE + PROM_BYTES as u64;

pub(super) enum Target {
    Pic1(DeviceAddr),
    Hpc1(DeviceAddr),
    Scsi(DeviceAddr),
    CpuAuxControl,
    Int2(DeviceAddr),
    Serial(usize, DeviceAddr),
    UnpopulatedSerial(DeviceAddr),
    Mdac(DeviceAddr),
    Rtc(DeviceAddr),
    BoardRevision,
    Dsp56001(DeviceAddr),
    Prom(DeviceAddr),
    UnpopulatedGio,
}

pub(super) fn route(address: PhysAddr, length: usize) -> Result<Target, BusFault> {
    if !(1..=4).contains(&length) {
        return Err(BusFault::UnsupportedAccess);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusFault::UnsupportedAccess)?;
    let end = start.checked_add(length).ok_or(BusFault::Unmapped)?;

    if contains(start, end, PROM_BASE, PROM_END) {
        return Ok(Target::Prom(DeviceAddr::new(start - PROM_BASE)));
    }
    if overlaps(start, end, PROM_BASE, PROM_END) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, GIO_BASE, GIO_END) {
        return Ok(Target::UnpopulatedGio);
    }
    if overlaps(start, end, GIO_BASE, GIO_END) {
        return Err(BusFault::Unmapped);
    }

    for (range_start, range_end) in [
        (HPC1_ETHERNET_STATUS_BASE, HPC1_ETHERNET_STATUS_END),
        (HPC1_ETHERNET_POINTER_BASE, HPC1_ETHERNET_POINTER_END),
        (HPC1_ETHERNET_FIFO_BASE, HPC1_ETHERNET_FIFO_END),
        (HPC1_SCSI_REGISTERS_BASE, HPC1_SCSI_REGISTERS_END),
        (HPC1_ENDIAN_CONTROL_BASE, HPC1_ENDIAN_CONTROL_END),
        (
            HPC1_MISCELLANEOUS_CONTROL_BASE,
            HPC1_MISCELLANEOUS_CONTROL_END,
        ),
    ] {
        if contains(start, end, range_start, range_end) {
            return Ok(Target::Hpc1(DeviceAddr::new(start - HPC1_BASE)));
        }
        if overlaps(start, end, range_start, range_end) {
            return Err(BusFault::Unmapped);
        }
    }

    if contains(start, end, SCSI_BASE, SCSI_END) {
        return Ok(Target::Scsi(DeviceAddr::new(start - SCSI_BASE)));
    }
    if overlaps(start, end, SCSI_BASE, SCSI_END) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, CPU_AUX_CONTROL, CPU_AUX_CONTROL + 1) {
        return Ok(Target::CpuAuxControl);
    }
    if overlaps(start, end, CPU_AUX_CONTROL, CPU_AUX_CONTROL + 1) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, INT2_BASE, INT2_END) {
        return Ok(Target::Int2(DeviceAddr::new(start - INT2_BASE)));
    }
    if overlaps(start, end, INT2_BASE, INT2_END) {
        return Err(BusFault::Unmapped);
    }

    for (index, range_start, range_end) in [
        (0, SERIAL_0_BASE, SERIAL_0_END),
        (1, SERIAL_1_BASE, SERIAL_1_END),
    ] {
        if contains(start, end, range_start, range_end) {
            return Ok(Target::Serial(index, DeviceAddr::new(start - range_start)));
        }
        if overlaps(start, end, range_start, range_end) {
            return Err(BusFault::Unmapped);
        }
    }
    if contains(start, end, SERIAL_2_BASE, SERIAL_2_END) {
        return Ok(Target::UnpopulatedSerial(DeviceAddr::new(
            start - SERIAL_2_BASE,
        )));
    }
    if overlaps(start, end, SERIAL_2_BASE, SERIAL_2_END) {
        return Err(BusFault::Unmapped);
    }

    if contains(start, end, MDAC_BASE, MDAC_END) {
        return Ok(Target::Mdac(DeviceAddr::new(start - MDAC_BASE)));
    }
    if overlaps(start, end, MDAC_BASE, MDAC_END) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, RTC_BASE, RTC_END) {
        return Ok(Target::Rtc(DeviceAddr::new(start - RTC_BASE)));
    }
    if overlaps(start, end, RTC_BASE, RTC_END) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, BOARD_REVISION_BASE, BOARD_REVISION_END) {
        return Ok(Target::BoardRevision);
    }
    if overlaps(start, end, BOARD_REVISION_BASE, BOARD_REVISION_END) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, DSP56001_BASE, DSP56001_END) {
        return Ok(Target::Dsp56001(DeviceAddr::new(start - DSP56001_BASE)));
    }
    if overlaps(start, end, DSP56001_BASE, DSP56001_END) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, PIC1_BASE, PIC1_END) {
        return Ok(Target::Pic1(DeviceAddr::new(start - PIC1_BASE)));
    }
    Err(BusFault::Unmapped)
}

pub(super) fn local_memory_transaction_is_contained(
    address: PhysAddr,
    length: usize,
) -> Result<bool, BusFault> {
    if !(1..=4).contains(&length) {
        return Err(BusFault::UnsupportedAccess);
    }
    let Some(end) = address.get().checked_add(length as u64) else {
        return Ok(false);
    };
    Ok(end <= LOCAL_MEMORY_END)
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

    use super::super::test_support::{bus, read_byte, read_word};
    use super::{
        CPU_AUX_CONTROL, DSP56001_END, HPC1_ETHERNET_FIFO_BASE, HPC1_ETHERNET_POINTER_BASE,
        HPC1_MISCELLANEOUS_CONTROL_BASE, PROM_BASE, PROM_END, SERIAL_2_BASE,
    };

    #[test]
    fn hpc1_routes_only_implemented_register_windows() {
        let mut bus = bus();

        bus.write(
            PhysAddr::new(HPC1_MISCELLANEOUS_CONTROL_BASE),
            &9_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(PhysAddr::new(HPC1_ETHERNET_POINTER_BASE + 3), &[0x5a])
            .unwrap();

        assert_eq!(read_word(&mut bus, HPC1_MISCELLANEOUS_CONTROL_BASE), Ok(9));
        assert_eq!(
            read_byte(&mut bus, HPC1_ETHERNET_POINTER_BASE + 3),
            Ok(0x5a)
        );
        assert_eq!(read_byte(&mut bus, HPC1_ETHERNET_FIFO_BASE + 3), Ok(0));
        assert_eq!(read_word(&mut bus, 0x1fb8_0098), Ok(0));
        assert_eq!(read_byte(&mut bus, 0x1fb8_0100), Err(BusFault::Unmapped));
    }

    #[test]
    fn unpopulated_third_serial_controller_accepts_byte_writes() {
        let mut bus = bus();
        let address = PhysAddr::new(SERIAL_2_BASE + 0x0b);

        assert_eq!(bus.read(address, &mut [0]), Err(BusFault::Unmapped));
        assert_eq!(bus.debug_read(address, &mut [0]), Err(BusFault::Unmapped));
        for value in [0x09, 0xc0, 0x05, 0x00] {
            assert_eq!(bus.write(address, &[value]), Ok(()));
        }
        assert_eq!(
            bus.write(address, &[0; 2]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            bus.write(PhysAddr::new(SERIAL_2_BASE + 1), &[0]),
            Err(BusFault::Unmapped)
        );
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
    fn rejects_unmapped_and_crossing_transactions_atomically() {
        let mut bus = bus();

        for address in [PROM_END, 0x1fff_ffff, u64::MAX] {
            assert_eq!(
                bus.read(PhysAddr::new(address), &mut [0; 1]),
                Err(BusFault::Unmapped)
            );
        }
        assert_eq!(
            bus.read(PhysAddr::new(PROM_BASE - 1), &mut [0; 1]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            bus.read(PhysAddr::new(PROM_END - 2), &mut [0; 4]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            bus.write(PhysAddr::new(CPU_AUX_CONTROL - 1), &[0xaa, 0xbb]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            bus.write(PhysAddr::new(HPC1_ETHERNET_POINTER_BASE + 3), &[0xaa, 0xbb]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            bus.write(PhysAddr::new(DSP56001_END - 2), &[0xaa; 4]),
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
}
