use std::sync::{Arc, Barrier};
use std::thread;

use se_float::SoftFloatBackend;
use se_float::env::{ExceptionFlags, Relation, RoundingFacts, RoundingMode};

#[test]
fn alternating_instances_replace_same_thread_transaction_state() {
    let first = SoftFloatBackend;
    let second = SoftFloatBackend;

    let upward = first.add_f32(0x3f80_0000, 0x3380_0000, RoundingMode::TowardPositive);
    assert_eq!(upward.value, 0x3f80_0001);
    assert_eq!(upward.flags, ExceptionFlags::INEXACT);
    assert_eq!(
        upward.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        }
    );

    // Comparison does not enter round-pack and must clear the preceding mode,
    // flag, and discarded-precision state.
    let exact_compare = second.compare_f64(0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0000);
    assert_eq!(exact_compare.value, Relation::Equal);
    assert!(exact_compare.flags.is_empty());
    assert_eq!(exact_compare.rounding, RoundingFacts::default());

    // Exact exponent overflow enters round-pack but discards no precision.
    let exact_overflow = first.mul_f64(
        0x7fe0_0000_0000_0000,
        0x4000_0000_0000_0000,
        RoundingMode::TowardZero,
    );
    assert_eq!(exact_overflow.value, 0x7fef_ffff_ffff_ffff);
    assert_eq!(
        exact_overflow.flags,
        ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT
    );
    assert_eq!(
        exact_overflow.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: false,
        }
    );

    // Integer conversion reports discarded fractional information without
    // entering either floating-point round-pack path.
    let scalar_inexact = second.f64_to_i32(0x3ff8_0000_0000_0000, RoundingMode::TowardPositive);
    assert_eq!(scalar_inexact.value, Some(2));
    assert_eq!(scalar_inexact.flags, ExceptionFlags::INEXACT);
    assert_eq!(
        scalar_inexact.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        }
    );

    let inexact_overflow = first.mul_f32(0x7f7f_ffff, 0x3f80_0001, RoundingMode::NearestEven);
    assert_eq!(inexact_overflow.value, 0x7f80_0000);
    assert_eq!(
        inexact_overflow.flags,
        ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT
    );
    assert_eq!(
        inexact_overflow.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        }
    );

    let exact_scalar = second.i32_to_f64(7);
    assert_eq!(exact_scalar.value, 0x401c_0000_0000_0000);
    assert!(exact_scalar.flags.is_empty());
    assert_eq!(exact_scalar.rounding, RoundingFacts::default());

    let downward = first.add_f32(0xbf80_0000, 0xb380_0000, RoundingMode::TowardNegative);
    assert_eq!(downward.value, 0xbf80_0001);
    assert_eq!(downward.flags, ExceptionFlags::INEXACT);

    let exact_tiny = second.f64_to_f32(0x380f_ffff_c000_0000, RoundingMode::NearestEven);
    assert_eq!(exact_tiny.value, 0x007f_ffff);
    assert!(exact_tiny.flags.is_empty());
    assert_eq!(
        exact_tiny.rounding,
        RoundingFacts {
            tiny_after_rounding: true,
            precision_inexact: false,
        }
    );

    let final_exact = first.add_f32(0x3f80_0000, 0x3f80_0000, RoundingMode::NearestEven);
    assert_eq!(final_exact.value, 0x4000_0000);
    assert!(final_exact.flags.is_empty());
    assert_eq!(final_exact.rounding, RoundingFacts::default());
}

#[test]
fn synchronized_threads_keep_rounding_flags_and_round_pack_facts_isolated() {
    const PHASES: usize = 32;

    let barrier = Arc::new(Barrier::new(2));
    let first_barrier = Arc::clone(&barrier);
    let first = thread::spawn(move || {
        let backend = SoftFloatBackend;
        let mut halfway = Vec::with_capacity(PHASES);
        let mut overflow = Vec::with_capacity(PHASES);
        let mut comparisons = Vec::with_capacity(PHASES);

        for _ in 0..PHASES {
            first_barrier.wait();
            halfway.push(backend.add_f32(0x3f80_0000, 0x3380_0000, RoundingMode::TowardPositive));
            first_barrier.wait();

            first_barrier.wait();
            overflow.push(backend.mul_f32(0x7f7f_ffff, 0x3f80_0001, RoundingMode::NearestEven));
            first_barrier.wait();

            first_barrier.wait();
            comparisons.push(backend.compare_f64(0x3ff0_0000_0000_0000, 0x4000_0000_0000_0000));
            first_barrier.wait();
        }

        (halfway, overflow, comparisons)
    });

    let second = thread::spawn(move || {
        let backend = SoftFloatBackend;
        let mut halfway = Vec::with_capacity(PHASES);
        let mut overflow = Vec::with_capacity(PHASES);
        let mut tiny = Vec::with_capacity(PHASES);

        for _ in 0..PHASES {
            barrier.wait();
            halfway.push(backend.add_f64(
                0xbff0_0000_0000_0000,
                0xbca0_0000_0000_0000,
                RoundingMode::TowardNegative,
            ));
            barrier.wait();

            barrier.wait();
            overflow.push(backend.mul_f64(
                0x7fe0_0000_0000_0000,
                0x4000_0000_0000_0000,
                RoundingMode::TowardZero,
            ));
            barrier.wait();

            barrier.wait();
            tiny.push(backend.f64_to_f32(0x380f_ffff_c000_0000, RoundingMode::NearestEven));
            barrier.wait();
        }

        (halfway, overflow, tiny)
    });

    let (positive_halfway, inexact_overflow, comparisons) =
        first.join().expect("first worker completed");
    let (negative_halfway, exact_overflow, exact_tiny) =
        second.join().expect("second worker completed");

    for outcome in positive_halfway {
        assert_eq!(outcome.value, 0x3f80_0001);
        assert_eq!(outcome.flags, ExceptionFlags::INEXACT);
        assert_eq!(
            outcome.rounding,
            RoundingFacts {
                tiny_after_rounding: false,
                precision_inexact: true,
            }
        );
    }
    for outcome in negative_halfway {
        assert_eq!(outcome.value, 0xbff0_0000_0000_0001);
        assert_eq!(outcome.flags, ExceptionFlags::INEXACT);
        assert_eq!(
            outcome.rounding,
            RoundingFacts {
                tiny_after_rounding: false,
                precision_inexact: true,
            }
        );
    }
    for outcome in inexact_overflow {
        assert_eq!(outcome.value, 0x7f80_0000);
        assert_eq!(
            outcome.flags,
            ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT
        );
        assert_eq!(
            outcome.rounding,
            RoundingFacts {
                tiny_after_rounding: false,
                precision_inexact: true,
            }
        );
    }
    for outcome in exact_overflow {
        assert_eq!(outcome.value, 0x7fef_ffff_ffff_ffff);
        assert_eq!(
            outcome.flags,
            ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT
        );
        assert_eq!(
            outcome.rounding,
            RoundingFacts {
                tiny_after_rounding: false,
                precision_inexact: false,
            }
        );
    }
    for outcome in comparisons {
        assert_eq!(outcome.value, Relation::Less);
        assert!(outcome.flags.is_empty());
        assert_eq!(outcome.rounding, RoundingFacts::default());
    }
    for outcome in exact_tiny {
        assert_eq!(outcome.value, 0x007f_ffff);
        assert!(outcome.flags.is_empty());
        assert_eq!(
            outcome.rounding,
            RoundingFacts {
                tiny_after_rounding: true,
                precision_inexact: false,
            }
        );
    }
}
