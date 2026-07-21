//! MACE video channel state and fixed-point pixel pipeline helpers.

/// Software-visible MACE video pixel formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum VideoPixelFormat {
    Rgba8888,
    Abgr8888,
    Rgba5551,
    Yuv422_8,
    Yuv422_10,
    Yuva4224,
}

/// One video channel register set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoChannel {
    pub output: bool,
    pub control: u16,
    pub status: u16,
    pub config: u32,
    pub next_descriptor: u32,
    pub field_offset: u16,
    pub line_or_field_size: u32,
    pub geometry: [u64; 6],
    pub descriptors: [u64; 8],
    pub field_counter: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct VideoChannelState {
    output: bool,
    control: u16,
    status: u16,
    config: u32,
    next_descriptor: u32,
    field_offset: u16,
    line_or_field_size: u32,
    geometry: [u64; 6],
    descriptors: [u64; 8],
    field_counter: u32,
}

impl VideoChannel {
    pub const fn new(output: bool) -> Self {
        Self {
            output,
            control: 1,
            status: 0,
            config: 0,
            next_descriptor: 0,
            field_offset: 0,
            line_or_field_size: 0,
            geometry: [0; 6],
            descriptors: [0; 8],
            field_counter: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.output);
    }

    pub(super) fn save_state(&self) -> VideoChannelState {
        VideoChannelState {
            output: self.output,
            control: self.control,
            status: self.status,
            config: self.config,
            next_descriptor: self.next_descriptor,
            field_offset: self.field_offset,
            line_or_field_size: self.line_or_field_size,
            geometry: self.geometry,
            descriptors: self.descriptors,
            field_counter: self.field_counter,
        }
    }

    pub(super) const fn configuration_matches(&self, state: &VideoChannelState) -> bool {
        self.output == state.output
    }

    pub(super) fn validate_state(state: &VideoChannelState) -> Result<(), &'static str> {
        let config_mask = if state.output {
            0x003f_ffff
        } else {
            0x0001_ffff
        };
        let size_mask = if state.output { 0x003f_ffff } else { 0x0ff8 };
        if state.control & !0x03ff != 0
            || state.status & !0x07ff != 0
            || state.config & !config_mask != 0
            || state.next_descriptor & !0xffff_ffc7 != 0
            || state.field_offset & !0xfff8 != 0
            || state.line_or_field_size & !size_mask != 0
        {
            return Err("MACE video registers must use implemented bit encodings");
        }
        Ok(())
    }

    pub(super) fn restore_state(&mut self, state: VideoChannelState) {
        self.control = state.control;
        self.status = state.status;
        self.config = state.config;
        self.next_descriptor = state.next_descriptor;
        self.field_offset = state.field_offset;
        self.line_or_field_size = state.line_or_field_size;
        self.geometry = state.geometry;
        self.descriptors = state.descriptors;
        self.field_counter = state.field_counter;
    }
    pub const fn enabled(&self) -> bool {
        self.control & 2 != 0 && self.control & 1 == 0
    }
    pub fn complete_field(&mut self) {
        self.field_counter = self.field_counter.wrapping_add(1);
        self.status |= 1;
    }

    pub fn read(&self, offset: u64) -> Option<u64> {
        match offset {
            0x00 => Some(u64::from(self.control)),
            0x08 => Some(u64::from(self.status)),
            0x10 => Some(u64::from(self.config)),
            0x18 => Some(u64::from(self.next_descriptor)),
            0x20 => Some(u64::from(self.field_offset)),
            0x28 => Some(u64::from(self.line_or_field_size)),
            0x30..=0x58 if offset & 7 == 0 => Some(self.geometry[((offset - 0x30) / 8) as usize]),
            0x80..=0xb8 if offset & 7 == 0 => {
                Some(self.descriptors[((offset - 0x80) / 8) as usize])
            }
            _ => None,
        }
    }

    pub fn write(&mut self, offset: u64, value: u64) -> bool {
        match offset {
            0x00 => {
                self.control = value as u16 & 0x03ff;
                if self.control & 1 != 0 {
                    self.reset();
                }
            }
            0x08 => self.status &= !(value as u16 & 0x07ff),
            0x10 => {
                self.config = value as u32
                    & if self.output {
                        0x003f_ffff
                    } else {
                        0x0001_ffff
                    }
            }
            0x18 => self.next_descriptor = value as u32 & 0xffff_ffc7,
            0x20 => self.field_offset = value as u16 & 0xfff8,
            0x28 => {
                self.line_or_field_size =
                    value as u32 & if self.output { 0x003f_ffff } else { 0x0ff8 }
            }
            0x30..=0x58 if offset & 7 == 0 => self.geometry[((offset - 0x30) / 8) as usize] = value,
            0x80..=0xb8 if offset & 7 == 0 => {
                self.descriptors[((offset - 0x80) / 8) as usize] = value
            }
            _ => return false,
        }
        true
    }
}

