use se_core::bus::BusError;
use se_core::time::VirtualDuration;
use se_device::int2::Int2;
use se_device::nmc93cs46::Nmc93cs46;
use se_device::scsi::{ScsiDataDirection, ScsiTransferResult};
use se_device::wd33c93b::WdWork;
use se_device::z85230::Z85230;

use super::super::events::EventKind;
use super::Ip12Bus;

const CPU_AUX_OUTPUT_BITS: u8 = 0x0f;
const SCSI_INTERRUPT: u8 = 1 << 2;
const PARALLEL_INTERRUPT: u8 = 1 << 1;
const SERIAL_INTERRUPT: u8 = 1 << 5;
const DSP_INTERRUPT: u8 = 1 << 4;

impl Ip12Bus {
    pub(super) fn synchronize_serial_interrupt(&mut self) {
        let asserted = self.serial.iter().any(Z85230::interrupt_asserted);
        self.int2
            .set_local_interrupt_0_input(SERIAL_INTERRUPT, asserted);
    }

    pub(super) fn synchronize_scsi_interrupt(&mut self) {
        self.int2
            .set_local_interrupt_0_input(SCSI_INTERRUPT, self.wd33c93b.interrupt_asserted());
    }

    pub(super) fn synchronize_hpc1_interrupts(&mut self) {
        drive_hpc1_interrupt_inputs(
            &mut self.int2,
            self.hpc1.parallel_interrupt_asserted(),
            self.hpc1.dsp_interrupt_asserted(),
        );
    }

    pub(super) fn handle_scsi_register_write(&mut self) {
        if self.wd33c93b.take_reset_completion() {
            self.pending_scsi = None;
            self.scsi_bus.cancel_transaction();
            self.events.schedule(EventKind::Scsi, None);
        }
        if let Some(request) = self.wd33c93b.take_request() {
            self.pending_scsi = Some(request);
            self.events
                .schedule(EventKind::Scsi, Some(VirtualDuration::ZERO));
        }
        self.synchronize_scsi_interrupt();
    }

    pub(super) fn handle_hpc1_outputs(&mut self) {
        if self.hpc1.take_ethernet_reset_request() {
            self.seeq8003.reset();
        }
        if self.hpc1.take_scsi_reset_request() {
            self.wd33c93b.reset();
            self.pending_scsi = None;
            self.scsi_bus.cancel_transaction();
            self.events.schedule(EventKind::Scsi, None);
        }
        self.service_scsi_descriptor_fetch();
        if self.wd33c93b.dma_pending() {
            self.transfer_active_scsi_data(self.wd33c93b.remaining_transfer_bytes());
        }
        self.synchronize_scsi_interrupt();
        self.synchronize_hpc1_interrupts();
    }

    pub(super) fn process_scsi_event(&mut self) {
        let Some(request) = self.pending_scsi.take() else {
            return;
        };
        match self.wd33c93b.service_request(request, &mut self.scsi_bus) {
            Ok(WdWork::Idle) => {}
            Ok(WdWork::Dma {
                direction,
                byte_count,
            }) => match direction {
                ScsiDataDirection::In => self.transfer_scsi_data_in(byte_count),
                ScsiDataDirection::Out => self.transfer_scsi_data_out(byte_count),
            },
            Ok(WdWork::SelectionWait(duration)) => {
                self.pending_scsi = self.wd33c93b.take_request();
                self.events.schedule(EventKind::Scsi, duration);
            }
            Err(_) => self.hpc1.stop_scsi_dma(),
        }
        self.synchronize_scsi_interrupt();
    }

    fn transfer_active_scsi_data(&mut self, wd_bytes_remaining: u32) {
        match self.scsi_bus.active_data_direction() {
            Some(ScsiDataDirection::In) => self.transfer_scsi_data_in(wd_bytes_remaining),
            Some(ScsiDataDirection::Out) => self.transfer_scsi_data_out(wd_bytes_remaining),
            None => self.hpc1.stop_scsi_dma(),
        }
    }

    fn transfer_scsi_data_in(&mut self, mut wd_bytes_remaining: u32) {
        if wd_bytes_remaining == 0 {
            self.hpc1.finish_scsi_dma();
            self.finish_scsi_dma_window(None);
            return;
        }

        loop {
            let Some(window) = self.next_scsi_dma_window() else {
                return;
            };
            if !window.to_memory() {
                self.hpc1.stop_scsi_dma();
                return;
            }
            let maximum_bytes = usize::from(
                window
                    .byte_count()
                    .min(u16::try_from(wd_bytes_remaining).unwrap_or(u16::MAX)),
            );
            let buffer_address = window.buffer_address();
            let result = {
                let Self {
                    pic1,
                    memory,
                    hpc1,
                    wd33c93b,
                    scsi_bus,
                    ..
                } = self;
                scsi_bus.transfer_data_in(maximum_bytes, |bytes| {
                    if !memory.write_dma(pic1, buffer_address, bytes) {
                        return false;
                    }
                    let Ok(byte_count) = u16::try_from(bytes.len()) else {
                        return false;
                    };
                    hpc1.consume_scsi_dma_bytes(byte_count)
                        && wd33c93b.consume_transfer_bytes(u32::from(byte_count))
                })
            };

            match result {
                Ok(ScsiTransferResult::Rejected) | Err(_) => {
                    self.hpc1.stop_scsi_dma();
                    return;
                }
                Ok(ScsiTransferResult::More { transferred, .. }) => {
                    let Ok(transferred) = u32::try_from(transferred) else {
                        self.hpc1.stop_scsi_dma();
                        return;
                    };
                    wd_bytes_remaining -= transferred;
                    if wd_bytes_remaining == 0 {
                        self.hpc1.finish_scsi_dma();
                        self.finish_scsi_dma_window(None);
                        return;
                    }
                }
                Ok(ScsiTransferResult::Complete { status, .. }) => {
                    self.hpc1.finish_scsi_dma();
                    self.finish_scsi_dma_window(Some(status));
                    return;
                }
            }
        }
    }

    fn transfer_scsi_data_out(&mut self, mut wd_bytes_remaining: u32) {
        if wd_bytes_remaining == 0 {
            self.hpc1.finish_scsi_dma();
            self.finish_scsi_dma_window(None);
            return;
        }

        loop {
            let Some(window) = self.next_scsi_dma_window() else {
                return;
            };
            if window.to_memory() {
                self.hpc1.stop_scsi_dma();
                return;
            }
            let maximum_bytes = usize::from(
                window
                    .byte_count()
                    .min(u16::try_from(wd_bytes_remaining).unwrap_or(u16::MAX)),
            );
            let buffer_address = window.buffer_address();
            let result = {
                let Self {
                    pic1,
                    memory,
                    scsi_bus,
                    ..
                } = self;
                scsi_bus.transfer_data_out(maximum_bytes, |bytes| {
                    memory.read_dma(pic1, buffer_address, bytes)
                })
            };

            match result {
                Ok(ScsiTransferResult::Rejected) | Err(_) => {
                    self.hpc1.stop_scsi_dma();
                    return;
                }
                Ok(ScsiTransferResult::More { transferred, .. }) => {
                    let transferred = self.advance_scsi_data_out(transferred);
                    wd_bytes_remaining = wd_bytes_remaining
                        .checked_sub(transferred)
                        .expect("SCSI bus cannot exceed the WD transfer window");
                    if wd_bytes_remaining == 0 {
                        self.hpc1.finish_scsi_dma();
                        self.finish_scsi_dma_window(None);
                        return;
                    }
                }
                Ok(ScsiTransferResult::Complete {
                    transferred,
                    status,
                }) => {
                    if transferred != 0 {
                        self.advance_scsi_data_out(transferred);
                    }
                    self.hpc1.finish_scsi_dma();
                    self.finish_scsi_dma_window(Some(status));
                    return;
                }
            }
        }
    }

