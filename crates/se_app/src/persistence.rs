//! Host persistence for emulated nonvolatile machine state.

use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::BaseDirs;
use se_machine::indigo::ip12::{Ip12NonvolatileState, Ip12NonvolatileStateParts};
use se_machine::machine::MachineNonvolatileState;
use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u32 = 1;
const IP12_STATE_FILE: &str = "indigo-ip12.toml";

/// A restored machine state and the host time elapsed since it was saved.
pub struct RestoredMachineState {
    /// Retained hardware state.
    pub state: MachineNonvolatileState,
    /// Nonnegative elapsed host time in milliseconds.
    pub offline_milliseconds: u64,
}

#[derive(Deserialize, Serialize)]
struct Ip12StateFile {
    format_version: u32,
    saved_at_unix_milliseconds: u64,
    nvram: NvramStateFile,
    rtc: RtcStateFile,
}

#[derive(Deserialize, Serialize)]
struct NvramStateFile {
    words: Vec<u16>,
}

#[derive(Deserialize, Serialize)]
struct RtcStateFile {
    registers: Vec<u8>,
    alternate_control_registers: Vec<u8>,
    prescaler_phase_attoseconds: u64,
    millisecond_within_hundredth: u8,
    oscillator_failed: bool,
    single_supply: bool,
    alarm_match_active: bool,
}

impl Ip12StateFile {
    fn from_state(state: &Ip12NonvolatileState, saved_at_unix_milliseconds: u64) -> Self {
        let parts = state.parts();
        Self {
            format_version: FORMAT_VERSION,
            saved_at_unix_milliseconds,
            nvram: NvramStateFile {
                words: parts.nvram_words.to_vec(),
            },
            rtc: RtcStateFile {
                registers: parts.rtc_registers.to_vec(),
                alternate_control_registers: parts.rtc_alternate_control_registers.to_vec(),
                prescaler_phase_attoseconds: parts.rtc_prescaler_phase_attoseconds,
                millisecond_within_hundredth: parts.rtc_millisecond_within_hundredth,
                oscillator_failed: parts.rtc_oscillator_failed,
                single_supply: parts.rtc_single_supply,
                alarm_match_active: parts.rtc_alarm_match_active,
            },
        }
    }

    fn into_state(self) -> Result<Ip12NonvolatileState, Box<dyn Error>> {
        if self.format_version != FORMAT_VERSION {
            return Err(invalid_data(format!(
                "unsupported Indigo IP12 state format version: {}",
                self.format_version
            ))
            .into());
        }

        let nvram_words: [u16; 64] = self.nvram.words.try_into().map_err(|words: Vec<u16>| {
            invalid_data(format!(
                "invalid Indigo IP12 NVRAM word count: expected 64, got {}",
                words.len()
            ))
        })?;
        let registers: [u8; 32] = self
            .rtc
            .registers
            .try_into()
            .map_err(|registers: Vec<u8>| {
                invalid_data(format!(
                    "invalid DP8573A register count: expected 32, got {}",
                    registers.len()
                ))
            })?;
        let alternate_control_registers: [u8; 4] = self
            .rtc
            .alternate_control_registers
            .try_into()
            .map_err(|registers: Vec<u8>| {
                invalid_data(format!(
                    "invalid DP8573A alternate register count: expected 4, got {}",
                    registers.len()
                ))
            })?;
        Ok(Ip12NonvolatileState::try_from_parts(
            Ip12NonvolatileStateParts {
                nvram_words,
                rtc_registers: registers,
                rtc_alternate_control_registers: alternate_control_registers,
                rtc_prescaler_phase_attoseconds: self.rtc.prescaler_phase_attoseconds,
                rtc_millisecond_within_hundredth: self.rtc.millisecond_within_hundredth,
                rtc_oscillator_failed: self.rtc.oscillator_failed,
                rtc_single_supply: self.rtc.single_supply,
                rtc_alarm_match_active: self.rtc.alarm_match_active,
            },
        )?)
    }
}

/// Loads retained state for one configured machine model.
///
/// # Errors
///
/// Returns an error when the host data directory is unavailable or an
/// existing state file cannot be read, parsed, or validated.
pub fn load(model: &str) -> Result<Option<RestoredMachineState>, Box<dyn Error>> {
    let path = state_path(model)?;
    load_at(&path, SystemTime::now())
}

/// Atomically saves the retained state returned by the runtime.
///
/// # Errors
///
/// Returns an error when the host data directory is unavailable or the state
/// cannot be serialized and written.
pub fn save(state: &MachineNonvolatileState) -> Result<(), Box<dyn Error>> {
    let path = match state {
        MachineNonvolatileState::IndigoIp12(_) => state_path("indigo-ip12")?,
    };
    save_at(&path, state, SystemTime::now())
}

