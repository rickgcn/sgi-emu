//! Frontend-visible output produced by an emulated machine.

use std::sync::Arc;

use crate::endpoint::{EndpointKey, EndpointKind};

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
    /// A board is installed but drives no valid video signal.
    NoSignal,
    /// A valid signal is present, carrying a frame when one is complete.
    Active {
        /// The picture to display, or black while none has been produced.
        frame: Option<VideoFrame>,
    },
}

/// Output accumulated across machine time advancements until the runtime drains it.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct MachineOutput {
    entries: Vec<(EndpointKey, EndpointOutput)>,
}

/// One strongly typed output stream or current state.
#[derive(Debug, Eq, PartialEq)]
pub enum EndpointOutput {
    /// Ordered serial bytes from one endpoint.
    Serial(Vec<u8>),
    /// Ordered Ethernet frames from one endpoint.
    Ethernet(Vec<Vec<u8>>),
    /// The latest complete video state for one endpoint.
    Video(VideoOutput),
}

impl EndpointOutput {
    /// Returns the endpoint payload family produced by this output.
    #[must_use]
    pub const fn kind(&self) -> EndpointKind {
        match self {
            Self::Serial(_) => EndpointKind::Serial,
            Self::Ethernet(_) => EndpointKind::Ethernet,
            Self::Video(_) => EndpointKind::Video,
        }
    }
}

impl MachineOutput {
    /// Returns outputs in deterministic machine emission order.
    #[must_use]
    pub fn entries(&self) -> &[(EndpointKey, EndpointOutput)] {
        &self.entries
    }

    /// Takes all outputs in deterministic machine emission order.
    pub fn into_entries(self) -> Vec<(EndpointKey, EndpointOutput)> {
        self.entries
    }

    /// Finds output for one exact endpoint identity.
    #[must_use]
    pub fn get(&self, key: &EndpointKey) -> Option<&EndpointOutput> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, output)| output)
    }

    /// Returns emitted bytes for one serial endpoint.
    #[must_use]
    pub fn serial(&self, key: &EndpointKey) -> &[u8] {
        match self.get(key) {
            Some(EndpointOutput::Serial(bytes)) => bytes,
            _ => &[],
        }
    }

    /// Returns the display update, if the machine produced one.
    ///
    /// [`None`] means the display is unchanged and the frontend keeps showing
    /// whatever it already presents.
    #[must_use]
    pub fn video(&self, key: &EndpointKey) -> Option<&VideoOutput> {
        match self.get(key) {
            Some(EndpointOutput::Video(video)) => Some(video),
            _ => None,
        }
    }

    /// Reports whether the machine produced no frontend-visible output.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Takes completed Ethernet frames before frontend output is dispatched.
    #[cfg(test)]
    pub(crate) fn take_ethernet_frames(&mut self) -> Vec<Vec<u8>> {
        self.take_ethernet_outputs()
            .into_iter()
            .flat_map(|(_, frames)| frames)
            .collect()
    }

    /// Takes endpoint-keyed Ethernet frames before frontend output is dispatched.
    pub fn take_ethernet_outputs(&mut self) -> Vec<(EndpointKey, Vec<Vec<u8>>)> {
        let mut outputs = Vec::new();
        let mut index = 0;
        while index < self.entries.len() {
            if matches!(self.entries[index].1, EndpointOutput::Ethernet(_)) {
                let (key, EndpointOutput::Ethernet(frames)) = self.entries.remove(index) else {
                    unreachable!()
                };
                outputs.push((key, frames));
            } else {
                index += 1;
            }
        }
        outputs
    }

    pub(crate) fn push_ethernet(&mut self, key: EndpointKey, frame: Vec<u8>) {
        match self
            .entries
            .iter_mut()
            .find(|(candidate, _)| candidate == &key)
        {
            Some((_, EndpointOutput::Ethernet(frames))) => frames.push(frame),
            None => self
                .entries
                .push((key, EndpointOutput::Ethernet(vec![frame]))),
            Some(_) => unreachable!("one endpoint cannot publish different output kinds"),
        }
    }

    pub(crate) fn push_serial(&mut self, key: EndpointKey, value: u8) {
        match self
            .entries
            .iter_mut()
            .find(|(candidate, _)| candidate == &key)
        {
            Some((_, EndpointOutput::Serial(bytes))) => bytes.push(value),
            None => self
                .entries
                .push((key, EndpointOutput::Serial(vec![value]))),
            Some(_) => unreachable!("one endpoint cannot publish different output kinds"),
        }
    }

    /// Records the display state to present, replacing any earlier update.
    ///
    /// Only the newest state matters, so a machine that changes its display
    /// several times within one advancement delivers one update.
    pub(crate) fn publish_video(&mut self, key: EndpointKey, output: VideoOutput) {
        match self
            .entries
            .iter_mut()
            .find(|(candidate, _)| candidate == &key)
        {
            Some((_, EndpointOutput::Video(video))) => *video = output,
            None => self.entries.push((key, EndpointOutput::Video(output))),
            Some(_) => unreachable!("one endpoint cannot publish different output kinds"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::endpoint::EndpointKey;

    use super::{MachineOutput, VideoFrame, VideoOutput};

    /// Builds a frame of the requested size filled with one byte value.
    fn frame(width: u32, height: u32, fill: u8) -> Option<VideoFrame> {
        let count = (width * height * 4) as usize;
        VideoFrame::new(width, height, Arc::new(vec![fill; count]))
    }

    #[test]
    fn serial_ports_keep_independent_byte_order() {
        let mut output = MachineOutput::default();
        let a = EndpointKey::new("serial.external.a");
        let b = EndpointKey::new("serial.external.b");
        output.push_serial(b.clone(), 3);
        output.push_serial(a.clone(), 1);
        output.push_serial(a.clone(), 2);

        assert_eq!(output.serial(&a), [1, 2]);
        assert_eq!(output.serial(&b), [3]);
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
        assert_eq!(output.video(&EndpointKey::new("video.0")), None);
    }

    #[test]
    fn publishing_a_display_state_makes_the_output_non_empty() {
        let mut output = MachineOutput::default();

        let key = EndpointKey::new("video.0");
        output.publish_video(key.clone(), VideoOutput::NoSignal);

        assert!(!output.is_empty());
        assert_eq!(output.video(&key), Some(&VideoOutput::NoSignal));
    }

    #[test]
    fn only_the_newest_display_state_is_delivered() {
        let mut output = MachineOutput::default();

        let key = EndpointKey::new("video.0");
        output.publish_video(key.clone(), VideoOutput::NoSignal);
        output.publish_video(
            key.clone(),
            VideoOutput::Active {
                frame: frame(2, 2, 1),
            },
        );

        assert_eq!(
            output.video(&key),
            Some(&VideoOutput::Active {
                frame: frame(2, 2, 1)
            })
        );
    }

    #[test]
    fn an_active_signal_without_a_frame_is_distinct_from_no_signal() {
        let mut blank = MachineOutput::default();
        let mut absent = MachineOutput::default();

        let key = EndpointKey::new("video.0");
        blank.publish_video(key.clone(), VideoOutput::Active { frame: None });
        absent.publish_video(key.clone(), VideoOutput::NoSignal);

        assert_ne!(blank.video(&key), absent.video(&key));
        assert_ne!(blank.video(&key), None);
    }
}
