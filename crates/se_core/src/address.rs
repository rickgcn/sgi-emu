//! Machine-independent physical and device-local address types.

use std::error::Error;
use std::fmt;

/// A physical address issued by a CPU or DMA-capable device.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// Creates a physical address from its raw value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw address value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Adds an offset without wrapping.
    #[must_use]
    pub const fn checked_add(self, offset: u64) -> Option<Self> {
        match self.0.checked_add(offset) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl From<u64> for PhysAddr {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<PhysAddr> for u64 {
    fn from(value: PhysAddr) -> Self {
        value.get()
    }
}

/// An address in a device's private local address space.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeviceAddr(u64);

impl DeviceAddr {
    /// Creates a device-local address from its raw value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw address value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Adds an offset without wrapping.
    #[must_use]
    pub const fn checked_add(self, offset: u64) -> Option<Self> {
        match self.0.checked_add(offset) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl From<u64> for DeviceAddr {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<DeviceAddr> for u64 {
    fn from(value: DeviceAddr) -> Self {
        value.get()
    }
}

/// Errors produced while constructing a physical range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysRangeError {
    /// A mapping cannot contain zero bytes.
    ZeroLength,
    /// The exclusive end cannot be represented without wrapping.
    EndOverflow,
}

impl fmt::Display for PhysRangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLength => formatter.write_str("physical range length is zero"),
            Self::EndOverflow => formatter.write_str("physical range end overflows u64"),
        }
    }
}

impl Error for PhysRangeError {}

/// A non-empty half-open physical range `[start, end)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysRange {
    start: PhysAddr,
    end_exclusive: u64,
}

impl PhysRange {
    /// Creates a range from a start address and byte length.
    pub fn from_start_len(start: PhysAddr, len: u64) -> Result<Self, PhysRangeError> {
        if len == 0 {
            return Err(PhysRangeError::ZeroLength);
        }
        let end_exclusive = start
            .get()
            .checked_add(len)
            .ok_or(PhysRangeError::EndOverflow)?;
        Ok(Self {
            start,
            end_exclusive,
        })
    }

    /// Returns the inclusive start address.
    #[must_use]
    pub const fn start(self) -> PhysAddr {
        self.start
    }

    /// Returns the exclusive end as a raw address value.
    #[must_use]
    pub const fn end_exclusive(self) -> u64 {
        self.end_exclusive
    }

    /// Returns the range length in bytes.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.end_exclusive - self.start.get()
    }

    /// Returns whether the range is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }

    /// Returns whether an address belongs to the range.
    #[must_use]
    pub const fn contains(self, addr: PhysAddr) -> bool {
        addr.get() >= self.start.get() && addr.get() < self.end_exclusive
    }

    /// Returns whether the complete non-empty byte span belongs to the range.
    #[must_use]
    pub fn contains_span(self, addr: PhysAddr, len: u64) -> bool {
        if len == 0 || addr.get() < self.start.get() {
            return false;
        }
        addr.get()
            .checked_add(len)
            .is_some_and(|end| end <= self.end_exclusive)
    }
}

/// Physical address geometry selected by a machine profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AddressSpaceConfig {
    /// Number of implemented physical address bits.
    pub physical_address_bits: u8,
}

impl AddressSpaceConfig {
    /// Returns whether a raw address is implemented by this configuration.
    #[must_use]
    pub const fn contains(self, addr: PhysAddr) -> bool {
        match self.physical_address_bits {
            1..=63 => addr.get() < (1_u64 << self.physical_address_bits),
            _ => false,
        }
    }

    /// Returns whether a physical range fits completely in this configuration.
    #[must_use]
    pub const fn contains_range(self, range: PhysRange) -> bool {
        match self.physical_address_bits {
            1..=63 => range.end_exclusive() <= (1_u64 << self.physical_address_bits),
            _ => false,
        }
    }

    /// Returns the exclusive address-space limit for a supported geometry.
    #[must_use]
    pub const fn upper_bound_exclusive(self) -> Option<u64> {
        match self.physical_address_bits {
            1..=63 => Some(1_u64 << self.physical_address_bits),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AddressSpaceConfig, DeviceAddr, PhysAddr, PhysRange, PhysRangeError};

    #[test]
    fn physical_and_device_addresses_are_distinct_values() {
        let physical = PhysAddr::new(0x1234);
        let device = DeviceAddr::new(0x1234);
        assert_eq!(physical.get(), device.get());
        assert_eq!(physical.checked_add(4), Some(PhysAddr::new(0x1238)));
        assert_eq!(device.checked_add(4), Some(DeviceAddr::new(0x1238)));
    }

    #[test]
    fn physical_range_rejects_zero_and_overflow() {
        assert_eq!(
            PhysRange::from_start_len(PhysAddr::new(0), 0),
            Err(PhysRangeError::ZeroLength)
        );
        assert_eq!(
            PhysRange::from_start_len(PhysAddr::new(u64::MAX), 1),
            Err(PhysRangeError::EndOverflow)
        );
    }

    #[test]
    fn physical_range_uses_exact_half_open_bounds() {
        let range = PhysRange::from_start_len(PhysAddr::new(0x1000), 0x100).unwrap();
        assert!(range.contains(PhysAddr::new(0x1000)));
        assert!(range.contains(PhysAddr::new(0x10ff)));
        assert!(!range.contains(PhysAddr::new(0x1100)));
        assert!(range.contains_span(PhysAddr::new(0x10f8), 8));
        assert!(!range.contains_span(PhysAddr::new(0x10f9), 8));
    }

    #[test]
    fn configured_space_accepts_a_range_ending_at_its_limit() {
        let config = AddressSpaceConfig {
            physical_address_bits: 40,
        };
        let range =
            PhysRange::from_start_len(PhysAddr::new((1_u64 << 40) - 0x1_0000), 0x1_0000).unwrap();
        assert!(config.contains_range(range));
        assert!(!config.contains(PhysAddr::new(1_u64 << 40)));
    }

    #[test]
    fn sixty_three_bit_configuration_has_a_representable_exclusive_limit() {
        let config = AddressSpaceConfig {
            physical_address_bits: 63,
        };
        let limit = 1_u64 << 63;
        let final_byte = PhysRange::from_start_len(PhysAddr::new(limit - 1), 1).unwrap();
        assert!(config.contains(PhysAddr::new(limit - 1)));
        assert!(config.contains_range(final_byte));
        assert!(!config.contains(PhysAddr::new(limit)));
        assert_eq!(config.upper_bound_exclusive(), Some(limit));
    }

    #[test]
    fn sixty_four_bit_configuration_is_not_supported() {
        let config = AddressSpaceConfig {
            physical_address_bits: 64,
        };
        assert!(!config.contains(PhysAddr::new(u64::MAX)));
        assert_eq!(config.upper_bound_exclusive(), None);
    }
}
