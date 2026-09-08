use se_core::time::VirtualDuration;
use se_device::z85230::Channel;

use crate::output::MachineOutput;
use crate::serial::SerialPort;

use super::super::events::EventKind;
use super::Ip12Bus;

impl Ip12Bus {
    pub(in super::super) fn advance_time(
        &mut self,
        elapsed: VirtualDuration,
        output: &mut MachineOutput,
    ) {
        self.events.advance(elapsed);
        while let Some(kind) = self.events.take_due() {
            match kind {
                EventKind::Int2 => {
                    self.synchronize_int2_time();
                    self.reschedule_int2();
                }
                EventKind::Rtc => self.synchronize_rtc_time(),
                EventKind::Hpc1Time => self.synchronize_hpc1_time(),
                EventKind::Ethernet => {
                    self.synchronize_ethernet_time();
                    self.service_ethernet_request();
                    self.reschedule_ethernet();
                }
                EventKind::Serial0 => self.synchronize_serial_time(0, output),
                EventKind::Serial1 => self.synchronize_serial_time(1, output),
                EventKind::Scsi => {
                    let _ = self.events.synchronize(EventKind::Scsi);
                    self.process_scsi_event();
                }
                EventKind::Gio => {
                    self.synchronize_gio_time();
                    self.reschedule_gio();
                }
            }
        }
        for frame in self.ethernet_output.drain(..) {
            output.push_ethernet(frame);
        }
        if let Some(video) = self.take_video_output_update() {
            output.publish_video(video);
        }
    }

    pub(super) fn schedule_timed_devices(&mut self) {
        self.reschedule_int2();
        self.events
            .schedule(EventKind::Rtc, self.rtc.time_until_event());
        self.events.schedule(EventKind::Hpc1Time, None);
        self.reschedule_serial(0);
        self.reschedule_serial(1);
        self.events.schedule(EventKind::Scsi, None);
        self.reschedule_ethernet();
        self.synchronize_gio_interrupts();
        self.reschedule_gio();
    }

    /// Advances the GIO bus and transfers its output pins.
    pub(super) fn synchronize_gio_time(&mut self) {
        let elapsed = self.events.synchronize(EventKind::Gio);
        self.gio.advance_time(elapsed);
        self.synchronize_gio_interrupts();
    }

    /// Schedules the next event produced by an attached GIO device.
    pub(super) fn reschedule_gio(&mut self) {
        self.events
            .schedule(EventKind::Gio, self.gio.time_until_event());
    }

    pub(super) fn synchronize_int2_time(&mut self) {
        let elapsed = self.events.synchronize(EventKind::Int2);
        self.int2.advance_time(elapsed);
    }

    pub(super) fn reschedule_int2(&mut self) {
        self.events
            .schedule(EventKind::Int2, self.int2.time_until_event());
    }

    pub(super) fn synchronize_rtc_time(&mut self) {
        let elapsed = self.events.synchronize(EventKind::Rtc);
        self.rtc.advance_time(elapsed);
        self.events
            .schedule(EventKind::Rtc, self.rtc.time_until_event());
    }

    pub(super) fn synchronize_hpc1_time(&mut self) {
        let elapsed = self.events.synchronize(EventKind::Hpc1Time);
        self.hpc1.advance_time(elapsed);
    }

    pub(super) fn synchronize_ethernet_time(&mut self) {
        self.synchronize_hpc1_time();
        let elapsed = self.events.synchronize(EventKind::Ethernet);
        self.seeq8003.advance_time(elapsed);
        self.transfer_ethernet_signals();
        self.reschedule_ethernet();
    }

    pub(super) fn reschedule_ethernet(&mut self) {
        let after = [
            self.seeq8003.time_until_event(),
            self.hpc1
                .ethernet_time_until_event(self.seeq8003.transmit_ready()),
        ]
        .into_iter()
        .flatten()
        .min();
        self.events.schedule(EventKind::Ethernet, after);
    }

    fn synchronize_serial_time(&mut self, index: usize, output: &mut MachineOutput) {
        let kind = serial_event_kind(index);
        let elapsed = self.events.synchronize(kind);
        self.serial[index].advance_time(elapsed, |channel, value| {
            if index == 1 {
                let port = match channel {
                    Channel::A => SerialPort::A,
                    Channel::B => SerialPort::B,
                };
                output.push_serial(port, value);
            }
        });
        self.synchronize_serial_interrupt();
        self.reschedule_serial(index);
    }

