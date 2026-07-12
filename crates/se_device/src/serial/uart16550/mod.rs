//! Deterministic 16550-compatible UART.

use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;

use crate::bus::irq::{IrqOutput, IrqSource, IrqTransaction};
use crate::bus::isa::{
    IsaBusError, IsaCompletion, IsaCompletionPayload, IsaDeviceResponse, IsaTransaction,
    IsaTransfer,
};

/// Interrupt output driven by a UART.
pub const UART16550_IRQ_OUTPUT: IrqOutput = IrqOutput::new(0);

const FIFO_CAPACITY: usize = 16;

/// Observable UART action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Uart16550Action {
    SetIrq(IrqTransaction),
    Idle,
}

/// Software-visible 16550 register file and FIFOs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Uart16550 {
    id: ComponentId,
    name: String,
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
    irq_asserted: bool,
    actions: VecDeque<Uart16550Action>,
}

impl Uart16550 {
    /// Creates a reset UART.
    pub fn new(id: ComponentId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            divisor: 0,
            interrupt_enable: 0,
            fifo_control: 0,
            line_control: 0,
            modem_control: 0,
            line_status_errors: 0,
            modem_status: 0,
            scratch: 0,
            infrared_control: 0,
            receive: VecDeque::new(),
            transmit: VecDeque::new(),
            irq_asserted: false,
            actions: VecDeque::new(),
        }
    }

    /// Queues externally received bytes in order.
    pub fn receive_bytes(&mut self, bytes: &[u8]) -> usize {
        let available = FIFO_CAPACITY.saturating_sub(self.receive.len());
        let accepted = available.min(bytes.len());
        self.receive.extend(bytes[..accepted].iter().copied());
        if accepted != bytes.len() {
            self.line_status_errors |= 1 << 1;
        }
        self.update_irq();
        accepted
    }

    /// Removes the oldest byte emitted by software.
    pub fn poll_transmit(&mut self) -> Option<u8> {
        let byte = self.transmit.pop_front();
        self.update_irq();
        byte
    }

    /// Polls one interrupt line change.
    pub fn poll(&mut self) -> Uart16550Action {
        self.actions.pop_front().unwrap_or(Uart16550Action::Idle)
    }

    fn read(&mut self, register: u8) -> Result<u8, IsaBusError> {
        let dlab = self.line_control & 0x80 != 0;
        let value = match (register & 7, dlab) {
            (0, true) => self.divisor as u8,
            (1, true) => (self.divisor >> 8) as u8,
            (7, true) => self.infrared_control,
            (0, false) => self.receive.pop_front().unwrap_or(0),
            (1, false) => self.interrupt_enable,
            (2, false) => self.interrupt_identification(),
            (3, false) => self.line_control,
            (4, false) => self.modem_control,
            (5, false) => self.line_status(),
            (6, false) => self.modem_status,
            (7, false) => self.scratch,
            _ => return Err(IsaBusError::Address),
        };
        self.update_irq();
        Ok(value)
    }

    fn write(&mut self, register: u8, value: u8) -> Result<(), IsaBusError> {
        let dlab = self.line_control & 0x80 != 0;
        match (register & 7, dlab) {
            (0, true) => self.divisor = self.divisor & 0xff00 | u16::from(value),
            (1, true) => self.divisor = self.divisor & 0x00ff | u16::from(value) << 8,
            (7, true) => self.infrared_control = value,
            (0, false) => {
                if self.transmit.len() >= FIFO_CAPACITY {
                    return Err(IsaBusError::Access);
                }
                self.transmit.push_back(value);
            }
            (1, false) => self.interrupt_enable = value & 0x0f,
            (2, false) => {
                self.fifo_control = value & 0xc9;
                if value & 0x02 != 0 {
                    self.receive.clear();
                    self.line_status_errors = 0;
                }
                if value & 0x04 != 0 {
                    self.transmit.clear();
                }
            }
            (3, false) => self.line_control = value,
            (4, false) => self.modem_control = value & 0x1f,
            (7, false) => self.scratch = value,
            (5 | 6, false) => return Err(IsaBusError::Access),
            _ => return Err(IsaBusError::Address),
        }
        self.update_irq();
        Ok(())
    }

    fn line_status(&self) -> u8 {
        u8::from(!self.receive.is_empty())
            | self.line_status_errors
            | (u8::from(self.transmit.is_empty()) << 5)
            | (u8::from(self.transmit.is_empty()) << 6)
    }

    fn interrupt_identification(&self) -> u8 {
        let identifier = if self.line_status_errors != 0 && self.interrupt_enable & 0x04 != 0 {
            0x06
        } else if !self.receive.is_empty() && self.interrupt_enable & 0x01 != 0 {
            0x04
        } else if self.transmit.is_empty() && self.interrupt_enable & 0x02 != 0 {
            0x02
        } else if self.modem_status & 0x0f != 0 && self.interrupt_enable & 0x08 != 0 {
            0x00
        } else {
            0x01
        };
        identifier | (u8::from(self.fifo_control & 1 != 0) * 0xc0)
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
}

impl Component for Uart16550 {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {
        *self = Self::new(self.id, self.name.clone());
    }
}

impl BusDeviceRole<IsaTransaction> for Uart16550 {
    type Response = IsaDeviceResponse;

    fn accept(&mut self, transaction: IsaTransaction) -> Self::Response {
        let result = match transaction.transfer {
            IsaTransfer::Read { length: 1 } => self
                .read(transaction.address as u8)
                .map(|value| IsaCompletionPayload::ReadData(vec![value])),
            IsaTransfer::Write { data, byte_enable }
                if data.len() == 1 && byte_enable == [true] =>
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
    use se_core::scheduler::SimTime;

    use super::*;
    use crate::bus::isa::IsaTransactionId;

    #[test]
    fn fifo_clear_write_used_by_the_ip32_prom_is_accepted() {
        let mut uart = Uart16550::new(ComponentId::new(1), "UART");
        let response = uart.accept(IsaTransaction {
            id: IsaTransactionId::new(1),
            time: SimTime::ZERO,
            controller: ComponentId::new(2),
            target: ComponentId::new(1),
            address: 2,
            transfer: IsaTransfer::Write {
                data: vec![6],
                byte_enable: vec![true],
            },
        });
        assert!(matches!(
            response,
            IsaDeviceResponse::Complete(IsaCompletion {
                result: Ok(IsaCompletionPayload::WriteComplete),
                ..
            })
        ));
    }
}
