//! Deterministic 16550-compatible UART.

use std::{collections::VecDeque, fmt};

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;
use se_core::scheduler::SimDuration;

use crate::bus::irq::{IrqOutput, IrqSource, IrqTransaction};
use crate::bus::isa::{
    IsaBusError, IsaCompletion, IsaCompletionPayload, IsaDeviceResponse, IsaTransaction,
    IsaTransferView,
};

/// Interrupt output driven by a UART.
pub const UART16550_IRQ_OUTPUT: IrqOutput = IrqOutput::new(0);

const FIFO_CAPACITY: usize = 16;
const DEFAULT_MODEM_INPUTS: u8 = 0xb0;

/// Construction parameters for one UART.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Uart16550Config {
    /// Clock supplied to the external prescaler.
    pub input_clock_hz: u64,
    /// Number of scheduler ticks in one second.
    pub timebase_hz: u64,
    /// Maximum number of bytes waiting at the external receive pin.
    pub external_queue_capacity: usize,
}

impl Uart16550Config {
    /// Validates the configuration.
    pub fn validate(self) -> Result<(), Uart16550Error> {
        if self.input_clock_hz == 0 {
            return Err(Uart16550Error::ZeroInputClock);
        }
        if self.timebase_hz == 0 {
            return Err(Uart16550Error::ZeroTimebase);
        }
        if self.external_queue_capacity == 0 {
            return Err(Uart16550Error::ZeroExternalQueueCapacity);
        }
        Ok(())
    }
}

/// UART construction or external-link error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Uart16550Error {
    ZeroInputClock,
    ZeroTimebase,
    ZeroExternalQueueCapacity,
    ExternalReceiveQueueFull,
}

impl fmt::Display for Uart16550Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroInputClock => write!(formatter, "UART input clock must be nonzero"),
            Self::ZeroTimebase => write!(formatter, "UART timebase must be nonzero"),
            Self::ZeroExternalQueueCapacity => {
                write!(formatter, "UART external receive queue must be nonempty")
            }
            Self::ExternalReceiveQueueFull => {
                write!(formatter, "UART external receive queue is full")
            }
        }
    }
}

impl std::error::Error for Uart16550Error {}

/// Scheduled UART transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Uart16550Event {
    TransmitComplete { epoch: u64 },
    ReceiveComplete { epoch: u64 },
    ReceiveTimeout { epoch: u64, generation: u64 },
}

/// Observable UART action.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Uart16550Action {
    Schedule {
        delay: SimDuration,
        event: Uart16550Event,
    },
    SetIrq(IrqTransaction),
    Transmit {
        byte: u8,
    },
    Idle,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct SerialClock {
    remainder: u128,
}

impl SerialClock {
    fn delay(
        &mut self,
        config: Uart16550Config,
        divisor: u16,
        prescaler_twice: u8,
        frame_half_bits: u8,
    ) -> Option<SimDuration> {
        if divisor == 0 || prescaler_twice == 0 {
            return None;
        }

        let numerator = u128::from(config.timebase_hz)
            * u128::from(divisor)
            * u128::from(prescaler_twice)
            * u128::from(frame_half_bits)
            * 4;
        let denominator = u128::from(config.input_clock_hz);
        let accumulated = numerator + self.remainder;
        let ticks = accumulated / denominator;
        self.remainder = accumulated % denominator;
        Some(SimDuration::new(
            ticks.max(1).min(u128::from(u64::MAX)) as u64
        ))
    }
}

