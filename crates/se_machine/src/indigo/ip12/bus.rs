use se_core::bus::{BusFault, DeviceAddr, PhysAddr, PhysicalBus};
use se_device::dp8573a::{Dp8573a, Dp8573aBatteryState};
use se_device::dsp56001::Dsp56001;
use se_device::hpc1::Hpc1;
use se_device::int2::Int2;
use se_device::mdac::Mdac;
use se_device::nmc93cs46::{Nmc93cs46, Nmc93cs46Contents};
use se_device::pic1::Pic1;
use se_device::ram::Ram;
use se_device::rom::Rom;
use se_device::scsi::ScsiBus;
use se_device::wd33c93b::{SelectAndTransferRequest, Wd33c93b};
use se_device::z85230::{Channel, Z85230};

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

impl Ip12Bus {
    #[allow(clippy::too_many_arguments, reason = "the IP12 topology is fixed")]
    pub(super) fn new(
        pic1: Pic1,
        memory: [Option<Ram>; 4],
        hpc1: Hpc1,
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
        bus
    }

    pub(super) fn reset(&mut self) {
        self.pic1.reset();
        self.hpc1.reset();
        self.int2.reset();
        self.wd33c93b.reset();
        self.pending_scsi = None;
        self.scsi_bus.cancel_transaction();
        for serial in &mut self.serial {
            serial.reset();
        }
        self.events.reset();
        self.schedule_timed_devices();
        self.synchronize_serial_interrupt();
        self.synchronize_scsi_interrupt();
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

    pub(super) fn receive_serial(&mut self, port: SerialPort, bytes: &[u8]) -> usize {
        let channel = match port {
            SerialPort::A => Channel::A,
            SerialPort::B => Channel::B,
        };
        let consumed = self.serial[1].receive(channel, bytes);
        self.synchronize_serial_interrupt();
        consumed
    }

    pub(super) fn debug_read(&self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        if address.get() < LOCAL_MEMORY_END {
            return self.memory.read(&self.pic1, address, data);
        }

        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.read(address, data),
            Target::Hpc1(address) => self.hpc1.read(address, data),
            Target::Scsi(address) => self.wd33c93b.debug_read(address, data),
            Target::CpuAuxControl => read_cpu_aux_control(self.cpu_aux_control, &self.nvram, data),
            Target::Int2(address) => self.int2.debug_read(address, data),
            Target::Serial(index, address) => self.serial[index].debug_read(address, data),
            Target::UnpopulatedSerial(_) => Err(BusFault::Unmapped),
            Target::Mdac(address) => self.mdac.read(address, data),
            Target::Rtc(address) => self.rtc.debug_read(address, data),
            Target::BoardRevision => read_board_revision(data),
            Target::Dsp56001(address) => self.dsp56001.read(address, data),
            Target::Prom(address) => self.prom.read(address, data),
            Target::UnpopulatedGio => read_unpopulated_gio(data),
        }
    }
}

impl PhysicalBus for Ip12Bus {
    fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        if address.get() < LOCAL_MEMORY_END {
            return self.memory.read(&self.pic1, address, data);
        }

        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.read(address, data),
            Target::Hpc1(address) => self.hpc1.read(address, data),
            Target::Scsi(address) => {
                let result = self.wd33c93b.read(address, data);
                self.synchronize_scsi_interrupt();
                result
            }
            Target::CpuAuxControl => read_cpu_aux_control(self.cpu_aux_control, &self.nvram, data),
            Target::Int2(address) => {
                self.synchronize_int2_time();
                self.int2.read(address, data)
            }
            Target::Serial(index, address) => {
                self.synchronize_serial_for_mmio(index);
                let result = self.serial[index].read(address, data);
                self.synchronize_serial_interrupt();
                self.reschedule_serial(index);
                result
            }
            Target::UnpopulatedSerial(_) => Err(BusFault::Unmapped),
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
            Target::UnpopulatedGio => read_unpopulated_gio(data),
        }
    }

    fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
        if address.get() < LOCAL_MEMORY_END {
            return self.memory.write(&mut self.pic1, address, data);
        }

        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.write(address, data),
            Target::Hpc1(address) => {
                self.hpc1.write(address, data)?;
                self.handle_hpc_scsi_state();
                Ok(())
            }
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
                self.int2.write(address, data)
            }
            Target::Serial(index, address) => {
                self.synchronize_serial_for_mmio(index);
                let result = self.serial[index].write(address, data);
                self.synchronize_serial_interrupt();
                self.reschedule_serial(index);
                result
            }
            Target::UnpopulatedSerial(address) => write_unpopulated_serial(address, data),
            Target::Mdac(address) => self.mdac.write(address, data),
            Target::Rtc(address) => {
                self.synchronize_rtc_time();
                let result = self.rtc.write(address, data);
                self.events
                    .schedule(EventKind::Rtc, self.rtc.time_until_event());
                result
            }
            Target::BoardRevision => Err(BusFault::UnsupportedAccess),
            Target::Dsp56001(address) => self.dsp56001.write(address, data),
            Target::Prom(_) => Ok(()),
            Target::UnpopulatedGio => Ok(()),
        }
    }
}

fn read_board_revision(data: &mut [u8]) -> Result<(), BusFault> {
    if data.len() != 4 {
        return Err(BusFault::UnsupportedAccess);
    }
    data.copy_from_slice(&BOARD_REVISION.to_be_bytes());
    Ok(())
}

fn read_unpopulated_gio(data: &mut [u8]) -> Result<(), BusFault> {
    data.fill(0);
    Ok(())
}

fn write_unpopulated_serial(address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
    if data.len() != 1 {
        return Err(BusFault::UnsupportedAccess);
    }

    match address.get() {
        0x03 | 0x07 | 0x0b | 0x0f => Ok(()),
        _ => Err(BusFault::Unmapped),
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

    use super::address::{
        BOARD_REVISION_BASE, CPU_AUX_CONTROL, DSP56001_BASE, DSP56001_END, GIO_BASE, GIO_END,
        HPC1_ENDIAN_CONTROL_BASE, INT2_BASE, MDAC_BASE, PIC1_BASE, PROM_BASE, RTC_BASE, SCSI_BASE,
        SERIAL_0_BASE, SERIAL_1_BASE,
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
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            bus.write(PhysAddr::new(BOARD_REVISION_BASE), &[0; 4]),
            Err(BusFault::UnsupportedAccess)
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
        bus.write(PhysAddr::new(INT2_BASE + 7), &[0xa5]).unwrap();
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
        assert_eq!(read_word(&mut bus, INT2_BASE + 4), Ok(0));
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
