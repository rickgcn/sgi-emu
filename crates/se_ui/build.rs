use std::{
    env,
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    process::Command,
};

use qt_build_utils::{QResource, QResourceFile, QResources, QtBuild, QtPlatformLinker};

const TRANSLATION_SOURCE: &str = "translations/en_US.ts";
const ICON_SOURCES: [(&str, &str); 4] = [
    ("icons/run.svg", "run.svg"),
    ("icons/pause.svg", "pause.svg"),
    ("icons/hard-reset.svg", "hard-reset.svg"),
    ("icons/emulation-settings.svg", "emulation-settings.svg"),
];

fn main() {
    let resource_collection = build_translation_resource(Path::new(TRANSLATION_SOURCE));
    let qt_modules = ["Core", "Gui", "Svg", "Widgets"]
        .into_iter()
        .map(String::from)
        .collect();
    let qt = QtBuild::new(qt_modules).expect("Failed to find the Qt installation");
    let resource = qt.rcc().compile(resource_collection);

    let mut build = cxx_build::bridge("src/application.rs");
    build
        .file("src/application.cpp")
        .file(resource.file.expect("rcc must generate a C++ source file"))
        .std("c++17");

    for include_path in qt.include_paths() {
        build.include(include_path);
    }

    qt.cargo_link_libraries(&mut build);
    QtPlatformLinker::init();
    build.compile("se_ui");

    println!("cargo::rerun-if-changed=include/application.h");
    println!("cargo::rerun-if-changed=src/application.cpp");
}

fn build_translation_resource(source: &Path) -> PathBuf {
    println!("cargo::rerun-if-changed={}", source.display());
    println!("cargo::rerun-if-env-changed=QMAKE");

    let output_directory = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    let translation = output_directory.join("sgi-emu_en_US.qm");
    let lrelease = qt_host_binary("lrelease");
    let status = Command::new(&lrelease)
        .arg(source)
        .arg("-qm")
        .arg(&translation)
        .status()
        .unwrap_or_else(|error| panic!("Failed to run {}: {error}", lrelease.display()));

    assert!(status.success(), "Qt lrelease failed with {status}");

    let resource_collection = output_directory.join("se_ui_translations.qrc");
    let mut resource_file =
        File::create(&resource_collection).expect("Failed to create translation resources");
    let mut icons = QResource::new().prefix("/icons");
    for (source, alias) in ICON_SOURCES {
        println!("cargo::rerun-if-changed={source}");
        icons = icons.file(QResourceFile::new(source).alias(alias));
    }

    QResources::new()
        .resource(
            QResource::new()
                .prefix("/i18n")
                .file(QResourceFile::new(translation).alias("sgi-emu_en_US.qm")),
        )
        .resource(icons)
        .write(&mut resource_file)
        .expect("Failed to write translation resources");

    resource_collection
}

fn qt_host_binary(name: &str) -> PathBuf {
    let qmake = qmake_command();
    let output = Command::new(&qmake)
        .args(["-query", "QT_HOST_BINS"])
        .output()
        .unwrap_or_else(|error| panic!("Failed to run {:?}: {error}", qmake));

    assert!(output.status.success(), "qmake query failed");

    let host_bins = String::from_utf8(output.stdout).expect("QT_HOST_BINS must be UTF-8");
    let executable = format!("{name}{}", env::consts::EXE_SUFFIX);
    let path = PathBuf::from(host_bins.trim()).join(executable);

    assert!(path.is_file(), "Qt tool not found: {}", path.display());
    path
}

fn qmake_command() -> OsString {
    if let Some(qmake) = env::var_os("QMAKE") {
        return qmake;
    }

    for candidate in ["qmake6", "qmake"] {
        if Command::new(candidate)
            .args(["-query", "QT_VERSION"])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return OsString::from(candidate);
        }
    }

    panic!("Unable to find qmake6 or qmake")
}
