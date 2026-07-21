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
use se_device::chipset::crime::config::{CrimeConfig, CrimeConfigError};
use se_device::chipset::crime::iou::{CrimeCgiBusState, CrimeCmiBusState};
use se_device::chipset::crime::memory::CrimeSdramState;
use se_device::chipset::crime::memory::bus::CrimeMemoryBusState;
use se_device::chipset::gbe::GbeState;
use se_device::chipset::gbe::protocol::GbeFrame;
use se_device::chipset::mace::MaceState;
use se_device::chipset::mace::config::{MaceConfig, MacePortConfig};
use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use se_device::cpu::mips4::model::r5000::cpu::R5000CpuState;
use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
use se_device::input::ps2::{Ps2KeyboardState, Ps2MouseState};
use se_device::memory::ds2502::{Ds2502Config, Ds2502State};
use se_device::memory::flash::{SystemFlashState, SystemFlashStateError};
use se_device::parallel::ieee1284::Ieee1284State;
use se_device::rtc::ds1687::state::Ds1687State;
use se_device::serial::uart16550::Uart16550State;
use se_runtime::runtime::state::RuntimeState;

use super::bus::{Ip32StubEndpointState, Ip32SysAdBusState};
use super::event::{Ip32Event, Ip32HostOutput};
use super::timing::IP32_TIMEBASE_HZ;

/// Current IP32 serialized-state schema.
pub const IP32_STATE_SCHEMA_VERSION: u32 = 1;

/// Construction settings that do not contain PROM or battery-backed bytes.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32PersistentConfig {
    pub(super) processor: R5000Profile,
    pub(super) boot_mode: R5000BootMode,
    pub(super) crime: CrimeConfig,
    pub(super) mace: MaceConfig,
    pub(super) nic_identity: Ds2502Config,
}

impl Ip32PersistentConfig {
    pub(super) fn from_machine_config(config: &super::machine::Ip32MachineConfig) -> Self {
        Self {
            processor: config.processor,
            boot_mode: config.boot_mode,
            crime: config.crime,
            mace: config.mace,
            nic_identity: config.nic_identity.clone(),
        }
    }

    /// Returns the configured processor profile.
    pub const fn processor(&self) -> R5000Profile {
        self.processor
    }

    /// Returns the sampled boot-mode stream.
    pub const fn boot_mode(&self) -> R5000BootMode {
        self.boot_mode
    }

    /// Returns CRIME construction settings.
    pub const fn crime(&self) -> CrimeConfig {
        self.crime
    }

    /// Returns MACE construction settings.
    pub const fn mace(&self) -> MaceConfig {
        self.mace
    }

    /// Returns board-identity settings.
    pub const fn nic_identity(&self) -> &Ds2502Config {
        &self.nic_identity
    }

    /// Validates construction settings without requiring a PROM or battery image.
    pub fn validate(&self) -> Result<(), Ip32PersistentConfigError> {
        let frequency_hz = self.processor.processor_frequency_hz;
        if !(1..=IP32_TIMEBASE_HZ).contains(&frequency_hz) {
            return Err(Ip32PersistentConfigError::InvalidProcessorFrequency { frequency_hz });
        }
        self.crime
            .validate()
            .map_err(Ip32PersistentConfigError::Crime)
    }

    /// Creates machine construction input by adding session-specific PROM and RTC data.
    pub fn machine_config(
        &self,
        prom_image: Vec<u8>,
        rtc_unix_seconds: i64,
        rtc_nvram: Vec<u8>,
    ) -> super::machine::Ip32MachineConfig {
        super::machine::Ip32MachineConfig {
            jit_enabled: false,
            processor: self.processor,
            boot_mode: self.boot_mode,
            crime: self.crime,
            mace: self.mace,
            rtc: se_device::rtc::ds1687::Ds1687Config {
                initial_unix_seconds: rtc_unix_seconds,
                nvram: rtc_nvram,
            },
            nic_identity: self.nic_identity.clone(),
            prom_image,
        }
    }
}

/// Invalid persisted IP32 construction settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip32PersistentConfigError {
    /// The processor frequency cannot be represented by the machine timebase.
    InvalidProcessorFrequency { frequency_hz: u64 },
    /// CRIME construction settings are invalid.
    Crime(CrimeConfigError),
}

impl fmt::Display for Ip32PersistentConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProcessorFrequency { frequency_hz } => write!(
                formatter,
                "invalid R5000 processor frequency {frequency_hz} Hz"
            ),
            Self::Crime(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Ip32PersistentConfigError {}

impl Default for Ip32PersistentConfig {
    fn default() -> Self {
        Self::from_machine_config(&super::machine::Ip32MachineConfig::default())
    }
}

/// Machine-private orchestration state that is not owned by a component.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct MachineControlState {
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
    pub(super) sysad_transactions: u64,
    pub(super) memory_transactions: u64,
    pub(super) cmi_transactions: u64,
    pub(super) cgi_transactions: u64,
    pub(super) cpu_continuation_quantum: usize,
    pub(super) inline_sysad_completion: bool,
    pub(super) fusion_sysad: bool,
    pub(super) fusion_memory: bool,
    pub(super) fusion_cmi: bool,
    pub(super) fusion_cgi: bool,
    pub(super) fusion_isa: bool,
    pub(super) fusion_budget: u8,
}

