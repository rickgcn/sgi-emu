use se_core::bus::{BusFault, DeviceAddr, PhysAddr, PhysicalBus};
use se_core::time::VirtualDuration;
use se_device::dp8573a::Dp8573a;
use se_device::dsp56001::Dsp56001;
use se_device::hpc1::Hpc1;
use se_device::int2::Int2;
use se_device::mdac::Mdac;
use se_device::nmc93cs46::Nmc93cs46;
use se_device::pic1::Pic1;
use se_device::ram::Ram;
use se_device::rom::Rom;
use se_device::wd33c93b::Wd33c93b;
use se_device::z85230::{Channel, Z85230};

use crate::output::MachineOutput;
use crate::serial::SerialPort;

use super::PROM_BYTES;

const LOCAL_MEMORY_END: u64 = 0x1000_0000;

const GIO_BASE: u64 = 0x1f00_0000;
const GIO_END: u64 = 0x1f40_0000;

const PIC1_BASE: u64 = 0x1fa0_0000;
const PIC1_END: u64 = 0x1fab_0000;

const HPC1_BASE: u64 = 0x1fb8_0000;
const HPC1_ETHERNET_STATUS_BASE: u64 = 0x1fb8_0034;
const HPC1_ETHERNET_STATUS_END: u64 = 0x1fb8_0040;
const HPC1_ETHERNET_POINTER_BASE: u64 = 0x1fb8_0058;
const HPC1_ETHERNET_POINTER_END: u64 = 0x1fb8_005c;
const HPC1_ETHERNET_FIFO_BASE: u64 = 0x1fb8_005c;
const HPC1_ETHERNET_FIFO_END: u64 = 0x1fb8_0060;
const HPC1_SCSI_CONTROL_BASE: u64 = 0x1fb8_0094;
const HPC1_SCSI_CONTROL_END: u64 = 0x1fb8_0098;
const HPC1_ENDIAN_CONTROL_BASE: u64 = 0x1fb8_00c0;
const HPC1_ENDIAN_CONTROL_END: u64 = 0x1fb8_00c4;
const HPC1_MISCELLANEOUS_CONTROL_BASE: u64 = 0x1fb8_01b0;
const HPC1_MISCELLANEOUS_CONTROL_END: u64 = 0x1fb8_01b4;

const SCSI_BASE: u64 = 0x1fb8_0122;
const SCSI_END: u64 = 0x1fb8_0127;
const CPU_AUX_CONTROL: u64 = 0x1fb8_01bf;
const CPU_AUX_OUTPUT_BITS: u8 = 0x0f;
const INT2_BASE: u64 = 0x1fb8_01c0;
const INT2_END: u64 = 0x1fb8_0200;
const SERIAL_INTERRUPT: u8 = 1 << 5;

const SERIAL_0_BASE: u64 = 0x1fb8_0d00;
const SERIAL_0_END: u64 = 0x1fb8_0d10;
const SERIAL_1_BASE: u64 = 0x1fb8_0d10;
const SERIAL_1_END: u64 = 0x1fb8_0d20;
const SERIAL_2_BASE: u64 = 0x1fb8_0d20;
const SERIAL_2_END: u64 = 0x1fb8_0d30;
const MDAC_BASE: u64 = 0x1fb8_0d33;
const MDAC_END: u64 = 0x1fb8_0d38;
const RTC_BASE: u64 = 0x1fb8_0e00;
const RTC_END: u64 = 0x1fb8_0e80;

const BOARD_REVISION_BASE: u64 = 0x1fbd_0000;
const BOARD_REVISION_END: u64 = 0x1fbd_0004;
const BOARD_REVISION: u32 = 0x0000_8000;

const DSP56001_BASE: u64 = 0x1fbe_0000;
const DSP56001_END: u64 = 0x1fc0_0000;

const PROM_BASE: u64 = 0x1fc0_0000;
const PROM_END: u64 = PROM_BASE + PROM_BYTES as u64;

