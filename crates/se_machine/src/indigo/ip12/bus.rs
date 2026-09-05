use se_core::bus::{BusError, PhysAddr, PhysicalBus};
use se_device::centronics::CentronicsPort;
use se_device::dp8573a::{Dp8573a, Dp8573aBatteryState};
use se_device::dsp56001::Dsp56001;
use se_device::hpc1::Hpc1;
use se_device::int2::Int2;
use se_device::mdac::Mdac;
use se_device::nmc93cs46::{Nmc93cs46, Nmc93cs46Contents};
use se_device::pic1::Pic1;
use se_device::ram::Ram;
use se_device::rom::Rom;
use se_device::scsi::{ScsiBus, ScsiBusSnapshot, ScsiSnapshotError};
use se_device::seeq8003::Seeq8003;
use se_device::wd33c93b::{SelectAndTransferRequest, Wd33c93b};
use se_device::z85230::{Channel, Z85230};
use serde::{Deserialize, Serialize};

use super::events::{EventKind, Ip12Events};
use crate::serial::SerialPort;

const BOARD_REVISION: u32 = 0x0000_8000;

mod address;
mod memory;
#[cfg(test)]
mod test_support;
mod timing;
mod wiring;

use self::address::{LOCAL_MEMORY_END, Target, route};
use self::memory::LocalMemory;
use self::wiring::{read_cpu_aux_control, write_cpu_aux_control};

pub(super) struct Ip12Bus {
    pic1: Pic1,
    memory: LocalMemory,
    hpc1: Hpc1,
    centronics: CentronicsPort,
    seeq8003: Seeq8003,
    int2: Int2,
    wd33c93b: Wd33c93b,
    scsi_bus: ScsiBus,
    pending_scsi: Option<SelectAndTransferRequest>,
    serial: [Z85230; 2],
    rtc: Dp8573a,
    mdac: Mdac,
    nvram: Nmc93cs46,
    dsp56001: Dsp56001,
    prom: Rom,
    cpu_aux_control: u8,
    events: Ip12Events,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Ip12BusSnapshot {
    pic1: Pic1,
    memory: LocalMemory,
    hpc1: Hpc1,
    centronics: CentronicsPort,
    seeq8003: Seeq8003,
    int2: Int2,
    wd33c93b: Wd33c93b,
    scsi_bus: ScsiBusSnapshot,
    pending_scsi: Option<SelectAndTransferRequest>,
    serial: [Z85230; 2],
    rtc: Dp8573a,
    mdac: Mdac,
    nvram: Nmc93cs46,
    dsp56001: Dsp56001,
    cpu_aux_control: u8,
    events: Ip12Events,
}

impl Ip12Bus {
    #[allow(clippy::too_many_arguments, reason = "the IP12 topology is fixed")]
    pub(super) fn new(
        pic1: Pic1,
        memory: [Option<Ram>; 4],
        hpc1: Hpc1,
        centronics: CentronicsPort,
        seeq8003: Seeq8003,
        int2: Int2,
        wd33c93b: Wd33c93b,
        scsi_bus: ScsiBus,
        serial: [Z85230; 2],
        rtc: Dp8573a,
        mdac: Mdac,
        nvram: Nmc93cs46,
        dsp56001: Dsp56001,
        prom: Rom,
    ) -> Self {
        let mut bus = Self {
            pic1,
            memory: LocalMemory::new(memory),
            hpc1,
            centronics,
            seeq8003,
            int2,
            wd33c93b,
            scsi_bus,
            pending_scsi: None,
            serial,
            rtc,
            mdac,
            nvram,
            dsp56001,
            prom,
            cpu_aux_control: 0,
            events: Ip12Events::new(),
        };
        bus.schedule_timed_devices();
        bus.synchronize_scsi_interrupt();
        bus.synchronize_hpc1_interrupts();
        bus
    }

