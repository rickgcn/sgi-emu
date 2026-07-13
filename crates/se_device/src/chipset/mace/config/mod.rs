//! MACE 2.0 construction configuration.

/// Bounded host-neutral port capacities.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MacePortConfig {
    pub ethernet_frames: usize,
    pub audio_sample_pairs: usize,
    pub video_fields: usize,
    pub byte_stream_bytes: usize,
}

impl Default for MacePortConfig {
    fn default() -> Self {
        Self {
            ethernet_frames: 256,
            audio_sample_pairs: 65_536,
            video_fields: 4,
            byte_stream_bytes: 65_536,
        }
    }
}

/// Complete MACE 2.0 configuration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceConfig {
    pub ports: MacePortConfig,
}
