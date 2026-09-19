use std::io;

use se_core::bus::{BusError, PhysAddr, PhysicalBus};
use se_core::storage::StorageMedium;
use se_device::centronics::CentronicsPort;
use se_device::dp8573a::Dp8573a;
use se_device::dsp56001::Dsp56001;
use se_device::gio::GioBus;
use se_device::hpc1::Hpc1;
use se_device::int2::Int2;
use se_device::mdac::Mdac;
use se_device::nmc93cs46::Nmc93cs46;
use se_device::pic1::Pic1;
use se_device::ram::Ram;
use se_device::rom::Rom;
use se_device::scsi::ScsiBus;
use se_device::scsi_cdrom::ScsiCdrom;
use se_device::scsi_disk::ScsiDisk;
use se_device::seeq8003::Seeq8003;
use se_device::sgi_keyboard::SgiKeyboard;
use se_device::sgi_mouse::SgiMouse;
use se_device::wd33c93b::Wd33c93b;
use se_device::z85230::Z85230;

use super::super::PROM_BYTES;
use super::Ip12Bus;
use super::address::{CPU_AUX_CONTROL, PIC1_BASE};

pub(super) fn bus() -> Ip12Bus {
    bus_with_memory([Some(Ram::new(8 * 1024 * 1024)), None, None, None])
}

pub(super) fn bus_with_memory(memory: [Option<Ram>; 4]) -> Ip12Bus {
    bus_with_memory_and_gio(memory, GioBus::new())
}

pub(super) fn bus_with_gio(gio: GioBus) -> Ip12Bus {
    bus_with_memory_and_gio([Some(Ram::new(8 * 1024 * 1024)), None, None, None], gio)
}

fn bus_with_memory_and_gio(memory: [Option<Ram>; 4], gio: GioBus) -> Ip12Bus {
    let bytes = (0..PROM_BYTES).map(|index| index as u8).collect();
    Ip12Bus::new(
        Pic1::new(0xf7, 2, true),
        memory,
        Hpc1::new(),
        CentronicsPort::new(),
        Seeq8003::new(),
        Int2::new(),
        Wd33c93b::new(super::super::SCSI_CLOCK_HZ),
        ScsiBus::new(),
        [Z85230::new(3_686_400), Z85230::new(3_686_400)],
        Some(SgiKeyboard::new()),
        Some(SgiMouse::new()),
        Dp8573a::new(),
        Mdac::new(),
        Nmc93cs46::new(),
        Dsp56001::new(),
        Rom::new(bytes),
        gio,
    )
}

struct MemoryStorage {
    bytes: Vec<u8>,
    fail_reads: bool,
    fail_writes: bool,
    writable: bool,
}

impl StorageMedium for MemoryStorage {
    fn size_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        if self.fail_reads {
            return Err(io::Error::other("injected storage failure"));
        }
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::other("storage offset does not fit usize"))?;
        let end = start
            .checked_add(buffer.len())
            .ok_or_else(|| io::Error::other("storage range overflow"))?;
        let source = self
            .bytes
            .get(start..end)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short storage"))?;
        buffer.copy_from_slice(source);
        Ok(())
    }

    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        if !self.writable {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "storage is read-only",
            ));
        }
        if self.fail_writes {
            return Err(io::Error::other("injected storage failure"));
        }
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::other("storage offset does not fit usize"))?;
        let end = start
            .checked_add(data.len())
            .ok_or_else(|| io::Error::other("storage range overflow"))?;
        let destination = self
            .bytes
            .get_mut(start..end)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short storage"))?;
        destination.copy_from_slice(data);
        Ok(())
    }
}

pub(super) fn bus_with_disk(bytes: Vec<u8>, fail_reads: bool) -> Ip12Bus {
    bus_with_disk_failures(bytes, fail_reads, false)
}

pub(super) fn bus_with_disk_failures(
    bytes: Vec<u8>,
    fail_reads: bool,
    fail_writes: bool,
) -> Ip12Bus {
    let storage_bytes = bytes.len() as u64;
    let mut scsi_bus = ScsiBus::new();
    scsi_bus
        .attach(
            1,
            0,
            Box::new(ScsiDisk::try_new(storage_bytes).unwrap()),
            Box::new(MemoryStorage {
                bytes,
                fail_reads,
                fail_writes,
                writable: true,
            }),
        )
        .unwrap();
    let prom = (0..PROM_BYTES).map(|index| index as u8).collect();
    Ip12Bus::new(
        Pic1::new(0xf7, 2, true),
        [Some(Ram::new(8 * 1024 * 1024)), None, None, None],
        Hpc1::new(),
        CentronicsPort::new(),
        Seeq8003::new(),
        Int2::new(),
        Wd33c93b::new(super::super::SCSI_CLOCK_HZ),
        scsi_bus,
        [Z85230::new(3_686_400), Z85230::new(3_686_400)],
        Some(SgiKeyboard::new()),
        Some(SgiMouse::new()),
        Dp8573a::new(),
        Mdac::new(),
        Nmc93cs46::new(),
        Dsp56001::new(),
        Rom::new(prom),
        GioBus::new(),
    )
}

