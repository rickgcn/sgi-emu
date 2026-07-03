use super::*;

#[test]
fn machine_timing_new_stores_timebase() {
    let timing = MachineTiming::new(123_456_789);

    assert_eq!(timing.timebase_hz(), 123_456_789);
}
