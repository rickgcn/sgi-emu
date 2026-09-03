use std::io;

use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
use se_device::centronics::CentronicsPort;
use se_device::dp8573a::Dp8573a;
use se_device::dsp56001::Dsp56001;
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
use se_device::storage::BlockStorage;
use se_device::wd33c93b::Wd33c93b;
use se_device::z85230::Z85230;

use super::super::PROM_BYTES;
use super::Ip12Bus;
use super::address::{CPU_AUX_CONTROL, HPC1_SCSI_REGISTERS_BASE, PIC1_BASE, SCSI_BASE};

pub(super) fn bus() -> Ip12Bus {
    bus_with_memory([Some(Ram::new(8 * 1024 * 1024)), None, None, None])
}

pub(super) fn bus_with_memory(memory: [Option<Ram>; 4]) -> Ip12Bus {
    let bytes = (0..PROM_BYTES).map(|index| index as u8).collect();
    Ip12Bus::new(
        Pic1::new(0xf7, 2, true),
        memory,
        Hpc1::new(),
        CentronicsPort::new(),
        Seeq8003::new(),
        Int2::new(),
        Wd33c93b::new(),
        ScsiBus::new(),
        [Z85230::new(3_686_400), Z85230::new(3_686_400)],
        Dp8573a::new(),
        Mdac::new(),
        Nmc93cs46::new(),
        Dsp56001::new(),
        Rom::new(bytes),
    )
}

struct MemoryStorage {
    bytes: Vec<u8>,
    fail_reads: bool,
    fail_writes: bool,
    writable: bool,
}

impl BlockStorage for MemoryStorage {
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
        Wd33c93b::new(),
        scsi_bus,
        [Z85230::new(3_686_400), Z85230::new(3_686_400)],
        Dp8573a::new(),
        Mdac::new(),
        Nmc93cs46::new(),
        Dsp56001::new(),
        Rom::new(prom),
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
        Wd33c93b::new(),
        scsi_bus,
        [Z85230::new(3_686_400), Z85230::new(3_686_400)],
        Dp8573a::new(),
        Mdac::new(),
        Nmc93cs46::new(),
        Dsp56001::new(),
        Rom::new(prom),
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
        Wd33c93b::new(),
        scsi_bus,
        [Z85230::new(3_686_400), Z85230::new(3_686_400)],
        Dp8573a::new(),
        Mdac::new(),
        Nmc93cs46::new(),
        Dsp56001::new(),
        Rom::new(prom),
    )
}

pub(super) fn read_word(bus: &mut Ip12Bus, address: u64) -> Result<u32, BusFault> {
    let mut bytes = [0; 4];
    bus.read(PhysAddr::new(address), &mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

pub(super) fn read_byte(bus: &mut Ip12Bus, address: u64) -> Result<u8, BusFault> {
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

pub(super) fn write_scsi_register(bus: &mut Ip12Bus, register: u8, value: u8) {
    bus.write(PhysAddr::new(SCSI_BASE), &[register]).unwrap();
    bus.write(PhysAddr::new(SCSI_BASE + 4), &[value]).unwrap();
}

pub(super) fn read_scsi_register(bus: &mut Ip12Bus, register: u8) -> u8 {
    bus.write(PhysAddr::new(SCSI_BASE), &[register]).unwrap();
    read_byte(bus, SCSI_BASE + 4).unwrap()
}

pub(super) fn configure_single_scsi_descriptor(bus: &mut Ip12Bus, buffer_address: u32) {
    configure_scsi_descriptor_chain(bus, 0x1000, buffer_address, &[512]);
}

pub(super) fn configure_single_scsi_write_descriptor(bus: &mut Ip12Bus, buffer_address: u32) {
    configure_scsi_write_descriptor_chain(bus, 0x1000, buffer_address, &[512]);
}

pub(super) fn configure_scsi_descriptor_chain(
    bus: &mut Ip12Bus,
    first_descriptor_address: u32,
    first_buffer_address: u32,
    byte_counts: &[u16],
) {
    configure_scsi_descriptor_chain_direction(
        bus,
        first_descriptor_address,
        first_buffer_address,
        byte_counts,
        true,
    );
}

pub(super) fn configure_scsi_write_descriptor_chain(
    bus: &mut Ip12Bus,
    first_descriptor_address: u32,
    first_buffer_address: u32,
    byte_counts: &[u16],
) {
    configure_scsi_descriptor_chain_direction(
        bus,
        first_descriptor_address,
        first_buffer_address,
        byte_counts,
        false,
    );
}

fn configure_scsi_descriptor_chain_direction(
    bus: &mut Ip12Bus,
    first_descriptor_address: u32,
    first_buffer_address: u32,
    byte_counts: &[u16],
    to_memory: bool,
) {
    configure_memory(bus, 0x0100_023f, 0x023f_023f);
    let mut descriptor_address = first_descriptor_address;
    let mut buffer_address = first_buffer_address;
    for (index, byte_count) in byte_counts.iter().copied().enumerate() {
        let is_last = index + 1 == byte_counts.len();
        let buffer_word = if is_last {
            buffer_address | (1 << 31)
        } else {
            buffer_address
        };
        for (address, value) in [
            (descriptor_address, u32::from(byte_count)),
            (descriptor_address + 4, buffer_word),
            (descriptor_address + 8, descriptor_address + 12),
        ] {
            bus.write(PhysAddr::new(u64::from(address)), &value.to_be_bytes())
                .unwrap();
        }
        descriptor_address += 12;
        buffer_address += u32::from(byte_count);
    }
    bus.write(
        PhysAddr::new(HPC1_SCSI_REGISTERS_BASE + 8),
        &first_descriptor_address.to_be_bytes(),
    )
    .unwrap();
    let control = 0x80 | if to_memory { 0x10 } else { 0 };
    bus.write(PhysAddr::new(HPC1_SCSI_REGISTERS_BASE + 0x0f), &[control])
        .unwrap();
}

pub(super) fn issue_scsi_command(
    bus: &mut Ip12Bus,
    target: u8,
    lun: u8,
    transfer_count: u32,
    cdb: &[u8],
) {
    write_scsi_register(bus, 0x15, target);
    write_scsi_register(bus, 0x0f, lun);
    for (register, value) in [
        (0x12, (transfer_count >> 16) as u8),
        (0x13, (transfer_count >> 8) as u8),
        (0x14, transfer_count as u8),
    ] {
        write_scsi_register(bus, register, value);
    }
    for (offset, value) in cdb.iter().copied().enumerate() {
        write_scsi_register(bus, 0x03 + offset as u8, value);
    }
    write_scsi_register(bus, 0x18, 0x09);
}

pub(super) fn issue_read_ten(bus: &mut Ip12Bus, target: u8, lba: u32) {
    let mut cdb = [0; 10];
    cdb[0] = 0x28;
    cdb[2..6].copy_from_slice(&lba.to_be_bytes());
    cdb[8] = 1;
    issue_scsi_command(bus, target, 0, 512, &cdb);
}

pub(super) fn issue_write_ten(bus: &mut Ip12Bus, target: u8, lba: u32) {
    let mut cdb = [0; 10];
    cdb[0] = 0x2a;
    cdb[2..6].copy_from_slice(&lba.to_be_bytes());
    cdb[8] = 1;
    issue_scsi_command(bus, target, 0, 512, &cdb);
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
