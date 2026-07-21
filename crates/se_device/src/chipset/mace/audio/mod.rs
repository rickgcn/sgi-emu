//! MACE audio TDM control and stereo DMA rings.

/// Direction of an audio DMA channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum AudioDirection {
    Input,
    Output,
}

/// One fixed 4 KiB stereo DMA ring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioDmaChannel {
    pub direction: AudioDirection,
    pub control: u16,
    pub read_pointer: u16,
    pub write_pointer: u16,
    pub memory_error: bool,
}

impl AudioDmaChannel {
    pub const fn new(direction: AudioDirection) -> Self {
        Self {
            direction,
            control: 1 << 10,
            read_pointer: 0,
            write_pointer: 0,
            memory_error: false,
        }
    }
    pub const fn enabled(&self) -> bool {
        self.control & (1 << 9) != 0 && self.control & (1 << 10) == 0
    }
    pub const fn depth(&self) -> u16 {
        self.write_pointer.wrapping_sub(self.read_pointer) & 0x0fe0
    }
    pub fn set_control(&mut self, value: u16) {
        self.control = value & 0x06e0;
        if value & (1 << 10) != 0 {
            let direction = self.direction;
            *self = Self::new(direction);
        }
    }
    pub fn threshold_interrupt(&self) -> bool {
        let depth = self.depth() >> 5;
        let condition = (self.control >> 5) & 7;
        let level = match condition {
            1 => 32,
            2 => 64,
            3 => 96,
            _ => 0,
        };
        match (condition, self.direction) {
            (0, _) => false,
            (1..=3, AudioDirection::Input) => depth >= level,
            (1..=3, AudioDirection::Output) => depth < level,
            (4, _) => depth == 0,
            (5, _) => depth != 0,
            (6, _) => depth == 127,
            (7, _) => depth != 127,
            _ => false,
        }
    }
}

/// Complete MACE audio interface state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaceAudio {
    pub control: u32,
    pub codec_control: u32,
    pub codec_mask: u16,
    pub codec_status: u16,
    pub channels: [AudioDmaChannel; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct AudioDmaChannelState {
    direction: AudioDirection,
    control: u16,
    read_pointer: u16,
    write_pointer: u16,
    memory_error: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct MaceAudioState {
    control: u32,
    codec_control: u32,
    codec_mask: u16,
    codec_status: u16,
    channels: [AudioDmaChannelState; 3],
}

impl MaceAudio {
    pub const fn new() -> Self {
        Self {
            control: 1,
            codec_control: 1 << 16,
            codec_mask: 0,
            codec_status: 0,
            channels: [
                AudioDmaChannel::new(AudioDirection::Input),
                AudioDmaChannel::new(AudioDirection::Output),
                AudioDmaChannel::new(AudioDirection::Output),
            ],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
    pub const fn codec_interrupt(&self) -> bool {
        self.codec_status & self.codec_mask != 0
    }

    pub(super) fn save_state(&self) -> MaceAudioState {
        MaceAudioState {
            control: self.control,
            codec_control: self.codec_control,
            codec_mask: self.codec_mask,
            codec_status: self.codec_status,
            channels: self.channels.map(|channel| AudioDmaChannelState {
                direction: channel.direction,
                control: channel.control,
                read_pointer: channel.read_pointer,
                write_pointer: channel.write_pointer,
                memory_error: channel.memory_error,
            }),
        }
    }

    pub(super) fn configuration_matches(&self, state: &MaceAudioState) -> bool {
        self.channels
            .iter()
            .zip(&state.channels)
            .all(|(current, saved)| current.direction == saved.direction)
    }

    pub(super) fn validate_state(state: &MaceAudioState) -> Result<(), &'static str> {
        if state.control & !0x01ff_ffff != 0
            || state.codec_control & !0x00ff_ffff != 0
            || state.channels.iter().any(|channel| {
                channel.control & !0x06e0 != 0
                    || channel.read_pointer & !0x0fe0 != 0
                    || channel.write_pointer & !0x0fe0 != 0
            })
        {
            return Err("MACE audio registers and DMA rings must use valid encodings");
        }
        Ok(())
    }

    pub(super) fn restore_state(&mut self, state: MaceAudioState) {
        self.control = state.control;
        self.codec_control = state.codec_control;
        self.codec_mask = state.codec_mask;
        self.codec_status = state.codec_status;
        for (current, saved) in self.channels.iter_mut().zip(state.channels) {
            current.control = saved.control;
            current.read_pointer = saved.read_pointer;
            current.write_pointer = saved.write_pointer;
            current.memory_error = saved.memory_error;
        }
    }

    /// Packs one signed stereo input sample into the DMA format.
    pub fn pack_input(left: i32, right: i32) -> u64 {
        let left = (left as u32 & 0xffff_ff00) as u64;
        let right = (right as u32 & 0xffff_ff00) as u64;
        left << 32 | right
    }

    /// Saturates the 24-bit DMA representation to a signed 16-bit codec sample.
    pub fn unpack_output(word: u64) -> (i16, i16) {
        (
            saturate_sample((word >> 32) as i32),
            saturate_sample(word as i32),
        )
    }
}

impl Default for MaceAudio {
    fn default() -> Self {
        Self::new()
    }
}

fn saturate_sample(value: i32) -> i16 {
    let sample = value >> 8;
    sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_samples_saturate() {
        assert_eq!(
            MaceAudio::unpack_output(0x7fff_ff00_8000_0000),
            (i16::MAX, i16::MIN)
        );
    }
    #[test]
    fn ring_threshold_follows_direction() {
        let mut input = AudioDmaChannel::new(AudioDirection::Input);
        input.control = 1 << 5;
        input.write_pointer = 32 << 5;
        assert!(input.threshold_interrupt());
    }
}
