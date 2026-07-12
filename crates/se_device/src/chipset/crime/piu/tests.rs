use super::*;

const TIMEBASE_HZ: u64 = 1_000_000_000;

#[test]
fn id_control_and_timer_reset_values_match_crime_11() {
    let piu = CrimePiu::new();

    assert_eq!(
        piu.read(registers::ID, SimTime::ZERO, TIMEBASE_HZ),
        Some(0xA1)
    );
    assert_eq!(
        piu.read(registers::CONTROL, SimTime::ZERO, TIMEBASE_HZ),
        Some(registers::CONTROL_BIG_ENDIAN)
    );
    assert_eq!(
        piu.read(registers::TIMER, SimTime::ZERO, TIMEBASE_HZ),
        Some(0)
    );
}

#[test]
fn crime_timer_uses_the_documented_master_frequency() {
    let mut piu = CrimePiu::new();
    piu.write(registers::TIMER, 10, SimTime::new(100), TIMEBASE_HZ);

    assert_eq!(
        piu.read(registers::TIMER, SimTime::new(1_000_000_100), TIMEBASE_HZ),
        Some(66_666_510)
    );
}

#[test]
fn enabled_interrupt_changes_output_only_when_combined_status_changes() {
    let mut piu = CrimePiu::new();
    assert_eq!(
        piu.write(
            registers::INTERRUPT_ENABLE,
            u64::from(registers::INTERRUPT_MEMORY_ERROR),
            SimTime::ZERO,
            TIMEBASE_HZ,
        )
        .effects,
        Vec::new()
    );
    assert_eq!(
        piu.set_hardware_level(registers::INTERRUPT_MEMORY_ERROR, true),
        Some(PiuEffect::InterruptOutput(true))
    );
    assert_eq!(
        piu.set_hardware_level(registers::INTERRUPT_MEMORY_ERROR, true),
        None
    );
    assert_eq!(
        piu.set_hardware_level(registers::INTERRUPT_MEMORY_ERROR, false),
        Some(PiuEffect::InterruptOutput(false))
    );
}

#[test]
fn watchdog_uses_two_distinct_crime_11_stages() {
    let mut piu = CrimePiu::new();
    let effects = piu
        .write(
            registers::CONTROL,
            registers::CONTROL_WATCHDOG_ENABLE,
            SimTime::ZERO,
            TIMEBASE_HZ,
        )
        .effects;
    let PiuEffect::ArmWatchdog { epoch, stage, .. } = effects[0] else {
        panic!("watchdog was not armed");
    };
    assert_eq!(stage, 1);

    let effects = piu.handle_watchdog(epoch, 1, TIMEBASE_HZ);
    assert!(matches!(effects[0], PiuEffect::WarmReset));
    assert!(matches!(
        effects[1],
        PiuEffect::ArmWatchdog { stage: 2, .. }
    ));
    assert_eq!(
        piu.handle_watchdog(epoch, 2, TIMEBASE_HZ),
        vec![PiuEffect::HardReset]
    );
}

#[test]
fn rewriting_watchdog_invalidates_an_old_event() {
    let mut piu = CrimePiu::new();
    let first = piu
        .write(
            registers::CONTROL,
            registers::CONTROL_WATCHDOG_ENABLE,
            SimTime::ZERO,
            TIMEBASE_HZ,
        )
        .effects;
    let PiuEffect::ArmWatchdog { epoch, .. } = first[0] else {
        panic!("watchdog was not armed");
    };
    piu.write(registers::WATCHDOG, 0, SimTime::ZERO, TIMEBASE_HZ);

    assert!(piu.handle_watchdog(epoch, 1, TIMEBASE_HZ).is_empty());
}
