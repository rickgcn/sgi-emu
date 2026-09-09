//! Device-independent coordination for the SGI GIO bus.
//!
//! The bus owns the devices installed in the three physical slots, routes
//! slot-local transactions, combines interrupt levels, and preserves the
//! configured topology while restoring device state.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use se_core::bus::{BusError, DeviceAddr};
use se_core::time::VirtualDuration;
use serde::{Deserialize, Serialize};

use crate::lg1::Lg1;

/// A physical GIO slot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GioSlot {
    /// Expansion slot zero.
    Slot0,
    /// Expansion slot one.
    Slot1,
    /// Primary graphics slot.
    Graphics,
}

impl GioSlot {
    const fn index(self) -> usize {
        match self {
            Self::Slot0 => 0,
            Self::Slot1 => 1,
            Self::Graphics => 2,
        }
    }
}

/// One of the three shared GIO interrupt levels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GioInterrupt {
    /// GIO interrupt level zero.
    Interrupt0,
    /// GIO interrupt level one.
    Interrupt1,
    /// GIO interrupt level two.
    Interrupt2,
}

/// Display output driven by one GIO device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GioDisplayState {
    /// No valid video signal is present.
    NoSignal,
    /// Video timing is valid but no picture is available.
    Blank,
    /// A complete RGBA8888 frame is available.
    Active {
        /// Displayed width in pixels.
        width: u32,
        /// Displayed height in pixels.
        height: u32,
        /// Tightly packed pixels in top-row-first order.
        pixels: Arc<Vec<u8>>,
    },
}

/// A functional device attached to a [`GioBus`].
pub trait GioDevice: Send {
    /// Restores device reset state.
    fn reset(&mut self);

    /// Reads one transaction without changing device state.
    fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError>;

    /// Reads one device-local transaction.
    fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError>;

    /// Writes one device-local transaction.
    fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError>;

    /// Reads one DMA burst whose address remains asserted for the whole stream.
    ///
    /// Unlike MMIO, successive bytes do not select successive device
    /// addresses. The device interprets the byte stream at the selected port.
    fn read_dma(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError>;

    /// Writes one DMA burst whose address remains asserted for the whole stream.
    ///
    /// Unlike MMIO, successive bytes do not select successive device
    /// addresses. The device interprets the byte stream at the selected port.
    fn write_dma(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError>;

    /// Reports the device's graphics-DMA synchronization output.
    fn dma_sync_asserted(&self) -> bool;

    /// Advances device time.
    fn advance_time(&mut self, elapsed: VirtualDuration);

    /// Returns the duration until the next device event.
    fn time_until_event(&self) -> Option<VirtualDuration>;

    /// Reports whether the device drives one shared GIO interrupt level.
    fn interrupt_asserted(&self, interrupt: GioInterrupt) -> bool;

    /// Returns the display output driven by the device, if any.
    ///
    /// Returning `Some` identifies a display-capable device; the value inside
    /// may change while the device remains attached.
    fn display_state(&self) -> Option<GioDisplayState>;

    /// Reports and clears a pending display update.
    fn take_display_update(&mut self) -> bool;

    /// Captures restorable device state.
    fn snapshot(&self) -> GioDeviceSnapshot;

    /// Reports whether the snapshot matches this configured device.
    fn accepts_snapshot(&self, snapshot: &GioDeviceSnapshot) -> bool;

    /// Restores a snapshot already accepted by [`Self::accepts_snapshot`].
    fn restore_snapshot(&mut self, snapshot: GioDeviceSnapshot);
}

/// Restorable state of a supported GIO device.
#[derive(Clone, Deserialize, Serialize)]
pub enum GioDeviceSnapshot {
    /// LG1 graphics-board state.
    Lg1(Box<Lg1>),
}

/// Complete restorable state of a GIO bus and its devices.
#[derive(Clone, Deserialize, Serialize)]
pub struct GioBusSnapshot {
    slots: [Option<GioDeviceSnapshot>; 3],
}

/// An error encountered while attaching a device to a GIO bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GioAttachError {
    /// The selected physical slot already contains a device.
    SlotOccupied(GioSlot),
}

impl fmt::Display for GioAttachError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::SlotOccupied(slot) = self;
        write!(formatter, "GIO slot {slot:?} is already occupied")
    }
}

