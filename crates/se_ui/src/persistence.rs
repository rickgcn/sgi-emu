//! Versioned application, battery, and machine-state persistence.

use std::{
    fmt,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use directories::ProjectDirs;
use se_device::rtc::ds1687::state::Ds1687PersistentState;
use se_machine::o2::ip32::state::{
    IP32_STATE_SCHEMA_VERSION, Ip32MachineState, Ip32PersistentConfig, Ip32PersistentConfigError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

pub(crate) const MACHINE_ID: &str = "sgi-o2-ip32";
const EMULATION_CONFIG_VERSION: u32 = 1;
const BATTERY_VERSION: u32 = 1;
const STATE_CONTAINER_VERSION: u32 = 1;
const STATE_MAGIC: [u8; 8] = *b"SESTATE\0";
const BATTERY_MAGIC: [u8; 8] = *b"SERTCNV\0";
const HEADER_SIZE: usize = 8 + 4 + 8 + 8 + 8 + 32;
const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_COMPRESSED_STATE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_UNCOMPRESSED_STATE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const STATE_NON_MEMORY_ALLOWANCE_BYTES: u64 = 256 * 1024 * 1024;
const ZSTD_LEVEL: i32 = 3;
const DEFAULT_RTC_UNIX_SECONDS: i64 = 946_684_800;

/// Host-time policy applied when opening a normal machine session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RtcPersistenceMode {
    #[default]
    RealTime,
    Frozen,
    SynchronizeWithHost,
}

impl RtcPersistenceMode {
    pub(crate) const fn as_u8(self) -> u8 {
        match self {
            Self::RealTime => 0,
            Self::Frozen => 1,
            Self::SynchronizeWithHost => 2,
        }
    }

    pub(crate) const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::RealTime),
            1 => Some(Self::Frozen),
            2 => Some(Self::SynchronizeWithHost),
            _ => None,
        }
    }
}

/// Versioned emulator configuration stored outside the Qt UI settings.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct EmulationConfig {
    version: u32,
    machine_id: String,
    prom_path: PathBuf,
    prom_sha256: String,
    rtc_mode: RtcPersistenceMode,
    machine: Ip32PersistentConfig,
}

impl EmulationConfig {
    pub(crate) fn new(
        prom_path: PathBuf,
        prom_hash: [u8; 32],
        rtc_mode: RtcPersistenceMode,
        machine: Ip32PersistentConfig,
    ) -> Result<Self, PersistenceError> {
        if !prom_path.is_absolute() {
            return Err(PersistenceError::PromPathNotAbsolute(prom_path));
        }
        Ok(Self {
            version: EMULATION_CONFIG_VERSION,
            machine_id: MACHINE_ID.to_owned(),
            prom_path,
            prom_sha256: encode_hash(prom_hash),
            rtc_mode,
            machine,
        })
    }

    pub(crate) fn validate(&self) -> Result<(), PersistenceError> {
        if self.version != EMULATION_CONFIG_VERSION {
            return Err(PersistenceError::UnsupportedEmulationConfig {
                version: self.version,
            });
        }
        if self.machine_id != MACHINE_ID {
            return Err(PersistenceError::WrongMachine {
                machine_id: self.machine_id.clone(),
            });
        }
        if !self.prom_path.is_absolute() {
            return Err(PersistenceError::PromPathNotAbsolute(
                self.prom_path.clone(),
            ));
        }
        decode_hash(&self.prom_sha256)?;
        self.machine
            .validate()
            .map_err(PersistenceError::InvalidMachineConfiguration)?;
        Ok(())
    }

    pub(crate) fn prom_path(&self) -> &Path {
        &self.prom_path
    }

    pub(crate) fn prom_hash(&self) -> Result<[u8; 32], PersistenceError> {
        decode_hash(&self.prom_sha256)
    }

    pub(crate) const fn rtc_mode(&self) -> RtcPersistenceMode {
        self.rtc_mode
    }

    pub(crate) const fn machine(&self) -> &Ip32PersistentConfig {
        &self.machine
    }

    pub(crate) fn with_prom_path(mut self, path: PathBuf) -> Result<Self, PersistenceError> {
        if !path.is_absolute() {
            return Err(PersistenceError::PromPathNotAbsolute(path));
        }
        self.prom_path = path;
        Ok(self)
    }
}

