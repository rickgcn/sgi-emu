use se_core::bus::BusFault;
use se_core::time::VirtualDuration;
use se_device::nmc93cs46::Nmc93cs46;
use se_device::scsi::{ScsiCommandStart, ScsiDataDirection, ScsiTransferResult};
use se_device::z85230::Z85230;

use super::super::events::EventKind;
use super::Ip12Bus;

const CPU_AUX_OUTPUT_BITS: u8 = 0x0f;
const SCSI_INTERRUPT: u8 = 1;
const SERIAL_INTERRUPT: u8 = 1 << 5;

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

    pub(super) fn handle_scsi_register_write(&mut self) {
        if self.wd33c93b.take_reset_completion() {
            self.pending_scsi = None;
            self.scsi_bus.cancel_transaction();
            self.events.schedule(EventKind::Scsi, None);
        }
        if let Some(request) = self.wd33c93b.take_select_and_transfer_request() {
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
        self.synchronize_scsi_interrupt();
    }

    pub(super) fn process_scsi_event(&mut self) {
        let Some(request) = self.pending_scsi.take() else {
            return;
        };
        let address = (request.destination_id(), request.lun());
        match self.scsi_bus.active_address() {
            Some(active_address) if active_address == address => {
                self.transfer_active_scsi_data(request.transfer_count());
            }
            Some(_) => self.hpc1.stop_scsi_dma(),
            None => match self.scsi_bus.start_command(
                request.destination_id(),
                request.lun(),
                request.cdb(),
            ) {
                Ok(ScsiCommandStart::SelectionTimeout) => {
                    self.wd33c93b.finish_selection_timeout();
                }
                Ok(ScsiCommandStart::Complete { status }) => {
                    self.wd33c93b.finish_select_and_transfer(status.byte());
                }
                Ok(ScsiCommandStart::DataIn { .. }) => {
                    self.transfer_scsi_data_in(request.transfer_count());
                }
                Ok(ScsiCommandStart::DataOut { .. }) => {
                    self.transfer_scsi_data_out(request.transfer_count());
                }
                Err(_) => self.hpc1.stop_scsi_dma(),
            },
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
            self.wd33c93b.request_data_in_continuation();
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
                        self.wd33c93b.request_data_in_continuation();
                        return;
                    }
                }
                Ok(ScsiTransferResult::Complete { status, .. }) => {
                    self.hpc1.finish_scsi_dma();
                    self.wd33c93b.finish_select_and_transfer(status.byte());
                    return;
                }
            }
        }
    }

    fn transfer_scsi_data_out(&mut self, mut wd_bytes_remaining: u32) {
        if wd_bytes_remaining == 0 {
            self.hpc1.finish_scsi_dma();
            self.wd33c93b.request_data_out_continuation();
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
                        self.wd33c93b.request_data_out_continuation();
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
                    self.wd33c93b.finish_select_and_transfer(status.byte());
                    return;
                }
            }
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

pub(super) fn read_cpu_aux_control(
    value: u8,
    nvram: &Nmc93cs46,
    data: &mut [u8],
) -> Result<(), BusFault> {
    if data.len() != 1 {
        return Err(BusFault::UnsupportedAccess);
    }
    data[0] = value & CPU_AUX_OUTPUT_BITS | u8::from(nvram.data_out()) << 4;
    Ok(())
}

pub(super) fn write_cpu_aux_control(
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

#[cfg(test)]
mod tests {
    use se_core::bus::{DeviceAddr, PhysAddr, PhysicalBus};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use crate::output::MachineOutput;
    use crate::serial::SerialPort;

    use super::Ip12Bus;

    use super::super::address::{
        HPC1_SCSI_CONTROL_BASE, HPC1_SCSI_REGISTERS_BASE, INT2_BASE, SCSI_BASE, SERIAL_1_BASE,
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
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok((1 << 5) | 1));
        assert!(bus.local_interrupt_0_asserted());

        assert_eq!(read_byte(&mut bus, SERIAL_1_BASE + 0x0f), Ok(b'A'));
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(1));
        assert!(!bus.local_interrupt_0_asserted());
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
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok((1 << 5) | 1));
        assert_eq!(read_byte(&mut bus, SERIAL_1_BASE + 0x0f), Ok(0xa5));
        assert!(!bus.local_interrupt_0_asserted());
    }

    #[test]
    fn hpc1_scsi_reset_reaches_the_wd33c93b() {
        let mut bus = bus();
        bus.write(PhysAddr::new(SCSI_BASE), &[2]).unwrap();
        bus.write(PhysAddr::new(SCSI_BASE + 4), &[0xa5]).unwrap();
        bus.write(PhysAddr::new(HPC1_SCSI_CONTROL_BASE + 3), &[1])
            .unwrap();
        bus.write(PhysAddr::new(SCSI_BASE), &[2]).unwrap();

        assert_eq!(read_byte(&mut bus, SCSI_BASE + 4), Ok(0xa5));
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0x80));
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
    fn scsi_read_ten_moves_storage_through_hpc_dma_and_interrupts_int2() {
        let disk: Vec<u8> = (0..512).map(|index| index as u8).collect();
        let mut bus = bus_with_disk(disk.clone(), false);
        configure_single_scsi_descriptor(&mut bus, 0x2000);

        issue_read_ten(&mut bus, 1, 0);
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0x30));
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
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(1));
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0x16);
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(0));
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
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0x30));
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
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0x30));
    }

    #[test]
    fn absent_targets_and_wrong_luns_complete_with_selection_timeout() {
        for (target, lun) in [(1, 0), (4, 0), (4, 1)] {
            let mut bus = if lun == 0 {
                bus()
            } else {
                bus_with_cdrom(vec![0; 2048], false)
            };
            write_scsi_register(&mut bus, 0x17, 0);
            issue_scsi_command(&mut bus, target, lun, 0, &[0, 0, 0, 0, 0, 0]);
            let mut output = MachineOutput::default();

            bus.advance_time(VirtualDuration::ZERO, &mut output);

            assert_eq!(read_scsi_register(&mut bus, 0x17), 0x42);
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
        assert_eq!(read_byte(&mut bus, SCSI_BASE), Ok(0x30));
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
