//! SGI Indigo keyboard serial protocol.

use std::collections::VecDeque;

use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};
use serde::{Deserialize, Serialize};

const BAUD: u128 = 600;
const FRAME_BITS: u128 = 11;
const REPEAT_DELAY_ATTOSECONDS: u128 = 13 * ATTOSECONDS_PER_SECOND / 20;
const REPEAT_RATE: u128 = 28;

/// A physical key on the fixed US SGI Indigo keyboard.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[repr(u8)]
pub enum SgiKey {
    /// Left Control.
    LeftControl = 2,
    /// Caps Lock.
    CapsLock = 3,
    /// Right Shift.
    RightShift = 4,
    /// Left Shift.
    LeftShift = 5,
    /// Escape.
    Escape = 6,
    /// Main-row digit 1.
    Digit1 = 7,
    /// Tab.
    Tab = 8,
    /// Q.
    KeyQ = 9,
    /// A.
    KeyA = 10,
    /// S.
    KeyS = 11,
    /// Main-row digit 2.
    Digit2 = 13,
    /// Main-row digit 3.
    Digit3 = 14,
    /// W.
    KeyW = 15,
    /// E.
    KeyE = 16,
    /// D.
    KeyD = 17,
    /// F.
    KeyF = 18,
    /// Z.
    KeyZ = 19,
    /// X.
    KeyX = 20,
    /// Main-row digit 4.
    Digit4 = 21,
    /// Main-row digit 5.
    Digit5 = 22,
    /// R.
    KeyR = 23,
    /// T.
    KeyT = 24,
    /// G.
    KeyG = 25,
    /// H.
    KeyH = 26,
    /// C.
    KeyC = 27,
    /// V.
    KeyV = 28,
    /// Main-row digit 6.
    Digit6 = 29,
    /// Main-row digit 7.
    Digit7 = 30,
    /// Y.
    KeyY = 31,
    /// U.
    KeyU = 32,
    /// J.
    KeyJ = 33,
    /// K.
    KeyK = 34,
    /// B.
    KeyB = 35,
    /// N.
    KeyN = 36,
    /// Main-row digit 8.
    Digit8 = 37,
    /// Main-row digit 9.
    Digit9 = 38,
    /// I.
    KeyI = 39,
    /// O.
    KeyO = 40,
    /// L.
    KeyL = 41,
    /// Semicolon.
    Semicolon = 42,
    /// M.
    KeyM = 43,
    /// Comma.
    Comma = 44,
    /// Main-row digit 0.
    Digit0 = 45,
    /// Minus.
    Minus = 46,
    /// P.
    KeyP = 47,
    /// Left bracket.
    LeftBracket = 48,
    /// Apostrophe.
    Apostrophe = 49,
    /// Main Enter.
    Enter = 50,
    /// Period.
    Period = 51,
    /// Slash.
    Slash = 52,
    /// Equal.
    Equal = 53,
    /// Grave accent.
    Grave = 54,
    /// Right bracket.
    RightBracket = 55,
    /// Backslash.
    Backslash = 56,
    /// Keypad 1.
    Keypad1 = 57,
    /// Keypad 0.
    Keypad0 = 58,
    /// Backspace.
    Backspace = 60,
    /// Delete.
    Delete = 61,
    /// Keypad 4.
    Keypad4 = 62,
    /// Keypad 2.
    Keypad2 = 63,
    /// Keypad 3.
    Keypad3 = 64,
    /// Keypad decimal point.
    KeypadPeriod = 65,
    /// Keypad 7.
    Keypad7 = 66,
    /// Keypad 8.
    Keypad8 = 67,
    /// Keypad 5.
    Keypad5 = 68,
    /// Keypad 6.
    Keypad6 = 69,
    /// Left arrow.
    ArrowLeft = 72,
    /// Down arrow.
    ArrowDown = 73,
    /// Keypad 9.
    Keypad9 = 74,
    /// Keypad minus.
    KeypadMinus = 75,
    /// Right arrow.
    ArrowRight = 79,
    /// Up arrow.
    ArrowUp = 80,
    /// Keypad Enter.
    KeypadEnter = 81,
    /// Space.
    Space = 82,
    /// Left Alt.
    LeftAlt = 83,
    /// Right Alt.
    RightAlt = 84,
    /// Right Control.
    RightControl = 85,
    /// Function key F1.
    F1 = 86,
    /// Function key F2.
    F2 = 87,
    /// Function key F3.
    F3 = 88,
    /// Function key F4.
    F4 = 89,
    /// Function key F5.
    F5 = 90,
    /// Function key F6.
    F6 = 91,
    /// Function key F7.
    F7 = 92,
    /// Function key F8.
    F8 = 93,
    /// Function key F9.
    F9 = 94,
    /// Function key F10.
    F10 = 95,
    /// Function key F11.
    F11 = 96,
    /// Function key F12.
    F12 = 97,
    /// Print Screen.
    PrintScreen = 98,
    /// Scroll Lock.
    ScrollLock = 99,
    /// Pause.
    Pause = 100,
    /// Insert.
    Insert = 101,
    /// Home.
    Home = 102,
    /// Page Up.
    PageUp = 103,
    /// End.
    End = 104,
    /// Page Down.
    PageDown = 105,
    /// Num Lock.
    NumLock = 106,
    /// Keypad slash.
    KeypadSlash = 107,
    /// Keypad asterisk.
    KeypadAsterisk = 108,
    /// Keypad plus.
    KeypadPlus = 109,
}

