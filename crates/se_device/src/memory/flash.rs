//! Read-array system flash endpoint for byte-oriented buses.

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;

use crate::bus::isa::{
    IsaBusError, IsaCompletion, IsaCompletionPayload, IsaDeviceResponse, IsaTransaction,
    IsaTransfer,
};

/// System flash endpoint whose programming command set is intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadArrayFlash {
    id: ComponentId,
    name: String,
    bytes: Vec<u8>,
}

impl ReadArrayFlash {
    /// Creates a flash device in read-array mode.
    pub fn new(id: ComponentId, name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            id,
            name: name.into(),
            bytes,
        }
    }

    /// Returns the physical flash capacity.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether the flash contains no bytes.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl Component for ReadArrayFlash {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {}
}

impl BusDeviceRole<IsaTransaction> for ReadArrayFlash {
    type Response = IsaDeviceResponse;

    fn accept(&mut self, transaction: IsaTransaction) -> Self::Response {
        let result = match transaction.transfer {
            IsaTransfer::Read { length } if !self.bytes.is_empty() => {
                let start = transaction.address as usize % self.bytes.len();
                let length = usize::from(length);
                if start
                    .checked_add(length)
                    .is_some_and(|end| end <= self.bytes.len())
                {
                    Ok(IsaCompletionPayload::ReadData(
                        self.bytes[start..start + length].to_vec(),
                    ))
                } else {
                    Err(IsaBusError::Address)
                }
            }
            IsaTransfer::Read { .. } => Err(IsaBusError::Address),
            IsaTransfer::Write { .. } => Err(IsaBusError::ReadOnly),
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
    use crate::bus::isa::IsaTransactionId;
    use se_core::scheduler::SimTime;

    #[test]
    fn reads_and_mirrors_at_the_target_supplied_offset() {
        let mut flash = ReadArrayFlash::new(ComponentId::new(1), "flash", vec![1, 2, 3, 4]);
        let response = flash.accept(IsaTransaction {
            id: IsaTransactionId::new(1),
            time: SimTime::ZERO,
            controller: ComponentId::new(2),
            target: ComponentId::new(1),
            address: 1,
            transfer: IsaTransfer::Read { length: 2 },
        });
        assert!(
            matches!(response, IsaDeviceResponse::Complete(IsaCompletion { result: Ok(IsaCompletionPayload::ReadData(data)), .. }) if data == [2, 3])
        );
    }
}
