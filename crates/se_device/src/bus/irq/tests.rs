use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;

use super::*;

const BUS: ComponentId = ComponentId::new(1);
const SOURCE_A: ComponentId = ComponentId::new(2);
const SOURCE_B: ComponentId = ComponentId::new(3);
const TARGET_A: ComponentId = ComponentId::new(4);
const TARGET_B: ComponentId = ComponentId::new(5);
const OUTPUT: IrqOutput = IrqOutput::new(0);
const INPUT: IrqInput = IrqInput::new(2);

const fn source(component: ComponentId) -> IrqSource {
    IrqSource {
        component,
        output: OUTPUT,
    }
}

const fn target(component: ComponentId) -> IrqTarget {
    IrqTarget {
        component,
        input: INPUT,
    }
}

const fn route(source_component: ComponentId, target_component: ComponentId) -> IrqRoute {
    IrqRoute {
        source: source(source_component),
        target: target(target_component),
    }
}

fn assert_level(bus: &mut IrqBus, source_component: ComponentId, asserted: bool) {
    BusRole::route(
        bus,
        IrqTransaction {
            source: source(source_component),
            asserted,
        },
    )
    .unwrap();
}

#[test]
fn irq_bus_implements_component_and_bus_roles() {
    fn assert_roles<T: Component + BusRole<IrqTransaction>>() {}
    assert_roles::<IrqBus>();
}

#[test]
fn duplicate_routes_are_rejected() {
    let route = route(SOURCE_A, TARGET_A);
    assert_eq!(
        IrqBus::new(BUS, "IRQ", [route, route]),
        Err(IrqBusBuildError::DuplicateRoute(route))
    );
}

#[test]
fn changed_levels_are_delivered_and_duplicates_are_suppressed() {
    let mut bus = IrqBus::new(BUS, "IRQ", [route(SOURCE_A, TARGET_A)]).unwrap();

    assert_level(&mut bus, SOURCE_A, true);
    assert_eq!(
        bus.poll(),
        IrqBusAction::Deliver {
            target: TARGET_A,
            delivery: IrqDelivery {
                input: INPUT,
                asserted: true,
            },
        }
    );
    assert_level(&mut bus, SOURCE_A, true);
    assert_eq!(bus.poll(), IrqBusAction::Idle);
    assert_level(&mut bus, SOURCE_A, false);
    assert_eq!(
        bus.poll(),
        IrqBusAction::Deliver {
            target: TARGET_A,
            delivery: IrqDelivery {
                input: INPUT,
                asserted: false,
            },
        }
    );
}

#[test]
fn one_source_can_fan_out_to_multiple_targets() {
    let mut bus = IrqBus::new(
        BUS,
        "IRQ",
        [route(SOURCE_A, TARGET_A), route(SOURCE_A, TARGET_B)],
    )
    .unwrap();

    assert_level(&mut bus, SOURCE_A, true);
    assert!(matches!(
        bus.poll(),
        IrqBusAction::Deliver {
            target: TARGET_A,
            ..
        }
    ));
    assert!(matches!(
        bus.poll(),
        IrqBusAction::Deliver {
            target: TARGET_B,
            ..
        }
    ));
}

#[test]
fn multiple_sources_are_wired_or_at_each_target_input() {
    let mut bus = IrqBus::new(
        BUS,
        "IRQ",
        [route(SOURCE_A, TARGET_A), route(SOURCE_B, TARGET_A)],
    )
    .unwrap();

    assert_level(&mut bus, SOURCE_A, true);
    assert!(matches!(
        bus.poll(),
        IrqBusAction::Deliver {
            delivery: IrqDelivery { asserted: true, .. },
            ..
        }
    ));
    assert_level(&mut bus, SOURCE_B, true);
    assert_eq!(bus.poll(), IrqBusAction::Idle);
    assert_level(&mut bus, SOURCE_A, false);
    assert_eq!(bus.poll(), IrqBusAction::Idle);
    assert_level(&mut bus, SOURCE_B, false);
    assert!(matches!(
        bus.poll(),
        IrqBusAction::Deliver {
            delivery: IrqDelivery {
                asserted: false,
                ..
            },
            ..
        }
    ));
}

#[test]
fn distinct_target_inputs_change_independently() {
    let input_b = IrqInput::new(3);
    let mut bus = IrqBus::new(
        BUS,
        "IRQ",
        [
            route(SOURCE_A, TARGET_A),
            IrqRoute {
                source: source(SOURCE_B),
                target: IrqTarget {
                    component: TARGET_A,
                    input: input_b,
                },
            },
        ],
    )
    .unwrap();

    assert_level(&mut bus, SOURCE_A, true);
    assert!(matches!(
        bus.poll(),
        IrqBusAction::Deliver {
            delivery: IrqDelivery { input: INPUT, .. },
            ..
        }
    ));
    assert_level(&mut bus, SOURCE_B, true);
    assert!(matches!(
        bus.poll(),
        IrqBusAction::Deliver {
            delivery: IrqDelivery { input, .. },
            ..
        } if input == input_b
    ));
}

#[test]
fn unrouted_sources_are_rejected_without_actions() {
    let mut bus = IrqBus::new(BUS, "IRQ", [route(SOURCE_A, TARGET_A)]).unwrap();
    let unknown = source(SOURCE_B);
    assert_eq!(
        BusRole::route(
            &mut bus,
            IrqTransaction {
                source: unknown,
                asserted: true,
            }
        ),
        Err(IrqBusRouteError::UnroutedSource(unknown))
    );
    assert_eq!(bus.poll(), IrqBusAction::Idle);
}

#[test]
fn reset_clears_levels_and_pending_deliveries() {
    let mut bus = IrqBus::new(BUS, "IRQ", [route(SOURCE_A, TARGET_A)]).unwrap();
    assert_level(&mut bus, SOURCE_A, true);
    Component::reset(&mut bus);
    assert_eq!(bus.poll(), IrqBusAction::Idle);

    assert_level(&mut bus, SOURCE_A, true);
    assert!(matches!(bus.poll(), IrqBusAction::Deliver { .. }));
}