    fn finish_scsi_dma_window(&mut self, status: Option<se_device::scsi::ScsiStatus>) {
        if self
            .wd33c93b
            .finish_dma(&mut self.scsi_bus, status)
            .is_err()
        {
            self.hpc1.stop_scsi_dma();
        }
    }

    fn advance_scsi_data_out(&mut self, transferred: usize) -> u32 {
        let byte_count = u16::try_from(transferred)
            .expect("SCSI transfer chunk must fit the HPC1 byte-count field");
        assert!(self.hpc1.consume_scsi_dma_bytes(byte_count));
        assert!(self.wd33c93b.consume_transfer_bytes(u32::from(byte_count)));
        u32::from(byte_count)
    }

    fn next_scsi_dma_window(&mut self) -> Option<se_device::hpc1::ScsiDmaWindow> {
        loop {
            if let Some(window) = self.hpc1.scsi_dma_window() {
                return Some(window);
            }
            let descriptor_address = self.hpc1.take_scsi_descriptor_fetch()?;
            let mut descriptor = [0; 12];
            if !self.read_dma_memory(descriptor_address, &mut descriptor) {
                self.hpc1.stop_scsi_dma();
                return None;
            }
            self.hpc1.load_scsi_descriptor(descriptor);
            if self.hpc1.scsi_dma_window().is_none() {
                self.hpc1.stop_scsi_dma();
                return None;
            }
        }
    }

    fn service_scsi_descriptor_fetch(&mut self) {
        let Some(descriptor_address) = self.hpc1.take_scsi_descriptor_fetch() else {
            return;
        };
        let mut descriptor = [0; 12];
        if self.read_dma_memory(descriptor_address, &mut descriptor) {
            self.hpc1.load_scsi_descriptor(descriptor);
        } else {
            self.hpc1.stop_scsi_dma();
        }
    }

    fn read_dma_memory(&mut self, address: u32, data: &mut [u8]) -> bool {
        self.memory.read_dma(&mut self.pic1, address, data)
    }
}

pub(super) fn drive_hpc1_interrupt_inputs(int2: &mut Int2, parallel: bool, dsp: bool) {
    int2.set_local_interrupt_0_input(PARALLEL_INTERRUPT, parallel);
    int2.set_local_interrupt_1_input(DSP_INTERRUPT, dsp);
}

pub(super) fn read_cpu_aux_control(
    value: u8,
    nvram: &Nmc93cs46,
    data: &mut [u8],
) -> Result<(), BusError> {
    if !(1..=4).contains(&data.len()) {
        return Err(BusError::InvalidTransaction);
    }
    if data.len() != 1 {
        return Err(BusError::UnimplementedAccess);
    }
    data[0] = value & CPU_AUX_OUTPUT_BITS | u8::from(nvram.data_out()) << 4;
    Ok(())
}

