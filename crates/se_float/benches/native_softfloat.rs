use std::hint::black_box;
use std::time::{Duration, Instant};

use se_float::env::RoundingMode;
use se_float::{NativeBackend, SoftFloatBackend};

const BATCH_SIZE: usize = 256;
const BATCH_REPETITIONS: usize = 1_000;
const SAMPLES: usize = 5;
const WARMUP_REPETITIONS: usize = 32;

fn binary_inputs() -> Vec<(u32, u32)> {
    (0..BATCH_SIZE)
        .map(|index| {
            let left = 1.0_f32 + index as f32 / 512.0;
            let right = 0.75_f32 + (index.wrapping_mul(17) % BATCH_SIZE) as f32 / 1_024.0;
            (left.to_bits(), right.to_bits())
        })
        .collect()
}

fn unary_inputs() -> Vec<u32> {
    (0..BATCH_SIZE)
        .map(|index| (0.5_f32 + index as f32 / 128.0).to_bits())
        .collect()
}

fn conversion_inputs() -> Vec<u64> {
    (0..BATCH_SIZE)
        .map(|index| {
            let sign = if index & 1 == 0 { 1.0 } else { -1.0 };
            (sign * (0.25_f64 + index as f64 / 64.0)).to_bits()
        })
        .collect()
}

fn measure(batch: &mut impl FnMut() -> u64) -> Duration {
    for _ in 0..WARMUP_REPETITIONS {
        black_box(batch());
    }

    let start = Instant::now();
    let mut digest = 0_u64;
    for _ in 0..BATCH_REPETITIONS {
        digest ^= black_box(batch());
    }
    black_box(digest);
    start.elapsed()
}

fn median_ns_per_operation(mut batch: impl FnMut() -> u64) -> (f64, Duration) {
    let mut samples = (0..SAMPLES)
        .map(|_| measure(&mut batch))
        .collect::<Vec<_>>();
    samples.sort_unstable();
    let median = samples[SAMPLES / 2];
    let operations = (BATCH_SIZE * BATCH_REPETITIONS) as f64;
    (median.as_secs_f64() * 1e9 / operations, median)
}

fn report(name: &str, implementation: &str, batch: impl FnMut() -> u64) {
    let (ns_per_operation, elapsed) = median_ns_per_operation(batch);
    println!("{name:<12} {implementation:<9} {ns_per_operation:>9.3} ns/op  median {elapsed:?}");
}

fn main() {
    let native = NativeBackend;
    let accurate = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;
    let binary = binary_inputs();
    let unary = unary_inputs();
    let conversions = conversion_inputs();

    println!("se_float native and SoftFloat release benchmark");
    println!("samples={SAMPLES} batch_size={BATCH_SIZE} repetitions={BATCH_REPETITIONS}");

    report("add_f32", "native", || {
        let mut digest = 0_u32;
        for &(a, b) in black_box(binary.as_slice()) {
            digest = digest.rotate_left(5) ^ black_box(native.add_f32(black_box(a), black_box(b)));
        }
        u64::from(black_box(digest))
    });
    report("add_f32", "softfloat", || {
        let mut digest = 0_u32;
        for &(a, b) in black_box(binary.as_slice()) {
            digest = digest.rotate_left(5)
                ^ black_box(accurate.add_f32(black_box(a), black_box(b), rounding).value);
        }
        u64::from(black_box(digest))
    });

    report("mul_f32", "native", || {
        let mut digest = 0_u32;
        for &(a, b) in black_box(binary.as_slice()) {
            digest = digest.rotate_left(5) ^ black_box(native.mul_f32(black_box(a), black_box(b)));
        }
        u64::from(black_box(digest))
    });
    report("mul_f32", "softfloat", || {
        let mut digest = 0_u32;
        for &(a, b) in black_box(binary.as_slice()) {
            digest = digest.rotate_left(5)
                ^ black_box(accurate.mul_f32(black_box(a), black_box(b), rounding).value);
        }
        u64::from(black_box(digest))
    });

    report("div_f32", "native", || {
        let mut digest = 0_u32;
        for &(a, b) in black_box(binary.as_slice()) {
            digest = digest.rotate_left(5) ^ black_box(native.div_f32(black_box(a), black_box(b)));
        }
        u64::from(black_box(digest))
    });
    report("div_f32", "softfloat", || {
        let mut digest = 0_u32;
        for &(a, b) in black_box(binary.as_slice()) {
            digest = digest.rotate_left(5)
                ^ black_box(accurate.div_f32(black_box(a), black_box(b), rounding).value);
        }
        u64::from(black_box(digest))
    });

    report("sqrt_f32", "native", || {
        let mut digest = 0_u32;
        for &value in black_box(unary.as_slice()) {
            digest = digest.rotate_left(5) ^ black_box(native.sqrt_f32(black_box(value)));
        }
        u64::from(black_box(digest))
    });
    report("sqrt_f32", "softfloat", || {
        let mut digest = 0_u32;
        for &value in black_box(unary.as_slice()) {
            digest = digest.rotate_left(5)
                ^ black_box(accurate.sqrt_f32(black_box(value), rounding).value);
        }
        u64::from(black_box(digest))
    });

    report("f64_to_f32", "native", || {
        let mut digest = 0_u32;
        for &value in black_box(conversions.as_slice()) {
            digest = digest.rotate_left(5) ^ black_box(native.f64_to_f32(black_box(value)));
        }
        u64::from(black_box(digest))
    });
    report("f64_to_f32", "softfloat", || {
        let mut digest = 0_u32;
        for &value in black_box(conversions.as_slice()) {
            digest = digest.rotate_left(5)
                ^ black_box(accurate.f64_to_f32(black_box(value), rounding).value);
        }
        u64::from(black_box(digest))
    });
}
