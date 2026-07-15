use super::*;
use se_core::role::BusRole;

use crate::{
    bus::two_wire::{TwoWireBus, TwoWireBusAction},
    chipset::mace::peripheral::{Ps2Port, Ps2PortAction},
};

#[test]
fn serial_frames_use_lsb_first_data_and_odd_parity() {
    for byte in 0u8..=u8::MAX {
        let frame = serial_frame(byte);
        assert_eq!(frame & 1, 0);
        assert_eq!((frame >> 1) as u8, byte);
        assert_eq!(frame >> 10, 1);
        assert_eq!(((frame >> 1) & 0x1ff).count_ones() & 1, 1);
    }
}

#[test]
fn two_to_one_scaling_uses_the_standard_small_motion_table() {
    let expected = [0, 1, 1, 3, 6, 9, 12];
    for (input, output) in expected.into_iter().enumerate() {
        assert_eq!(super::mouse::scale_2_to_1(input as i64), output);
        assert_eq!(super::mouse::scale_2_to_1(-(input as i64)), -output);
    }
}

#[test]
fn device_clock_waits_while_the_host_inhibits_the_bus() {
    let controller = ComponentId::new(1);
    let device = ComponentId::new(2);
    let bus = ComponentId::new(3);
    let mut link =
        Ps2DeviceLink::new(device, Ps2Wiring { controller, bus }, 1_000_000_000).unwrap();
    assert!(link.start_device_byte(0x5a));
    assert!(matches!(link.poll(), Some(LinkAction::Drive(_))));
    let Some(LinkAction::Schedule { event, .. }) = link.poll() else {
        panic!("device transmission must schedule its first clock edge");
    };
    link.observe_lines(TwoWireLineDelivery {
        bus,
        source: controller,
        time: SimTime::new(1),
        source_clock_low: true,
        source_data_low: false,
        clock_low: true,
        data_low: true,
    })
    .unwrap();
    let transfer = link.transfer;

    assert_eq!(link.handle_event(event), None);
    assert_eq!(link.transfer, transfer);
    let Some(LinkAction::Schedule { delay, event }) = link.poll() else {
        panic!("an inhibited device must poll for clock release");
    };
    assert_eq!(delay, SimDuration::new(100_000));

    link.observe_lines(TwoWireLineDelivery {
        bus,
        source: controller,
        time: SimTime::new(100_001),
        source_clock_low: false,
        source_data_low: false,
        clock_low: false,
        data_low: true,
    })
    .unwrap();
    assert_eq!(link.handle_event(event), None);
    assert!(matches!(
        link.transfer,
        LinkTransfer::DeviceTransmit {
            bit: 0,
            clock_low: true,
            ..
        }
    ));
}

#[test]
fn twelve_kilohertz_clock_projection_has_no_long_term_drift() {
    let mut link = Ps2DeviceLink::new(
        ComponentId::new(2),
        Ps2Wiring {
            controller: ComponentId::new(1),
            bus: ComponentId::new(3),
        },
        1_000_000_000,
    )
    .unwrap();
    let mut elapsed = 0u64;
    for _ in 0..24_000 {
        link.schedule_half_clock();
        let Some(LinkAction::Schedule { delay, .. }) = link.poll() else {
            panic!("each half clock must produce one schedule action");
        };
        elapsed += delay.get();
    }

    assert_eq!(elapsed, 1_000_000_000);
    assert_eq!(link.half_clock_remainder, 0);
}

#[test]
fn keyboard_state_round_trip_preserves_mid_frame_and_typematic_work() {
    let wiring = Ps2Wiring {
        controller: ComponentId::new(1),
        bus: ComponentId::new(3),
    };
    let mut reference =
        Ps2Keyboard::new(ComponentId::new(2), "keyboard", wiring, 1_000_000_000).unwrap();
    reference.scanning_enabled = true;
    reference.apply_input(Ps2KeyboardInput {
        key: Ps2KeyPosition::A,
        pressed: true,
    });
    assert!(matches!(reference.link.poll(), Some(LinkAction::Drive(_))));
    let Some(LinkAction::Schedule { event, .. }) = reference.link.poll() else {
        panic!("the first scan byte must schedule a clock edge");
    };
    reference.link.handle_event(event);

    let encoded = postcard::to_stdvec(&reference.save_state()).unwrap();
    let state: Ps2KeyboardState = postcard::from_bytes(&encoded).unwrap();
    let mut restored =
        Ps2Keyboard::new(ComponentId::new(2), "keyboard", wiring, 1_000_000_000).unwrap();
    restored.restore_state(state).unwrap();

    assert_eq!(
        postcard::to_stdvec(&restored.save_state()).unwrap(),
        encoded
    );
}