/// Converts studio-range YUV into clamped RGB using MACE fixed coefficients.
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = i32::from(y);
    let u = i32::from(u);
    let v = i32::from(v);
    [
        clamp8((1192 * y + 1634 * v - 227_712) >> 10),
        clamp8((1192 * y - 832 * v - 401 * u + 139_264) >> 10),
        clamp8((1192 * y + 2066 * u - 283_008) >> 10),
    ]
}

/// Converts RGB into studio-range YUV using MACE fixed coefficients.
pub fn rgb_to_yuv(red: u8, green: u8, blue: u8) -> [u8; 3] {
    let red = i32::from(red);
    let green = i32::from(green);
    let blue = i32::from(blue);
    [
        clamp8((263 * red + 516 * green + 100 * blue + 16_896) >> 10),
        clamp8((-152 * red - 298 * green + 450 * blue + 131_584) >> 10),
        clamp8((450 * red - 377 * green - 73 * blue + 131_584) >> 10),
    ]
}

/// Applies the 1/4, 1/2, 1/4 chroma filter with hardware rounding.
pub fn subsample(left: u8, center: u8, right: u8) -> u8 {
    ((u16::from(left) + 2 * u16::from(center) + u16::from(right) + 2) >> 2) as u8
}

/// Applies the documented 4x4 ordered dither before RGB555 packing.
pub fn dither_555(red: u8, green: u8, blue: u8, x: usize, y: usize) -> u16 {
    const MATRIX: [[u8; 4]; 4] = [[12, 7, 11, 0], [10, 1, 13, 6], [5, 14, 2, 9], [3, 8, 4, 15]];
    let threshold = MATRIX[y & 3][x & 3];
    let quantize = |value: u8| ((u16::from(value) * 31 + u16::from(threshold)) / 255).min(31);
    quantize(red) << 11 | quantize(green) << 6 | quantize(blue) << 1 | 1
}

/// Produces one weighted average for arbitrary down-scaling.
pub fn block_filter(samples: &[u16], weights: &[u16]) -> u16 {
    assert_eq!(samples.len(), weights.len());
    let total_weight: u64 = weights.iter().map(|&value| u64::from(value)).sum();
    if total_weight == 0 {
        return 0;
    }
    let sum: u64 = samples
        .iter()
        .zip(weights)
        .map(|(&sample, &weight)| u64::from(sample) * u64::from(weight))
        .sum();
    ((sum + total_weight / 2) / total_weight) as u16
}

fn clamp8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn color_conversion_uses_studio_ranges() {
        assert_eq!(rgb_to_yuv(0, 0, 0), [16, 128, 128]);
        assert_eq!(yuv_to_rgb(16, 128, 128), [0, 0, 0]);
    }
    #[test]
    fn descriptor_registers_are_independent() {
        let mut channel = VideoChannel::new(false);
        assert!(channel.write(0x98, 0x1234));
        assert_eq!(channel.read(0x98), Some(0x1234));
    }
}
