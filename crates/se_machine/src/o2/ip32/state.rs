//! Versioned IP32 machine-state data.

use core::fmt;

use se_device::bus::i2c::I2cBusState;
use se_device::bus::irq::IrqBusState;
use se_device::bus::isa::IsaBusState;
use se_device::bus::media::MediaBusState;
use se_device::bus::one_wire::OneWireBusState;
use se_device::bus::pci::{PciBusState, PciConfigurationEndpointState};
use se_device::bus::two_wire::TwoWireBusState;
use se_device::chipset::crime::CrimeState;
use se_device::chipset::crime::iou::{CrimeCgiBusState, CrimeCmiBusState};
use se_device::chipset::crime::memory::CrimeSdramState;
use se_device::chipset::gbe::GbeState;
use se_device::chipset::gbe::protocol::GbeFrame;
use se_device::chipset::mace::MaceState;
use se_device::chipset::mace::config::MacePortConfig;
use se_device::cpu::execution::protocol::ExecutionTransaction;
use se_device::cpu::mips4::execution::bus::Mips4ExecutionTransaction;
use se_device::cpu::mips4::model::r5000::cpu::R5000CpuState;
use se_device::input::ps2::{Ps2KeyboardState, Ps2MouseState};
use se_device::memory::ds2502::Ds2502State;
use se_device::memory::flash::{SystemFlashState, SystemFlashStateError};
use se_device::parallel::ieee1284::Ieee1284State;
use se_device::rtc::ds1687::state::Ds1687State;
use se_device::serial::uart16550::Uart16550State;
use se_runtime::runtime::state::RuntimeState;

use super::bus::{Ip32StubEndpointState, Ip32SysAdBusState};
use super::config::Ip32PersistentConfig;
use super::event::{Ip32Event, Ip32HostOutput};

/// Current IP32 serialized-state schema.
pub const IP32_STATE_SCHEMA_VERSION: u32 = 2;

/// Machine control state that is not owned by a component.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct RuntimeControlState {
    pub(super) cpu_generation: u64,
    pub(super) cpu_clock_remainder: u64,
    pub(super) host_generation: u64,
    pub(super) host_capacities: MacePortConfig,
    pub(super) host_reservations: [usize; 12],
    pub(super) host_outputs: Vec<Ip32HostOutput>,
    pub(super) host_output_units: [usize; 12],
    pub(super) host_dropped_output_bytes: [u64; 12],
    pub(super) latest_display_frame: Option<GbeFrame>,
    pub(super) dropped_display_frames: u64,
    #[serde(default)]
    pub(super) display_frame_awaiting_take: bool,
    #[serde(default)]
    pub(super) skipped_display_frames: u64,
    pub(super) sysad_transactions: u64,
    pub(super) memory_transactions: u64,
    pub(super) cmi_transactions: u64,
    pub(super) cgi_transactions: u64,
    pub(super) pending_sysad: Option<ExecutionTransaction<Mips4ExecutionTransaction>>,
    pub(super) cpu_continuation_quantum: usize,
}

/// Complete deterministic IP32 machine state at one outer event boundary.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct Ip32MachineState {
    pub(super) schema_version: u32,
    pub(super) config: Ip32PersistentConfig,
    pub(super) runtime: RuntimeState<Ip32Event>,
    pub(super) control: RuntimeControlState,
    pub(super) cpu: R5000CpuState,
    pub(super) sysad: Ip32SysAdBusState,
    pub(super) cpu_irq: IrqBusState,
    pub(super) mace_irq: IrqBusState,
    pub(super) one_wire: OneWireBusState,
    pub(super) gbe_ddc: [TwoWireBusState; 2],
    pub(super) ps2_buses: [TwoWireBusState; 2],
    pub(super) crime: CrimeState,
    pub(super) cmi: CrimeCmiBusState,
    pub(super) cgi: CrimeCgiBusState,
    pub(super) sdram: CrimeSdramState,
    pub(super) isa: IsaBusState,
    pub(super) pci: PciBusState,
    pub(super) i2c: [I2cBusState; 2],
    pub(super) media: MediaBusState,
    pub(super) mace: MaceState,
    pub(super) keyboard: Ps2KeyboardState,
    pub(super) mouse: Ps2MouseState,
    pub(super) gbe: GbeState,
    pub(super) vice: Ip32StubEndpointState,
    pub(super) system_flash: SystemFlashState,
    pub(super) rtc: Ds1687State,
    pub(super) nic_identity: Ds2502State,
    pub(super) serial: [Uart16550State; 2],
    pub(super) scsi: PciConfigurationEndpointState,
    pub(super) parallel: Ieee1284State,
}

impl Ip32MachineState {
    /// Returns the schema version stored in this state.
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns the construction settings recorded with the state.
    pub const fn config(&self) -> &Ip32PersistentConfig {
        &self.config
    }
}

/// Failure while capturing or restoring IP32 state.
#[derive(Debug)]
pub enum Ip32StateError {
    /// The state uses an unsupported schema.
    UnsupportedSchema {
        /// Schema version stored in the state.
        version: u32,
    },
    /// The supplied construction settings do not match the saved hardware.
    ConfigurationMismatch,
    /// The rebuilt registry does not match the fixed IP32 topology.
    Registry(se_runtime::registry::RegistryLookupError),
    /// Scheduler state is internally inconsistent.
    Scheduler(se_core::scheduler::state::SchedulerStateError),
    /// RTC state is internally inconsistent.
    Rtc(se_device::rtc::ds1687::state::Ds1687StateError),
    /// System Flash state is internally inconsistent.
    SystemFlash(SystemFlashStateError),
    /// A component state belongs to a different fixed topology identity.
    Component(se_core::component::ComponentStateError),
    /// The machine could not be constructed from saved configuration.
    Build(super::machine::Ip32RuntimeBuildError),
}

impl fmt::Display for Ip32StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { version } => {
                write!(formatter, "unsupported IP32 state schema {version}")
            }
            Self::ConfigurationMismatch => {
                formatter.write_str("IP32 state configuration does not match the requested machine")
            }
            Self::Registry(error) => error.fmt(formatter),
            Self::Scheduler(error) => error.fmt(formatter),
            Self::Rtc(error) => error.fmt(formatter),
            Self::SystemFlash(error) => error.fmt(formatter),
            Self::Component(error) => error.fmt(formatter),
            Self::Build(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Ip32StateError {}