pub(super) fn bus_with_cdrom(bytes: Vec<u8>, fail_reads: bool) -> Ip12Bus {
    let storage_bytes = bytes.len() as u64;
    let mut scsi_bus = ScsiBus::new();
    scsi_bus
        .attach(
            4,
            0,
            Box::new(ScsiCdrom::try_new(storage_bytes).unwrap()),
            Box::new(MemoryStorage {
                bytes,
                fail_reads,
                fail_writes: false,
                writable: false,
            }),
        )
        .unwrap();
    let prom = (0..PROM_BYTES).map(|index| index as u8).collect();
    Ip12Bus::new(
        Pic1::new(0xf7, 2, true),
        [Some(Ram::new(8 * 1024 * 1024)), None, None, None],
        Hpc1::new(),
        CentronicsPort::new(),
        Seeq8003::new(),
        Int2::new(),
        Wd33c93b::new(super::super::SCSI_CLOCK_HZ),
        scsi_bus,
        [Z85230::new(3_686_400), Z85230::new(3_686_400)],
        Some(SgiKeyboard::new()),
        Some(SgiMouse::new()),
        Dp8573a::new(),
        Mdac::new(),
        Nmc93cs46::new(),
        Dsp56001::new(),
        Rom::new(prom),
        GioBus::new(),
    )
}

pub(super) fn bus_with_disk_and_cdrom(disk_bytes: Vec<u8>, cdrom_bytes: Vec<u8>) -> Ip12Bus {
    let disk_storage_bytes = disk_bytes.len() as u64;
    let cdrom_storage_bytes = cdrom_bytes.len() as u64;
    let mut scsi_bus = ScsiBus::new();
    scsi_bus
        .attach(
            1,
            0,
            Box::new(ScsiDisk::try_new(disk_storage_bytes).unwrap()),
            Box::new(MemoryStorage {
                bytes: disk_bytes,
                fail_reads: false,
                fail_writes: false,
                writable: true,
            }),
        )
        .unwrap();
    scsi_bus
        .attach(
            4,
            0,
            Box::new(ScsiCdrom::try_new(cdrom_storage_bytes).unwrap()),
            Box::new(MemoryStorage {
                bytes: cdrom_bytes,
                fail_reads: false,
                fail_writes: false,
                writable: false,
            }),
        )
        .unwrap();
    let prom = (0..PROM_BYTES).map(|index| index as u8).collect();
    Ip12Bus::new(
        Pic1::new(0xf7, 2, true),
        [Some(Ram::new(8 * 1024 * 1024)), None, None, None],
        Hpc1::new(),
        CentronicsPort::new(),
        Seeq8003::new(),
        Int2::new(),
        Wd33c93b::new(super::super::SCSI_CLOCK_HZ),
        scsi_bus,
        [Z85230::new(3_686_400), Z85230::new(3_686_400)],
        Some(SgiKeyboard::new()),
        Some(SgiMouse::new()),
        Dp8573a::new(),
        Mdac::new(),
        Nmc93cs46::new(),
        Dsp56001::new(),
        Rom::new(prom),
        GioBus::new(),
    )
}

pub(super) fn read_word(bus: &mut Ip12Bus, address: u64) -> Result<u32, BusError> {
    let mut bytes = [0; 4];
    bus.read(PhysAddr::new(address), &mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

pub(super) fn read_byte(bus: &mut Ip12Bus, address: u64) -> Result<u8, BusError> {
    let mut byte = [0];
    bus.read(PhysAddr::new(address), &mut byte)?;
    Ok(byte[0])
}

pub(super) fn configure_memory(bus: &mut Ip12Bus, configuration_0: u32, configuration_1: u32) {
    bus.write(
        PhysAddr::new(PIC1_BASE + 0x1_0000),
        &configuration_0.to_be_bytes(),
    )
    .unwrap();
    bus.write(
        PhysAddr::new(PIC1_BASE + 0x1_0004),
        &configuration_1.to_be_bytes(),
    )
    .unwrap();
}

pub(super) fn write_serial_register(bus: &mut Ip12Bus, base: u64, control: u8, value: u8) {
    bus.write(PhysAddr::new(base + 0x0b), &[control]).unwrap();
    bus.write(PhysAddr::new(base + 0x0b), &[value]).unwrap();
}

pub(super) fn configure_serial_a(bus: &mut Ip12Bus, base: u64) {
    for (register, value) in [(4, 0x44), (11, 0x10), (12, 10), (13, 0), (14, 1), (5, 0x68)] {
        write_serial_register(bus, base, register, value);
    }
}

pub(super) fn nvram_clock_bit(bus: &mut Ip12Bus, bit: bool) -> bool {
    let value = 0x02 | u8::from(bit) << 3;
    bus.write(PhysAddr::new(CPU_AUX_CONTROL), &[value]).unwrap();
    bus.write(PhysAddr::new(CPU_AUX_CONTROL), &[value | 0x04])
        .unwrap();
    read_byte(bus, CPU_AUX_CONTROL).unwrap() & 0x10 != 0
}

pub(super) fn nvram_shift_command(bus: &mut Ip12Bus, command: u16) {
    for bit in (0..11).rev() {
        nvram_clock_bit(bus, command & (1 << bit) != 0);
    }
}

pub(super) fn nvram_deselect(bus: &mut Ip12Bus) {
    bus.write(PhysAddr::new(CPU_AUX_CONTROL), &[0]).unwrap();
}

pub(super) fn nvram_command(bus: &mut Ip12Bus, command: u16) {
    nvram_deselect(bus);
    nvram_shift_command(bus, command);
    nvram_deselect(bus);
}

pub(super) fn nvram_write_word(bus: &mut Ip12Bus, address: u16, value: u16) {
    nvram_deselect(bus);
    nvram_shift_command(bus, 0x0500 | address);
    for bit in (0..16).rev() {
        nvram_clock_bit(bus, value & (1 << bit) != 0);
    }
    nvram_deselect(bus);
}

pub(super) fn nvram_read_word(bus: &mut Ip12Bus, address: u16) -> u16 {
    nvram_deselect(bus);
    nvram_shift_command(bus, 0x0600 | address);
    let mut value = 0;
    for _ in 0..16 {
        value = value << 1 | u16::from(nvram_clock_bit(bus, false));
    }
    nvram_deselect(bus);
    value
}
