//! MACE audio TDM control and stereo DMA rings.

/// Direction of an audio DMA channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum AudioDirection {
    Input,
    Output,
}

/// One fixed 4 KiB stereo DMA ring.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
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
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceAudio {
    pub control: u32,
    pub codec_control: u32,
    pub codec_mask: u16,
    pub codec_status: u16,
    pub channels: [AudioDmaChannel; 3],
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
