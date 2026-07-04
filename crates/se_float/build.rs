use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let c_dir = manifest_dir.join("c");
    let workspace_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("crate must live under workspace crates directory");
    let softfloat_dir = workspace_dir.join("3rdparty/softfloat3/source");
    let specialize_dir = softfloat_dir.join("RISCV");
    let include_dir = softfloat_dir.join("include");
    let makefile = workspace_dir.join("3rdparty/softfloat3/build/Linux-x86_64-GCC/Makefile");
    let platform_header = c_dir.join("platform.h");
    let wrapper = c_dir.join("softfloat3_wrapper.c");

    let primitive_objects = makefile_objects(&makefile, "OBJS_PRIMITIVES");
    let specialize_objects = makefile_objects(&makefile, "OBJS_SPECIALIZE");
    let other_objects = makefile_objects(&makefile, "OBJS_OTHERS");

    let mut build = cc::Build::new();
    build
        .warnings(false)
        .include(&c_dir)
        .include(&specialize_dir)
        .include(&include_dir)
        .define("SOFTFLOAT_FAST_INT64", None)
        .define("SOFTFLOAT_ROUND_ODD", None)
        .define("SOFTFLOAT_FAST_DIV32TO16", None)
        .define("SOFTFLOAT_FAST_DIV64TO32", None)
        .file(&wrapper);

    if env::var("CARGO_CFG_TARGET_ENDIAN").unwrap() == "little" {
        build.define("SE_FLOAT_TARGET_LITTLE_ENDIAN", Some("1"));
    }

    for object in primitive_objects.iter().chain(other_objects.iter()) {
        build.file(softfloat_dir.join(format!("{object}.c")));
    }

    for object in specialize_objects {
        build.file(specialize_dir.join(format!("{object}.c")));
    }

    build.compile("se_softfloat3");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", platform_header.display());
    println!("cargo:rerun-if-changed={}", wrapper.display());
    println!("cargo:rerun-if-changed={}", makefile.display());
    println!("cargo:rerun-if-changed={}", softfloat_dir.display());
}

fn makefile_objects(makefile: &Path, section: &str) -> Vec<String> {
    let text = fs::read_to_string(makefile).unwrap();
    let marker = format!("{section} = \\");
    let start = text
        .find(&marker)
        .unwrap_or_else(|| panic!("missing SoftFloat object section {section}"));
    let section_text = &text[start + marker.len()..];
    let end = section_text
        .find("\n\n")
        .unwrap_or_else(|| panic!("unterminated SoftFloat object section {section}"));

    section_text[..end]
        .split_whitespace()
        .filter_map(|token| {
            let token = token.trim_end_matches('\\');
            token.strip_suffix("$(OBJ)").map(str::to_owned)
        })
        .collect()
}
