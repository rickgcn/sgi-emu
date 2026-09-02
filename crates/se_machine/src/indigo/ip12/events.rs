//! Private event bookkeeping for the Indigo IP12.

use se_core::time::{VirtualDuration, VirtualInstant};

const EVENT_COUNT: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EventKind {
    Int2,
    Rtc,
    Serial0,
    Serial1,
    Scsi,
}

impl EventKind {
    const ALL: [Self; EVENT_COUNT] = [
        Self::Int2,
        Self::Rtc,
        Self::Serial0,
        Self::Serial1,
        Self::Scsi,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Int2 => 0,
            Self::Rtc => 1,
            Self::Serial0 => 2,
            Self::Serial1 => 3,
            Self::Scsi => 4,
        }
    }
}

#[derive(Clone, Copy)]
struct EventSlot {
    synchronized_at: VirtualInstant,
    deadline: Option<VirtualInstant>,
}

impl EventSlot {
    const EMPTY: Self = Self {
        synchronized_at: VirtualInstant::ZERO,
        deadline: None,
    };
}

pub(super) struct Ip12Events {
    now: VirtualInstant,
    slots: [EventSlot; EVENT_COUNT],
}

impl Ip12Events {
    pub(super) const fn new() -> Self {
        Self {
            now: VirtualInstant::ZERO,
            slots: [EventSlot::EMPTY; EVENT_COUNT],
        }
    }

    pub(super) fn advance(&mut self, elapsed: VirtualDuration) {
        self.now.advance(elapsed);
    }

    pub(super) fn schedule(&mut self, kind: EventKind, after: Option<VirtualDuration>) {
        self.slots[kind.index()].deadline = after.map(|after| {
            let mut deadline = self.now;
            deadline.advance(after);
            deadline
        });
    }

    pub(super) fn take_due(&mut self) -> Option<EventKind> {
        for kind in EventKind::ALL {
            let slot = &mut self.slots[kind.index()];
            if slot.deadline.is_some_and(|deadline| deadline <= self.now) {
                slot.deadline = None;
                return Some(kind);
            }
        }
        None
    }

    pub(super) fn synchronize(&mut self, kind: EventKind) -> VirtualDuration {
        let slot = &mut self.slots[kind.index()];
        let elapsed = self.now.duration_since(slot.synchronized_at);
        slot.synchronized_at = self.now;
        elapsed
    }

    pub(super) fn reset(&mut self) {
        self.now = VirtualInstant::ZERO;
        self.slots = [EventSlot::EMPTY; EVENT_COUNT];
    }
}

#[cfg(test)]
mod tests {
    use se_core::time::VirtualDuration;

    use super::{EventKind, Ip12Events};

    #[test]
    fn due_events_follow_the_fixed_order() {
        let mut events = Ip12Events::new();
        for kind in [EventKind::Scsi, EventKind::Serial1, EventKind::Rtc] {
            events.schedule(kind, Some(VirtualDuration::ZERO));
        }

        assert_eq!(events.take_due(), Some(EventKind::Rtc));
        assert_eq!(events.take_due(), Some(EventKind::Serial1));
        assert_eq!(events.take_due(), Some(EventKind::Scsi));
        assert_eq!(events.take_due(), None);
    }

    #[test]
    fn synchronization_tracks_each_device_independently() {
        let mut events = Ip12Events::new();
        events.advance(VirtualDuration::from_attoseconds(7));

        assert_eq!(
            events.synchronize(EventKind::Rtc),
            VirtualDuration::from_attoseconds(7)
        );
        events.advance(VirtualDuration::from_attoseconds(5));
        assert_eq!(
            events.synchronize(EventKind::Rtc),
            VirtualDuration::from_attoseconds(5)
        );
        assert_eq!(
            events.synchronize(EventKind::Int2),
            VirtualDuration::from_attoseconds(12)
        );
    }

    #[test]
    fn rescheduling_and_cancellation_replace_the_old_deadline() {
        let mut events = Ip12Events::new();
        events.schedule(EventKind::Rtc, Some(VirtualDuration::from_attoseconds(10)));
        events.schedule(EventKind::Rtc, None);
        events.advance(VirtualDuration::from_attoseconds(10));
        assert_eq!(events.take_due(), None);

        events.schedule(EventKind::Rtc, Some(VirtualDuration::ZERO));
        assert_eq!(events.take_due(), Some(EventKind::Rtc));
    }

    #[test]
    fn reset_returns_all_slots_to_the_origin() {
        let mut events = Ip12Events::new();
        events.advance(VirtualDuration::from_attoseconds(11));
        events.schedule(EventKind::Scsi, Some(VirtualDuration::ZERO));
        events.reset();

        assert_eq!(events.take_due(), None);
        assert_eq!(events.synchronize(EventKind::Scsi), VirtualDuration::ZERO);
    }
}
