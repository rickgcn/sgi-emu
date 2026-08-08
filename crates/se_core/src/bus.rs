//! Defines master-side physical bus and device-side MMIO contracts.
//!
//! [`Bus`] accepts [`PhysAddr`] values from CPUs and DMA-capable devices.
//! Routing translates each transaction into an [`MmioAccess`] containing a
//! [`DeviceAddr`] and the original [`BusInitiator`]. Fixed-width methods are
//! single transactions; block methods may complete a prefix before failing.
//!
//! [`DirectSpan`] exposes borrowed direct memory without granting pointer
//! stability beyond the bus or device borrow that produced it.

use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::address::{DeviceAddr, PhysAddr};
use crate::device::DeviceId;

/// Identifies a CPU within one machine profile.
///
/// The profile assigns raw values; this identity is independent of host threads.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CpuId(u32);

impl CpuId {
    /// Creates a CPU identity from its profile-defined raw value.
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

/// Identifies the master that issued a bus transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusInitiator {
    /// A transaction issued by a guest CPU.
    Cpu(CpuId),
    /// A DMA transaction issued by a registered device.
    Device(DeviceId),
}

/// Describes one device-side access after physical address translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmioAccess {
    /// Master that issued the transaction.
    pub initiator: BusInitiator,
    /// Address in the destination device's private address space.
    pub addr: DeviceAddr,
}

/// Reports why a bus transaction did not complete successfully.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusFault {
    /// No single mapping completely covers the transaction.
    Unmapped,
    /// A mapped device rejected the transaction.
    Fault,
}

impl fmt::Display for BusFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped => formatter.write_str("unmapped bus transaction"),
            Self::Fault => formatter.write_str("mapped device rejected bus transaction"),
        }
    }
}

impl Error for BusFault {}

/// Declares how a caller intends to use a direct memory span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectAccess {
    /// The caller will only read the span.
    Read,
    /// The caller may modify the span.
    Write,
}

/// Provides an immediately usable pointer to a contiguous byte region.
///
/// The lifetime prevents the originating bus or device from being accessed while
/// the span exists. The pointer is not stable beyond that borrow, and
/// [`DirectAccess`] does not enforce access permissions at the type level.
#[derive(Debug)]
pub struct DirectSpan<'a> {
    pointer: NonNull<u8>,
    len: usize,
    borrow: PhantomData<&'a mut [u8]>,
}

impl<'a> DirectSpan<'a> {
    /// Creates a direct span covering an entire mutable byte slice.
    ///
    /// Returns `None` when `bytes` is empty.
    #[must_use]
    pub fn from_slice(bytes: &'a mut [u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        Some(Self {
            pointer: NonNull::from(&mut bytes[0]),
            len: bytes.len(),
            borrow: PhantomData,
        })
    }

    /// Creates a direct span from raw parts, returning `None` when `len` is zero.
    ///
    /// # Safety
    ///
    /// For nonzero `len`, `pointer` must refer to `len` consecutive initialized
    /// bytes in one live allocation. That region must remain valid for reads and
    /// writes and must not be accessed through any other pointer throughout
    /// `'a`.
    pub unsafe fn from_raw_parts(pointer: NonNull<u8>, len: usize) -> Option<Self> {
        if len == 0 {
            return None;
        }
        Some(Self {
            pointer,
            len,
            borrow: PhantomData,
        })
    }

    /// Returns the first byte pointer.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut u8 {
        self.pointer.as_ptr()
    }

    /// Returns the number of contiguous bytes available from the pointer.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `false`; constructors never produce an empty span.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Limits the span to at most `maximum` bytes without changing its pointer.
    ///
    /// Returns `None` when `maximum` is zero.
    #[must_use]
    pub fn truncate(mut self, maximum: usize) -> Option<Self> {
        self.len = self.len.min(maximum);
        if self.len == 0 { None } else { Some(self) }
    }
}

/// Physical bus operations used by CPUs and device DMA engines.
///
/// Multi-byte values use guest big-endian byte order. Each fixed-width method is
/// one device transaction and must not be decomposed into narrower accesses.
/// [`BusFault::Unmapped`] means no device transaction was issued because one
/// physical route did not cover the complete access. [`BusFault::Fault`] means a
/// mapped device rejected the transaction; the bus does not promise to roll back
/// device side effects associated with that rejection.
pub trait Bus {
    /// Reads one byte.
    fn read8(&mut self, addr: PhysAddr) -> Result<u8, BusFault>;