impl TryFrom<u8> for SgiKey {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            2 => Ok(Self::LeftControl),
            3 => Ok(Self::CapsLock),
            4 => Ok(Self::RightShift),
            5 => Ok(Self::LeftShift),
            6 => Ok(Self::Escape),
            7 => Ok(Self::Digit1),
            8 => Ok(Self::Tab),
            9 => Ok(Self::KeyQ),
            10 => Ok(Self::KeyA),
            11 => Ok(Self::KeyS),
            13 => Ok(Self::Digit2),
            14 => Ok(Self::Digit3),
            15 => Ok(Self::KeyW),
            16 => Ok(Self::KeyE),
            17 => Ok(Self::KeyD),
            18 => Ok(Self::KeyF),
            19 => Ok(Self::KeyZ),
            20 => Ok(Self::KeyX),
            21 => Ok(Self::Digit4),
            22 => Ok(Self::Digit5),
            23 => Ok(Self::KeyR),
            24 => Ok(Self::KeyT),
            25 => Ok(Self::KeyG),
            26 => Ok(Self::KeyH),
            27 => Ok(Self::KeyC),
            28 => Ok(Self::KeyV),
            29 => Ok(Self::Digit6),
            30 => Ok(Self::Digit7),
            31 => Ok(Self::KeyY),
            32 => Ok(Self::KeyU),
            33 => Ok(Self::KeyJ),
            34 => Ok(Self::KeyK),
            35 => Ok(Self::KeyB),
            36 => Ok(Self::KeyN),
            37 => Ok(Self::Digit8),
            38 => Ok(Self::Digit9),
            39 => Ok(Self::KeyI),
            40 => Ok(Self::KeyO),
            41 => Ok(Self::KeyL),
            42 => Ok(Self::Semicolon),
            43 => Ok(Self::KeyM),
            44 => Ok(Self::Comma),
            45 => Ok(Self::Digit0),
            46 => Ok(Self::Minus),
            47 => Ok(Self::KeyP),
            48 => Ok(Self::LeftBracket),
            49 => Ok(Self::Apostrophe),
            50 => Ok(Self::Enter),
            51 => Ok(Self::Period),
            52 => Ok(Self::Slash),
            53 => Ok(Self::Equal),
            54 => Ok(Self::Grave),
            55 => Ok(Self::RightBracket),
            56 => Ok(Self::Backslash),
            57 => Ok(Self::Keypad1),
            58 => Ok(Self::Keypad0),
            60 => Ok(Self::Backspace),
            61 => Ok(Self::Delete),
            62 => Ok(Self::Keypad4),
            63 => Ok(Self::Keypad2),
            64 => Ok(Self::Keypad3),
            65 => Ok(Self::KeypadPeriod),
            66 => Ok(Self::Keypad7),
            67 => Ok(Self::Keypad8),
            68 => Ok(Self::Keypad5),
            69 => Ok(Self::Keypad6),
            72 => Ok(Self::ArrowLeft),
            73 => Ok(Self::ArrowDown),
            74 => Ok(Self::Keypad9),
            75 => Ok(Self::KeypadMinus),
            79 => Ok(Self::ArrowRight),
            80 => Ok(Self::ArrowUp),
            81 => Ok(Self::KeypadEnter),
            82 => Ok(Self::Space),
            83 => Ok(Self::LeftAlt),
            84 => Ok(Self::RightAlt),
            85 => Ok(Self::RightControl),
            86 => Ok(Self::F1),
            87 => Ok(Self::F2),
            88 => Ok(Self::F3),
            89 => Ok(Self::F4),
            90 => Ok(Self::F5),
            91 => Ok(Self::F6),
            92 => Ok(Self::F7),
            93 => Ok(Self::F8),
            94 => Ok(Self::F9),
            95 => Ok(Self::F10),
            96 => Ok(Self::F11),
            97 => Ok(Self::F12),
            98 => Ok(Self::PrintScreen),
            99 => Ok(Self::ScrollLock),
            100 => Ok(Self::Pause),
            101 => Ok(Self::Insert),
            102 => Ok(Self::Home),
            103 => Ok(Self::PageUp),
            104 => Ok(Self::End),
            105 => Ok(Self::PageDown),
            106 => Ok(Self::NumLock),
            107 => Ok(Self::KeypadSlash),
            108 => Ok(Self::KeypadAsterisk),
            109 => Ok(Self::KeypadPlus),
            _ => Err(()),
        }
    }
}

