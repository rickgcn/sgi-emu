use super::*;

#[test]
fn result_preserves_value_and_flags() {
    let result = FloatResult::new(7_u32, FloatExceptionFlags::INEXACT);

    assert_eq!(result.value, 7);
    assert_eq!(result.flags, FloatExceptionFlags::INEXACT);
}