    /// Reads one big-endian 16-bit value.
    fn read16(&mut self, addr: PhysAddr) -> Result<u16, BusFault>;

    /// Reads one big-endian 32-bit value.
    fn read32(&mut self, addr: PhysAddr) -> Result<u32, BusFault>;

    /// Reads one big-endian 64-bit value.
    fn read64(&mut self, addr: PhysAddr) -> Result<u64, BusFault>;

    /// Writes one byte.
    fn write8(&mut self, addr: PhysAddr, value: u8) -> Result<(), BusFault>;

    /// Writes one big-endian 16-bit value.
    fn write16(&mut self, addr: PhysAddr, value: u16) -> Result<(), BusFault>;

    /// Writes one big-endian 32-bit value.
    fn write32(&mut self, addr: PhysAddr, value: u32) -> Result<(), BusFault>;

    /// Writes one big-endian 64-bit value.
    fn write64(&mut self, addr: PhysAddr, value: u64) -> Result<(), BusFault>;

    /// Reads a contiguous byte sequence in ascending physical-address order.
    ///
    /// An empty output succeeds without resolving `addr`. On failure, an
    /// unspecified prefix of `output` may already contain transferred bytes;
    /// bytes after that prefix retain their previous values.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::Unmapped`] when an address or route is unavailable and
    /// [`BusFault::Fault`] when a mapped device rejects a routed chunk.
    fn read_block(&mut self, addr: PhysAddr, output: &mut [u8]) -> Result<(), BusFault> {
        for (offset, byte) in output.iter_mut().enumerate() {
            let offset = u64::try_from(offset).map_err(|_| BusFault::Unmapped)?;
            let addr = addr.checked_add(offset).ok_or(BusFault::Unmapped)?;
            *byte = self.read8(addr)?;
        }
        Ok(())
    }

    /// Writes a contiguous byte sequence in ascending physical-address order.
    ///
    /// An empty input succeeds without resolving `addr`. On failure, an
    /// unspecified prefix may already have been committed to one or more devices.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::Unmapped`] when an address or route is unavailable and
    /// [`BusFault::Fault`] when a mapped device rejects a routed chunk.
    fn write_block(&mut self, addr: PhysAddr, input: &[u8]) -> Result<(), BusFault> {
        for (offset, &byte) in input.iter().enumerate() {
            let offset = u64::try_from(offset).map_err(|_| BusFault::Unmapped)?;
            let addr = addr.checked_add(offset).ok_or(BusFault::Unmapped)?;
            self.write8(addr, byte)?;
        }
        Ok(())
    }

    /// Requests a direct span beginning at a physical address.
    ///
    /// A successful `None` result means that the route provides no direct access.
    /// Any returned span must contain at most `requested` bytes and must not cross
    /// a physical route boundary.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault::Unmapped`] when the first byte is not routed and
    /// [`BusFault::Fault`] when the mapped device rejects the request.
    fn direct_span(
        &mut self,
        addr: PhysAddr,
        requested: usize,
        access: DirectAccess,
    ) -> Result<Option<DirectSpan<'_>>, BusFault>;
}

/// Device-side MMIO operations over a private local address space.
///
/// Implementations independently select legal access widths and side effects.
/// Each fixed-width method receives one indivisible transaction in guest
/// big-endian value form. Returning [`BusFault::Fault`] does not promise rollback
/// of device side effects.
pub trait MmioDevice {
    /// Reads one byte.
    fn read8(&mut self, access: MmioAccess) -> Result<u8, BusFault>;

    /// Reads one big-endian 16-bit value.
    fn read16(&mut self, access: MmioAccess) -> Result<u16, BusFault>;

    /// Reads one big-endian 32-bit value.
    fn read32(&mut self, access: MmioAccess) -> Result<u32, BusFault>;

    /// Reads one big-endian 64-bit value.
    fn read64(&mut self, access: MmioAccess) -> Result<u64, BusFault>;

    /// Writes one byte.
    fn write8(&mut self, access: MmioAccess, value: u8) -> Result<(), BusFault>;

    /// Writes one big-endian 16-bit value.
    fn write16(&mut self, access: MmioAccess, value: u16) -> Result<(), BusFault>;

    /// Writes one big-endian 32-bit value.
    fn write32(&mut self, access: MmioAccess, value: u32) -> Result<(), BusFault>;

    /// Writes one big-endian 64-bit value.
    fn write64(&mut self, access: MmioAccess, value: u64) -> Result<(), BusFault>;

