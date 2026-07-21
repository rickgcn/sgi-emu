//! MACE interrupt source aggregation and CRIME posting.

/// CRIME interrupt slot assigned to a MACE group.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[repr(u8)]
pub enum MaceInterruptGroup {
    VideoInput1 = 0,
    VideoInput2 = 1,
    VideoOutput = 2,
    Ethernet = 3,
    SerialParallel = 4,
    Miscellaneous = 5,
    Audio = 6,
    PciError = 7,
    Pci0 = 8,
    Pci1 = 9,
    Pci2 = 10,
    Pci3 = 11,
    Pci4 = 12,
    Pci5 = 13,
    Pci6 = 14,
    Pci7 = 15,
}

/// Internal interrupt state and last values posted to CRIME.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaceInterruptController {
    peripheral_status: u32,
    peripheral_mask: u32,
    groups: u16,
    posted: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct MaceInterruptControllerState {
    peripheral_status: u32,
    peripheral_mask: u32,
    groups: u16,
    posted: u16,
}

impl MaceInterruptController {
    pub const fn new() -> Self {
        Self {
            peripheral_status: 0,
            peripheral_mask: 0,
            groups: 0,
            posted: 0,
        }
    }

    pub(super) fn save_state(&self) -> MaceInterruptControllerState {
        MaceInterruptControllerState {
            peripheral_status: self.peripheral_status,
            peripheral_mask: self.peripheral_mask,
            groups: self.groups,
            posted: self.posted,
        }
    }

    pub(super) fn validate_state(state: &MaceInterruptControllerState) -> Result<(), &'static str> {
        let active = state.peripheral_status & state.peripheral_mask;
        let expected = u16::from(active & 0x0000_00ff != 0) << MaceInterruptGroup::Audio as u8
            | u16::from(active & 0x0000_ff00 != 0) << MaceInterruptGroup::Miscellaneous as u8
            | u16::from(active & 0xffff_0000 != 0) << MaceInterruptGroup::SerialParallel as u8;
        let peripheral_groups = (1 << MaceInterruptGroup::Audio as u8)
            | (1 << MaceInterruptGroup::Miscellaneous as u8)
            | (1 << MaceInterruptGroup::SerialParallel as u8);
        if state.groups & peripheral_groups != expected {
            return Err("MACE peripheral interrupt groups must match status and mask registers");
        }
        Ok(())
    }

    pub(super) fn restore_state(&mut self, state: MaceInterruptControllerState) {
        self.peripheral_status = state.peripheral_status;
        self.peripheral_mask = state.peripheral_mask;
        self.groups = state.groups;
        self.posted = state.posted;
    }

    pub const fn peripheral_status(&self) -> u32 {
        self.peripheral_status
    }
    pub const fn peripheral_mask(&self) -> u32 {
        self.peripheral_mask
    }

    pub fn set_peripheral_mask(&mut self, mask: u32) {
        self.peripheral_mask = mask;
        self.recompute_peripheral_groups();
    }

    pub fn set_peripheral_source(&mut self, bit: u8, asserted: bool) {
        if bit >= 32 {
            return;
        }
        let mask = 1_u32 << bit;
        if asserted {
            self.peripheral_status |= mask;
        } else {
            self.peripheral_status &= !mask;
        }
        self.recompute_peripheral_groups();
    }

    pub fn clear_edge_sources(&mut self, mask: u32) {
        const WRITABLE_EDGE_MASK: u32 = (1 << 16) | (1 << 22) | (1 << 28);
        self.peripheral_status &= !(mask & WRITABLE_EDGE_MASK);
        self.recompute_peripheral_groups();
    }

    pub fn set_group(&mut self, group: MaceInterruptGroup, asserted: bool) {
        let mask = 1_u16 << group as u8;
        if asserted {
            self.groups |= mask;
        } else {
            self.groups &= !mask;
        }
    }

    pub fn take_changed_posts(&mut self) -> Vec<(u8, bool)> {
        let changed = self.groups ^ self.posted;
        let mut posts = Vec::new();
        for bit in 0..16 {
            if changed & (1 << bit) != 0 {
                posts.push((bit, self.groups & (1 << bit) != 0));
            }
        }
        self.posted = self.groups;
        posts
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn recompute_peripheral_groups(&mut self) {
        let active = self.peripheral_status & self.peripheral_mask;
        self.set_group(MaceInterruptGroup::Audio, active & 0x0000_00ff != 0);
        self.set_group(MaceInterruptGroup::Miscellaneous, active & 0x0000_ff00 != 0);
        self.set_group(
            MaceInterruptGroup::SerialParallel,
            active & 0xffff_0000 != 0,
        );
    }
}

impl Default for MaceInterruptController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peripheral_sources_map_to_three_crime_groups() {
        let mut interrupts = MaceInterruptController::new();
        interrupts.set_peripheral_mask(u32::MAX);
        interrupts.set_peripheral_source(0, true);
        interrupts.set_peripheral_source(9, true);
        interrupts.set_peripheral_source(20, true);
        assert_eq!(
            interrupts.take_changed_posts(),
            vec![(4, true), (5, true), (6, true)]
        );
    }
}
