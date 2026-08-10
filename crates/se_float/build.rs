use std::env;
use std::path::{Path, PathBuf};

const UPSTREAM_TRANSLATION_UNITS: &[&str] = &[
    "RISCV/s_propagateNaNF32UI.c",
    "RISCV/s_propagateNaNF64UI.c",
    "RISCV/softfloat_raiseFlags.c",
    "f32_add.c",
    "f32_div.c",
    "f32_eq.c",
    "f32_lt_quiet.c",
    "f32_mul.c",
    "f32_sqrt.c",
    "f32_sub.c",
    "f32_to_f64.c",
    "f32_to_i32.c",
    "f32_to_i64.c",
    "f64_add.c",
    "f64_div.c",
    "f64_eq.c",
    "f64_lt_quiet.c",
    "f64_mul.c",
    "f64_sqrt.c",
    "f64_sub.c",
    "f64_to_f32.c",
    "f64_to_i32.c",
    "f64_to_i64.c",
    "i32_to_f32.c",
    "i32_to_f64.c",
    "i64_to_f32.c",
    "i64_to_f64.c",
    "s_addMagsF32.c",
    "s_addMagsF64.c",
    "s_approxRecipSqrt_1Ks.c",
    "s_approxRecip_1Ks.c",
    "s_countLeadingZeros8.c",
    "s_normRoundPackToF32.c",
    "s_normRoundPackToF64.c",
    "s_normSubnormalF32Sig.c",
    "s_normSubnormalF64Sig.c",
    "s_roundToI32.c",
    "s_roundToI64.c",
    "s_subMagsF32.c",
    "s_subMagsF64.c",
    "softfloat_state.c",
];

const LOCAL_SOURCES: &[&str] = &[
    "csrc/platform.h",
    "csrc/primitives.c",
    "csrc/probe.c",
    "csrc/rename.h",
    "csrc/round_pack.c",
    "csrc/round_pack.h",
    "csrc/shim.c",
    "csrc/shim.h",
];

const UPSTREAM_HEADERS: &[&str] = &[
    "RISCV/specialize.h",
    "include/internals.h",
    "include/primitiveTypes.h",
    "include/primitives.h",
    "include/softfloat.h",
    "include/softfloat_types.h",
    "s_roundPackToF32.c",
    "s_roundPackToF64.c",
];

#[derive(Clone, Copy)]
enum CompilerProfile {
    Gcc,
    AppleClang,
    Msvc,
}

fn main() {
    let manifest_dir = PathBuf::from(required_env("CARGO_MANIFEST_DIR"));
    let source_dir = manifest_dir.join("../../3rdparty/softfloat3/source");
    let csrc_dir = manifest_dir.join("csrc");
    let target = required_env("TARGET");

    validate_target(&target);
    validate_manifest(&source_dir, &manifest_dir);
    emit_rerun_paths(&source_dir, &manifest_dir);

    let profile = detect_compiler(&target);

    let mut probe = configured_build(profile, &target, &csrc_dir, &source_dir);
    probe.cargo_metadata(false);
    strict_warnings(&mut probe, profile);
    probe.file(csrc_dir.join("probe.c"));
    let _probe_objects = probe.compile_intermediates();

    let mut upstream = configured_build(profile, &target, &csrc_dir, &source_dir);
    upstream_warnings(&mut upstream, profile);
    for path in upstream_paths(&source_dir) {
        upstream.file(path);
    }
    let upstream_objects = upstream.compile_intermediates();

    let mut shim = configured_build(profile, &target, &csrc_dir, &source_dir);
    strict_warnings(&mut shim, profile);
    shim.file(csrc_dir.join("shim.c"));
    shim.compile("se_float_shim");

    let mut softfloat = configured_build(profile, &target, &csrc_dir, &source_dir);
    strict_warnings(&mut softfloat, profile);
    softfloat.objects(upstream_objects);
    softfloat.file(csrc_dir.join("primitives.c"));
    softfloat.file(csrc_dir.join("round_pack.c"));
    softfloat.compile("se_float_softfloat");
}

fn required_env(name: &str) -> String {
    env::var(name)
        .unwrap_or_else(|_| panic!("se_float build requires Cargo environment variable {name}"))
}

fn validate_target(target: &str) {
    match target {
        "aarch64-apple-darwin" | "x86_64-pc-windows-msvc" | "x86_64-unknown-linux-gnu" => {}
        _ => panic!(
            "unsupported se_float target {target}; supported targets are aarch64-apple-darwin, x86_64-pc-windows-msvc, and x86_64-unknown-linux-gnu"
        ),
    }

    let pointer_width = required_env("CARGO_CFG_TARGET_POINTER_WIDTH");
    assert!(
        pointer_width == "64",
        "unsupported se_float pointer width {pointer_width}; the fixed profile requires 64 bits"
    );
    let endian = required_env("CARGO_CFG_TARGET_ENDIAN");
    assert!(
        endian == "little",
        "unsupported se_float target endianness {endian}; the validated targets are little-endian"
    );
}

