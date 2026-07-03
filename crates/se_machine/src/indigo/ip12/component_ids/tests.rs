use std::collections::BTreeSet;

use super::*;

#[test]
fn all_component_ids_are_unique() {
    let mut seen = BTreeSet::new();

    for id in ALL_COMPONENT_IDS {
        assert!(seen.insert(id.get()), "duplicate component id {}", id.get());
    }
}

#[test]
fn component_ids_are_stably_increasing_in_definition_order() {
    for ids in ALL_COMPONENT_IDS.windows(2) {
        assert!(
            ids[0] < ids[1],
            "component id {} must be lower than {}",
            ids[0].get(),
            ids[1].get()
        );
    }
}

#[test]
fn bus_component_ids_are_present_and_distinct() {
    assert_eq!(BUS_COMPONENT_IDS, [CPU_BUS, GIO32_BUS, PBUS]);
    assert_ne!(CPU_BUS, GIO32_BUS);
    assert_ne!(CPU_BUS, PBUS);
    assert_ne!(GIO32_BUS, PBUS);
}
