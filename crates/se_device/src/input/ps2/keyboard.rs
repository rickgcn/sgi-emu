use super::*;

const ACK: u8 = 0xfa;
const RESEND: u8 = 0xfe;
const DEFAULT_TYPEMATIC: u8 = 0x2b;

#[derive(Clone, Copy)]
struct ScanCodeEntry {
    key: Ps2KeyPosition,
    number: u8,
    set1: u8,
    set1_extended: bool,
    set2: u8,
    set2_extended: bool,
    set3: u8,
}

macro_rules! key {
    ($key:ident, $number:expr, $set1:expr, $set2:expr, $set3:expr) => {
        ScanCodeEntry {
            key: Ps2KeyPosition::$key,
            number: $number,
            set1: $set1,
            set1_extended: false,
            set2: $set2,
            set2_extended: false,
            set3: $set3,
        }
    };
    ($key:ident, $number:expr, e0 $set1:expr, e0 $set2:expr, $set3:expr) => {
        ScanCodeEntry {
            key: Ps2KeyPosition::$key,
            number: $number,
            set1: $set1,
            set1_extended: true,
            set2: $set2,
            set2_extended: true,
            set3: $set3,
        }
    };
}

const SCAN_CODES: &[ScanCodeEntry] = &[
    key!(Escape, 110, 0x01, 0x76, 0x08),
    key!(F1, 112, 0x3b, 0x05, 0x07),
    key!(F2, 113, 0x3c, 0x06, 0x0f),
    key!(F3, 114, 0x3d, 0x04, 0x17),
    key!(F4, 115, 0x3e, 0x0c, 0x1f),
    key!(F5, 116, 0x3f, 0x03, 0x27),
    key!(F6, 117, 0x40, 0x0b, 0x2f),
    key!(F7, 118, 0x41, 0x83, 0x37),
    key!(F8, 119, 0x42, 0x0a, 0x3f),
    key!(F9, 120, 0x43, 0x01, 0x47),
    key!(F10, 121, 0x44, 0x09, 0x4f),
    key!(F11, 122, 0x57, 0x78, 0x56),
    key!(F12, 123, 0x58, 0x07, 0x5e),
    key!(PrintScreen, 124, 0x37, 0x7c, 0x57),
    key!(ScrollLock, 125, 0x46, 0x7e, 0x5f),
    key!(Pause, 126, 0x45, 0x77, 0x62),
    key!(Grave, 1, 0x29, 0x0e, 0x0e),
    key!(Digit1, 2, 0x02, 0x16, 0x16),
    key!(Digit2, 3, 0x03, 0x1e, 0x1e),
    key!(Digit3, 4, 0x04, 0x26, 0x26),
    key!(Digit4, 5, 0x05, 0x25, 0x25),
    key!(Digit5, 6, 0x06, 0x2e, 0x2e),
    key!(Digit6, 7, 0x07, 0x36, 0x36),
    key!(Digit7, 8, 0x08, 0x3d, 0x3d),
    key!(Digit8, 9, 0x09, 0x3e, 0x3e),
    key!(Digit9, 10, 0x0a, 0x46, 0x46),
    key!(Digit0, 11, 0x0b, 0x45, 0x45),
    key!(Minus, 12, 0x0c, 0x4e, 0x4e),
    key!(Equal, 13, 0x0d, 0x55, 0x55),
    key!(Backspace, 15, 0x0e, 0x66, 0x66),
    key!(Insert, 75, e0 0x52, e0 0x70, 0x67),
    key!(Home, 80, e0 0x47, e0 0x6c, 0x6e),
    key!(PageUp, 85, e0 0x49, e0 0x7d, 0x6f),
    key!(NumLock, 90, 0x45, 0x77, 0x76),
    key!(NumpadDivide, 95, e0 0x35, e0 0x4a, 0x77),
    key!(NumpadMultiply, 100, 0x37, 0x7c, 0x7e),
    key!(NumpadSubtract, 105, 0x4a, 0x7b, 0x84),
    key!(Tab, 16, 0x0f, 0x0d, 0x0d),
    key!(Q, 17, 0x10, 0x15, 0x15),
    key!(W, 18, 0x11, 0x1d, 0x1d),
    key!(E, 19, 0x12, 0x24, 0x24),
    key!(R, 20, 0x13, 0x2d, 0x2d),
    key!(T, 21, 0x14, 0x2c, 0x2c),
    key!(Y, 22, 0x15, 0x35, 0x35),
    key!(U, 23, 0x16, 0x3c, 0x3c),
    key!(I, 24, 0x17, 0x43, 0x43),
    key!(O, 25, 0x18, 0x44, 0x44),
    key!(P, 26, 0x19, 0x4d, 0x4d),
    key!(LeftBracket, 27, 0x1a, 0x54, 0x54),
    key!(RightBracket, 28, 0x1b, 0x5b, 0x5b),
    key!(Backslash, 29, 0x2b, 0x5d, 0x5c),
    key!(IsoHash, 42, 0x2b, 0x5d, 0x53),
    key!(Delete, 76, e0 0x53, e0 0x71, 0x64),
    key!(End, 81, e0 0x4f, e0 0x69, 0x65),
    key!(PageDown, 86, e0 0x51, e0 0x7a, 0x6d),
    key!(Numpad7, 91, 0x47, 0x6c, 0x6c),
    key!(Numpad8, 96, 0x48, 0x75, 0x75),
    key!(Numpad9, 101, 0x49, 0x7d, 0x7d),
    key!(NumpadAdd, 106, 0x4e, 0x79, 0x7c),
    key!(CapsLock, 30, 0x3a, 0x58, 0x14),
    key!(A, 31, 0x1e, 0x1c, 0x1c),
    key!(S, 32, 0x1f, 0x1b, 0x1b),
    key!(D, 33, 0x20, 0x23, 0x23),
    key!(F, 34, 0x21, 0x2b, 0x2b),
    key!(G, 35, 0x22, 0x34, 0x34),
    key!(H, 36, 0x23, 0x33, 0x33),
    key!(J, 37, 0x24, 0x3b, 0x3b),
    key!(K, 38, 0x25, 0x42, 0x42),
    key!(L, 39, 0x26, 0x4b, 0x4b),
    key!(Semicolon, 40, 0x27, 0x4c, 0x4c),
    key!(Apostrophe, 41, 0x28, 0x52, 0x52),
    key!(Enter, 43, 0x1c, 0x5a, 0x5a),
    key!(Numpad4, 92, 0x4b, 0x6b, 0x6b),
    key!(Numpad5, 97, 0x4c, 0x73, 0x73),
    key!(Numpad6, 102, 0x4d, 0x74, 0x74),
    key!(LeftShift, 44, 0x2a, 0x12, 0x12),
    key!(Iso102, 45, 0x56, 0x61, 0x13),
    key!(Z, 46, 0x2c, 0x1a, 0x1a),
    key!(X, 47, 0x2d, 0x22, 0x22),
    key!(C, 48, 0x2e, 0x21, 0x21),
    key!(V, 49, 0x2f, 0x2a, 0x2a),
    key!(B, 50, 0x30, 0x32, 0x32),
    key!(N, 51, 0x31, 0x31, 0x31),
    key!(M, 52, 0x32, 0x3a, 0x3a),
    key!(Comma, 53, 0x33, 0x41, 0x41),
    key!(Period, 54, 0x34, 0x49, 0x49),
    key!(Slash, 55, 0x35, 0x4a, 0x4a),
    key!(RightShift, 57, 0x36, 0x59, 0x59),
    key!(ArrowUp, 83, e0 0x48, e0 0x75, 0x63),
    key!(Numpad1, 93, 0x4f, 0x69, 0x69),
    key!(Numpad2, 98, 0x50, 0x72, 0x72),
    key!(Numpad3, 103, 0x51, 0x7a, 0x7a),
    key!(NumpadEnter, 108, e0 0x1c, e0 0x5a, 0x79),
    key!(LeftControl, 58, 0x1d, 0x14, 0x11),
    key!(LeftAlt, 60, 0x38, 0x11, 0x19),
    key!(Space, 61, 0x39, 0x29, 0x29),
    key!(RightAlt, 62, e0 0x38, e0 0x11, 0x39),
    key!(RightControl, 64, e0 0x1d, e0 0x14, 0x58),
    key!(ArrowLeft, 79, e0 0x4b, e0 0x6b, 0x61),
    key!(ArrowDown, 84, e0 0x50, e0 0x72, 0x60),
    key!(ArrowRight, 89, e0 0x4d, e0 0x74, 0x6a),
    key!(Numpad0, 99, 0x52, 0x70, 0x70),
    key!(NumpadDecimal, 104, 0x53, 0x71, 0x71),
];