impl Error for GioAttachError {}

/// A snapshot whose topology differs from the configured GIO bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GioSnapshotError;

impl fmt::Display for GioSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GIO snapshot does not match the configured topology")
    }
}

impl Error for GioSnapshotError {}

/// A configured GIO bus.
#[derive(Default)]
pub struct GioBus {
    slots: [Option<Box<dyn GioDevice>>; 3],
}

impl GioBus {
    /// Creates an empty GIO bus.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [None, None, None],
        }
    }

    /// Attaches one device to a physical slot.
    ///
    /// # Errors
    ///
    /// Returns [`GioAttachError`] when the selected slot is occupied.
    pub fn attach(
        &mut self,
        slot: GioSlot,
        device: Box<dyn GioDevice>,
    ) -> Result<(), GioAttachError> {
        let target = &mut self.slots[slot.index()];
        if target.is_some() {
            return Err(GioAttachError::SlotOccupied(slot));
        }
        *target = Some(device);
        Ok(())
    }

    /// Reports whether a physical slot contains a device.
    #[must_use]
    pub fn slot_occupied(&self, slot: GioSlot) -> bool {
        self.slots[slot.index()].is_some()
    }

    /// Reads one transaction without changing device state.
    ///
    /// Empty address space reads as zero.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid width or
    /// address overflow, or returns a device error.
    pub fn debug_read(
        &self,
        slot: GioSlot,
        address: DeviceAddr,
        data: &mut [u8],
    ) -> Result<(), BusError> {
        validate_transaction(address, data.len())?;
        match &self.slots[slot.index()] {
            Some(device) => device.debug_read(address, data),
            None => {
                data.fill(0);
                Ok(())
            }
        }
    }

    /// Reads one transaction, returning zero for empty address space.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid width or
    /// address overflow, or returns a device error.
    pub fn read(
        &mut self,
        slot: GioSlot,
        address: DeviceAddr,
        data: &mut [u8],
    ) -> Result<(), BusError> {
        validate_transaction(address, data.len())?;
        match &mut self.slots[slot.index()] {
            Some(device) => device.read(address, data),
            None => {
                data.fill(0);
                Ok(())
            }
        }
    }

    /// Writes one transaction, dropping writes to empty address space.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::InvalidTransaction`] for an invalid width or
    /// address overflow, or returns a device error.
    pub fn write(
        &mut self,
        slot: GioSlot,
        address: DeviceAddr,
        data: &[u8],
    ) -> Result<(), BusError> {
        validate_transaction(address, data.len())?;
        match &mut self.slots[slot.index()] {
            Some(device) => device.write(address, data),
            None => Ok(()),
        }
    }

    /// Reads one DMA burst from an attached device.
    ///
    /// Empty slots fail instead of inheriting the zero-filled MMIO behavior,
    /// because a DMA channel must observe the absence of its endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::HardwareFault`] for an empty slot, or returns the
    /// attached device's DMA error.
    pub fn read_dma(
        &mut self,
        slot: GioSlot,
        address: DeviceAddr,
        data: &mut [u8],
    ) -> Result<(), BusError> {
        self.slots[slot.index()]
            .as_mut()
            .ok_or(BusError::HardwareFault)?
            .read_dma(address, data)
    }

    /// Writes one DMA burst to an attached device.
    ///
    /// Empty slots fail instead of inheriting the dropped-write MMIO behavior,
    /// because a DMA channel must observe the absence of its endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`BusError::HardwareFault`] for an empty slot, or returns the
    /// attached device's DMA error.
    pub fn write_dma(
        &mut self,
        slot: GioSlot,
        address: DeviceAddr,
        data: &[u8],
    ) -> Result<(), BusError> {
        self.slots[slot.index()]
            .as_mut()
            .ok_or(BusError::HardwareFault)?
            .write_dma(address, data)
    }

    /// Restores every attached device to reset state.
    pub fn reset(&mut self) {
        for device in self.slots.iter_mut().flatten() {
            device.reset();
        }
    }

    /// Advances every attached device by the same duration.
    pub fn advance_time(&mut self, elapsed: VirtualDuration) {
        for device in self.slots.iter_mut().flatten() {
            device.advance_time(elapsed);
        }
    }

    /// Returns the earliest pending event across all attachments.
    #[must_use]
    pub fn time_until_event(&self) -> Option<VirtualDuration> {
        self.slots
            .iter()
            .flatten()
            .filter_map(|device| device.time_until_event())
            .min()
    }

    /// Reports the wired-OR level driven by every attached device.
    #[must_use]
    pub fn interrupt_asserted(&self, interrupt: GioInterrupt) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|device| device.interrupt_asserted(interrupt))
    }

    /// Reports the wired graphics-DMA synchronization level.
    #[must_use]
    pub fn dma_sync_asserted(&self) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|device| device.dma_sync_asserted())
    }

    /// Returns the display output of the device in one slot, if any.
    #[must_use]
    pub fn display_state(&self, slot: GioSlot) -> Option<GioDisplayState> {
        self.slots[slot.index()]
            .as_ref()
            .and_then(|device| device.display_state())
    }

    /// Reports and clears a display update from the device in one slot.
    pub fn take_display_update(&mut self, slot: GioSlot) -> bool {
        self.slots[slot.index()]
            .as_mut()
            .is_some_and(|device| device.take_display_update())
    }

    /// Captures bus topology and device state.
    #[must_use]
    pub fn snapshot(&self) -> GioBusSnapshot {
        GioBusSnapshot {
            slots: std::array::from_fn(|index| {
                self.slots[index].as_ref().map(|device| device.snapshot())
            }),
        }
    }

    /// Reports whether a snapshot exactly matches the configured topology.
    #[must_use]
    pub fn accepts_snapshot(&self, snapshot: &GioBusSnapshot) -> bool {
        self.slots
            .iter()
            .zip(&snapshot.slots)
            .all(|(device, saved)| match (device.as_ref(), saved.as_ref()) {
                (None, None) => true,
                (Some(device), Some(saved)) => device.accepts_snapshot(saved),
                _ => false,
            })
    }

    /// Restores device state without replacing configured topology.
    ///
    /// # Errors
    ///
    /// Returns [`GioSnapshotError`] without changing any device when the
    /// snapshot topology differs from the configured bus.
    pub fn restore_snapshot(&mut self, snapshot: GioBusSnapshot) -> Result<(), GioSnapshotError> {
        if !self.accepts_snapshot(&snapshot) {
            return Err(GioSnapshotError);
        }
        for (device, saved) in self.slots.iter_mut().zip(snapshot.slots) {
            if let (Some(device), Some(saved)) = (device.as_mut(), saved) {
                device.restore_snapshot(saved);
            }
        }
        Ok(())
    }
}

