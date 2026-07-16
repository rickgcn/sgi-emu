//! CRIME framebuffer layout within one 256-bit memory word.

/// Number of bytes in one CRIME framebuffer memory word.
pub(crate) const WORD_BYTES: usize = 32;

const SUBWORD_BYTES: usize = 4;
const SUBWORDS_PER_WORD: usize = WORD_BYTES / SUBWORD_BYTES;

/// Failure while translating a logical framebuffer lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FramebufferLayoutError {
    /// The logical or physical lane is outside one memory word.
    LaneOutOfRange,
    /// The requested pixel size is not used by a CRIME framebuffer.
    UnsupportedPixelSize,
    /// The logical pixel is not aligned to its encoded size.
    UnalignedPixel,
    /// The encoded pixel would cross a memory-word boundary.
    PixelCrossesWord,
}

/// Maps a logical framebuffer byte to its physical memory lane.
pub(crate) const fn physical_lane(logical_lane: usize) -> Option<usize> {
    if logical_lane >= WORD_BYTES {
        return None;
    }
    let logical_subword = logical_lane / SUBWORD_BYTES;
    let byte_in_subword = logical_lane % SUBWORD_BYTES;
    Some((SUBWORDS_PER_WORD - 1 - logical_subword) * SUBWORD_BYTES + byte_in_subword)
}

/// Maps a physical memory lane back to its logical framebuffer byte.
#[cfg(test)]
pub(crate) const fn logical_lane(lane: usize) -> Option<usize> {
    physical_lane(lane)
}

/// Returns the first physical lane occupied by one logical pixel.
pub(crate) fn physical_pixel_lane(
    logical_lane: usize,
    bytes_per_pixel: usize,
) -> Result<usize, FramebufferLayoutError> {
    if !matches!(bytes_per_pixel, 1 | 2 | 4) {
        return Err(FramebufferLayoutError::UnsupportedPixelSize);
    }
    if logical_lane >= WORD_BYTES {
        return Err(FramebufferLayoutError::LaneOutOfRange);
    }
    if !logical_lane.is_multiple_of(bytes_per_pixel) {
        return Err(FramebufferLayoutError::UnalignedPixel);
    }
    let logical_end = logical_lane
        .checked_add(bytes_per_pixel)
        .ok_or(FramebufferLayoutError::PixelCrossesWord)?;
    if logical_end > WORD_BYTES {
        return Err(FramebufferLayoutError::PixelCrossesWord);
    }
    let physical_start =
        physical_lane(logical_lane).ok_or(FramebufferLayoutError::LaneOutOfRange)?;
    let physical_end =
        physical_lane(logical_end - 1).ok_or(FramebufferLayoutError::LaneOutOfRange)?;
    if physical_end + 1 != physical_start + bytes_per_pixel {
        return Err(FramebufferLayoutError::PixelCrossesWord);
    }
    Ok(physical_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_and_physical_lanes_are_bijective() {
        for logical in 0..WORD_BYTES {
            let physical = physical_lane(logical).unwrap();
            assert_eq!(logical_lane(physical), Some(logical));
        }
        assert_eq!(physical_lane(WORD_BYTES), None);
        assert_eq!(logical_lane(WORD_BYTES), None);
    }

    #[test]
    fn subwords_reverse_while_bytes_remain_big_endian() {
        assert_eq!(physical_lane(0), Some(28));
        assert_eq!(physical_lane(3), Some(31));
        assert_eq!(physical_lane(4), Some(24));
        assert_eq!(physical_lane(28), Some(0));
        assert_eq!(physical_lane(31), Some(3));
    }

    #[test]
    fn framebuffer_pixel_sizes_map_to_contiguous_physical_lanes() {
        for bytes_per_pixel in [1, 2, 4] {
            for logical in (0..WORD_BYTES).step_by(bytes_per_pixel) {
                let physical = physical_pixel_lane(logical, bytes_per_pixel).unwrap();
                assert!(physical + bytes_per_pixel <= WORD_BYTES);
                for byte in 0..bytes_per_pixel {
                    assert_eq!(physical_lane(logical + byte), Some(physical + byte));
                }
            }
        }
        assert_eq!(
            physical_pixel_lane(1, 2),
            Err(FramebufferLayoutError::UnalignedPixel)
        );
        assert_eq!(
            physical_pixel_lane(0, 3),
            Err(FramebufferLayoutError::UnsupportedPixelSize)
        );
    }
}
