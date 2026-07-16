//! Deterministic PixelPipe candidate generation.

use std::collections::BTreeSet;

/// One candidate pixel emitted by a rasterizer.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub(crate) struct RasterPosition {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

/// Serializable cursor over a frozen candidate list.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Rasterizer {
    positions: Vec<RasterPosition>,
    index: usize,
}

impl Rasterizer {
    pub(crate) fn point(x: u16, y: u16) -> Self {
        Self::new(vec![RasterPosition { x, y }])
    }

    pub(crate) fn line_x(
        x0: u16,
        y0: u16,
        x1: u16,
        y1: u16,
        half_width_subpixel: u16,
        skip_last: bool,
    ) -> Self {
        let mut center = bresenham(x0, y0, x1, y1);
        if skip_last {
            center.pop();
        }
        let width = (u32::from(half_width_subpixel) * 2).div_ceil(64).max(1) as i32;
        if width == 1 {
            return Self::new(center);
        }
        let low = -(width / 2);
        let high = low + width - 1;
        let mut positions = BTreeSet::new();
        for center in center {
            for y_offset in low..=high {
                for x_offset in low..=high {
                    if let (Some(x), Some(y)) = (
                        add_coordinate(center.x, x_offset),
                        add_coordinate(center.y, y_offset),
                    ) {
                        positions.insert(RasterPosition { x, y });
                    }
                }
            }
        }
        Self::new(positions.into_iter().collect())
    }

    pub(crate) fn line_gl(
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        half_width_subpixel: u16,
        skip_last: bool,
    ) -> Self {
        let radius = i64::from(half_width_subpixel.max(32));
        let min_x = ((i64::from(x0.min(x1)) - radius - 32).div_euclid(64)).max(0);
        let max_x = ((i64::from(x0.max(x1)) + radius - 32).div_euclid(64)).min(65_535);
        let min_y = ((i64::from(y0.min(y1)) - radius - 32).div_euclid(64)).max(0);
        let max_y = ((i64::from(y0.max(y1)) + radius - 32).div_euclid(64)).min(65_535);
        let dx = i128::from(x1) - i128::from(x0);
        let dy = i128::from(y1) - i128::from(y0);
        let length_squared = dx * dx + dy * dy;
        let mut positions = Vec::new();
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = i128::from(x * 64 + 32 - i64::from(x0));
                let py = i128::from(y * 64 + 32 - i64::from(y0));
                let projection = (px * dx + py * dy).clamp(0, length_squared);
                if skip_last && projection == length_squared {
                    continue;
                }
                let distance_numerator = (px * dy - py * dx).pow(2);
                if length_squared == 0 {
                    if px * px + py * py <= i128::from(radius * radius) {
                        positions.push(RasterPosition {
                            x: x as u16,
                            y: y as u16,
                        });
                    }
                } else if distance_numerator <= i128::from(radius * radius) * length_squared
                    && projection >= 0
                    && projection <= length_squared
                {
                    positions.push(RasterPosition {
                        x: x as u16,
                        y: y as u16,
                    });
                }
            }
        }
        Self::new(positions)
    }

    pub(crate) fn rectangle_x(x0: u16, y0: u16, x1: u16, y1: u16, edge_type: u8) -> Self {
        let min_x = x0.min(x1);
        let max_x = x0.max(x1);
        let min_y = y0.min(y1);
        let max_y = y0.max(y1);
        let left_to_right = edge_type & 1 == 0;
        let top_to_bottom = edge_type & 2 != 0;
        let mut positions = Vec::new();
        let rows: Box<dyn Iterator<Item = u16>> = if top_to_bottom {
            Box::new(min_y..=max_y)
        } else {
            Box::new((min_y..=max_y).rev())
        };
        for y in rows {
            if left_to_right {
                positions.extend((min_x..=max_x).map(|x| RasterPosition { x, y }));
            } else {
                positions.extend((min_x..=max_x).rev().map(|x| RasterPosition { x, y }));
            }
        }
        Self::new(positions)
    }

    pub(crate) fn rectangle_gl(x0: i32, y0: i32, x1: i32, y1: i32, edge_type: u8) -> Self {
        let min_x = ((i64::from(x0.min(x1)) - 32).div_euclid(64) + 1).max(0) as u16;
        let max_x = ((i64::from(x0.max(x1)) - 32).div_euclid(64)).clamp(0, 65_535) as u16;
        let min_y = ((i64::from(y0.min(y1)) - 32).div_euclid(64) + 1).max(0) as u16;
        let max_y = ((i64::from(y0.max(y1)) - 32).div_euclid(64)).clamp(0, 65_535) as u16;
        if min_x > max_x || min_y > max_y {
            return Self::new(Vec::new());
        }
        Self::rectangle_x(min_x, min_y, max_x, max_y, edge_type)
    }

    pub(crate) fn triangle(vertices: [(i32, i32); 3]) -> Self {
        let area = edge(vertices[0], vertices[1], vertices[2]);
        if area == 0 {
            return Self::new(Vec::new());
        }
        let vertices = if area < 0 {
            [vertices[0], vertices[2], vertices[1]]
        } else {
            vertices
        };
        let min_x = ((i64::from(vertices.iter().map(|v| v.0).min().unwrap()) - 32).div_euclid(64)
            + 1)
        .max(0);
        let max_x = ((i64::from(vertices.iter().map(|v| v.0).max().unwrap()) - 32).div_euclid(64))
            .min(65_535);
        let min_y = ((i64::from(vertices.iter().map(|v| v.1).min().unwrap()) - 32).div_euclid(64)
            + 1)
        .max(0);
        let max_y = ((i64::from(vertices.iter().map(|v| v.1).max().unwrap()) - 32).div_euclid(64))
            .min(65_535);
        let mut positions = Vec::new();
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let sample = ((x * 64 + 32) as i32, (y * 64 + 32) as i32);
                if edge_passes(vertices[0], vertices[1], sample)
                    && edge_passes(vertices[1], vertices[2], sample)
                    && edge_passes(vertices[2], vertices[0], sample)
                {
                    positions.push(RasterPosition {
                        x: x as u16,
                        y: y as u16,
                    });
                }
            }
        }
        Self::new(positions)
    }

    pub(crate) fn empty() -> Self {
        Self::new(Vec::new())
    }

    fn new(positions: Vec<RasterPosition>) -> Self {
        Self {
            positions,
            index: 0,
        }
    }

    pub(crate) fn complete(&self) -> bool {
        self.index >= self.positions.len()
    }

    pub(crate) fn position(&self) -> RasterPosition {
        self.positions[self.index]
    }

    pub(crate) fn contiguous(&self) -> bool {
        self.remaining_in_row() > 1
    }

    pub(crate) fn remaining_in_row(&self) -> u16 {
        let Some(first) = self.positions.get(self.index) else {
            return 0;
        };
        let mut count = 1_usize;
        while let Some(next) = self.positions.get(self.index + count) {
            if next.y != first.y || next.x != first.x.saturating_add(count as u16) {
                break;
            }
            count += 1;
        }
        count.min(usize::from(u16::MAX)) as u16
    }

    pub(crate) fn advance(&mut self, pixels: u16) {
        self.index = (self.index + usize::from(pixels)).min(self.positions.len());
    }
}