    pub(super) fn synchronize_serial_for_mmio(&mut self, index: usize) {
        let kind = serial_event_kind(index);
        let elapsed = self.events.synchronize(kind);
        let mut produced_output = false;
        self.serial[index].advance_time(elapsed, |_, _| produced_output = true);
        debug_assert!(
            !produced_output,
            "serial deadline must be dispatched before a later MMIO access"
        );
        self.reschedule_serial(index);
    }

    pub(super) fn reschedule_serial(&mut self, index: usize) {
        self.events.schedule(
            serial_event_kind(index),
            self.serial[index].time_until_event(),
        );
    }
}

const fn serial_event_kind(index: usize) -> EventKind {
    match index {
        0 => EventKind::Serial0,
        1 => EventKind::Serial1,
        _ => panic!("IP12 has exactly two serial controllers"),
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusError, DeviceAddr, PhysAddr, PhysicalBus};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
    use se_device::gio::{
        GioBus, GioDevice, GioDeviceSnapshot, GioDisplayState, GioInterrupt, GioSlot,
    };

    use crate::output::MachineOutput;
    use crate::serial::SerialPort;

    use super::super::address::{
        GIO_GRAPHICS_BASE, HPC1_COUNTER_BASE, HPC1_ETHERNET_TIMER_BASE, INT2_BASE, RTC_BASE,
        SERIAL_0_BASE, SERIAL_1_BASE,
    };
    use super::super::test_support::{
        bus, bus_with_gio, configure_serial_a, read_byte, read_scsi_register, read_word,
    };

    use super::EventKind;

    const ATTOSECONDS_PER_MICROSECOND: u128 = ATTOSECONDS_PER_SECOND / 1_000_000;
    const TIMER_ACKNOWLEDGE: u64 = INT2_BASE + 0x23;
    const TIMER_COUNTER_0: u64 = INT2_BASE + 0x33;
    const TIMER_COUNTER_1: u64 = INT2_BASE + 0x37;
    const TIMER_COUNTER_2: u64 = INT2_BASE + 0x3b;
    const TIMER_CONTROL: u64 = INT2_BASE + 0x3f;
    const LOCAL_INTERRUPT_0_STATUS: u64 = INT2_BASE;
    const LOCAL_INTERRUPT_1_STATUS: u64 = INT2_BASE + 0x08;
    const VME_INTERRUPT_STATUS: u64 = INT2_BASE + 0x10;
    const OUTPUT_PORT: u64 = INT2_BASE + 0x1c;
    const TEST_DEVICE_BASE: u64 = 0x100;
    const TEST_DEVICE_PHYSICAL_BASE: u64 = GIO_GRAPHICS_BASE + TEST_DEVICE_BASE;
    const RETRACE_BOUNDARY: VirtualDuration = VirtualDuration::from_attoseconds(10);

    struct TimedInterruptDevice {
        enabled: bool,
        asserted: bool,
        interrupt: GioInterrupt,
    }

    impl TimedInterruptDevice {
        const fn new(interrupt: GioInterrupt) -> Self {
            Self {
                enabled: true,
                asserted: false,
                interrupt,
            }
        }
    }

    impl GioDevice for TimedInterruptDevice {
        fn reset(&mut self) {
            self.enabled = true;
            self.asserted = false;
        }

        fn debug_read(&self, _address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            data.fill(0);
            Ok(())
        }

        fn read(&mut self, _address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            data.fill(0);
            Ok(())
        }

        fn write(&mut self, _address: DeviceAddr, _data: &[u8]) -> Result<(), BusError> {
            self.enabled = false;
            self.asserted = false;
            Ok(())
        }

        fn advance_time(&mut self, elapsed: VirtualDuration) {
            if self.enabled && elapsed == RETRACE_BOUNDARY {
                self.asserted = !self.asserted;
            }
        }

        fn time_until_event(&self) -> Option<VirtualDuration> {
            self.enabled.then_some(RETRACE_BOUNDARY)
        }

        fn interrupt_asserted(&self, interrupt: GioInterrupt) -> bool {
            self.asserted && interrupt == self.interrupt
        }

        fn display_state(&self) -> Option<GioDisplayState> {
            None
        }

        fn take_display_update(&mut self) -> bool {
            false
        }

        fn snapshot(&self) -> GioDeviceSnapshot {
            panic!("this test device is never snapshotted")
        }

        fn accepts_snapshot(&self, _snapshot: &GioDeviceSnapshot) -> bool {
            false
        }

        fn restore_snapshot(&mut self, _snapshot: GioDeviceSnapshot) {
            panic!("this test device rejects every snapshot")
        }
    }

    fn bus_with_timed_interrupt(interrupt: GioInterrupt) -> super::Ip12Bus {
        let mut gio = GioBus::new();
        gio.attach(
            GioSlot::Graphics,
            Box::new(TimedInterruptDevice::new(interrupt)),
        )
        .unwrap();
        bus_with_gio(gio)
    }

    fn configure_timer(bus: &mut super::Ip12Bus, control: u8, address: u64, reload: u16) {
        bus.write(PhysAddr::new(TIMER_CONTROL), &[control]).unwrap();
        for value in reload.to_le_bytes() {
            bus.write(PhysAddr::new(address), &[value]).unwrap();
        }
    }

    #[test]
    fn a_gio_device_can_withdraw_retrace_and_cancel_its_deadline() {
        let mut bus = bus_with_timed_interrupt(GioInterrupt::Interrupt2);
        let mut output = MachineOutput::default();
        bus.write(PhysAddr::new(OUTPUT_PORT + 3), &[0x08]).unwrap();
        let blanking_start = bus.gio.time_until_event().unwrap();

        bus.advance_time(blanking_start, &mut output);

        assert_eq!(read_word(&mut bus, VME_INTERRUPT_STATUS), Ok(0));
        assert_eq!(read_word(&mut bus, LOCAL_INTERRUPT_1_STATUS), Ok(0x80));
        bus.write(PhysAddr::new(TEST_DEVICE_PHYSICAL_BASE), &[0])
            .unwrap();
        assert_eq!(read_word(&mut bus, VME_INTERRUPT_STATUS), Ok(1));
        assert_eq!(read_word(&mut bus, LOCAL_INTERRUPT_1_STATUS), Ok(0x80));
        assert!(!bus.events.has_deadline(EventKind::Gio));

        bus.advance_time(blanking_start, &mut output);
        assert_eq!(read_word(&mut bus, VME_INTERRUPT_STATUS), Ok(1));
        assert_eq!(read_word(&mut bus, LOCAL_INTERRUPT_1_STATUS), Ok(0x80));
    }

    #[test]
    fn gio_interrupt_levels_zero_and_one_reach_their_distinct_int2_inputs() {
        for (interrupt, expected_status) in [
            (GioInterrupt::Interrupt0, 1),
            (GioInterrupt::Interrupt1, 1 << 6),
        ] {
            let mut bus = bus_with_timed_interrupt(interrupt);
            let baseline = read_word(&mut bus, LOCAL_INTERRUPT_0_STATUS).unwrap();
            assert_eq!(baseline & expected_status, 0);
            bus.advance_time(RETRACE_BOUNDARY, &mut MachineOutput::default());

            assert_eq!(
                read_word(&mut bus, LOCAL_INTERRUPT_0_STATUS),
                Ok(baseline | expected_status)
            );
            assert_eq!(read_word(&mut bus, LOCAL_INTERRUPT_1_STATUS), Ok(0));
        }
    }

    #[test]
    fn an_empty_gio_bus_has_no_retrace_or_deadline() {
        let mut bus = bus();
        bus.reset();
        bus.write(PhysAddr::new(OUTPUT_PORT + 3), &[0x08]).unwrap();

        assert_eq!(read_word(&mut bus, VME_INTERRUPT_STATUS), Ok(1));
        assert_eq!(read_word(&mut bus, LOCAL_INTERRUPT_1_STATUS), Ok(0));
        assert!(!bus.events.has_deadline(EventKind::Gio));

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND),
            &mut MachineOutput::default(),
        );
        assert_eq!(read_word(&mut bus, VME_INTERRUPT_STATUS), Ok(1));
        assert_eq!(read_word(&mut bus, LOCAL_INTERRUPT_1_STATUS), Ok(0));
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
    fn machine_time_advances_the_rtc_without_connecting_its_interrupt_to_int2() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        assert_eq!(read_scsi_register(&mut bus, 0x17), 0);
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(0));
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
    fn hpc1_time_synchronizes_lazily_on_normal_mmio() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        let mut counter = [0; 4];

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        bus.debug_read(PhysAddr::new(HPC1_COUNTER_BASE), &mut counter)
            .unwrap();
        assert_eq!(u32::from_be_bytes(counter), 0);
        assert_eq!(read_word(&mut bus, HPC1_COUNTER_BASE), Ok(33));

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        bus.debug_read(PhysAddr::new(HPC1_COUNTER_BASE), &mut counter)
            .unwrap();
        assert_eq!(u32::from_be_bytes(counter), 33);
        assert_eq!(read_word(&mut bus, HPC1_COUNTER_BASE), Ok(66));
    }

    #[test]
    fn hpc1_time_advances_the_ethernet_timer_with_the_reference_counter() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        bus.write(
            PhysAddr::new(HPC1_ETHERNET_TIMER_BASE),
            &(100_u32 << 4).to_be_bytes(),
        )
        .unwrap();

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        let mut timer = [0; 4];
        bus.debug_read(PhysAddr::new(HPC1_ETHERNET_TIMER_BASE), &mut timer)
            .unwrap();
        assert_eq!(u32::from_be_bytes(timer), 100 << 4);
        assert_eq!(read_word(&mut bus, HPC1_ETHERNET_TIMER_BASE), Ok(67 << 4));
        assert_eq!(read_word(&mut bus, HPC1_COUNTER_BASE), Ok(33));
    }

    #[test]
    fn reset_clears_the_hpc1_counter_and_its_synchronization_origin() {
        let mut bus = bus();
        let mut output = MachineOutput::default();

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        assert_eq!(read_word(&mut bus, HPC1_COUNTER_BASE), Ok(33));
        bus.reset();
        assert_eq!(read_word(&mut bus, HPC1_COUNTER_BASE), Ok(0));

        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        assert_eq!(read_word(&mut bus, HPC1_COUNTER_BASE), Ok(33));
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
    fn ide_counter_1_sequence_uses_the_physical_ports_and_event_scheduler() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        configure_timer(&mut bus, 0xb4, TIMER_COUNTER_2, 1_000);
        configure_timer(&mut bus, 0x74, TIMER_COUNTER_1, 2_000);

        bus.advance_time(
            VirtualDuration::from_attoseconds(2 * ATTOSECONDS_PER_SECOND - 1),
            &mut output,
        );
        assert!(!bus.timer_1_interrupt_asserted());
        bus.advance_time(VirtualDuration::from_attoseconds(1), &mut output);
        assert!(!bus.timer_1_interrupt_asserted());
        bus.advance_time(
            VirtualDuration::from_attoseconds(ATTOSECONDS_PER_SECOND / 1_000 - 1),
            &mut output,
        );
        assert!(!bus.timer_1_interrupt_asserted());
        bus.advance_time(VirtualDuration::from_attoseconds(1), &mut output);
        assert!(bus.timer_1_interrupt_asserted());

        bus.write(PhysAddr::new(TIMER_ACKNOWLEDGE), &[2]).unwrap();
        assert!(!bus.timer_1_interrupt_asserted());
        bus.advance_time(
            VirtualDuration::from_attoseconds(2 * ATTOSECONDS_PER_SECOND),
            &mut output,
        );
        assert!(bus.timer_1_interrupt_asserted());
    }

    #[test]
    fn failed_timer_mmio_preserves_the_rescheduled_deadline() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        configure_timer(&mut bus, 0xb4, TIMER_COUNTER_2, 3);
        configure_timer(&mut bus, 0x74, TIMER_COUNTER_1, 2);
        bus.advance_time(
            VirtualDuration::from_attoseconds(2 * ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );

        assert_eq!(
            bus.write(PhysAddr::new(TIMER_CONTROL), &[0x76]),
            Err(BusError::UnimplementedAccess)
        );
        bus.advance_time(
            VirtualDuration::from_attoseconds(7 * ATTOSECONDS_PER_MICROSECOND - 1),
            &mut output,
        );
        assert!(!bus.timer_1_interrupt_asserted());
        bus.advance_time(VirtualDuration::from_attoseconds(1), &mut output);
        assert!(bus.timer_1_interrupt_asserted());
    }

    #[test]
    fn reprogramming_replaces_the_old_timer_deadline() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        configure_timer(&mut bus, 0xb4, TIMER_COUNTER_2, 3);
        configure_timer(&mut bus, 0x74, TIMER_COUNTER_1, 2);
        bus.advance_time(
            VirtualDuration::from_attoseconds(2 * ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        configure_timer(&mut bus, 0x74, TIMER_COUNTER_1, 4);

        bus.advance_time(
            VirtualDuration::from_attoseconds(4 * ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        assert!(!bus.timer_1_interrupt_asserted());
        bus.advance_time(
            VirtualDuration::from_attoseconds(9 * ATTOSECONDS_PER_MICROSECOND - 1),
            &mut output,
        );
        assert!(!bus.timer_1_interrupt_asserted());
        bus.advance_time(VirtualDuration::from_attoseconds(1), &mut output);
        assert!(bus.timer_1_interrupt_asserted());
    }

    #[test]
    fn acknowledgement_and_control_follow_due_event_ordering() {
        let mut acknowledged = bus();
        let mut output = MachineOutput::default();
        configure_timer(&mut acknowledged, 0xb4, TIMER_COUNTER_2, 3);
        configure_timer(&mut acknowledged, 0x74, TIMER_COUNTER_1, 2);
        acknowledged.advance_time(
            VirtualDuration::from_attoseconds(9 * ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        acknowledged
            .write(PhysAddr::new(TIMER_ACKNOWLEDGE), &[2])
            .unwrap();
        assert!(!acknowledged.timer_1_interrupt_asserted());
        acknowledged.advance_time(
            VirtualDuration::from_attoseconds(6 * ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        assert!(acknowledged.timer_1_interrupt_asserted());

        let mut quiesced = bus();
        configure_timer(&mut quiesced, 0xb4, TIMER_COUNTER_2, 3);
        configure_timer(&mut quiesced, 0x74, TIMER_COUNTER_1, 2);
        quiesced.advance_time(
            VirtualDuration::from_attoseconds(6 * ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        quiesced
            .write(PhysAddr::new(TIMER_CONTROL), &[0x78])
            .unwrap();
        quiesced.advance_time(
            VirtualDuration::from_attoseconds(3 * ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );
        assert!(!quiesced.timer_1_interrupt_asserted());
    }

    #[test]
    fn timer_debug_reads_do_not_synchronize_a_due_event() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        configure_timer(&mut bus, 0xb4, TIMER_COUNTER_2, 3);
        configure_timer(&mut bus, 0x74, TIMER_COUNTER_1, 2);
        bus.events.advance(VirtualDuration::from_attoseconds(
            9 * ATTOSECONDS_PER_MICROSECOND,
        ));

        assert_eq!(
            bus.debug_read(PhysAddr::new(TIMER_ACKNOWLEDGE), &mut [0]),
            Err(BusError::UnimplementedAccess)
        );
        assert!(!bus.timer_1_interrupt_asserted());

        bus.advance_time(VirtualDuration::ZERO, &mut output);
        assert!(bus.timer_1_interrupt_asserted());
    }

    #[test]
    fn reset_cancels_the_timer_event_and_clears_pending_outputs() {
        let mut bus = bus();
        let mut output = MachineOutput::default();
        configure_timer(&mut bus, 0xb4, TIMER_COUNTER_2, 3);
        configure_timer(&mut bus, 0x34, TIMER_COUNTER_0, 2);
        configure_timer(&mut bus, 0x74, TIMER_COUNTER_1, 2);

        bus.reset();
        bus.advance_time(
            VirtualDuration::from_attoseconds(100 * ATTOSECONDS_PER_MICROSECOND),
            &mut output,
        );

        assert!(!bus.timer_0_interrupt_asserted());
        assert!(!bus.timer_1_interrupt_asserted());
    }
}
