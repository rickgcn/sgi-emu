//! Evidence-backed transparent line-stipple state.

/// Frozen line-stipple register state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PixelStippleMode {
    pub(crate) index: u8,
    pub(crate) max_index: u8,
    pub(crate) repeat_count: u8,
    pub(crate) max_repeat: u8,
}

impl PixelStippleMode {
    pub(crate) const fn decode(raw: u32) -> Self {
        Self {
            index: (raw >> 24) as u8,
            max_index: (raw >> 16) as u8,
            repeat_count: (raw >> 8) as u8,
            max_repeat: raw as u8,
        }
    }
}

/// Cursor for line and polygon stipple repetition and wrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PixelStippleCursor {
    pattern: u32,
    index: u8,
    max_index: u8,
    repeat_count: u8,
    max_repeat: u8,
}

impl PixelStippleCursor {
    pub(crate) const fn new(pattern: u32, mode: PixelStippleMode) -> Self {
        Self {
            pattern,
            index: mode.index,
            max_index: mode.max_index,
            repeat_count: mode.repeat_count,
            max_repeat: mode.max_repeat,
        }
    }

    pub(crate) const fn index(self) -> u8 {
        self.index
    }

    pub(crate) fn permits(self, candidate_offset: u16) -> bool {
        let mut cursor = self;
        cursor.advance(candidate_offset);
        cursor.pattern & (1_u32 << (31 - cursor.index)) != 0
    }

    pub(crate) fn advance(&mut self, candidates: u16) {
        for _ in 0..candidates {
            if self.repeat_count < self.max_repeat {
                self.repeat_count += 1;
            } else {
                self.repeat_count = 0;
                self.index = if self.index >= self.max_index {
                    0
                } else {
                    self.index + 1
                };
            }
        }
    }
}
