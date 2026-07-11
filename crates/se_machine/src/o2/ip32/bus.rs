//! IP32 CPU physical address bus and unimplemented ASIC targets.

use se_core::component::{Component, ComponentId};
use se_core::role::{BusDeviceRole, BusRole};
use se_device::cpu::execution::protocol::ExecutionTransaction;
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};

use super::address_map::{Ip32AddressResolution, Ip32PhysicalRegion, resolve};

/// Behavior selected for mapped but unimplemented device accesses.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Ip32UnimplementedAccessPolicy {
    /// Raise a bus error and preserve the missing behavior as visible failure.
    #[default]
    Strict,

    /// Read zero and ignore writes while retaining trace visibility.
    Permissive,
}

/// Routed IP32 CPU transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ip32BusRoute {
    /// A transaction routed to RAM or PROM.
    Memory {
        /// Named physical region.
        region: Ip32PhysicalRegion,
        /// Target memory component.
        target: ComponentId,
        /// Device-local byte offset.
        offset: u64,
        /// Whether ECC checking is bypassed for this alias.
        no_ecc: bool,
        /// Original correlated CPU transaction.
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
    },

    /// A transaction routed to an unimplemented ASIC stub.
    Stub {
        /// Named physical region.
        region: Ip32PhysicalRegion,
        /// Target ASIC component.
        target: ComponentId,
        /// Original correlated CPU transaction.
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
    },

    /// A transaction that has no implemented responder.
    Unmapped {
        /// Known region containing the first byte, when applicable.
        region: Option<Ip32PhysicalRegion>,
        /// Original correlated CPU transaction.
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
    },
}

/// CPU-facing IP32 physical address bus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32CpuAddressBus {
    id: ComponentId,
    name: String,
    ram_size_bytes: u64,
}

impl Ip32CpuAddressBus {
    /// Creates a CPU address bus for the installed RAM size.
    pub fn new(id: ComponentId, name: impl Into<String>, ram_size_bytes: u64) -> Self {
        Self {
            id,
            name: name.into(),
            ram_size_bytes,
        }
    }

    /// Returns the installed RAM size used by address classification.
    pub const fn ram_size_bytes(&self) -> u64 {
        self.ram_size_bytes
    }
}

impl Component for Ip32CpuAddressBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {}
}

impl BusRole<ExecutionTransaction<Mips4ExecutionTransaction>> for Ip32CpuAddressBus {
    type Response = Ip32BusRoute;

    fn route(
        &mut self,
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
    ) -> Self::Response {
        let (physical_address, size) = transaction_shape(transaction.payload);
        match resolve(physical_address, size, self.ram_size_bytes) {
            Ip32AddressResolution::Memory {
                region,
                target,
                offset,
                no_ecc,
            } => Ip32BusRoute::Memory {
                region,
                target,
                offset,
                no_ecc,
                transaction,
            },
            Ip32AddressResolution::Stub { region, target } => Ip32BusRoute::Stub {
                region,
                target,
                transaction,
            },
            Ip32AddressResolution::Unmapped { region } => Ip32BusRoute::Unmapped {
                region,
                transaction,
            },
        }
    }
}

/// Placeholder for an IP32 ASIC whose register semantics are not implemented.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32MmioStub {
    id: ComponentId,
    name: String,
    policy: Ip32UnimplementedAccessPolicy,
}

impl Ip32MmioStub {
    /// Creates an unimplemented MMIO target.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        policy: Ip32UnimplementedAccessPolicy,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            policy,
        }
    }

    /// Returns the selected unimplemented-access policy.
    pub const fn policy(&self) -> Ip32UnimplementedAccessPolicy {
        self.policy
    }
}

impl Component for Ip32MmioStub {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {}
}

impl BusDeviceRole<Mips4ExecutionTransaction> for Ip32MmioStub {
    type Response = Mips4ExecutionCompletion;

    fn accept(&mut self, transaction: Mips4ExecutionTransaction) -> Self::Response {
        match self.policy {
            Ip32UnimplementedAccessPolicy::Strict => Mips4ExecutionCompletion::BusError,
            Ip32UnimplementedAccessPolicy::Permissive => match transaction {
                Mips4ExecutionTransaction::Read { .. } => Mips4ExecutionCompletion::ReadData(0),
                Mips4ExecutionTransaction::Write { .. } => Mips4ExecutionCompletion::WriteComplete,
            },
        }
    }
}

const fn transaction_shape(transaction: Mips4ExecutionTransaction) -> (u64, u8) {
    match transaction {
        Mips4ExecutionTransaction::Read {
            physical_address,
            size,
            ..
        }
        | Mips4ExecutionTransaction::Write {
            physical_address,
            size,
            ..
        } => (physical_address, size.bytes()),
    }
}

#[cfg(test)]
mod tests;