    /// Reads a contiguous local byte sequence within one physical route.
    ///
    /// The default implementation performs ascending `read8` transactions. On
    /// failure, an unspecified prefix of `output` may already be modified.
    fn read_block(&mut self, access: MmioAccess, output: &mut [u8]) -> Result<(), BusFault> {
        for (offset, byte) in output.iter_mut().enumerate() {
            let offset = u64::try_from(offset).map_err(|_| BusFault::Fault)?;
            let addr = access.addr.checked_add(offset).ok_or(BusFault::Fault)?;
            *byte = self.read8(MmioAccess { addr, ..access })?;
        }
        Ok(())
    }

    /// Writes a contiguous local byte sequence within one physical route.
    ///
    /// The default implementation performs ascending `write8` transactions. On
    /// failure, an unspecified prefix may already be committed.
    fn write_block(&mut self, access: MmioAccess, input: &[u8]) -> Result<(), BusFault> {
        for (offset, &byte) in input.iter().enumerate() {
            let offset = u64::try_from(offset).map_err(|_| BusFault::Fault)?;
            let addr = access.addr.checked_add(offset).ok_or(BusFault::Fault)?;
            self.write8(MmioAccess { addr, ..access }, byte)?;
        }
        Ok(())
    }

    /// Requests a direct span beginning at a device-local address.
    ///
    /// A successful `None` result means that the device provides no direct
    /// access. Any returned span must contain at most `requested` bytes.
    fn direct_span(
        &mut self,
        access: MmioAccess,
        requested: usize,
        kind: DirectAccess,
    ) -> Result<Option<DirectSpan<'_>>, BusFault>;
}

#[cfg(test)]
mod tests {
    use super::{Bus, BusFault, DirectAccess, DirectSpan};
    use crate::address::PhysAddr;

    struct ByteBus(Vec<u8>);

    impl Bus for ByteBus {
        fn read8(&mut self, addr: PhysAddr) -> Result<u8, BusFault> {
            self.0
                .get(addr.get() as usize)
                .copied()
                .ok_or(BusFault::Unmapped)
        }

        fn read16(&mut self, _addr: PhysAddr) -> Result<u16, BusFault> {
            Err(BusFault::Fault)
        }

        fn read32(&mut self, _addr: PhysAddr) -> Result<u32, BusFault> {
            Err(BusFault::Fault)
        }

        fn read64(&mut self, _addr: PhysAddr) -> Result<u64, BusFault> {
            Err(BusFault::Fault)
        }

        fn write8(&mut self, addr: PhysAddr, value: u8) -> Result<(), BusFault> {
            let byte = self
                .0
                .get_mut(addr.get() as usize)
                .ok_or(BusFault::Unmapped)?;
            *byte = value;
            Ok(())
        }

        fn write16(&mut self, _addr: PhysAddr, _value: u16) -> Result<(), BusFault> {
            Err(BusFault::Fault)
        }

        fn write32(&mut self, _addr: PhysAddr, _value: u32) -> Result<(), BusFault> {
            Err(BusFault::Fault)
        }

        fn write64(&mut self, _addr: PhysAddr, _value: u64) -> Result<(), BusFault> {
            Err(BusFault::Fault)
        }

        fn direct_span(
            &mut self,
            addr: PhysAddr,
            requested: usize,
            _access: DirectAccess,
        ) -> Result<Option<DirectSpan<'_>>, BusFault> {
            let start = usize::try_from(addr.get()).map_err(|_| BusFault::Unmapped)?;
            let bytes = self.0.get_mut(start..).ok_or(BusFault::Unmapped)?;
            let available = bytes.len().min(requested);
            Ok(DirectSpan::from_slice(&mut bytes[..available]))
        }
    }

    #[test]
    fn default_block_access_has_defined_partial_completion() {
        let mut bus = ByteBus(vec![1, 2, 3]);
        let mut output = [0xaa; 3];
        assert_eq!(
            bus.read_block(PhysAddr::new(2), &mut output),
            Err(BusFault::Unmapped)
        );
        assert_eq!(output, [3, 0xaa, 0xaa]);
        assert_eq!(
            bus.write_block(PhysAddr::new(2), &[7, 8]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(bus.0, vec![1, 2, 7]);
    }

    #[test]
    fn direct_span_reports_a_bounded_non_empty_length() {
        let mut bus = ByteBus(vec![0; 8]);
        let span = bus
            .direct_span(PhysAddr::new(6), 8, DirectAccess::Write)
            .unwrap()
            .unwrap();
        assert_eq!(span.len(), 2);
        assert!(!span.is_empty());
    }
}
