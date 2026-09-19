use std::env;
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

const SLIRP_SOURCES: &[&str] = &[
    "arp_table",
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
    "udp",
    "udp6",
    "util",
    "version",
    "vmstate",
];

/// Downstream BOOTP patch for legacy client compatibility.
const BOOTP_PATCH: &str = "build/bootp-client-compat.patch";

/// Downstream TFTP patch that routes host file access through GLib.
const TFTP_PATCH: &str = "build/tftp-host-path.patch";

/// Pinned libslirp translation units replaced by reviewed downstream adaptations.
const SLIRP_ADAPTATIONS: &[(&str, &str)] = &[("bootp", BOOTP_PATCH), ("tftp", TFTP_PATCH)];

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
    for (name, _) in SLIRP_ADAPTATIONS {
        assert!(
            !SLIRP_SOURCES.contains(name),
            "{name} must only be compiled from the adapted source"
        );
    }
    // Adapting the pinned source first keeps a revision mismatch an immediate,
    // cheap failure instead of one after the fixed GLib build.
    let adapted_sources = SLIRP_ADAPTATIONS
        .iter()
        .map(|(name, patch)| adapted_slirp_source(&manifest, &slirp, &native, name, patch))
        .collect::<Vec<_>>();
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
    for source in adapted_sources {
        cc.file(source);
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

/// Writes one adapted libslirp compilation unit and returns its generated path.
///
/// The submodule is never edited: each change lives in a downstream patch and
/// is applied in memory. A hunk that no longer matches the pinned source fails
/// the build, so a new revision cannot silently drop an adaptation, and the
/// original translation unit is not compiled beside the generated one.
fn adapted_slirp_source(
    manifest: &Path,
    slirp: &Path,
    native: &Path,
    name: &str,
    patch_name: &str,
) -> PathBuf {
    let origin = slirp.join(format!("src/{name}.c"));
    let patch = manifest.join(patch_name);
    let source = fs::read_to_string(&origin).unwrap_or_else(|error| {
        panic!("cannot read {}: {error}", origin.display());
    });
    let diff = fs::read_to_string(&patch).unwrap_or_else(|error| {
        panic!("cannot read {}: {error}", patch.display());
    });
    fs::create_dir_all(native).unwrap();
    let generated = native.join(format!("{name}.c"));
    fs::write(&generated, apply_patch(&source, &diff, name, patch_name)).unwrap();
    generated
}

/// One whole-line hunk of a unified diff, addressed by its unpatched position.
struct Hunk {
    /// One-based first line this hunk covers in the unpatched file.
    old_start: usize,
    /// Hunk body in order, each line tagged with its ` `, `+` or `-` marker.
    body: Vec<(char, String)>,
}

impl Hunk {
    /// Counts the lines this hunk consumes from the unpatched file.
    fn old_len(&self) -> usize {
        self.body
            .iter()
            .filter(|(marker, _)| *marker != '+')
            .count()
    }

    /// Iterates the lines this hunk expects in the unpatched file.
    fn old_lines(&self) -> impl Iterator<Item = &str> {
        self.body
            .iter()
            .filter(|(marker, _)| *marker != '+')
            .map(|(_, line)| line.as_str())
    }

    /// Iterates the lines this hunk leaves in the patched file.
    fn new_lines(&self) -> impl Iterator<Item = &str> {
        self.body
            .iter()
            .filter(|(marker, _)| *marker != '-')
            .map(|(_, line)| line.as_str())
    }
}

/// Applies one unified diff to a pinned source file and returns the result.
///
/// Only whole-line hunks with context are supported, which is everything the
/// downstream patches need. Each hunk must match the pinned source at its
/// recorded position, so an unexpected revision fails the build instead of
/// producing a partially adapted compilation unit.
fn apply_patch(source: &str, diff: &str, name: &str, patch_name: &str) -> String {
    assert!(
        source.ends_with('\n'),
        "pinned libslirp {name}.c must end with a newline"
    );
    let mut lines: Vec<String> = source.lines().map(str::to_owned).collect();
    let mut shift = 0isize;
    let mut applied = 0usize;
    for hunk in parse_hunks(diff, patch_name) {
        let start = usize::try_from(hunk.old_start as isize - 1 + shift).unwrap_or_else(|_| {
            panic!(
                "{patch_name} addresses line {} before the pinned libslirp {name}.c",
                hunk.old_start
            )
        });
        assert!(
            lines
                .get(start..start + hunk.old_len())
                .is_some_and(|window| window.iter().map(String::as_str).eq(hunk.old_lines())),
            "pinned libslirp {name}.c does not match the hunk at line {}; \
             review {patch_name} against the pinned revision",
            hunk.old_start
        );
        let replacement = hunk.new_lines().map(str::to_owned).collect::<Vec<_>>();
        shift += replacement.len() as isize - hunk.old_len() as isize;
        lines.splice(start..start + hunk.old_len(), replacement);
        applied += 1;
    }
    assert!(applied > 0, "{patch_name} must contain a hunk");
    let mut result = lines.join("\n");
    result.push('\n');
    result
}

/// Parses the hunks of one unified diff, ignoring its file headers.
fn parse_hunks(diff: &str, patch_name: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            let old_start = header
                .split(' ')
                .next()
                .and_then(|field| field.strip_prefix('-'))
                .and_then(|field| field.split(',').next())
                .and_then(|start| start.parse().ok())
                .unwrap_or_else(|| panic!("unreadable hunk header {line:?}"));
            hunks.push(Hunk {
                old_start,
                body: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };
        let (marker, content) = match line.as_bytes().first() {
            Some(b' ') => (' ', &line[1..]),
            Some(b'+') => ('+', &line[1..]),
            Some(b'-') => ('-', &line[1..]),
            // Every hunk line carries its marker, so an empty context line is
            // written as a single space and a blank addition as a lone plus.
            None => panic!("{patch_name} has a zero-length line inside a hunk"),
            Some(_) => panic!("{patch_name} has an unsupported line {line:?}"),
        };
        hunk.body.push((marker, content.to_owned()));
    }
    hunks
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
