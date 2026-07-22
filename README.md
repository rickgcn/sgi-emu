# sgi-emu

`sgi-emu` is an emulator for selected Silicon Graphics systems, written primarily in Rust.

The project uses deterministic event-driven machine and device models, Berkeley SoftFloat for guest floating-point behavior, an optional Cranelift-based MIPS IV JIT, and a native Qt Widgets frontend.

> [!WARNING]
> The emulator is under active development. Hardware coverage, public APIs, and saved-state compatibility may change.

## Current Scope

The repository currently contains:

- SGI O2 IP32 machine profiles.
- MIPS IV processor infrastructure.
- R5000, CRIME, MACE, GBE, memory, storage, serial, input, and bus models.
- Berkeley SoftFloat integration specialized for MIPS IV behavior.
- An optional Cranelift-based MIPS IV JIT backend.
- A Qt Widgets desktop frontend for the O2 application path.

The implementation is incomplete and should not yet be considered a drop-in replacement for a mature general-purpose emulator.

## Repository Layout

| Crate | Purpose |
| --- | --- |
| `se_core` | Components, scheduling, roles, and structured tracing |
| `se_runtime` | Event-driven runtime orchestration |
| `se_float` | Native and Berkeley SoftFloat floating-point backends |
| `se_device` | Processor, chipset, peripheral, memory, and bus models |
| `se_jit` | Cranelift-based MIPS IV JIT backend |
| `se_machine` | SGI machine profiles and board-level integration |
| `se_ui` | Native Qt Widgets frontend and application integration |

## Requirements

### Core

The non-UI workspace requires:

- Git
- Rustup
- A C compiler and build toolchain
- The Berkeley SoftFloat Git submodule

The repository pins Rust 1.95.0 through `rust-toolchain.toml`. Rustup will select and install the required toolchain automatically.

Initialize the submodule after cloning:

```sh
git submodule update --init --recursive
```

### Qt Frontend

Building `se_ui` additionally requires:

- A C++17 compiler
- Qt 6 development files for Core, Gui, Svg, and Widgets
- `qmake6`
- Qt translation tools providing `lrelease`

If Qt cannot be detected automatically, set `QMAKE` explicitly:

```sh
QMAKE=/path/to/qmake6 cargo build -p se_ui --release
```

Linux is the current primary development environment. The core crates are intended to remain portable, while frontend support on other platforms is not yet continuously verified.

## Building the Core

The default workspace members exclude `se_ui`, so the following commands do not require Qt:

```sh
cargo build
cargo test
```

To validate all non-UI workspace crates explicitly:

```sh
cargo test --workspace --exclude se_ui
```

A command using `--workspace` without excluding `se_ui` will also build the Qt frontend.

## Running the Application

Build and run the Qt frontend in release mode:

```sh
cargo run -p se_ui --release
```

A release build without launching the application can be produced with:

```sh
cargo build -p se_ui --release
```

Running an SGI O2 guest requires a compatible 512 KiB IP32 System PROM image.

SGI PROM images are proprietary and are not included in this repository. Users must obtain any required firmware legally from hardware or other authorized sources.

## Features

`se_machine` provides an optional `jit` feature:

```sh
cargo test -p se_machine --no-default-features
cargo test -p se_machine --all-features
```

The interpreter remains available without the feature. Enabling `jit` adds the Cranelift-based native execution backend.

The Qt frontend currently enables the JIT feature explicitly.

## Development Checks

Before submitting changes, run:

```sh
cargo fmt --all --check

cargo clippy \
  --workspace \
  --exclude se_ui \
  --all-targets \
  --all-features \
  -- -D warnings

cargo test --workspace --exclude se_ui

cargo test -p se_machine --no-default-features
cargo test -p se_machine --all-features

RUSTDOCFLAGS="-D warnings" \
  cargo doc --workspace --exclude se_ui --no-deps
```

When Qt is available, also run:

```sh
cargo build -p se_ui --release
cargo test -p se_ui
```

Tests requiring local proprietary PROM images are ignored by default.

## License

`sgi-emu` is licensed under the [GNU General Public License version 3 or later](LICENSE).

Third-party components retain their respective licenses.