#[derive(Clone, Copy)]
enum TestEvent {
    Controller {
        at: SimTime,
        epoch: u64,
    },
    Keyboard {
        at: SimTime,
        event: Ps2KeyboardEvent,
    },
}

impl TestEvent {
    fn at(self) -> SimTime {
        match self {
            Self::Controller { at, .. } | Self::Keyboard { at, .. } => at,
        }
    }
}

fn drain_test_lines(
    now: SimTime,
    controller: &mut Ps2Port,
    keyboard: &mut Ps2Keyboard,
    bus: &mut TwoWireBus,
    events: &mut Vec<TestEvent>,
) {
    loop {
        let mut progressed = false;
        while let Some(action) = controller.poll() {
            progressed = true;
            match action {
                Ps2PortAction::Schedule { delay, epoch } => {
                    events.push(TestEvent::Controller {
                        at: SimTime::new(now.get().saturating_add(delay.get())),
                        epoch,
                    });
                }
                Ps2PortAction::Drive { drive, .. } => bus.route(drive).unwrap(),
            }
        }
        while let Ps2KeyboardPoll::Action(action) = keyboard.poll() {
            progressed = true;
            match action {
                Ps2KeyboardAction::Schedule { delay, event } => {
                    events.push(TestEvent::Keyboard {
                        at: SimTime::new(now.get().saturating_add(delay.get())),
                        event,
                    });
                }
                Ps2KeyboardAction::Drive(drive) => bus.route(drive).unwrap(),
            }
        }
        while let TwoWireBusAction::Deliver { target, delivery } = bus.poll() {
            progressed = true;
            if target == ComponentId::new(1) {
                controller.observe_lines(delivery);
            } else {
                keyboard.observe_lines(delivery).unwrap();
            }
        }
        if !progressed {
            break;
        }
    }
}

fn run_next_test_event(
    now: &mut SimTime,
    controller: &mut Ps2Port,
    keyboard: &mut Ps2Keyboard,
    bus: &mut TwoWireBus,
    events: &mut Vec<TestEvent>,
) -> bool {
    let Some((index, _)) = events
        .iter()
        .enumerate()
        .min_by_key(|(_, event)| event.at())
    else {
        return false;
    };
    let event = events.swap_remove(index);
    *now = event.at();
    match event {
        TestEvent::Controller { epoch, .. } => controller.handle_event(*now, epoch),
        TestEvent::Keyboard { event, .. } => keyboard.handle_event(*now, event),
    }
    drain_test_lines(*now, controller, keyboard, bus, events);
    true
}

fn exchange_host_byte(
    byte: u8,
    now: &mut SimTime,
    controller: &mut Ps2Port,
    keyboard: &mut Ps2Keyboard,
    bus: &mut TwoWireBus,
    events: &mut Vec<TestEvent>,
) -> u8 {
    controller.write_transmit(byte);
    drain_test_lines(*now, controller, keyboard, bus, events);
    for _ in 0..256 {
        if controller.status() & 0x10 != 0 {
            let response = controller.read_receive() as u8;
            drain_test_lines(*now, controller, keyboard, bus, events);
            while run_next_test_event(now, controller, keyboard, bus, events) {}
            return response;
        }
        assert!(run_next_test_event(now, controller, keyboard, bus, events));
    }
    panic!("PS/2 host transaction did not complete");
}

#[test]
fn mace_and_keyboard_exchange_ide_set_three_commands_over_open_drain_lines() {
    let controller_id = ComponentId::new(1);
    let keyboard_id = ComponentId::new(2);
    let bus_id = ComponentId::new(3);
    let mut controller = Ps2Port::new(controller_id, bus_id, 1_000_000_000);
    let mut keyboard = Ps2Keyboard::new(
        keyboard_id,
        "keyboard",
        Ps2Wiring {
            controller: controller_id,
            bus: bus_id,
        },
        1_000_000_000,
    )
    .unwrap();
    let mut bus = TwoWireBus::new(bus_id, "PS/2", [controller_id, keyboard_id]).unwrap();
    let mut events = Vec::new();
    let mut now = SimTime::ZERO;
    controller.set_control(0x12);

    for byte in [0xf5, 0xf0, 0x03] {
        assert_eq!(
            exchange_host_byte(
                byte,
                &mut now,
                &mut controller,
                &mut keyboard,
                &mut bus,
                &mut events,
            ),
            0xfa
        );
    }

    assert_eq!(keyboard.scan_set(), 3);
    assert_eq!(controller.status() & 0x38, 0x08);
}