impl SgiKey {
    const fn repeatable(self) -> bool {
        !matches!(
            self,
            Self::LeftControl
                | Self::RightControl
                | Self::LeftShift
                | Self::RightShift
                | Self::LeftAlt
                | Self::RightAlt
                | Self::CapsLock
                | Self::Escape
                | Self::Tab
                | Self::KeypadEnter
                | Self::PrintScreen
                | Self::ScrollLock
                | Self::Pause
                | Self::Insert
                | Self::Home
                | Self::PageUp
                | Self::End
                | Self::PageDown
                | Self::NumLock
        )
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct ActiveByte {
    value: u8,
    remaining_attoseconds: u128,
}

#[derive(Clone, Deserialize, Serialize)]
struct TimedOutput {
    queued: VecDeque<u8>,
    active: Option<ActiveByte>,
    timing_remainder: u16,
}

impl TimedOutput {
    const fn new() -> Self {
        Self {
            queued: VecDeque::new(),
            active: None,
            timing_remainder: 0,
        }
    }

    fn push(&mut self, value: u8) {
        self.queued.push_back(value);
    }

    fn ensure_active(&mut self) {
        if self.active.is_some() {
            return;
        }
        let Some(value) = self.queued.pop_front() else {
            return;
        };
        let numerator = FRAME_BITS * ATTOSECONDS_PER_SECOND + u128::from(self.timing_remainder);
        self.timing_remainder = u16::try_from(numerator % BAUD)
            .expect("keyboard timing remainder must fit in the baud rate");
        self.active = Some(ActiveByte {
            value,
            remaining_attoseconds: numerator / BAUD,
        });
    }

    fn time_until_event(&self) -> Option<u128> {
        self.active
            .map(|active| active.remaining_attoseconds)
            .or_else(|| {
                (!self.queued.is_empty()).then(|| {
                    (FRAME_BITS * ATTOSECONDS_PER_SECOND + u128::from(self.timing_remainder)) / BAUD
                })
            })
    }

    fn elapse(&mut self, elapsed_attoseconds: u128, mut output: impl FnMut(u8)) {
        self.ensure_active();
        let Some(active) = self.active.as_mut() else {
            return;
        };
        debug_assert!(elapsed_attoseconds <= active.remaining_attoseconds);
        active.remaining_attoseconds -= elapsed_attoseconds;
        if active.remaining_attoseconds == 0 {
            let value = active.value;
            self.active = None;
            output(value);
        }
    }
}

/// The fixed US SGI Indigo keyboard protocol endpoint.
///
/// The model exposes completed serial characters instead of electrical line
/// transitions. Character and repeat phases are accumulated in integer
/// attoseconds with carried rational remainders, so splitting an elapsed
/// duration into different calls does not change observable output.
///
/// If several repeatable keys are held, the most recently pressed one is the
/// sole repeat candidate. Releasing it stops repetition without resuming an
/// older key. This is a deterministic emulator policy for behavior that is
/// not defined by the keyboard protocol documentation.
#[derive(Clone, Deserialize, Serialize)]
pub struct SgiKeyboard {
    pressed: [u64; 2],
    output: TimedOutput,
    repeat_enabled: bool,
    repeat_candidate: Option<SgiKey>,
    repeat_remaining_attoseconds: Option<u128>,
    repeat_timing_remainder: u8,
    click_disabled: bool,
    leds: [bool; 7],
}

impl Default for SgiKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl SgiKeyboard {
    /// Creates a connected keyboard in its reset state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pressed: [0; 2],
            output: TimedOutput::new(),
            repeat_enabled: false,
            repeat_candidate: None,
            repeat_remaining_attoseconds: None,
            repeat_timing_remainder: 0,
            click_disabled: false,
            leds: [false; 7],
        }
    }

