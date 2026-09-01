//! Command-line and headless frontends.

pub mod headless;
pub mod terminal;

use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};

/// Parsed command-line configuration.
#[derive(Debug, Parser)]
#[command(name = "sgi-emu", version, about = "Silicon Graphics emulator")]
pub struct Arguments {
    /// Runs without creating the Qt graphical interface.
    #[arg(long)]
    headless: bool,

    /// Selects the emulated machine.
    #[arg(long, value_enum)]
    machine: Option<MachineChoice>,

    /// Uses the specified Indigo IP12 PROM image.
    #[arg(long)]
    prom: Option<PathBuf>,

    /// Selects the floating-point backend.
    #[arg(long, value_enum)]
    float_backend: Option<FloatBackendChoice>,
}

impl Arguments {
    /// Parses the process command line and handles help or version output.
    #[must_use]
    pub fn parse_process() -> Self {
        Self::parse()
    }

    /// Reports whether the headless frontend was requested.
    #[must_use]
    pub const fn headless(&self) -> bool {
        self.headless
    }

    /// Returns the selected machine identifier, if overridden.
    #[must_use]
    pub fn machine(&self) -> Option<&'static str> {
        self.machine.map(MachineChoice::identifier)
    }

    /// Returns the selected PROM path, if overridden.
    #[must_use]
    pub fn prom(&self) -> Option<&Path> {
        self.prom.as_deref()
    }

    /// Returns the selected floating-point backend identifier, if overridden.
    #[must_use]
    pub fn float_backend(&self) -> Option<&'static str> {
        self.float_backend.map(FloatBackendChoice::identifier)
    }
}

/// Machines accepted by the command-line frontend.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum MachineChoice {
    /// SGI Indigo IP12.
    IndigoIp12,
}

impl MachineChoice {
    const fn identifier(self) -> &'static str {
        match self {
            Self::IndigoIp12 => "indigo-ip12",
        }
    }
}

/// Floating-point backends accepted by the command-line frontend.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum FloatBackendChoice {
    /// Berkeley SoftFloat.
    Softfloat,
    /// Host-native floating point.
    Native,
}

impl FloatBackendChoice {
    const fn identifier(self) -> &'static str {
        match self {
            Self::Softfloat => "softfloat",
            Self::Native => "native",
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Arguments;

    #[test]
    fn no_arguments_select_the_graphical_frontend_without_overrides() {
        let arguments = Arguments::try_parse_from(["sgi-emu"]).unwrap();

        assert!(!arguments.headless());
        assert_eq!(arguments.machine(), None);
        assert_eq!(arguments.prom(), None);
        assert_eq!(arguments.float_backend(), None);
    }

    #[test]
    fn fixed_command_line_surface_accepts_all_headless_overrides() {
        let arguments = Arguments::try_parse_from([
            "sgi-emu",
            "--headless",
            "--machine",
            "indigo-ip12",
            "--prom",
            "prom.bin",
            "--float-backend",
            "native",
        ])
        .unwrap();

        assert!(arguments.headless());
        assert_eq!(arguments.machine(), Some("indigo-ip12"));
        assert_eq!(arguments.prom().unwrap().to_string_lossy(), "prom.bin");
        assert_eq!(arguments.float_backend(), Some("native"));
    }
}