impl Ps2Keyboard {
    /// Creates a keyboard attached to one open-drain PS/2 bus.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        wiring: Ps2Wiring,
        timebase_hz: u64,
    ) -> Result<Self, Ps2DeviceBuildError> {
        Ok(Self {
            id,
            name: name.into(),
            link: Ps2DeviceLink::new(id, wiring, timebase_hz)?,
            responses: VecDeque::new(),
            scan_fifo: VecDeque::new(),
            scan_overrun: false,
            pressed: BTreeSet::new(),
            set3_types: BTreeMap::new(),
            command_parameter: KeyboardParameter::None,
            scan_set: 2,
            leds: 0,
            typematic_parameter: DEFAULT_TYPEMATIC,
            typematic_key: None,
            typematic_epoch: 0,
            scanning_enabled: false,
            resume_scanning_after_id: false,
            bat_epoch: 0,
            bat_active: false,
            last_sent: None,
            actions: VecDeque::new(),
        })
    }

    /// Starts the deterministic power-on BAT interval.
    pub fn power_on(&mut self, now: SimTime) {
        self.actions.clear();
        self.link.observe_time(now);
        self.link.reset();
        self.reset_device_defaults(false);
        self.pressed.clear();
        self.responses.clear();
        self.last_sent = None;
        self.bat_epoch = self.bat_epoch.wrapping_add(1);
        self.bat_active = true;
        self.actions.push_back(Ps2KeyboardAction::Schedule {
            delay: milliseconds(self.link.timebase_hz, BAT_MILLISECONDS),
            event: Ps2KeyboardEvent::BatComplete {
                epoch: self.bat_epoch,
            },
        });
    }

    /// Applies one physical host key transition.
    pub fn apply_input(&mut self, input: Ps2KeyboardInput) {
        let changed = if input.pressed {
            self.pressed.insert(input.key)
        } else {
            self.pressed.remove(&input.key)
        };
        if !changed {
            return;
        }
        if input.pressed {
            self.typematic_key = Some(input.key);
            self.typematic_epoch = self.typematic_epoch.wrapping_add(1);
            if self.scanning_enabled && !self.bat_active {
                self.enqueue_key(input.key, true, false);
                if self.key_type(input.key).repeats() && input.key != Ps2KeyPosition::Pause {
                    self.schedule_typematic_delay();
                }
            }
        } else {
            if self.typematic_key == Some(input.key) {
                self.typematic_key = None;
                self.typematic_epoch = self.typematic_epoch.wrapping_add(1);
            }
            if self.scanning_enabled && !self.bat_active && self.key_type(input.key).sends_break() {
                self.enqueue_key(input.key, false, false);
            }
        }
        self.pump_output();
    }

    /// Releases every physical key without synthesizing host text.
    pub fn release_all(&mut self) {
        let keys: Vec<_> = self.pressed.iter().copied().collect();
        for key in keys {
            self.apply_input(Ps2KeyboardInput {
                key,
                pressed: false,
            });
        }
    }

    /// Observes one aggregate line transition from the attached bus.
    pub fn observe_lines(&mut self, delivery: TwoWireLineDelivery) -> Result<(), Ps2KeyboardError> {
        if let Some(signal) = self
            .link
            .observe_lines(delivery)
            .map_err(Ps2KeyboardError::InvalidBus)?
        {
            self.handle_link_signal(signal);
        }
        self.pump_output();
        Ok(())
    }

    /// Handles one scheduled device transition.
    pub fn handle_event(&mut self, now: SimTime, event: Ps2KeyboardEvent) {
        self.link.observe_time(now);
        match event {
            Ps2KeyboardEvent::Link(event) => {
                if let Some(signal) = self.link.handle_event(event) {
                    self.handle_link_signal(signal);
                }
            }
            Ps2KeyboardEvent::BatComplete { epoch }
                if epoch == self.bat_epoch && self.bat_active =>
            {
                self.bat_active = false;
                self.scanning_enabled = true;
                self.responses.push_back(0xaa);
                self.synchronize_pressed_keys();
            }
            Ps2KeyboardEvent::Typematic { epoch }
                if epoch == self.typematic_epoch && self.scanning_enabled =>
            {
                if let Some(key) = self.typematic_key
                    && self.pressed.contains(&key)
                    && self.key_type(key).repeats()
                {
                    if self.link.can_start()
                        && self.responses.is_empty()
                        && !self.link.has_deferred_device_byte()
                        && self.scan_fifo.is_empty()
                    {
                        self.enqueue_key(key, true, true);
                    }
                    self.schedule_typematic_period();
                }
            }
            Ps2KeyboardEvent::BatComplete { .. } | Ps2KeyboardEvent::Typematic { .. } => {}
        }
        self.pump_output();
    }

    /// Polls one pending schedule or line-drive action.
    pub fn poll(&mut self) -> Ps2KeyboardPoll {
        if let Some(action) = self.actions.pop_front() {
            return Ps2KeyboardPoll::Action(action);
        }
        match self.link.poll() {
            Some(LinkAction::Schedule { delay, event }) => {
                Ps2KeyboardPoll::Action(Ps2KeyboardAction::Schedule {
                    delay,
                    event: Ps2KeyboardEvent::Link(event),
                })
            }
            Some(LinkAction::Drive(drive)) => {
                Ps2KeyboardPoll::Action(Ps2KeyboardAction::Drive(drive))
            }
            None => Ps2KeyboardPoll::Idle,
        }
    }

    /// Returns the active scan-code set.
    pub const fn scan_set(&self) -> u8 {
        self.scan_set
    }

    /// Returns the guest-programmed keyboard LED mask.
    pub const fn leds(&self) -> u8 {
        self.leds
    }

    fn handle_link_signal(&mut self, signal: LinkSignal) {
        match signal {
            LinkSignal::HostByte { byte, valid } => {
                if valid {
                    self.handle_command_byte(byte);
                } else {
                    self.responses.push_back(RESEND);
                }
            }
            LinkSignal::DeviceByteComplete { byte } => {
                if byte != RESEND {
                    self.last_sent = Some(byte);
                }
                if byte == 0x83 && self.resume_scanning_after_id {
                    self.resume_scanning_after_id = false;
                    self.restore_scanning(true);
                }
            }
        }
    }

    fn handle_command_byte(&mut self, byte: u8) {
        if self.bat_active && byte != 0xff {
            return;
        }
        if self.command_parameter != KeyboardParameter::None && byte >= 0xed {
            self.command_parameter = KeyboardParameter::None;
            self.handle_command(byte);
            return;
        }
        match self.command_parameter {
            KeyboardParameter::None => self.handle_command(byte),
            KeyboardParameter::Leds { was_enabled } => {
                if byte & 0xf8 != 0 {
                    self.responses.push_back(RESEND);
                } else {
                    self.leds = byte;
                    self.responses.push_back(ACK);
                }
                self.restore_scanning(was_enabled);
                self.command_parameter = KeyboardParameter::None;
            }
            KeyboardParameter::ScanSet { was_enabled } => {
                if byte <= 3 {
                    self.responses.push_back(ACK);
                    if byte == 0 {
                        self.responses.push_back(self.scan_set);
                    } else {
                        self.scan_set = byte;
                    }
                } else {
                    self.responses.push_back(RESEND);
                }
                self.restore_scanning(was_enabled);
                self.command_parameter = KeyboardParameter::None;
            }
            KeyboardParameter::Typematic { was_enabled } => {
                if byte & 0x80 == 0 {
                    self.typematic_parameter = byte;
                    self.responses.push_back(ACK);
                } else {
                    self.responses.push_back(RESEND);
                }
                self.restore_scanning(was_enabled);
                self.command_parameter = KeyboardParameter::None;
            }
            KeyboardParameter::KeyType(key_type) => {
                if let Some(entry) = SCAN_CODES.iter().find(|entry| entry.set3 == byte) {
                    self.set3_types.insert(entry.key, key_type);
                    self.responses.push_back(ACK);
                } else {
                    self.responses.push_back(RESEND);
                }
                self.command_parameter = KeyboardParameter::None;
            }
        }
    }

    fn handle_command(&mut self, command: u8) {
        match command {
            0xed => {
                let was_enabled = self.scanning_enabled;
                self.scanning_enabled = false;
                self.responses.push_back(ACK);
                self.command_parameter = KeyboardParameter::Leds { was_enabled };
            }
            0xee => self.responses.push_back(0xee),
            0xf0 => {
                let was_enabled = self.scanning_enabled;
                self.scanning_enabled = false;
                self.clear_scan_output();
                self.clear_typematic();
                self.responses.push_back(ACK);
                self.command_parameter = KeyboardParameter::ScanSet { was_enabled };
            }
            0xf2 => {
                self.resume_scanning_after_id = self.scanning_enabled;
                self.scanning_enabled = false;
                self.responses.extend([ACK, 0xab, 0x83]);
            }
            0xf3 => {
                let was_enabled = self.scanning_enabled;
                self.scanning_enabled = false;
                self.responses.push_back(ACK);
                self.command_parameter = KeyboardParameter::Typematic { was_enabled };
            }
            0xf4 => {
                self.responses.push_back(ACK);
                self.clear_scan_output();
                self.scanning_enabled = true;
                self.synchronize_pressed_keys();
            }
            0xf5 => {
                self.responses.push_back(ACK);
                self.reset_device_defaults(true);
                self.scanning_enabled = false;
            }
            0xf6 => {
                self.responses.push_back(ACK);
                let enabled = self.scanning_enabled;
                self.reset_device_defaults(true);
                self.restore_scanning(enabled);
            }
            0xf7..=0xfa => {
                self.responses.push_back(ACK);
                self.clear_scan_output();
                let key_type = match command {
                    0xf7 => Ps2KeyType::Typematic,
                    0xf8 => Ps2KeyType::MakeBreak,
                    0xf9 => Ps2KeyType::MakeOnly,
                    _ => Ps2KeyType::TypematicMakeBreak,
                };
                self.set3_types = SCAN_CODES
                    .iter()
                    .map(|entry| (entry.key, key_type))
                    .collect();
            }
            0xfb..=0xfd => {
                self.responses.push_back(ACK);
                self.clear_scan_output();
                self.command_parameter = KeyboardParameter::KeyType(match command {
                    0xfb => Ps2KeyType::Typematic,
                    0xfc => Ps2KeyType::MakeBreak,
                    _ => Ps2KeyType::MakeOnly,
                });
            }
            0xfe => {
                if let Some(byte) = self.last_sent {
                    self.responses.push_back(byte);
                }
            }
            0xff => {
                self.responses.push_back(ACK);
                self.reset_device_defaults(false);
                self.bat_epoch = self.bat_epoch.wrapping_add(1);
                self.bat_active = true;
                self.actions.push_back(Ps2KeyboardAction::Schedule {
                    delay: milliseconds(self.link.timebase_hz, BAT_MILLISECONDS),
                    event: Ps2KeyboardEvent::BatComplete {
                        epoch: self.bat_epoch,
                    },
                });
            }
            0xef | 0xf1 | 0x00..=0xec => self.responses.push_back(RESEND),
        }
    }

    fn reset_device_defaults(&mut self, preserve_leds: bool) {
        let leds = self.leds;
        self.scan_set = 2;
        self.typematic_parameter = DEFAULT_TYPEMATIC;
        self.command_parameter = KeyboardParameter::None;
        self.resume_scanning_after_id = false;
        self.set3_types.clear();
        self.clear_scan_output();
        self.clear_typematic();
        self.scanning_enabled = false;
        if !preserve_leds {
            self.leds = 0;
        } else {
            self.leds = leds;
        }
    }

    fn clear_typematic(&mut self) {
        self.typematic_key = None;
        self.typematic_epoch = self.typematic_epoch.wrapping_add(1);
    }

    fn clear_scan_output(&mut self) {
        self.scan_fifo.clear();
        self.scan_overrun = false;
    }

    fn synchronize_pressed_keys(&mut self) {
        self.clear_typematic();
        let keys: Vec<_> = self.pressed.iter().copied().collect();
        let mut typematic_key = None;
        for key in keys {
            self.enqueue_key(key, true, false);
            if self.key_type(key).repeats() && key != Ps2KeyPosition::Pause {
                typematic_key = Some(key);
            }
        }
        self.typematic_key = typematic_key;
        if self.scanning_enabled && self.typematic_key.is_some() {
            self.schedule_typematic_delay();
        }
    }

    fn restore_scanning(&mut self, enabled: bool) {
        self.scanning_enabled = enabled;
        if enabled {
            self.synchronize_pressed_keys();
        }
    }

    fn key_type(&self, key: Ps2KeyPosition) -> Ps2KeyType {
        if self.scan_set != 3 {
            if key == Ps2KeyPosition::Pause {
                Ps2KeyType::MakeOnly
            } else {
                Ps2KeyType::TypematicMakeBreak
            }
        } else {
            self.set3_types
                .get(&key)
                .copied()
                .unwrap_or_else(|| default_set3_type(scan_code(key).number))
        }
    }

    fn enqueue_key(&mut self, key: Ps2KeyPosition, pressed: bool, typematic: bool) {
        let sequence = self.key_sequence(key, pressed, typematic);
        self.enqueue_scan_sequence(&sequence);
    }

    fn enqueue_scan_sequence(&mut self, sequence: &[u8]) {
        if sequence.is_empty() || self.scan_overrun {
            return;
        }
        if self.scan_fifo.len() + sequence.len() > KEYBOARD_FIFO_CAPACITY {
            let marker = if self.scan_set == 1 { 0xff } else { 0x00 };
            if self.scan_fifo.len() == KEYBOARD_FIFO_CAPACITY {
                self.scan_fifo.pop_back();
            }
            self.scan_fifo.push_back(marker);
            self.scan_overrun = true;
            return;
        }
        self.scan_fifo.extend(sequence.iter().copied());
    }

    fn key_sequence(&self, key: Ps2KeyPosition, pressed: bool, typematic: bool) -> Vec<u8> {
        if typematic && !pressed {
            return vec![];
        }
        match (self.scan_set, key) {
            (1, Ps2KeyPosition::PrintScreen) => self.print_screen_set1(pressed),
            (2, Ps2KeyPosition::PrintScreen) => self.print_screen_set2(pressed),
            (1, Ps2KeyPosition::Pause) => self.pause_set1(pressed),
            (2, Ps2KeyPosition::Pause) => self.pause_set2(pressed),
            (1 | 2, key) if is_navigation_key(key) => self.navigation_sequence(key, pressed),
            _ => regular_sequence(scan_code(key), self.scan_set, pressed),
        }
    }

    fn print_screen_set1(&self, pressed: bool) -> Vec<u8> {
        if self.alt_pressed() {
            return vec![if pressed { 0x54 } else { 0xd4 }];
        }
        if self.control_pressed() || self.shift_pressed() {
            return if pressed {
                vec![0xe0, 0x37]
            } else {
                vec![0xe0, 0xb7]
            };
        }
        if pressed {
            vec![0xe0, 0x2a, 0xe0, 0x37]
        } else {
            vec![0xe0, 0xb7, 0xe0, 0xaa]
        }
    }

    fn print_screen_set2(&self, pressed: bool) -> Vec<u8> {
        if self.alt_pressed() {
            return if pressed {
                vec![0x84]
            } else {
                vec![0xf0, 0x84]
            };
        }
        if self.control_pressed() || self.shift_pressed() {
            return if pressed {
                vec![0xe0, 0x7c]
            } else {
                vec![0xe0, 0xf0, 0x7c]
            };
        }
        if pressed {
            vec![0xe0, 0x12, 0xe0, 0x7c]
        } else {
            vec![0xe0, 0xf0, 0x7c, 0xe0, 0xf0, 0x12]
        }
    }

    fn pause_set1(&self, pressed: bool) -> Vec<u8> {
        if !pressed {
            return vec![];
        }
        if self.control_pressed() {
            vec![0xe0, 0x46, 0xe0, 0xc6]
        } else {
            vec![0xe1, 0x1d, 0x45, 0xe1, 0x9d, 0xc5]
        }
    }

    fn pause_set2(&self, pressed: bool) -> Vec<u8> {
        if !pressed {
            return vec![];
        }
        if self.control_pressed() {
            vec![0xe0, 0x7e, 0xe0, 0xf0, 0x7e]
        } else {
            vec![0xe1, 0x14, 0x77, 0xe1, 0xf0, 0x14, 0xf0, 0x77]
        }
    }

    fn navigation_sequence(&self, key: Ps2KeyPosition, pressed: bool) -> Vec<u8> {
        let entry = scan_code(key);
        let left_shift = self.pressed.contains(&Ps2KeyPosition::LeftShift);
        let right_shift = self.pressed.contains(&Ps2KeyPosition::RightShift);
        let shift = left_shift || right_shift;
        let num_lock = self.leds & 2 != 0;
        let cancel_shift = shift && !num_lock;
        let synthesize_shift = !shift && num_lock;
        match self.scan_set {
            1 => {
                let mut result = Vec::new();
                if pressed {
                    if cancel_shift {
                        result.extend(if right_shift {
                            [0xe0, 0xb6]
                        } else {
                            [0xe0, 0xaa]
                        });
                    } else if synthesize_shift {
                        result.extend([0xe0, 0x2a]);
                    }
                    result.extend([0xe0, entry.set1]);
                } else {
                    result.extend([0xe0, entry.set1 | 0x80]);
                    if cancel_shift {
                        result.extend(if right_shift {
                            [0xe0, 0x36]
                        } else {
                            [0xe0, 0x2a]
                        });
                    } else if synthesize_shift {
                        result.extend([0xe0, 0xaa]);
                    }
                }
                result
            }
            2 => {
                let shift_code = if right_shift { 0x59 } else { 0x12 };
                let mut result = Vec::new();
                if pressed {
                    if cancel_shift {
                        result.extend([0xe0, 0xf0, shift_code]);
                    } else if synthesize_shift {
                        result.extend([0xe0, 0x12]);
                    }
                    result.extend([0xe0, entry.set2]);
                } else {
                    result.extend([0xe0, 0xf0, entry.set2]);
                    if cancel_shift {
                        result.extend([0xe0, shift_code]);
                    } else if synthesize_shift {
                        result.extend([0xe0, 0xf0, 0x12]);
                    }
                }
                result
            }
            _ => regular_sequence(entry, self.scan_set, pressed),
        }
    }

    fn shift_pressed(&self) -> bool {
        self.pressed.contains(&Ps2KeyPosition::LeftShift)
            || self.pressed.contains(&Ps2KeyPosition::RightShift)
    }

    fn control_pressed(&self) -> bool {
        self.pressed.contains(&Ps2KeyPosition::LeftControl)
            || self.pressed.contains(&Ps2KeyPosition::RightControl)
    }

    fn alt_pressed(&self) -> bool {
        self.pressed.contains(&Ps2KeyPosition::LeftAlt)
            || self.pressed.contains(&Ps2KeyPosition::RightAlt)
    }

    fn schedule_typematic_delay(&mut self) {
        let delay_units = u64::from((self.typematic_parameter >> 5) & 3) + 1;
        self.actions.push_back(Ps2KeyboardAction::Schedule {
            delay: milliseconds(self.link.timebase_hz, delay_units * 250),
            event: Ps2KeyboardEvent::Typematic {
                epoch: self.typematic_epoch,
            },
        });
    }

    fn schedule_typematic_period(&mut self) {
        let rate = self.typematic_parameter & 0x1f;
        let a = u64::from(rate & 7);
        let b = u64::from((rate >> 3) & 3);
        let microseconds = (8 + a) * (1 << b) * 4_170;
        self.actions.push_back(Ps2KeyboardAction::Schedule {
            delay: SimDuration::new(self.link.timebase_hz.saturating_mul(microseconds) / 1_000_000),
            event: Ps2KeyboardEvent::Typematic {
                epoch: self.typematic_epoch,
            },
        });
    }

    fn pump_output(&mut self) {
        if !self.link.can_start() {
            return;
        }
        if let Some(byte) = self.responses.pop_front() {
            let started = self.link.start_device_byte(byte);
            debug_assert!(started, "an idle PS/2 link must accept one byte");
            return;
        }
        if self.link.resume_deferred_device_byte() {
            return;
        }
        let Some(byte) = self.scan_fifo.pop_front() else {
            return;
        };
        if self.scan_overrun && byte == self.overrun_marker() {
            self.scan_overrun = false;
        }
        let started = self.link.start_device_byte(byte);
        debug_assert!(started, "an idle PS/2 link must accept one byte");
    }

    fn overrun_marker(&self) -> u8 {
        if self.scan_set == 1 { 0xff } else { 0x00 }
    }
}

