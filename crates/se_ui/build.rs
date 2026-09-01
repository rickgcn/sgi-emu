use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use qt_build_utils::{QtBuild, QtInstallationQMake};

const CPP_SOURCES: [&str; 9] = [
    "cpp/src/main_window.cpp",
    "cpp/src/display_widget.cpp",
    "cpp/src/settings_dialog.cpp",
    "cpp/src/debugger/registers_dock.cpp",
    "cpp/src/debugger/tlb_dock.cpp",
    "cpp/src/debugger/cache_dock.cpp",
    "cpp/src/debugger/disassembly_dock.cpp",
    "cpp/src/debugger/memory_dock.cpp",
    "cpp/src/serial_console_dock.cpp",
];

const HEADERS: [&str; 9] = [
    "cpp/include/se_ui/main_window.h",
    "cpp/include/se_ui/display_widget.h",
    "cpp/include/se_ui/settings_dialog.h",
    "cpp/include/se_ui/debugger/registers_dock.h",
    "cpp/include/se_ui/debugger/tlb_dock.h",
    "cpp/include/se_ui/debugger/cache_dock.h",
    "cpp/include/se_ui/debugger/disassembly_dock.h",
    "cpp/include/se_ui/debugger/memory_dock.h",
    "cpp/include/se_ui/serial_console_dock.h",
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
    for path in CPP_SOURCES.iter().chain(HEADERS.iter()) {
        println!("cargo::rerun-if-changed={path}");
    }

    let installation = QtInstallationQMake::try_from(find_qmake()?)?;
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
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flags(["/Zc:__cplusplus", "/permissive-"]);
    }
    qt.cargo_link_libraries(&mut build);
    build.compile("se_ui_qt");

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
