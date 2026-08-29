use std::env;
use std::path::{Path, PathBuf};

const PRIMITIVE_SOURCES: &[&str] = &[
    "s_approxRecip_1Ks.c",
    "s_approxRecip32_1.c",
    "s_countLeadingZeros8.c",
    "s_countLeadingZeros32.c",
    "s_countLeadingZeros64.c",
    "s_mul64To128.c",
    "s_shiftRightJam32.c",
    "s_shiftRightJam64.c",
    "s_shortShiftRightJam64.c",
];

const CORE_SOURCES: &[&str] = &[
    "softfloat_state.c",
    "s_roundToI32.c",
    "s_normSubnormalF32Sig.c",
    "s_roundPackToF32.c",
    "s_normRoundPackToF32.c",
    "s_addMagsF32.c",
    "s_subMagsF32.c",
    "s_normSubnormalF64Sig.c",
    "s_roundPackToF64.c",
    "s_normRoundPackToF64.c",
    "s_addMagsF64.c",
    "s_subMagsF64.c",
];

const OPERATION_SOURCES: &[&str] = &[
    "i32_to_f32.c",
    "i32_to_f64.c",
    "f32_to_f64.c",
    "f64_to_f32.c",
    "f32_to_i32.c",
    "f64_to_i32.c",
    "f32_add.c",
    "f32_sub.c",
    "f32_mul.c",
    "f32_div.c",
    "f64_add.c",
    "f64_sub.c",
    "f64_mul.c",
    "f64_div.c",
    "f32_eq.c",
    "f32_eq_signaling.c",
    "f32_lt_quiet.c",
    "f64_eq.c",
    "f64_eq_signaling.c",
    "f64_lt_quiet.c",
];

const SPECIALIZATION_SOURCES: &[&str] = &[
    "softfloat_raiseFlags.c",
    "s_f32UIToCommonNaN.c",
    "s_f64UIToCommonNaN.c",
    "s_commonNaNToF32UI.c",
    "s_commonNaNToF64UI.c",
    "s_propagateNaNF32UI.c",
    "s_propagateNaNF64UI.c",
];

const UPSTREAM_HEADERS: &[&str] = &[
    "internals.h",
    "primitives.h",
    "primitiveTypes.h",
    "softfloat.h",
    "softfloat_types.h",
];

pub fn compile() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo must provide target arch");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo must provide target OS");
    let supported = matches!(
        (target_arch.as_str(), target_os.as_str()),
        ("x86_64", "windows") | ("x86_64", "linux") | ("aarch64", "macos")
    );
    assert!(
        supported,
        "se_float supports only Windows x86_64, Linux x86_64, and macOS aarch64"
    );

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide manifest dir"));
    let csrc_dir = manifest_dir.join("csrc");
    let specialization_dir = csrc_dir.join("mips_legacy");
    let upstream_dir = manifest_dir.join("../../3rdparty/softfloat3/source");
    let upstream_include_dir = upstream_dir.join("include");

    let bridge_source = csrc_dir.join("bridge.c");
    track(&bridge_source);
    track(&csrc_dir.join("bridge.h"));
    track(&csrc_dir.join("platform.h"));
    track(&specialization_dir.join("specialize.h"));

    let mut build = cc::Build::new();
    build
        .std("c11")
        .warnings(true)
        .include(&csrc_dir)
        .include(&specialization_dir)
        .include(&upstream_include_dir)
        .define("SOFTFLOAT_FAST_INT64", None)
        .file(&bridge_source);

    for source in PRIMITIVE_SOURCES
        .iter()
        .chain(CORE_SOURCES)
        .chain(OPERATION_SOURCES)
    {
        let path = upstream_dir.join(source);
        track(&path);
        build.file(path);
    }

    for source in SPECIALIZATION_SOURCES {
        let path = specialization_dir.join(source);
        track(&path);
        build.file(path);
    }

    for header in UPSTREAM_HEADERS {
        track(&upstream_include_dir.join(header));
    }

    build.compile("se_float_softfloat");
}

fn track(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}
