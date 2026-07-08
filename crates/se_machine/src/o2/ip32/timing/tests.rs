use super::*;

#[test]
fn ip32_timing_uses_fixed_timebase() {
    assert_eq!(IP32_TIMING.timebase_hz(), 1_000_000_000);
}
