//! SGI Indigo mouse serial protocol.

use std::collections::VecDeque;

use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
use serde::{Deserialize, Serialize};

const BAUD: u128 = 4_800;
const FRAME_BITS: u128 = 10;

/// A button on the three-button SGI Indigo mouse.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SgiMouseButton {
    /// Left button.
    Left,
    /// Middle button.
    Middle,
    /// Right button.
    Right,
}

impl SgiMouseButton {
    const fn mask(self) -> u8 {
        match self {
            Self::Left => 1 << 2,
            Self::Middle => 1 << 1,
            Self::Right => 1,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
enum Segment {
    Motion { buttons: u8, dx: i64, dy: i64 },
    Button { buttons: u8 },
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct Packet {
    bytes: [u8; 5],
    next: u8,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct ActiveByte {
    value: u8,
    remaining_attoseconds: u128,
}

/// The fixed three-button SGI Indigo mouse protocol endpoint.
///
/// Consecutive relative motions under one button state are normalized into
/// checked `i64` segments. A packet is materialized only when its first byte
/// can begin transmission, and its two signed samples per axis are then
/// immutable. Button transitions are ordering barriers and always produce a
/// zero-motion packet. This deterministic normalization is an emulator policy,
/// not a claim about buffering inside the physical mouse.
///
/// Serial character phases carry their rational remainder in virtual time, so
/// output is invariant under different `advance_time` fragmentation.
#[derive(Clone, Deserialize, Serialize)]
pub struct SgiMouse {
    buttons: u8,
    pending: VecDeque<Segment>,
    packet: Option<Packet>,
    active: Option<ActiveByte>,
    timing_remainder: u16,
}

impl Default for SgiMouse {
    fn default() -> Self {
        Self::new()
    }
}

impl SgiMouse {
    /// Creates a connected mouse with all buttons released.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buttons: 0,
            pending: VecDeque::new(),
            packet: None,
            active: None,
            timing_remainder: 0,
        }
    }

    /// Restores the mouse reset state without emitting a packet.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Queues relative motion in SGI coordinates: positive X is right and
    /// positive Y is up. Zero motion is ignored.
    pub fn move_relative(&mut self, delta_x: i32, delta_y: i32) {
        if delta_x == 0 && delta_y == 0 {
            return;
        }
        if let Some(Segment::Motion { buttons, dx, dy }) = self.pending.back_mut()
            && *buttons == self.buttons
            && let (Some(next_dx), Some(next_dy)) = (
                dx.checked_add(i64::from(delta_x)),
                dy.checked_add(i64::from(delta_y)),
            )
        {
            *dx = next_dx;
            *dy = next_dy;
            return;
        }
        self.pending.push_back(Segment::Motion {
            buttons: self.buttons,
            dx: i64::from(delta_x),
            dy: i64::from(delta_y),
        });
    }

    /// Applies one physical button state. Duplicate states are ignored.
    pub fn set_button_state(&mut self, button: SgiMouseButton, pressed: bool) {
        let mask = button.mask();
        let was_pressed = self.buttons & mask != 0;
        if was_pressed == pressed {
            return;
        }
        if pressed {
            self.buttons |= mask;
        } else {
            self.buttons &= !mask;
        }
        self.pending.push_back(Segment::Button {
            buttons: self.buttons,
        });
    }

    /// Advances mouse protocol time and reports completed characters.
    pub fn advance_time(&mut self, elapsed: VirtualDuration, mut output: impl FnMut(u8)) {
        let mut remaining = elapsed.as_attoseconds();
        loop {
            self.ensure_active();
            let Some(active) = self.active.as_mut() else {
                return;
            };
            if remaining < active.remaining_attoseconds {
                active.remaining_attoseconds -= remaining;
                return;
            }
            remaining -= active.remaining_attoseconds;
            let value = active.value;
            self.active = None;
            output(value);
        }
    }

    /// Returns the virtual duration until the next serial character.
    #[must_use]
    pub fn time_until_event(&self) -> Option<VirtualDuration> {
        self.active
            .map(|active| active.remaining_attoseconds)
            .or_else(|| {
                (self.packet.is_some() || !self.pending.is_empty()).then(|| {
                    (FRAME_BITS * ATTOSECONDS_PER_SECOND + u128::from(self.timing_remainder)) / BAUD
                })
            })
            .map(VirtualDuration::from_attoseconds)
    }

    fn ensure_active(&mut self) {
        if self.active.is_some() {
            return;
        }
        if self.packet.is_none() {
            self.materialize_packet();
        }
        let Some(packet) = self.packet.as_mut() else {
            return;
        };
        let value = packet.bytes[usize::from(packet.next)];
        packet.next += 1;
        if packet.next == 5 {
            self.packet = None;
        }
        let numerator = FRAME_BITS * ATTOSECONDS_PER_SECOND + u128::from(self.timing_remainder);
        self.timing_remainder = u16::try_from(numerator % BAUD)
            .expect("mouse timing remainder must fit in the baud rate");
        self.active = Some(ActiveByte {
            value,
            remaining_attoseconds: numerator / BAUD,
        });
    }

    fn materialize_packet(&mut self) {
        let Some(segment) = self.pending.pop_front() else {
            return;
        };
        let (buttons, dx, dy) = match segment {
            Segment::Button { buttons } => (buttons, 0, 0),
            Segment::Motion { buttons, dx, dy } => (buttons, dx, dy),
        };
        let (x1, x2, used_x) = split_axis(dx);
        let (y1, y2, used_y) = split_axis(dy);
        if let Segment::Motion { .. } = segment
            && (dx != used_x || dy != used_y)
        {
            self.pending.push_front(Segment::Motion {
                buttons,
                dx: dx - used_x,
                dy: dy - used_y,
            });
        }
        self.packet = Some(Packet {
            bytes: [
                0x80 | (!buttons & 0x07),
                x1 as u8,
                y1 as u8,
                x2 as u8,
                y2 as u8,
            ],
            next: 0,
        });
    }
}

fn split_axis(value: i64) -> (i8, i8, i64) {
    let first = value.clamp(i64::from(i8::MIN), i64::from(i8::MAX)) as i8;
    let remaining = value - i64::from(first);
    let second = remaining.clamp(i64::from(i8::MIN), i64::from(i8::MAX)) as i8;
    (first, second, i64::from(first) + i64::from(second))
}

#[cfg(test)]
mod tests {
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use super::{SgiMouse, SgiMouseButton};

    const CHARACTER_TIME: u128 = 10 * ATTOSECONDS_PER_SECOND / 4_800;

    fn advance(mouse: &mut SgiMouse, characters: u128) -> Vec<u8> {
        let mut bytes = Vec::new();
        mouse.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_TIME * characters + characters),
            |value| bytes.push(value),
        );
        bytes
    }

    #[test]
    fn encodes_axes_and_active_low_buttons() {
        let mut mouse = SgiMouse::new();
        mouse.set_button_state(SgiMouseButton::Left, true);
        mouse.move_relative(12, -9);

        assert_eq!(
            advance(&mut mouse, 10),
            [0x83, 0, 0, 0, 0, 0x83, 12, 247, 0, 0]
        );
    }

    #[test]
    fn coalesces_motion_and_splits_large_displacements() {
        let mut mouse = SgiMouse::new();
        mouse.move_relative(100, 0);
        mouse.move_relative(200, 0);

        assert_eq!(
            advance(&mut mouse, 10),
            [0x87, 127, 0, 127, 0, 0x87, 46, 0, 0, 0]
        );
    }

    #[test]
    fn button_transitions_are_zero_motion_barriers() {
        let mut mouse = SgiMouse::new();
        mouse.move_relative(1, 2);
        mouse.set_button_state(SgiMouseButton::Right, true);
        mouse.move_relative(3, 4);

        assert_eq!(
            advance(&mut mouse, 15),
            [0x87, 1, 2, 0, 0, 0x86, 0, 0, 0, 0, 0x86, 3, 4, 0, 0]
        );
    }

    #[test]
    fn an_in_flight_packet_is_immutable_across_a_button_transition() {
        let mut mouse = SgiMouse::new();
        mouse.move_relative(1, 2);
        let mut bytes = advance(&mut mouse, 1);
        mouse.set_button_state(SgiMouseButton::Left, true);
        bytes.extend(advance(&mut mouse, 9));

        assert_eq!(bytes, [0x87, 1, 2, 0, 0, 0x83, 0, 0, 0, 0]);
    }

    #[test]
    fn extreme_motion_uses_bounded_signed_samples_without_wrapping() {
        let mut mouse = SgiMouse::new();
        mouse.move_relative(i32::MAX, i32::MIN);

        assert_eq!(advance(&mut mouse, 5), [0x87, 127, 128, 127, 128]);
    }

    #[test]
    fn snapshot_preserves_an_in_flight_packet() {
        let mut mouse = SgiMouse::new();
        mouse.move_relative(12, -9);
        let first = advance(&mut mouse, 2);
        let bytes = bincode::serde::encode_to_vec(&mouse, bincode::config::standard()).unwrap();
        let (mut restored, _): (SgiMouse, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        let mut all = first;
        all.extend(advance(&mut restored, 3));

        assert_eq!(all, [0x87, 12, 247, 0, 0]);
    }
}
