use std::env;
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

const SLIRP_SOURCES: &[&str] = &[
    "arp_table",
    "bootp",
    "cksum",
    "dhcpv6",
    "dnssearch",
    "if",
    "ip6_icmp",
    "ip6_input",
    "ip6_output",
    "ip_icmp",
    "ip_input",
    "ip_output",
    "mbuf",
    "misc",
    "ncsi",
    "ndp_table",
    "sbuf",
    "slirp",
    "socket",
    "state",
    "stream",
    "tcp_input",
    "tcp_output",
    "tcp_subr",
    "tcp_timer",
    "tftp",
    "udp",
    "udp6",
    "util",
    "version",
    "vmstate",
];

pub fn compile() {
    for variable in [
        "SE_NETWORK_MESON",
        "SE_NETWORK_OFFLINE",
        "MESON_PACKAGE_CACHE_DIR",
        "CC",
        "CFLAGS",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap().parent().unwrap();
    let slirp = root.join("3rdparty/libslirp");
    let glib = root.join("3rdparty/glib");
    for source in [
        &slirp,
        &glib,
        &manifest.join("csrc"),
        &manifest.join("build"),
    ] {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    verify_revision(&slirp, "d09dc9a70360ee7838acccbd234ab83f3b9b37a5");
    verify_revision(&glib, "43bc79ea8803e33c5eb368085e2d5906f9f98079");
    let target = env::var("TARGET").unwrap();
    assert_eq!(
        env::var("HOST").unwrap(),
        target,
        "native NAT dependencies require a native target build"
    );
    let compiler = cc::Build::new().get_compiler();
    let static_crt = env::var("CARGO_CFG_TARGET_FEATURE")
        .unwrap_or_default()
        .split(',')
        .any(|f| f == "crt-static");
    let mut key = std::collections::hash_map::DefaultHasher::new();
    (
        target.as_str(),
        compiler.path(),
        compiler.args(),
        static_crt,
        "glib-2.88.3",
        include_bytes!("native.rs").as_slice(),
    )
        .hash(&mut key);
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let native = out.join(format!("native-{:016x}", key.finish()));
    let source = native.join("source");
    let build = native.join("build");
    if !source.join(".prepared").exists() {
        copy_tree(&glib, &source);
        fs::write(source.join(".prepared"), b"glib-2.88.3").unwrap();
    }
    // Empty gitlink directories must not shadow Meson's pinned wrap fallbacks.
    for entry in fs::read_dir(source.join("subprojects")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir()
            && fs::read_dir(entry.path()).unwrap().next().is_none()
        {
            fs::remove_dir(entry.path()).unwrap();
        }
    }
    let meson = env::var_os("SE_NETWORK_MESON").unwrap_or_else(|| OsString::from("meson"));
    let native_file = native.join("compiler.ini");
    let arguments = compiler
        .args()
        .iter()
        .map(|arg| meson_string(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        &native_file,
        format!(
            "[binaries]\nc = [{}]\n[built-in options]\nc_args = [{arguments}]\n",
            meson_string(&compiler.path().to_string_lossy().replace('\\', "/")),
        ),
    )
    .unwrap();
    let mut setup = native_command(&meson, &compiler);
    setup
        .arg("setup")
        .arg(&build)
        .arg(&source)
        .arg("--native-file")
        .arg(&native_file)
        .args([
            "--buildtype=release",
            "--default-library=static",
            "--force-fallback-for=libpcre2-8,libffi,zlib,intl,gvdb",
            "-Dtests=false",
            "-Dinstalled_tests=false",
            "-Ddocumentation=false",
            "-Dman-pages=disabled",
            "-Dintrospection=disabled",
            "-Dsysprof=disabled",
            "-Dlibmount=disabled",
            "-Dselinux=disabled",
            "-Dlibelf=disabled",
            "-Ddtrace=disabled",
            "-Dsystemtap=disabled",
            "-Dglib_debug=disabled",
            "-Dnls=disabled",
        ]);
    if target.contains("msvc") {
        setup.arg(if static_crt {
            "-Db_vscrt=mt"
        } else {
            "-Db_vscrt=md"
        });
    }
    if build.join("build.ninja").exists() {
        setup.arg("--reconfigure");
    }
    if env::var_os("SE_NETWORK_OFFLINE").is_some() {
        setup.arg("--wrap-mode=nodownload");
    }
    run(
        &mut setup,
        "configure fixed GLib dependencies (Meson/Python/Ninja and source downloads are required)",
    );
    let config = fs::read_to_string(build.join("config.h")).unwrap();
    let targets: serde_json::Value =
        serde_json::from_slice(&fs::read(build.join("meson-info/intro-targets.json")).unwrap())
            .expect("Meson target introspection must be valid JSON");
    let mut names = vec!["glib-2.0", "charset", "pcre2-8", "intl"];
    if !config_enabled(&config, "USE_SYSTEM_PRINTF") {
        names.insert(2, "gnulib");
    }
    let libraries: Vec<_> = names
        .iter()
        .map(|name| static_archive(&targets, name))
        .collect();
    run(
        native_command(&meson, &compiler)
            .arg("compile")
            .arg("-C")
            .arg(&build)
            .args(&names),
        "compile static GLib and its archive dependencies",
    );
    let version = fs::read_to_string(slirp.join("src/libslirp-version.h.in"))
        .unwrap()
        .replace("@SLIRP_MAJOR_VERSION@", "4")
        .replace("@SLIRP_MINOR_VERSION@", "9")
        .replace("@SLIRP_MICRO_VERSION@", "4")
        .replace("@SLIRP_VERSION_STRING@", "\"4.9.4\"");
    fs::write(out.join("libslirp-version.h"), version).unwrap();
    let mut cc = cc::Build::new();
    cc.std(if target.contains("msvc") {
        "c11"
    } else {
        "gnu11"
    })
    .warnings(false)
    .include(slirp.join("src"))
    .include(&out)
    .include(&source)
    .include(source.join("glib"))
    .include(&build)
    .include(build.join("glib"))
    .include(manifest.join("csrc"))
    .define("LIBSLIRP_STATIC", None)
    .define("GLIB_STATIC_COMPILATION", None)
    .define("BUILDING_LIBSLIRP", None)
    .define("G_LOG_DOMAIN", "\"Slirp\"");
    for file in SLIRP_SOURCES {
        cc.file(slirp.join(format!("src/{file}.c")));
    }
    cc.file(manifest.join("csrc/slirp_bridge.c"))
        .compile("se_network_slirp");
    for library in libraries {
        assert!(
            library.is_file(),
            "missing static archive {}",
            library.display()
        );
        let stem = library.file_stem().unwrap().to_str().unwrap();
        let name = if library.extension().is_some_and(|ext| ext == "a") {
            stem.strip_prefix("lib").unwrap_or(stem)
        } else {
            stem
        };
        println!(
            "cargo:rustc-link-search=native={}",
            library.parent().unwrap().display()
        );
        println!("cargo:rustc-link-lib=static={name}");
    }
    let system = if target.contains("windows") {
        &[
            "ws2_32", "iphlpapi", "ole32", "winmm", "shlwapi", "uuid", "user32", "advapi32",
            "shell32",
        ][..]
    } else if target.contains("apple") {
        &["iconv", "resolv", "pthread", "m"][..]
    } else {
        &["pthread", "m", "dl", "rt"][..]
    };
    for library in system {
        println!("cargo:rustc-link-lib={library}");
    }
    if target.contains("apple") {
        if config_enabled(&config, "HAVE_COCOA") {
            for framework in ["Foundation", "CoreFoundation", "AppKit"] {
                println!("cargo:rustc-link-lib=framework={framework}");
            }
        }
        if config_enabled(&config, "HAVE_CARBON") {
            println!("cargo:rustc-link-lib=framework=Carbon");
        }
    }
}

fn meson_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn config_enabled(config: &str, name: &str) -> bool {
    config.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("#define")
            && words.next() == Some(name)
            && matches!(words.next(), None | Some("1"))
    })
}

/// Resolves only the configured archive target, never unrelated build artifacts.
fn static_archive(targets: &serde_json::Value, name: &str) -> PathBuf {
    let matching: Vec<_> = targets
        .as_array()
        .expect("Meson targets must be an array")
        .iter()
        .filter(|target| {
            target["name"].as_str() == Some(name)
                && target["type"].as_str() == Some("static library")
        })
        .collect();
    assert_eq!(matching.len(), 1, "expected one static Meson target {name}");
    let filenames = matching[0]["filename"]
        .as_array()
        .expect("Meson target filenames must be an array");
    assert_eq!(filenames.len(), 1, "expected one archive for {name}");
    PathBuf::from(
        filenames[0]
            .as_str()
            .expect("Meson archive filename must be a string"),
    )
}

fn native_command(program: &OsString, compiler: &cc::Tool) -> Command {
    let mut command = Command::new(program);
    command.envs(compiler.env().iter().cloned());
    let mut paths = vec![compiler.path().parent().unwrap().to_path_buf()];
    if let Some(parent) = Path::new(program)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        paths.push(parent.to_path_buf());
    }
    let compiler_path = compiler
        .env()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(_, v)| v.clone());
    paths.extend(env::split_paths(
        &compiler_path
            .or_else(|| env::var_os("PATH"))
            .unwrap_or_default(),
    ));
    command.env("PATH", env::join_paths(paths).unwrap());
    command
}

fn verify_revision(source: &Path, expected: &str) {
    assert!(
        source.join("meson.build").exists(),
        "missing {}: run git submodule update --init --recursive",
        source.display()
    );
    let result = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("Git is required to verify native source versions");
    assert!(
        result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == expected,
        "{} must use the pinned submodule revision {expected}",
        source.display()
    );
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".git" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn run(command: &mut Command, description: &str) {
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("Cannot {description}: {error}"));
    assert!(
        result.status.success(),
        "Cannot {description}:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
