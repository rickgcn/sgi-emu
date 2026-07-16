use super::*;

const ACK: u8 = 0xfa;
const RESEND: u8 = 0xfe;

impl Ps2Mouse {
    /// Creates a standard three-button mouse on one PS/2 bus.
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
            mode: MouseMode::Stream,
            wrap_mode: false,
            reporting_enabled: false,
            scaling_2_to_1: false,
            resolution: 2,
            sample_rate: 100,
            parameter: MouseParameter::None,
            accumulated_x_eighths: 0,
            accumulated_y_eighths: 0,
            buttons: Ps2MouseButtons::default(),
            last_reported_buttons: Ps2MouseButtons::default(),
            last_packet: [0; 3],
            last_packet_valid: false,
            sample_epoch: 0,
            sample_remainder: 0,
            bat_epoch: 0,
            bat_active: false,
            actions: VecDeque::new(),
        })
    }

    /// Starts the deterministic power-on BAT interval.
    pub fn power_on(&mut self, now: SimTime) {
        self.actions.clear();
        self.link.observe_time(now);
        self.link.reset();
        self.responses.clear();
        self.reset_defaults();
        self.buttons = Ps2MouseButtons::default();
        self.last_reported_buttons = self.buttons;
        self.last_packet = [0; 3];
        self.last_packet_valid = false;
        self.bat_epoch = self.bat_epoch.wrapping_add(1);
        self.bat_active = true;
        self.actions.push_back(Ps2MouseAction::Schedule {
            delay: milliseconds(self.link.timebase_hz, BAT_MILLISECONDS),
            event: Ps2MouseEvent::BatComplete {
                epoch: self.bat_epoch,
            },
        });
    }

    /// Applies relative host movement and authoritative button state.
    pub fn apply_input(&mut self, input: Ps2MouseInput) {
        let scale = match self.resolution {
            0 => 2,
            1 => 4,
            2 => 8,
            _ => 16,
        };
        self.accumulated_x_eighths = self
            .accumulated_x_eighths
            .saturating_add(i64::from(input.delta_x).saturating_mul(scale));
        self.accumulated_y_eighths = self
            .accumulated_y_eighths
            .saturating_add(i64::from(input.delta_y).saturating_mul(scale));
        self.buttons = input.buttons;
    }

    /// Releases all buttons while preserving pending relative movement.
    pub fn release_all(&mut self) {
        self.buttons = Ps2MouseButtons::default();
    }

    /// Observes one aggregate line transition from the attached bus.
    pub fn observe_lines(&mut self, delivery: TwoWireLineDelivery) -> Result<(), Ps2MouseError> {
        if let Some(signal) = self
            .link
            .observe_lines(delivery)
            .map_err(Ps2MouseError::InvalidBus)?
        {
            self.handle_link_signal(signal);
        }
        self.pump_output();
        Ok(())
    }

    /// Handles one scheduled device transition.
    pub fn handle_event(&mut self, now: SimTime, event: Ps2MouseEvent) {
        self.link.observe_time(now);
        match event {
            Ps2MouseEvent::Link(event) => {
                if let Some(signal) = self.link.handle_event(event) {
                    self.handle_link_signal(signal);
                }
            }
            Ps2MouseEvent::BatComplete { epoch } if epoch == self.bat_epoch && self.bat_active => {
                self.bat_active = false;
                self.responses.extend([0xaa, 0x00]);
            }
            Ps2MouseEvent::Sample { epoch }
                if epoch == self.sample_epoch
                    && self.mode == MouseMode::Stream
                    && self.reporting_enabled =>
            {
                if self.has_reportable_change()
                    && self.link.can_start()
                    && self.responses.is_empty()
                    && !self.link.has_deferred_device_byte()
                {
                    self.queue_packet();
                }
                self.schedule_sample();
            }
            Ps2MouseEvent::BatComplete { .. } | Ps2MouseEvent::Sample { .. } => {}
        }
        self.pump_output();
    }

    /// Polls one pending schedule or line-drive action.
    pub fn poll(&mut self) -> Ps2MousePoll {
        if let Some(action) = self.actions.pop_front() {
            return Ps2MousePoll::Action(action);
        }
        match self.link.poll() {
            Some(LinkAction::Schedule { delay, event }) => {
                Ps2MousePoll::Action(Ps2MouseAction::Schedule {
                    delay,
                    event: Ps2MouseEvent::Link(event),
                })
            }
            Some(LinkAction::Drive(drive)) => Ps2MousePoll::Action(Ps2MouseAction::Drive(drive)),
            None => Ps2MousePoll::Idle,
        }
    }

    /// Returns whether data reporting is enabled.
    pub const fn reporting_enabled(&self) -> bool {
        self.reporting_enabled
    }

    /// Returns the programmed sample rate in reports per second.
    pub const fn sample_rate(&self) -> u16 {
        self.sample_rate
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
            LinkSignal::DeviceByteComplete { .. } => {}
        }
    }

    fn handle_command_byte(&mut self, byte: u8) {
        if self.bat_active && byte != 0xff {
            return;
        }
        if self.wrap_mode && !matches!(byte, 0xec | 0xff) {
            self.responses.push_back(byte);
            return;
        }
        if self.parameter != MouseParameter::None && byte >= 0xe6 {
            self.parameter = MouseParameter::None;
            self.handle_command(byte);
            return;
        }
        match self.parameter {
            MouseParameter::None => self.handle_command(byte),
            MouseParameter::Resolution => {
                if byte <= 3 {
                    self.resolution = byte;
                    self.responses.push_back(ACK);
                } else {
                    self.responses.push_back(RESEND);
                }
                self.parameter = MouseParameter::None;
            }
            MouseParameter::SampleRate => {
                if matches!(byte, 10 | 20 | 40 | 60 | 80 | 100 | 200) {
                    self.sample_rate = u16::from(byte);
                    self.sample_remainder = 0;
                    self.responses.push_back(ACK);
                    if self.mode == MouseMode::Stream && self.reporting_enabled {
                        self.start_sampling();
                    }
                } else {
                    self.responses.push_back(RESEND);
                }
                self.parameter = MouseParameter::None;
            }
        }
    }

    fn handle_command(&mut self, command: u8) {
        match command {
            0xe6 => {
                self.scaling_2_to_1 = false;
                self.responses.push_back(ACK);
            }
            0xe7 => {
                self.scaling_2_to_1 = true;
                self.responses.push_back(ACK);
            }
            0xe8 => {
                self.responses.push_back(ACK);
                self.parameter = MouseParameter::Resolution;
            }
            0xe9 => {
                let mut status = 0u8;
                status |= u8::from(self.mode == MouseMode::Remote) << 6;
                status |= u8::from(self.reporting_enabled) << 5;
                status |= u8::from(self.scaling_2_to_1) << 4;
                status |= u8::from(self.buttons.middle) << 2;
                status |= u8::from(self.buttons.right) << 1;
                status |= u8::from(self.buttons.left);
                self.responses
                    .extend([ACK, status, self.resolution, self.sample_rate as u8]);
            }
            0xea => {
                self.mode = MouseMode::Stream;
                self.responses.push_back(ACK);
                if self.reporting_enabled {
                    self.start_sampling();
                }
            }
            0xeb => {
                self.responses.push_back(ACK);
                self.queue_packet();
            }
            0xec => {
                self.wrap_mode = false;
                self.responses.push_back(ACK);
            }
            0xee => {
                self.wrap_mode = true;
                self.responses.push_back(ACK);
            }
            0xf0 => {
                self.mode = MouseMode::Remote;
                self.cancel_sampling();
                self.responses.push_back(ACK);
            }
            0xf2 => self.responses.extend([ACK, 0x00]),
            0xf3 => {
                self.responses.push_back(ACK);
                self.parameter = MouseParameter::SampleRate;
            }
            0xf4 => {
                self.reporting_enabled = true;
                self.responses.push_back(ACK);
                if self.mode == MouseMode::Stream {
                    self.start_sampling();
                }
            }
            0xf5 => {
                self.reporting_enabled = false;
                self.cancel_sampling();
                self.responses.push_back(ACK);
            }
            0xf6 => {
                self.reset_defaults();
                self.responses.push_back(ACK);
            }
            0xfe => {
                if self.last_packet_valid {
                    self.responses.extend(self.last_packet);
                }
            }
            0xff => {
                self.responses.push_back(ACK);
                self.reset_defaults();
                self.bat_epoch = self.bat_epoch.wrapping_add(1);
                self.bat_active = true;
                self.actions.push_back(Ps2MouseAction::Schedule {
                    delay: milliseconds(self.link.timebase_hz, BAT_MILLISECONDS),
                    event: Ps2MouseEvent::BatComplete {
                        epoch: self.bat_epoch,
                    },
                });
            }
            _ => self.responses.push_back(RESEND),
        }
    }

    fn reset_defaults(&mut self) {
        self.mode = MouseMode::Stream;
        self.wrap_mode = false;
        self.reporting_enabled = false;
        self.scaling_2_to_1 = false;
        self.resolution = 2;
        self.sample_rate = 100;
        self.parameter = MouseParameter::None;
        self.accumulated_x_eighths = 0;
        self.accumulated_y_eighths = 0;
        self.last_reported_buttons = self.buttons;
        self.sample_remainder = 0;
        self.cancel_sampling();
    }

    fn has_reportable_change(&self) -> bool {
        self.accumulated_x_eighths.abs() >= 8
            || self.accumulated_y_eighths.abs() >= 8
            || self.buttons != self.last_reported_buttons
    }

    fn queue_packet(&mut self) {
        let raw_x = whole_counts(&mut self.accumulated_x_eighths);
        let raw_y = whole_counts(&mut self.accumulated_y_eighths);
        let scaled_x = if self.scaling_2_to_1 {
            scale_2_to_1(raw_x)
        } else {
            raw_x
        };
        let scaled_y = if self.scaling_2_to_1 {
            scale_2_to_1(raw_y)
        } else {
            raw_y
        };
        let x_overflow = !(-255..=255).contains(&scaled_x);
        let y_overflow = !(-255..=255).contains(&scaled_y);
        let x = scaled_x.clamp(-255, 255);
        let y = scaled_y.clamp(-255, 255);
        let mut first = 0x08;
        first |= u8::from(self.buttons.left);
        first |= u8::from(self.buttons.right) << 1;
        first |= u8::from(self.buttons.middle) << 2;
        first |= u8::from(x < 0) << 4;
        first |= u8::from(y < 0) << 5;
        first |= u8::from(x_overflow) << 6;
        first |= u8::from(y_overflow) << 7;
        self.last_packet = [first, x as i16 as u8, y as i16 as u8];
        self.last_packet_valid = true;
        self.last_reported_buttons = self.buttons;
        self.responses.extend(self.last_packet);
    }

    fn start_sampling(&mut self) {
        self.sample_epoch = self.sample_epoch.wrapping_add(1);
        self.schedule_sample();
    }

    fn cancel_sampling(&mut self) {
        self.sample_epoch = self.sample_epoch.wrapping_add(1);
    }

    fn schedule_sample(&mut self) {
        let mut projection = RationalClockProjection::new(
            self.link.timebase_hz,
            u64::from(self.sample_rate),
            1,
            self.sample_remainder,
        );
        let delay = projection
            .advance(1)
            .expect("the bounded mouse sample projection cannot overflow");
        self.sample_remainder = projection.remainder();
        self.actions.push_back(Ps2MouseAction::Schedule {
            delay,
            event: Ps2MouseEvent::Sample {
                epoch: self.sample_epoch,
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
        self.link.resume_deferred_device_byte();
    }
}

impl Component for Ps2Mouse {
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

fn whole_counts(accumulator: &mut i64) -> i64 {
    let result = *accumulator / 8;
    *accumulator %= 8;
    result
}

pub(super) fn scale_2_to_1(value: i64) -> i64 {
    let sign = value.signum();
    let magnitude = value.saturating_abs();
    let scaled = match magnitude {
        0 => 0,
        1 => 1,
        2 => 1,
        3 => 3,
        4 => 6,
        5 => 9,
        _ => magnitude.saturating_mul(2),
    };
    scaled.saturating_mul(sign)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse() -> Ps2Mouse {
        let mut mouse = Ps2Mouse::new(
            ComponentId::new(1),
            "mouse",
            Ps2Wiring {
                controller: ComponentId::new(2),
                bus: ComponentId::new(3),
            },
            1_000_000_000,
        )
        .expect("the mouse must build");
        mouse.bat_active = false;
        mouse
    }

    #[test]
    fn ide_initialization_selects_forty_hertz_stream_reporting() {
        let mut mouse = mouse();
        for byte in [0xf6, 0xf3, 40, 0xf4] {
            mouse.handle_command_byte(byte);
        }

        assert_eq!(mouse.responses, [ACK; 4]);
        assert_eq!(mouse.mode, MouseMode::Stream);
        assert_eq!(mouse.sample_rate, 40);
        assert!(mouse.reporting_enabled);
    }

    #[test]
    fn packet_encodes_guest_up_as_positive_y_and_standard_buttons() {
        let mut mouse = mouse();
        mouse.apply_input(Ps2MouseInput {
            delta_x: 4,
            delta_y: -3,
            buttons: Ps2MouseButtons {
                left: true,
                middle: false,
                right: false,
            },
        });

        mouse.queue_packet();

        assert_eq!(mouse.responses, [0x29, 0x04, 0xfd]);
        assert_eq!(mouse.accumulated_x_eighths, 0);
        assert_eq!(mouse.accumulated_y_eighths, 0);
    }

    #[test]
    fn packet_sets_overflow_and_clamps_each_axis() {
        let mut mouse = mouse();
        mouse.apply_input(Ps2MouseInput {
            delta_x: 300,
            delta_y: -300,
            buttons: Ps2MouseButtons::default(),
        });

        mouse.queue_packet();

        assert_eq!(mouse.responses, [0xe8, 0xff, 0x01]);
    }

    #[test]
    fn remote_read_reports_and_clears_accumulated_counts() {
        let mut mouse = mouse();
        mouse.handle_command_byte(0xf0);
        mouse.responses.clear();
        mouse.apply_input(Ps2MouseInput {
            delta_x: 2,
            delta_y: 1,
            buttons: Ps2MouseButtons {
                left: false,
                middle: true,
                right: true,
            },
        });

        mouse.handle_command_byte(0xeb);

        assert_eq!(mouse.responses, [ACK, 0x0e, 0x02, 0x01]);
        assert_eq!(mouse.accumulated_x_eighths, 0);
        assert_eq!(mouse.accumulated_y_eighths, 0);
    }

    #[test]
    fn wrap_mode_echoes_commands_until_reset_wrap() {
        let mut mouse = mouse();
        mouse.handle_command_byte(0xee);
        mouse.handle_command_byte(0xf2);
        mouse.handle_command_byte(0xec);

        assert_eq!(mouse.responses, [ACK, 0xf2, ACK]);
        assert!(!mouse.wrap_mode);
    }

    #[test]
    fn inhibited_stream_keeps_motion_in_counters_without_packet_backlog() {
        let mut mouse = mouse();
        mouse.mode = MouseMode::Stream;
        mouse.reporting_enabled = true;
        mouse.link.observed_clock_low = true;
        mouse.apply_input(Ps2MouseInput {
            delta_x: 9,
            delta_y: -7,
            buttons: Ps2MouseButtons::default(),
        });
        let epoch = mouse.sample_epoch;

        mouse.handle_event(SimTime::new(1), Ps2MouseEvent::Sample { epoch });

        assert!(mouse.responses.is_empty());
        assert_eq!(mouse.accumulated_x_eighths, 72);
        assert_eq!(mouse.accumulated_y_eighths, -56);
    }
}
