//! Host-neutral IEEE 1284 register endpoint.

use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;

use crate::bus::irq::{IrqOutput, IrqSource, IrqTransaction};
use crate::bus::isa::{
    IsaBusError, IsaCompletion, IsaCompletionPayload, IsaDeviceResponse, IsaTransaction,
    IsaTransferView,
};

/// Parallel-port interrupt output.
pub const IEEE1284_IRQ_OUTPUT: IrqOutput = IrqOutput::new(0);

/// Observable parallel-port action.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ieee1284Action {
    SetIrq(IrqTransaction),
    Idle,
}

/// Software-visible byte registers for the external Super I/O parallel port.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ieee1284 {
    id: ComponentId,
    name: String,
    data: u8,
    status: u8,
    control: u8,
    epp_address: u8,
    epp_data: u8,
    irq_asserted: bool,
    actions: VecDeque<Ieee1284Action>,
}

se_core::component_state!(Ieee1284State, Ieee1284);

impl Ieee1284 {
    /// Creates a reset parallel port.
    pub fn new(id: ComponentId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            data: 0,
            status: 0x80,
            control: 0x0c,
            epp_address: 0,
            epp_data: 0,
            irq_asserted: false,
            actions: VecDeque::new(),
        }
    }

    /// Updates externally driven status inputs.
    pub fn set_status(&mut self, status: u8) {
        self.status = status;
        self.update_irq();
    }

    /// Polls one interrupt line transition.
    pub fn poll(&mut self) -> Ieee1284Action {
        self.actions.pop_front().unwrap_or(Ieee1284Action::Idle)
    }

    fn update_irq(&mut self) {
        let asserted = self.control & 0x10 != 0 && self.status & 0x40 == 0;
        if asserted == self.irq_asserted {
            return;
        }
        self.irq_asserted = asserted;
        self.actions
            .push_back(Ieee1284Action::SetIrq(IrqTransaction {
                source: IrqSource {
                    component: self.id,
                    output: IEEE1284_IRQ_OUTPUT,
                },
                asserted,
            }));
    }
}

impl Component for Ieee1284 {
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

impl BusDeviceRole<IsaTransaction> for Ieee1284 {
    type Response = IsaDeviceResponse;

    fn accept(&mut self, transaction: IsaTransaction) -> Self::Response {
        let register = transaction.address as u8 & 7;
        let result = match transaction.transfer.view() {
            IsaTransferView::Read { length: 1 } => Ok(IsaCompletionPayload::ReadData(
                [match register {
                    0 => self.data,
                    1 => self.status,
                    2 => self.control,
                    3 => self.epp_address,
                    4 => self.epp_data,
                    _ => 0xff,
                }]
                .into(),
            )),
            IsaTransferView::Write { data, byte_enable }
                if data.len() == 1 && byte_enable.iter().eq([true]) =>
            {
                match register {
                    0 => self.data = data[0],
                    2 => {
                        self.control = data[0];
                        self.update_irq();
                    }
                    3 => self.epp_address = data[0],
                    4 => self.epp_data = data[0],
                    1 => {
                        return IsaDeviceResponse::Complete(IsaCompletion {
                            id: transaction.id,
                            result: Err(IsaBusError::Access),
                        });
                    }
                    _ => {}
                }
                Ok(IsaCompletionPayload::WriteComplete)
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
    #[test]
    fn interrupt_follows_ack_and_enable() {
        let mut port = Ieee1284::new(ComponentId::new(1), "parallel");
        port.control |= 0x10;
        port.set_status(0);
        assert!(matches!(
            port.poll(),
            Ieee1284Action::SetIrq(IrqTransaction { asserted: true, .. })
        ));
    }
}
