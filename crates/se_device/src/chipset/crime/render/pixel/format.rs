//! Pixel format conversion used by transfer and fragment paths.

use super::command::PixelFormat;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rgba8 {
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
    pub(crate) a: u8,
}

pub(crate) fn decode(format: PixelFormat, bytes: &[u8]) -> Rgba8 {
    match (format, bytes) {
        (PixelFormat::ColorIndex(8), [index]) => gray(*index),
        (PixelFormat::ColorIndex(16 | 32), bytes) => {
            let index =
                u16::from_be_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]]) & 0x0fff;
            gray(((u32::from(index) * 255 + 2047) / 4095) as u8)
        }
        (PixelFormat::Rgb(8) | PixelFormat::Rgba(8) | PixelFormat::Abgr(8), [value]) => Rgba8 {
            r: expand(*value >> 5, 3),
            g: expand((*value >> 2) & 7, 3),
            b: expand(*value & 3, 2),
            a: 255,
        },
        (PixelFormat::Rgb(16) | PixelFormat::Rgba(16), [high, low]) => {
            let value = u16::from_be_bytes([*high, *low]);
            Rgba8 {
                r: expand(((value >> 10) & 31) as u8, 5),
                g: expand(((value >> 5) & 31) as u8, 5),
                b: expand((value & 31) as u8, 5),
                a: if value & 0x8000 != 0 { 255 } else { 0 },
            }
        }
        (PixelFormat::Abgr(16), [high, low]) => {
            let value = u16::from_be_bytes([*high, *low]);
            Rgba8 {
                r: expand((value & 31) as u8, 5),
                g: expand(((value >> 5) & 31) as u8, 5),
                b: expand(((value >> 10) & 31) as u8, 5),
                a: if value & 0x8000 != 0 { 255 } else { 0 },
            }
        }
        (PixelFormat::Rgb(32) | PixelFormat::Rgba(32), [r, g, b, a]) => Rgba8 {
            r: *r,
            g: *g,
            b: *b,
            a: if matches!(format, PixelFormat::Rgb(_)) {
                255
            } else {
                *a
            },
        },
        (PixelFormat::Abgr(32), [a, b, g, r]) => Rgba8 {
            r: *r,
            g: *g,
            b: *b,
            a: *a,
        },
        (PixelFormat::YCrCb(32), [y, cr, cb, _]) => ycrcb(*y, *cr, *cb),
        _ => Rgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
    }
}

pub(crate) fn encode(format: PixelFormat, color: Rgba8) -> Vec<u8> {
    match format {
        PixelFormat::ColorIndex(8) => vec![luminance(color)],
        PixelFormat::ColorIndex(16) => {
            let value = (u16::from(luminance(color)) * 4095 + 127) / 255;
            value.to_be_bytes().to_vec()
        }
        PixelFormat::ColorIndex(32) => {
            let value = (u32::from(luminance(color)) * 4095 + 127) / 255;
            value.to_be_bytes().to_vec()
        }
        PixelFormat::Rgb(8) | PixelFormat::Rgba(8) | PixelFormat::Abgr(8) => {
            vec![(color.r & 0xe0) | ((color.g >> 3) & 0x1c) | (color.b >> 6)]
        }
        PixelFormat::Rgb(16) | PixelFormat::Rgba(16) => {
            let value = (u16::from(color.a >= 128) << 15)
                | (u16::from(color.r >> 3) << 10)
                | (u16::from(color.g >> 3) << 5)
                | u16::from(color.b >> 3);
            value.to_be_bytes().to_vec()
        }
        PixelFormat::Abgr(16) => {
            let value = (u16::from(color.a >= 128) << 15)
                | (u16::from(color.b >> 3) << 10)
                | (u16::from(color.g >> 3) << 5)
                | u16::from(color.r >> 3);
            value.to_be_bytes().to_vec()
        }
        PixelFormat::Rgb(32) | PixelFormat::Rgba(32) => {
            vec![color.r, color.g, color.b, color.a]
        }
        PixelFormat::Abgr(32) => vec![color.a, color.b, color.g, color.r],
        _ => Vec::new(),
    }
}

const fn gray(value: u8) -> Rgba8 {
    Rgba8 {
        r: value,
        g: value,
        b: value,
        a: 255,
    }
}

const fn expand(value: u8, bits: u8) -> u8 {
    let maximum = (1_u16 << bits) - 1;
    ((value as u16 * 255 + maximum / 2) / maximum) as u8
}

const fn luminance(color: Rgba8) -> u8 {
    ((77_u32 * color.r as u32 + 150_u32 * color.g as u32 + 29_u32 * color.b as u32 + 128) >> 8)
        as u8
}

fn ycrcb(y: u8, cr: u8, cb: u8) -> Rgba8 {
    let c = i32::from(y).saturating_sub(16);
    let d = i32::from(cb) - 128;
    let e = i32::from(cr) - 128;
    Rgba8 {
        r: clamp((298 * c + 409 * e + 128) >> 8),
        g: clamp((298 * c - 100 * d - 208 * e + 128) >> 8),
        b: clamp((298 * c + 516 * d + 128) >> 8),
        a: 255,
    }
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_and_abgr_round_trip_channel_order() {
        let color = Rgba8 {
            r: 0x11,
            g: 0x22,
            b: 0x33,
            a: 0x44,
        };
        assert_eq!(
            encode(PixelFormat::Rgba(32), color),
            [0x11, 0x22, 0x33, 0x44]
        );
        assert_eq!(
            encode(PixelFormat::Abgr(32), color),
            [0x44, 0x33, 0x22, 0x11]
        );
        assert_eq!(
            decode(PixelFormat::Rgba(32), &[0x11, 0x22, 0x33, 0x44]),
            color
        );
        assert_eq!(
            decode(PixelFormat::Abgr(32), &[0x44, 0x33, 0x22, 0x11]),
            color
        );
    }

    #[test]
    fn bt601_limited_range_clamps_black_and_white() {
        assert_eq!(decode(PixelFormat::YCrCb(32), &[16, 128, 128, 0]), gray(0));
        assert_eq!(
            decode(PixelFormat::YCrCb(32), &[235, 128, 128, 0]),
            gray(255)
        );
    }
}
