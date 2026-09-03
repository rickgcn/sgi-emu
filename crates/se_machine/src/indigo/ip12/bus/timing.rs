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
                EventKind::Hpc1Counter => self.synchronize_hpc1_counter_time(),
                EventKind::Serial0 => self.synchronize_serial_time(0, output),
                EventKind::Serial1 => self.synchronize_serial_time(1, output),
                EventKind::Scsi => {
                    let _ = self.events.synchronize(EventKind::Scsi);
                    self.process_scsi_event();
                }
            }
        }
    }

    pub(super) fn schedule_timed_devices(&mut self) {
        self.reschedule_int2();
        self.events
            .schedule(EventKind::Rtc, self.rtc.time_until_event());
        self.events.schedule(EventKind::Hpc1Counter, None);
        self.reschedule_serial(0);
        self.reschedule_serial(1);
        self.events.schedule(EventKind::Scsi, None);
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

    pub(super) fn synchronize_hpc1_counter_time(&mut self) {
        let elapsed = self.events.synchronize(EventKind::Hpc1Counter);
        self.hpc1.advance_time(elapsed);
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
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use crate::output::MachineOutput;
    use crate::serial::SerialPort;

    use super::super::address::{
        HPC1_COUNTER_BASE, INT2_BASE, RTC_BASE, SERIAL_0_BASE, SERIAL_1_BASE,
    };
    use super::super::test_support::{bus, configure_serial_a, read_byte, read_word};

    const ATTOSECONDS_PER_MICROSECOND: u128 = ATTOSECONDS_PER_SECOND / 1_000_000;
    const TIMER_ACKNOWLEDGE: u64 = INT2_BASE + 0x23;
    const TIMER_COUNTER_0: u64 = INT2_BASE + 0x33;
    const TIMER_COUNTER_1: u64 = INT2_BASE + 0x37;
    const TIMER_COUNTER_2: u64 = INT2_BASE + 0x3b;
    const TIMER_CONTROL: u64 = INT2_BASE + 0x3f;

    fn configure_timer(bus: &mut super::Ip12Bus, control: u8, address: u64, reload: u16) {
        bus.write(PhysAddr::new(TIMER_CONTROL), &[control]).unwrap();
        for value in reload.to_le_bytes() {
            bus.write(PhysAddr::new(address), &[value]).unwrap();
        }
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
        assert_eq!(read_word(&mut bus, INT2_BASE), Ok(1));
        assert_eq!(read_byte(&mut bus, RTC_BASE + 0x0f), Ok(0x30));
        bus.debug_read(PhysAddr::new(RTC_BASE + 0x0f), &mut periodic_flags)
            .unwrap();
        assert_eq!(periodic_flags, [0]);
    }

    #[test]
    fn hpc1_counter_synchronizes_lazily_on_normal_mmio() {
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
            Err(BusFault::UnsupportedAccess)
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
            Err(BusFault::UnsupportedAccess)
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
