//! Defines registered device identity and device-facing services.
//!
//! [`Device`] combines MMIO, snapshot, introspection, reset, and scheduled-event
//! behavior. During an event callback, [`DeviceCtx`] supplies a physical bus
//! whose initiator is that device and a [`SchedulerHandle`] whose destination is
//! the same device identity.

use std::any::Any;
use std::error::Error;
use std::fmt;

use crate::bus::{Bus, BusFault, MmioDevice};
use crate::event::{EventQueueError, SchedulerHandle};
use crate::inspect::Introspect;
use crate::save::Saveable;
use crate::time::VTime;

/// Identifies a registered device by its registration-order value.
///
/// The value remains stable for the lifetime and snapshots of one machine
/// profile. It is not a snapshot component key.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeviceId(u32);

impl DeviceId {
    /// Creates a device identity from its profile-defined raw value.
    #[must_use]
    pub const fn from_raw(value: u32) -> Self {
        Self(value)
    }

    /// Returns the profile-defined raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Provides object-safe access to a concrete device's [`Any`] identity.
pub trait AsAny {
    /// Returns this value as an immutable dynamic `Any` reference.
    fn as_any(&self) -> &dyn Any;

    /// Returns this value as a mutable dynamic `Any` reference.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Provides machine services during a scheduled device event.
pub struct DeviceCtx<'a> {
    /// Machine virtual time at which the event is dispatched.
    pub now: VTime,
    /// Physical bus access bound to this device as the transaction initiator.
    pub bus: &'a mut dyn Bus,
    /// Scheduler access bound to this device as the event destination.
    pub sched: &'a SchedulerHandle,
}

/// Reports a device event callback failure.
#[derive(Debug)]
pub enum DeviceError {
    /// A DMA bus transaction failed.
    Bus(BusFault),
    /// An event scheduling operation failed.
    Event(EventQueueError),
    /// A device-specific invariant or operation failed.
    Failed(String),
}

impl fmt::Display for DeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(error) => write!(formatter, "device bus access failed: {error}"),
            Self::Event(error) => write!(formatter, "device scheduling failed: {error}"),
            Self::Failed(reason) => write!(formatter, "device operation failed: {reason}"),
        }
    }
}

impl Error for DeviceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bus(error) => Some(error),
            Self::Event(error) => Some(error),
            Self::Failed(_) => None,
        }
    }
}

impl From<BusFault> for DeviceError {
    fn from(error: BusFault) -> Self {
        Self::Bus(error)
    }
}

impl From<EventQueueError> for DeviceError {
    fn from(error: EventQueueError) -> Self {
        Self::Event(error)
    }
}

/// Defines the complete object-safe behavior of a registered device.
pub trait Device: MmioDevice + Saveable + Introspect + AsAny {
    /// Resets the device according to its hard- or soft-reset contract.
    ///
    /// `soft == true` requests a soft reset and `false` requests a hard reset. The
    /// concrete device defines which state, if any, each operation preserves.
    fn reset(&mut self, soft: bool);

    /// Delivers a scheduled event at the context's virtual time.
    ///
    /// An error does not roll back state or bus effects already produced by the
    /// callback.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] when DMA, event scheduling, or a device invariant
    /// prevents the callback from completing.
    fn on_event(
        &mut self,
        tag: u32,
        payload: u64,
        context: &mut DeviceCtx<'_>,
    ) -> Result<(), DeviceError>;
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use crate::address::DeviceAddr;
    use crate::bus::{
        BusFault, BusInitiator, CpuId, DirectAccess, DirectSpan, MmioAccess, MmioDevice,
    };
    use crate::inspect::{InspectCommand, InspectError, Introspect};
    use crate::save::{Saveable, StateError, StateReader, StateWriter};

    use super::{Device, DeviceCtx, DeviceError};

    struct DowncastDevice;

    impl MmioDevice for DowncastDevice {
        fn read8(&mut self, _access: MmioAccess) -> Result<u8, BusFault> {
            Ok(0)
        }

        fn read16(&mut self, _access: MmioAccess) -> Result<u16, BusFault> {
            Err(BusFault::Fault)
        }

        fn read32(&mut self, _access: MmioAccess) -> Result<u32, BusFault> {
            Err(BusFault::Fault)
        }

        fn read64(&mut self, _access: MmioAccess) -> Result<u64, BusFault> {
            Err(BusFault::Fault)
        }

        fn write8(&mut self, _access: MmioAccess, _value: u8) -> Result<(), BusFault> {
            Ok(())
        }

        fn write16(&mut self, _access: MmioAccess, _value: u16) -> Result<(), BusFault> {
            Err(BusFault::Fault)
        }

        fn write32(&mut self, _access: MmioAccess, _value: u32) -> Result<(), BusFault> {
            Err(BusFault::Fault)
        }

        fn write64(&mut self, _access: MmioAccess, _value: u64) -> Result<(), BusFault> {
            Err(BusFault::Fault)
        }

        fn direct_span(
            &mut self,
            _access: MmioAccess,
            _requested: usize,
            _kind: DirectAccess,
        ) -> Result<Option<DirectSpan<'_>>, BusFault> {
            Ok(None)
        }
    }

    impl Saveable for DowncastDevice {
        fn snapshot_version(&self) -> u32 {
            1
        }

        fn save(&self, writer: &mut StateWriter<'_>) -> Result<(), StateError> {
            writer.serialize(&())
        }

        fn load(&mut self, version: u32, reader: &mut StateReader<'_>) -> Result<(), StateError> {
            if version != 1 {
                return Err(StateError::UnsupportedVersion(version));
            }
            reader.deserialize::<()>()
        }
    }

    impl Introspect for DowncastDevice {
        fn commands(&self) -> &[InspectCommand] {
            &[]
        }

        fn execute(
            &mut self,
            command: &str,
            _arguments: &[&str],
            _output: &mut dyn Write,
        ) -> Result<(), InspectError> {
            Err(InspectError::UnknownCommand(command.to_owned()))
        }
    }

    impl Device for DowncastDevice {
        fn reset(&mut self, _soft: bool) {}

        fn on_event(
            &mut self,
            _tag: u32,
            _payload: u64,
            _context: &mut DeviceCtx<'_>,
        ) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    #[test]
    fn device_trait_supports_object_safe_downcasting() {
        let mut device: Box<dyn Device> = Box::new(DowncastDevice);
        assert!(device.as_any().is::<DowncastDevice>());
        assert!(
            device
                .as_any_mut()
                .downcast_mut::<DowncastDevice>()
                .is_some()
        );
        let access = MmioAccess {
            initiator: BusInitiator::Cpu(CpuId::from_raw(4)),
            addr: DeviceAddr::new(0),
        };
        assert_eq!(device.read8(access), Ok(0));
    }
}