pub(super) struct Ip12Bus {
    pic1: Pic1,
    memory: [Option<Ram>; 4],
    hpc1: Hpc1,
    int2: Int2,
    scsi: Wd33c93b,
    serial: [Z85230; 2],
    rtc: Dp8573a,
    mdac: Mdac,
    nvram: Nmc93cs46,
    dsp56001: Dsp56001,
    prom: Rom,
    cpu_aux_control: u8,
}

impl Ip12Bus {
    #[allow(clippy::too_many_arguments, reason = "the IP12 topology is fixed")]
    pub(super) const fn new(
        pic1: Pic1,
        memory: [Option<Ram>; 4],
        hpc1: Hpc1,
        int2: Int2,
        scsi: Wd33c93b,
        serial: [Z85230; 2],
        rtc: Dp8573a,
        mdac: Mdac,
        nvram: Nmc93cs46,
        dsp56001: Dsp56001,
        prom: Rom,
    ) -> Self {
        Self {
            pic1,
            memory,
            hpc1,
            int2,
            scsi,
            serial,
            rtc,
            mdac,
            nvram,
            dsp56001,
            prom,
            cpu_aux_control: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.pic1.reset();
        self.hpc1.reset();
        self.int2.reset();
        self.scsi.reset();
        for serial in &mut self.serial {
            serial.reset();
        }
        self.synchronize_serial_interrupt();
        self.mdac.reset();
        self.nvram.reset();
        self.cpu_aux_control = 0;
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

    pub(super) fn advance_time(&mut self, elapsed: VirtualDuration, output: &mut MachineOutput) {
        self.int2.advance_time(elapsed);
        self.rtc.advance_time(elapsed);
        self.serial[0].advance_time(elapsed, |_, _| {});
        self.serial[1].advance_time(elapsed, |channel, value| {
            let port = match channel {
                Channel::A => SerialPort::A,
                Channel::B => SerialPort::B,
            };
            output.push_serial(port, value);
        });
    }

    fn synchronize_serial_interrupt(&mut self) {
        let asserted = self.serial.iter().any(Z85230::interrupt_asserted);
        self.int2
            .set_local_interrupt_0_input(SERIAL_INTERRUPT, asserted);
    }

    pub(super) fn debug_read(&self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        if address.get() < LOCAL_MEMORY_END {
            return self.read_memory(address, data);
        }

        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.read(address, data),
            Target::Hpc1(address) => self.hpc1.read(address, data),
            Target::Scsi(address) => self.scsi.debug_read(address, data),
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

    fn read_memory(&self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        if !local_memory_transaction_is_contained(address, data.len())? {
            return Err(BusFault::Unmapped);
        }

        let Some((index, offset)) = self.pic1.decode_memory(address, data.len())? else {
            return Err(BusFault::Unmapped);
        };

        let Some(ram) = &self.memory[index] else {
            data.fill(0);
            return Ok(());
        };

        match ram.read(offset, data) {
            Ok(()) => Ok(()),
            Err(BusFault::Unmapped) => {
                data.fill(0);
                Ok(())
            }
            Err(fault) => Err(fault),
        }
    }

    fn write_memory(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
        if !local_memory_transaction_is_contained(address, data.len())? {
            self.pic1.report_cpu_write_bus_error();
            return Ok(());
        }

        let Some((index, offset)) = self.pic1.decode_memory(address, data.len())? else {
            self.pic1.report_cpu_write_bus_error();
            return Ok(());
        };

        let Some(ram) = &mut self.memory[index] else {
            return Ok(());
        };

        match ram.write(offset, data) {
            Ok(()) | Err(BusFault::Unmapped) => Ok(()),
            Err(fault) => Err(fault),
        }
    }
}

impl PhysicalBus for Ip12Bus {
    fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
        if address.get() < LOCAL_MEMORY_END {
            return self.read_memory(address, data);
        }

        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.read(address, data),
            Target::Hpc1(address) => self.hpc1.read(address, data),
            Target::Scsi(address) => self.scsi.read(address, data),
            Target::CpuAuxControl => read_cpu_aux_control(self.cpu_aux_control, &self.nvram, data),
            Target::Int2(address) => self.int2.read(address, data),
            Target::Serial(index, address) => {
                let result = self.serial[index].read(address, data);
                self.synchronize_serial_interrupt();
                result
            }
            Target::UnpopulatedSerial(_) => Err(BusFault::Unmapped),
            Target::Mdac(address) => self.mdac.read(address, data),
            Target::Rtc(address) => self.rtc.read(address, data),
            Target::BoardRevision => read_board_revision(data),
            Target::Dsp56001(address) => self.dsp56001.read(address, data),
            Target::Prom(address) => self.prom.read(address, data),
            Target::UnpopulatedGio => read_unpopulated_gio(data),
        }
    }

    fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
        if address.get() < LOCAL_MEMORY_END {
            return self.write_memory(address, data);
        }

        match route(address, data.len())? {
            Target::Pic1(address) => self.pic1.write(address, data),
            Target::Hpc1(address) => {
                self.hpc1.write(address, data)?;
                if self.hpc1.take_scsi_reset_request() {
                    self.scsi.reset();
                }
                Ok(())
            }
            Target::Scsi(address) => self.scsi.write(address, data),
            Target::CpuAuxControl => {
                write_cpu_aux_control(&mut self.cpu_aux_control, &mut self.nvram, data)
            }
            Target::Int2(address) => self.int2.write(address, data),
            Target::Serial(index, address) => {
                let result = self.serial[index].write(address, data);
                self.synchronize_serial_interrupt();
                result
            }
            Target::UnpopulatedSerial(address) => write_unpopulated_serial(address, data),
            Target::Mdac(address) => self.mdac.write(address, data),
            Target::Rtc(address) => self.rtc.write(address, data),
            Target::BoardRevision => Err(BusFault::UnsupportedAccess),
            Target::Dsp56001(address) => self.dsp56001.write(address, data),
            Target::Prom(_) => Ok(()),
            Target::UnpopulatedGio => Ok(()),
        }
    }
}