/// Platform-native locations for application-owned persistence.
#[derive(Clone, Debug)]
pub(crate) struct PersistencePaths {
    config_file: PathBuf,
    battery_file: PathBuf,
}

impl PersistencePaths {
    pub(crate) fn discover() -> Result<Self, PersistenceError> {
        let project = ProjectDirs::from("cn", "rickgcn", "sgi-emu")
            .ok_or(PersistenceError::ApplicationDirectoriesUnavailable)?;
        Ok(Self {
            config_file: project.config_dir().join("emulation.toml"),
            battery_file: project.data_dir().join("machines/ip32/rtc_nvram.bin"),
        })
    }

    pub(crate) fn config_file(&self) -> &Path {
        &self.config_file
    }

    pub(crate) fn battery_file(&self) -> &Path {
        &self.battery_file
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StateMetadata {
    machine_id: String,
    state_schema_version: u32,
    application_version: String,
    emulation_config: EmulationConfig,
}

pub(crate) struct LoadedStateFile {
    pub(crate) metadata_config: EmulationConfig,
    pub(crate) state: Ip32MachineState,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BatteryPayload {
    version: u32,
    machine_id: String,
    host_utc_anchor: i64,
    state: Ds1687PersistentState,
}

pub(crate) struct BatteryLoad {
    pub(crate) state: Ds1687PersistentState,
    pub(crate) warning: Option<String>,
}

pub(crate) fn host_utc_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

pub(crate) fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(crate) fn load_emulation_config(
    paths: &PersistencePaths,
) -> Result<Option<EmulationConfig>, PersistenceError> {
    let contents = match fs::read_to_string(paths.config_file()) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PersistenceError::Io(error)),
    };
    let parsed = toml::from_str::<EmulationConfig>(&contents)
        .map_err(PersistenceError::TomlRead)
        .and_then(|config| {
            config.validate()?;
            Ok(config)
        });
    match parsed {
        Ok(config) => Ok(Some(config)),
        Err(error) => {
            let _ = preserve_corrupt_file(paths.config_file(), host_utc_seconds());
            Err(error)
        }
    }
}

pub(crate) fn save_emulation_config(
    paths: &PersistencePaths,
    config: &EmulationConfig,
) -> Result<(), PersistenceError> {
    config.validate()?;
    let contents = toml::to_string_pretty(config).map_err(PersistenceError::TomlWrite)?;
    atomic_write(paths.config_file(), contents.as_bytes())
}

pub(crate) fn load_prom(config: &EmulationConfig) -> Result<Vec<u8>, PersistenceError> {
    let bytes = fs::read(config.prom_path()).map_err(PersistenceError::Io)?;
    if hash_bytes(&bytes) != config.prom_hash()? {
        return Err(PersistenceError::PromHashMismatch);
    }
    Ok(bytes)
}

pub(crate) fn load_battery(
    paths: &PersistencePaths,
    mode: RtcPersistenceMode,
    host_now: i64,
) -> BatteryLoad {
    match read_battery(paths.battery_file()) {
        Ok(Some(payload)) => BatteryLoad {
            state: apply_rtc_mode(payload.state, payload.host_utc_anchor, mode, host_now),
            warning: None,
        },
        Ok(None) => BatteryLoad {
            state: default_battery(mode, host_now),
            warning: None,
        },
        Err(error) => {
            let backup = preserve_corrupt_file(paths.battery_file(), host_now);
            let suffix = backup
                .map(|path| format!("; preserved as {}", path.display()))
                .unwrap_or_default();
            BatteryLoad {
                state: default_battery(mode, host_now),
                warning: Some(format!(
                    "failed to load RTC/NVRAM battery file: {error}{suffix}"
                )),
            }
        }
    }
}

pub(crate) fn save_battery(
    paths: &PersistencePaths,
    state: Ds1687PersistentState,
    host_now: i64,
) -> Result<(), PersistenceError> {
    let payload = BatteryPayload {
        version: BATTERY_VERSION,
        machine_id: MACHINE_ID.to_owned(),
        host_utc_anchor: host_now,
        state,
    };
    let serialized = postcard::to_stdvec(&payload).map_err(PersistenceError::PostcardWrite)?;
    let mut file = Vec::with_capacity(8 + 8 + serialized.len() + 32);
    file.extend_from_slice(&BATTERY_MAGIC);
    file.extend_from_slice(&(serialized.len() as u64).to_le_bytes());
    file.extend_from_slice(&serialized);
    file.extend_from_slice(&hash_bytes(&serialized));
    atomic_write(paths.battery_file(), &file)
}

pub(crate) fn write_state_file(
    path: &Path,
    application_version: &str,
    config: &EmulationConfig,
    state: &Ip32MachineState,
) -> Result<(), PersistenceError> {
    config.validate()?;
    let metadata = StateMetadata {
        machine_id: MACHINE_ID.to_owned(),
        state_schema_version: IP32_STATE_SCHEMA_VERSION,
        application_version: application_version.to_owned(),
        emulation_config: config.clone(),
    };
    let metadata = postcard::to_stdvec(&metadata).map_err(PersistenceError::PostcardWrite)?;
    if metadata.len() as u64 > MAX_METADATA_BYTES {
        return Err(PersistenceError::MetadataTooLarge {
            length: metadata.len() as u64,
        });
    }
    let payload = postcard::to_stdvec(state).map_err(PersistenceError::PostcardWrite)?;
    if payload.len() as u64 > MAX_UNCOMPRESSED_STATE_BYTES {
        return Err(PersistenceError::StateTooLarge {
            length: payload.len() as u64,
        });
    }
    let compressed =
        zstd::stream::encode_all(payload.as_slice(), ZSTD_LEVEL).map_err(PersistenceError::Io)?;
    if compressed.len() as u64 > MAX_COMPRESSED_STATE_BYTES {
        return Err(PersistenceError::CompressedStateTooLarge {
            length: compressed.len() as u64,
        });
    }
    let mut output = Vec::with_capacity(HEADER_SIZE + metadata.len() + compressed.len());
    output.extend_from_slice(&STATE_MAGIC);
    output.extend_from_slice(&STATE_CONTAINER_VERSION.to_le_bytes());
    output.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    output.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    output.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
    output.extend_from_slice(&hash_bytes(&payload));
    output.extend_from_slice(&metadata);
    output.extend_from_slice(&compressed);
    atomic_write(path, &output)
}

pub(crate) fn read_state_file(path: &Path) -> Result<LoadedStateFile, PersistenceError> {
    let mut file = File::open(path).map_err(PersistenceError::Io)?;
    let file_length = file.metadata().map_err(PersistenceError::Io)?.len();
    let mut header = [0; HEADER_SIZE];
    file.read_exact(&mut header).map_err(PersistenceError::Io)?;
    if header[..8] != STATE_MAGIC {
        return Err(PersistenceError::InvalidStateMagic);
    }
    let version = read_u32(&header[8..12]);
    if version != STATE_CONTAINER_VERSION {
        return Err(PersistenceError::UnsupportedStateContainer { version });
    }
    let metadata_length = read_u64(&header[12..20]);
    let uncompressed_length = read_u64(&header[20..28]);
    let compressed_length = read_u64(&header[28..36]);
    validate_state_lengths(metadata_length, uncompressed_length, compressed_length)?;
    let expected_file_length = (HEADER_SIZE as u64)
        .checked_add(metadata_length)
        .and_then(|length| length.checked_add(compressed_length))
        .ok_or(PersistenceError::StateFileLengthMismatch {
            expected: u64::MAX,
            actual: file_length,
        })?;
    if file_length != expected_file_length {
        return Err(PersistenceError::StateFileLengthMismatch {
            expected: expected_file_length,
            actual: file_length,
        });
    }
    let expected_hash: [u8; 32] = header[36..68]
        .try_into()
        .expect("state hash header has a fixed length");

    let mut metadata = vec![0; metadata_length as usize];
    file.read_exact(&mut metadata)
        .map_err(PersistenceError::Io)?;
    let metadata: StateMetadata =
        postcard::from_bytes(&metadata).map_err(PersistenceError::PostcardRead)?;
    if metadata.machine_id != MACHINE_ID {
        return Err(PersistenceError::WrongMachine {
            machine_id: metadata.machine_id,
        });
    }
    if metadata.state_schema_version != IP32_STATE_SCHEMA_VERSION {
        return Err(PersistenceError::UnsupportedStateSchema {
            version: metadata.state_schema_version,
        });
    }
    metadata.emulation_config.validate()?;
    let topology_limit = state_topology_limit(&metadata.emulation_config);
    if uncompressed_length > topology_limit {
        return Err(PersistenceError::StateExceedsTopologyLimit {
            length: uncompressed_length,
            limit: topology_limit,
        });
    }

    let decoder = zstd::stream::read::Decoder::new(file.take(compressed_length))
        .map_err(PersistenceError::Io)?;
    let mut payload = Vec::with_capacity(uncompressed_length as usize);
    decoder
        .take(uncompressed_length + 1)
        .read_to_end(&mut payload)
        .map_err(PersistenceError::Io)?;
    if payload.len() as u64 != uncompressed_length {
        return Err(PersistenceError::StateLengthMismatch {
            expected: uncompressed_length,
            actual: payload.len() as u64,
        });
    }
    if hash_bytes(&payload) != expected_hash {
        return Err(PersistenceError::StateChecksumMismatch);
    }
    let state = postcard::from_bytes(&payload).map_err(PersistenceError::PostcardRead)?;
    Ok(LoadedStateFile {
        metadata_config: metadata.emulation_config,
        state,
    })
}

fn read_battery(path: &Path) -> Result<Option<BatteryPayload>, PersistenceError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PersistenceError::Io(error)),
    };
    if bytes.len() < 8 + 8 + 32 || bytes[..8] != BATTERY_MAGIC {
        return Err(PersistenceError::InvalidBatteryFile);
    }
    let length = read_u64(&bytes[8..16]) as usize;
    let expected = 16_usize
        .checked_add(length)
        .and_then(|length| length.checked_add(32))
        .ok_or(PersistenceError::InvalidBatteryFile)?;
    if bytes.len() != expected {
        return Err(PersistenceError::InvalidBatteryFile);
    }
    let payload = &bytes[16..16 + length];
    if hash_bytes(payload) != bytes[16 + length..] {
        return Err(PersistenceError::BatteryChecksumMismatch);
    }
    let payload: BatteryPayload =
        postcard::from_bytes(payload).map_err(PersistenceError::PostcardRead)?;
    if payload.version != BATTERY_VERSION {
        return Err(PersistenceError::UnsupportedBattery {
            version: payload.version,
        });
    }
    if payload.machine_id != MACHINE_ID {
        return Err(PersistenceError::WrongMachine {
            machine_id: payload.machine_id,
        });
    }
    Ok(Some(payload))
}

