//! Frontend-visible output produced by an emulated machine.

use std::sync::Arc;

use crate::serial::SerialPort;

/// Bytes occupied by one pixel of a video frame.
const BYTES_PER_PIXEL: u32 = 4;

/// One complete picture produced by an emulated graphics board.
///
/// Pixels are RGBA8888, tightly packed, and ordered from the top row down and
/// from left to right within a row. Alpha is always fully opaque: the guest
/// picture never blends with the host.
///
/// A published frame is immutable. Cloning shares the pixels rather than
/// copying them, so delivering a frame across threads or into the frontend
/// costs one reference count.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoFrame {
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
}

impl VideoFrame {
    /// Creates a frame from shared pixels.
    ///
    /// Returns [`None`] unless both dimensions are nonzero and the pixel
    /// length is exactly `width * height * 4`.
    #[must_use]
    pub fn new(width: u32, height: u32, pixels: Arc<Vec<u8>>) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let expected = width
            .checked_mul(height)
            .and_then(|count| count.checked_mul(BYTES_PER_PIXEL))
            .and_then(|bytes| usize::try_from(bytes).ok())?;
        if pixels.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Returns the frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns the frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns the RGBA8888 pixels.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// What a machine currently drives onto its display.
///
/// Each value describes the complete picture to present, so a frontend never
/// has to combine one update with an earlier one. In particular an active
/// signal without a frame means black, not "keep showing the last picture".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VideoOutput {
    /// The machine has no graphics board installed.
    NoGraphicsBoard,
    /// A board is installed but drives no valid video signal.
    NoSignal,
    /// A valid signal is present, carrying a frame when one is complete.
    Active {
        /// The picture to display, or black while none has been produced.
        frame: Option<VideoFrame>,
    },
}

/// Output accumulated during one machine time advancement.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct MachineOutput {
    serial_a: Vec<u8>,
    serial_b: Vec<u8>,
    ethernet: Vec<Vec<u8>>,
    video: Option<VideoOutput>,
}

impl MachineOutput {
    /// Returns the bytes emitted by one external serial port.
    #[must_use]
    pub fn serial(&self, port: SerialPort) -> &[u8] {
        match port {
            SerialPort::A => &self.serial_a,
            SerialPort::B => &self.serial_b,
        }
    }

    /// Returns the display update, if the machine produced one.
    ///
    /// [`None`] means the display is unchanged and the frontend keeps showing
    /// whatever it already presents.
    #[must_use]
    pub const fn video(&self) -> Option<&VideoOutput> {
        self.video.as_ref()
    }

    /// Reports whether the machine produced no frontend-visible output.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.serial_a.is_empty()
            && self.serial_b.is_empty()
            && self.ethernet.is_empty()
            && self.video.is_none()
    }

    /// Takes completed Ethernet frames before frontend output is dispatched.
    pub fn take_ethernet_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.ethernet)
    }

    pub(crate) fn push_ethernet(&mut self, frame: Vec<u8>) {
        self.ethernet.push(frame);
    }

    pub(crate) fn push_serial(&mut self, port: SerialPort, value: u8) {
        match port {
            SerialPort::A => self.serial_a.push(value),
            SerialPort::B => self.serial_b.push(value),
        }
    }

    /// Records the display state to present, replacing any earlier update.
    ///
    /// Only the newest state matters, so a machine that changes its display
    /// several times within one advancement delivers one update.
    pub(crate) fn publish_video(&mut self, output: VideoOutput) {
        self.video = Some(output);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::serial::SerialPort;

    use super::{MachineOutput, VideoFrame, VideoOutput};

    /// Builds a frame of the requested size filled with one byte value.
    fn frame(width: u32, height: u32, fill: u8) -> Option<VideoFrame> {
        let count = (width * height * 4) as usize;
        VideoFrame::new(width, height, Arc::new(vec![fill; count]))
    }

    #[test]
    fn serial_ports_keep_independent_byte_order() {
        let mut output = MachineOutput::default();
        output.push_serial(SerialPort::B, 3);
        output.push_serial(SerialPort::A, 1);
        output.push_serial(SerialPort::A, 2);

        assert_eq!(output.serial(SerialPort::A), [1, 2]);
        assert_eq!(output.serial(SerialPort::B), [3]);
        assert!(!output.is_empty());
    }

    #[test]
    fn a_frame_requires_pixels_matching_its_dimensions() {
        assert!(frame(4, 2, 0).is_some());
        assert!(VideoFrame::new(4, 2, Arc::new(vec![0; 31])).is_none());
        assert!(VideoFrame::new(4, 2, Arc::new(vec![0; 33])).is_none());
    }

    #[test]
    fn a_frame_rejects_empty_dimensions() {
        assert!(VideoFrame::new(0, 2, Arc::new(Vec::new())).is_none());
        assert!(VideoFrame::new(4, 0, Arc::new(Vec::new())).is_none());
    }

    #[test]
    fn a_frame_rejects_dimensions_that_overflow_its_pixel_count() {
        assert!(VideoFrame::new(u32::MAX, u32::MAX, Arc::new(Vec::new())).is_none());
    }

    #[test]
    fn cloning_a_frame_shares_the_pixels() {
        let original = frame(8, 8, 0xa5).unwrap();

        let copy = original.clone();

        assert_eq!(copy.width(), 8);
        assert_eq!(copy.height(), 8);
        assert!(std::ptr::eq(original.pixels(), copy.pixels()));
    }

    #[test]
    fn an_empty_output_carries_no_display_update() {
        let output = MachineOutput::default();

        assert!(output.is_empty());
        assert_eq!(output.video(), None);
    }

    #[test]
    fn publishing_a_display_state_makes_the_output_non_empty() {
        let mut output = MachineOutput::default();

        output.publish_video(VideoOutput::NoSignal);

        assert!(!output.is_empty());
        assert_eq!(output.video(), Some(&VideoOutput::NoSignal));
    }

    #[test]
    fn only_the_newest_display_state_is_delivered() {
        let mut output = MachineOutput::default();

        output.publish_video(VideoOutput::NoSignal);
        output.publish_video(VideoOutput::Active {
            frame: frame(2, 2, 1),
        });

        assert_eq!(
            output.video(),
            Some(&VideoOutput::Active {
                frame: frame(2, 2, 1)
            })
        );
    }

    #[test]
    fn an_active_signal_without_a_frame_is_distinct_from_no_signal() {
        let mut blank = MachineOutput::default();
        let mut absent = MachineOutput::default();

        blank.publish_video(VideoOutput::Active { frame: None });
        absent.publish_video(VideoOutput::NoSignal);

        assert_ne!(blank.video(), absent.video());
        assert_ne!(blank.video(), Some(&VideoOutput::NoGraphicsBoard));
    }
}
