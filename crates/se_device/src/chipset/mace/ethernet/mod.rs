//! MACE MAC110 register state, filtering, and DMA-visible formats.

use std::collections::VecDeque;

/// MAC implementation revision reported by MACE 2.0.
pub const MAC_IMPLEMENTATION_REVISION: u32 = 1;

const DEFAULT_PHY_ADDRESS: u8 = 0;
const ICS1890_ID1: u16 = 0x0015;
const ICS1890_ID2: u16 = 0xf420;
const PHY_BUSY: u32 = 1 << 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum PhyOperation {
    Read,
    Write(u16),
}

/// Receive metadata used to form a MACE status vector.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ReceiveStatus {
    pub code_violation: bool,
    pub dribble: bool,
    pub crc_error: bool,
    pub multicast: bool,
    pub broadcast: bool,
    pub bad_packet: bool,
    pub filter_match: bool,
    pub physical_match: bool,
}

/// MACE Fast Ethernet interface.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceEthernet {
    pub mac_control: u32,
    pub interrupt_status: u32,
    pub dma_control: u16,
    pub interrupt_delay: u8,
    pub tx_info: u32,
    pub station_address: [u8; 6],
    pub secondary_address: [u8; 6],
    pub multicast_filter: u64,
    pub tx_ring_base: u32,
    pub rx_clusters: VecDeque<u32>,
    pub sequence: u8,
    pub last_tx_vector: u64,
    phy_address: u16,
    phy_data: u16,
    phy_busy: bool,
    phy_operation: Option<PhyOperation>,
    phy_registers: [u16; 32],
    backoff_state: u16,
}