fn apply_rtc_mode(
    state: Ds1687PersistentState,
    anchor: i64,
    mode: RtcPersistenceMode,
    host_now: i64,
) -> Ds1687PersistentState {
    let unix_seconds = match mode {
        RtcPersistenceMode::RealTime => state
            .unix_seconds()
            .saturating_add(host_now.saturating_sub(anchor).max(0)),
        RtcPersistenceMode::Frozen => state.unix_seconds(),
        RtcPersistenceMode::SynchronizeWithHost => host_now,
    };
    Ds1687PersistentState::new(unix_seconds, state.nvram().to_vec(), state.revision())
        .expect("a decoded battery image has already been validated")
}

fn default_battery(mode: RtcPersistenceMode, host_now: i64) -> Ds1687PersistentState {
    let unix_seconds = match mode {
        RtcPersistenceMode::Frozen => DEFAULT_RTC_UNIX_SECONDS,
        RtcPersistenceMode::RealTime | RtcPersistenceMode::SynchronizeWithHost => host_now,
    };
    Ds1687PersistentState::new(unix_seconds, vec![0; 256], 0)
        .expect("the default battery image has the hardware size")
}

fn validate_state_lengths(
    metadata: u64,
    uncompressed: u64,
    compressed: u64,
) -> Result<(), PersistenceError> {
    if metadata > MAX_METADATA_BYTES {
        return Err(PersistenceError::MetadataTooLarge { length: metadata });
    }
    if uncompressed > MAX_UNCOMPRESSED_STATE_BYTES {
        return Err(PersistenceError::StateTooLarge {
            length: uncompressed,
        });
    }
    if compressed > MAX_COMPRESSED_STATE_BYTES {
        return Err(PersistenceError::CompressedStateTooLarge { length: compressed });
    }
    Ok(())
}