fn state_path(model: &str) -> io::Result<PathBuf> {
    let file_name = match model {
        "indigo-ip12" => IP12_STATE_FILE,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported machine model: {model}"),
            ));
        }
    };
    BaseDirs::new()
        .map(|directories| directories.data_local_dir().join("sgi-emu").join(file_name))
        .ok_or_else(|| io::Error::other("the host local data directory is unavailable"))
}

fn load_at(path: &Path, now: SystemTime) -> Result<Option<RestoredMachineState>, Box<dyn Error>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let file: Ip12StateFile = toml::from_str(&contents)?;
    let saved_at = file.saved_at_unix_milliseconds;
    let state = file.into_state()?;
    let now = unix_milliseconds(now)?;
    Ok(Some(RestoredMachineState {
        state: MachineNonvolatileState::IndigoIp12(state),
        offline_milliseconds: now.saturating_sub(saved_at),
    }))
}

fn save_at(
    path: &Path,
    state: &MachineNonvolatileState,
    now: SystemTime,
) -> Result<(), Box<dyn Error>> {
    let saved_at = unix_milliseconds(now)?;
    let file = match state {
        MachineNonvolatileState::IndigoIp12(state) => Ip12StateFile::from_state(state, saved_at),
    };
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("machine state path has no parent directory"))?;
    fs::create_dir_all(parent)?;

    let temporary_path = path.with_extension("toml.tmp");
    let mut temporary_file = File::create(&temporary_path)?;
    temporary_file.write_all(toml::to_string_pretty(&file)?.as_bytes())?;
    temporary_file.sync_all()?;
    drop(temporary_file);

    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error.into());
    }
    Ok(())
}

fn unix_milliseconds(time: SystemTime) -> io::Result<u64> {
    let milliseconds = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| io::Error::other("host time is before the Unix epoch"))?
        .as_millis();
    u64::try_from(milliseconds).map_err(|_| io::Error::other("host time is out of range"))
}

fn invalid_data(reason: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    use se_machine::indigo::ip12::{Ip12NonvolatileState, Ip12NonvolatileStateParts};
    use se_machine::machine::MachineNonvolatileState;

    use super::{FORMAT_VERSION, load_at, save_at};

    fn sample_state() -> MachineNonvolatileState {
        let mut words = [u16::MAX; 64];
        words[3] = 0x1234;
        let mut registers = [0; 32];
        registers[6] = 0x42;
        MachineNonvolatileState::IndigoIp12(
            Ip12NonvolatileState::try_from_parts(Ip12NonvolatileStateParts {
                nvram_words: words,
                rtc_registers: registers,
                rtc_alternate_control_registers: [0x08, 0x11, 0x22, 0x33],
                rtc_prescaler_phase_attoseconds: 500,
                rtc_millisecond_within_hundredth: 7,
                rtc_oscillator_failed: false,
                rtc_single_supply: true,
                rtc_alarm_match_active: false,
            })
            .unwrap(),
        )
    }

    fn temporary_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sgi-emu-state-{name}-{}.toml", std::process::id()))
    }

    #[test]
    fn absent_state_file_uses_device_defaults() {
        let path = temporary_path("absent");
        let _ = fs::remove_file(&path);

        assert!(load_at(&path, UNIX_EPOCH).unwrap().is_none());
    }

    #[test]
    fn state_round_trip_preserves_values_and_elapsed_time() {
        let path = temporary_path("round-trip");
        let _ = fs::remove_file(&path);
        let state = sample_state();
        let saved_at = UNIX_EPOCH + Duration::from_millis(10_000);
        save_at(&path, &state, saved_at).unwrap();
        save_at(&path, &state, saved_at + Duration::from_millis(500)).unwrap();

        let restored = load_at(&path, saved_at + Duration::from_millis(2_500))
            .unwrap()
            .unwrap();

        assert_eq!(restored.state, state);
        assert_eq!(restored.offline_milliseconds, 2_000);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn future_saved_time_clamps_offline_elapsed_to_zero() {
        let path = temporary_path("future");
        let _ = fs::remove_file(&path);
        let state = sample_state();
        let saved_at = UNIX_EPOCH + Duration::from_millis(20_000);
        save_at(&path, &state, saved_at).unwrap();

        let restored = load_at(&path, saved_at - Duration::from_millis(1_000))
            .unwrap()
            .unwrap();

        assert_eq!(restored.offline_milliseconds, 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn unsupported_format_is_rejected_without_replacing_the_file() {
        let path = temporary_path("unsupported");
        let _ = fs::remove_file(&path);
        let contents = format!(
            "format_version = {}\nsaved_at_unix_milliseconds = 0\n\n[nvram]\nwords = []\n\n[rtc]\nregisters = []\nalternate_control_registers = []\nprescaler_phase_attoseconds = 0\nmillisecond_within_hundredth = 0\noscillator_failed = false\nsingle_supply = false\nalarm_match_active = false\n",
            FORMAT_VERSION + 1
        );
        fs::write(&path, &contents).unwrap();

        assert!(load_at(&path, UNIX_EPOCH).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        fs::remove_file(path).unwrap();
    }
}
