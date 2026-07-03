use super::*;

#[test]
fn ip12_timing_uses_fixed_timebase() {
    assert_eq!(IP12_TIMING.timebase_hz(), 3_300_000_000);
}