fn validate_manifest(source_dir: &Path, manifest_dir: &Path) {
    assert!(
        source_dir.join("include/softfloat.h").is_file(),
        "Berkeley SoftFloat sources are missing; initialize 3rdparty/softfloat3 recursively"
    );

    for pair in UPSTREAM_TRANSLATION_UNITS.windows(2) {
        assert!(
            pair[0] < pair[1],
            "se_float upstream translation-unit manifest must be strictly sorted: {} then {}",
            pair[0],
            pair[1]
        );
    }

    for path in upstream_paths(source_dir) {
        assert!(
            path.is_file(),
            "required Berkeley SoftFloat translation unit is missing: {}",
            path.display()
        );
    }
    for path in UPSTREAM_HEADERS {
        let path = source_dir.join(path);
        assert!(
            path.is_file(),
            "required Berkeley SoftFloat source or header is missing: {}",
            path.display()
        );
    }
    for path in LOCAL_SOURCES {
        let path = manifest_dir.join(path);
        assert!(
            path.is_file(),
            "required se_float C source or header is missing: {}",
            path.display()
        );
    }
}

fn emit_rerun_paths(source_dir: &Path, manifest_dir: &Path) {
    println!("cargo:rerun-if-changed=build.rs");
    for path in upstream_paths(source_dir) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for path in UPSTREAM_HEADERS {
        println!("cargo:rerun-if-changed={}", source_dir.join(path).display());
    }
    for path in LOCAL_SOURCES {
        println!(
            "cargo:rerun-if-changed={}",
            manifest_dir.join(path).display()
        );
    }
}

fn upstream_paths(source_dir: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    UPSTREAM_TRANSLATION_UNITS
        .iter()
        .map(|path| source_dir.join(path))
}

fn detect_compiler(target: &str) -> CompilerProfile {
    let mut detection = cc::Build::new();
    detection.target(target);
    let compiler = detection.get_compiler();
    match target {
        "x86_64-unknown-linux-gnu" if compiler.is_like_gnu() && !compiler.is_like_clang() => {
            CompilerProfile::Gcc
        }
        "aarch64-apple-darwin" if compiler.is_like_clang() => CompilerProfile::AppleClang,
        "x86_64-pc-windows-msvc" if compiler.is_like_msvc() && !compiler.is_like_clang() => {
            CompilerProfile::Msvc
        }
        _ => panic!(
            "unsupported C compiler {} for se_float target {target}",
            compiler.path().display()
        ),
    }
}

fn configured_build(
    profile: CompilerProfile,
    target: &str,
    csrc_dir: &Path,
    source_dir: &Path,
) -> cc::Build {
    let mut build = cc::Build::new();
    build
        .target(target)
        .std("c11")
        .include(csrc_dir)
        .include(source_dir.join("RISCV"))
        .include(source_dir.join("include"))
        .include(source_dir)
        .define("SOFTFLOAT_FAST_INT64", None)
        .define("SE_FLOAT_TARGET_LITTLE_ENDIAN", None);
    match profile {
        CompilerProfile::Gcc => {
            build.define("SE_FLOAT_COMPILER_GCC", None);
        }
        CompilerProfile::AppleClang => {
            build.define("SE_FLOAT_COMPILER_APPLE_CLANG", None);
        }
        CompilerProfile::Msvc => {
            build.define("SE_FLOAT_COMPILER_MSVC", None);
        }
    }
    build
}

fn strict_warnings(build: &mut cc::Build, profile: CompilerProfile) {
    match profile {
        CompilerProfile::Gcc | CompilerProfile::AppleClang => {
            build.flag("-Wall");
            build.flag("-Wextra");
            build.flag("-Wpedantic");
            build.flag("-Werror");
        }
        CompilerProfile::Msvc => {
            build.flag("/W4");
            build.flag("/WX");
        }
    }
}

fn upstream_warnings(build: &mut cc::Build, profile: CompilerProfile) {
    strict_warnings(build, profile);
    match profile {
        CompilerProfile::Gcc => {
            build.flag("-Wno-unused-parameter");
            build.flag("-Wno-unused-variable");
        }
        CompilerProfile::AppleClang => {}
        CompilerProfile::Msvc => {
            build.flag("/wd4100");
            build.flag("/wd4101");
            build.flag("/wd4146");
            build.flag("/wd4244");
        }
    }
}
