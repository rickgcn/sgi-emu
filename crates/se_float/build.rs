use std::env;
use std::path::{Path, PathBuf};

const SOFTFLOAT_SOURCES: &[&str] = &[
    "f32_add",
    "f32_div",
    "f32_eq",
    "f32_eq_signaling",
    "f32_lt",
    "f32_mul",
    "f32_mulAdd",
    "f32_sqrt",
    "f32_sub",
    "f32_to_f64",
    "f32_to_i32",
    "f32_to_i64",
    "f64_add",
    "f64_div",
    "f64_eq",
    "f64_eq_signaling",
    "f64_lt",
    "f64_mul",
    "f64_mulAdd",
    "f64_sqrt",
    "f64_sub",
    "f64_to_f32",
    "f64_to_i32",
    "f64_to_i64",
    "i32_to_f32",
    "i32_to_f64",
    "i64_to_f32",
    "i64_to_f64",
    "s_add128",
    "s_addMagsF32",
    "s_addMagsF64",
    "s_approxRecipSqrt32_1",
    "s_approxRecipSqrt_1Ks",
    "s_countLeadingZeros8",
    "s_countLeadingZeros32",
    "s_countLeadingZeros64",
    "s_mul64To128",
    "s_mulAddF32",
    "s_mulAddF64",
    "s_normRoundPackToF32",
    "s_normRoundPackToF64",
    "s_normSubnormalF32Sig",
    "s_normSubnormalF64Sig",
    "s_roundPackToF32",
    "s_roundPackToF64",
    "s_roundToI32",
    "s_roundToI64",
    "s_shiftRightJam32",
    "s_shiftRightJam64",
    "s_shiftRightJam64Extra",
    "s_shiftRightJam128",
    "s_shortShiftLeft128",
    "s_shortShiftRightJam64",
    "s_shortShiftRightJam128",
    "s_sub128",
    "s_subMagsF32",
    "s_subMagsF64",
    "softfloat_state",
];

const MIPS4_SPECIALIZATION_SOURCES: &[&str] = &[
    "s_commonNaNToF32UI",
    "s_commonNaNToF64UI",
    "s_f32UIToCommonNaN",
    "s_f64UIToCommonNaN",
    "s_propagateNaNF32UI",
    "s_propagateNaNF64UI",
    "softfloat_raiseFlags",
];

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let c_dir = manifest_dir.join("c");
    let workspace_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("crate must live under workspace crates directory");
    let softfloat_dir = workspace_dir.join("3rdparty/softfloat3/source");
    let specialize_dir = c_dir.join("softfloat3/mips4");
    let include_dir = softfloat_dir.join("include");
    let platform_header = c_dir.join("platform.h");
    let wrapper = c_dir.join("softfloat3_wrapper.c");
    let little_endian = env::var("CARGO_CFG_TARGET_ENDIAN").unwrap() == "little";

    let mut wrapper_build = configured_build(&c_dir, &specialize_dir, &include_dir, little_endian);
    wrapper_build
        .warnings(true)
        .extra_warnings(true)
        .warnings_into_errors(true)
        .file(&wrapper)
        .compile("se_softfloat3_wrapper");

    let mut softfloat_build =
        configured_build(&c_dir, &specialize_dir, &include_dir, little_endian);
    softfloat_build.warnings(false);
    add_sources(&mut softfloat_build, &softfloat_dir, SOFTFLOAT_SOURCES);
    add_sources(
        &mut softfloat_build,
        &specialize_dir,
        MIPS4_SPECIALIZATION_SOURCES,
    );
    softfloat_build.compile("se_softfloat3_mips4");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", platform_header.display());
    println!("cargo:rerun-if-changed={}", wrapper.display());
    println!(
        "cargo:rerun-if-changed={}",
        specialize_dir.join("specialize.h").display()
    );
    emit_source_reruns(&softfloat_dir, SOFTFLOAT_SOURCES);
    emit_source_reruns(&specialize_dir, MIPS4_SPECIALIZATION_SOURCES);
    for header in [
        "internals.h",
        "opts-GCC.h",
        "primitiveTypes.h",
        "primitives.h",
        "softfloat.h",
        "softfloat_types.h",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            include_dir.join(header).display()
        );
    }
}

fn configured_build(
    c_dir: &Path,
    specialize_dir: &Path,
    include_dir: &Path,
    little_endian: bool,
) -> cc::Build {
    let mut build = cc::Build::new();
    build
        .include(c_dir)
        .include(specialize_dir)
        .include(include_dir)
        .define("SOFTFLOAT_FAST_INT64", None)
        .define("SOFTFLOAT_FAST_DIV64TO32", None);
    if little_endian {
        build.define("SE_FLOAT_TARGET_LITTLE_ENDIAN", Some("1"));
    }
    build
}

fn add_sources(build: &mut cc::Build, directory: &Path, sources: &[&str]) {
    for source in sources {
        build.file(directory.join(format!("{source}.c")));
    }
}

fn emit_source_reruns(directory: &Path, sources: &[&str]) {
    for source in sources {
        println!(
            "cargo:rerun-if-changed={}",
            directory.join(format!("{source}.c")).display()
        );
    }
}
