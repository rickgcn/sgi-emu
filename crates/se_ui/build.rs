use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use qt_build_utils::{QtBuild, QtInstallationQMake};

const CPP_SOURCES: [&str; 14] = [
    "cpp/src/main_window.cpp",
    "cpp/src/display_widget.cpp",
    "cpp/src/display_workspace.cpp",
    "cpp/src/frontend/host_mouse_capture.cpp",
    "cpp/src/endpoint_identity.cpp",
    "cpp/src/machine_output_sink.cpp",
    "cpp/src/settings_dialog.cpp",
    "cpp/src/debugger/registers_dock.cpp",
    "cpp/src/debugger/tlb_dock.cpp",
    "cpp/src/debugger/cache_dock.cpp",
    "cpp/src/debugger/disassembly_dock.cpp",
    "cpp/src/debugger/memory_dock.cpp",
    "cpp/src/serial_console_dock.cpp",
    "cpp/src/vt100_widget.cpp",
];

const HEADERS: [&str; 15] = [
    "cpp/include/se_ui/main_window.h",
    "cpp/include/se_ui/display_widget.h",
    "cpp/include/se_ui/display_workspace.h",
    "cpp/include/se_ui/frontend/host_mouse_capture.h",
    "cpp/src/frontend/host_mouse_capture_p.h",
    "cpp/include/se_ui/endpoint_identity.h",
    "cpp/include/se_ui/machine_output_sink.h",
    "cpp/include/se_ui/settings_dialog.h",
    "cpp/include/se_ui/debugger/registers_dock.h",
    "cpp/include/se_ui/debugger/tlb_dock.h",
    "cpp/include/se_ui/debugger/cache_dock.h",
    "cpp/include/se_ui/debugger/disassembly_dock.h",
    "cpp/include/se_ui/debugger/memory_dock.h",
    "cpp/include/se_ui/serial_console_dock.h",
    "cpp/include/se_ui/vt100_widget.h",
];

const PLATFORM_SOURCES: [&str; 4] = [
    "cpp/src/frontend/host_mouse_capture_windows.cpp",
    "cpp/src/frontend/host_mouse_capture_x11.cpp",
    "cpp/src/frontend/host_mouse_capture_wayland.cpp",
    "cpp/src/frontend/host_mouse_capture_macos.mm",
];

#[cfg(windows)]
const QMAKE_NAMES: [&str; 2] = ["qmake6.exe", "qmake.exe"];

#[cfg(not(windows))]
const QMAKE_NAMES: [&str; 2] = ["qmake6", "qmake"];

fn main() -> Result<(), Box<dyn Error>> {
    for variable in ["QMAKE", "QT_DIR", "QT_ROOT_DIR"] {
        println!("cargo::rerun-if-env-changed={variable}");
    }
    println!("cargo::rerun-if-changed=src/bridge.rs");
    for path in CPP_SOURCES
        .iter()
        .chain(HEADERS.iter())
        .chain(PLATFORM_SOURCES.iter())
    {
        println!("cargo::rerun-if-changed={path}");
    }

    let qmake = find_qmake()?;
    let installation = QtInstallationQMake::try_from(qmake.clone())?;
    let qt = QtBuild::with_installation(
        Box::new(installation),
        ["Core", "Gui", "Widgets"]
            .into_iter()
            .map(String::from)
            .collect(),
    );
    if qt.version().major != 6 {
        return Err(io::Error::other(format!(
            "sgi-emu requires Qt 6, but qmake reported Qt {}",
            qt.version()
        ))
        .into());
    }

    let mut build = cxx_build::bridge("src/bridge.rs");
    build
        .files(CPP_SOURCES)
        .include("cpp/include")
        .includes(qt.include_paths())
        .std("c++17");
    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => {
            build.file("cpp/src/frontend/host_mouse_capture_windows.cpp");
        }
        Ok("linux") => {
            configure_linux_mouse(&mut build, &qmake, &qt.version().to_string())?;
        }
        Ok("macos") => {
            build.file("cpp/src/frontend/host_mouse_capture_macos.mm");
            println!("cargo::rustc-link-lib=framework=ApplicationServices");
        }
        _ => {}
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flags(["/Zc:__cplusplus", "/permissive-"]);
    }
    qt.cargo_link_libraries(&mut build);
    build.compile("se_ui_qt");

    Ok(())
}

