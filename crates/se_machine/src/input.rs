//! Typed frontend-neutral inputs accepted by emulated machines.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::endpoint::{EndpointKey, EndpointKind};

const MAX_ETHERNET_FRAME_BYTES: usize = 16_384;

/// One physical or logical key supported by the frontend.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum KeyboardKey {
    /// An uppercase ASCII letter, A through Z.
    Letter(u8),
    /// A main-row digit, zero through nine.
    Digit(u8),
    /// A keypad digit, zero through nine.
    KeypadDigit(u8),
    /// A function key, F1 through F12.
    Function(u8),
    /// A named key that is not a letter, digit, or function key.
    Named(KeyboardNamedKey),
}

/// Named frontend keyboard keys.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum KeyboardNamedKey {
    LeftControl,
    RightControl,
    LeftShift,
    RightShift,
    LeftAlt,
    RightAlt,
    CapsLock,
    Escape,
    Tab,
    Enter,
    Backspace,
    Delete,
    Space,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    PrintScreen,
    ScrollLock,
    Pause,
    NumLock,
    Semicolon,
    Comma,
    Minus,
    LeftBracket,
    RightBracket,
    Apostrophe,
    Period,
    Slash,
    Equal,
    Grave,
    Backslash,
    KeypadPeriod,
    KeypadMinus,
    KeypadPlus,
    KeypadSlash,
    KeypadAsterisk,
    KeypadEnter,
}

/// One physical pointer button.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
}

/// Strongly typed input data carried to one endpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MachineInputPayload {
    SerialByte(u8),
    Keyboard {
        key: KeyboardKey,
        pressed: bool,
    },
    PointerMotion {
        delta_x: i32,
        delta_y: i32,
    },
    PointerButton {
        button: PointerButton,
        pressed: bool,
    },
    EthernetFrame {
        #[serde(deserialize_with = "deserialize_ethernet_frame")]
        bytes: Vec<u8>,
    },
}

impl MachineInputPayload {
    /// Returns the endpoint payload family required by this input.
    #[must_use]
    pub const fn kind(&self) -> EndpointKind {
        match self {
            Self::SerialByte(_) => EndpointKind::Serial,
            Self::Keyboard { .. } => EndpointKind::Keyboard,
            Self::PointerMotion { .. } | Self::PointerButton { .. } => EndpointKind::Pointer,
            Self::EthernetFrame { .. } => EndpointKind::Ethernet,
        }
    }
}

/// One input submitted at a deterministic machine boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MachineInput {
    endpoint: EndpointKey,
    payload: MachineInputPayload,
}

impl MachineInput {
    /// Constructs one typed input for an opaque machine endpoint.
    #[must_use]
    pub fn new(endpoint: EndpointKey, payload: MachineInputPayload) -> Self {
        Self { endpoint, payload }
    }

    /// Returns the destination endpoint.
    #[must_use]
    pub const fn endpoint(&self) -> &EndpointKey {
        &self.endpoint
    }

    /// Returns the typed input data.
    #[must_use]
    pub const fn payload(&self) -> &MachineInputPayload {
        &self.payload
    }
}

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
    use super::{MAX_ETHERNET_FRAME_BYTES, MachineInput, MachineInputPayload};
    use crate::endpoint::EndpointKey;

    #[test]
    fn serialized_ethernet_input_rejects_an_oversize_allocation() {
        let input = MachineInput::new(
            EndpointKey::new("ethernet.0"),
            MachineInputPayload::EthernetFrame {
                bytes: vec![0; MAX_ETHERNET_FRAME_BYTES + 1],
            },
        );
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
