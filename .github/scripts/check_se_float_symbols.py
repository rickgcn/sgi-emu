#!/usr/bin/env python3
"""Validate the two se_float C archives against normalized symbol lists."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


SHIM_SYMBOLS = (
    "se_float_shim_add_f32",
    "se_float_shim_add_f64",
    "se_float_shim_compare_f32",
    "se_float_shim_compare_f64",
    "se_float_shim_div_f32",
    "se_float_shim_div_f64",
    "se_float_shim_f32_to_f64",
    "se_float_shim_f32_to_i32",
    "se_float_shim_f32_to_i64",
    "se_float_shim_f64_to_f32",
    "se_float_shim_f64_to_i32",
    "se_float_shim_f64_to_i64",
    "se_float_shim_i32_to_f32",
    "se_float_shim_i32_to_f64",
    "se_float_shim_i64_to_f32",
    "se_float_shim_i64_to_f64",
    "se_float_shim_mul_f32",
    "se_float_shim_mul_f64",
    "se_float_shim_sqrt_f32",
    "se_float_shim_sqrt_f64",
    "se_float_shim_sub_f32",
    "se_float_shim_sub_f64",
)

SOFTFLOAT_SYMBOLS = (
    "se_float_sf_addMagsF32",
    "se_float_sf_addMagsF64",
    "se_float_sf_approxRecip32_1",
    "se_float_sf_approxRecipSqrt32_1",
    "se_float_sf_approxRecipSqrt_1k0s",
    "se_float_sf_approxRecipSqrt_1k1s",
    "se_float_sf_approxRecip_1k0s",
    "se_float_sf_approxRecip_1k1s",
    "se_float_sf_countLeadingZeros32",
    "se_float_sf_countLeadingZeros64",
    "se_float_sf_countLeadingZeros8",
    "se_float_sf_detectTininess",
    "se_float_sf_exceptionFlags",
    "se_float_sf_extF80_roundingPrecision",
    "se_float_sf_f32_add",
    "se_float_sf_f32_div",
    "se_float_sf_f32_eq",
    "se_float_sf_f32_lt_quiet",
    "se_float_sf_f32_mul",
    "se_float_sf_f32_sqrt",
    "se_float_sf_f32_sub",
    "se_float_sf_f32_to_f64",
    "se_float_sf_f32_to_i32",
    "se_float_sf_f32_to_i64",
    "se_float_sf_f64_add",
    "se_float_sf_f64_div",
    "se_float_sf_f64_eq",
    "se_float_sf_f64_lt_quiet",
    "se_float_sf_f64_mul",
    "se_float_sf_f64_sqrt",
    "se_float_sf_f64_sub",
    "se_float_sf_f64_to_f32",
    "se_float_sf_f64_to_i32",
    "se_float_sf_f64_to_i64",
    "se_float_sf_i32_to_f32",
    "se_float_sf_i32_to_f64",
    "se_float_sf_i64_to_f32",
    "se_float_sf_i64_to_f64",
    "se_float_sf_mul64To128",
    "se_float_sf_normRoundPackToF32",
    "se_float_sf_normRoundPackToF64",
    "se_float_sf_normSubnormalF32Sig",
    "se_float_sf_normSubnormalF64Sig",
    "se_float_sf_propagateNaNF32UI",
    "se_float_sf_propagateNaNF64UI",
    "se_float_sf_raiseFlags",
    "se_float_sf_roundPackReset",
    "se_float_sf_roundPackToF32",
    "se_float_sf_roundPackToF32PrecisionInexact",
    "se_float_sf_roundPackToF32_impl",
    "se_float_sf_roundPackToF64",
    "se_float_sf_roundPackToF64PrecisionInexact",
    "se_float_sf_roundPackToF64_impl",
    "se_float_sf_roundToI32",
    "se_float_sf_roundToI64",
    "se_float_sf_roundingMode",
    "se_float_sf_shiftRightJam32",
    "se_float_sf_shiftRightJam64",
    "se_float_sf_shiftRightJam64Extra",
    "se_float_sf_shortShiftRightJam64",
    "se_float_sf_subMagsF32",
    "se_float_sf_subMagsF64",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cargo-messages", required=True, type=Path)
    parser.add_argument("--llvm-nm", required=True, type=Path)
    parser.add_argument("--target", required=True)
    return parser.parse_args()


def find_out_dir(messages_path: Path) -> Path:
    out_dirs: list[Path] = []
    with messages_path.open(encoding="utf-8") as messages:
        for line in messages:
            message = json.loads(line)
            if message.get("reason") != "build-script-executed":
                continue
            package_id = message.get("package_id", "")
            package_source = package_id.split("#", maxsplit=1)[0].rstrip("/")
            if "#se_float@" in package_id or package_source.endswith("/se_float"):
                linked_libs = message.get("linked_libs", [])
                expected_libs = [
                    "static=se_float_shim",
                    "static=se_float_softfloat",
                ]
                if linked_libs != expected_libs:
                    raise RuntimeError(
                        "se_float link metadata order differs: "
                        f"expected {expected_libs}, found {linked_libs}"
                    )
                out_dirs.append(Path(message["out_dir"]))
    if len(out_dirs) != 1:
        raise RuntimeError(
            f"expected one se_float build-script out_dir, found {len(out_dirs)}"
        )
    return out_dirs[0]


def archive_paths(out_dir: Path, target: str) -> tuple[Path, Path]:
    if target.endswith("-msvc"):
        expected = {
            "se_float_shim.lib",
            "se_float_softfloat.lib",
        }
        archives = {path.name for path in out_dir.glob("*.lib")}
        shim = out_dir / "se_float_shim.lib"
        softfloat = out_dir / "se_float_softfloat.lib"
    else:
        expected = {
            "libse_float_shim.a",
            "libse_float_softfloat.a",
        }
        archives = {path.name for path in out_dir.glob("*.a")}
        shim = out_dir / "libse_float_shim.a"
        softfloat = out_dir / "libse_float_softfloat.a"
    if archives != expected:
        raise RuntimeError(
            f"formal archive set differs for {target}: expected {sorted(expected)}, "
            f"found {sorted(archives)}"
        )
    return shim, softfloat


def exported_symbols(llvm_nm: Path, archive: Path, target: str) -> list[str]:
    result = subprocess.run(
        [str(llvm_nm), "--export-symbols", "--no-demangle", str(archive)],
        check=True,
        capture_output=True,
        text=True,
    )
    lines = result.stdout.splitlines()
    if any(not line for line in lines):
        raise RuntimeError(f"llvm-nm emitted an empty symbol record for {archive}")
    if target == "aarch64-apple-darwin":
        lines = [line[1:] if line.startswith("_") else line for line in lines]
    return sorted(set(lines))


def verify_archive(
    llvm_nm: Path,
    archive: Path,
    target: str,
    prefix: str,
    expected: tuple[str, ...],
) -> None:
    actual = exported_symbols(llvm_nm, archive, target)
    wrong_prefix = [symbol for symbol in actual if not symbol.startswith(prefix)]
    if wrong_prefix:
        raise RuntimeError(
            f"{archive} exports symbols outside {prefix}: {wrong_prefix}"
        )
    if actual != list(expected):
        missing = sorted(set(expected) - set(actual))
        extra = sorted(set(actual) - set(expected))
        raise RuntimeError(
            f"symbol allow-list mismatch for {archive} on {target}; "
            f"missing={missing}, extra={extra}"
        )


def main() -> int:
    args = parse_args()
    if not args.llvm_nm.is_file():
        raise RuntimeError(f"fixed-sysroot llvm-nm is missing: {args.llvm_nm}")
    out_dir = find_out_dir(args.cargo_messages)
    shim, softfloat = archive_paths(out_dir, args.target)
    verify_archive(
        args.llvm_nm,
        shim,
        args.target,
        "se_float_shim_",
        SHIM_SYMBOLS,
    )
    verify_archive(
        args.llvm_nm,
        softfloat,
        args.target,
        "se_float_sf_",
        SOFTFLOAT_SYMBOLS,
    )
    print(f"validated se_float archives for {args.target}: {out_dir}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, subprocess.CalledProcessError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
