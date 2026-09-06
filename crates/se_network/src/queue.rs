//! Bounded drop-newest queues at the nondeterministic host boundary.

use std::collections::VecDeque;

pub(crate) const MAX_FRAME_BYTES: usize = 16_384;
const MAX_FRAMES: usize = 256;
const MAX_BYTES: usize = 512 * 1024;

#[derive(Default)]
pub(crate) struct FrameQueue {
    frames: VecDeque<Vec<u8>>,
    bytes: usize,
}

impl FrameQueue {
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > MAX_FRAME_BYTES
            || self.frames.len() >= MAX_FRAMES
            || self.bytes + bytes.len() > MAX_BYTES
        {
            return false;
        }
        self.frames.push_back(bytes.to_vec());
        self.bytes += bytes.len();
        true
    }
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let frame = self.frames.pop_front()?;
        self.bytes -= frame.len();
        Some(frame)
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_frames_and_bytes_independently_and_preserves_order() {
        let mut queue = FrameQueue::default();
        for index in 0..256 {
            assert!(queue.push(&[index as u8]));
        }
        assert!(!queue.push(&[99]));
        for index in 0..256 {
            assert_eq!(queue.pop(), Some(vec![index as u8]));
        }
        for _ in 0..32 {
            assert!(queue.push(&[0; MAX_FRAME_BYTES]));
        }
        assert!(!queue.push(&[1]));
        queue.pop();
        assert!(queue.push(&[1]));
    }
}
