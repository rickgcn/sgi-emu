use se_core::bus::{BusError, DeviceAddr, PhysAddr};

use super::super::PROM_BYTES;

pub(super) const LOCAL_MEMORY_END: u64 = 0x1000_0000;
pub(super) const GIO_BASE: u64 = 0x1f00_0000;
pub(super) const GIO_END: u64 = 0x1f40_0000;
pub(super) const PIC1_BASE: u64 = 0x1fa0_0000;
const PIC1_END: u64 = 0x1fab_0000;
const HPC1_BASE: u64 = 0x1fb8_0000;
const HPC1_INTERNAL_END: u64 = 0x1fb8_00c0;
#[cfg(test)]
pub(super) const HPC1_ETHERNET_TIMER_BASE: u64 = 0x1fb8_002c;
#[cfg(test)]
pub(super) const HPC1_ETHERNET_POINTER_BASE: u64 = 0x1fb8_0058;
#[cfg(test)]
pub(super) const HPC1_ETHERNET_FIFO_BASE: u64 = 0x1fb8_005c;
#[cfg(test)]
pub(super) const HPC1_SCSI_REGISTERS_BASE: u64 = 0x1fb8_0088;
#[cfg(test)]
pub(super) const HPC1_SCSI_CONTROL_BASE: u64 = 0x1fb8_0094;
pub(super) const HPC1_ENDIAN_CONTROL_BASE: u64 = 0x1fb8_00c0;
pub(super) const HPC1_COUNTER_BASE: u64 = 0x1fb8_0194;
pub(super) const HPC1_DSP_INTERRUPT_STATUS_BASE: u64 = 0x1fb8_01a0;
pub(super) const HPC1_DSP_INTERRUPT_MASK_BASE: u64 = 0x1fb8_01a4;
pub(super) const HPC1_MISCELLANEOUS_CONTROL_BASE: u64 = 0x1fb8_01b0;
const HPC1_ENDIAN_CONTROL_END: u64 = 0x1fb8_00c4;
const HPC1_COUNTER_END: u64 = 0x1fb8_0198;
const HPC1_DSP_INTERRUPT_END: u64 = HPC1_DSP_INTERRUPT_MASK_BASE + 4;
const HPC1_MISCELLANEOUS_CONTROL_END: u64 = 0x1fb8_01b4;
pub(super) const SEEQ8003_EXTERNAL_BASE: u64 = 0x1fb8_0100;
const SEEQ8003_EXTERNAL_END: u64 = 0x1fb8_0120;
const SEEQ8003_RECEIVE_ALIAS: u64 = 0x1fb8_005b;
const SEEQ8003_TRANSMIT_ALIAS: u64 = 0x1fb8_005f;
const SEEQ8003_RECEIVE_COMMAND: u64 = 0x1b;
const SEEQ8003_TRANSMIT_COMMAND: u64 = 0x1f;
pub(super) const SCSI_BASE: u64 = 0x1fb8_0122;
const SCSI_END: u64 = 0x1fb8_0127;
pub(super) const CENTRONICS_EXTERNAL_BASE: u64 = 0x1fb8_0134;
const CENTRONICS_EXTERNAL_END: u64 = 0x1fb8_0138;
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
    Centronics(DeviceAddr),
    Seeq8003(DeviceAddr),
    Scsi(DeviceAddr),
    CpuAuxControl,
    Int2(DeviceAddr),
    Serial(usize, DeviceAddr),
    Mdac(DeviceAddr),
    Rtc(DeviceAddr),
    BoardRevision,
    Dsp56001(DeviceAddr),
    Prom(DeviceAddr),
    Gio(DeviceAddr),
}