    pub(super) fn snapshot(&self) -> Result<Ip12BusSnapshot, ScsiSnapshotError> {
        Ok(Ip12BusSnapshot {
            pic1: self.pic1.clone(),
            memory: self.memory.clone(),
            hpc1: self.hpc1.clone(),
            centronics: self.centronics.clone(),
            seeq8003: self.seeq8003.clone(),
            int2: self.int2.clone(),
            wd33c93b: self.wd33c93b.clone(),
            scsi_bus: self.scsi_bus.snapshot()?,
            pending_scsi: self.pending_scsi,
            serial: self.serial.clone(),
            rtc: self.rtc.clone(),
            mdac: self.mdac.clone(),
            nvram: self.nvram.clone(),
            dsp56001: self.dsp56001.clone(),
            cpu_aux_control: self.cpu_aux_control,
            events: self.events.clone(),
        })
    }

    pub(super) fn restore_snapshot(
        &mut self,
        snapshot: Ip12BusSnapshot,
    ) -> Result<(), ScsiSnapshotError> {
        self.scsi_bus.restore_snapshot(snapshot.scsi_bus)?;
        self.pic1 = snapshot.pic1;
        self.memory = snapshot.memory;
        self.hpc1 = snapshot.hpc1;
        self.centronics = snapshot.centronics;
        self.seeq8003 = snapshot.seeq8003;
        self.int2 = snapshot.int2;
        self.wd33c93b = snapshot.wd33c93b;
        self.pending_scsi = snapshot.pending_scsi;
        self.serial = snapshot.serial;
        self.rtc = snapshot.rtc;
        self.mdac = snapshot.mdac;
        self.nvram = snapshot.nvram;
        self.dsp56001 = snapshot.dsp56001;
        self.cpu_aux_control = snapshot.cpu_aux_control;
        self.events = snapshot.events;
        Ok(())
    }

    pub(super) fn reset(&mut self) {
        self.pic1.reset();
        self.hpc1.reset();
        self.centronics.reset();
        self.seeq8003.reset();
        self.wd33c93b.reset();
        self.pending_scsi = None;
        self.scsi_bus.cancel_transaction();
        for serial in &mut self.serial {
            serial.reset();
        }
        self.int2.reset();
        self.events.reset();
        self.schedule_timed_devices();
        self.synchronize_serial_interrupt();
        self.synchronize_scsi_interrupt();
        self.synchronize_hpc1_interrupts();
        self.mdac.reset();
        self.nvram.reset();
        self.cpu_aux_control = 0;
    }

    pub(super) fn nonvolatile_state(&self) -> (Nmc93cs46Contents, Dp8573aBatteryState) {
        (self.nvram.contents(), self.rtc.battery_state())
    }

    pub(super) fn restore_nonvolatile_state(
        &mut self,
        nvram: Nmc93cs46Contents,
        rtc: Dp8573aBatteryState,
        offline_milliseconds: u64,
    ) {
        self.nvram.restore_contents(nvram);
        self.rtc.restore_battery_state(rtc, offline_milliseconds);
    }

    pub(super) fn take_system_reset_request(&mut self) -> bool {
        self.pic1.take_system_reset_request()
    }

    pub(super) fn error_interrupt_asserted(&self) -> bool {
        self.pic1.error_interrupt_asserted()
    }

    pub(super) fn local_interrupt_0_asserted(&self) -> bool {
        self.int2.local_interrupt_0_asserted()
    }

    pub(super) fn local_interrupt_1_asserted(&self) -> bool {
        self.int2.local_interrupt_1_asserted()
    }

    #[cfg(test)]
    pub(super) fn set_hpc1_interrupt_levels_for_test(&mut self, parallel: bool, dsp: bool) {
        self::wiring::drive_hpc1_interrupt_inputs(&mut self.int2, parallel, dsp);
    }

    pub(super) fn timer_0_interrupt_asserted(&self) -> bool {
        self.int2.timer_0_interrupt_asserted()
    }

    pub(super) fn timer_1_interrupt_asserted(&self) -> bool {
        self.int2.timer_1_interrupt_asserted()
    }

    pub(super) fn receive_serial(&mut self, port: SerialPort, bytes: &[u8]) -> usize {
        let channel = match port {
            SerialPort::A => Channel::A,
            SerialPort::B => Channel::B,
        };
        let consumed = self.serial[1].receive(channel, bytes);
        self.synchronize_serial_interrupt();
        consumed
    }