/// Exact IP32 state at one outer event boundary.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct Ip32MachineState {
    pub(super) schema_version: u32,
    pub(super) config: Ip32PersistentConfig,
    pub(super) runtime: RuntimeState<Ip32Event>,
    pub(super) control: MachineControlState,
    pub(super) cpu: R5000CpuState,
    pub(super) sysad: Ip32SysAdBusState,
    pub(super) cpu_irq: IrqBusState,
    pub(super) mace_irq: IrqBusState,
    pub(super) one_wire: OneWireBusState,
    pub(super) gbe_ddc: [TwoWireBusState; 2],
    pub(super) ps2_buses: [TwoWireBusState; 2],
    pub(super) crime: CrimeState,
    pub(super) memory_bus: CrimeMemoryBusState,
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
    UnsupportedSchema { version: u32 },
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
    Build(super::machine::Ip32MachineBuildError),
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

#[cfg(test)]
mod tests {
    use super::*;
    use se_core::component::ComponentId;
    use se_core::role::BusDeviceRole;
    use se_core::scheduler::SimTime;

    use crate::o2::ip32::bus::Ip32StubEndpoint;
    use crate::o2::ip32::component_ids;
    use crate::o2::ip32::machine::{Ip32Machine, Ip32MachineConfig};
    use se_device::bus::isa::{
        IsaCompletionPayload, IsaDeviceResponse, IsaTransaction, IsaTransactionId, IsaTransfer,
    };
    use se_device::memory::flash::{SystemFlash, SystemFlashPersistentState};
    use se_device::rtc::ds1687::state::Ds1687PersistentState;

    #[test]
    fn exact_machine_state_round_trips_and_continues_deterministically() {
        let config = Ip32MachineConfig::default();
        let mut reference = Ip32Machine::from_config(config.clone()).unwrap();
        reference.schedule_power_on().unwrap();
        reference.run_steps(128).unwrap();

        let encoded = postcard::to_stdvec(&reference.save_state().unwrap()).unwrap();
        let decoded: Ip32MachineState = postcard::from_bytes(&encoded).unwrap();
        let mut restored = Ip32Machine::from_state_with_trace_sink(
            config,
            decoded,
            se_core::tracing::NoopTraceSink,
        )
        .unwrap();

        reference.run_steps(256).unwrap();
        restored.run_steps(256).unwrap();
        let reference = postcard::to_stdvec(&reference.save_state().unwrap()).unwrap();
        let restored = postcard::to_stdvec(&restored.save_state().unwrap()).unwrap();
        assert_eq!(restored, reference);
    }

    #[test]
    fn machine_state_does_not_store_the_prom_image() {
        let mut config = Ip32MachineConfig::default();
        let marker: Vec<u8> = (0..64).map(|index| 0xa5 ^ index).collect();
        config.prom_image[..marker.len()].copy_from_slice(&marker);
        let machine = Ip32Machine::from_config(config).unwrap();
        let encoded = postcard::to_stdvec(&machine.save_state().unwrap()).unwrap();
        assert!(!encoded.windows(marker.len()).any(|window| window == marker));
    }

    #[test]
    fn machine_state_restores_programmed_system_flash_without_the_base_image() {
        let mut config = Ip32MachineConfig::default();
        config.prom_image.fill(0xff);
        config.rtc.nvram[0x20] = 0x3c;
        let expected_flash = programmed_flash_state(&config, 0x4000, 0x5a);
        let replacement_flash = programmed_flash_state(&config, 0x4000, 0xa5);
        let mut machine = Ip32Machine::from_config(config.clone()).unwrap();
        machine
            .restore_system_flash_persistent_state(&expected_flash)
            .unwrap();
        let expected_rtc = machine.rtc_persistent_state().unwrap();
        let state = machine.save_state().unwrap();

        machine
            .restore_system_flash_persistent_state(&replacement_flash)
            .unwrap();
        machine
            .restore_rtc_persistent_state(
                &Ds1687PersistentState::new(123, vec![0x77; 256], 9).unwrap(),
            )
            .unwrap();
        let restored =
            Ip32Machine::from_state_with_trace_sink(config, state, se_core::tracing::NoopTraceSink)
                .unwrap();
        assert_eq!(
            restored.system_flash_persistent_state().unwrap(),
            expected_flash
        );
        assert_eq!(restored.rtc_persistent_state().unwrap(), expected_rtc);
        assert_eq!(expected_flash.changes().len(), 1);
        assert_eq!(expected_flash.changes()[0].offset(), 0x4000);
        assert_eq!(expected_flash.changes()[0].bytes(), &[0x5a]);
    }

    #[test]
    fn restore_rejects_component_state_from_another_topology_identity() {
        let config = Ip32MachineConfig::default();
        let machine = Ip32Machine::from_config(config.clone()).unwrap();
        let mut state = machine.save_state().unwrap();
        state.vice = Ip32StubEndpoint::new(ComponentId::new(0xdead_beef), "foreign").save_state();

        assert!(matches!(
            Ip32Machine::from_state_with_trace_sink(config, state, se_core::tracing::NoopTraceSink,),
            Err(Ip32StateError::Component(
                se_core::component::ComponentStateError::ComponentIdMismatch { .. }
            ))
        ));
    }

    fn programmed_flash_state(
        config: &Ip32MachineConfig,
        address: u32,
        value: u8,
    ) -> SystemFlashPersistentState {
        let mut flash = SystemFlash::new(
            component_ids::PROM,
            "System Flash",
            config.prom_image.clone(),
        );
        let response = flash.accept(IsaTransaction {
            id: IsaTransactionId::new(1),
            time: SimTime::ZERO,
            controller: component_ids::MACE,
            target: component_ids::PROM,
            address,
            transfer: IsaTransfer::write([value].into(), [true].into()),
        });
        assert!(matches!(
            response,
            IsaDeviceResponse::Complete(se_device::bus::isa::IsaCompletion {
                result: Ok(IsaCompletionPayload::WriteComplete),
                ..
            })
        ));
        flash.persistent_state()
    }
}
