use std::sync::{Arc, Barrier};

use se_float::backend::Backend;
use se_float::format::Float32;
use se_float::operation::{ExceptionFlags, RoundingMode};

const THREAD_COUNT: usize = 8;
const ITERATIONS: usize = 2_000;

#[test]
fn concurrent_calls_keep_softfloat_state_operation_local() {
    let barrier = Arc::new(Barrier::new(THREAD_COUNT));
    let handles: Vec<_> = (0..THREAD_COUNT)
        .map(|index| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let backend = Backend::SoftFloat;
                let (rounding, expected) = if index % 2 == 0 {
                    (RoundingMode::TowardPositive, 0x3f80_0001)
                } else {
                    (RoundingMode::TowardNegative, 0x3f80_0000)
                };

                barrier.wait();
                for _ in 0..ITERATIONS {
                    let rounded = backend.add_f32(
                        Float32::from_bits(1.0_f32.to_bits()),
                        Float32::from_bits(0x3380_0000),
                        rounding,
                    );
                    assert_eq!(rounded.value.to_bits(), expected);
                    assert_eq!(rounded.flags, ExceptionFlags::INEXACT);

                    let divide_by_zero = backend.div_f32(
                        Float32::from_bits(1.0_f32.to_bits()),
                        Float32::from_bits(0.0_f32.to_bits()),
                        rounding,
                    );
                    assert_eq!(divide_by_zero.flags, ExceptionFlags::DIVIDE_BY_ZERO);

                    let exact = backend.add_f32(
                        Float32::from_bits(1.0_f32.to_bits()),
                        Float32::from_bits(1.0_f32.to_bits()),
                        rounding,
                    );
                    assert_eq!(exact.value.to_bits(), 2.0_f32.to_bits());
                    assert_eq!(exact.flags, ExceptionFlags::empty());
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("SoftFloat worker panicked");
    }
}