impl Component for Ps2Keyboard {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.power_on(self.link.now);
    }
}

fn scan_code(key: Ps2KeyPosition) -> &'static ScanCodeEntry {
    SCAN_CODES
        .iter()
        .find(|entry| entry.key == key)
        .expect("every public PS/2 key position must have scan codes")
}

fn regular_sequence(entry: &ScanCodeEntry, scan_set: u8, pressed: bool) -> Vec<u8> {
    match scan_set {
        1 => {
            let mut result = Vec::with_capacity(2);
            if entry.set1_extended {
                result.push(0xe0);
            }
            result.push(if pressed {
                entry.set1
            } else {
                entry.set1 | 0x80
            });
            result
        }
        2 => {
            let mut result = Vec::with_capacity(3);
            if entry.set2_extended {
                result.push(0xe0);
            }
            if !pressed {
                result.push(0xf0);
            }
            result.push(entry.set2);
            result
        }
        3 => {
            if pressed {
                vec![entry.set3]
            } else {
                vec![0xf0, entry.set3]
            }
        }
        _ => vec![],
    }
}

fn default_set3_type(number: u8) -> Ps2KeyType {
    match number {
        30 | 44 | 57 | 58 | 60 => Ps2KeyType::MakeBreak,
        62 | 64 | 75 | 80 | 81 | 85 | 86 | 90..=105 | 108 | 110..=126 => Ps2KeyType::MakeOnly,
        _ => Ps2KeyType::Typematic,
    }
}