    pub(super) fn debug_read(&self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusError> {
        if address.get() < LOCAL_MEMORY_END {
            return self.memory.read(&self.pic1, address, data);
        }

        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.read(address, data),
            Target::Hpc1(address) => self.hpc1.read(address, data),
            Target::Centronics(address) => self.centronics.read(address, data),
            Target::Seeq8003(address) => self.seeq8003.read(address, data),
            Target::Scsi(address) => self.wd33c93b.debug_read(address, data),
            Target::CpuAuxControl => read_cpu_aux_control(self.cpu_aux_control, &self.nvram, data),
            Target::Int2(address) => self.int2.debug_read(address, data),
            Target::Serial(index, address) => self
                .serial
                .get(index)
                .ok_or(BusError::HardwareFault)?
                .debug_read(address, data),
            Target::Mdac(address) => self.mdac.read(address, data),
            Target::Rtc(address) => self.rtc.debug_read(address, data),
            Target::BoardRevision => read_board_revision(data),
            Target::Dsp56001(address) => self.dsp56001.read(address, data),
            Target::Prom(address) => self.prom.read(address, data),
            Target::Gio(_address) => {
                data.fill(0);
                Ok(())
            }
        }
    }
}

impl PhysicalBus for Ip12Bus {
    fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusError> {
        if address.get() < LOCAL_MEMORY_END {
            return self.memory.read(&self.pic1, address, data);
        }

        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.read(address, data),
            Target::Hpc1(address) => {
                self.synchronize_hpc1_time();
                self.hpc1.read(address, data)
            }
            Target::Centronics(address) => self.centronics.read(address, data),
            Target::Seeq8003(address) => self.seeq8003.read(address, data),
            Target::Scsi(address) => {
                let result = self.wd33c93b.read(address, data);
                self.synchronize_scsi_interrupt();
                result
            }
            Target::CpuAuxControl => read_cpu_aux_control(self.cpu_aux_control, &self.nvram, data),
            Target::Int2(address) => {
                self.synchronize_int2_time();
                let result = self.int2.read(address, data);
                self.reschedule_int2();
                result
            }
            Target::Serial(index, address) if index < self.serial.len() => {
                self.synchronize_serial_for_mmio(index);
                let result = self.serial[index].read(address, data);
                self.synchronize_serial_interrupt();
                self.reschedule_serial(index);
                result
            }
            Target::Serial(_, _) => Err(BusError::HardwareFault),
            Target::Mdac(address) => self.mdac.read(address, data),
            Target::Rtc(address) => {
                self.synchronize_rtc_time();
                let result = self.rtc.read(address, data);
                self.events
                    .schedule(EventKind::Rtc, self.rtc.time_until_event());
                result
            }
            Target::BoardRevision => read_board_revision(data),
            Target::Dsp56001(address) => self.dsp56001.read(address, data),
            Target::Prom(address) => self.prom.read(address, data),
            Target::Gio(_address) => {
                data.fill(0);
                Ok(())
            }
        }
    }

    fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusError> {
        let result = if address.get() < LOCAL_MEMORY_END {
            self.memory.write(&self.pic1, address, data)
        } else {
            route(address, data.len()).and_then(|target| match target {
                Target::Pic1(address) => self.pic1.write(address, data),
                Target::Hpc1(address) => {
                    self.synchronize_hpc1_time();
                    self.hpc1.write(address, data)?;
                    self.handle_hpc1_outputs();
                    Ok(())
                }
                Target::Centronics(address) => self.centronics.write(address, data),
                Target::Seeq8003(address) => self.seeq8003.write(address, data),
                Target::Scsi(address) => {
                    self.wd33c93b.write(address, data)?;
                    self.handle_scsi_register_write();
                    Ok(())
                }
                Target::CpuAuxControl => {
                    write_cpu_aux_control(&mut self.cpu_aux_control, &mut self.nvram, data)
                }
                Target::Int2(address) => {
                    self.synchronize_int2_time();
                    let result = self.int2.write(address, data);
                    self.reschedule_int2();
                    result
                }
                Target::Serial(index, address) if index < self.serial.len() => {
                    self.synchronize_serial_for_mmio(index);
                    let result = self.serial[index].write(address, data);
                    self.synchronize_serial_interrupt();
                    self.reschedule_serial(index);
                    result
                }
                Target::Serial(_, address) => {
                    if data.len() == 1 && matches!(address.get(), 0x03 | 0x07 | 0x0b | 0x0f) {
                        Ok(())
                    } else {
                        Err(BusError::UnimplementedAccess)
                    }
                }
                Target::Mdac(address) => self.mdac.write(address, data),
                Target::Rtc(address) => {
                    self.synchronize_rtc_time();
                    let result = self.rtc.write(address, data);
                    self.events
                        .schedule(EventKind::Rtc, self.rtc.time_until_event());
                    result
                }
                Target::BoardRevision => Err(BusError::UnimplementedAccess),
                Target::Dsp56001(address) => self.dsp56001.write(address, data),
                Target::Prom(_) => Ok(()),
                Target::Gio(_address) => Ok(()),
            })
        };

        match result {
            Err(BusError::HardwareFault) => {
                self.pic1.report_address_error();
                Ok(())
            }
            result => result,
        }
    }
}