pub(super) fn route(address: PhysAddr, length: usize) -> Result<Target, BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusError::InvalidTransaction)?;
    let end = start
        .checked_add(length)
        .ok_or(BusError::InvalidTransaction)?;

    if contains(start, end, PROM_BASE, PROM_END) {
        return Ok(Target::Prom(DeviceAddr::new(start - PROM_BASE)));
    }
    if overlaps(start, end, PROM_BASE, PROM_END) {
        return Err(BusError::HardwareFault);
    }
    if contains(start, end, GIO_BASE, GIO_END) {
        return Ok(Target::Gio(DeviceAddr::new(start - GIO_BASE)));
    }
    if overlaps(start, end, GIO_BASE, GIO_END) {
        return Err(BusError::HardwareFault);
    }

    if start == SEEQ8003_RECEIVE_ALIAS && end == start + 1 {
        return Ok(Target::Seeq8003(DeviceAddr::new(SEEQ8003_RECEIVE_COMMAND)));
    }
    if start == SEEQ8003_TRANSMIT_ALIAS && end == start + 1 {
        return Ok(Target::Seeq8003(DeviceAddr::new(SEEQ8003_TRANSMIT_COMMAND)));
    }

    for (range_start, range_end) in [
        (HPC1_BASE, HPC1_INTERNAL_END),
        (HPC1_ENDIAN_CONTROL_BASE, HPC1_ENDIAN_CONTROL_END),
        (HPC1_COUNTER_BASE, HPC1_COUNTER_END),
        (HPC1_DSP_INTERRUPT_STATUS_BASE, HPC1_DSP_INTERRUPT_END),
        (
            HPC1_MISCELLANEOUS_CONTROL_BASE,
            HPC1_MISCELLANEOUS_CONTROL_END,
        ),
    ] {
        if contains(start, end, range_start, range_end) {
            return Ok(Target::Hpc1(DeviceAddr::new(start - HPC1_BASE)));
        }
        if overlaps(start, end, range_start, range_end) {
            return Err(BusError::HardwareFault);
        }
    }

    if contains(start, end, SEEQ8003_EXTERNAL_BASE, SEEQ8003_EXTERNAL_END) {
        return Ok(Target::Seeq8003(DeviceAddr::new(
            start - SEEQ8003_EXTERNAL_BASE,
        )));
    }
    if overlaps(start, end, SEEQ8003_EXTERNAL_BASE, SEEQ8003_EXTERNAL_END) {
        return Err(BusError::HardwareFault);
    }

    if contains(start, end, SCSI_BASE, SCSI_END) {
        return Ok(Target::Scsi(DeviceAddr::new(start - SCSI_BASE)));
    }
    if overlaps(start, end, SCSI_BASE, SCSI_END) {
        return Err(BusError::HardwareFault);
    }
    if contains(
        start,
        end,
        CENTRONICS_EXTERNAL_BASE,
        CENTRONICS_EXTERNAL_END,
    ) {
        return Ok(Target::Centronics(DeviceAddr::new(
            start - CENTRONICS_EXTERNAL_BASE,
        )));
    }
    if overlaps(
        start,
        end,
        CENTRONICS_EXTERNAL_BASE,
        CENTRONICS_EXTERNAL_END,
    ) {
        return Err(BusError::UnimplementedAccess);
    }
    if contains(start, end, CPU_AUX_CONTROL, CPU_AUX_CONTROL + 1) {
        return Ok(Target::CpuAuxControl);
    }
    if overlaps(start, end, CPU_AUX_CONTROL, CPU_AUX_CONTROL + 1) {
        return Err(BusError::HardwareFault);
    }
    if contains(start, end, INT2_BASE, INT2_END) {
        return Ok(Target::Int2(DeviceAddr::new(start - INT2_BASE)));
    }
    if overlaps(start, end, INT2_BASE, INT2_END) {
        return Err(BusError::HardwareFault);
    }

    for (index, range_start, range_end) in [
        (0, SERIAL_0_BASE, SERIAL_0_END),
        (1, SERIAL_1_BASE, SERIAL_1_END),
        (2, SERIAL_2_BASE, SERIAL_2_END),
    ] {
        if contains(start, end, range_start, range_end) {
            return Ok(Target::Serial(index, DeviceAddr::new(start - range_start)));
        }
        if overlaps(start, end, range_start, range_end) {
            return Err(BusError::HardwareFault);
        }
    }
    if contains(start, end, MDAC_BASE, MDAC_END) {
        return Ok(Target::Mdac(DeviceAddr::new(start - MDAC_BASE)));
    }
    if overlaps(start, end, MDAC_BASE, MDAC_END) {
        return Err(BusError::HardwareFault);
    }
    if contains(start, end, RTC_BASE, RTC_END) {
        return Ok(Target::Rtc(DeviceAddr::new(start - RTC_BASE)));
    }
    if overlaps(start, end, RTC_BASE, RTC_END) {
        return Err(BusError::HardwareFault);
    }
    if contains(start, end, BOARD_REVISION_BASE, BOARD_REVISION_END) {
        return Ok(Target::BoardRevision);
    }
    if overlaps(start, end, BOARD_REVISION_BASE, BOARD_REVISION_END) {
        return Err(BusError::HardwareFault);
    }
    if contains(start, end, DSP56001_BASE, DSP56001_END) {
        return Ok(Target::Dsp56001(DeviceAddr::new(start - DSP56001_BASE)));
    }
    if overlaps(start, end, DSP56001_BASE, DSP56001_END) {
        return Err(BusError::HardwareFault);
    }
    if contains(start, end, PIC1_BASE, PIC1_END) {
        return Ok(Target::Pic1(DeviceAddr::new(start - PIC1_BASE)));
    }
    Err(BusError::HardwareFault)
}