pub(super) fn write_cpu_aux_control(
    value: &mut u8,
    nvram: &mut Nmc93cs46,
    data: &[u8],
) -> Result<(), BusError> {
    if !(1..=4).contains(&data.len()) {
        return Err(BusError::InvalidTransaction);
    }
    if data.len() != 1 {
        return Err(BusError::UnimplementedAccess);
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

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr, PhysAddr, PhysicalBus};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use crate::output::MachineOutput;
    use crate::serial::SerialPort;

    use super::{Ip12Bus, SCSI_INTERRUPT, drive_hpc1_interrupt_inputs};

    use super::super::address::{
        HPC1_SCSI_CONTROL_BASE, HPC1_SCSI_REGISTERS_BASE, INT2_BASE, SCSI_ADDRESS_PORT,
        SCSI_DATA_PORT, SCSI_WINDOW_BASE, SERIAL_1_BASE,
    };
    use super::super::test_support::{
        bus, bus_with_cdrom, bus_with_disk, bus_with_disk_and_cdrom, bus_with_disk_failures,
        configure_scsi_descriptor_chain, configure_scsi_write_descriptor_chain, configure_serial_a,
        configure_single_scsi_descriptor, configure_single_scsi_write_descriptor, issue_read_ten,
        issue_scsi_command, issue_write_ten, nvram_command, nvram_read_word, nvram_write_word,
        read_byte, read_scsi_register, read_word, write_scsi_register, write_serial_register,
    };

    fn write_memory(bus: &mut Ip12Bus, address: u32, bytes: &[u8]) {
        for (offset, chunk) in bytes.chunks(4).enumerate() {
            bus.write(
                PhysAddr::new(u64::from(address) + (offset * 4) as u64),
                chunk,
            )
            .unwrap();
        }
    }

    fn read_memory(bus: &Ip12Bus, address: u32, byte_count: usize) -> Vec<u8> {
        let mut bytes = vec![0; byte_count];
        bus.memory
            .module(0)
            .unwrap()
            .read(DeviceAddr::new(u64::from(address)), &mut bytes)
            .unwrap();
        bytes
    }

    #[test]
    fn external_serial_input_drives_the_masked_int2_local_interrupt() {
        let mut bus = bus();
        write_serial_register(&mut bus, SERIAL_1_BASE, 3, 1);
        write_serial_register(&mut bus, SERIAL_1_BASE, 1, 0x10);
        write_serial_register(&mut bus, SERIAL_1_BASE, 9, 1 << 3);
        bus.write(PhysAddr::new(INT2_BASE + 7), &[1 << 5]).unwrap();

        assert_eq!(bus.receive_serial(SerialPort::A, b"A"), 1);
        assert_eq!(
            read_word(&mut bus, INT2_BASE),
            Ok(u32::from((1 << 5) | SCSI_INTERRUPT))
        );
        assert!(bus.local_interrupt_0_asserted());

        assert_eq!(read_byte(&mut bus, SERIAL_1_BASE + 0x0f), Ok(b'A'));
        assert_eq!(
            read_word(&mut bus, INT2_BASE),
            Ok(u32::from(SCSI_INTERRUPT))
        );
        assert!(!bus.local_interrupt_0_asserted());
    }

    #[test]
    fn hpc1_interrupt_levels_drive_distinct_int2_local_banks() {
        let mut bus = bus();
        bus.write(PhysAddr::new(INT2_BASE + 7), &[1 << 1]).unwrap();
        bus.write(PhysAddr::new(INT2_BASE + 0x0f), &[1 << 4])
            .unwrap();

        drive_hpc1_interrupt_inputs(&mut bus.int2, true, true);

        assert_eq!(
            read_word(&mut bus, INT2_BASE),
            Ok(u32::from((1 << 1) | SCSI_INTERRUPT))
        );
        assert_eq!(read_word(&mut bus, INT2_BASE + 8), Ok(1 << 4));
        assert!(bus.local_interrupt_0_asserted());
        assert!(bus.local_interrupt_1_asserted());

        drive_hpc1_interrupt_inputs(&mut bus.int2, false, false);
        assert_eq!(
            read_word(&mut bus, INT2_BASE),
            Ok(u32::from(SCSI_INTERRUPT))
        );
        assert_eq!(read_word(&mut bus, INT2_BASE + 8), Ok(0));
        assert!(!bus.local_interrupt_0_asserted());
        assert!(!bus.local_interrupt_1_asserted());
    }

    #[test]
    fn timed_local_loopback_receive_interrupt_reaches_int2() {
        let mut bus = bus();
        configure_serial_a(&mut bus, SERIAL_1_BASE);
        write_serial_register(&mut bus, SERIAL_1_BASE, 3, 1);
        write_serial_register(&mut bus, SERIAL_1_BASE, 1, 0x10);
        write_serial_register(&mut bus, SERIAL_1_BASE, 9, 1 << 3);
        write_serial_register(&mut bus, SERIAL_1_BASE, 14, 0x11);
        bus.write(PhysAddr::new(INT2_BASE + 7), &[1 << 5]).unwrap();
        bus.write(PhysAddr::new(SERIAL_1_BASE + 0x0f), &[0xa5])
            .unwrap();
        let mut output = MachineOutput::default();

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND / 960),
            &mut output,
        );

        assert_eq!(output.serial(SerialPort::A), [0xa5]);
        assert!(bus.local_interrupt_0_asserted());
        assert_eq!(
            read_word(&mut bus, INT2_BASE),
            Ok(u32::from((1 << 5) | SCSI_INTERRUPT))
        );
        assert_eq!(read_byte(&mut bus, SERIAL_1_BASE + 0x0f), Ok(0xa5));
        assert!(!bus.local_interrupt_0_asserted());
    }

    #[test]
    fn transmit_fifo_empty_interrupt_reaches_int2_and_clears_on_refill() {
        let mut bus = bus();
        write_serial_register(&mut bus, SERIAL_1_BASE, 9, 0xc8);
        configure_serial_a(&mut bus, SERIAL_1_BASE);
        write_serial_register(&mut bus, SERIAL_1_BASE, 1, 1 << 1);
        bus.write(PhysAddr::new(INT2_BASE + 7), &[1 << 5]).unwrap();
        for value in b"The " {
            bus.write(PhysAddr::new(SERIAL_1_BASE + 0x0f), &[*value])
                .unwrap();
            assert!(!bus.local_interrupt_0_asserted());
        }
        let mut output = MachineOutput::default();

        assert!(!bus.local_interrupt_0_asserted());
        bus.advance_time(
            VirtualDuration::from_attoseconds(2 * ATTOSECONDS_PER_SECOND / 960),
            &mut output,
        );
        assert_eq!(output.serial(SerialPort::A), b"Th");
        assert!(!bus.local_interrupt_0_asserted());

        bus.advance_time(
            VirtualDuration::from_attoseconds(
                3 * ATTOSECONDS_PER_SECOND / 960 - 2 * ATTOSECONDS_PER_SECOND / 960,
            ),
            &mut output,
        );
        assert_eq!(output.serial(SerialPort::A), b"The");
        assert!(bus.local_interrupt_0_asserted());
        assert_eq!(
            read_word(&mut bus, INT2_BASE),
            Ok(u32::from((1 << 5) | SCSI_INTERRUPT))
        );

        bus.write(PhysAddr::new(SERIAL_1_BASE + 0x0f), b"s")
            .unwrap();
        assert!(!bus.local_interrupt_0_asserted());
        assert_eq!(
            read_word(&mut bus, INT2_BASE),
            Ok(u32::from(SCSI_INTERRUPT))
        );
    }

    #[test]
    fn word_data_reads_advance_once_and_debug_reads_preserve_status() {
        let mut bus = bus();
        write_scsi_register(&mut bus, 2, 0xa5);
        write_scsi_register(&mut bus, 3, 0x5a);
        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[2]).unwrap();
        assert_eq!(read_word(&mut bus, SCSI_WINDOW_BASE + 4), Ok(0xa500));
        assert_eq!(read_byte(&mut bus, SCSI_DATA_PORT), Ok(0x5a));

        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[0x17])
            .unwrap();
        let mut status = [0xff; 4];
        for _ in 0..2 {
            bus.debug_read(PhysAddr::new(SCSI_WINDOW_BASE + 4), &mut status)
                .unwrap();
            assert_eq!(status, [0; 4]);
            assert_eq!(read_word(&mut bus, INT2_BASE).unwrap() & 4, 4);
        }
        assert_eq!(read_word(&mut bus, SCSI_WINDOW_BASE + 4), Ok(0));
        assert_eq!(read_word(&mut bus, INT2_BASE).unwrap() & 4, 0);
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0));

        // Status advanced to Command: this word write executes software reset.
        bus.write(PhysAddr::new(SCSI_WINDOW_BASE + 4), &[0xff, 0xff, 0, 0xff])
            .unwrap();
        assert_eq!(read_word(&mut bus, INT2_BASE).unwrap() & 4, 4);
        assert!(!bus.wd33c93b.take_reset_completion());
    }

    #[test]
    fn word_commands_schedule_requests_and_reset_cancels_them() {
        let mut bus = bus();
        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[0x17])
            .unwrap();
        read_word(&mut bus, SCSI_WINDOW_BASE + 4).unwrap();
        bus.write(
            PhysAddr::new(SCSI_WINDOW_BASE + 4),
            &0x800_u32.to_be_bytes(),
        )
        .unwrap();
        assert!(bus.pending_scsi.is_some());
        bus.write(PhysAddr::new(SCSI_WINDOW_BASE + 4), &0_u32.to_be_bytes())
            .unwrap();
        assert!(bus.pending_scsi.is_none());
        assert!(!bus.wd33c93b.take_reset_completion());
    }

    #[test]
    fn unsupported_scsi_cross_port_access_does_not_latch_a_hardware_error() {
        let mut bus = bus();
        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[0x17])
            .unwrap();
        let mut bytes = [0xa5; 4];
        let address = PhysAddr::new(SCSI_ADDRESS_PORT);
        assert_eq!(
            bus.read(address, &mut bytes),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(bytes, [0xa5; 4]);
        assert_eq!(
            bus.debug_read(address, &mut bytes),
            Err(BusError::UnimplementedAccess)
        );
        assert_eq!(bytes, [0xa5; 4]);
        assert_eq!(
            bus.write(address, &bytes),
            Err(BusError::UnimplementedAccess)
        );
        assert!(!bus.error_interrupt_asserted());
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x80));
        assert_eq!(read_byte(&mut bus, SCSI_DATA_PORT), Ok(0));
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0));
    }

    #[test]
    fn netbsd_scsi_probe_reads_words_without_acknowledging_reset() {
        let mut bus = bus();
        write_scsi_register(&mut bus, 2, 0xa5);
        for _ in 0..100 {
            read_word(&mut bus, HPC1_SCSI_CONTROL_BASE).unwrap();
        }
        bus.write(PhysAddr::new(HPC1_SCSI_CONTROL_BASE), &1_u32.to_be_bytes())
            .unwrap();
        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND / 1000),
            &mut MachineOutput::default(),
        );
        bus.write(PhysAddr::new(HPC1_SCSI_CONTROL_BASE), &0_u32.to_be_bytes())
            .unwrap();
        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[2]).unwrap();
        for _ in 0..100 {
            let status = read_word(&mut bus, SCSI_WINDOW_BASE).unwrap();
            assert_eq!((status >> 8) & 0xff, 0x80);
            assert_eq!(read_word(&mut bus, INT2_BASE).unwrap() & 4, 4);
        }
        assert_eq!(read_byte(&mut bus, SCSI_DATA_PORT), Ok(0xa5));
    }

    #[test]
    fn hpc1_scsi_reset_reaches_the_wd33c93b() {
        let mut bus = bus();
        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[2]).unwrap();
        bus.write(PhysAddr::new(SCSI_DATA_PORT), &[0xa5]).unwrap();
        bus.write(PhysAddr::new(HPC1_SCSI_CONTROL_BASE + 3), &[1])
            .unwrap();
        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[2]).unwrap();

        assert_eq!(read_byte(&mut bus, SCSI_DATA_PORT), Ok(0xa5));
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x80));
    }

    #[test]
    fn inactive_scsi_descriptor_write_does_not_fetch_guest_memory() {
        let mut bus = bus();

        bus.write(
            PhysAddr::new(HPC1_SCSI_REGISTERS_BASE + 8),
            &0x0c00_0000_u32.to_be_bytes(),
        )
        .unwrap();

        assert_eq!(
            read_word(&mut bus, HPC1_SCSI_REGISTERS_BASE + 8),
            Ok(0x0c00_0000)
        );
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn scsi_debug_reads_do_not_acknowledge_status() {
        let mut bus = bus();
        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[0x17])
            .unwrap();
        let mut status = [0xff];

        bus.debug_read(PhysAddr::new(SCSI_DATA_PORT), &mut status)
            .unwrap();

        assert_eq!(status, [0]);
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x80));
        assert_eq!(read_byte(&mut bus, SCSI_DATA_PORT), Ok(0));
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0));
    }

    #[test]
    fn scsi_read_ten_moves_storage_through_hpc_dma_and_interrupts_int2() {
        let disk: Vec<u8> = (0..512).map(|index| index as u8).collect();
        let mut bus = bus_with_disk(disk.clone(), false);
        configure_single_scsi_descriptor(&mut bus, 0x2000);
        bus.write(PhysAddr::new(INT2_BASE + 7), &[SCSI_INTERRUPT])
            .unwrap();

        issue_read_ten(&mut bus, 1, 0);
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x30));
        let mut output = MachineOutput::default();
        bus.advance_time(VirtualDuration::ZERO, &mut output);

        let mut copied = vec![0; 512];
        bus.memory
            .module(0)
            .unwrap()
            .read(DeviceAddr::new(0x2000), &mut copied)
            .unwrap();
        assert_eq!(copied, disk);
        assert_eq!(read_scsi_register(&mut bus, 0x12), 0);
        assert_eq!(read_scsi_register(&mut bus, 0x13), 0);
        assert_eq!(read_scsi_register(&mut bus, 0x14), 0);
        assert_eq!(
            read_word(&mut bus, INT2_BASE),
            Ok(u32::from(SCSI_INTERRUPT))
        );
        assert!(bus.local_interrupt_0_asserted());
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(0));
        assert!(!bus.local_interrupt_0_asserted());
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn scsi_write_ten_uses_multiple_hpc_descriptors_and_reads_back() {
        const BYTE_COUNT: usize = 1024;

        let payload: Vec<u8> = (0..BYTE_COUNT)
            .map(|index| ((index * 29) ^ (index >> 3)) as u8)
            .collect();
        let mut bus = bus_with_disk(vec![0; BYTE_COUNT], false);
        configure_scsi_write_descriptor_chain(&mut bus, 0x1000, 0x2000, &[256, 768]);
        write_memory(&mut bus, 0x2000, &payload);
        let mut cdb = [0; 10];
        cdb[0] = 0x2a;
        cdb[8] = 2;
        issue_scsi_command(&mut bus, 1, 0, BYTE_COUNT as u32, &cdb);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert_eq!(bus.scsi_bus.active_address(), None);
        configure_scsi_descriptor_chain(&mut bus, 0x1800, 0x3000, &[400, 624]);
        cdb[0] = 0x28;
        issue_scsi_command(&mut bus, 1, 0, BYTE_COUNT as u32, &cdb);
        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(read_memory(&bus, 0x3000, BYTE_COUNT), payload);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn scsi_write_ten_continues_after_the_wd_transfer_window() {
        const BYTE_COUNT: usize = 1024;

        let payload: Vec<u8> = (0..BYTE_COUNT)
            .map(|index| ((index * 17) ^ (index >> 2)) as u8)
            .collect();
        let mut bus = bus_with_disk(vec![0; BYTE_COUNT], false);
        configure_single_scsi_write_descriptor(&mut bus, 0x2000);
        write_memory(&mut bus, 0x2000, &payload);
        let mut cdb = [0; 10];
        cdb[0] = 0x2a;
        cdb[8] = 2;
        issue_scsi_command(&mut bus, 1, 0, 512, &cdb);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(read_scsi_register(&mut bus, 0x10), 0x46);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x48);
        assert_eq!(bus.scsi_bus.active_address(), Some((1, 0)));

        configure_single_scsi_write_descriptor(&mut bus, 0x2200);
        for (register, value) in [
            (0x12, 0),
            (0x13, 2),
            (0x14, 0),
            (0x10, 0x45),
            (0x15, 1),
            (0x0f, 0),
        ] {
            write_scsi_register(&mut bus, register, value);
        }
        write_scsi_register(&mut bus, 0x18, 0x08);
        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert_eq!(bus.scsi_bus.active_address(), None);
        configure_scsi_descriptor_chain(&mut bus, 0x1800, 0x3000, &[512, 512]);
        cdb[0] = 0x28;
        issue_scsi_command(&mut bus, 1, 0, BYTE_COUNT as u32, &cdb);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_memory(&bus, 0x3000, BYTE_COUNT), payload);
    }

    #[test]
    fn reset_cancels_data_out_continuation_without_rolling_back_committed_data() {
        let payload = vec![0xa5; 1024];
        let mut bus = bus_with_disk(vec![0; 1024], false);
        configure_single_scsi_write_descriptor(&mut bus, 0x2000);
        write_memory(&mut bus, 0x2000, &payload);
        let mut cdb = [0; 10];
        cdb[0] = 0x2a;
        cdb[8] = 2;
        issue_scsi_command(&mut bus, 1, 0, 512, &cdb);
        let mut output = MachineOutput::default();
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(bus.scsi_bus.active_address(), Some((1, 0)));

        bus.reset();

        assert_eq!(bus.scsi_bus.active_address(), None);
        configure_single_scsi_descriptor(&mut bus, 0x3000);
        issue_read_ten(&mut bus, 1, 0);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        configure_single_scsi_descriptor(&mut bus, 0x3200);
        issue_read_ten(&mut bus, 1, 1);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_memory(&bus, 0x3000, 512), vec![0xa5; 512]);
        assert_eq!(read_memory(&bus, 0x3200, 512), vec![0; 512]);
    }

    #[test]
    fn wrong_hpc_direction_stops_data_out_without_writing_storage() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        configure_single_scsi_descriptor(&mut bus, 0x2000);
        write_memory(&mut bus, 0x2000, &vec![0x5a; 512]);
        issue_write_ten(&mut bus, 1, 0);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(bus.scsi_bus.active_address(), Some((1, 0)));
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x20));
        assert_eq!(
            read_word(&mut bus, HPC1_SCSI_REGISTERS_BASE + 0x0c),
            Ok(0x10)
        );
        assert!(!bus.error_interrupt_asserted());

        bus.reset();
        configure_single_scsi_descriptor(&mut bus, 0x3000);
        issue_read_ten(&mut bus, 1, 0);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_memory(&bus, 0x3000, 512), vec![0; 512]);
    }

    #[test]
    fn data_out_ram_fault_preserves_target_and_controller_residuals() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        configure_single_scsi_write_descriptor(&mut bus, 0x00c0_0000);
        issue_write_ten(&mut bus, 1, 0);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert!(bus.error_interrupt_asserted());
        assert_eq!(bus.scsi_bus.active_address(), Some((1, 0)));
        assert_eq!(read_scsi_register(&mut bus, 0x13), 2);
        assert_eq!(read_scsi_register(&mut bus, 0x14), 0);
        assert_eq!(read_word(&mut bus, HPC1_SCSI_REGISTERS_BASE), Ok(512));
        assert_eq!(
            read_word(&mut bus, HPC1_SCSI_REGISTERS_BASE + 4),
            Ok(0x80c0_0000)
        );
    }

    #[test]
    fn data_out_storage_failure_returns_check_condition_with_unchanged_residuals() {
        let mut bus = bus_with_disk_failures(vec![0; 512], false, true);
        configure_single_scsi_write_descriptor(&mut bus, 0x2000);
        write_memory(&mut bus, 0x2000, &vec![0x5a; 512]);
        issue_write_ten(&mut bus, 1, 0);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(bus.scsi_bus.active_address(), None);
        assert_eq!(read_scsi_register(&mut bus, 0x0f), 2);
        assert_eq!(read_scsi_register(&mut bus, 0x13), 2);
        assert_eq!(read_scsi_register(&mut bus, 0x14), 0);
        assert_eq!(read_word(&mut bus, HPC1_SCSI_REGISTERS_BASE), Ok(512));
        assert_eq!(
            read_word(&mut bus, HPC1_SCSI_REGISTERS_BASE + 4),
            Ok(0x8000_2000)
        );
        assert_eq!(read_word(&mut bus, HPC1_SCSI_REGISTERS_BASE + 0x0c), Ok(0));
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert!(!bus.error_interrupt_asserted());

        configure_single_scsi_descriptor(&mut bus, 0x2400);
        issue_scsi_command(&mut bus, 1, 0, 18, &[0x03, 0, 0, 0, 18, 0]);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        let sense = read_memory(&bus, 0x2400, 18);
        assert_eq!((sense[2], sense[12], sense[13]), (4, 0x44, 0));
    }

    #[test]
    fn disk_write_and_cdrom_write_protection_remain_target_local() {
        let mut bus = bus_with_disk_and_cdrom(vec![0; 512], vec![0x44; 2048]);
        configure_single_scsi_write_descriptor(&mut bus, 0x2000);
        write_memory(&mut bus, 0x2000, &vec![0x77; 512]);
        issue_write_ten(&mut bus, 1, 0);
        let mut output = MachineOutput::default();
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);

        configure_single_scsi_write_descriptor(&mut bus, 0x2200);
        issue_write_ten(&mut bus, 4, 0);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_scsi_register(&mut bus, 0x0f), 2);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);

        bus.reset();
        configure_single_scsi_descriptor(&mut bus, 0x3000);
        issue_read_ten(&mut bus, 1, 0);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        configure_single_scsi_descriptor(&mut bus, 0x3200);
        issue_read_ten(&mut bus, 4, 0);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_memory(&bus, 0x3000, 512), vec![0x77; 512]);
        assert_eq!(read_memory(&bus, 0x3200, 512), vec![0x44; 512]);
    }

    #[test]
    fn cdrom_read_ten_uses_512_byte_guest_lbas_through_the_shared_dma_path() {
        let cdrom: Vec<u8> = (0..4096).map(|index| (index / 512) as u8).collect();

        for (lba, expected) in [(1, 1), (4, 4)] {
            let mut bus = bus_with_cdrom(cdrom.clone(), false);
            configure_single_scsi_descriptor(&mut bus, 0x2000);
            issue_read_ten(&mut bus, 4, lba);
            let mut output = MachineOutput::default();

            bus.advance_time(VirtualDuration::ZERO, &mut output);

            let mut copied = vec![0; 512];
            bus.memory
                .module(0)
                .unwrap()
                .read(DeviceAddr::new(0x2000), &mut copied)
                .unwrap();
            assert_eq!(copied, vec![expected; 512]);
            assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
            assert!(!bus.error_interrupt_asserted());
        }
    }

    #[test]
    fn cdrom_read_ten_walks_a_full_prom_descriptor_array() {
        const BLOCK_COUNT: u16 = 512;
        const PAGE_COUNT: u32 = 64;
        const BYTE_COUNT: usize = BLOCK_COUNT as usize * 512;

        let cdrom: Vec<u8> = (0..BYTE_COUNT).map(|index| (index / 4096) as u8).collect();
        let mut bus = bus_with_cdrom(cdrom.clone(), false);
        configure_scsi_descriptor_chain(&mut bus, 0x1000, 0x2000, &[4096; PAGE_COUNT as usize]);
        let mut cdb = [0; 10];
        cdb[0] = 0x28;
        cdb[7..9].copy_from_slice(&BLOCK_COUNT.to_be_bytes());
        issue_scsi_command(&mut bus, 4, 0, BYTE_COUNT as u32, &cdb);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        let mut copied = vec![0; BYTE_COUNT];
        bus.memory
            .module(0)
            .unwrap()
            .read(DeviceAddr::new(0x2000), &mut copied)
            .unwrap();
        assert_eq!(copied, cdrom);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn cdrom_read_ten_continues_after_the_prom_dma_map_limit() {
        const BLOCK_COUNT: u16 = 983;
        const BYTE_COUNT: usize = BLOCK_COUNT as usize * 512;
        const MEDIA_BYTE_COUNT: usize = BYTE_COUNT + 512;
        const FIRST_BUFFER_ADDRESS: u32 = 0x0002_0070;
        const FIRST_WINDOW_BYTES: u32 = 3984 + 63 * 4096;
        const SECOND_WINDOW_BYTES: u32 = BYTE_COUNT as u32 - FIRST_WINDOW_BYTES;

        let cdrom: Vec<u8> = (0..MEDIA_BYTE_COUNT)
            .map(|index| ((index * 31) ^ (index >> 8) ^ (index >> 16)) as u8)
            .collect();
        let mut bus = bus_with_cdrom(cdrom.clone(), false);
        let mut first_window = vec![4096_u16; 64];
        first_window[0] = 3984;
        configure_scsi_descriptor_chain(&mut bus, 0x1000, FIRST_BUFFER_ADDRESS, &first_window);
        let mut cdb = [0; 10];
        cdb[0] = 0x28;
        cdb[7..9].copy_from_slice(&BLOCK_COUNT.to_be_bytes());
        issue_scsi_command(&mut bus, 4, 0, FIRST_WINDOW_BYTES, &cdb);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(read_scsi_register(&mut bus, 0x10), 0x46);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x49);
        assert_eq!(bus.scsi_bus.active_address(), Some((4, 0)));

        let mut second_window = vec![4096_u16; 59];
        *second_window.last_mut().unwrap() = 3696;
        configure_scsi_descriptor_chain(
            &mut bus,
            0x1400,
            FIRST_BUFFER_ADDRESS + FIRST_WINDOW_BYTES,
            &second_window,
        );
        for (register, value) in [
            (0x12, (SECOND_WINDOW_BYTES >> 16) as u8),
            (0x13, (SECOND_WINDOW_BYTES >> 8) as u8),
            (0x14, SECOND_WINDOW_BYTES as u8),
            (0x10, 0x45),
            (0x15, 4),
            (0x0f, 0),
        ] {
            write_scsi_register(&mut bus, register, value);
        }
        write_scsi_register(&mut bus, 0x18, 0x08);
        bus.advance_time(VirtualDuration::ZERO, &mut output);

        let mut copied = vec![0; BYTE_COUNT];
        bus.memory
            .module(0)
            .unwrap()
            .read(
                DeviceAddr::new(u64::from(FIRST_BUFFER_ADDRESS)),
                &mut copied,
            )
            .unwrap();
        assert_eq!(copied, cdrom[..BYTE_COUNT]);
        assert_eq!(bus.scsi_bus.active_address(), None);
        assert_eq!(read_scsi_register(&mut bus, 0x10), 0x60);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn disk_and_cdrom_route_to_independent_targets() {
        let mut bus = bus_with_disk_and_cdrom(vec![0x11; 512], vec![0x44; 2048]);
        let mut output = MachineOutput::default();

        configure_single_scsi_descriptor(&mut bus, 0x2000);
        issue_read_ten(&mut bus, 1, 0);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);

        configure_single_scsi_descriptor(&mut bus, 0x2200);
        issue_read_ten(&mut bus, 4, 0);
        bus.advance_time(VirtualDuration::ZERO, &mut output);

        let mut disk_copy = vec![0; 512];
        let mut cdrom_copy = vec![0; 512];
        let memory = bus.memory.module(0).unwrap();
        memory
            .read(DeviceAddr::new(0x2000), &mut disk_copy)
            .unwrap();
        memory
            .read(DeviceAddr::new(0x2200), &mut cdrom_copy)
            .unwrap();
        assert_eq!(disk_copy, vec![0x11; 512]);
        assert_eq!(cdrom_copy, vec![0x44; 512]);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
    }

    #[test]
    fn different_target_does_not_replace_an_active_transaction() {
        let mut bus = bus_with_disk_and_cdrom(vec![0x11; 1024], vec![0x44; 2048]);
        configure_single_scsi_descriptor(&mut bus, 0x2000);
        let mut cdb = [0; 10];
        cdb[0] = 0x28;
        cdb[8] = 2;
        issue_scsi_command(&mut bus, 1, 0, 512, &cdb);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(bus.scsi_bus.active_address(), Some((1, 0)));
        issue_scsi_command(&mut bus, 4, 0, 0, &[0, 0, 0, 0, 0, 0]);
        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(bus.scsi_bus.active_address(), Some((1, 0)));
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x80));
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x40);
    }

    #[test]
    fn absent_targets_complete_after_the_programmed_selection_timeout() {
        for target in [1, 4] {
            let mut bus = bus();
            write_scsi_register(&mut bus, 0x17, 0);
            bus.write(PhysAddr::new(INT2_BASE + 7), &[SCSI_INTERRUPT])
                .unwrap();
            issue_scsi_command(&mut bus, target, 0, 0, &[0, 0, 0, 0, 0, 0]);
            let mut output = MachineOutput::default();

            bus.advance_time(VirtualDuration::ZERO, &mut output);

            assert!(!bus.local_interrupt_0_asserted());
            bus.advance_time(
                VirtualDuration::from_attoseconds(4 * ATTOSECONDS_PER_SECOND / 1000 - 1),
                &mut output,
            );
            assert!(!bus.local_interrupt_0_asserted());
            bus.advance_time(VirtualDuration::from_attoseconds(1), &mut output);

            assert_eq!(
                read_word(&mut bus, INT2_BASE),
                Ok(u32::from(SCSI_INTERRUPT))
            );
            assert!(bus.local_interrupt_0_asserted());
            assert_eq!(read_scsi_register(&mut bus, 0x17), 0x42);
            assert_eq!(read_word(&mut bus, INT2_BASE), Ok(0));
            assert!(!bus.local_interrupt_0_asserted());
        }
    }

    #[test]
    fn storage_failure_becomes_target_check_condition_without_pic1_error() {
        let mut bus = bus_with_disk(vec![0; 512], true);
        configure_single_scsi_descriptor(&mut bus, 0x2000);
        issue_read_ten(&mut bus, 1, 0);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(read_scsi_register(&mut bus, 0x0f), 2);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn cdrom_storage_failure_becomes_target_check_condition_without_pic1_error() {
        let mut bus = bus_with_cdrom(vec![0; 2048], true);
        configure_single_scsi_descriptor(&mut bus, 0x2000);
        issue_read_ten(&mut bus, 4, 0);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert_eq!(read_scsi_register(&mut bus, 0x0f), 2);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert!(!bus.error_interrupt_asserted());
    }

    #[test]
    fn reset_preserves_cdrom_readiness_and_target_state_is_isolated() {
        let mut bus = bus_with_disk_and_cdrom(vec![0; 512], vec![0; 2048]);
        let mut output = MachineOutput::default();
        issue_scsi_command(&mut bus, 4, 0, 0, &[0x1b, 0, 0, 0, 0, 0]);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_scsi_register(&mut bus, 0x0f), 0);

        bus.reset();

        issue_scsi_command(&mut bus, 4, 0, 0, &[0, 0, 0, 0, 0, 0]);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_scsi_register(&mut bus, 0x0f), 2);
        issue_scsi_command(&mut bus, 1, 0, 0, &[0, 0, 0, 0, 0, 0]);
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_scsi_register(&mut bus, 0x0f), 0);

        let mut rebuilt = bus_with_cdrom(vec![0; 2048], false);
        issue_scsi_command(&mut rebuilt, 4, 0, 0, &[0, 0, 0, 0, 0, 0]);
        rebuilt.advance_time(VirtualDuration::ZERO, &mut output);
        assert_eq!(read_scsi_register(&mut rebuilt, 0x0f), 0);
    }

    #[test]
    fn scsi_dma_memory_hole_raises_pic1_address_error_without_finishing_wd() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        configure_single_scsi_descriptor(&mut bus, 0x00c0_0000);
        issue_read_ten(&mut bus, 1, 0);
        let mut output = MachineOutput::default();

        bus.advance_time(VirtualDuration::ZERO, &mut output);

        assert!(bus.error_interrupt_asserted());
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x20));
    }

    #[test]
    fn reset_cancels_a_scheduled_scsi_command() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        configure_single_scsi_descriptor(&mut bus, 0x2000);
        issue_read_ten(&mut bus, 1, 0);
        assert!(bus.pending_scsi.is_some());

        bus.reset();

        assert!(bus.pending_scsi.is_none());
        let mut output = MachineOutput::default();
        bus.advance_time(VirtualDuration::ZERO, &mut output);
        let mut copied = vec![0; 512];
        bus.memory
            .module(0)
            .unwrap()
            .read(DeviceAddr::new(0x2000), &mut copied)
            .unwrap();
        assert_eq!(copied, vec![0; 512]);
    }

    fn service_scsi(bus: &mut super::Ip12Bus) {
        bus.advance_time(VirtualDuration::ZERO, &mut MachineOutput::default());
    }

    fn simple_scsi_command(bus: &mut super::Ip12Bus, command: u8) {
        write_scsi_register(bus, 0x18, command);
        service_scsi(bus);
    }

    fn scsi_count(bus: &mut super::Ip12Bus, count: u32) {
        for (register, value) in [
            (0x12, (count >> 16) as u8),
            (0x13, (count >> 8) as u8),
            (0x14, count as u8),
        ] {
            write_scsi_register(bus, register, value);
        }
    }

    fn select_scsi(bus: &mut super::Ip12Bus, target: u8) {
        read_scsi_register(bus, 0x17);
        write_scsi_register(bus, 0x15, target);
        simple_scsi_command(bus, 6);
        assert_eq!(read_scsi_register(bus, 0x17), 0x11);
        assert_eq!(read_byte(bus, SCSI_ADDRESS_PORT), Ok(0));
        service_scsi(bus);
        assert_eq!(read_scsi_register(bus, 0x17), 0x8e);
    }

    fn pio_send(bus: &mut super::Ip12Bus, bytes: &[u8]) -> u8 {
        write_scsi_register(bus, 1, 0);
        scsi_count(bus, bytes.len() as u32);
        simple_scsi_command(bus, 0x20);
        for &byte in bytes {
            assert_eq!(read_byte(bus, SCSI_ADDRESS_PORT), Ok(0x21));
            write_scsi_register(bus, 0x19, byte);
            service_scsi(bus);
        }
        assert_eq!(read_scsi_register(bus, 0x14), 0);
        read_scsi_register(bus, 0x17)
    }

    fn pio_receive(bus: &mut super::Ip12Bus, count: usize) -> (Vec<u8>, u8) {
        write_scsi_register(bus, 1, 0);
        scsi_count(bus, count as u32);
        simple_scsi_command(bus, 0x20);
        let mut bytes = Vec::new();
        for _ in 0..count {
            assert_eq!(read_byte(bus, SCSI_ADDRESS_PORT), Ok(0x21));
            bytes.push(read_scsi_register(bus, 0x19));
            service_scsi(bus);
        }
        (bytes, read_scsi_register(bus, 0x17))
    }

    fn finish_scsi(bus: &mut super::Ip12Bus) -> u8 {
        // Deliberately poison the automatic CDB registers. Resuming at 0x46
        // must finish the existing command without decoding another CDB.
        write_scsi_register(bus, 3, 0xff);
        scsi_count(bus, 0);
        write_scsi_register(bus, 0x10, 0x46);
        simple_scsi_command(bus, 8);
        assert_eq!(read_scsi_register(bus, 0x17), 0x16);
        assert_eq!(read_scsi_register(bus, 0x10), 0x60);
        assert_eq!(bus.scsi_bus.phase(), None);
        read_scsi_register(bus, 0x0f)
    }

    #[test]
    fn netbsd_selection_sdtr_and_polled_disk_probe_complete_in_phases() {
        let mut bus = bus_with_disk(vec![0; 1024], false);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80, 1, 3, 1, 25, 12]), 0x1f);
        let mut message = Vec::new();
        for index in 0..5 {
            // SBT ignores the preset and clears Transfer Count on success.
            scsi_count(&mut bus, 0x1234);
            simple_scsi_command(&mut bus, 0xa0);
            message.push(read_scsi_register(&mut bus, 0x19));
            service_scsi(&mut bus);
            assert_eq!(read_scsi_register(&mut bus, 0x17), 0x20);
            assert_eq!(read_scsi_register(&mut bus, 0x13), 0);
            assert_eq!(read_scsi_register(&mut bus, 0x14), 0);
            simple_scsi_command(&mut bus, 3);
            assert_eq!(
                read_scsi_register(&mut bus, 0x17),
                if index == 4 { 0x8a } else { 0x8f }
            );
        }
        assert_eq!(message, [1, 3, 1, 25, 0]);
        assert_eq!(pio_send(&mut bus, &[0x12, 0, 0, 0, 36, 0]), 0x19);
        let (inquiry, status) = pio_receive(&mut bus, 36);
        assert_eq!(status, 0x1b);
        assert_eq!(&inquiry[8..16], b"SGI-EMU ");
        assert_eq!(inquiry[7] & 2, 0);
        assert_eq!(finish_scsi(&mut bus), 0);

        for (cdb, count) in [
            (vec![0; 6], 0),
            (vec![0x03, 0, 0, 0, 18, 0], 18),
            (vec![0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8),
        ] {
            select_scsi(&mut bus, 1);
            scsi_count(&mut bus, 0);
            simple_scsi_command(&mut bus, 0xa0);
            write_scsi_register(&mut bus, 0x19, 0x80);
            service_scsi(&mut bus);
            assert_eq!(read_scsi_register(&mut bus, 0x17), 0x1a);
            assert_eq!(
                pio_send(&mut bus, &cdb),
                if count == 0 { 0x1b } else { 0x19 }
            );
            if count != 0 {
                let (data, csr) = pio_receive(&mut bus, count);
                assert_eq!(csr, 0x1b);
                if cdb[0] == 0x25 {
                    assert_eq!(data, [0, 0, 0, 1, 0, 0, 2, 0]);
                }
            }
            assert_eq!(finish_scsi(&mut bus), 0);
        }
    }

    #[test]
    fn independent_pio_write_and_dma_read_share_the_same_disk() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[0x2a, 0, 0, 0, 0, 0, 0, 0, 1, 0]), 0x18);
        let payload: Vec<u8> = (0..512).map(|index| (index ^ (index >> 8)) as u8).collect();
        assert_eq!(pio_send(&mut bus, &payload), 0x1b);
        assert_eq!(finish_scsi(&mut bus), 0);

        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0]), 0x19);
        write_scsi_register(&mut bus, 1, 0x80);
        scsi_count(&mut bus, 512);
        simple_scsi_command(&mut bus, 0x20);
        assert!(!bus.wd33c93b.interrupt_asserted());
        // The HPC may be started after the WD has already asserted its request.
        configure_single_scsi_descriptor(&mut bus, 0x2000);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x1b);
        assert_eq!(read_memory(&bus, 0x2000, 512), payload);
        assert_eq!(finish_scsi(&mut bus), 0);
    }

    #[test]
    fn six_byte_commands_read_modify_write_disk_sectors_and_read_cdrom() {
        let cdrom: Vec<u8> = (0..2048).map(|offset| (offset / 512) as u8).collect();
        let mut bus = bus_with_disk_and_cdrom(vec![0; 1024], cdrom);

        // Follow the label writer's initial read before updating sectors 0 and 1.
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[8, 0, 0, 0, 1, 0]), 0x19);
        let (original, csr) = pio_receive(&mut bus, 512);
        assert_eq!(original, vec![0; 512]);
        assert_eq!(csr, 0x1b);
        assert_eq!(finish_scsi(&mut bus), 0);

        for sector in 0..2 {
            select_scsi(&mut bus, 1);
            assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
            assert_eq!(pio_send(&mut bus, &[0x0a, 0, 0, sector, 1, 0]), 0x18);
            assert_eq!(pio_send(&mut bus, &[0x5a + sector; 512]), 0x1b);
            assert_eq!(finish_scsi(&mut bus), 0);
        }

        configure_scsi_descriptor_chain(&mut bus, 0x1000, 0x2000, &[512, 512]);
        issue_scsi_command(&mut bus, 1, 0, 1024, &[8, 0, 0, 0, 2, 0]);
        service_scsi(&mut bus);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert_eq!(read_memory(&bus, 0x2000, 512), vec![0x5a; 512]);
        assert_eq!(read_memory(&bus, 0x2200, 512), vec![0x5b; 512]);

        select_scsi(&mut bus, 4);
        assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[8, 0, 0, 1, 1, 0]), 0x19);
        let (data, csr) = pio_receive(&mut bus, 512);
        assert_eq!(data, vec![1; 512]);
        assert_eq!(csr, 0x1b);
        assert_eq!(finish_scsi(&mut bus), 0);
    }

    #[test]
    fn unsupported_lun_is_not_a_selection_timeout() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x81]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[0x12, 0, 0, 0, 36, 0]), 0x19);
        let (inquiry, csr) = pio_receive(&mut bus, 36);
        assert_eq!(inquiry[0], 0x7f);
        assert_eq!(csr, 0x1b);
        assert_eq!(finish_scsi(&mut bus), 0);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x81]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[0; 6]), 0x1b);
        assert_eq!(finish_scsi(&mut bus), 2);
    }

    #[test]
    fn pio_snapshot_and_debug_reads_preserve_the_unconsumed_byte() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[0x12, 0, 0, 0, 36, 0]), 0x19);
        scsi_count(&mut bus, 36);
        simple_scsi_command(&mut bus, 0x20);
        write_scsi_register(&mut bus, 0x1f, 0);
        bus.write(PhysAddr::new(SCSI_ADDRESS_PORT), &[0x19])
            .unwrap();
        let snapshot = bus.snapshot().unwrap();
        for _ in 0..2 {
            let mut byte = [0xff];
            bus.debug_read(PhysAddr::new(SCSI_DATA_PORT), &mut byte)
                .unwrap();
            assert_eq!(byte, [0]);
        }
        assert_eq!(read_scsi_register(&mut bus, 0x19), 0);
        service_scsi(&mut bus);
        assert_eq!(read_scsi_register(&mut bus, 0x14), 34);
        bus.restore_snapshot(snapshot).unwrap();
        assert_eq!(read_scsi_register(&mut bus, 0x14), 35);
        assert_eq!(read_scsi_register(&mut bus, 0x19), 0);
        service_scsi(&mut bus);
        assert_eq!(read_scsi_register(&mut bus, 0x14), 34);
    }

    #[test]
    fn disabled_selection_timeout_is_cancelled_by_abort_and_reset() {
        let mut bus = bus();
        read_scsi_register(&mut bus, 0x17);
        write_scsi_register(&mut bus, 0x15, 2);
        simple_scsi_command(&mut bus, 6);
        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND),
            &mut MachineOutput::default(),
        );
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x20));
        simple_scsi_command(&mut bus, 1);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x22);
        write_scsi_register(&mut bus, 2, 1);
        simple_scsi_command(&mut bus, 6);
        simple_scsi_command(&mut bus, 0);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0);
        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND),
            &mut MachineOutput::default(),
        );
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0));
    }

    #[test]
    fn zero_count_transfers_one_byte_and_selection_without_atn_skips_messages() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        read_scsi_register(&mut bus, 0x17);
        write_scsi_register(&mut bus, 0x15, 1);
        simple_scsi_command(&mut bus, 7);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x11);
        service_scsi(&mut bus);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x8a);
        for index in 0..6 {
            scsi_count(&mut bus, 0);
            simple_scsi_command(&mut bus, 0x20);
            assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x21));
            write_scsi_register(&mut bus, 0x19, 0);
            service_scsi(&mut bus);
            assert_eq!(
                read_scsi_register(&mut bus, 0x17),
                if index == 5 { 0x1b } else { 0x1a }
            );
            assert_eq!(read_scsi_register(&mut bus, 0x14), 0);
        }
        assert_eq!(finish_scsi(&mut bus), 0);
    }

    #[test]
    fn pio_storage_failures_preserve_residuals_without_fabricating_data() {
        for write in [false, true] {
            let mut bus = bus_with_disk_failures(vec![0; 512], !write, write);
            select_scsi(&mut bus, 1);
            assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
            assert_eq!(
                pio_send(
                    &mut bus,
                    &[if write { 0x2a } else { 0x28 }, 0, 0, 0, 0, 0, 0, 0, 1, 0]
                ),
                if write { 0x18 } else { 0x19 }
            );
            scsi_count(&mut bus, 512);
            simple_scsi_command(&mut bus, 0x20);
            if write {
                write_scsi_register(&mut bus, 0x19, 0x5a);
                service_scsi(&mut bus);
            }
            assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x80));
            assert_eq!(read_scsi_register(&mut bus, 0x17), 0x4b);
            assert_eq!(read_scsi_register(&mut bus, 0x13), 2);
            assert_eq!(read_scsi_register(&mut bus, 0x14), 0);
            assert_eq!(finish_scsi(&mut bus), 2);
            assert!(!bus.error_interrupt_asserted());
        }
    }

    #[test]
    fn abort_drains_prefetched_input_and_disconnect_releases_the_transaction() {
        let mut bus = bus_with_disk(vec![0x5a; 512], false);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0]), 0x19);
        scsi_count(&mut bus, 512);
        simple_scsi_command(&mut bus, 0x20);
        simple_scsi_command(&mut bus, 1);
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0x21));
        assert_eq!(read_scsi_register(&mut bus, 0x19), 0x5a);
        service_scsi(&mut bus);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x29);
        assert_eq!(read_scsi_register(&mut bus, 0x13), 1);
        assert_eq!(read_scsi_register(&mut bus, 0x14), 255);
        assert_eq!(bus.scsi_bus.active_address(), Some((1, 0)));
        simple_scsi_command(&mut bus, 4);
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0));
        assert_eq!(bus.scsi_bus.phase(), None);
        assert_eq!(bus.scsi_bus.active_address(), None);
    }

    #[test]
    fn attention_rejects_a_message_and_abort_message_releases_the_bus() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80, 1, 3, 1, 25, 8]), 0x1f);
        simple_scsi_command(&mut bus, 0xa0);
        assert_eq!(read_scsi_register(&mut bus, 0x19), 1);
        service_scsi(&mut bus);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x20);
        simple_scsi_command(&mut bus, 2);
        assert_eq!(read_byte(&mut bus, SCSI_ADDRESS_PORT), Ok(0));
        simple_scsi_command(&mut bus, 3);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x8e);
        assert_eq!(pio_send(&mut bus, &[7]), 0x1a);
        simple_scsi_command(&mut bus, 2);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x8e);
        assert_eq!(pio_send(&mut bus, &[6]), 0x85);
        assert_eq!(bus.scsi_bus.phase(), None);
    }

    #[test]
    fn dma_write_can_continue_in_pio_with_a_short_final_response() {
        let mut bus = bus_with_disk(vec![0; 512], false);
        configure_scsi_write_descriptor_chain(&mut bus, 0x1000, 0x2000, &[256]);
        write_memory(&mut bus, 0x2000, &[0xa5; 256]);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[0x2a, 0, 0, 0, 0, 0, 0, 0, 1, 0]), 0x18);
        write_scsi_register(&mut bus, 1, 0x80);
        scsi_count(&mut bus, 256);
        simple_scsi_command(&mut bus, 0x20);
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x18);
        assert_eq!(pio_send(&mut bus, &[0x5a; 256]), 0x1b);
        assert_eq!(finish_scsi(&mut bus), 0);
        select_scsi(&mut bus, 1);
        assert_eq!(pio_send(&mut bus, &[0x80]), 0x1a);
        assert_eq!(pio_send(&mut bus, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0]), 0x19);
        scsi_count(&mut bus, 1024);
        simple_scsi_command(&mut bus, 0x20);
        for index in 0..512 {
            assert_eq!(
                read_scsi_register(&mut bus, 0x19),
                if index < 256 { 0xa5 } else { 0x5a }
            );
            service_scsi(&mut bus);
        }
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x4b);
        assert_eq!(read_scsi_register(&mut bus, 0x13), 2);
        assert_eq!(finish_scsi(&mut bus), 0);
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
}