fn configure_linux_mouse(
    build: &mut cc::Build,
    qmake: &Path,
    qt_version: &str,
) -> Result<(), Box<dyn Error>> {
    build.files([
        "cpp/src/frontend/host_mouse_capture_wayland.cpp",
        "cpp/src/frontend/host_mouse_capture_x11.cpp",
    ]);

    let wayland = pkg_config::Config::new().probe("wayland-client")?;
    build.includes(wayland.include_paths.clone());
    for package in ["x11", "xi"] {
        let library = pkg_config::Config::new().probe(package)?;
        build.includes(library.include_paths);
    }

    let qt_headers = qmake_query(qmake, "QT_INSTALL_HEADERS")?;
    build.include(
        PathBuf::from(qt_headers)
            .join("QtGui")
            .join(qt_version)
            .join("QtGui"),
    );

    let protocol_root = PathBuf::from(pkg_config::get_variable("wayland-protocols", "pkgdatadir")?);
    let protocols = [
        (
            protocol_root.join("unstable/relative-pointer/relative-pointer-unstable-v1.xml"),
            "relative-pointer-unstable-v1-client-protocol",
        ),
        (
            protocol_root.join("unstable/pointer-constraints/pointer-constraints-unstable-v1.xml"),
            "pointer-constraints-unstable-v1-client-protocol",
        ),
    ];
    let scanner = pkg_config::get_variable("wayland-scanner", "wayland_scanner")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("wayland-scanner"));
    let output_directory =
        PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Cargo did not provide OUT_DIR")
        })?);
    let mut protocol_sources = Vec::new();
    for (xml, name) in protocols {
        println!("cargo::rerun-if-changed={}", xml.display());
        let header = output_directory.join(format!("{name}.h"));
        let source = output_directory.join(format!("{name}.c"));
        run_wayland_scanner(&scanner, "client-header", &xml, &header)?;
        run_wayland_scanner(&scanner, "private-code", &xml, &source)?;
        protocol_sources.push(source);
    }
    build.include(&output_directory);

    let mut protocol_build = cc::Build::new();
    protocol_build
        .files(protocol_sources)
        .includes(wayland.include_paths);
    protocol_build.compile("se_ui_wayland_protocols");
    Ok(())
}

fn qmake_query(qmake: &Path, variable: &str) -> Result<String, Box<dyn Error>> {
    let output = Command::new(qmake).args(["-query", variable]).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("qmake failed to query {variable}")).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn run_wayland_scanner(
    scanner: &Path,
    mode: &str,
    input: &Path,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    let status = Command::new(scanner)
        .args([mode])
        .arg(input)
        .arg(output)
        .status()?;
    if !status.success() {
        return Err(
            io::Error::other(format!("wayland-scanner failed for {}", input.display())).into(),
        );
    }
    Ok(())
}

fn find_qmake() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(qmake) = env::var_os("QMAKE") {
        return Ok(PathBuf::from(qmake));
    }

    for variable in ["QT_DIR", "QT_ROOT_DIR"] {
        if let Some(root) = env::var_os(variable) {
            return qmake_in_root(variable, root);
        }
    }

    for name in QMAKE_NAMES {
        let candidate = PathBuf::from(name);
        if QtInstallationQMake::try_from(candidate.clone()).is_ok() {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "could not find Qt 6 qmake; set QMAKE, QT_DIR, or QT_ROOT_DIR",
    )
    .into())
}

fn qmake_in_root(variable: &str, root: OsString) -> Result<PathBuf, Box<dyn Error>> {
    let root = PathBuf::from(root);
    for name in QMAKE_NAMES {
        let candidate = root.join("bin").join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{variable} points to {}, but its bin directory contains no qmake executable",
            root.display()
        ),
    )
    .into())
}
