//! Owned host-neutral media and peripheral link protocols.

use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;

/// External MACE port.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MediaPort {
    VideoInputAb,
    VideoInputCd,
    VideoOutput,
    AudioInput,
    AudioOutput1,
    AudioOutput2,
    Ethernet,
    Keyboard,
    Mouse,
    Serial0,
    Serial1,
    Parallel,
}

/// Packed video field supplied at the D1 boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoField {
    pub width: u16,
    pub height: u16,
    pub odd: bool,
    pub data: Vec<u8>,
}

/// Stereo sample block at the TDM boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioSampleBlock {
    pub sample_rate_hz: u32,
    pub samples: Vec<(i32, i32)>,
}

/// Ethernet frame at the MII boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EthernetFrame {
    pub data: Vec<u8>,
    pub crc_valid: bool,
    pub collision_count: u8,
}

/// Host-neutral data transported through a media link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaPayload {
    Video(VideoField),
    Audio(AudioSampleBlock),
    Ethernet(EthernetFrame),
    Bytes(Vec<u8>),
    Sync { asserted: bool },
}

/// Media link transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaTransaction {
    pub source: ComponentId,
    pub target: ComponentId,
    pub port: MediaPort,
    pub payload: MediaPayload,
}

/// Immediate point-to-point media bus action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaBusAction {
    Deliver {
        target: ComponentId,
        transaction: MediaTransaction,
    },
    Idle,
}

/// Ordered point-to-point media link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaBus {
    id: ComponentId,
    name: String,
    queue: VecDeque<MediaBusAction>,
}

impl MediaBus {
    pub fn new(id: ComponentId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            queue: VecDeque::new(),
        }
    }
    pub fn poll(&mut self) -> MediaBusAction {
        self.queue.pop_front().unwrap_or(MediaBusAction::Idle)
    }
}

impl Component for MediaBus {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {
        self.queue.clear();
    }
}

impl BusRole<MediaTransaction> for MediaBus {
    type Response = ();
    fn route(&mut self, transaction: MediaTransaction) {
        self.queue.push_back(MediaBusAction::Deliver {
            target: transaction.target,
            transaction,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use se_core::role::BusRole;
    #[test]
    fn media_delivery_preserves_payload() {
        let target = ComponentId::new(2);
        let transaction = MediaTransaction {
            source: ComponentId::new(1),
            target,
            port: MediaPort::Keyboard,
            payload: MediaPayload::Bytes(vec![0xaa]),
        };
        let mut bus = MediaBus::new(ComponentId::new(3), "PS/2");
        bus.route(transaction.clone());
        assert_eq!(
            bus.poll(),
            MediaBusAction::Deliver {
                target,
                transaction
            }
        );
    }
}
