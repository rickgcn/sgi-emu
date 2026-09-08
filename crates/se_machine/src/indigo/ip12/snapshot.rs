//! Restorable execution state for one cold-constructed Indigo IP12.

use se_cpu::mips1::r3000::R3000Snapshot;
use serde::{Deserialize, Serialize};

use super::bus::Ip12BusSnapshot;
use super::{Ip12, Ip12SnapshotError};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Ip12Snapshot {
    cpu: R3000Snapshot,
    bus: Ip12BusSnapshot,
}

impl Ip12 {
    pub(crate) fn snapshot(&self) -> Result<Ip12Snapshot, Ip12SnapshotError> {
        Ok(Ip12Snapshot {
            cpu: self.cpu.snapshot(),
            bus: self.bus.snapshot()?,
        })
    }

    pub(crate) fn restore_snapshot(
        &mut self,
        snapshot: Ip12Snapshot,
    ) -> Result<(), Ip12SnapshotError> {
        self.bus.restore_snapshot(snapshot.bus)?;
        self.cpu.restore_snapshot(snapshot.cpu);
        self.update_cp0_condition();
        Ok(())
    }
}