fn read_board_revision(data: &mut [u8]) -> Result<(), BusError> {
    if !(1..=4).contains(&data.len()) {
        return Err(BusError::InvalidTransaction);
    }
    if data.len() != 4 {
        return Err(BusError::UnimplementedAccess);
    }
    data.copy_from_slice(&BOARD_REVISION.to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, PhysAddr, PhysicalBus};

    use super::address::{
        BOARD_REVISION_BASE, CENTRONICS_EXTERNAL_BASE, CPU_AUX_CONTROL, DSP56001_BASE,
        DSP56001_END, GIO_BASE, GIO_END, HPC1_COUNTER_BASE, HPC1_DSP_INTERRUPT_MASK_BASE,
        HPC1_DSP_INTERRUPT_STATUS_BASE, HPC1_ENDIAN_CONTROL_BASE, INT2_BASE, MDAC_BASE, PIC1_BASE,
        PROM_BASE, RTC_BASE, SCSI_BASE, SERIAL_0_BASE, SERIAL_1_BASE,
    };
    use super::test_support::{
        bus, configure_memory, nvram_command, nvram_read_word, nvram_write_word, read_byte,
        read_word,
    };

    #[test]
    fn routes_every_p3_target() {
        let mut bus = bus();

        assert_eq!(read_word(&mut bus, PIC1_BASE + 4), Ok(0xf7));
        assert_eq!(read_word(&mut bus, HPC1_ENDIAN_CONTROL_BASE), Ok(0x40));
        assert_eq!(read_word(&mut bus, BOARD_REVISION_BASE), Ok(0x8000));
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0x80));
        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0b), Ok(0x04));
        assert_eq!(read_byte(&mut bus, SERIAL_1_BASE + 0x03), Ok(0x04));

        bus.write(PhysAddr::new(CPU_AUX_CONTROL), &[0xff]).unwrap();
        assert_eq!(read_byte(&mut bus, CPU_AUX_CONTROL), Ok(0x1f));
        bus.write(PhysAddr::new(INT2_BASE + 7), &[0xa5]).unwrap();
        assert_eq!(read_word(&mut bus, INT2_BASE + 4), Ok(0xa5));
        bus.write(PhysAddr::new(MDAC_BASE), &[0x5a]).unwrap();
        bus.write(PhysAddr::new(RTC_BASE + 0x57), &[0xa5]).unwrap();
        assert_eq!(read_byte(&mut bus, RTC_BASE + 0x57), Ok(0xa5));
    }

    #[test]
    fn board_revision_is_a_read_only_word() {
        let mut bus = bus();

        assert_eq!(read_word(&mut bus, BOARD_REVISION_BASE), Ok(0x8000));
        assert_eq!(
            bus.read(PhysAddr::new(BOARD_REVISION_BASE), &mut [0]),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(
            bus.write(PhysAddr::new(BOARD_REVISION_BASE), &[0; 4]),
            Err(BusError::UnimplementedAccess)
        );
    }

    #[test]
    fn prom_write_cycles_complete_without_changing_the_image() {
        let mut bus = bus();

        assert_eq!(
            bus.write(PhysAddr::new(PROM_BASE), &0xffff_ffff_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, PROM_BASE), Ok(0x0001_0203));
    }

    #[test]
    fn routes_p5_registers_and_device_windows() {
        let mut bus = bus();

        bus.write(PhysAddr::new(PIC1_BASE + 0x2_0008), &1_u32.to_be_bytes())
            .unwrap();
        bus.write(PhysAddr::new(PIC1_BASE + 0x2_000c), &0xf2_u32.to_be_bytes())
            .unwrap();
        bus.write(PhysAddr::new(DSP56001_BASE), &0xab12_3456_u32.to_be_bytes())
            .unwrap();
        bus.write(
            PhysAddr::new(DSP56001_END - 4),
            &0xcd65_4321_u32.to_be_bytes(),
        )
        .unwrap();

        assert_eq!(read_word(&mut bus, PIC1_BASE + 0x2_0008), Ok(1));
        assert_eq!(read_word(&mut bus, PIC1_BASE + 0x2_000c), Ok(0xf2));
        assert_eq!(read_word(&mut bus, DSP56001_BASE), Ok(0x0012_3456));
        assert_eq!(read_word(&mut bus, DSP56001_END - 4), Ok(0x0065_4321));
        assert_eq!(read_word(&mut bus, GIO_BASE), Ok(0));
        assert_eq!(
            bus.write(PhysAddr::new(GIO_END - 4), &0x1234_5678_u32.to_be_bytes()),
            Ok(())
        );
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn headless_gio_accesses_preserve_the_error_latch() {
        let mut bus = bus();
        for pending in [false, true] {
            if pending {
                bus.write(PhysAddr::new(0x1fb0_0010), &[0]).unwrap();
            }
            for length in 1..=4 {
                for address in [GIO_BASE, GIO_END - length as u64] {
                    let mut bytes = [0xff; 4];
                    bus.debug_read(PhysAddr::new(address), &mut bytes[..length])
                        .unwrap();
                    assert_eq!(&bytes[..length], &vec![0; length]);
                    assert_eq!(bus.error_interrupt_asserted(), pending);

                    bus.read(PhysAddr::new(address), &mut bytes[..length])
                        .unwrap();
                    assert_eq!(bus.error_interrupt_asserted(), pending);
                    assert_eq!(&bytes[..length], &vec![0; length]);

                    bus.write(PhysAddr::new(address), &bytes[..length]).unwrap();
                    assert_eq!(bus.error_interrupt_asserted(), pending);
                }
            }
        }
    }

    #[test]
    fn model_errors_are_not_completed_as_hardware_write_errors() {
        let mut bus = bus();
        for (address, length) in [
            (PIC1_BASE + 0x100, 4),
            (PIC1_BASE + 0x1_0000, 1),
            (HPC1_COUNTER_BASE, 4),
            (SERIAL_1_BASE + 0x0b, 2),
            (BOARD_REVISION_BASE, 4),
        ] {
            assert_eq!(
                bus.write(PhysAddr::new(address), &[0xa5; 4][..length]),
                Err(BusError::UnimplementedAccess)
            );
            assert!(!bus.error_interrupt_asserted());
        }
        assert_eq!(read_word(&mut bus, PIC1_BASE + 0x1_0000), Ok(0));
        let mut bytes = [0xa5; 4];
        assert_eq!(
            bus.read(PhysAddr::new(PIC1_BASE + 0x100), &mut bytes),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(bytes, [0xa5; 4]);
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn a_selected_target_hardware_write_error_uses_board_completion() {
        let mut bus = bus();
        let address = PhysAddr::new(PIC1_BASE + 3);
        let before = read_word(&mut bus, PIC1_BASE).unwrap();
        assert_eq!(bus.read(address, &mut [0; 2]), Err(BusError::HardwareFault));
        assert!(!bus.error_interrupt_asserted());
        assert_eq!(bus.write(address, &[0xaa, 0xbb]), Ok(()));
        assert!(bus.error_interrupt_asserted());
        assert_eq!(read_word(&mut bus, PIC1_BASE), Ok(before));
    }

    #[test]
    fn reset_restores_volatile_devices_but_preserves_rtc_ram_and_prom() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x0100_023f, 0x023f_023f);
        bus.write(
            PhysAddr::new(6 * 1024 * 1024),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(PhysAddr::new(12 * 1024 * 1024), &[0]).unwrap();
        assert!(bus.error_interrupt_asserted());
        bus.write(
            PhysAddr::new(PIC1_BASE + 0xa_0000),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(PhysAddr::new(HPC1_ENDIAN_CONTROL_BASE + 3), &[0x1f])
            .unwrap();
        bus.write(
            PhysAddr::new(HPC1_DSP_INTERRUPT_MASK_BASE),
            &7_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(PhysAddr::new(CENTRONICS_EXTERNAL_BASE + 1), &[3])
            .unwrap();
        bus.write(PhysAddr::new(INT2_BASE + 7), &[0xa5]).unwrap();
        bus.write(PhysAddr::new(INT2_BASE + 0x0f), &[1 << 4])
            .unwrap();
        bus.write(PhysAddr::new(CPU_AUX_CONTROL), &[0x0f]).unwrap();
        bus.write(PhysAddr::new(RTC_BASE + 0x57), &[0xa5]).unwrap();
        nvram_command(&mut bus, 0x04c0);
        nvram_write_word(&mut bus, 7, 0x5aa5);
        bus.write(
            PhysAddr::new(DSP56001_BASE + 4),
            &0x0012_3456_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(PhysAddr::new(PIC1_BASE + 0x2_000b), &[1])
            .unwrap();
        bus.write(PhysAddr::new(PIC1_BASE + 0x2_000f), &[0xf2])
            .unwrap();

        bus.reset();

        assert_eq!(read_word(&mut bus, PIC1_BASE + 0x1_0000), Ok(0));
        assert!(!bus.error_interrupt_asserted());
        configure_memory(&mut bus, 0x0100_023f, 0x023f_023f);
        assert_eq!(read_word(&mut bus, 6 * 1024 * 1024), Ok(0x0123_4567));
        assert_eq!(read_word(&mut bus, PIC1_BASE + 0xa_0000), Ok(0));
        assert_eq!(read_word(&mut bus, HPC1_ENDIAN_CONTROL_BASE), Ok(0x40));
        assert_eq!(read_word(&mut bus, HPC1_DSP_INTERRUPT_STATUS_BASE), Ok(0));
        assert_eq!(read_word(&mut bus, HPC1_DSP_INTERRUPT_MASK_BASE), Ok(0));
        assert_eq!(read_byte(&mut bus, CENTRONICS_EXTERNAL_BASE + 1), Ok(0));
        assert_eq!(read_word(&mut bus, INT2_BASE + 4), Ok(0));
        assert_eq!(read_word(&mut bus, INT2_BASE + 0x0c), Ok(0));
        assert_eq!(read_byte(&mut bus, CPU_AUX_CONTROL), Ok(0));
        assert_eq!(read_byte(&mut bus, RTC_BASE + 0x57), Ok(0xa5));
        assert_eq!(nvram_read_word(&mut bus, 7), 0x5aa5);
        assert_eq!(read_word(&mut bus, DSP56001_BASE + 4), Ok(0x0012_3456));
        assert_eq!(read_word(&mut bus, PIC1_BASE + 0x2_0008), Ok(0));
        assert_eq!(read_word(&mut bus, PIC1_BASE + 0x2_000c), Ok(0));
        let mut prom = [0; 4];
        assert_eq!(bus.read(PhysAddr::new(PROM_BASE), &mut prom), Ok(()));
        assert_eq!(prom, [0, 1, 2, 3]);
    }
}
