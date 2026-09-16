#!/usr/bin/env python3
"""Validate, package, and verify sgi-emu release artifacts."""

from __future__ import annotations

import argparse
import hashlib
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import zipfile
from pathlib import Path
from typing import NoReturn


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
SEMVER_PATTERN = re.compile(
    r"^(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)"
    r"(?:-((?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)
FORBIDDEN_SUFFIXES = {".dmg", ".img", ".iso", ".qcow", ".qcow2", ".vmdk"}


def fail(message: str) -> NoReturn:
    raise SystemExit(message)


def run(
    command: list[str | Path],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    capture: bool = False,
) -> subprocess.CompletedProcess[str]:
    rendered = [str(value) for value in command]
    print(f"+ {subprocess.list2cmdline(rendered)}", flush=True)
    return subprocess.run(
        rendered,
        cwd=cwd,
        env=env,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.STDOUT if capture else None,
    )


def workspace_version() -> str:
    with (REPOSITORY_ROOT / "Cargo.toml").open("rb") as manifest:
        data = tomllib.load(manifest)
    version = data["workspace"]["package"]["version"]
    if not isinstance(version, str) or SEMVER_PATTERN.fullmatch(version) is None:
        fail(f"workspace package version is not valid SemVer: {version!r}")
    return version


def write_github_output(name: str, value: str) -> None:
    output = os.environ.get("GITHUB_OUTPUT")
    if output is None:
        print(f"{name}={value}")
        return
    with Path(output).open("a", encoding="utf-8") as output_file:
        output_file.write(f"{name}={value}\n")


def metadata(args: argparse.Namespace) -> None:
    version = workspace_version()
    is_release = args.event_name == "push"
    if is_release:
        expected_tag = f"v{version}"
        if args.ref_name != expected_tag:
            fail(
                f"release tag {args.ref_name!r} does not match workspace version "
                f"{expected_tag!r}"
            )
        artifact_version = expected_tag
        tag = expected_tag
    else:
        if not re.fullmatch(r"[0-9a-fA-F]{40}", args.sha):
            fail(f"invalid Git commit SHA: {args.sha!r}")
        artifact_version = f"v{version}-g{args.sha[:8].lower()}"
        tag = ""

    prerelease = "-" in version
    write_github_output("version", version)
    write_github_output("artifact-version", artifact_version)
    write_github_output("is-release", str(is_release).lower())
    write_github_output("is-prerelease", str(prerelease).lower())
    write_github_output("tag", tag)


def require_file(path: Path) -> Path:
    if not path.is_file():
        fail(f"required file does not exist: {path}")
    return path


def require_directory(path: Path) -> Path:
    if not path.is_dir():
        fail(f"required directory does not exist: {path}")
    return path


def new_directory(path: Path) -> Path:
    if path.exists():
        fail(f"refusing to reuse release directory: {path}")
    path.mkdir(parents=True)
    return path


def copy_common_files(package_root: Path, qt_root: Path) -> None:
    shutil.copy2(require_file(REPOSITORY_ROOT / "README.md"), package_root / "README.md")
    shutil.copy2(require_file(REPOSITORY_ROOT / "LICENSE"), package_root / "LICENSE")
    shutil.copy2(
        require_file(REPOSITORY_ROOT / ".github/release/THIRD-PARTY-NOTICES.md"),
        package_root / "THIRD-PARTY-NOTICES.md",
    )

    licenses = package_root / "licenses"
    licenses.mkdir()
    license_files = {
        REPOSITORY_ROOT / "3rdparty/libslirp/LICENSE": "libslirp-LICENSE",
        REPOSITORY_ROOT / "3rdparty/softfloat3/COPYING.txt": "SoftFloat-COPYING.txt",
        REPOSITORY_ROOT / "3rdparty/glib/LICENSES/LGPL-2.1-or-later.txt": (
            "GLib-LGPL-2.1-or-later.txt"
        ),
        REPOSITORY_ROOT / ".github/release/licenses/Qt-LGPL-3.0-only.txt": (
            "Qt-LGPL-3.0-only.txt"
        ),
    }
    for source, destination in license_files.items():
        shutil.copy2(require_file(source), licenses / destination)

    qt_sbom_source = require_directory(qt_root / "sbom")
    qt_sbom_destination = licenses / "Qt-SBOM"
    qt_sbom_destination.mkdir()
    qt_sbom_files = [
        path
        for path in sorted(qt_sbom_source.glob("*.spdx"))
        if not path.name.endswith(".source.spdx")
    ]
    if not qt_sbom_files:
        fail(f"Qt installation contains no binary SPDX documents: {qt_sbom_source}")
    for source in qt_sbom_files:
        shutil.copy2(source, qt_sbom_destination / source.name)


def validate_package_tree(package_root: Path) -> None:
    required = [
        package_root / "README.md",
        package_root / "LICENSE",
        package_root / "THIRD-PARTY-NOTICES.md",
        package_root / "licenses/libslirp-LICENSE",
        package_root / "licenses/SoftFloat-COPYING.txt",
        package_root / "licenses/GLib-LGPL-2.1-or-later.txt",
        package_root / "licenses/Qt-LGPL-3.0-only.txt",
    ]
    for path in required:
        require_file(path)
    if not any((package_root / "licenses/Qt-SBOM").glob("*.spdx")):
        fail("release package contains no Qt SPDX documents")

    for path in package_root.rglob("*"):
        if path.name == ".git":
            fail(f"release package contains Git metadata: {path}")
        if path.is_file() and path.suffix.lower() in FORBIDDEN_SUFFIXES:
            fail(f"release package contains a forbidden media file: {path}")


def smoke_test(
    command: list[str | Path], *, cwd: Path, platform: str, qt_root: Path
) -> None:
    with tempfile.TemporaryDirectory(prefix="sgi-emu-smoke-") as temporary:
        isolated = Path(temporary)
        environment = os.environ.copy()
        for variable in (
            "DYLD_FRAMEWORK_PATH",
            "QML2_IMPORT_PATH",
            "QMAKE",
            "QT_DIR",
            "QT_PLUGIN_PATH",
            "QT_ROOT_DIR",
        ):
            environment.pop(variable, None)
        path_entries = [
            entry
            for entry in environment.get("PATH", "").split(os.pathsep)
            if entry and not is_relative_to(Path(entry), qt_root)
        ]
        environment["PATH"] = os.pathsep.join(path_entries)
        environment.pop("LD_LIBRARY_PATH", None)
        if platform == "windows":
            if environment.get("CI", "").lower() != "true":
                print("Skipping the Windows launch test outside an ephemeral CI runner.")
                return
        else:
            isolated_directories = {
                "HOME": isolated / "home",
                "XDG_CONFIG_HOME": isolated / "xdg-config",
                "XDG_DATA_HOME": isolated / "xdg-data",
            }
            for variable, directory in isolated_directories.items():
                directory.mkdir()
                environment[variable] = str(directory)
        actual_command = [str(value) for value in command]
        if platform == "linux":
            actual_command = ["xvfb-run", "-a", *actual_command]

        print(f"+ smoke test: {subprocess.list2cmdline(actual_command)}", flush=True)
        process = subprocess.Popen(
            actual_command,
            cwd=cwd,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        try:
            time.sleep(5)
            return_code = process.poll()
            if return_code is not None:
                output = process.stdout.read() if process.stdout is not None else ""
                fail(
                    f"packaged application exited during smoke test with code "
                    f"{return_code}:\n{output}"
                )
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)


def create_zip(package_root: Path, archive: Path) -> None:
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as output:
        for path in sorted(package_root.rglob("*")):
            if path.is_file():
                output.write(path, path.relative_to(package_root.parent))


def package_windows(
    binary: Path,
    package_root: Path,
    output_directory: Path,
    qt_root: Path,
) -> Path:
    executable = package_root / "sgi-emu.exe"
    shutil.copy2(require_file(binary), executable)
    deploy_tool = require_file(qt_root / "bin/windeployqt.exe")
    run(
        [
            deploy_tool,
            "--release",
            "--no-translations",
            "--dir",
            package_root,
            executable,
        ]
    )
    require_file(package_root / "platforms/qwindows.dll")
    copy_common_files(package_root, qt_root)
    validate_package_tree(package_root)
    smoke_test([executable], cwd=package_root, platform="windows", qt_root=qt_root)

    archive = output_directory / f"{package_root.name}.zip"
    create_zip(package_root, archive)
    return archive


def qmake_query(qmake: Path, variable: str) -> Path:
    result = run([qmake, "-query", variable], capture=True)
    value = result.stdout.strip()
    if not value:
        fail(f"qmake returned an empty value for {variable}")
    return Path(value)


def linux_dependencies(path: Path, environment: dict[str, str]) -> tuple[list[Path], str]:
    result = run(["ldd", path], env=environment, capture=True)
    dependencies: list[Path] = []
    for line in result.stdout.splitlines():
        match = re.search(r"=>\s+(/\S+)\s+\(", line)
        if match is None:
            match = re.match(r"\s*(/\S+)\s+\(", line)
        if match is not None:
            dependencies.append(Path(match.group(1)))
    return dependencies, result.stdout


def is_relative_to(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
    except ValueError:
        return False
    return True


def copy_linux_plugins(plugin_root: Path, destination: Path) -> list[Path]:
    plugin_patterns = {
        "platforms": ("libqxcb.so", "libqwayland*.so"),
        "wayland-decoration-client": ("*.so",),
        "wayland-graphics-integration-client": ("*.so",),
        "wayland-shell-integration": ("*.so",),
        "xcbglintegrations": ("*.so",),
    }
    copied: list[Path] = []
    for directory, patterns in plugin_patterns.items():
        source_directory = plugin_root / directory
        if not source_directory.is_dir():
            continue
        destination_directory = destination / directory
        for pattern in patterns:
            for source in sorted(source_directory.glob(pattern)):
                destination_directory.mkdir(parents=True, exist_ok=True)
                target = destination_directory / source.name
                if not target.exists():
                    shutil.copy2(source, target, follow_symlinks=True)
                    copied.append(target)

    require_file(destination / "platforms/libqxcb.so")
    wayland_plugins = list((destination / "platforms").glob("libqwayland*.so"))
    if not wayland_plugins:
        fail("Qt Wayland platform plugins were not installed")
    return copied


def collect_linux_qt_libraries(
    roots: list[Path],
    qt_library_root: Path,
    destination: Path,
) -> None:
    destination.mkdir()
    pending = list(roots)
    inspected: set[Path] = set()
    environment = os.environ.copy()
    environment["LD_LIBRARY_PATH"] = os.pathsep.join(
        [str(destination), str(qt_library_root), environment.get("LD_LIBRARY_PATH", "")]
    )

    while pending:
        current = pending.pop()
        resolved_current = current.resolve()
        if resolved_current in inspected:
            continue
        inspected.add(resolved_current)
        dependencies, _ = linux_dependencies(current, environment)
        for dependency in dependencies:
            if not is_relative_to(dependency, qt_library_root):
                continue
            target = destination / dependency.name
            if not target.exists():
                shutil.copy2(dependency, target, follow_symlinks=True)
                pending.append(target)


def validate_linux_dependencies(package_root: Path, qt_root: Path) -> None:
    library_root = package_root / "lib"
    environment = os.environ.copy()
    environment["LD_LIBRARY_PATH"] = str(library_root)
    binaries = [package_root / "bin/sgi-emu"]
    binaries.extend(path for path in (package_root / "plugins").rglob("*.so"))
    binaries.extend(path for path in library_root.glob("*.so*"))
    for binary in binaries:
        _, output = linux_dependencies(binary, environment)
        if "not found" in output:
            fail(f"packaged Linux dependency is missing for {binary}:\n{output}")
        if str(qt_root) in output:
            fail(f"packaged Linux file still resolves through the build Qt tree: {binary}")


def package_linux(
    binary: Path,
    package_root: Path,
    output_directory: Path,
    qt_root: Path,
) -> Path:
    qmake = require_file(qt_root / "bin/qmake")
    qt_library_root = require_directory(qmake_query(qmake, "QT_INSTALL_LIBS"))
    qt_plugin_root = require_directory(qmake_query(qmake, "QT_INSTALL_PLUGINS"))

    binary_directory = package_root / "bin"
    binary_directory.mkdir()
    executable = binary_directory / "sgi-emu"
    shutil.copy2(require_file(binary), executable)
    executable.chmod(0o755)

    plugin_files = copy_linux_plugins(qt_plugin_root, package_root / "plugins")
    collect_linux_qt_libraries(
        [executable, *plugin_files], qt_library_root, package_root / "lib"
    )

    (binary_directory / "qt.conf").write_text(
        "[Paths]\nPrefix = ..\nPlugins = plugins\n", encoding="utf-8", newline="\n"
    )
    launcher = package_root / "sgi-emu"
    launcher.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        'base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\n'
        'export LD_LIBRARY_PATH="$base/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"\n'
        'exec "$base/bin/sgi-emu" "$@"\n',
        encoding="utf-8",
        newline="\n",
    )
    launcher.chmod(0o755)

    copy_common_files(package_root, qt_root)
    validate_package_tree(package_root)
    validate_linux_dependencies(package_root, qt_root)
    smoke_test([launcher], cwd=package_root, platform="linux", qt_root=qt_root)

    archive = output_directory / f"{package_root.name}.tar.xz"
    with tarfile.open(archive, "w:xz", preset=9) as output:
        output.add(package_root, arcname=package_root.name, recursive=True)
    return archive


def package_macos(
    binary: Path,
    package_root: Path,
    output_directory: Path,
    qt_root: Path,
    app_version: str,
) -> Path:
    app = package_root / "sgi-emu.app"
    contents = app / "Contents"
    executable_directory = contents / "MacOS"
    executable_directory.mkdir(parents=True)
    executable = executable_directory / "sgi-emu"
    shutil.copy2(require_file(binary), executable)
    executable.chmod(0o755)

    bundle_version = app_version.split("-", maxsplit=1)[0].split("+", maxsplit=1)[0]
    info = {
        "CFBundleDisplayName": "sgi-emu",
        "CFBundleExecutable": "sgi-emu",
        "CFBundleIdentifier": "io.github.rickgcn.sgi-emu",
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": "sgi-emu",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": bundle_version,
        "CFBundleVersion": bundle_version,
        "NSHighResolutionCapable": True,
    }
    with (contents / "Info.plist").open("wb") as plist:
        plistlib.dump(info, plist, sort_keys=True)

    deploy_tool = require_file(qt_root / "bin/macdeployqt")
    run([deploy_tool, app, "-always-overwrite", "-codesign=-", "-verbose=2"])
    run(["codesign", "--verify", "--deep", "--strict", app])
    dependencies = run(["otool", "-L", executable], capture=True).stdout
    if str(qt_root) in dependencies:
        fail("packaged macOS application still references the build Qt tree")

    copy_common_files(package_root, qt_root)
    validate_package_tree(package_root)
    smoke_test([executable], cwd=package_root, platform="macos", qt_root=qt_root)

    archive = output_directory / f"{package_root.name}.zip"
    run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", package_root, archive])
    return archive


def package(args: argparse.Namespace) -> None:
    binary = Path(args.binary).resolve()
    qt_root = require_directory(Path(args.qt_root).resolve())
    work_directory = new_directory(Path(args.work_directory).resolve())
    output_directory = new_directory(Path(args.output_directory).resolve())
    package_name = f"sgi-emu-{args.artifact_version}-{args.target}"
    package_root = new_directory(work_directory / package_name)

    if args.platform == "windows":
        archive = package_windows(binary, package_root, output_directory, qt_root)
    elif args.platform == "linux":
        archive = package_linux(binary, package_root, output_directory, qt_root)
    elif args.platform == "macos":
        archive = package_macos(
            binary, package_root, output_directory, qt_root, args.app_version
        )
    else:
        fail(f"unsupported release platform: {args.platform}")

    require_file(archive)
    print(f"Created {archive}")
    write_github_output("archive", str(archive))


def expected_asset_names(artifact_version: str) -> set[str]:
    return {
        f"sgi-emu-{artifact_version}-x86_64-pc-windows-msvc.zip",
        f"sgi-emu-{artifact_version}-x86_64-unknown-linux-gnu.tar.xz",
        f"sgi-emu-{artifact_version}-aarch64-apple-darwin.zip",
    }


def assemble(args: argparse.Namespace) -> None:
    directory = require_directory(Path(args.directory).resolve())
    expected = expected_asset_names(args.artifact_version)
    actual = {path.name for path in directory.iterdir() if path.is_file()}
    if actual != expected:
        missing = sorted(expected - actual)
        unexpected = sorted(actual - expected)
        fail(f"release asset mismatch; missing={missing}, unexpected={unexpected}")

    checksums = []
    for name in sorted(expected):
        digest = hashlib.sha256()
        with (directory / name).open("rb") as asset:
            for chunk in iter(lambda: asset.read(1024 * 1024), b""):
                digest.update(chunk)
        checksums.append(f"{digest.hexdigest()}  {name}\n")
    (directory / "SHA256SUMS").write_text(
        "".join(checksums), encoding="utf-8", newline="\n"
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    metadata_parser = commands.add_parser("metadata")
    metadata_parser.add_argument("--event-name", required=True)
    metadata_parser.add_argument("--ref-name", required=True)
    metadata_parser.add_argument("--sha", required=True)
    metadata_parser.set_defaults(handler=metadata)

    package_parser = commands.add_parser("package")
    package_parser.add_argument(
        "--platform", choices=("windows", "linux", "macos"), required=True
    )
    package_parser.add_argument("--target", required=True)
    package_parser.add_argument("--binary", required=True)
    package_parser.add_argument("--qt-root", required=True)
    package_parser.add_argument("--app-version", required=True)
    package_parser.add_argument("--artifact-version", required=True)
    package_parser.add_argument("--work-directory", required=True)
    package_parser.add_argument("--output-directory", required=True)
    package_parser.set_defaults(handler=package)

    assemble_parser = commands.add_parser("assemble")
    assemble_parser.add_argument("--directory", required=True)
    assemble_parser.add_argument("--artifact-version", required=True)
    assemble_parser.set_defaults(handler=assemble)
    return parser


def main() -> None:
    args = build_parser().parse_args()
    args.handler(args)


if __name__ == "__main__":
    main()