fn state_topology_limit(config: &EmulationConfig) -> u64 {
    let memory_bytes = config.machine.crime().memory.total_size_bytes();
    memory_bytes
        .saturating_add(memory_bytes / 4)
        .saturating_add(STATE_NON_MEMORY_ALLOWANCE_BYTES)
        .min(MAX_UNCOMPRESSED_STATE_BYTES)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), PersistenceError> {
    let parent = path
        .parent()
        .ok_or_else(|| PersistenceError::MissingParent(path.to_owned()))?;
    fs::create_dir_all(parent).map_err(PersistenceError::Io)?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(PersistenceError::Io)?;
    temporary.write_all(bytes).map_err(PersistenceError::Io)?;
    temporary.flush().map_err(PersistenceError::Io)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(PersistenceError::Io)?;
    temporary
        .persist(path)
        .map_err(|error| PersistenceError::Io(error.error))?;
    Ok(())
}

fn preserve_corrupt_file(path: &Path, host_now: i64) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let backup = path.with_extension(format!("corrupt.{host_now}"));
    fs::rename(path, &backup).ok().map(|()| backup)
}

fn encode_hash(hash: [u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in hash {
        use std::fmt::Write as _;
        write!(output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn decode_hash(value: &str) -> Result<[u8; 32], PersistenceError> {
    if value.len() != 64 {
        return Err(PersistenceError::InvalidHash);
    }
    let mut output = [0; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| PersistenceError::InvalidHash)?;
    }
    Ok(output)
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(
        bytes
            .try_into()
            .expect("u32 header field has a fixed length"),
    )
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(
        bytes
            .try_into()
            .expect("u64 header field has a fixed length"),
    )
}

#[derive(Debug)]
pub(crate) enum PersistenceError {
    ApplicationDirectoriesUnavailable,
    Io(std::io::Error),
    TomlRead(toml::de::Error),
    TomlWrite(toml::ser::Error),
    PostcardRead(postcard::Error),
    PostcardWrite(postcard::Error),
    PromPathNotAbsolute(PathBuf),
    PromHashMismatch,
    InvalidHash,
    InvalidStateMagic,
    UnsupportedStateContainer { version: u32 },
    UnsupportedStateSchema { version: u32 },
    UnsupportedEmulationConfig { version: u32 },
    InvalidMachineConfiguration(Ip32PersistentConfigError),
    UnsupportedBattery { version: u32 },
    WrongMachine { machine_id: String },
    MetadataTooLarge { length: u64 },
    StateTooLarge { length: u64 },
    CompressedStateTooLarge { length: u64 },
    StateLengthMismatch { expected: u64, actual: u64 },
    StateExceedsTopologyLimit { length: u64, limit: u64 },
    StateFileLengthMismatch { expected: u64, actual: u64 },
    StateChecksumMismatch,
    InvalidBatteryFile,
    BatteryChecksumMismatch,
    MissingParent(PathBuf),
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApplicationDirectoriesUnavailable => {
                formatter.write_str("application persistence directories are unavailable")
            }
            Self::Io(error) => error.fmt(formatter),
            Self::TomlRead(error) => write!(formatter, "invalid emulation configuration: {error}"),
            Self::TomlWrite(error) => {
                write!(formatter, "cannot encode emulation configuration: {error}")
            }
            Self::PostcardRead(error) => write!(formatter, "invalid persistence payload: {error}"),
            Self::PostcardWrite(error) => {
                write!(formatter, "cannot encode persistence payload: {error}")
            }
            Self::PromPathNotAbsolute(path) => {
                write!(formatter, "PROM path is not absolute: {}", path.display())
            }
            Self::PromHashMismatch => {
                formatter.write_str("System PROM SHA-256 does not match the saved configuration")
            }
            Self::InvalidHash => {
                formatter.write_str("invalid SHA-256 value in persistence metadata")
            }
            Self::InvalidStateMagic => formatter.write_str("invalid sgi-emu state-file magic"),
            Self::UnsupportedStateContainer { version } => {
                write!(formatter, "unsupported state container version {version}")
            }
            Self::UnsupportedStateSchema { version } => {
                write!(formatter, "unsupported IP32 state schema {version}")
            }
            Self::UnsupportedEmulationConfig { version } => write!(
                formatter,
                "unsupported emulation configuration version {version}"
            ),
            Self::InvalidMachineConfiguration(error) => {
                write!(
                    formatter,
                    "invalid emulation machine configuration: {error}"
                )
            }
            Self::UnsupportedBattery { version } => {
                write!(formatter, "unsupported RTC/NVRAM battery version {version}")
            }
            Self::WrongMachine { machine_id } => write!(
                formatter,
                "persistence data targets machine {machine_id}, not {MACHINE_ID}"
            ),
            Self::MetadataTooLarge { length } => {
                write!(formatter, "state metadata is too large: {length} bytes")
            }
            Self::StateTooLarge { length } => {
                write!(formatter, "uncompressed state is too large: {length} bytes")
            }
            Self::CompressedStateTooLarge { length } => {
                write!(formatter, "compressed state is too large: {length} bytes")
            }
            Self::StateLengthMismatch { expected, actual } => write!(
                formatter,
                "state payload length mismatch: expected {expected}, got {actual}"
            ),
            Self::StateExceedsTopologyLimit { length, limit } => write!(
                formatter,
                "state payload length {length} exceeds the configured topology limit {limit}"
            ),
            Self::StateFileLengthMismatch { expected, actual } => write!(
                formatter,
                "state file length mismatch: expected {expected}, got {actual}"
            ),
            Self::StateChecksumMismatch => {
                formatter.write_str("state payload SHA-256 does not match")
            }
            Self::InvalidBatteryFile => formatter.write_str("invalid RTC/NVRAM battery file"),
            Self::BatteryChecksumMismatch => {
                formatter.write_str("RTC/NVRAM battery checksum does not match")
            }
            Self::MissingParent(path) => write!(
                formatter,
                "persistence path has no parent directory: {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for PersistenceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use se_machine::o2::ip32::machine::{Ip32Machine, Ip32MachineConfig};

    #[test]
    fn real_time_clamps_a_backward_host_clock() {
        let state = Ds1687PersistentState::new(1_000, vec![0; 256], 3).unwrap();
        assert_eq!(
            apply_rtc_mode(state, 2_000, RtcPersistenceMode::RealTime, 1_000).unix_seconds(),
            1_000
        );
    }

    #[test]
    fn hash_text_round_trips() {
        let hash = hash_bytes(b"sgi-emu");
        assert_eq!(decode_hash(&encode_hash(hash)).unwrap(), hash);
    }

    #[test]
    fn state_container_round_trips_and_rejects_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let prom_path = directory.path().join("ip32prom.bin");
        let prom = vec![0; 512 * 1024];
        fs::write(&prom_path, &prom).unwrap();
        let config = EmulationConfig::new(
            prom_path,
            hash_bytes(&prom),
            RtcPersistenceMode::Frozen,
            Ip32PersistentConfig::default(),
        )
        .unwrap();
        let machine = Ip32Machine::from_config(Ip32MachineConfig::default()).unwrap();
        let state = machine.save_state().unwrap();
        let path = directory.path().join("state.sestate");
        write_state_file(&path, "0.1.0", &config, &state).unwrap();
        let loaded = read_state_file(&path).unwrap();
        assert_eq!(loaded.metadata_config, config);
        assert_eq!(loaded.state.schema_version(), IP32_STATE_SCHEMA_VERSION);

        let original = fs::read(&path).unwrap();
        let mut oversized = original.clone();
        oversized[20..28].copy_from_slice(&(state_topology_limit(&config) + 1).to_le_bytes());
        fs::write(&path, oversized).unwrap();
        assert!(matches!(
            read_state_file(&path),
            Err(PersistenceError::StateExceedsTopologyLimit { .. })
        ));

        let mut corrupted = original;
        *corrupted.last_mut().unwrap() ^= 0x80;
        fs::write(&path, corrupted).unwrap();
        assert!(read_state_file(&path).is_err());
    }

    #[test]
    fn emulation_config_and_battery_use_atomic_files() {
        let directory = tempfile::tempdir().unwrap();
        let paths = PersistencePaths {
            config_file: directory.path().join("emulation.toml"),
            battery_file: directory.path().join("machines/ip32/rtc_nvram.bin"),
        };
        let prom_path = directory.path().join("prom.bin");
        let config = EmulationConfig::new(
            prom_path,
            [0x5a; 32],
            RtcPersistenceMode::RealTime,
            Ip32PersistentConfig::default(),
        )
        .unwrap();
        save_emulation_config(&paths, &config).unwrap();
        assert_eq!(load_emulation_config(&paths).unwrap(), Some(config));

        let rtc = Ds1687PersistentState::new(10, vec![0x33; 256], 4).unwrap();
        save_battery(&paths, rtc, 100).unwrap();
        let loaded = load_battery(&paths, RtcPersistenceMode::RealTime, 125);
        assert_eq!(loaded.state.unix_seconds(), 35);
        assert_eq!(loaded.state.revision(), 4);
    }
}
