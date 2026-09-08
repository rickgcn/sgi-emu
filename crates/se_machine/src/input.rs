//! Frontend-neutral inputs accepted by emulated machines.

use std::fmt;

use se_device::sgi_keyboard::SgiKey;
use se_device::sgi_mouse::SgiMouseButton;
use serde::{Deserialize, Serialize};

use crate::serial::SerialPort;

const MAX_ETHERNET_FRAME_BYTES: usize = 16_384;

/// One input submitted at a deterministic machine boundary.
///
/// Keyboard and mouse button variants contain validated physical identifiers.
/// Byte-oriented frontends can construct them without depending on device
/// types by using [`Self::sgi_keyboard`] and [`Self::sgi_mouse_button`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MachineInput {
    /// One byte arriving at an external serial port.
    SerialByte {
        /// External serial port.
        port: SerialPort,
        /// Received byte.
        value: u8,
    },
    /// One Ethernet frame arriving before MAC filtering.
    EthernetFrame {
        /// Frame bytes with a bounded serialized allocation.
        #[serde(deserialize_with = "deserialize_ethernet_frame")]
        bytes: Vec<u8>,
    },
    /// One physical SGI keyboard key state.
    SgiKeyboard {
        /// Physical keyboard key.
        key: SgiKey,
        /// Whether the key is pressed.
        pressed: bool,
    },
    /// Relative SGI mouse motion in guest coordinates.
    SgiMouseMotion {
        /// Horizontal displacement, positive to the right.
        delta_x: i32,
        /// Vertical displacement, positive upward.
        delta_y: i32,
    },
    /// One physical SGI mouse button state.
    SgiMouseButton {
        /// Physical mouse button.
        button: SgiMouseButton,
        /// Whether the button is pressed.
        pressed: bool,
    },
}

impl MachineInput {
    /// Creates a validated physical SGI keyboard transition from a protocol
    /// key code.
    #[must_use]
    pub fn sgi_keyboard(code: u8, pressed: bool) -> Option<Self> {
        let key = SgiKey::try_from(code).ok()?;
        Some(Self::SgiKeyboard { key, pressed })
    }

    /// Creates a validated physical SGI mouse button transition from a
    /// frontend-neutral button code.
    ///
    /// Button codes are zero for left, one for middle, and two for right.
    #[must_use]
    pub fn sgi_mouse_button(code: u8, pressed: bool) -> Option<Self> {
        let button = match code {
            0 => SgiMouseButton::Left,
            1 => SgiMouseButton::Middle,
            2 => SgiMouseButton::Right,
            _ => return None,
        };
        Some(Self::SgiMouseButton { button, pressed })
    }
}

/// Bounds Ethernet allocation before accepting a serialized sequence length.
fn deserialize_ethernet_frame<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    struct FrameVisitor;

    impl<'de> serde::de::Visitor<'de> for FrameVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an Ethernet frame within its allocation bound")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Vec<u8>, A::Error> {
            if sequence
                .size_hint()
                .is_some_and(|length| length > MAX_ETHERNET_FRAME_BYTES)
            {
                return Err(serde::de::Error::custom(
                    "Ethernet frame exceeds its allocation bound",
                ));
            }
            let mut bytes = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
            while let Some(byte) = sequence.next_element()? {
                if bytes.len() == MAX_ETHERNET_FRAME_BYTES {
                    return Err(serde::de::Error::custom(
                        "Ethernet frame exceeds its allocation bound",
                    ));
                }
                bytes.push(byte);
            }
            Ok(bytes)
        }
    }

    deserializer.deserialize_seq(FrameVisitor)
}

#[cfg(test)]
mod tests {
    use se_device::sgi_keyboard::SgiKey;

    use super::{MAX_ETHERNET_FRAME_BYTES, MachineInput};

    #[test]
    fn keyboard_constructor_accepts_exactly_the_device_key_codes() {
        for code in u8::MIN..=u8::MAX {
            assert_eq!(
                MachineInput::sgi_keyboard(code, true).is_some(),
                SgiKey::try_from(code).is_ok(),
                "unexpected validation result for key code {code}"
            );
        }
    }

    #[test]
    fn mouse_button_constructor_accepts_exactly_three_buttons() {
        for code in u8::MIN..=u8::MAX {
            assert_eq!(
                MachineInput::sgi_mouse_button(code, true).is_some(),
                code <= 2,
                "unexpected validation result for mouse button code {code}"
            );
        }
    }

    #[test]
    fn serialized_ethernet_input_rejects_an_oversize_allocation() {
        let input = MachineInput::EthernetFrame {
            bytes: vec![0; MAX_ETHERNET_FRAME_BYTES + 1],
        };
        let encoded = bincode::serde::encode_to_vec(&input, bincode::config::standard()).unwrap();

        assert!(
            bincode::serde::decode_from_slice::<MachineInput, _>(
                &encoded,
                bincode::config::standard()
            )
            .is_err()
        );
    }
}
