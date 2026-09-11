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
                EventKind::Serial0 => self.synchronize_serial_time(0, |_, _| {}),
                EventKind::Serial1 => {
                    self.synchronize_serial_time(1, |channel, value| {
                        let port = match channel {
                            Channel::A => SerialPort::A,
                            Channel::B => SerialPort::B,
                        };
                        output.push_serial(port, value);
                    });
                }
                EventKind::Scsi => {
                    let _ = self.events.synchronize(EventKind::Scsi);
                    self.process_scsi_event();
                }
                EventKind::Gio => {
                    self.synchronize_gio_time();
                    self.synchronize_graphics_dma_input();
                    self.reschedule_gio();
                }
                EventKind::Pic1 => {
                    self.synchronize_gio_time();
                    self.synchronize_graphics_dma_input();
                    self.service_graphics_dma_request();
                    self.synchronize_gio_interrupts();
                    self.synchronize_graphics_dma_input();
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
        self.pic1
            .set_graphics_dma_sync_input(self.gio.dma_sync_asserted());
        self.reschedule_gio();
        self.reschedule_pic1();
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

    /// Advances PIC1 to the current machine time without servicing DMA work.
    pub(super) fn synchronize_pic1_time(&mut self) {
        let elapsed = self.events.synchronize(EventKind::Pic1);
        self.pic1.advance_time(elapsed);
    }

    /// Transfers the current GIO DMASYNC level after synchronizing PIC1.
    pub(super) fn synchronize_graphics_dma_input(&mut self) {
        self.synchronize_pic1_time();
        self.pic1
            .set_graphics_dma_sync_input(self.gio.dma_sync_asserted());
        self.reschedule_pic1();
    }

    /// Schedules the next software-visible event produced by PIC1.
    pub(super) fn reschedule_pic1(&mut self) {
        self.events
            .schedule(EventKind::Pic1, self.pic1.graphics_dma_time_until_event());
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

    fn synchronize_serial_time(&mut self, index: usize, mut output: impl FnMut(Channel, u8)) {
        let kind = serial_event_kind(index);
        let elapsed = self.events.synchronize(kind);
        if index == 0 {
            let mut remaining = elapsed.as_attoseconds();
            while remaining != 0 {
                let next = [
                    self.sgi_keyboard.time_until_event(),
                    self.sgi_mouse.time_until_event(),
                    self.serial[0].time_until_event(),
                ]
                .into_iter()
                .flatten()
                .min();
                let Some(next) = next else {
                    break;
                };
                let step = remaining.min(next.as_attoseconds());
                let elapsed = VirtualDuration::from_attoseconds(step);
                let keyboard = &mut self.sgi_keyboard;
                let mouse = &mut self.sgi_mouse;
                let serial = &mut self.serial[0];
                keyboard.advance_time(elapsed, |value| {
                    let _ = serial.receive(Channel::A, &[value]);
                });
                mouse.advance_time(elapsed, |value| {
                    let _ = serial.receive(Channel::B, &[value]);
                });
                serial.advance_time(elapsed, |channel, value| {
                    if channel == Channel::A {
                        keyboard.receive_command(value);
                    }
                });
                remaining -= step;
            }
        } else {
            self.serial[index].advance_time(elapsed, &mut output);
        }
        self.synchronize_serial_interrupt();
        self.reschedule_serial(index);
    }

    pub(super) fn synchronize_serial_for_mmio(&mut self, index: usize) {
        let mut produced_output = false;
        self.synchronize_serial_time(index, |_, _| produced_output = true);
        debug_assert!(
            !produced_output,
            "serial deadline must be dispatched before a later MMIO access"
        );
    }

    pub(super) fn reschedule_serial(&mut self, index: usize) {
        let after = if index == 0 {
            [
                self.sgi_keyboard.time_until_event(),
                self.sgi_mouse.time_until_event(),
                self.serial[0].time_until_event(),
            ]
            .into_iter()
            .flatten()
            .min()
        } else {
            self.serial[index].time_until_event()
        };
        self.events.schedule(serial_event_kind(index), after);
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
    use std::sync::{Arc, Mutex};

    use se_core::bus::{BusError, DeviceAddr, PhysAddr, PhysicalBus};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
    use se_device::gio::{
        GioBus, GioDevice, GioDeviceSnapshot, GioDisplayState, GioInterrupt, GioSlot,
    };
    use se_device::lg1::Lg1;
    use se_device::sgi_keyboard::SgiKey;
    use se_device::sgi_mouse::SgiMouseButton;
    use se_device::z85230::Channel;

    use crate::output::MachineOutput;
    use crate::serial::SerialPort;

    use super::super::address::{
        GIO_GRAPHICS_BASE, HPC1_COUNTER_BASE, HPC1_ETHERNET_TIMER_BASE, INT2_BASE, PIC1_BASE,
        RTC_BASE, SERIAL_0_BASE, SERIAL_1_BASE,
    };
    use super::super::test_support::{
        bus, bus_with_gio, configure_memory, configure_serial_a, read_byte, read_scsi_register,
        read_word, write_serial_register,
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
    const GRAPHICS_DABR: u64 = PIC1_BASE + 0xa_0000;
    const GRAPHICS_START: u64 = PIC1_BASE + 0xa_0100;
    const GRAPHICS_DMA_INTERRUPT_ENABLE: u32 = 1 << 4;
    const GRAPHICS_DMA_SYNC_ENABLE: u32 = 1 << 5;
    const GRAPHICS_DMA_STRIDE_INCREMENT: u32 = 1 << 31;
    const GRAPHICS_DMA_LAST_DESCRIPTOR: u32 = 1 << 15;
    const GRAPHICS_DMA_LINE_INCREMENT: u32 = 1 << 14;
    const GRAPHICS_DMA_GIO_TO_MEMORY: u32 = 2 << 12;
    const GRAPHICS_DMA_CYCLE: u128 = ATTOSECONDS_PER_SECOND.div_ceil(33_000_000);
    const LG1_GRAPHICS_DMA_PORT: u32 = 0x1f3f_0870;
    const LG1_REX_SET_BASE: u64 = LG1_GRAPHICS_DMA_PORT as u64 - 0x0870;
    const RETRACE_BOUNDARY: VirtualDuration = VirtualDuration::from_attoseconds(10);
    const KEYBOARD_CHARACTER_TIME: u128 = 11 * ATTOSECONDS_PER_SECOND / 600;
    const MOUSE_CHARACTER_TIME: u128 = 10 * ATTOSECONDS_PER_SECOND / 4_800;
    const DUART_LOOPBACK_CHARACTER_TIME: u128 = 11 * ATTOSECONDS_PER_SECOND / 38_400;

    struct TimedInterruptDevice {
        enabled: bool,
        asserted: bool,
        interrupt: GioInterrupt,
    }

    #[derive(Default)]
    struct DmaObservations {
        reads: Vec<(u64, usize)>,
        writes: Vec<(u64, Vec<u8>)>,
    }

    struct DmaTestDevice {
        observations: Arc<Mutex<DmaObservations>>,
        sync: Arc<Mutex<bool>>,
    }

    impl GioDevice for DmaTestDevice {
        fn reset(&mut self) {}

        fn debug_read(&self, _address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            data.fill(0);
            Ok(())
        }

        fn read(&mut self, _address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            data.fill(0);
            Ok(())
        }

        fn write(&mut self, _address: DeviceAddr, _data: &[u8]) -> Result<(), BusError> {
            Ok(())
        }

        fn read_dma(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            self.observations
                .lock()
                .unwrap()
                .reads
                .push((address.get(), data.len()));
            for (index, byte) in data.iter_mut().enumerate() {
                *byte = address.get().wrapping_add(index as u64) as u8;
            }
            Ok(())
        }

        fn write_dma(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
            self.observations
                .lock()
                .unwrap()
                .writes
                .push((address.get(), data.to_vec()));
            Ok(())
        }

        fn dma_sync_asserted(&self) -> bool {
            *self.sync.lock().unwrap()
        }

        fn advance_time(&mut self, _elapsed: VirtualDuration) {}

        fn time_until_event(&self) -> Option<VirtualDuration> {
            None
        }

        fn interrupt_asserted(&self, _interrupt: GioInterrupt) -> bool {
            false
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

        fn read_dma(&mut self, _address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            data.fill(0);
            Ok(())
        }

        fn write_dma(&mut self, _address: DeviceAddr, _data: &[u8]) -> Result<(), BusError> {
            Ok(())
        }

        fn dma_sync_asserted(&self) -> bool {
            false
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

    fn bus_with_dma_device(
        sync_asserted: bool,
    ) -> (
        super::Ip12Bus,
        Arc<Mutex<DmaObservations>>,
        Arc<Mutex<bool>>,
    ) {
        let observations = Arc::new(Mutex::new(DmaObservations::default()));
        let sync = Arc::new(Mutex::new(sync_asserted));
        let mut gio = GioBus::new();
        gio.attach(
            GioSlot::Graphics,
            Box::new(DmaTestDevice {
                observations: Arc::clone(&observations),
                sync: Arc::clone(&sync),
            }),
        )
        .unwrap();
        (bus_with_gio(gio), observations, sync)
    }

    fn write_graphics_descriptor(bus: &mut super::Ip12Bus, address: u64, words: [u32; 5]) {
        for (index, word) in words.into_iter().enumerate() {
            bus.write(
                PhysAddr::new(address + (index * 4) as u64),
                &word.to_be_bytes(),
            )
            .unwrap();
        }
    }

    fn start_graphics_dma(bus: &mut super::Ip12Bus, descriptor_address: u32) {
        bus.write(
            PhysAddr::new(GRAPHICS_DABR),
            &descriptor_address.to_be_bytes(),
        )
        .unwrap();
        bus.write(PhysAddr::new(GRAPHICS_START), &0_u32.to_be_bytes())
            .unwrap();
    }

    fn read_memory(bus: &mut super::Ip12Bus, address: u64, length: usize) -> Vec<u8> {
        let mut bytes = vec![0; length];
        for (index, chunk) in bytes.chunks_mut(4).enumerate() {
            bus.read(PhysAddr::new(address + (index * 4) as u64), chunk)
                .unwrap();
        }
        bytes
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
    fn graphics_dma_transfers_strided_memory_lines_at_timed_boundaries() {
        let (mut bus, observations, _) = bus_with_dma_device(true);
        configure_memory(&mut bus, 0x0100_023f, 0x023f_023f);
        bus.write(
            PhysAddr::new(PIC1_BASE),
            &GRAPHICS_DMA_INTERRUPT_ENABLE.to_be_bytes(),
        )
        .unwrap();
        write_graphics_descriptor(
            &mut bus,
            0x1000,
            [
                0x2000,
                GRAPHICS_DMA_STRIDE_INCREMENT
                    | (4 << 16)
                    | GRAPHICS_DMA_LAST_DESCRIPTOR
                    | GRAPHICS_DMA_LINE_INCREMENT
                    | 4,
                TEST_DEVICE_PHYSICAL_BASE as u32,
                (4 << 16) | 1,
                0,
            ],
        );
        bus.write(PhysAddr::new(0x2000), &[1, 2, 3, 4]).unwrap();
        bus.write(PhysAddr::new(0x2008), &[5, 6, 7, 8]).unwrap();

        assert!(bus.interrupt_asserted());
        start_graphics_dma(&mut bus, 0x1000);
        assert!(!bus.interrupt_asserted());
        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 5 - 1),
            &mut MachineOutput::default(),
        );
        assert!(observations.lock().unwrap().writes.is_empty());
        bus.advance_time(
            VirtualDuration::from_attoseconds(1),
            &mut MachineOutput::default(),
        );
        assert_eq!(read_word(&mut bus, PIC1_BASE + 0xa_0004), Ok(0x2000));
        assert_eq!(read_word(&mut bus, PIC1_BASE + 0xa_000c), Ok(0x1f00_0100));
        assert!(observations.lock().unwrap().writes.is_empty());

        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE),
            &mut MachineOutput::default(),
        );
        assert_eq!(
            observations.lock().unwrap().writes,
            [(TEST_DEVICE_BASE, vec![1, 2, 3, 4])]
        );
        assert!(!bus.interrupt_asserted());

        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE),
            &mut MachineOutput::default(),
        );
        assert_eq!(
            observations.lock().unwrap().writes,
            [
                (TEST_DEVICE_BASE, vec![1, 2, 3, 4]),
                (TEST_DEVICE_BASE + 8, vec![5, 6, 7, 8]),
            ]
        );
        assert_eq!(read_word(&mut bus, PIC1_BASE + 8), Ok(0x88));
        assert!(bus.interrupt_asserted());
    }

    #[test]
    fn graphics_dma_reads_gio_before_committing_a_whole_memory_line() {
        let (mut bus, observations, _) = bus_with_dma_device(true);
        configure_memory(&mut bus, 0x0100_023f, 0x023f_023f);
        write_graphics_descriptor(
            &mut bus,
            0x1000,
            [
                0x3000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | GRAPHICS_DMA_GIO_TO_MEMORY | 6,
                (TEST_DEVICE_PHYSICAL_BASE + 0x20) as u32,
                0,
                0,
            ],
        );
        start_graphics_dma(&mut bus, 0x1000);

        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 7),
            &mut MachineOutput::default(),
        );

        assert_eq!(
            observations.lock().unwrap().reads,
            [(TEST_DEVICE_BASE + 0x20, 8)]
        );
        assert_eq!(
            read_memory(&mut bus, 0x3000, 8),
            [0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27]
        );
        assert_eq!(read_word(&mut bus, PIC1_BASE + 8), Ok(0x88));
    }

    #[test]
    fn zero_width_graphics_dma_completes_without_endpoint_transactions() {
        let (mut bus, observations, _) = bus_with_dma_device(true);
        configure_memory(&mut bus, 0x0100_023f, 0x023f_023f);
        write_graphics_descriptor(
            &mut bus,
            0x1000,
            [0xffff_ffff, GRAPHICS_DMA_LAST_DESCRIPTOR, 0xffff_ffff, 0, 0],
        );
        start_graphics_dma(&mut bus, 0x1000);

        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 6),
            &mut MachineOutput::default(),
        );

        let observations = observations.lock().unwrap();
        assert!(observations.reads.is_empty());
        assert!(observations.writes.is_empty());
        drop(observations);
        assert_eq!(read_word(&mut bus, PIC1_BASE + 8), Ok(0x88));
    }

    #[test]
    fn graphics_dma_gse_waits_for_the_wired_gio_sync_level() {
        let (mut bus, observations, sync) = bus_with_dma_device(false);
        configure_memory(&mut bus, 0x0100_023f, 0x023f_023f);
        write_graphics_descriptor(
            &mut bus,
            0x1000,
            [
                0x2000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | 4,
                TEST_DEVICE_PHYSICAL_BASE as u32,
                0,
                0,
            ],
        );
        bus.write(PhysAddr::new(0x2000), &[1, 2, 3, 4]).unwrap();
        bus.write(
            PhysAddr::new(PIC1_BASE),
            &GRAPHICS_DMA_SYNC_ENABLE.to_be_bytes(),
        )
        .unwrap();
        start_graphics_dma(&mut bus, 0x1000);

        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 100),
            &mut MachineOutput::default(),
        );
        assert!(observations.lock().unwrap().writes.is_empty());
        assert_eq!(read_word(&mut bus, PIC1_BASE + 8), Ok(0x80));

        *sync.lock().unwrap() = true;
        bus.read(PhysAddr::new(TEST_DEVICE_PHYSICAL_BASE), &mut [0; 4])
            .unwrap();
        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 6),
            &mut MachineOutput::default(),
        );

        assert_eq!(
            observations.lock().unwrap().writes,
            [(TEST_DEVICE_BASE, vec![1, 2, 3, 4])]
        );
        assert_eq!(read_word(&mut bus, PIC1_BASE + 8), Ok(0x88));
    }

    #[test]
    fn graphics_dma_endpoint_failures_stop_without_reordering_side_effects() {
        let mut empty_bus = bus();
        configure_memory(&mut empty_bus, 0x0100_023f, 0x023f_023f);
        write_graphics_descriptor(
            &mut empty_bus,
            0x1000,
            [
                0x2000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | 4,
                TEST_DEVICE_PHYSICAL_BASE as u32,
                0,
                0,
            ],
        );
        empty_bus
            .write(PhysAddr::new(0x2000), &[1, 2, 3, 4])
            .unwrap();
        start_graphics_dma(&mut empty_bus, 0x1000);
        empty_bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 6),
            &mut MachineOutput::default(),
        );
        assert_eq!(read_word(&mut empty_bus, PIC1_BASE + 8), Ok(0x8c));

        let (mut read_first_bus, observations, _) = bus_with_dma_device(true);
        configure_memory(&mut read_first_bus, 0x0100_023f, 0x023f_023f);
        write_graphics_descriptor(
            &mut read_first_bus,
            0x1000,
            [
                0x0100_0000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | GRAPHICS_DMA_GIO_TO_MEMORY | 4,
                TEST_DEVICE_PHYSICAL_BASE as u32,
                0,
                0,
            ],
        );
        start_graphics_dma(&mut read_first_bus, 0x1000);
        read_first_bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 6),
            &mut MachineOutput::default(),
        );

        assert_eq!(observations.lock().unwrap().reads, [(TEST_DEVICE_BASE, 4)]);
        assert_eq!(read_word(&mut read_first_bus, PIC1_BASE + 8), Ok(0x8c));
        assert!(read_first_bus.interrupt_asserted());
    }

    #[test]
    fn graphics_dma_snapshot_preserves_the_exact_pending_deadline() {
        let mut gio = GioBus::new();
        gio.attach(GioSlot::Graphics, Box::new(Lg1::new())).unwrap();
        let mut bus = bus_with_gio(gio);
        configure_memory(&mut bus, 0x0100_023f, 0x023f_023f);
        write_graphics_descriptor(
            &mut bus,
            0x1000,
            [
                0x2000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | 4,
                LG1_GRAPHICS_DMA_PORT,
                0,
                0,
            ],
        );
        bus.write(PhysAddr::new(0x2000), &[1, 2, 3, 4]).unwrap();
        start_graphics_dma(&mut bus, 0x1000);
        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 2),
            &mut MachineOutput::default(),
        );
        let saved = bus.snapshot().unwrap();

        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 4),
            &mut MachineOutput::default(),
        );
        let expected =
            bincode::serde::encode_to_vec(bus.snapshot().unwrap(), bincode::config::standard())
                .unwrap();

        bus.restore_snapshot(saved).unwrap();
        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 4),
            &mut MachineOutput::default(),
        );
        let restored =
            bincode::serde::encode_to_vec(bus.snapshot().unwrap(), bincode::config::standard())
                .unwrap();

        assert_eq!(restored, expected);
        assert_eq!(read_word(&mut bus, PIC1_BASE + 8), Ok(0x88));
    }

    #[test]
    fn graphics_dma_crosses_lg1_rectangle_scanlines_in_both_directions() {
        let mut gio = GioBus::new();
        gio.attach(GioSlot::Graphics, Box::new(Lg1::new())).unwrap();
        let mut bus = bus_with_gio(gio);
        configure_memory(&mut bus, 0x0100_023f, 0x023f_023f);
        let transfer = (0..39)
            .map(|value| value as u8)
            .chain([0xee])
            .chain((39..78).map(|value| value as u8))
            .chain([0xff])
            .collect::<Vec<_>>();
        for (index, bytes) in transfer.chunks(4).enumerate() {
            bus.write(PhysAddr::new(0x2000 + (index * 4) as u64), bytes)
                .unwrap();
        }
        for (offset, value) in [
            (0x000c_u64, 0x10_u32),
            (0x001c, 0),
            (0x0084, 0x36),
            (0x0088, 1),
            (0x47a8, 0x2000_0000),
            (0x0000, 0x3020_00a9),
            (0x0008, 0x13ff_0000),
        ] {
            bus.write(
                PhysAddr::new(LG1_REX_SET_BASE + offset),
                &value.to_be_bytes(),
            )
            .unwrap();
        }
        write_graphics_descriptor(
            &mut bus,
            0x1000,
            [
                0x2000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | 78,
                LG1_GRAPHICS_DMA_PORT,
                0,
                0,
            ],
        );
        start_graphics_dma(&mut bus, 0x1000);
        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 25),
            &mut MachineOutput::default(),
        );

        for (offset, value) in [(0x000c_u64, 0x10_u32), (0x001c, 0), (0x0000, 0x3000_00ab)] {
            bus.write(
                PhysAddr::new(LG1_REX_SET_BASE + offset),
                &value.to_be_bytes(),
            )
            .unwrap();
        }
        write_graphics_descriptor(
            &mut bus,
            0x1100,
            [
                0x3000,
                GRAPHICS_DMA_LAST_DESCRIPTOR | GRAPHICS_DMA_GIO_TO_MEMORY | 78,
                LG1_GRAPHICS_DMA_PORT,
                0,
                0,
            ],
        );
        start_graphics_dma(&mut bus, 0x1100);
        bus.advance_time(
            VirtualDuration::from_attoseconds(GRAPHICS_DMA_CYCLE * 25),
            &mut MachineOutput::default(),
        );

        let readback = read_memory(&mut bus, 0x3000, transfer.len());
        assert_eq!(readback[..39], transfer[..39]);
        assert_eq!(readback[40..79], transfer[40..79]);
        assert_eq!(read_word(&mut bus, PIC1_BASE + 8), Ok(0x88));
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
    fn sgi_keyboard_and_mouse_reach_scc_zero_channels_a_and_b() {
        let mut bus = bus();
        write_serial_register(&mut bus, SERIAL_0_BASE, 3, 1);
        bus.write(PhysAddr::new(SERIAL_0_BASE + 0x03), &[3])
            .unwrap();
        bus.write(PhysAddr::new(SERIAL_0_BASE + 0x03), &[1])
            .unwrap();
        bus.set_sgi_key_state(SgiKey::KeyA, true);
        bus.set_sgi_mouse_button_state(SgiMouseButton::Left, true);
        let mut output = MachineOutput::default();

        bus.advance_time(
            VirtualDuration::from_attoseconds(MOUSE_CHARACTER_TIME + 1),
            &mut output,
        );
        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x07), Ok(0x83));
        bus.advance_time(
            VirtualDuration::from_attoseconds(KEYBOARD_CHARACTER_TIME - MOUSE_CHARACTER_TIME),
            &mut output,
        );
        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0f), Ok(10));
        assert!(output.is_empty());
    }

    #[test]
    fn completed_scc_zero_commands_start_keyboard_responses_at_that_boundary() {
        let mut bus = bus();
        for (register, value) in [
            (4, 0x45),
            (11, 0x10),
            (12, 190),
            (13, 0),
            (14, 1),
            (3, 1),
            (5, 0x68),
        ] {
            write_serial_register(&mut bus, SERIAL_0_BASE, register, value);
        }
        bus.write(PhysAddr::new(SERIAL_0_BASE + 0x0f), &[1 << 4])
            .unwrap();
        let mut output = MachineOutput::default();

        bus.advance_time(
            VirtualDuration::from_attoseconds(KEYBOARD_CHARACTER_TIME * 2),
            &mut output,
        );
        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0f), Ok(0x6e));
        bus.advance_time(
            VirtualDuration::from_attoseconds(KEYBOARD_CHARACTER_TIME + 1),
            &mut output,
        );
        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0f), Ok(0x00));
    }

    #[test]
    fn scc_zero_internal_loopback_discards_keyboard_responses() {
        let mut bus = bus();
        for (register, value) in [
            (4, 0x45),
            (11, 0x10),
            (12, 1),
            (13, 0),
            (14, 0x11),
            (3, 1),
            (5, 0x68),
        ] {
            write_serial_register(&mut bus, SERIAL_0_BASE, register, value);
        }
        let mut output = MachineOutput::default();

        bus.write(PhysAddr::new(SERIAL_0_BASE + 0x0f), &[0xf0])
            .unwrap();
        bus.advance_time(
            VirtualDuration::from_attoseconds(DUART_LOOPBACK_CHARACTER_TIME + 1),
            &mut output,
        );
        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0f), Ok(0xf0));

        bus.advance_time(
            VirtualDuration::from_attoseconds(KEYBOARD_CHARACTER_TIME * 2 + 2),
            &mut output,
        );
        bus.write(PhysAddr::new(SERIAL_0_BASE + 0x0f), &[0xcc])
            .unwrap();
        bus.advance_time(
            VirtualDuration::from_attoseconds(DUART_LOOPBACK_CHARACTER_TIME + 1),
            &mut output,
        );

        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0f), Ok(0xcc));
        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0f), Ok(0));
        assert!(output.is_empty());
    }

    #[test]
    fn scc_zero_command_exchange_is_invariant_under_elapsed_fragmentation() {
        fn configured_bus() -> super::super::Ip12Bus {
            let mut bus = bus();
            for (register, value) in [
                (4, 0x45),
                (11, 0x10),
                (12, 190),
                (13, 0),
                (14, 1),
                (3, 1),
                (5, 0x68),
            ] {
                write_serial_register(&mut bus, SERIAL_0_BASE, register, value);
            }
            bus.write(PhysAddr::new(SERIAL_0_BASE + 0x0f), &[1 << 4])
                .unwrap();
            bus
        }

        let mut whole = configured_bus();
        whole.advance_time(
            VirtualDuration::from_attoseconds(KEYBOARD_CHARACTER_TIME * 3 + 1),
            &mut MachineOutput::default(),
        );
        let whole_response = [
            read_byte(&mut whole, SERIAL_0_BASE + 0x0f),
            read_byte(&mut whole, SERIAL_0_BASE + 0x0f),
        ];

        let mut split = configured_bus();
        split.advance_time(
            VirtualDuration::from_attoseconds(KEYBOARD_CHARACTER_TIME),
            &mut MachineOutput::default(),
        );
        split.advance_time(
            VirtualDuration::from_attoseconds(KEYBOARD_CHARACTER_TIME * 2 + 1),
            &mut MachineOutput::default(),
        );
        let split_response = [
            read_byte(&mut split, SERIAL_0_BASE + 0x0f),
            read_byte(&mut split, SERIAL_0_BASE + 0x0f),
        ];

        assert_eq!(whole_response, [Ok(0x6e), Ok(0)]);
        assert_eq!(split_response, whole_response);
    }

    #[test]
    fn scc_zero_full_receive_fifo_drops_a_completed_keyboard_character() {
        let mut bus = bus();
        write_serial_register(&mut bus, SERIAL_0_BASE, 3, 1);
        assert_eq!(bus.serial[0].receive(Channel::A, &[0x55; 8]), 8);
        bus.set_sgi_key_state(SgiKey::KeyA, true);
        bus.advance_time(
            VirtualDuration::from_attoseconds(KEYBOARD_CHARACTER_TIME + 1),
            &mut MachineOutput::default(),
        );

        for _ in 0..8 {
            assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0f), Ok(0x55));
        }
        assert_eq!(read_byte(&mut bus, SERIAL_0_BASE + 0x0f), Ok(0));
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