fn validate_transaction(address: DeviceAddr, length: usize) -> Result<(), BusError> {
    if !(1..=4).contains(&length) {
        return Err(BusError::InvalidTransaction);
    }
    address
        .get()
        .checked_add(u64::try_from(length).map_err(|_| BusError::InvalidTransaction)?)
        .ok_or(BusError::InvalidTransaction)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use se_core::bus::{BusError, DeviceAddr};
    use se_core::time::VirtualDuration;

    use super::{
        GioAttachError, GioBus, GioDevice, GioDeviceSnapshot, GioDisplayState, GioInterrupt,
        GioSlot, GioSnapshotError,
    };
    use crate::lg1::Lg1;

    #[derive(Default)]
    struct Observations {
        debug_reads: Vec<(DeviceAddr, usize)>,
        reads: Vec<(DeviceAddr, usize)>,
        writes: Vec<(DeviceAddr, Vec<u8>)>,
        dma_reads: Vec<(DeviceAddr, usize)>,
        dma_writes: Vec<(DeviceAddr, Vec<u8>)>,
        elapsed: Vec<VirtualDuration>,
        resets: usize,
        display_update: bool,
    }

    struct TestDevice {
        observations: Arc<Mutex<Observations>>,
        read_value: u8,
        deadline: Option<VirtualDuration>,
        interrupts: [bool; 3],
        display: Option<GioDisplayState>,
        dma_sync: bool,
    }

    impl TestDevice {
        fn new(observations: Arc<Mutex<Observations>>, read_value: u8) -> Self {
            Self {
                observations,
                read_value,
                deadline: None,
                interrupts: [false; 3],
                display: None,
                dma_sync: false,
            }
        }
    }

    impl GioDevice for TestDevice {
        fn reset(&mut self) {
            self.observations.lock().unwrap().resets += 1;
        }

        fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            self.observations
                .lock()
                .unwrap()
                .debug_reads
                .push((address, data.len()));
            data.fill(self.read_value);
            Ok(())
        }

        fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            self.observations
                .lock()
                .unwrap()
                .reads
                .push((address, data.len()));
            data.fill(self.read_value);
            Ok(())
        }

        fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
            self.observations
                .lock()
                .unwrap()
                .writes
                .push((address, data.to_vec()));
            Ok(())
        }

        fn read_dma(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusError> {
            self.observations
                .lock()
                .unwrap()
                .dma_reads
                .push((address, data.len()));
            data.fill(self.read_value);
            Ok(())
        }

        fn write_dma(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusError> {
            self.observations
                .lock()
                .unwrap()
                .dma_writes
                .push((address, data.to_vec()));
            Ok(())
        }

        fn dma_sync_asserted(&self) -> bool {
            self.dma_sync
        }

        fn advance_time(&mut self, elapsed: VirtualDuration) {
            self.observations.lock().unwrap().elapsed.push(elapsed);
        }

        fn time_until_event(&self) -> Option<VirtualDuration> {
            self.deadline
        }

        fn interrupt_asserted(&self, interrupt: GioInterrupt) -> bool {
            self.interrupts[match interrupt {
                GioInterrupt::Interrupt0 => 0,
                GioInterrupt::Interrupt1 => 1,
                GioInterrupt::Interrupt2 => 2,
            }]
        }

        fn display_state(&self) -> Option<GioDisplayState> {
            self.display.clone()
        }

        fn take_display_update(&mut self) -> bool {
            let mut observations = self.observations.lock().unwrap();
            let changed = observations.display_update;
            observations.display_update = false;
            changed
        }

        fn snapshot(&self) -> GioDeviceSnapshot {
            GioDeviceSnapshot::Lg1(Box::new(Lg1::new()))
        }

        fn accepts_snapshot(&self, _snapshot: &GioDeviceSnapshot) -> bool {
            false
        }

        fn restore_snapshot(&mut self, _snapshot: GioDeviceSnapshot) {
            unreachable!("test devices reject every snapshot")
        }
    }

    #[test]
    fn routing_uses_physical_slots_and_preserves_slot_local_addresses() {
        let first = Arc::new(Mutex::new(Observations::default()));
        let second = Arc::new(Mutex::new(Observations::default()));
        let mut bus = GioBus::new();
        bus.attach(
            GioSlot::Slot1,
            Box::new(TestDevice::new(Arc::clone(&second), 0x22)),
        )
        .unwrap();
        bus.attach(
            GioSlot::Slot0,
            Box::new(TestDevice::new(Arc::clone(&first), 0x11)),
        )
        .unwrap();

        let mut bytes = [0; 2];
        bus.read(GioSlot::Slot0, DeviceAddr::new(0x101), &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0x11; 2]);
        bus.debug_read(GioSlot::Slot1, DeviceAddr::new(0x202), &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0x22; 2]);
        bus.write(GioSlot::Slot1, DeviceAddr::new(0x200), &[3, 4])
            .unwrap();

        assert_eq!(first.lock().unwrap().reads, [(DeviceAddr::new(0x101), 2)]);
        let second = second.lock().unwrap();
        assert_eq!(second.debug_reads, [(DeviceAddr::new(0x202), 2)]);
        assert_eq!(second.writes, [(DeviceAddr::new(0x200), vec![3, 4])]);
    }

    #[test]
    fn empty_space_completes_without_reaching_an_attachment() {
        let observations = Arc::new(Mutex::new(Observations::default()));
        let mut bus = GioBus::new();
        bus.attach(
            GioSlot::Slot0,
            Box::new(TestDevice::new(Arc::clone(&observations), 0x11)),
        )
        .unwrap();

        let mut bytes = [0xa5; 4];
        bus.read(GioSlot::Slot1, DeviceAddr::new(0x20), &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0; 4]);
        bytes.fill(0xa5);
        bus.debug_read(GioSlot::Graphics, DeviceAddr::new(0x20), &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0; 4]);
        bus.write(GioSlot::Slot1, DeviceAddr::new(0x20), &[1, 2, 3, 4])
            .unwrap();

        let observations = observations.lock().unwrap();
        assert!(observations.reads.is_empty());
        assert!(observations.debug_reads.is_empty());
        assert!(observations.writes.is_empty());
    }

    #[test]
    fn invalid_transactions_are_rejected_before_side_effects() {
        let observations = Arc::new(Mutex::new(Observations::default()));
        let mut bus = GioBus::new();
        bus.attach(
            GioSlot::Slot0,
            Box::new(TestDevice::new(Arc::clone(&observations), 0x11)),
        )
        .unwrap();

        assert_eq!(
            bus.read(GioSlot::Slot0, DeviceAddr::new(0), &mut []),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            bus.write(GioSlot::Slot0, DeviceAddr::new(0), &[0; 5]),
            Err(BusError::InvalidTransaction)
        );
        assert_eq!(
            bus.read(GioSlot::Slot0, DeviceAddr::new(u64::MAX), &mut [0; 1],),
            Err(BusError::InvalidTransaction)
        );

        let observations = observations.lock().unwrap();
        assert!(observations.reads.is_empty());
        assert!(observations.writes.is_empty());
    }

    #[test]
    fn dma_routes_one_addressed_stream_and_rejects_empty_slots() {
        let observations = Arc::new(Mutex::new(Observations::default()));
        let mut bus = GioBus::new();
        bus.attach(
            GioSlot::Graphics,
            Box::new(TestDevice::new(Arc::clone(&observations), 0x5a)),
        )
        .unwrap();

        let mut read = [0; 7];
        assert_eq!(
            bus.read_dma(GioSlot::Graphics, DeviceAddr::new(0x1234), &mut read),
            Ok(())
        );
        assert_eq!(read, [0x5a; 7]);
        assert_eq!(
            bus.write_dma(GioSlot::Graphics, DeviceAddr::new(0x5678), &[1, 2, 3]),
            Ok(())
        );
        assert_eq!(
            bus.write_dma(GioSlot::Slot0, DeviceAddr::new(0), &[]),
            Err(BusError::HardwareFault)
        );

        let observations = observations.lock().unwrap();
        assert_eq!(observations.dma_reads, [(DeviceAddr::new(0x1234), 7)]);
        assert_eq!(
            observations.dma_writes,
            [(DeviceAddr::new(0x5678), vec![1, 2, 3])]
        );
    }

    #[test]
    fn dma_sync_is_the_wired_or_of_attached_devices() {
        let observations = Arc::new(Mutex::new(Observations::default()));
        let mut low = TestDevice::new(Arc::clone(&observations), 0);
        low.dma_sync = false;
        let mut high = TestDevice::new(observations, 0);
        high.dma_sync = true;
        let mut bus = GioBus::new();

        assert!(!bus.dma_sync_asserted());
        bus.attach(GioSlot::Slot0, Box::new(low)).unwrap();
        assert!(!bus.dma_sync_asserted());
        bus.attach(GioSlot::Graphics, Box::new(high)).unwrap();
        assert!(bus.dma_sync_asserted());
    }

    #[test]
    fn attachment_rejects_only_an_occupied_physical_slot() {
        let mut bus = GioBus::new();
        bus.attach(GioSlot::Graphics, Box::new(Lg1::new())).unwrap();
        assert_eq!(
            bus.attach(GioSlot::Graphics, Box::new(Lg1::new())),
            Err(GioAttachError::SlotOccupied(GioSlot::Graphics))
        );
        assert!(bus.slot_occupied(GioSlot::Graphics));
        assert!(!bus.slot_occupied(GioSlot::Slot0));
    }

    #[test]
    fn time_interrupts_reset_and_display_updates_cover_their_hardware_scope() {
        let first = Arc::new(Mutex::new(Observations {
            display_update: true,
            ..Observations::default()
        }));
        let second = Arc::new(Mutex::new(Observations::default()));
        let mut first_device = TestDevice::new(Arc::clone(&first), 0);
        first_device.deadline = Some(VirtualDuration::from_attoseconds(20));
        first_device.interrupts = [true, false, false];
        first_device.display = Some(GioDisplayState::NoSignal);
        let mut second_device = TestDevice::new(Arc::clone(&second), 0);
        second_device.deadline = Some(VirtualDuration::from_attoseconds(10));
        second_device.interrupts = [false, false, true];
        let mut bus = GioBus::new();
        bus.attach(GioSlot::Graphics, Box::new(first_device))
            .unwrap();
        bus.attach(GioSlot::Slot1, Box::new(second_device)).unwrap();

        assert_eq!(
            bus.time_until_event(),
            Some(VirtualDuration::from_attoseconds(10))
        );
        assert!(bus.interrupt_asserted(GioInterrupt::Interrupt0));
        assert!(!bus.interrupt_asserted(GioInterrupt::Interrupt1));
        assert!(bus.interrupt_asserted(GioInterrupt::Interrupt2));
        assert_eq!(
            bus.display_state(GioSlot::Graphics),
            Some(GioDisplayState::NoSignal)
        );
        assert_eq!(bus.display_state(GioSlot::Slot1), None);
        assert!(bus.take_display_update(GioSlot::Graphics));
        assert!(!bus.take_display_update(GioSlot::Graphics));
        bus.advance_time(VirtualDuration::from_attoseconds(7));
        bus.reset();

        for observations in [first, second] {
            let observations = observations.lock().unwrap();
            assert_eq!(observations.elapsed, [VirtualDuration::from_attoseconds(7)]);
            assert_eq!(observations.resets, 1);
        }
    }

    #[test]
    fn snapshot_restore_preserves_topology_and_device_progress() {
        const COMMAND: u64 = 0;
        const GO: u64 = 0x0800;
        const XSTARTI: u64 = 0x000c;
        const LG1_BASE: u64 = 0x003f_0000;

        let mut bus = GioBus::new();
        bus.attach(GioSlot::Graphics, Box::new(Lg1::new())).unwrap();
        bus.write(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + XSTARTI),
            &0x123_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + COMMAND),
            &0_u32.to_be_bytes(),
        )
        .unwrap();
        bus.read(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + COMMAND + GO),
            &mut [0; 4],
        )
        .unwrap();
        let snapshot = bus.snapshot();
        bus.write(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + XSTARTI),
            &0x456_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + COMMAND),
            &0_u32.to_be_bytes(),
        )
        .unwrap();
        bus.read(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + COMMAND + GO),
            &mut [0; 4],
        )
        .unwrap();

        bus.restore_snapshot(snapshot.clone()).unwrap();
        let mut bytes = [0; 4];
        bus.read(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + XSTARTI),
            &mut bytes,
        )
        .unwrap();
        assert_eq!(u32::from_be_bytes(bytes), 0x123);

        let mut incompatible = snapshot;
        incompatible.slots[GioSlot::Graphics.index()] = None;
        bus.write(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + XSTARTI),
            &0x789_u32.to_be_bytes(),
        )
        .unwrap();
        bus.write(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + COMMAND),
            &0_u32.to_be_bytes(),
        )
        .unwrap();
        bus.read(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + COMMAND + GO),
            &mut [0; 4],
        )
        .unwrap();
        assert_eq!(bus.restore_snapshot(incompatible), Err(GioSnapshotError));
        bus.read(
            GioSlot::Graphics,
            DeviceAddr::new(LG1_BASE + XSTARTI),
            &mut bytes,
        )
        .unwrap();
        assert_eq!(u32::from_be_bytes(bytes), 0x789);
    }
}