fn local_memory_transaction_is_contained(
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

enum Target {
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

fn route(address: PhysAddr, length: usize) -> Result<Target, BusFault> {
    if !(1..=4).contains(&length) {
        return Err(BusFault::UnsupportedAccess);
    }

    let start = address.get();
    let length = u64::try_from(length).map_err(|_| BusFault::UnsupportedAccess)?;
    let end = start.checked_add(length).ok_or(BusFault::Unmapped)?;

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
        (HPC1_SCSI_CONTROL_BASE, HPC1_SCSI_CONTROL_END),
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

    if contains(start, end, SERIAL_0_BASE, SERIAL_0_END) {
        return Ok(Target::Serial(0, DeviceAddr::new(start - SERIAL_0_BASE)));
    }
    if overlaps(start, end, SERIAL_0_BASE, SERIAL_0_END) {
        return Err(BusFault::Unmapped);
    }
    if contains(start, end, SERIAL_1_BASE, SERIAL_1_END) {
        return Ok(Target::Serial(1, DeviceAddr::new(start - SERIAL_1_BASE)));
    }
    if overlaps(start, end, SERIAL_1_BASE, SERIAL_1_END) {
        return Err(BusFault::Unmapped);
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
    if contains(start, end, PROM_BASE, PROM_END) {
        return Ok(Target::Prom(DeviceAddr::new(start - PROM_BASE)));
    }

    Err(BusFault::Unmapped)
}

fn read_cpu_aux_control(value: u8, nvram: &Nmc93cs46, data: &mut [u8]) -> Result<(), BusFault> {
    if data.len() != 1 {
        return Err(BusFault::UnsupportedAccess);
    }
    data[0] = value & CPU_AUX_OUTPUT_BITS | u8::from(nvram.data_out()) << 4;
    Ok(())
}

fn write_cpu_aux_control(
    value: &mut u8,
    nvram: &mut Nmc93cs46,
    data: &[u8],
) -> Result<(), BusFault> {
    if data.len() != 1 {
        return Err(BusFault::UnsupportedAccess);
    }
    *value = data[0] & CPU_AUX_OUTPUT_BITS;
    nvram.drive_pins(
        *value & 0x01 != 0,
        *value & 0x02 != 0,
        *value & 0x04 != 0,
        *value & 0x08 != 0,
    );
    Ok(())
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

const fn contains(start: u64, end: u64, range_start: u64, range_end: u64) -> bool {
    start >= range_start && end <= range_end
}

const fn overlaps(start: u64, end: u64, range_start: u64, range_end: u64) -> bool {
    start < range_end && end > range_start
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
    use se_device::dp8573a::Dp8573a;
    use se_device::dsp56001::Dsp56001;
    use se_device::hpc1::Hpc1;
    use se_device::int2::Int2;
    use se_device::mdac::Mdac;
    use se_device::nmc93cs46::Nmc93cs46;
    use se_device::pic1::Pic1;
    use se_device::ram::Ram;
    use se_device::rom::Rom;
    use se_device::wd33c93b::Wd33c93b;
    use se_device::z85230::Z85230;

    use crate::output::MachineOutput;
    use crate::serial::SerialPort;

    use super::{
        BOARD_REVISION_BASE, CPU_AUX_CONTROL, DSP56001_BASE, DSP56001_END, GIO_BASE, GIO_END,
        HPC1_ENDIAN_CONTROL_BASE, HPC1_ETHERNET_FIFO_BASE, HPC1_ETHERNET_POINTER_BASE,
        HPC1_MISCELLANEOUS_CONTROL_BASE, HPC1_SCSI_CONTROL_BASE, INT2_BASE, Ip12Bus,
        LOCAL_MEMORY_END, MDAC_BASE, PIC1_BASE, PROM_BASE, PROM_BYTES, PROM_END, RTC_BASE,
        SCSI_BASE, SERIAL_0_BASE, SERIAL_1_BASE, SERIAL_2_BASE,
    };

    fn bus() -> Ip12Bus {
        bus_with_memory([Some(Ram::new(8 * 1024 * 1024)), None, None, None])
    }

    fn bus_with_memory(memory: [Option<Ram>; 4]) -> Ip12Bus {
        let bytes = (0..PROM_BYTES).map(|index| index as u8).collect();
        Ip12Bus::new(
            Pic1::new(0xf7, 2, true),
            memory,
            Hpc1::new(),
            Int2::new(),
            Wd33c93b::new(),
            [Z85230::new(3_686_400), Z85230::new(3_686_400)],
            Dp8573a::new(),
            Mdac::new(),
            Nmc93cs46::new(),
            Dsp56001::new(),
            Rom::new(bytes),
        )
    }

    fn read_word(bus: &mut Ip12Bus, address: u64) -> Result<u32, BusFault> {
        let mut bytes = [0; 4];
        bus.read(PhysAddr::new(address), &mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn read_byte(bus: &mut Ip12Bus, address: u64) -> Result<u8, BusFault> {
        let mut byte = [0];
        bus.read(PhysAddr::new(address), &mut byte)?;
        Ok(byte[0])
    }

    fn configure_memory(bus: &mut Ip12Bus, configuration_0: u32, configuration_1: u32) {
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

    fn write_serial_register(bus: &mut Ip12Bus, base: u64, control: u8, value: u8) {
        bus.write(PhysAddr::new(base + 0x0b), &[control]).unwrap();
        bus.write(PhysAddr::new(base + 0x0b), &[value]).unwrap();
    }

    fn configure_serial_a(bus: &mut Ip12Bus, base: u64) {
        for (register, value) in [(4, 0x44), (11, 0x10), (12, 10), (13, 0), (14, 1), (5, 0x68)] {
            write_serial_register(bus, base, register, value);
        }
    }

    fn nvram_clock_bit(bus: &mut Ip12Bus, bit: bool) -> bool {
        let value = 0x02 | u8::from(bit) << 3;
        bus.write(PhysAddr::new(CPU_AUX_CONTROL), &[value]).unwrap();
        bus.write(PhysAddr::new(CPU_AUX_CONTROL), &[value | 0x04])
            .unwrap();
        read_byte(bus, CPU_AUX_CONTROL).unwrap() & 0x10 != 0
    }

    fn nvram_shift_command(bus: &mut Ip12Bus, command: u16) {
        for bit in (0..11).rev() {
            nvram_clock_bit(bus, command & (1 << bit) != 0);
        }
    }

    fn nvram_deselect(bus: &mut Ip12Bus) {
        bus.write(PhysAddr::new(CPU_AUX_CONTROL), &[0]).unwrap();
    }

    fn nvram_command(bus: &mut Ip12Bus, command: u16) {
        nvram_deselect(bus);
        nvram_shift_command(bus, command);
        nvram_deselect(bus);
    }

    fn nvram_write_word(bus: &mut Ip12Bus, address: u16, value: u16) {
        nvram_deselect(bus);
        nvram_shift_command(bus, 0x0500 | address);
        for bit in (0..16).rev() {
            nvram_clock_bit(bus, value & (1 << bit) != 0);
        }
        nvram_deselect(bus);
    }

    fn nvram_read_word(bus: &mut Ip12Bus, address: u16) -> u16 {
        nvram_deselect(bus);
        nvram_shift_command(bus, 0x0600 | address);
        let mut value = 0;
        for _ in 0..16 {
            value = value << 1 | u16::from(nvram_clock_bit(bus, false));
        }
        nvram_deselect(bus);
        value
    }

    #[test]
    fn routes_real_memory_through_the_pic1_configuration() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x0f00_023f, 0x023f_023f);

        assert_eq!(
            bus.write(PhysAddr::new(0), &0x0123_4567_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, 0), Ok(0x0123_4567));
        assert_eq!(
            bus.write(
                PhysAddr::new(8 * 1024 * 1024 - 4),
                &0x89ab_cdef_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, 8 * 1024 * 1024 - 4), Ok(0x89ab_cdef));
    }

    #[test]
    fn installed_storage_boundary_is_a_probe_hole_not_an_alias() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x0f00_023f, 0x023f_023f);
        bus.write(
            PhysAddr::new(8 * 1024 * 1024 - 4),
            &0x0123_4567_u32.to_be_bytes(),
        )
        .unwrap();

        assert_eq!(read_word(&mut bus, 8 * 1024 * 1024), Ok(0));
        assert_eq!(
            bus.write(
                PhysAddr::new(8 * 1024 * 1024),
                &0x89ab_cdef_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, 8 * 1024 * 1024 - 4), Ok(0x0123_4567));

        let mut crossing = [0xff; 4];
        assert_eq!(
            bus.read(PhysAddr::new(8 * 1024 * 1024 - 2), &mut crossing),
            Ok(())
        );
        assert_eq!(crossing, [0; 4]);
        assert_eq!(
            bus.write(
                PhysAddr::new(8 * 1024 * 1024 - 2),
                &0xffff_ffff_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, 8 * 1024 * 1024 - 4), Ok(0x0123_4567));
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn uninstalled_descriptor_is_a_probe_hole() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x0f00_0f10, 0x023f_023f);
        let address = 16_u64 << 22;

        assert_eq!(read_word(&mut bus, address), Ok(0));
        assert_eq!(
            bus.write(PhysAddr::new(address), &0x0123_4567_u32.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_word(&mut bus, address), Ok(0));
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn unmatched_local_reads_fault_and_writes_latch_an_error() {
        let mut bus = bus();
        configure_memory(&mut bus, 0x023f_023f, 0x023f_023f);

        assert_eq!(
            bus.read(PhysAddr::new(0), &mut [0; 4]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            bus.debug_read(PhysAddr::new(0), &mut [0; 4]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            bus.write(PhysAddr::new(0), &0x0123_4567_u32.to_be_bytes()),
            Ok(())
        );
        assert!(bus.error_interrupt_asserted());

        bus.write(PhysAddr::new(PIC1_BASE + 0x1_0210), &[0])
            .unwrap();
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn separate_ram_modules_do_not_alias() {
        let mut bus = bus_with_memory([
            Some(Ram::new(16 * 1024 * 1024)),
            Some(Ram::new(32 * 1024 * 1024)),
            None,
            None,
        ]);
        configure_memory(&mut bus, 0x0300_0704, 0x023f_023f);
        let second_base = 4_u64 << 22;

        bus.write(PhysAddr::new(0), &0x0123_4567_u32.to_be_bytes())
            .unwrap();
        bus.write(PhysAddr::new(second_base), &0x89ab_cdef_u32.to_be_bytes())
            .unwrap();

        assert_eq!(read_word(&mut bus, 0), Ok(0x0123_4567));
        assert_eq!(read_word(&mut bus, second_base), Ok(0x89ab_cdef));
        assert_eq!(
            bus.write(
                PhysAddr::new(second_base + 32 * 1024 * 1024 - 4),
                &0xfedc_ba98_u32.to_be_bytes()
            ),
            Ok(())
        );
        assert_eq!(
            read_word(&mut bus, second_base + 32 * 1024 * 1024 - 4),
            Ok(0xfedc_ba98)
        );
    }

    #[test]
    fn local_memory_window_crossing_writes_latch_an_error() {
        let mut bus = bus();

        assert_eq!(
            bus.write(PhysAddr::new(LOCAL_MEMORY_END - 2), &[1, 2, 3, 4]),
            Ok(())
        );
        assert!(bus.error_interrupt_asserted());
        assert_eq!(
            bus.read(PhysAddr::new(LOCAL_MEMORY_END - 2), &mut [0; 4]),
            Err(BusFault::Unmapped)
        );
    }

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
    fn only_the_second_serial_controller_reaches_external_machine_output() {
        let mut bus = bus();
        configure_serial_a(&mut bus, SERIAL_0_BASE);
        configure_serial_a(&mut bus, SERIAL_1_BASE);
        bus.write(PhysAddr::new(SERIAL_0_BASE + 0x0f), &[0x11])
            .unwrap();
        bus.write(PhysAddr::new(SERIAL_1_BASE + 0x0f), &[0x22])
            .unwrap();
        let mut output = MachineOutput::default();

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND / 960 - 1),
            &mut output,
        );
        assert!(output.is_empty());
        bus.advance_time(VirtualDuration::from_attoseconds(1), &mut output);

        assert_eq!(output.serial(SerialPort::A), [0x22]);
        assert!(output.serial(SerialPort::B).is_empty());
    }

    #[test]
    fn external_serial_input_drives_the_masked_int2_local_interrupt() {
        let mut bus = bus();
        write_serial_register(&mut bus, SERIAL_1_BASE, 3, 1);
        write_serial_register(&mut bus, SERIAL_1_BASE, 1, 0x10);
        write_serial_register(&mut bus, SERIAL_1_BASE, 9, 1 << 3);
        bus.write(PhysAddr::new(INT2_BASE + 7), &[1 << 5]).unwrap();

        assert_eq!(bus.receive_serial(SerialPort::A, b"A"), 1);
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(1 << 5));
        assert!(bus.local_interrupt_0_asserted());

        assert_eq!(read_byte(&mut bus, SERIAL_1_BASE + 0x0f), Ok(b'A'));
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(0));
        assert!(!bus.local_interrupt_0_asserted());
    }

    #[test]
    fn machine_time_advances_the_rtc_without_connecting_its_interrupt_to_int2() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        bus.write(PhysAddr::new(RTC_BASE + 0x03), &[0x40]).unwrap();
        bus.write(PhysAddr::new(RTC_BASE + 0x0f), &[0x20]).unwrap();
        bus.write(PhysAddr::new(RTC_BASE + 0x07), &[0x08]).unwrap();
        bus.write(PhysAddr::new(RTC_BASE + 0x03), &[0]).unwrap();

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND / 100),
            &mut output,
        );

        assert_eq!(read_byte(&mut bus, RTC_BASE + 0x17), Ok(1));
        let mut periodic_flags = [0xff];
        bus.debug_read(PhysAddr::new(RTC_BASE + 0x0f), &mut periodic_flags)
            .unwrap();
        assert_eq!(periodic_flags, [0x30]);
        assert_eq!(read_byte(&mut bus, RTC_BASE + 0x03), Ok(0x05));
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(0));
        assert_eq!(read_byte(&mut bus, RTC_BASE + 0x0f), Ok(0x30));
        bus.debug_read(PhysAddr::new(RTC_BASE + 0x0f), &mut periodic_flags)
            .unwrap();
        assert_eq!(periodic_flags, [0]);
    }

    #[test]
    fn reset_preserves_the_rtc_prescaler_phase() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        bus.write(PhysAddr::new(RTC_BASE + 0x03), &[0x40]).unwrap();
        bus.write(PhysAddr::new(RTC_BASE + 0x07), &[0x08]).unwrap();
        bus.write(PhysAddr::new(RTC_BASE + 0x03), &[0]).unwrap();
        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND * 9 / 1_000),
            &mut output,
        );

        bus.reset();
        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND / 1_000),
            &mut output,
        );

        assert_eq!(read_byte(&mut bus, RTC_BASE + 0x17), Ok(1));
    }

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
        assert_eq!(read_byte(&mut bus, 0x1fb8_0100), Err(BusFault::Unmapped));
    }

    #[test]
    fn hpc1_scsi_reset_reaches_the_wd33c93b() {
        let mut bus = bus();
        bus.write(PhysAddr::new(SCSI_BASE), &[2]).unwrap();
        bus.write(PhysAddr::new(SCSI_BASE + 4), &[0xa5]).unwrap();
        bus.write(PhysAddr::new(HPC1_SCSI_CONTROL_BASE + 3), &[1])
            .unwrap();
        bus.write(PhysAddr::new(SCSI_BASE), &[2]).unwrap();

        assert_eq!(read_byte(&mut bus, SCSI_BASE + 4), Ok(0));
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0x80));
    }

    #[test]
    fn scsi_debug_reads_do_not_acknowledge_status() {
        let mut bus = bus();
        bus.write(PhysAddr::new(SCSI_BASE), &[0x17]).unwrap();
        let mut status = [0xff];

        bus.debug_read(PhysAddr::new(SCSI_BASE + 4), &mut status)
            .unwrap();

        assert_eq!(status, [0]);
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0x80));
        assert_eq!(read_byte(&mut bus, SCSI_BASE + 4), Ok(0));
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0));
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
    fn cpu_aux_control_drives_serial_nvram() {
        let mut bus = bus();

        for address in [0, 63] {
            assert_eq!(nvram_read_word(&mut bus, address), u16::MAX);
        }
        nvram_command(&mut bus, 0x04c0);
        nvram_write_word(&mut bus, 17, 0x8123);
        assert_eq!(nvram_read_word(&mut bus, 17), 0x8123);
        nvram_command(&mut bus, 0x0400);
        nvram_write_word(&mut bus, 17, 0);
        assert_eq!(nvram_read_word(&mut bus, 17), 0x8123);
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
