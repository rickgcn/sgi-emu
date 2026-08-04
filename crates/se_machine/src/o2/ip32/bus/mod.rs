//! SGI O2 IP32 SysAD protocol mapping and stateless board endpoints.

use se_core::component::{
    Component, ComponentId, ComponentStateError, validate_component_state_id,
};
use se_core::scheduler::SimTime;
use se_device::chipset::crime::protocol::{
    CrimeByteEnable, CrimeCompletionPayload, CrimeData, CrimeSysAdCompletion, CrimeSysAdRequest,
    CrimeTransactionId, CrimeTransfer,
};
use se_device::cpu::execution::protocol::{
    ExecutionCompletion, ExecutionTransaction, ExecutionTransactionId,
};
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};

/// Fixed CPU-to-CRIME SysAD wiring and protocol mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32SysAdBus {
    id: ComponentId,
    name: String,
    controller: ComponentId,
    target: ComponentId,
}

/// Serializable identity proof for the fixed SysAD wiring.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct Ip32SysAdBusState {
    id: ComponentId,
    controller: ComponentId,
    target: ComponentId,
}

impl Ip32SysAdBus {
    /// Creates a SysAD protocol adapter connecting one CPU to CRIME.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        controller: ComponentId,
        target: ComponentId,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            controller,
            target,
        }
    }

    /// Captures the fixed endpoint identities.
    pub const fn save_state(&self) -> Ip32SysAdBusState {
        Ip32SysAdBusState {
            id: self.id,
            controller: self.controller,
            target: self.target,
        }
    }

    /// Validates that serialized state belongs to the same fixed wiring.
    pub fn restore_state(&mut self, state: Ip32SysAdBusState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        for (matches, field) in [
            (state.controller == self.controller, "SysAD controller"),
            (state.target == self.target, "SysAD target"),
        ] {
            if !matches {
                return Err(ComponentStateError::ConfigurationMismatch {
                    component: self.id,
                    field,
                });
            }
        }
        Ok(())
    }

    /// Returns the CPU endpoint.
    pub const fn controller(&self) -> ComponentId {
        self.controller
    }

    /// Returns the CRIME endpoint.
    pub const fn target(&self) -> ComponentId {
        self.target
    }

    /// Maps one CPU execution transaction to the CRIME SysAD protocol.
    pub fn translate_request(
        &self,
        transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
        time: SimTime,
    ) -> CrimeSysAdRequest {
        let (address, transfer) = match transaction.payload {
            Mips4ExecutionTransaction::Read {
                physical_address,
                size,
                ..
            } => (
                physical_address,
                CrimeTransfer::read(u16::from(size.bytes())),
            ),
            Mips4ExecutionTransaction::Write {
                physical_address,
                size,
                data,
                byte_enable,
                ..
            } => {
                let length = usize::from(size.bytes());
                (
                    physical_address,
                    CrimeTransfer::write(
                        data.to_le_bytes()[..length].iter().copied().collect(),
                        (0..length)
                            .map(|lane| byte_enable & (1 << lane) != 0)
                            .collect::<CrimeByteEnable>(),
                    ),
                )
            }
        };
        CrimeSysAdRequest {
            id: Self::crime_transaction_id(transaction.id),
            time,
            address,
            transfer,
        }
    }

    /// Maps a correlated CRIME response to the CPU execution protocol.
    pub fn translate_completion(
        &self,
        transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
        completion: CrimeSysAdCompletion,
    ) -> Option<ExecutionCompletion<Mips4ExecutionCompletion>> {
        if completion.id != Self::crime_transaction_id(transaction.id) {
            return None;
        }
        let payload = match (transaction.payload, completion.result) {
            (
                Mips4ExecutionTransaction::Read { size, .. },
                Ok(CrimeCompletionPayload::ReadData(data)),
            ) if data.len() == usize::from(size.bytes()) => Self::pack_read_data(&data)
                .map(Mips4ExecutionCompletion::ReadData)
                .unwrap_or(Mips4ExecutionCompletion::BusError),
            (
                Mips4ExecutionTransaction::Write { .. },
                Ok(CrimeCompletionPayload::WriteComplete),
            ) => Mips4ExecutionCompletion::WriteComplete,
            (_, Err(_))
            | (_, Ok(CrimeCompletionPayload::ReadData(_)))
            | (_, Ok(CrimeCompletionPayload::WriteComplete)) => Mips4ExecutionCompletion::BusError,
        };
        Some(ExecutionCompletion {
            id: transaction.id,
            payload,
        })
    }

    fn pack_read_data(data: &CrimeData) -> Option<u64> {
        if data.len() > 8 {
            return None;
        }
        let mut lanes = [0; 8];
        lanes[..data.len()].copy_from_slice(data);
        Some(u64::from_le_bytes(lanes))
    }

    const fn crime_transaction_id(id: ExecutionTransactionId) -> CrimeTransactionId {
        CrimeTransactionId::new(id.get())
    }
}

impl Component for Ip32SysAdBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {}
}

/// Identity-only endpoint for an unimplemented board device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32StubEndpoint {
    id: ComponentId,
    name: String,
}

/// Serializable identity proof for a stateless IP32 endpoint.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct Ip32StubEndpointState {
    id: ComponentId,
}

impl Ip32StubEndpoint {
    /// Creates an identity-only endpoint.
    pub fn new(id: ComponentId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }

    /// Captures the endpoint identity.
    pub const fn save_state(&self) -> Ip32StubEndpointState {
        Ip32StubEndpointState { id: self.id }
    }

    /// Validates endpoint identity; this component has no dynamic state.
    pub fn restore_state(
        &mut self,
        state: Ip32StubEndpointState,
    ) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)
    }
}

impl Component for Ip32StubEndpoint {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests;