pub(super) fn local_memory_transaction_is_contained(
    address: PhysAddr,
    length: usize,
) -> Result<bool, BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }
    let end = address
        .get()
        .checked_add(length as u64)
        .ok_or(BusError::InvalidTransaction)?;
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
    use se_core::bus::{BusError, PhysAddr, PhysicalBus};

    use super::super::test_support::{bus, read_byte, read_word};
    use super::{
        CENTRONICS_EXTERNAL_BASE, CPU_AUX_CONTROL, DSP56001_END, GIO_BASE, HPC1_COUNTER_BASE,
        HPC1_DSP_INTERRUPT_MASK_BASE, HPC1_DSP_INTERRUPT_STATUS_BASE, HPC1_ETHERNET_FIFO_BASE,
        HPC1_ETHERNET_POINTER_BASE, HPC1_MISCELLANEOUS_CONTROL_BASE, HPC1_SCSI_REGISTERS_BASE,
        PIC1_BASE, PROM_BASE, PROM_END, SEEQ8003_EXTERNAL_BASE, SERIAL_2_BASE,
    };

    #[test]
    fn routes_hpc1_and_external_peripheral_registers() {
        let mut bus = bus();

        bus.write(
            PhysAddr::new(HPC1_MISCELLANEOUS_CONTROL_BASE),
            &9_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(
            PhysAddr::new(HPC1_ETHERNET_POINTER_BASE),
            &0_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(PhysAddr::new(HPC1_ETHERNET_POINTER_BASE + 3), &[0])
            .unwrap();
        bus.write(PhysAddr::new(HPC1_ETHERNET_FIFO_BASE + 3), &[0])
            .unwrap();

        assert_eq!(read_word(&mut bus, 0x1fb8_0000), Ok(0));
        assert_eq!(read_word(&mut bus, HPC1_MISCELLANEOUS_CONTROL_BASE), Ok(9));
        assert_eq!(read_word(&mut bus, HPC1_ETHERNET_POINTER_BASE), Ok(0));
        assert_eq!(
            read_byte(&mut bus, HPC1_ETHERNET_POINTER_BASE + 3),
            Ok(0x80)
        );
        assert_eq!(read_byte(&mut bus, HPC1_ETHERNET_FIFO_BASE + 3), Ok(0));
        assert_eq!(read_word(&mut bus, 0x1fb8_0098), Ok(0));
        let mut scsi_channel_pointer = [0xff; 2];
        bus.read(
            PhysAddr::new(HPC1_SCSI_REGISTERS_BASE + 0x10),
            &mut scsi_channel_pointer,
        )
        .unwrap();
        assert_eq!(scsi_channel_pointer, [0; 2]);
        assert_eq!(read_word(&mut bus, HPC1_COUNTER_BASE), Ok(0));
        bus.write(
            PhysAddr::new(HPC1_DSP_INTERRUPT_MASK_BASE),
            &u32::MAX.to_be_bytes(),
        )
        .unwrap();
        bus.write(
            PhysAddr::new(HPC1_DSP_INTERRUPT_STATUS_BASE),
            &0_u32.to_be_bytes(),
        )
        .unwrap();
        assert_eq!(read_word(&mut bus, HPC1_DSP_INTERRUPT_MASK_BASE), Ok(7));
        assert_eq!(read_word(&mut bus, HPC1_DSP_INTERRUPT_STATUS_BASE), Ok(0));
        bus.write(PhysAddr::new(CENTRONICS_EXTERNAL_BASE + 1), &[0])
            .unwrap();
        bus.write(PhysAddr::new(CENTRONICS_EXTERNAL_BASE + 1), &[3])
            .unwrap();
        assert_eq!(read_byte(&mut bus, CENTRONICS_EXTERNAL_BASE + 1), Ok(0));
        assert_eq!(read_word(&mut bus, CENTRONICS_EXTERNAL_BASE), Ok(0));
        assert_eq!(read_word(&mut bus, SEEQ8003_EXTERNAL_BASE), Ok(0x80));
        assert_eq!(
            read_byte(&mut bus, HPC1_COUNTER_BASE + 3),
            Err(BusError::UnimplementedAccess)
        );
    }

    #[test]
    fn absent_targets_share_hardware_read_errors_and_write_completion() {
        let mut bus = bus();
        for address in [0x1fb0_0010, 0x1fb0_0050, 0x1f98_0010, 0x1f98_0050] {
            for length in 1..=4 {
                let mut data = [0xa5; 4];
                assert_eq!(
                    bus.read(PhysAddr::new(address), &mut data[..length]),
                    Err(BusError::HardwareFault)
                );
                assert_eq!(
                    bus.debug_read(PhysAddr::new(address), &mut data[..length]),
                    Err(BusError::HardwareFault)
                );
                assert_eq!(data, [0xa5; 4]);
                assert!(!bus.error_interrupt_asserted());

                bus.write(PhysAddr::new(address), &data[..length]).unwrap();
                assert!(bus.error_interrupt_asserted());
                bus.write(PhysAddr::new(PIC1_BASE + 0x1_0210), &[0])
                    .unwrap();
                assert!(!bus.error_interrupt_asserted());
            }
        }
    }

    #[test]
    fn absent_serial_byte_writes_complete_without_latching_an_error() {
        let mut bus = bus();
        for offset in [0x03, 0x07, 0x0b, 0x0f] {
            let address = PhysAddr::new(SERIAL_2_BASE + offset);
            let mut byte = [0xa5];
            assert_eq!(bus.read(address, &mut byte), Err(BusError::HardwareFault));
            assert_eq!(
                bus.debug_read(address, &mut byte),
                Err(BusError::HardwareFault)
            );
            assert_eq!(byte, [0xa5]);
            for value in [9, 0xc0, 5, 0] {
                assert_eq!(bus.write(address, &[value]), Ok(()));
                assert!(!bus.error_interrupt_asserted());
            }
        }
        for (offset, length) in [(0, 1), (1, 1), (3, 2), (3, 3), (3, 4)] {
            assert_eq!(
                bus.write(PhysAddr::new(SERIAL_2_BASE + offset), &[0; 4][..length]),
                Err(BusError::UnimplementedAccess)
            );
            assert!(!bus.error_interrupt_asserted());
        }

        bus.write(PhysAddr::new(0x1fb0_0010), &[0]).unwrap();
        bus.write(PhysAddr::new(SERIAL_2_BASE + 0x0b), &[0])
            .unwrap();
        assert!(bus.error_interrupt_asserted());
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

        for address in [PROM_END, 0x1fff_ffff] {
            assert_eq!(
                bus.read(PhysAddr::new(address), &mut [0; 1]),
                Err(BusError::HardwareFault)
            );
        }
        assert_eq!(
            bus.read(PhysAddr::new(PROM_BASE - 1), &mut [0; 1]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            bus.read(PhysAddr::new(PROM_END - 2), &mut [0; 4]),
            Err(BusError::HardwareFault)
        );
        assert_eq!(
            bus.write(PhysAddr::new(CPU_AUX_CONTROL - 1), &[0xaa, 0xbb]),
            Ok(())
        );
        assert!(bus.error_interrupt_asserted());
        bus.write(PhysAddr::new(PIC1_BASE + 0x1_0210), &[0])
            .unwrap();
        assert_eq!(
            bus.write(PhysAddr::new(HPC1_ETHERNET_POINTER_BASE + 3), &[0xaa, 0xbb]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            bus.read(PhysAddr::new(SEEQ8003_EXTERNAL_BASE + 3), &mut [0; 2]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            bus.write(PhysAddr::new(CENTRONICS_EXTERNAL_BASE + 3), &[0; 2]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            bus.write(PhysAddr::new(DSP56001_END - 2), &[0xaa; 4]),
            Ok(())
        );
        assert!(bus.error_interrupt_asserted());
    }

    #[test]
    fn invalid_transactions_never_reach_a_target_or_latch_a_hardware_error() {
        let mut bus = bus();

        for address in [0, PROM_BASE, GIO_BASE, SERIAL_2_BASE, PIC1_BASE] {
            for length in [0, 5] {
                let mut bytes = vec![0xa5; length];
                assert_eq!(
                    bus.read(PhysAddr::new(address), &mut bytes),
                    Err(BusError::InvalidTransaction)
                );
                assert_eq!(
                    bus.debug_read(PhysAddr::new(address), &mut bytes),
                    Err(BusError::InvalidTransaction)
                );
                assert_eq!(
                    bus.write(PhysAddr::new(address), &bytes),
                    Err(BusError::InvalidTransaction)
                );
                assert_eq!(bytes, vec![0xa5; length]);
                assert!(!bus.error_interrupt_asserted());
            }
        }
        for length in 1..=4 {
            let mut bytes = [0xa5; 4];
            assert_eq!(
                bus.read(PhysAddr::new(u64::MAX), &mut bytes[..length]),
                Err(BusError::InvalidTransaction)
            );
            assert_eq!(
                bus.write(PhysAddr::new(u64::MAX), &bytes[..length]),
                Err(BusError::InvalidTransaction)
            );
            assert_eq!(bytes, [0xa5; 4]);
            assert!(!bus.error_interrupt_asserted());
        }
    }
}
