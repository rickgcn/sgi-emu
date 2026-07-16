//! Ordered CRIME fragment pipeline arithmetic.

use super::super::{compare, logic_operation};
use super::command::{DecodedPixelCommand, PixelFormat};
use super::format::{Rgba8, decode, encode};

const BAYER_4X4: [[i16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct FragmentResult {
    pub(crate) color: Option<Vec<u8>>,
    pub(crate) stencil_depth: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct TextureTap {
    pub(crate) address: u32,
    pub(crate) weight: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct TextureSamplePlan {
    pub(crate) texel_bytes: u8,
    pub(crate) taps: Vec<TextureTap>,
    pub(crate) border_weight: u32,
}

pub(crate) fn run(
    command: &DecodedPixelCommand,
    x: u16,
    y: u16,
    incoming: Option<Rgba8>,
    destination_bytes: &[u8],
    texture_sample: Option<Rgba8>,
    stencil_depth_bytes: Option<&[u8]>,
) -> FragmentResult {
    let mut color = incoming.unwrap_or_else(|| shade(command, x, y));
    if command.features.texture() {
        let texel = texture_sample.unwrap_or_else(|| texture_border(command));
        color = apply_texture(command, color, texel);
    }
    if command.features.fog() {
        color = apply_fog(command, color, x, y);
    }
    if command.features.coverage() {
        color.a = apply_coverage(command, color.a, x, y);
    }
    if command.features.alpha_test() {
        let alpha = command.snapshot.register32(0x198);
        if !compare(((alpha >> 8) & 7) as u8, u32::from(color.a), alpha & 0xff) {
            return FragmentResult {
                color: None,
                stencil_depth: None,
            };
        }
    }

    let (depth_passes, stencil_depth) = depth_stencil(command, x, y, stencil_depth_bytes);
    if !depth_passes {
        return FragmentResult {
            color: None,
            stencil_depth,
        };
    }

    let destination = decode(command.destination.format, destination_bytes);
    if command.features.blend() {
        color = blend(command, color, destination);
    }
    if command.features.dither() {
        color = dither(command.destination.format, color, x, y);
    }
    let source_encoded = encode(command.destination.format, color);
    let color = apply_logic_and_masks(command, &source_encoded, destination_bytes);
    FragmentResult {
        color: Some(color),
        stencil_depth,
    }
}

fn shade(command: &DecodedPixelCommand, x: u16, y: u16) -> Rgba8 {
    if !command.features.smooth_shade()
        || matches!(
            command.primitive_kind,
            super::command::PixelPrimitiveKind::Point
                | super::command::PixelPrimitiveKind::Rectangle
        )
    {
        return decode(command.destination.format, &command.pixel_bytes());
    }
    let dx = i64::from(x) - i64::from(command.x0);
    let dy = i64::from(y) - i64::from(command.y0);
    let component = |start, slope_x, slope_y| {
        let value = i64::from(command.snapshot.register32(start) as i32)
            + dx * i64::from(command.snapshot.register32(slope_x) as i32)
            + dy * i64::from(command.snapshot.register32(slope_y) as i32);
        (value >> 12).clamp(0, 255) as u8
    };
    Rgba8 {
        r: component(0x0e0, 0x0f0, 0x0f8),
        g: component(0x0e4, 0x0f4, 0x0fc),
        b: component(0x0e8, 0x100, 0x108),
        a: component(0x0ec, 0x104, 0x10c),
    }
}

pub(crate) fn texture_plan(command: &DecodedPixelCommand, x: u16, y: u16) -> TextureSamplePlan {
    let mode = command.snapshot.register32(0x110);
    let texel_bytes = match (mode >> 18) & 3 {
        1 => 2,
        2 => 4,
        _ => unreachable!("invalid texel depth was rejected"),
    };
    let format = command.snapshot.register64(0x118);
    let width = u32::from(((format >> 16) & 0xffff) as u16) + 1;
    let height = u32::from((format & 0xffff) as u16) + 1;
    let base_level = (mode >> 14) & 0x0f;
    let max_level = (mode >> 10) & 0x0f;
    let Some((s, t)) = texture_coordinates(command, x, y) else {
        return TextureSamplePlan {
            texel_bytes,
            taps: Vec::new(),
            border_weight: 65_536,
        };
    };
    let lod = texture_lod(command, x, y, width, height);
    let magnifying = lod <= 0;
    let filter = if magnifying {
        (mode >> 6) & 1
    } else {
        (mode >> 7) & 7
    };
    let clamped_lod = (i64::from(base_level) << 16)
        .saturating_add(lod.max(0))
        .clamp(i64::from(base_level) << 16, i64::from(max_level) << 16);
    let mut plan = TextureSamplePlan {
        texel_bytes,
        taps: Vec::new(),
        border_weight: 0,
    };
    match filter {
        0 => append_texture_level(
            &mut plan, command, s, t, width, height, base_level, 65_536, false,
        ),
        1 => append_texture_level(
            &mut plan, command, s, t, width, height, base_level, 65_536, true,
        ),
        2 | 3 => {
            let level = ((clamped_lod + 0x8000) >> 16) as u32;
            append_texture_level(
                &mut plan,
                command,
                s,
                t,
                width,
                height,
                level,
                65_536,
                filter == 3,
            );
        }
        4 | 5 => {
            let lower = (clamped_lod >> 16) as u32;
            let upper = (lower + 1).min(max_level);
            let upper_weight = (clamped_lod & 0xffff) as u32;
            append_texture_level(
                &mut plan,
                command,
                s,
                t,
                width,
                height,
                lower,
                65_536 - upper_weight,
                filter == 5,
            );
            append_texture_level(
                &mut plan,
                command,
                s,
                t,
                width,
                height,
                upper,
                upper_weight,
                filter == 5,
            );
        }
        _ => unreachable!("invalid texture filter was rejected"),
    }
    plan
}

pub(crate) fn resolve_texture(
    command: &DecodedPixelCommand,
    plan: &TextureSamplePlan,
    texels: &[Vec<u8>],
) -> Rgba8 {
    let border = texture_border(command);
    let mut channels = [
        u64::from(border.r) * u64::from(plan.border_weight),
        u64::from(border.g) * u64::from(plan.border_weight),
        u64::from(border.b) * u64::from(plan.border_weight),
        u64::from(border.a) * u64::from(plan.border_weight),
    ];
    let mut total = u64::from(plan.border_weight);
    for (tap, bytes) in plan.taps.iter().zip(texels) {
        let texel = decode_texture(command, bytes);
        let values = [texel.r, texel.g, texel.b, texel.a];
        for index in 0..4 {
            channels[index] += u64::from(values[index]) * u64::from(tap.weight);
        }
        total += u64::from(tap.weight);
    }
    if total == 0 {
        return border;
    }
    let channel = |index: usize| ((channels[index] + total / 2) / total).min(255_u64) as u8;
    Rgba8 {
        r: channel(0),
        g: channel(1),
        b: channel(2),
        a: channel(3),
    }
}

fn texture_border(command: &DecodedPixelCommand) -> Rgba8 {
    let [r, g, b, a] = command.snapshot.register32(0x160).to_be_bytes();
    Rgba8 { r, g, b, a }
}

fn texture_coordinates(command: &DecodedPixelCommand, x: u16, y: u16) -> Option<(i64, i64)> {
    let dx = i128::from(x) - i128::from(command.x0);
    let dy = i128::from(y) - i128::from(command.y0);
    let interpolate64 = |start, slope_x, slope_y| {
        i128::from(command.snapshot.register64(start) as i64)
            + dx * i128::from(command.snapshot.register64(slope_x) as i64)
            + dy * i128::from(command.snapshot.register64(slope_y) as i64)
    };
    let q = i128::from(command.snapshot.register32(0x130) as i32)
        + dx * i128::from(command.snapshot.register32(0x158) as i32)
        + dy * i128::from(command.snapshot.register32(0x15c) as i32);
    if q == 0 {
        return None;
    }
    let divide = |value: i128| ((value << 32) / q).clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    Some((
        divide(interpolate64(0x120, 0x138, 0x140)),
        divide(interpolate64(0x128, 0x148, 0x150)),
    ))
}

fn texture_lod(command: &DecodedPixelCommand, x: u16, y: u16, width: u32, height: u32) -> i64 {
    let Some(current) = texture_coordinates(command, x, y) else {
        return 0;
    };
    let x_next = x
        .checked_add(1)
        .and_then(|next| texture_coordinates(command, next, y))
        .unwrap_or(current);
    let y_next = y
        .checked_add(1)
        .and_then(|next| texture_coordinates(command, x, next))
        .unwrap_or(current);
    let scaled_delta =
        |left: i64, right: i64, size: u32| i128::from(left.abs_diff(right)) * i128::from(size);
    let rho = scaled_delta(current.0, x_next.0, width)
        .max(scaled_delta(current.1, x_next.1, height))
        .max(scaled_delta(current.0, y_next.0, width))
        .max(scaled_delta(current.1, y_next.1, height));
    let unit = 1_i128 << 32;
    if rho <= unit {
        return 0;
    }
    let floor = (127 - rho.leading_zeros() as i64 - 32).max(0);
    let base = unit << floor;
    let fraction = (((rho - base) << 16) / base).clamp(0, 0xffff) as i64;
    (floor << 16) | fraction
}

#[allow(clippy::too_many_arguments)]
fn append_texture_level(
    plan: &mut TextureSamplePlan,
    command: &DecodedPixelCommand,
    s: i64,
    t: i64,
    base_width: u32,
    base_height: u32,
    level: u32,
    level_weight: u32,
    linear: bool,
) {
    if level_weight == 0 {
        return;
    }
    let mode = command.snapshot.register32(0x110);
    let base_level = (mode >> 14) & 0x0f;
    let relative_level = level.saturating_sub(base_level).min(31);
    let width = (base_width >> relative_level).max(1);
    let height = (base_height >> relative_level).max(1);
    let wrap_s = (mode >> 4) & 3;
    let wrap_t = (mode >> 2) & 3;
    let mut u = i128::from(s) * i128::from(width) - (1_i128 << 31);
    let mut v = i128::from(t) * i128::from(height) - (1_i128 << 31);
    u = clamp_texture_coordinate(u, width, wrap_s);
    v = clamp_texture_coordinate(v, height, wrap_t);
    if linear {
        let u0 = u.div_euclid(1_i128 << 32);
        let v0 = v.div_euclid(1_i128 << 32);
        let uf = (u.rem_euclid(1_i128 << 32) >> 16) as u32;
        let vf = (v.rem_euclid(1_i128 << 32) >> 16) as u32;
        for (sample_v, weight_v) in [(v0, 65_536 - vf), (v0 + 1, vf)] {
            for (sample_u, weight_u) in [(u0, 65_536 - uf), (u0 + 1, uf)] {
                let area_weight =
                    ((u64::from(weight_u) * u64::from(weight_v) + 32_768) >> 16) as u32;
                let weight =
                    ((u64::from(area_weight) * u64::from(level_weight) + 32_768) >> 16) as u32;
                append_texture_tap(
                    plan, command, sample_u, sample_v, width, height, level, weight,
                );
            }
        }
    } else {
        let sample_u = (u + (1_i128 << 31)).div_euclid(1_i128 << 32);
        let sample_v = (v + (1_i128 << 31)).div_euclid(1_i128 << 32);
        append_texture_tap(
            plan,
            command,
            sample_u,
            sample_v,
            width,
            height,
            level,
            level_weight,
        );
    }
}

fn clamp_texture_coordinate(coordinate: i128, size: u32, wrap: u32) -> i128 {
    let maximum = i128::from(size.saturating_sub(1)) << 32;
    match wrap {
        0 => coordinate.clamp(-(1_i128 << 31), (i128::from(size) << 32) - (1_i128 << 31)),
        2 => coordinate.clamp(0, maximum),
        1 | 3 => coordinate,
        _ => unreachable!("wrap encoding is two bits"),
    }
}

#[allow(clippy::too_many_arguments)]
fn append_texture_tap(
    plan: &mut TextureSamplePlan,
    command: &DecodedPixelCommand,
    u: i128,
    v: i128,
    width: u32,
    height: u32,
    level: u32,
    weight: u32,
) {
    if weight == 0 {
        return;
    }
    let mode = command.snapshot.register32(0x110);
    let Some(u) = wrap_texture_index(u, width, (mode >> 4) & 3) else {
        plan.border_weight = plan.border_weight.saturating_add(weight);
        return;
    };
    let Some(v) = wrap_texture_index(v, height, (mode >> 2) & 3) else {
        plan.border_weight = plan.border_weight.saturating_add(weight);
        return;
    };
    let base_level = (mode >> 14) & 0x0f;
    let mut level_y = 0_u32;
    for previous in base_level..level {
        let shift = previous.saturating_sub(base_level).min(31);
        level_y = level_y.saturating_add(
            (u32::from((command.snapshot.register64(0x118) & 0xffff) as u16) + 1)
                .checked_shr(shift)
                .unwrap_or(0)
                .max(1),
        );
    }
    let y_byte = level_y
        .saturating_add(v)
        .saturating_mul(u32::from(plan.texel_bytes));
    plan.taps.push(TextureTap {
        address: (y_byte & 0xffff) << 16 | (u & 0xffff),
        weight,
    });
}

fn wrap_texture_index(index: i128, size: u32, wrap: u32) -> Option<u32> {
    let size = i128::from(size);
    match wrap {
        1 => Some(index.rem_euclid(size) as u32),
        0 | 2 | 3 if (0..size).contains(&index) => Some(index as u32),
        0 | 2 | 3 => None,
        _ => unreachable!("wrap encoding is two bits"),
    }
}

fn decode_texture(command: &DecodedPixelCommand, bytes: &[u8]) -> Rgba8 {
    let mode = command.snapshot.register32(0x110);
    let depth = if bytes.len() == 2 { 16 } else { 32 };
    match (mode >> 20) & 0x0f {
        1 => decode(PixelFormat::Rgb(depth), bytes),
        2 => decode(PixelFormat::Rgba(depth), bytes),
        4 => {
            let value = u16::from_be_bytes([bytes[0], bytes[1]]);
            Rgba8 {
                r: ((value >> 12) as u8 & 0x0f) * 17,
                g: ((value >> 8) as u8 & 0x0f) * 17,
                b: ((value >> 4) as u8 & 0x0f) * 17,
                a: (value as u8 & 0x0f) * 17,
            }
        }
        5 => Rgba8 {
            r: 255,
            g: 255,
            b: 255,
            a: bytes[bytes.len() - 1],
        },
        6 => {
            let value = bytes[0];
            Rgba8 {
                r: value,
                g: value,
                b: value,
                a: value,
            }
        }
        7 => {
            let value = bytes[0];
            Rgba8 {
                r: value,
                g: value,
                b: value,
                a: 255,
            }
        }
        8 => {
            let (luminance, alpha) = if bytes.len() == 2 {
                (bytes[0], bytes[1])
            } else {
                (bytes[0], bytes[2])
            };
            Rgba8 {
                r: luminance,
                g: luminance,
                b: luminance,
                a: alpha,
            }
        }
        _ => unreachable!("invalid texel type was rejected"),
    }
}

fn apply_texture(command: &DecodedPixelCommand, fragment: Rgba8, texel: Rgba8) -> Rgba8 {
    let mode = command.snapshot.register32(0x110);
    let environment = decode(
        PixelFormat::Rgba(32),
        &command.snapshot.register32(0x168).to_be_bytes(),
    );
    match mode & 3 {
        0 => Rgba8 {
            r: multiply(fragment.r, texel.r),
            g: multiply(fragment.g, texel.g),
            b: multiply(fragment.b, texel.b),
            a: multiply(fragment.a, texel.a),
        },
        1 => Rgba8 {
            r: lerp(fragment.r, texel.r, texel.a),
            g: lerp(fragment.g, texel.g, texel.a),
            b: lerp(fragment.b, texel.b, texel.a),
            a: fragment.a,
        },
        2 => Rgba8 {
            r: multiply(fragment.r, lerp(255, environment.r, texel.r)),
            g: multiply(fragment.g, lerp(255, environment.g, texel.g)),
            b: multiply(fragment.b, lerp(255, environment.b, texel.b)),
            a: multiply(fragment.a, texel.a),
        },
        _ => texel,
    }
}

fn apply_fog(command: &DecodedPixelCommand, color: Rgba8, x: u16, y: u16) -> Rgba8 {
    let dx = i64::from(x) - i64::from(command.x0);
    let dy = i64::from(y) - i64::from(command.y0);
    let factor = i64::from(command.snapshot.register32(0x178) as i32)
        + dx * i64::from(command.snapshot.register32(0x180) as i32)
        + dy * i64::from(command.snapshot.register32(0x188) as i32);
    let factor = ((factor * 255 + 0x800) >> 12).clamp(0, 255) as u8;
    let [fog_r, fog_g, fog_b, _] = command.snapshot.register32(0x170).to_be_bytes();
    Rgba8 {
        r: lerp(fog_r, color.r, factor),
        g: lerp(fog_g, color.g, factor),
        b: lerp(fog_b, color.b, factor),
        a: color.a,
    }
}

fn apply_coverage(command: &DecodedPixelCommand, alpha: u8, x: u16, y: u16) -> u8 {
    let raw = command.snapshot.register32(0x194);
    let start = ((raw >> 16) & 0x7f) as u8;
    let end = (raw & 0x7f) as u8;
    let programmed = if (x, y) == (command.x0, command.y0) && start != 0 {
        (u16::from(start) * 255 + 63) / 127
    } else if (x, y) == (command.x1, command.y1) && end != 0 {
        (u16::from(end) * 255 + 63) / 127
    } else {
        255
    }
    .min(255) as u8;
    let sampled = if command.features.line_antialias()
        && matches!(
            command.primitive_kind,
            super::command::PixelPrimitiveKind::Line
        ) {
        line_sample_coverage(command, x, y)
    } else {
        255
    };
    multiply(alpha, multiply(programmed, sampled))
}

fn line_sample_coverage(command: &DecodedPixelCommand, x: u16, y: u16) -> u8 {
    let (x0, y0) = command.vertices_subpixel[0];
    let (x1, y1) = command.vertices_subpixel[1];
    let dx = i128::from(x1) - i128::from(x0);
    let dy = i128::from(y1) - i128::from(y0);
    let length_squared = dx * dx + dy * dy;
    let radius = i128::from(command.line_width.max(32));
    let mut covered = 0_u16;
    for sample_y in 0..8_i128 {
        for sample_x in 0..8_i128 {
            let px = i128::from(x) * 64 + 4 + sample_x * 8 - i128::from(x0);
            let py = i128::from(y) * 64 + 4 + sample_y * 8 - i128::from(y0);
            let inside = if length_squared == 0 {
                px * px + py * py <= radius * radius
            } else {
                let projection = px * dx + py * dy;
                if projection <= 0 {
                    px * px + py * py <= radius * radius
                } else if projection >= length_squared {
                    let end_x = px - dx;
                    let end_y = py - dy;
                    end_x * end_x + end_y * end_y <= radius * radius
                } else {
                    (px * dy - py * dx).pow(2) <= radius * radius * length_squared
                }
            };
            covered += u16::from(inside);
        }
    }
    ((covered * 255 + 32) / 64) as u8
}

fn depth_stencil(
    command: &DecodedPixelCommand,
    x: u16,
    y: u16,
    bytes: Option<&[u8]>,
) -> (bool, Option<Vec<u8>>) {
    if !command.features.depth_test()
        && !command.features.depth_mask()
        && !command.features.stencil_test()
    {
        return (true, None);
    }
    let mut packed = [0_u8; 4];
    if let Some(bytes) = bytes {
        packed[..bytes.len().min(4)].copy_from_slice(&bytes[..bytes.len().min(4)]);
    }
    let original = packed;
    let mut stencil = packed[0];
    let destination_depth = u32::from_be_bytes([0, packed[1], packed[2], packed[3]]);
    let stencil_mode = command.snapshot.register32(0x1e0);
    let stencil_mask = command.snapshot.register32(0x1e8) as u8;
    let stencil_reference = (stencil_mode >> 24) as u8;
    let stencil_passes = !command.features.stencil_test()
        || compare(
            ((stencil_mode >> 12) & 7) as u8,
            u32::from(stencil_reference & stencil_mask),
            u32::from(stencil & stencil_mask),
        );
    if !stencil_passes {
        stencil = stencil_operation((stencil_mode >> 8) & 0x0f, stencil, stencil_reference);
        packed[0] = (original[0] & !stencil_mask) | (stencil & stencil_mask);
        return (false, (packed != original).then(|| packed.to_vec()));
    }

    let dx = i128::from(x) - i128::from(command.x0);
    let dy = i128::from(y) - i128::from(command.y0);
    let depth_value = i128::from(command.snapshot.register64(0x1c8) as i64)
        + dx * i128::from(command.snapshot.register64(0x1d0) as i64)
        + dy * i128::from(command.snapshot.register64(0x1d8) as i64);
    let incoming_depth = (depth_value >> 12).clamp(0, 0x00ff_ffff) as u32;
    let depth_mode = command.snapshot.register32(0x1c0);
    let depth_passes = !command.features.depth_test()
        || compare(
            ((depth_mode >> 25) & 7) as u8,
            incoming_depth,
            destination_depth,
        );
    let stencil_operation = if depth_passes {
        stencil_mode & 0x0f
    } else {
        (stencil_mode >> 4) & 0x0f
    };
    if command.features.stencil_test() {
        stencil = self::stencil_operation(stencil_operation, stencil, stencil_reference);
        packed[0] = (original[0] & !stencil_mask) | (stencil & stencil_mask);
    }
    if depth_passes && command.features.depth_mask() {
        let bytes = incoming_depth.to_be_bytes();
        packed[1..].copy_from_slice(&bytes[1..]);
    }
    (depth_passes, (packed != original).then(|| packed.to_vec()))
}

fn stencil_operation(operation: u32, value: u8, reference: u8) -> u8 {
    match operation {
        0 => value,
        1 => 0,
        2 => reference,
        3 => value.saturating_add(1),
        4 => value.saturating_sub(1),
        5 => !value,
        _ => value,
    }
}

fn blend(command: &DecodedPixelCommand, source: Rgba8, destination: Rgba8) -> Rgba8 {
    let function = command.snapshot.register32(0x1a8);
    let constant = decode(
        PixelFormat::Rgba(32),
        &command.snapshot.register32(0x1a0).to_be_bytes(),
    );
    let source_factor = factor((function >> 4) & 0x0f, source, destination, constant, true);
    let destination_factor = factor(function & 0x0f, source, destination, constant, false);
    let source_channels = [source.r, source.g, source.b, source.a];
    let destination_channels = [destination.r, destination.g, destination.b, destination.a];
    let mut output = [0_u8; 4];
    for index in 0..4 {
        let source_value = scale(source_channels[index], source_factor[index]);
        let destination_value = scale(destination_channels[index], destination_factor[index]);
        output[index] = match (function >> 8) & 0x0f {
            0 => source_value.saturating_add(destination_value),
            1 => source_channels[index].min(destination_channels[index]),
            2 => source_channels[index].max(destination_channels[index]),
            3 => source_value.saturating_sub(destination_value),
            4 => destination_value.saturating_sub(source_value),
            _ => source_value,
        };
    }
    Rgba8 {
        r: output[0],
        g: output[1],
        b: output[2],
        a: output[3],
    }
}

fn factor(
    selector: u32,
    source: Rgba8,
    destination: Rgba8,
    constant: Rgba8,
    source_factor: bool,
) -> [u8; 4] {
    let source_color = [source.r, source.g, source.b, source.a];
    let destination_color = [destination.r, destination.g, destination.b, destination.a];
    let constant_color = [constant.r, constant.g, constant.b, constant.a];
    match selector {
        0 => [0; 4],
        1 => [255; 4],
        2 if source_factor => destination_color,
        2 => source_color,
        3 if source_factor => destination_color.map(|value| 255 - value),
        3 => source_color.map(|value| 255 - value),
        4 => [source.a; 4],
        5 => [255 - source.a; 4],
        6 => [destination.a; 4],
        7 => [255 - destination.a; 4],
        8 => constant_color,
        9 => constant_color.map(|value| 255 - value),
        10 => [constant.a; 4],
        11 => [255 - constant.a; 4],
        12 if source_factor => {
            let value = source.a.min(255 - destination.a);
            [value, value, value, 255]
        }
        _ => [0; 4],
    }
}

fn dither(format: PixelFormat, mut color: Rgba8, x: u16, y: u16) -> Rgba8 {
    let bits = match format {
        PixelFormat::Rgb(8) | PixelFormat::Rgba(8) | PixelFormat::Abgr(8) => 3,
        PixelFormat::Rgb(16) | PixelFormat::Rgba(16) | PixelFormat::Abgr(16) => 5,
        _ => return color,
    };
    let threshold = BAYER_4X4[usize::from(y & 3)][usize::from(x & 3)] - 8;
    let quantum = 1_i16 << (8 - bits);
    let adjust = threshold * quantum / 16;
    color.r = (i16::from(color.r) + adjust).clamp(0, 255) as u8;
    color.g = (i16::from(color.g) + adjust).clamp(0, 255) as u8;
    color.b = (i16::from(color.b) + adjust).clamp(0, 255) as u8;
    color
}

fn apply_logic_and_masks(
    command: &DecodedPixelCommand,
    source: &[u8],
    destination: &[u8],
) -> Vec<u8> {
    let mut source_word = [0_u8; 4];
    let mut destination_word = [0_u8; 4];
    let offset = 4 - source.len();
    source_word[offset..].copy_from_slice(source);
    destination_word[offset..].copy_from_slice(destination);
    let source_value = u32::from_be_bytes(source_word);
    let destination_value = u32::from_be_bytes(destination_word);
    let logic_value = if command.features.logic() {
        logic_operation(
            (command.snapshot.logic_operation() & 0x0f) as u8,
            source_value,
            destination_value,
        )
    } else {
        source_value
    };
    let plane_mask = if command.features.color_mask() {
        command.snapshot.color_mask()
    } else {
        u32::MAX
    };
    let masked = (logic_value & plane_mask) | (destination_value & !plane_mask);
    let mut bytes = masked.to_be_bytes()[offset..].to_vec();
    let byte_mask = command.features.color_byte_mask();
    for (index, byte) in bytes.iter_mut().enumerate() {
        let component = source.len() - 1 - index;
        if byte_mask & (1 << component) == 0 {
            *byte = destination[index];
        }
    }
    bytes
}

const fn multiply(left: u8, right: u8) -> u8 {
    ((left as u16 * right as u16 + 127) / 255) as u8
}

const fn lerp(left: u8, right: u8, factor: u8) -> u8 {
    let inverse = 255 - factor;
    ((left as u16 * inverse as u16 + right as u16 * factor as u16 + 127) / 255) as u8
}

const fn scale(value: u8, factor: u8) -> u8 {
    multiply(value, factor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chipset::crime::render::{PIXEL_PIPE_BASE, PIXEL_PIPE_NULL};

    use super::super::command::{PixelCommandValidation, PixelRegisters};

    fn texture_command(min_filter: u32, wrap: u32) -> DecodedPixelCommand {
        texture_command_with_coordinate(min_filter, wrap, 1 << 10)
    }

    fn texture_command_with_coordinate(
        min_filter: u32,
        wrap: u32,
        coordinate: i64,
    ) -> DecodedPixelCommand {
        let mut registers = PixelRegisters::new();
        registers.write(PIXEL_PIPE_BASE + 0x018, 4, (1_u32 << 15).into());
        registers.write(
            PIXEL_PIPE_BASE + 0x110,
            4,
            (2_u32 << 20 | 2 << 18 | 2 << 10 | min_filter << 7 | wrap | 3).into(),
        );
        registers.write(PIXEL_PIPE_BASE + 0x118, 8, (3_u64 << 16) | 3);
        registers.write(PIXEL_PIPE_BASE + 0x120, 8, coordinate as u64);
        registers.write(PIXEL_PIPE_BASE + 0x128, 8, coordinate as u64);
        registers.write(PIXEL_PIPE_BASE + 0x130, 4, 1_u64 << 12);
        registers.write(PIXEL_PIPE_BASE + 0x138, 8, 3_u64 << 10);
        PixelCommandValidation::decode(registers.command_snapshot(PIXEL_PIPE_NULL)).decoded
    }

    #[test]
    fn all_stencil_operations_are_defined() {
        assert_eq!(stencil_operation(0, 7, 9), 7);
        assert_eq!(stencil_operation(1, 7, 9), 0);
        assert_eq!(stencil_operation(2, 7, 9), 9);
        assert_eq!(stencil_operation(3, 255, 9), 255);
        assert_eq!(stencil_operation(4, 0, 9), 0);
        assert_eq!(stencil_operation(5, 0x55, 9), 0xaa);
    }

    #[test]
    fn bayer_phase_is_stable() {
        let color = Rgba8 {
            r: 128,
            g: 128,
            b: 128,
            a: 255,
        };
        assert_ne!(
            dither(PixelFormat::Rgb(8), color, 0, 0),
            dither(PixelFormat::Rgb(8), color, 1, 0)
        );
    }

    #[test]
    fn all_six_texture_filters_build_the_expected_tap_shape() {
        for (filter, expected_taps) in [(0, 1), (1, 4), (2, 1), (3, 4), (4, 2), (5, 5)] {
            let command = texture_command(filter, 1 << 4 | 1 << 2);
            let plan = texture_plan(&command, 0, 0);
            assert_eq!(plan.taps.len(), expected_taps, "filter {filter}");
            assert_eq!(
                plan.taps.iter().map(|tap| tap.weight).sum::<u32>() + plan.border_weight,
                65_536,
                "filter {filter}"
            );
        }
    }

    #[test]
    fn clamp_to_border_contributes_border_weight() {
        let command = texture_command_with_coordinate(0, 3 << 4 | 3 << 2, -4096);
        let plan = texture_plan(&command, 0, 0);
        assert!(plan.taps.is_empty());
        assert_eq!(plan.border_weight, 65_536);
    }
}