/// Software-visible 16550 register file, FIFOs, and serial shift registers.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Uart16550 {
    id: ComponentId,
    name: String,
    config: Uart16550Config,
    divisor: u16,
    interrupt_enable: u8,
    fifo_control: u8,
    line_control: u8,
    modem_control: u8,
    line_status_errors: u8,
    modem_status: u8,
    scratch: u8,
    infrared_control: u8,
    receive: VecDeque<u8>,
    transmit: VecDeque<u8>,
    external_receive: VecDeque<u8>,
    receive_shift: Option<u8>,
    transmit_shift: Option<u8>,
    receive_clock: SerialClock,
    transmit_clock: SerialClock,
    epoch: u64,
    timeout_generation: u64,
    receive_timeout_pending: bool,
    thre_interrupt_pending: bool,
    irq_asserted: bool,
    actions: VecDeque<Uart16550Action>,
}

crate::component_state!(Uart16550State, Uart16550);

impl Uart16550 {
    /// Creates a reset UART.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        config: Uart16550Config,
    ) -> Result<Self, Uart16550Error> {
        config.validate()?;
        Ok(Self {
            id,
            name: name.into(),
            config,
            divisor: 0,
            interrupt_enable: 0,
            fifo_control: 0,
            line_control: 0,
            modem_control: 0,
            line_status_errors: 0,
            modem_status: DEFAULT_MODEM_INPUTS,
            scratch: 0,
            infrared_control: 0,
            receive: VecDeque::new(),
            transmit: VecDeque::new(),
            external_receive: VecDeque::new(),
            receive_shift: None,
            transmit_shift: None,
            receive_clock: SerialClock::default(),
            transmit_clock: SerialClock::default(),
            epoch: 0,
            timeout_generation: 0,
            receive_timeout_pending: false,
            thre_interrupt_pending: false,
            irq_asserted: false,
            actions: VecDeque::new(),
        })
    }

    /// Queues externally received bytes atomically.
    pub fn receive_bytes(&mut self, bytes: &[u8]) -> Result<(), Uart16550Error> {
        if self.infrared_enabled() && !self.loopback_enabled() {
            return Ok(());
        }
        if self.external_receive.len() + bytes.len() > self.config.external_queue_capacity {
            return Err(Uart16550Error::ExternalReceiveQueueFull);
        }
        self.external_receive.extend(bytes.iter().copied());
        self.start_receive();
        Ok(())
    }

    /// Applies one scheduled UART transition.
    pub fn handle_event(&mut self, event: Uart16550Event) {
        match event {
            Uart16550Event::TransmitComplete { epoch } if epoch == self.epoch => {
                if let Some(byte) = self.transmit_shift.take() {
                    if self.loopback_enabled() {
                        self.external_receive.push_front(byte);
                        self.start_receive();
                    } else if !self.infrared_enabled() {
                        self.actions.push_back(Uart16550Action::Transmit { byte });
                    }
                }
                self.start_transmit();
            }
            Uart16550Event::ReceiveComplete { epoch } if epoch == self.epoch => {
                if let Some(byte) = self.receive_shift.take() {
                    let capacity = self.receive_capacity();
                    if self.receive.len() == capacity {
                        self.line_status_errors |= 1 << 1;
                    } else {
                        self.receive.push_back(byte);
                    }
                }
                self.schedule_receive_timeout();
                self.start_receive();
            }
            Uart16550Event::ReceiveTimeout { epoch, generation }
                if epoch == self.epoch && generation == self.timeout_generation =>
            {
                self.receive_timeout_pending = self.fifo_enabled()
                    && !self.receive.is_empty()
                    && self.receive.len() < self.receive_trigger();
            }
            Uart16550Event::TransmitComplete { .. }
            | Uart16550Event::ReceiveComplete { .. }
            | Uart16550Event::ReceiveTimeout { .. } => {}
        }
        self.update_irq();
    }

    /// Polls one UART action.
    pub fn poll(&mut self) -> Uart16550Action {
        self.actions.pop_front().unwrap_or(Uart16550Action::Idle)
    }

    /// Returns the number of bytes waiting outside the receiver.
    pub fn external_receive_len(&self) -> usize {
        self.external_receive.len() + usize::from(self.receive_shift.is_some())
    }

    fn read(&mut self, register: u8) -> Result<u8, IsaBusError> {
        let dlab = self.line_control & 0x80 != 0;
        let value = match (register & 7, dlab) {
            (0, true) => self.divisor as u8,
            (1, true) => (self.divisor >> 8) as u8,
            (7, true) => self.infrared_control,
            (0, false) => {
                let value = self.receive.pop_front().unwrap_or(0);
                self.receive_timeout_pending = false;
                self.schedule_receive_timeout();
                value
            }
            (1, false) => self.interrupt_enable,
            (2, _) => {
                let value = self.interrupt_identification();
                if value & 0x0f == 0x02 {
                    self.thre_interrupt_pending = false;
                }
                value
            }
            (3, _) => self.line_control,
            (4, _) => self.modem_control,
            (5, _) => {
                let value = self.line_status();
                self.line_status_errors = 0;
                value
            }
            (6, _) => {
                let value = self.modem_status;
                self.modem_status &= 0xf0;
                value
            }
            (7, false) => self.scratch,
            _ => return Err(IsaBusError::Address),
        };
        self.update_irq();
        Ok(value)
    }

    fn write(&mut self, register: u8, value: u8) -> Result<(), IsaBusError> {
        let dlab = self.line_control & 0x80 != 0;
        match (register & 7, dlab) {
            (0, true) => {
                self.divisor = self.divisor & 0xff00 | u16::from(value);
                self.start_serial_clocks();
            }
            (1, true) => {
                self.divisor = self.divisor & 0x00ff | u16::from(value) << 8;
                self.start_serial_clocks();
            }
            (7, true) => {
                self.infrared_control = value & 0xbf;
                self.start_serial_clocks();
            }
            (0, false) => {
                if self.transmit.len() >= self.transmit_capacity() {
                    return Err(IsaBusError::Access);
                }
                self.transmit.push_back(value);
                self.thre_interrupt_pending = false;
                self.start_transmit();
            }
            (1, false) => {
                let was_thre_disabled = self.interrupt_enable & 0x02 == 0;
                self.interrupt_enable = value & 0x0f;
                if was_thre_disabled
                    && self.interrupt_enable & 0x02 != 0
                    && self.transmit.is_empty()
                {
                    self.thre_interrupt_pending = true;
                }
            }
            (2, _) => {
                self.fifo_control = value & 0xc9;
                if value & 0x02 != 0 {
                    self.receive.clear();
                    self.line_status_errors = 0;
                    self.cancel_receive_timeout();
                }
                if value & 0x04 != 0 {
                    self.transmit.clear();
                    self.thre_interrupt_pending = true;
                }
            }
            (3, _) => self.line_control = value,
            (4, _) => {
                self.modem_control = value & 0x1f;
                self.update_modem_inputs();
            }
            (7, false) => self.scratch = value,
            (5 | 6, _) => return Err(IsaBusError::Access),
            _ => return Err(IsaBusError::Address),
        }
        self.update_irq();
        Ok(())
    }

    fn start_serial_clocks(&mut self) {
        self.start_transmit();
        self.start_receive();
    }

    fn start_transmit(&mut self) {
        if self.transmit_shift.is_some() || self.transmit.is_empty() {
            return;
        }
        let Some(delay) = self.next_transmit_delay() else {
            return;
        };
        self.transmit_shift = self.transmit.pop_front();
        if self.transmit.is_empty() {
            self.thre_interrupt_pending = true;
        }
        self.actions.push_back(Uart16550Action::Schedule {
            delay,
            event: Uart16550Event::TransmitComplete { epoch: self.epoch },
        });
    }

    fn start_receive(&mut self) {
        if self.receive_shift.is_some()
            || self.external_receive.is_empty()
            || self.infrared_enabled() && !self.loopback_enabled()
        {
            return;
        }
        let Some(delay) = self.next_receive_delay() else {
            return;
        };
        self.receive_shift = self.external_receive.pop_front();
        self.actions.push_back(Uart16550Action::Schedule {
            delay,
            event: Uart16550Event::ReceiveComplete { epoch: self.epoch },
        });
    }

    fn next_transmit_delay(&mut self) -> Option<SimDuration> {
        let config = self.config;
        let divisor = self.divisor;
        let prescaler = self.prescaler_twice();
        let frame = self.frame_half_bits();
        self.transmit_clock.delay(config, divisor, prescaler, frame)
    }

    fn next_receive_delay(&mut self) -> Option<SimDuration> {
        let config = self.config;
        let divisor = self.divisor;
        let prescaler = self.prescaler_twice();
        let frame = self.frame_half_bits();
        self.receive_clock.delay(config, divisor, prescaler, frame)
    }

    fn schedule_receive_timeout(&mut self) {
        self.cancel_receive_timeout();
        if !self.fifo_enabled()
            || self.receive.is_empty()
            || self.receive.len() >= self.receive_trigger()
        {
            return;
        }
        let mut clock = SerialClock::default();
        let Some(character) = clock.delay(
            self.config,
            self.divisor,
            self.prescaler_twice(),
            self.frame_half_bits(),
        ) else {
            return;
        };
        let delay = SimDuration::new(character.get().saturating_mul(4));
        self.actions.push_back(Uart16550Action::Schedule {
            delay,
            event: Uart16550Event::ReceiveTimeout {
                epoch: self.epoch,
                generation: self.timeout_generation,
            },
        });
    }

    fn cancel_receive_timeout(&mut self) {
        self.timeout_generation = self.timeout_generation.wrapping_add(1);
        self.receive_timeout_pending = false;
    }

    fn frame_half_bits(&self) -> u8 {
        let data_bits = (self.line_control & 0x03) + 5;
        let parity = u8::from(self.line_control & 0x08 != 0);
        let stop_half_bits = if self.line_control & 0x04 == 0 {
            2
        } else if data_bits == 5 {
            3
        } else {
            4
        };
        2 + data_bits * 2 + parity * 2 + stop_half_bits
    }

    fn prescaler_twice(&self) -> u8 {
        let raw = self.infrared_control & 0x3f;
        (raw & 0x1f) * 2 + (raw >> 5)
    }

    fn fifo_enabled(&self) -> bool {
        self.fifo_control & 1 != 0
    }

    fn infrared_enabled(&self) -> bool {
        self.infrared_control & 0x80 != 0
    }

    fn loopback_enabled(&self) -> bool {
        self.modem_control & 0x10 != 0
    }

    fn receive_capacity(&self) -> usize {
        if self.fifo_enabled() {
            FIFO_CAPACITY
        } else {
            1
        }
    }

    fn transmit_capacity(&self) -> usize {
        if self.fifo_enabled() {
            FIFO_CAPACITY
        } else {
            1
        }
    }

    fn receive_trigger(&self) -> usize {
        match (self.fifo_control >> 6) & 3 {
            0 => 1,
            1 => 4,
            2 => 8,
            _ => 14,
        }
    }

    fn line_status(&self) -> u8 {
        u8::from(!self.receive.is_empty())
            | self.line_status_errors
            | (u8::from(self.transmit.is_empty()) << 5)
            | (u8::from(self.transmit.is_empty() && self.transmit_shift.is_none()) << 6)
    }

    fn interrupt_identification(&self) -> u8 {
        let identifier = if self.line_status_errors != 0 && self.interrupt_enable & 0x04 != 0 {
            0x06
        } else if self.receive.len() >= self.receive_trigger() && self.interrupt_enable & 0x01 != 0
        {
            0x04
        } else if self.receive_timeout_pending && self.interrupt_enable & 0x01 != 0 {
            0x0c
        } else if self.thre_interrupt_pending && self.interrupt_enable & 0x02 != 0 {
            0x02
        } else if self.modem_status & 0x0f != 0 && self.interrupt_enable & 0x08 != 0 {
            0x00
        } else {
            0x01
        };
        identifier | (u8::from(self.fifo_enabled()) * 0xc0)
    }

    fn update_modem_inputs(&mut self) {
        let new_inputs = if self.loopback_enabled() {
            ((self.modem_control & 0x02) << 3)
                | ((self.modem_control & 0x01) << 5)
                | ((self.modem_control & 0x04) << 4)
                | ((self.modem_control & 0x08) << 4)
        } else {
            DEFAULT_MODEM_INPUTS
        };
        let previous = self.modem_status & 0xf0;
        let changed = previous ^ new_inputs;
        let mut delta = 0;
        if changed & 0x10 != 0 {
            delta |= 0x01;
        }
        if changed & 0x20 != 0 {
            delta |= 0x02;
        }
        if previous & 0x40 != 0 && new_inputs & 0x40 == 0 {
            delta |= 0x04;
        }
        if changed & 0x80 != 0 {
            delta |= 0x08;
        }
        self.modem_status = new_inputs | (self.modem_status & 0x0f) | delta;
    }

    fn update_irq(&mut self) {
        let asserted = self.interrupt_identification() & 1 == 0;
        if asserted == self.irq_asserted {
            return;
        }
        self.irq_asserted = asserted;
        self.actions
            .push_back(Uart16550Action::SetIrq(IrqTransaction {
                source: IrqSource {
                    component: self.id,
                    output: UART16550_IRQ_OUTPUT,
                },
                asserted,
            }));
    }

    fn reset_state(&mut self) {
        let next_epoch = self.epoch.wrapping_add(1);
        let id = self.id;
        let name = self.name.clone();
        let config = self.config;
        *self =
            Self::new(id, name, config).expect("validated UART configuration must remain valid");
        self.epoch = next_epoch;
    }
}

