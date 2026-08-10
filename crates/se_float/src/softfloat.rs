//! Owns the private Rust boundary to the SoftFloat C transaction layer.
//!
//! C-backed calls use fixed-width integer values and tagged `u8` control and
//! result fields. Each call resets rounding mode, tininess detection, flags,
//! and round-pack observations before executing one primitive.

#[cfg(test)]
mod tests {
    use std::panic;

    use crate::bits::{is_nonzero_subnormal_f32, is_nonzero_subnormal_f64};
    use crate::env::{ExceptionFlags, Outcome, Relation, RoundingFacts, RoundingMode};

    unsafe extern "C" {
        fn se_float_shim_add_f32(
            a: u32,
            b: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_sub_f32(
            a: u32,
            b: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_mul_f32(
            a: u32,
            b: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_div_f32(
            a: u32,
            b: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_sqrt_f32(
            value: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_compare_f32(
            a: u32,
            b: u32,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u8;
        fn se_float_shim_add_f64(
            a: u64,
            b: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_sub_f64(
            a: u64,
            b: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_mul_f64(
            a: u64,
            b: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_div_f64(
            a: u64,
            b: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_sqrt_f64(
            value: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_compare_f64(
            a: u64,
            b: u64,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u8;
        fn se_float_shim_f32_to_f64(
            value: u32,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_f64_to_f32(
            value: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_i32_to_f32(
            value: i32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_i64_to_f32(
            value: i64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_i32_to_f64(
            value: i32,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_i64_to_f64(
            value: i64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_f32_to_i32(
            value: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> i32;
        fn se_float_shim_f32_to_i64(
            value: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> i64;
        fn se_float_shim_f64_to_i32(
            value: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> i32;
        fn se_float_shim_f64_to_i64(
            value: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> i64;
    }

    fn encode_rounding(rounding: RoundingMode) -> u8 {
        match rounding {
            RoundingMode::NearestEven => 0,
            RoundingMode::TowardZero => 1,
            RoundingMode::TowardPositive => 2,
            RoundingMode::TowardNegative => 3,
        }
    }

    fn decode_flags(raw: u8) -> ExceptionFlags {
        ExceptionFlags::from_bits(raw)
            .unwrap_or_else(|| panic!("invalid SoftFloat flag byte {raw:#04x}"))
    }

    fn decode_fact(raw: u8) -> bool {
        match raw {
            0 => false,
            1 => true,
            _ => panic!("invalid SoftFloat rounding-fact byte {raw:#04x}"),
        }
    }

    fn decode_relation(raw: u8) -> Relation {
        match raw {
            0 => Relation::Less,
            1 => Relation::Equal,
            2 => Relation::Greater,
            3 => Relation::Unordered,
            _ => panic!("invalid SoftFloat relation byte {raw:#04x}"),
        }
    }

    fn outcome_f32(value: u32, raw_flags: u8, raw_precision_inexact: u8) -> Outcome<u32> {
        let flags = decode_flags(raw_flags);
        let precision_inexact = decode_fact(raw_precision_inexact);
        validate_float_flags(flags, is_nonzero_subnormal_f32(value));
        Outcome::new(
            value,
            flags,
            RoundingFacts {
                tiny_after_rounding: flags.contains(ExceptionFlags::UNDERFLOW)
                    || (is_nonzero_subnormal_f32(value)
                        && !flags.contains(ExceptionFlags::INEXACT)),
                precision_inexact,
            },
        )
    }

    fn outcome_f64(value: u64, raw_flags: u8, raw_precision_inexact: u8) -> Outcome<u64> {
        let flags = decode_flags(raw_flags);
        let precision_inexact = decode_fact(raw_precision_inexact);
        validate_float_flags(flags, is_nonzero_subnormal_f64(value));
        Outcome::new(
            value,
            flags,
            RoundingFacts {
                tiny_after_rounding: flags.contains(ExceptionFlags::UNDERFLOW)
                    || (is_nonzero_subnormal_f64(value)
                        && !flags.contains(ExceptionFlags::INEXACT)),
                precision_inexact,
            },
        )
    }

    fn validate_float_flags(flags: ExceptionFlags, nonzero_subnormal: bool) {
        assert!(
            !flags.contains(ExceptionFlags::UNDERFLOW) || flags.contains(ExceptionFlags::INEXACT)
        );
        assert!(
            !nonzero_subnormal
                || !flags.contains(ExceptionFlags::INEXACT)
                || flags.contains(ExceptionFlags::UNDERFLOW)
        );
    }

    fn add_f32(a: u32, b: u32, rounding: RoundingMode) -> Outcome<u32> {
        let mut flags = 0;
        let mut precision_inexact = 0;
        // The two output pointers address initialized one-byte Rust storage for
        // the duration of the call, and all value and tag arguments match shim.h.
        let value = unsafe {
            se_float_shim_add_f32(
                a,
                b,
                encode_rounding(rounding),
                &mut flags,
                &mut precision_inexact,
            )
        };
        outcome_f32(value, flags, precision_inexact)
    }

    fn mul_f32(a: u32, b: u32, rounding: RoundingMode) -> Outcome<u32> {
        let mut flags = 0;
        let mut precision_inexact = 0;
        // The two output pointers address initialized one-byte Rust storage for
        // the duration of the call, and all value and tag arguments match shim.h.
        let value = unsafe {
            se_float_shim_mul_f32(
                a,
                b,
                encode_rounding(rounding),
                &mut flags,
                &mut precision_inexact,
            )
        };
        outcome_f32(value, flags, precision_inexact)
    }

    fn f64_to_f32(value: u64, rounding: RoundingMode) -> Outcome<u32> {
        let mut flags = 0;
        let mut precision_inexact = 0;
        // The two output pointers address initialized one-byte Rust storage for
        // the duration of the call, and all value and tag arguments match shim.h.
        let value = unsafe {
            se_float_shim_f64_to_f32(
                value,
                encode_rounding(rounding),
                &mut flags,
                &mut precision_inexact,
            )
        };
        outcome_f32(value, flags, precision_inexact)
    }

    #[test]
    fn all_twenty_two_shims_link() {
        let mut flags = 0;
        let mut precision_inexact = 0;
        let flags_ptr = &mut flags;
        let precision_ptr = &mut precision_inexact;

        // Every pointer remains valid and uniquely borrowed for each call. The
        // zero tags and fixed-width values satisfy the common shim.h contract.
        unsafe {
            se_float_shim_add_f32(0, 0, 0, flags_ptr, precision_ptr);
            se_float_shim_sub_f32(0, 0, 0, flags_ptr, precision_ptr);
            se_float_shim_mul_f32(0, 0, 0, flags_ptr, precision_ptr);
            se_float_shim_div_f32(0, 0, 0, flags_ptr, precision_ptr);
            se_float_shim_sqrt_f32(0, 0, flags_ptr, precision_ptr);
            se_float_shim_compare_f32(0, 0, flags_ptr, precision_ptr);
            se_float_shim_add_f64(0, 0, 0, flags_ptr, precision_ptr);
            se_float_shim_sub_f64(0, 0, 0, flags_ptr, precision_ptr);
            se_float_shim_mul_f64(0, 0, 0, flags_ptr, precision_ptr);
            se_float_shim_div_f64(0, 0, 0, flags_ptr, precision_ptr);
            se_float_shim_sqrt_f64(0, 0, flags_ptr, precision_ptr);
            se_float_shim_compare_f64(0, 0, flags_ptr, precision_ptr);
            se_float_shim_f32_to_f64(0, flags_ptr, precision_ptr);
            se_float_shim_f64_to_f32(0, 0, flags_ptr, precision_ptr);
            se_float_shim_i32_to_f32(0, 0, flags_ptr, precision_ptr);
            se_float_shim_i64_to_f32(0, 0, flags_ptr, precision_ptr);
            se_float_shim_i32_to_f64(0, flags_ptr, precision_ptr);
            se_float_shim_i64_to_f64(0, 0, flags_ptr, precision_ptr);
            se_float_shim_f32_to_i32(0, 0, flags_ptr, precision_ptr);
            se_float_shim_f32_to_i64(0, 0, flags_ptr, precision_ptr);
            se_float_shim_f64_to_i32(0, 0, flags_ptr, precision_ptr);
            se_float_shim_f64_to_i64(0, 0, flags_ptr, precision_ptr);
        }
    }

    #[test]
    fn tagged_byte_codecs_are_exhaustive() {
        assert_eq!(encode_rounding(RoundingMode::NearestEven), 0);
        assert_eq!(encode_rounding(RoundingMode::TowardZero), 1);
        assert_eq!(encode_rounding(RoundingMode::TowardPositive), 2);
        assert_eq!(encode_rounding(RoundingMode::TowardNegative), 3);
        assert_eq!(decode_relation(0), Relation::Less);
        assert_eq!(decode_relation(1), Relation::Equal);
        assert_eq!(decode_relation(2), Relation::Greater);
        assert_eq!(decode_relation(3), Relation::Unordered);
        assert!(!decode_fact(0));
        assert!(decode_fact(1));
    }

    #[test]
    fn invalid_tagged_bytes_fail_immediately() {
        assert!(panic::catch_unwind(|| decode_flags(0x80)).is_err());
        assert!(panic::catch_unwind(|| decode_fact(2)).is_err());
        assert!(panic::catch_unwind(|| decode_relation(4)).is_err());

        let mut flags = 0;
        let mut precision_inexact = 0;
        // The output pointers are valid; 0xFF intentionally exercises the
        // shim's invalid rounding-tag contract rather than a Rust enum value.
        let value = unsafe {
            se_float_shim_add_f32(
                0x3f80_0000,
                0x3f80_0000,
                0xff,
                &mut flags,
                &mut precision_inexact,
            )
        };
        assert_eq!(value, 0);
        assert_eq!(flags, 0x80);
        assert_eq!(precision_inexact, 2);
    }

    #[test]
    fn formal_tininess_vectors_match_after_rounding_contract() {
        let rounds_to_tiny = f64_to_f32(0x380f_ffff_e100_0000, RoundingMode::NearestEven);
        let rounds_to_normal = f64_to_f32(0x380f_ffff_fc00_0000, RoundingMode::NearestEven);
        let exact_subnormal = f64_to_f32(0x380f_ffff_c000_0000, RoundingMode::NearestEven);

        assert_eq!(rounds_to_tiny.value, 0x0080_0000);
        assert_eq!(
            rounds_to_tiny.flags,
            ExceptionFlags::UNDERFLOW | ExceptionFlags::INEXACT
        );
        assert_eq!(
            rounds_to_tiny.rounding,
            RoundingFacts {
                tiny_after_rounding: true,
                precision_inexact: true,
            }
        );
        assert_eq!(rounds_to_normal.value, 0x0080_0000);
        assert_eq!(rounds_to_normal.flags, ExceptionFlags::INEXACT);
        assert_eq!(
            rounds_to_normal.rounding,
            RoundingFacts {
                tiny_after_rounding: false,
                precision_inexact: true,
            }
        );
        assert_eq!(exact_subnormal.value, 0x007f_ffff);
        assert!(exact_subnormal.flags.is_empty());
        assert_eq!(
            exact_subnormal.rounding,
            RoundingFacts {
                tiny_after_rounding: true,
                precision_inexact: false,
            }
        );
    }

    #[test]
    fn overflow_observation_distinguishes_precision_loss() {
        let exact = mul_f32(0x7f00_0000, 0x4000_0000, RoundingMode::NearestEven);
        let inexact = mul_f32(0x7f7f_ffff, 0x3f80_0001, RoundingMode::NearestEven);
        let expected_flags = ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT;

        assert_eq!(exact.value, 0x7f80_0000);
        assert_eq!(exact.flags, expected_flags);
        assert!(!exact.rounding.tiny_after_rounding);
        assert!(!exact.rounding.precision_inexact);
        assert_eq!(inexact.value, 0x7f80_0000);
        assert_eq!(inexact.flags, expected_flags);
        assert!(!inexact.rounding.tiny_after_rounding);
        assert!(inexact.rounding.precision_inexact);
    }

    #[test]
    fn sequential_transactions_replace_rounding_flags_and_facts() {
        let upward = add_f32(0x3f80_0000, 0x3380_0000, RoundingMode::TowardPositive);
        let nearest = add_f32(0x3f80_0000, 0x3380_0000, RoundingMode::NearestEven);
        let overflow = mul_f32(0x7f7f_ffff, 0x3f80_0001, RoundingMode::NearestEven);
        let exact = add_f32(0x3f80_0000, 0x3f80_0000, RoundingMode::NearestEven);

        assert_eq!(upward.value, 0x3f80_0001);
        assert_eq!(nearest.value, 0x3f80_0000);
        assert_eq!(nearest.flags, ExceptionFlags::INEXACT);
        assert!(nearest.rounding.precision_inexact);
        assert!(overflow.rounding.precision_inexact);
        assert_eq!(exact.value, 0x4000_0000);
        assert!(exact.flags.is_empty());
        assert_eq!(exact.rounding, RoundingFacts::default());
    }

    #[test]
    fn float_to_integer_shims_request_inexact_reporting() {
        let mut flags = 0;
        let mut precision_inexact = 0;
        // The output pointers address one writable byte each, and the input is
        // a raw binary32 encoding under the nearest-even tag.
        let value =
            unsafe { se_float_shim_f32_to_i32(0x3fc0_0000, 0, &mut flags, &mut precision_inexact) };

        assert_eq!(value, 2);
        assert_eq!(decode_flags(flags), ExceptionFlags::INEXACT);
        assert!(decode_fact(precision_inexact));
    }

    #[test]
    fn binary64_result_codec_recovers_exact_subnormal_tininess() {
        let outcome = outcome_f64(1, 0, 0);

        assert!(outcome.rounding.tiny_after_rounding);
        assert!(!outcome.rounding.precision_inexact);
    }

    #[test]
    fn comparison_relation_uses_softfloat_transaction() {
        let mut flags = 0;
        let mut precision_inexact = 0;
        // The output pointers address one writable byte each, and both values
        // are raw binary32 encodings as required by shim.h.
        let relation = unsafe {
            se_float_shim_compare_f32(0x7f80_0001, 0x3f80_0000, &mut flags, &mut precision_inexact)
        };

        assert_eq!(decode_relation(relation), Relation::Unordered);
        assert_eq!(decode_flags(flags), ExceptionFlags::INVALID);
        assert!(!decode_fact(precision_inexact));
    }
}

#[cfg(test)]
mod source_contract_tests {
    use std::fs;
    use std::path::Path;

    const PLATFORM_PRIMITIVES: &[&str] = &[
        "approxRecip32_1",
        "approxRecipSqrt32_1",
        "countLeadingZeros32",
        "countLeadingZeros64",
        "mul64To128",
        "shiftRightJam32",
        "shiftRightJam64",
        "shiftRightJam64Extra",
        "shortShiftRightJam64",
    ];

    fn manifest_entries(build_script: &str) -> Vec<&str> {
        let start = build_script
            .find("const UPSTREAM_TRANSLATION_UNITS")
            .expect("translation-unit manifest declaration");
        let body = &build_script[start..];
        let end = body.find("\n];").expect("translation-unit manifest end");
        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                line.strip_prefix('"')
                    .and_then(|line| line.strip_suffix("\","))
            })
            .collect()
    }

    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.match_indices(needle).count()
    }

    #[test]
    fn source_manifest_is_sorted_explicit_and_complete_on_disk() {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let source_dir = crate_dir.join("../../3rdparty/softfloat3/source");
        let build_script = include_str!("../build.rs");
        let entries = manifest_entries(build_script);

        assert!(!entries.is_empty());
        assert!(entries.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(entries.iter().all(|path| source_dir.join(path).is_file()));
        assert!(!build_script.contains("read_dir"));
        assert!(!build_script.contains("walkdir"));
    }

    #[test]
    fn platform_and_round_pack_replacements_are_unique() {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let build_script = include_str!("../build.rs");
        let entries = manifest_entries(build_script);
        let platform =
            fs::read_to_string(crate_dir.join("csrc/platform.h")).expect("read platform.h");
        let primitives =
            fs::read_to_string(crate_dir.join("csrc/primitives.c")).expect("read primitives.c");
        let rename = fs::read_to_string(crate_dir.join("csrc/rename.h")).expect("read rename.h");
        let round_pack =
            fs::read_to_string(crate_dir.join("csrc/round_pack.c")).expect("read round_pack.c");

        for primitive in PLATFORM_PRIMITIVES {
            let upstream_file = format!("s_{primitive}.c");
            assert!(!entries.contains(&upstream_file.as_str()));
            assert_eq!(
                occurrences(&primitives, &format!("se_float_sf_{primitive}(")),
                1
            );
            assert_eq!(
                occurrences(&primitives, &format!("source/{upstream_file}")),
                1
            );
            assert_eq!(
                occurrences(
                    &platform,
                    &format!("#define softfloat_{primitive} se_float_sf_{primitive}")
                ),
                1
            );
            assert!(!rename.contains(&format!("#define softfloat_{primitive} ")));
        }

        for round_pack_name in ["s_roundPackToF32.c", "s_roundPackToF64.c"] {
            assert!(!entries.contains(&round_pack_name));
            assert_eq!(occurrences(&round_pack, round_pack_name), 2);
            assert_eq!(
                occurrences(&round_pack, &format!("#include \"{round_pack_name}\"")),
                1
            );
        }
    }
}
