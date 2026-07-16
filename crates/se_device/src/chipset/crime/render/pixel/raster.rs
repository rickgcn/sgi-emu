//! Deterministic PixelPipe raster iteration.

/// One candidate pixel emitted by a rasterizer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct RasterPosition {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

/// Direction of an axis-aligned X line.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum AxisLineDirection {
    Horizontal,
    Vertical,
}

/// Inclusive axis-aligned line iterator.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct AxisLineRasterizer {
    pub(crate) direction: AxisLineDirection,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) end_x: u16,
    pub(crate) end_y: u16,
}

impl AxisLineRasterizer {
    pub(crate) fn complete(&self) -> bool {
        match self.direction {
            AxisLineDirection::Horizontal => self.x > self.end_x,
            AxisLineDirection::Vertical => self.y > self.end_y,
        }
    }

    pub(crate) fn position(&self) -> RasterPosition {
        RasterPosition {
            x: self.x,
            y: self.y,
        }
    }

    pub(crate) fn contiguous(&self) -> bool {
        self.direction == AxisLineDirection::Horizontal
    }

    pub(crate) fn remaining_in_row(&self) -> u16 {
        if self.contiguous() {
            self.end_x - self.x + 1
        } else {
            1
        }
    }

    pub(crate) fn advance(&mut self, pixels: u16) {
        match self.direction {
            AxisLineDirection::Horizontal => self.x = self.x.saturating_add(pixels),
            AxisLineDirection::Vertical => self.y = self.y.saturating_add(pixels),
        }
    }
}

/// Vertical row direction selected by the rectangle traversal field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum RectangleRowDirection {
    Ascending,
    Descending,
}

/// Inclusive rectangle iterator with left-to-right rows.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct InclusiveRectangleRasterizer {
    pub(crate) x_start: u16,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) end_x: u16,
    pub(crate) end_y: u16,
    pub(crate) row_direction: RectangleRowDirection,
    pub(crate) finished: bool,
}

impl InclusiveRectangleRasterizer {
    pub(crate) fn complete(&self) -> bool {
        self.finished
    }

    pub(crate) fn position(&self) -> RasterPosition {
        RasterPosition {
            x: self.x,
            y: self.y,
        }
    }

    pub(crate) fn remaining_in_row(&self) -> u16 {
        self.end_x - self.x + 1
    }

    pub(crate) fn advance(&mut self, pixels: u16) {
        let next = u32::from(self.x) + u32::from(pixels);
        if next > u32::from(self.end_x) {
            self.x = self.x_start;
            if self.y == self.end_y {
                self.finished = true;
            } else {
                self.y = match self.row_direction {
                    RectangleRowDirection::Ascending => self.y + 1,
                    RectangleRowDirection::Descending => self.y - 1,
                };
            }
        } else {
            self.x = next as u16;
        }
    }
}

/// Rasterizer selected by a decoded PixelPipe command.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum Rasterizer {
    AxisLine(AxisLineRasterizer),
    InclusiveRectangle(InclusiveRectangleRasterizer),
}

impl Rasterizer {
    pub(crate) fn complete(&self) -> bool {
        match self {
            Self::AxisLine(rasterizer) => rasterizer.complete(),
            Self::InclusiveRectangle(rasterizer) => rasterizer.complete(),
        }
    }

    pub(crate) fn position(&self) -> RasterPosition {
        match self {
            Self::AxisLine(rasterizer) => rasterizer.position(),
            Self::InclusiveRectangle(rasterizer) => rasterizer.position(),
        }
    }

    pub(crate) fn contiguous(&self) -> bool {
        match self {
            Self::AxisLine(rasterizer) => rasterizer.contiguous(),
            Self::InclusiveRectangle(_) => true,
        }
    }

    pub(crate) fn remaining_in_row(&self) -> u16 {
        match self {
            Self::AxisLine(rasterizer) => rasterizer.remaining_in_row(),
            Self::InclusiveRectangle(rasterizer) => rasterizer.remaining_in_row(),
        }
    }

    pub(crate) fn advance(&mut self, pixels: u16) {
        match self {
            Self::AxisLine(rasterizer) => rasterizer.advance(pixels),
            Self::InclusiveRectangle(rasterizer) => rasterizer.advance(pixels),
        }
    }
}