impl Component for Uart16550 {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {
        self.reset_state();
    }
}

impl BusDeviceRole<IsaTransaction> for Uart16550 {
    type Response = IsaDeviceResponse;

    fn accept(&mut self, transaction: IsaTransaction) -> Self::Response {
        let result = match transaction.transfer.view() {
            IsaTransferView::Read { length: 1 } => self
                .read(transaction.address as u8)
                .map(|value| IsaCompletionPayload::ReadData([value].into())),
            IsaTransferView::Write { data, byte_enable }
                if data.len() == 1 && byte_enable.iter().eq([true]) =>
            {
                self.write(transaction.address as u8, data[0])
                    .map(|()| IsaCompletionPayload::WriteComplete)
            }
            _ => Err(IsaBusError::Access),
        };
        IsaDeviceResponse::Complete(IsaCompletion {
            id: transaction.id,
            result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::isa::{IsaTransactionId, IsaTransfer};
    use se_core::scheduler::SimTime;

    const UART: ComponentId = ComponentId::new(1);
    const CONTROLLER: ComponentId = ComponentId::new(2);

    fn uart() -> Uart16550 {
        Uart16550::new(
            UART,
            "UART",
            Uart16550Config {
                input_clock_hz: 22_000_000,
                timebase_hz: 1_000_000_000,
                external_queue_capacity: 65_536,
            },
        )
        .expect("UART must build")
    }

    fn write(uart: &mut Uart16550, register: u32, value: u8) {
        let response = uart.accept(IsaTransaction {
            id: IsaTransactionId::new(1),
            time: SimTime::ZERO,
            controller: CONTROLLER,
            target: UART,
            address: register,
            transfer: IsaTransfer::write([value].into(), [true].into()),
        });
        assert!(matches!(
            response,
            IsaDeviceResponse::Complete(IsaCompletion {
                result: Ok(IsaCompletionPayload::WriteComplete),
                ..
            })
        ));
    }

    fn read(uart: &mut Uart16550, register: u32) -> u8 {
        let IsaDeviceResponse::Complete(IsaCompletion {
            result: Ok(IsaCompletionPayload::ReadData(data)),
            ..
        }) = uart.accept(IsaTransaction {
            id: IsaTransactionId::new(2),
            time: SimTime::ZERO,
            controller: CONTROLLER,
            target: UART,
            address: register,
            transfer: IsaTransfer::read(1),
        })
        else {
            panic!("UART read must complete")
        };
        data[0]
    }

    fn configure_9600_8n1(uart: &mut Uart16550) {
        write(uart, 3, 0x80);
        write(uart, 0, 48);
        write(uart, 1, 0);
        write(uart, 7, 3);
        write(uart, 3, 0x03);
    }

    #[test]
    fn fifo_clear_write_used_by_the_ip32_prom_is_accepted() {
        let mut uart = uart();
        write(&mut uart, 2, 6);
    }

    #[test]
    fn configured_character_delay_uses_prescaler_divisor_and_frame() {
        let mut uart = uart();
        configure_9600_8n1(&mut uart);
        write(&mut uart, 0, b'A');
        assert_eq!(
            uart.poll(),
            Uart16550Action::Schedule {
                delay: SimDuration::new(1_047_272),
                event: Uart16550Event::TransmitComplete { epoch: 0 }
            }
        );
        assert_eq!(read(&mut uart, 5) & 0x60, 0x20);
        uart.handle_event(Uart16550Event::TransmitComplete { epoch: 0 });
        assert_eq!(uart.poll(), Uart16550Action::Transmit { byte: b'A' });
        assert_eq!(read(&mut uart, 5) & 0x60, 0x60);
    }

    #[test]
    fn receive_fifo_threshold_and_timeout_drive_irq() {
        let mut uart = uart();
        configure_9600_8n1(&mut uart);
        write(&mut uart, 2, 0x41);
        write(&mut uart, 1, 0x01);
        uart.receive_bytes(b"A").expect("input must fit");
        let Uart16550Action::Schedule {
            event: Uart16550Event::ReceiveComplete { epoch },
            ..
        } = uart.poll()
        else {
            panic!("receiver must schedule a character")
        };
        uart.handle_event(Uart16550Event::ReceiveComplete { epoch });
        let timeout = loop {
            if let Uart16550Action::Schedule {
                event: Uart16550Event::ReceiveTimeout { epoch, generation },
                ..
            } = uart.poll()
            {
                break (epoch, generation);
            }
        };
        uart.handle_event(Uart16550Event::ReceiveTimeout {
            epoch: timeout.0,
            generation: timeout.1,
        });
        assert_eq!(uart.interrupt_identification() & 0x0f, 0x0c);
        assert_eq!(read(&mut uart, 0), b'A');
    }

    #[test]
    fn loopback_returns_transmitted_byte_to_receiver() {
        let mut uart = uart();
        configure_9600_8n1(&mut uart);
        write(&mut uart, 4, 0x10);
        write(&mut uart, 0, b'X');
        let _ = uart.poll();
        uart.handle_event(Uart16550Event::TransmitComplete { epoch: 0 });
        assert!(matches!(
            uart.poll(),
            Uart16550Action::Schedule {
                event: Uart16550Event::ReceiveComplete { .. },
                ..
            }
        ));
        uart.handle_event(Uart16550Event::ReceiveComplete { epoch: 0 });
        assert_eq!(read(&mut uart, 0), b'X');
    }

    #[test]
    fn reset_invalidates_old_events() {
        let mut uart = uart();
        configure_9600_8n1(&mut uart);
        write(&mut uart, 0, b'A');
        let _ = uart.poll();
        uart.reset();
        uart.handle_event(Uart16550Event::TransmitComplete { epoch: 0 });
        assert_eq!(uart.poll(), Uart16550Action::Idle);
    }
}