fn bresenham(x0: u16, y0: u16, x1: u16, y1: u16) -> Vec<RasterPosition> {
    let mut x = i32::from(x0);
    let mut y = i32::from(y0);
    let end_x = i32::from(x1);
    let end_y = i32::from(y1);
    let dx = (end_x - x).abs();
    let sx = if x < end_x { 1 } else { -1 };
    let dy = -(end_y - y).abs();
    let sy = if y < end_y { 1 } else { -1 };
    let mut error = dx + dy;
    let mut positions = Vec::new();
    loop {
        positions.push(RasterPosition {
            x: x as u16,
            y: y as u16,
        });
        if x == end_x && y == end_y {
            break;
        }
        let doubled = error * 2;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
    positions
}

fn add_coordinate(value: u16, offset: i32) -> Option<u16> {
    u16::try_from(i32::from(value) + offset).ok()
}

fn edge(start: (i32, i32), end: (i32, i32), point: (i32, i32)) -> i64 {
    i64::from(point.0 - start.0) * i64::from(end.1 - start.1)
        - i64::from(point.1 - start.1) * i64::from(end.0 - start.0)
}

fn top_left(start: (i32, i32), end: (i32, i32)) -> bool {
    let dy = end.1 - start.1;
    let dx = end.0 - start.0;
    dy < 0 || dy == 0 && dx > 0
}

fn edge_passes(start: (i32, i32), end: (i32, i32), point: (i32, i32)) -> bool {
    let value = edge(start, end, point);
    value > 0 || value == 0 && top_left(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bresenham_covers_reverse_and_degenerate_lines() {
        assert_eq!(bresenham(2, 2, 0, 0).len(), 3);
        assert_eq!(bresenham(4, 5, 4, 5), [RasterPosition { x: 4, y: 5 }]);
    }

    #[test]
    fn triangle_uses_top_left_shared_edge_rule() {
        let first = Rasterizer::triangle([(0, 0), (128, 0), (0, 128)]);
        let second = Rasterizer::triangle([(128, 0), (128, 128), (0, 128)]);
        let first = first.positions.into_iter().collect::<BTreeSet<_>>();
        let second = second.positions.into_iter().collect::<BTreeSet<_>>();
        assert!(first.is_disjoint(&second));
        assert_eq!(first.len() + second.len(), 4);
    }
}