impl MaceEthernet {
    pub fn new() -> Self {
        let mut phy_registers = [0; 32];
        phy_registers[2] = ICS1890_ID1;
        phy_registers[3] = ICS1890_ID2;
        Self {
            mac_control: 1 | MAC_IMPLEMENTATION_REVISION << 29,
            interrupt_status: 0,
            dma_control: 0,
            interrupt_delay: 0,
            tx_info: 0,
            station_address: [0; 6],
            secondary_address: [0; 6],
            multicast_filter: 0,
            tx_ring_base: 0,
            rx_clusters: VecDeque::with_capacity(16),
            sequence: 0,
            last_tx_vector: 0,
            phy_address: 0,
            phy_data: 0,
            phy_busy: false,
            phy_operation: None,
            phy_registers,
            backoff_state: 1,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
    pub const fn interrupt(&self) -> bool {
        self.interrupt_status & 0xff != 0
    }

    pub fn clear_interrupts(&mut self, mask: u32) {
        self.interrupt_status &= !(mask & 0xff);
    }

    pub const fn phy_data(&self) -> u32 {
        self.phy_data as u32 | if self.phy_busy { PHY_BUSY } else { 0 }
    }

    pub const fn phy_address(&self) -> u16 {
        self.phy_address
    }

    pub fn set_phy_address(&mut self, value: u16) {
        self.phy_address = value & 0x03ff;
    }

    pub fn start_phy_read(&mut self) {
        self.phy_busy = true;
        self.phy_operation = Some(PhyOperation::Read);
    }

    pub fn start_phy_write(&mut self, value: u16) {
        self.phy_data = value;
        self.phy_busy = true;
        self.phy_operation = Some(PhyOperation::Write(value));
    }

    pub fn complete_phy_operation(&mut self) {
        let Some(operation) = self.phy_operation.take() else {
            self.phy_busy = false;
            return;
        };
        let device = (self.phy_address >> 5) as u8;
        let register = usize::from(self.phy_address & 0x1f);
        match operation {
            PhyOperation::Read => {
                self.phy_data = if device == DEFAULT_PHY_ADDRESS {
                    self.phy_registers[register]
                } else {
                    u16::MAX
                };
            }
            PhyOperation::Write(value) if device == DEFAULT_PHY_ADDRESS => {
                if register == 0 && value & 0x8000 != 0 {
                    let mut registers = [0; 32];
                    registers[2] = ICS1890_ID1;
                    registers[3] = ICS1890_ID2;
                    self.phy_registers = registers;
                } else if !matches!(register, 2 | 3) {
                    self.phy_registers[register] = value;
                }
            }
            PhyOperation::Write(_) => {}
        }
        self.phy_busy = false;
    }

    pub fn seed_backoff(&mut self, value: u16) {
        self.backoff_state = value & 0x07ff;
    }

    pub fn push_receive_cluster(&mut self, address: u32) -> bool {
        if address & 0xfff != 0 || self.rx_clusters.len() == 16 {
            return false;
        }
        self.rx_clusters.push_back(address);
        true
    }

    pub fn accepts(&self, frame: &[u8]) -> bool {
        if frame.len() < 6 {
            return false;
        }
        let destination: [u8; 6] = frame[..6].try_into().expect("the length was checked");
        match (self.mac_control >> 5) & 3 {
            3 => true,
            0 => destination == self.station_address,
            1 => {
                destination == self.station_address
                    || destination == [0xff; 6]
                    || destination[0] & 1 != 0 && self.multicast_matches(destination)
            }
            2 => {
                destination == self.station_address
                    || destination == [0xff; 6]
                    || destination[0] & 1 != 0
            }
            _ => false,
        }
    }

    pub fn receive_status_vector(&mut self, frame: &[u8], status: ReceiveStatus) -> u64 {
        let sequence = self.sequence & 0x1f;
        self.sequence = self.sequence.wrapping_add(1) & 0x1f;
        let mut value = frame.len().min(u16::MAX as usize) as u64;
        value |= u64::from(status.code_violation) << 16;
        value |= u64::from(status.dribble) << 17;
        value |= u64::from(status.crc_error) << 18;
        value |= u64::from(status.multicast) << 19;
        value |= u64::from(status.broadcast) << 20;
        value |= u64::from(status.bad_packet) << 23;
        value |= u64::from(status.filter_match) << 25;
        value |= u64::from(status.physical_match) << 26;
        value |= u64::from(sequence) << 27;
        value |= u64::from(internet_checksum(frame)) << 32;
        value | 1 << 63
    }

    pub fn complete_transmit(&mut self, length: usize, collisions: u8, success: bool) -> u64 {
        let mut value = length.min(u16::MAX as usize) as u64;
        value |= u64::from(collisions.min(15)) << 16;
        value |= u64::from(success) << 23;
        value |= 1 << 63;
        self.last_tx_vector = value;
        value
    }

    pub fn next_backoff(&mut self) -> u16 {
        let feedback = (self.backoff_state ^ (self.backoff_state >> 2)) & 1;
        self.backoff_state = (self.backoff_state >> 1) | feedback << 10;
        self.backoff_state
    }

    fn multicast_matches(&self, address: [u8; 6]) -> bool {
        let index = (ethernet_crc32(&address) >> 26) as u8;
        self.multicast_filter & (1_u64 << index) != 0
    }
}

impl Default for MaceEthernet {
    fn default() -> Self {
        Self::new()
    }
}

/// Computes the receive DMA one's-complement carry sum.
pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0_u32;
    for chunk in data.chunks(2) {
        let word = u16::from_be_bytes([chunk[0], chunk.get(1).copied().unwrap_or(0)]);
        sum += u32::from(word);
        sum = (sum & 0xffff) + (sum >> 16);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum as u16
}

fn ethernet_crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        let mut value = byte;
        for _ in 0..8 {
            let mix = (crc ^ u32::from(value)) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0xedb8_8320;
            }
            value >>= 1;
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_and_broadcast_filters_work() {
        let mut ethernet = MaceEthernet::new();
        ethernet.station_address = [0, 1, 2, 3, 4, 5];
        ethernet.mac_control |= 1 << 5;
        assert!(ethernet.accepts(&[0, 1, 2, 3, 4, 5]));
        assert!(ethernet.accepts(&[0xff; 6]));
        assert!(!ethernet.accepts(&[0, 0, 0, 0, 0, 1]));
    }
    #[test]
    fn checksum_folds_carries() {
        assert_eq!(internet_checksum(&[0xff, 0xff, 0, 1]), 1);
    }

    #[test]
    fn mdio_reports_a_link_down_ics1890_and_absent_addresses() {
        let mut ethernet = MaceEthernet::new();
        ethernet.set_phy_address(2);
        ethernet.start_phy_read();
        assert_eq!(ethernet.phy_data(), PHY_BUSY);
        ethernet.complete_phy_operation();
        assert_eq!(ethernet.phy_data(), u32::from(ICS1890_ID1));

        ethernet.set_phy_address(3);
        ethernet.start_phy_read();
        ethernet.complete_phy_operation();
        assert_eq!(ethernet.phy_data(), u32::from(ICS1890_ID2));

        ethernet.set_phy_address((1 << 5) | 2);
        ethernet.start_phy_read();
        ethernet.complete_phy_operation();
        assert_eq!(ethernet.phy_data(), u32::from(u16::MAX));
    }
}
