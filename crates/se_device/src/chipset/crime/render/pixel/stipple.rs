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

/// Cursor for the proven, non-repeating stipple subset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PixelStippleCursor {
    pattern: u32,
    index: u8,
    max_index: u8,
}

impl PixelStippleCursor {
    pub(crate) const fn new(pattern: u32, mode: PixelStippleMode) -> Self {
        Self {
            pattern,
            index: mode.index,
            max_index: mode.max_index,
        }
    }

    pub(crate) const fn index(self) -> u8 {
        self.index
    }

    pub(crate) fn permits(self, candidate_offset: u16) -> bool {
        let index = u16::from(self.index) + candidate_offset;
        debug_assert!(index <= 31);
        self.pattern & (1_u32 << (31 - index)) != 0
    }

    pub(crate) fn advance(&mut self, candidates: u16) {
        self.index = self.index.saturating_add(candidates as u8);
    }
}