    /// Restores the keyboard reset state without emitting a power-on byte.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Applies one physical key state and queues an SGI downcode or upcode for
    /// an actual transition. Duplicate states are ignored.
    pub fn set_key_state(&mut self, key: SgiKey, pressed: bool) {
        let code = key as u8;
        let word = usize::from(code / 64);
        let mask = 1_u64 << (code % 64);
        let was_pressed = self.pressed[word] & mask != 0;
        if was_pressed == pressed {
            return;
        }
        if pressed {
            self.pressed[word] |= mask;
            self.output.push(code);
            if self.repeat_enabled && key.repeatable() {
                self.repeat_candidate = Some(key);
                self.repeat_remaining_attoseconds = Some(REPEAT_DELAY_ATTOSECONDS);
                self.repeat_timing_remainder = 0;
            }
        } else {
            self.pressed[word] &= !mask;
            self.output.push(code | 0x80);
            if self.repeat_candidate == Some(key) {
                self.repeat_candidate = None;
                self.repeat_remaining_attoseconds = None;
                self.repeat_timing_remainder = 0;
            }
        }
    }

    /// Receives one completed command character from the host SCC channel.
    pub fn receive_command(&mut self, command: u8) {
        if command & 1 == 0 {
            self.click_disabled = command & (1 << 3) != 0;
            if command & (1 << 4) != 0 {
                self.output.push(0x6e);
                self.output.push(0x00);
            }
            self.leds[0] = command & (1 << 5) != 0;
            self.leds[1] = command & (1 << 6) != 0;
            let repeat_enabled = command & (1 << 7) != 0;
            if self.repeat_enabled && !repeat_enabled {
                self.repeat_candidate = None;
                self.repeat_remaining_attoseconds = None;
                self.repeat_timing_remainder = 0;
            }
            self.repeat_enabled = repeat_enabled;
        } else {
            if command & (1 << 1) != 0 {
                self.leds[0] = !self.leds[0];
                self.leds[1] = !self.leds[1];
            }
            self.leds[2] = command & (1 << 2) != 0;
            self.leds[3] = command & (1 << 3) != 0;
            self.leds[4] = command & (1 << 4) != 0;
            self.leds[5] = command & (1 << 5) != 0;
            self.leds[6] = command & (1 << 6) != 0;
        }
    }

