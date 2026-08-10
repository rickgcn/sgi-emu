const NATIVE_SOURCE: &str = include_str!("../src/native.rs");

#[test]
fn native_production_source_excludes_forbidden_mechanisms() {
    let forbidden = [
        ("foreign ABI declaration", "extern \""),
        ("unsafe code", "unsafe"),
        ("core foreign interface", "core::ffi"),
        ("standard foreign interface", "std::ffi"),
        ("C header inclusion", "#include"),
        ("crate shim symbol", "se_float_shim_"),
        ("inline assembly", "asm!"),
        ("global assembly", "global_asm!"),
        ("core architecture module", "core::arch"),
        ("standard architecture module", "std::arch"),
        ("target-specific function", "target_feature"),
        ("host environment API", "fenv"),
        ("x86 floating-point control register", "mxcsr"),
        ("Arm floating-point control register", "fpcr"),
        ("external math library", "libm"),
        ("algebraic floating-point operation", "algebraic_"),
        ("fused multiply-add", ".mul_add("),
        ("accurate backend dependency", "SoftFloat"),
        ("rounding control type", "RoundingMode"),
        ("accurate outcome type", "Outcome"),
        ("exception flag type", "ExceptionFlags"),
        ("rounding fact type", "RoundingFacts"),
        ("public abstraction declaration", "trait "),
    ];

    for (mechanism, fragment) in forbidden {
        assert!(
            !NATIVE_SOURCE.contains(fragment),
            "native production source contains {mechanism}: {fragment}"
        );
    }

    assert_eq!(NATIVE_SOURCE.matches("    pub fn ").count(), 22);
}