fn is_navigation_key(key: Ps2KeyPosition) -> bool {
    matches!(
        key,
        Ps2KeyPosition::Insert
            | Ps2KeyPosition::Delete
            | Ps2KeyPosition::Home
            | Ps2KeyPosition::End
            | Ps2KeyPosition::PageUp
            | Ps2KeyPosition::PageDown
            | Ps2KeyPosition::ArrowLeft
            | Ps2KeyPosition::ArrowRight
            | Ps2KeyPosition::ArrowUp
            | Ps2KeyPosition::ArrowDown
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyboard() -> Ps2Keyboard {
        let mut keyboard = Ps2Keyboard::new(
            ComponentId::new(1),
            "keyboard",
            Ps2Wiring {
                controller: ComponentId::new(2),
                bus: ComponentId::new(3),
            },
            1_000_000_000,
        )
        .expect("the keyboard must build");
        keyboard.bat_active = false;
        keyboard
    }

    #[test]
    fn ide_set_three_initialization_programs_make_break_keys() {
        let mut keyboard = keyboard();
        for byte in [0xf5, 0xf0, 0x03, 0xfa, 0xfc, 0x14, 0xfc, 0x76] {
            keyboard.handle_command_byte(byte);
        }

        assert_eq!(keyboard.scan_set, 3);
        assert_eq!(keyboard.responses, [ACK; 8]);
        assert_eq!(
            keyboard.key_type(Ps2KeyPosition::CapsLock),
            Ps2KeyType::MakeBreak
        );
        assert_eq!(
            keyboard.key_type(Ps2KeyPosition::NumLock),
            Ps2KeyType::MakeBreak
        );
        assert_eq!(
            keyboard.key_type(Ps2KeyPosition::A),
            Ps2KeyType::TypematicMakeBreak
        );
    }

    #[test]
    fn special_keys_use_ibm_set_one_and_set_two_sequences() {
        let mut keyboard = keyboard();
        keyboard.scan_set = 1;
        assert_eq!(
            keyboard.key_sequence(Ps2KeyPosition::PrintScreen, true, false),
            [0xe0, 0x2a, 0xe0, 0x37]
        );
        assert_eq!(
            keyboard.key_sequence(Ps2KeyPosition::PrintScreen, false, false),
            [0xe0, 0xb7, 0xe0, 0xaa]
        );
        assert_eq!(
            keyboard.key_sequence(Ps2KeyPosition::Pause, true, false),
            [0xe1, 0x1d, 0x45, 0xe1, 0x9d, 0xc5]
        );

        keyboard.scan_set = 2;
        assert_eq!(
            keyboard.key_sequence(Ps2KeyPosition::PrintScreen, true, false),
            [0xe0, 0x12, 0xe0, 0x7c]
        );
        assert_eq!(
            keyboard.key_sequence(Ps2KeyPosition::Pause, true, false),
            [0xe1, 0x14, 0x77, 0xe1, 0xf0, 0x14, 0xf0, 0x77]
        );
        assert!(
            keyboard
                .key_sequence(Ps2KeyPosition::Pause, false, false)
                .is_empty()
        );
    }

    #[test]
    fn scan_fifo_overrun_replaces_no_partial_sequence() {
        let mut keyboard = keyboard();
        keyboard.scan_set = 2;
        keyboard.scan_fifo.extend([0x55; 15]);

        keyboard.enqueue_key(Ps2KeyPosition::RightControl, false, false);

        assert_eq!(keyboard.scan_fifo.len(), KEYBOARD_FIFO_CAPACITY);
        assert_eq!(keyboard.scan_fifo.back(), Some(&0x00));
        assert!(keyboard.scan_overrun);
        let snapshot = keyboard.scan_fifo.clone();
        keyboard.enqueue_key(Ps2KeyPosition::A, true, false);
        assert_eq!(keyboard.scan_fifo, snapshot);
    }

    #[test]
    fn command_responses_precede_pending_scan_codes() {
        let mut keyboard = keyboard();
        keyboard.scanning_enabled = true;
        keyboard.enqueue_key(Ps2KeyPosition::A, true, false);
        keyboard.handle_command_byte(0xf2);

        assert_eq!(keyboard.responses, [ACK, 0xab, 0x83]);
        assert_eq!(keyboard.scan_fifo, [0x1c]);
    }

    #[test]
    fn read_id_pauses_scanning_until_the_second_id_byte_completes() {
        let mut keyboard = keyboard();
        keyboard.scanning_enabled = true;

        keyboard.handle_command_byte(0xf2);

        assert!(!keyboard.scanning_enabled);
        assert!(keyboard.resume_scanning_after_id);
        keyboard.handle_link_signal(LinkSignal::DeviceByteComplete { byte: 0xab });
        assert!(!keyboard.scanning_enabled);
        keyboard.handle_link_signal(LinkSignal::DeviceByteComplete { byte: 0x83 });
        assert!(keyboard.scanning_enabled);
        assert!(!keyboard.resume_scanning_after_id);
    }

    #[test]
    fn typematic_output_does_not_accumulate_while_clock_is_inhibited() {
        let mut keyboard = keyboard();
        keyboard.scanning_enabled = true;
        keyboard.pressed.insert(Ps2KeyPosition::A);
        keyboard.typematic_key = Some(Ps2KeyPosition::A);
        keyboard.link.observed_clock_low = true;
        let epoch = keyboard.typematic_epoch;

        keyboard.handle_event(SimTime::new(1), Ps2KeyboardEvent::Typematic { epoch });

        assert!(keyboard.scan_fifo.is_empty());
        assert!(matches!(
            keyboard.actions.back(),
            Some(Ps2KeyboardAction::Schedule {
                event: Ps2KeyboardEvent::Typematic { .. },
                ..
            })
        ));
    }

    #[test]
    fn enabling_scanning_resynchronizes_keys_pressed_while_disabled() {
        let mut keyboard = keyboard();
        keyboard.apply_input(Ps2KeyboardInput {
            key: Ps2KeyPosition::B,
            pressed: true,
        });
        assert!(keyboard.scan_fifo.is_empty());

        keyboard.handle_command_byte(0xf4);

        assert!(keyboard.scanning_enabled);
        assert_eq!(keyboard.responses, [ACK]);
        assert_eq!(keyboard.scan_fifo, [0x32]);
        assert_eq!(keyboard.typematic_key, Some(Ps2KeyPosition::B));
    }
}