    /// Advances keyboard protocol time and reports completed characters.
    pub fn advance_time(&mut self, elapsed: VirtualDuration, mut output: impl FnMut(u8)) {
        let mut remaining = elapsed.as_attoseconds();
        loop {
            self.output.ensure_active();
            let next = [
                self.output.time_until_event(),
                self.repeat_remaining_attoseconds,
            ]
            .into_iter()
            .flatten()
            .min();
            let Some(next) = next else {
                return;
            };
            if remaining < next {
                self.output.elapse(remaining, &mut output);
                if let Some(repeat) = self.repeat_remaining_attoseconds.as_mut() {
                    *repeat -= remaining;
                }
                return;
            }

            self.output.elapse(next, &mut output);
            if let Some(repeat) = self.repeat_remaining_attoseconds.as_mut() {
                *repeat -= next;
            }
            remaining -= next;
            if self.repeat_remaining_attoseconds == Some(0) {
                if let Some(key) = self.repeat_candidate {
                    self.output.push(key as u8);
                    let numerator =
                        ATTOSECONDS_PER_SECOND + u128::from(self.repeat_timing_remainder);
                    self.repeat_timing_remainder = u8::try_from(numerator % REPEAT_RATE)
                        .expect("repeat timing remainder must fit in the repeat rate");
                    self.repeat_remaining_attoseconds = Some(numerator / REPEAT_RATE);
                } else {
                    self.repeat_remaining_attoseconds = None;
                }
            }
        }
    }

    /// Returns the virtual duration until the next serial character or repeat
    /// deadline.
    #[must_use]
    pub fn time_until_event(&self) -> Option<VirtualDuration> {
        [
            self.output.time_until_event(),
            self.repeat_remaining_attoseconds,
        ]
        .into_iter()
        .flatten()
        .min()
        .map(VirtualDuration::from_attoseconds)
    }
}

#[cfg(test)]
mod tests {
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use super::{SgiKey, SgiKeyboard};

    const CHARACTER_TIME: u128 = 11 * ATTOSECONDS_PER_SECOND / 600;

    fn advance(keyboard: &mut SgiKeyboard, attoseconds: u128) -> Vec<u8> {
        let mut bytes = Vec::new();
        keyboard.advance_time(VirtualDuration::from_attoseconds(attoseconds), |value| {
            bytes.push(value);
        });
        bytes
    }

    #[test]
    fn accepts_exactly_the_physical_us_keycodes() {
        let keys: Vec<_> = (0..=u8::MAX)
            .filter_map(|value| SgiKey::try_from(value).ok())
            .collect();
        assert_eq!(keys.len(), 101);
        assert_eq!(keys.first().copied(), Some(SgiKey::LeftControl));
        assert_eq!(keys.last().copied(), Some(SgiKey::KeypadPlus));
    }

    #[test]
    fn transitions_emit_downcodes_and_upcodes_once() {
        let mut keyboard = SgiKeyboard::new();
        keyboard.set_key_state(SgiKey::KeyA, true);
        keyboard.set_key_state(SgiKey::KeyA, true);
        keyboard.set_key_state(SgiKey::KeyA, false);
        keyboard.set_key_state(SgiKey::KeyA, false);

        assert_eq!(advance(&mut keyboard, CHARACTER_TIME * 2 + 1), [10, 138]);
    }

    #[test]
    fn configuration_request_reports_us_keyboard() {
        let mut keyboard = SgiKeyboard::new();
        keyboard.receive_command(1 << 4);

        assert_eq!(advance(&mut keyboard, CHARACTER_TIME * 2 + 1), [0x6e, 0x00]);
    }

    #[test]
    fn commands_assign_and_complement_the_documented_control_state() {
        let mut keyboard = SgiKeyboard::new();
        keyboard.receive_command(0xe8);
        assert!(keyboard.click_disabled);
        assert!(keyboard.repeat_enabled);
        assert_eq!(
            keyboard.leds,
            [true, true, false, false, false, false, false]
        );

        keyboard.receive_command(0x7f);
        assert_eq!(keyboard.leds, [false, false, true, true, true, true, true]);
        keyboard.receive_command(0x06);
        assert!(advance(&mut keyboard, CHARACTER_TIME).is_empty());
    }

    #[test]
    fn repeat_starts_after_delay_and_uses_the_latest_repeatable_key() {
        let mut keyboard = SgiKeyboard::new();
        keyboard.receive_command(0x80);
        keyboard.set_key_state(SgiKey::KeyA, true);
        let mut bytes = advance(&mut keyboard, CHARACTER_TIME + 1);
        keyboard.set_key_state(SgiKey::LeftShift, true);
        bytes.extend(advance(
            &mut keyboard,
            13 * ATTOSECONDS_PER_SECOND / 20 - CHARACTER_TIME - 1,
        ));
        bytes.extend(advance(&mut keyboard, CHARACTER_TIME + 1));

        assert_eq!(bytes, [10, 5, 10]);
    }

    #[test]
    fn releasing_latest_repeat_candidate_does_not_resume_an_older_key() {
        let mut keyboard = SgiKeyboard::new();
        keyboard.receive_command(0x80);
        keyboard.set_key_state(SgiKey::KeyA, true);
        keyboard.set_key_state(SgiKey::KeyB, true);
        keyboard.set_key_state(SgiKey::KeyB, false);

        assert_eq!(keyboard.repeat_candidate, None);
        assert_eq!(keyboard.repeat_remaining_attoseconds, None);
        assert_eq!(
            advance(&mut keyboard, ATTOSECONDS_PER_SECOND),
            [10, 35, 163]
        );
    }

    #[test]
    fn repeat_reenable_waits_for_a_new_physical_down_transition() {
        let mut keyboard = SgiKeyboard::new();
        keyboard.receive_command(0x80);
        keyboard.set_key_state(SgiKey::KeyA, true);
        keyboard.receive_command(0x00);
        keyboard.receive_command(0x80);

        assert_eq!(
            advance(&mut keyboard, ATTOSECONDS_PER_SECOND),
            [SgiKey::KeyA as u8]
        );
    }

    #[test]
    fn elapsed_fragmentation_preserves_serial_phase() {
        let mut whole = SgiKeyboard::new();
        whole.set_key_state(SgiKey::KeyA, true);
        whole.set_key_state(SgiKey::KeyA, false);
        let whole_bytes = advance(&mut whole, CHARACTER_TIME * 2 + 1);

        let mut split = SgiKeyboard::new();
        split.set_key_state(SgiKey::KeyA, true);
        split.set_key_state(SgiKey::KeyA, false);
        let mut split_bytes = advance(&mut split, CHARACTER_TIME / 3);
        split_bytes.extend(advance(
            &mut split,
            CHARACTER_TIME * 2 + 1 - CHARACTER_TIME / 3,
        ));

        assert_eq!(split_bytes, whole_bytes);
        assert_eq!(split.time_until_event(), whole.time_until_event());
    }

    #[test]
    fn snapshot_preserves_in_flight_character() {
        let mut keyboard = SgiKeyboard::new();
        keyboard.set_key_state(SgiKey::KeyA, true);
        assert!(advance(&mut keyboard, CHARACTER_TIME / 2).is_empty());
        let bytes = bincode::serde::encode_to_vec(&keyboard, bincode::config::standard()).unwrap();
        let (mut restored, _): (SgiKeyboard, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();

        assert_eq!(
            advance(&mut restored, CHARACTER_TIME - CHARACTER_TIME / 2 + 1),
            [10]
        );
    }
}
