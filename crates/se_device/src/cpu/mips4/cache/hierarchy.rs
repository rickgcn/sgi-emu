//! Functional cache storage shared by MIPS IV processor models.
//!
//! The ISA leaves cache organization and `CACHE` suboperations to concrete
//! processors. This module supplies validated set-associative storage and
//! physical-byte cache lines; processor policies remain responsible for
//! selecting geometry, access policy, and diagnostic tag encodings.

use core::fmt;

/// Number of bytes transferred by one functional cache line operation.
pub const MIPS4_FUNCTIONAL_CACHE_LINE_BYTES: usize = 32;

/// Processor-resolved behavior for a translated memory reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4CacheAccessPolicy {
    /// Bypass every cache level.
    Uncached,
    /// Update memory on every store and do not allocate on a store miss.
    WriteThroughNoWriteAllocate,
    /// Update memory on every store and allocate on a store miss.
    WriteThroughWriteAllocate,
    /// Allocate on misses and defer memory stores until writeback.
    WriteBackWriteAllocate,
}

impl Mips4CacheAccessPolicy {
    /// Returns whether ordinary references may use cache storage.
    pub const fn is_cached(self) -> bool {
        !matches!(self, Self::Uncached)
    }

    /// Returns whether a store miss allocates a line.
    pub const fn write_allocates(self) -> bool {
        matches!(
            self,
            Self::WriteThroughWriteAllocate | Self::WriteBackWriteAllocate
        )
    }

    /// Returns whether stores update external memory immediately.
    pub const fn is_write_through(self) -> bool {
        matches!(
            self,
            Self::WriteThroughNoWriteAllocate | Self::WriteThroughWriteAllocate
        )
    }

    /// Returns whether dirty lines can be produced.
    pub const fn is_write_back(self) -> bool {
        matches!(self, Self::WriteBackWriteAllocate)
    }
}

/// Geometry of one functional cache level.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4CacheGeometry {
    /// Total data capacity.
    pub size_bytes: u32,
    /// Bytes represented by one tag.
    pub line_size_bytes: u32,
    /// Number of ways in each set.
    pub ways: u8,
}

impl Mips4CacheGeometry {
    /// Creates cache geometry.
    pub const fn new(size_bytes: u32, line_size_bytes: u32, ways: u8) -> Self {
        Self {
            size_bytes,
            line_size_bytes,
            ways,
        }
    }

    fn validate(self) -> Result<usize, Mips4CacheConfigError> {
        if self.line_size_bytes as usize != MIPS4_FUNCTIONAL_CACHE_LINE_BYTES {
            return Err(Mips4CacheConfigError::UnsupportedLineSize {
                line_size_bytes: self.line_size_bytes,
            });
        }
        if self.ways == 0 || !self.ways.is_power_of_two() {
            return Err(Mips4CacheConfigError::InvalidWayCount { ways: self.ways });
        }
        let set_bytes = self
            .line_size_bytes
            .checked_mul(u32::from(self.ways))
            .ok_or(Mips4CacheConfigError::InvalidCapacity {
                size_bytes: self.size_bytes,
            })?;
        if self.size_bytes == 0
            || !self.size_bytes.is_power_of_two()
            || !self.size_bytes.is_multiple_of(set_bytes)
        {
            return Err(Mips4CacheConfigError::InvalidCapacity {
                size_bytes: self.size_bytes,
            });
        }
        Ok((self.size_bytes / set_bytes) as usize)
    }
}

/// Geometry of the complete functional cache hierarchy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4CacheHierarchyConfig {
    /// Primary instruction cache, when present.
    pub instruction: Option<Mips4CacheGeometry>,
    /// Primary data cache, when present.
    pub data: Option<Mips4CacheGeometry>,
    /// Unified secondary cache, when present.
    pub secondary: Option<Mips4CacheGeometry>,
}

impl Mips4CacheHierarchyConfig {
    /// Creates a hierarchy configuration.
    pub const fn new(
        instruction: Option<Mips4CacheGeometry>,
        data: Option<Mips4CacheGeometry>,
        secondary: Option<Mips4CacheGeometry>,
    ) -> Self {
        Self {
            instruction,
            data,
            secondary,
        }
    }

    /// Creates a hierarchy that bypasses all caches.
    pub const fn disabled() -> Self {
        Self::new(None, None, None)
    }
}

/// Invalid functional cache configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4CacheConfigError {
    /// Only the cache-line size supported by the functional bus protocol may be used.
    UnsupportedLineSize {
        /// Requested line size.
        line_size_bytes: u32,
    },
    /// The way count was zero or not a power of two.
    InvalidWayCount {
        /// Requested way count.
        ways: u8,
    },
    /// Capacity was zero, not a power of two, or incompatible with the geometry.
    InvalidCapacity {
        /// Requested capacity.
        size_bytes: u32,
    },
    /// The secondary cache was not direct-mapped.
    SecondaryNotDirectMapped,
    /// An R5000 primary cache did not use its fixed geometry.
    InvalidR5000PrimaryGeometry,
    /// An R5000 secondary cache had an unsupported capacity or line size.
    InvalidR5000SecondaryGeometry,
    /// Profile and sampled boot-mode secondary-cache settings disagree.
    R5000SecondaryBootConflict,
}

impl fmt::Display for Mips4CacheConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedLineSize { line_size_bytes } => {
                write!(f, "unsupported cache line size {line_size_bytes}")
            }
            Self::InvalidWayCount { ways } => write!(f, "invalid cache way count {ways}"),
            Self::InvalidCapacity { size_bytes } => {
                write!(f, "invalid cache capacity {size_bytes}")
            }
            Self::SecondaryNotDirectMapped => write!(f, "secondary cache must be direct-mapped"),
            Self::InvalidR5000PrimaryGeometry => {
                write!(f, "R5000 primary caches must be 32 KiB with 32-byte lines")
            }
            Self::InvalidR5000SecondaryGeometry => write!(
                f,
                "R5000 secondary cache must be 512 KiB, 1 MiB, or 2 MiB with 32-byte lines"
            ),
            Self::R5000SecondaryBootConflict => {
                write!(
                    f,
                    "R5000 profile and boot-mode secondary cache settings conflict"
                )
            }
        }
    }
}

impl std::error::Error for Mips4CacheConfigError {}

/// One physical-byte cache line.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Mips4CacheLine {
    pub(crate) physical_line_base: u64,
    pub(crate) valid: bool,
    pub(crate) dirty: bool,
    pub(crate) virtual_index: u8,
    pub(crate) data: [u8; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
    pub(crate) check_bits: [u8; 4],
    pub(crate) tag_check_bit: bool,
}

impl Mips4CacheLine {
    pub(crate) const INVALID: Self = Self {
        physical_line_base: 0,
        valid: false,
        dirty: false,
        virtual_index: 0,
        data: [0; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
        check_bits: [0; 4],
        tag_check_bit: false,
    };

    pub(crate) fn from_data(
        physical_line_base: u64,
        virtual_address: u64,
        data: [u8; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
    ) -> Self {
        Self {
            physical_line_base,
            valid: true,
            dirty: false,
            virtual_index: ((virtual_address >> 12) & 0x07) as u8,
            check_bits: data_check_bits(&data),
            tag_check_bit: tag_check_bit(physical_line_base),
            data,
        }
    }

    pub(crate) fn read_lanes(self, physical_address: u64, bytes: usize) -> u64 {
        let offset = (physical_address - self.physical_line_base) as usize;
        let mut lanes = [0; 8];
        lanes[..bytes].copy_from_slice(&self.data[offset..offset + bytes]);
        u64::from_le_bytes(lanes)
    }

    pub(crate) fn write_lanes(
        &mut self,
        physical_address: u64,
        bytes: usize,
        lanes: u64,
        byte_enable: u8,
    ) {
        let offset = (physical_address - self.physical_line_base) as usize;
        let source = lanes.to_le_bytes();
        for (index, byte) in source.iter().copied().enumerate().take(bytes) {
            if byte_enable & (1 << index) != 0 {
                self.data[offset + index] = byte;
            }
        }
        self.check_bits = data_check_bits(&self.data);
    }

    pub(crate) fn recompute_check_bits(&mut self) {
        self.check_bits = data_check_bits(&self.data);
        self.tag_check_bit = tag_check_bit(self.physical_line_base);
    }

    pub(crate) fn check_errors(self, physical_address: u64, bytes: usize) -> (bool, bool) {
        let expected = data_check_bits(&self.data);
        let offset = (physical_address - self.physical_line_base) as usize;
        let first_doubleword = offset / 8;
        let last_doubleword = (offset + bytes - 1) / 8;
        let data_error = (first_doubleword..=last_doubleword)
            .any(|doubleword| self.check_bits[doubleword] != expected[doubleword]);
        let tag_error = self.tag_check_bit != tag_check_bit(self.physical_line_base);
        (data_error, tag_error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct Mips4SetAssociativeCache {
    geometry: Mips4CacheGeometry,
    sets: Vec<Vec<Mips4CacheLine>>,
    next_victim: Vec<u8>,
}

impl Mips4SetAssociativeCache {
    fn new(geometry: Mips4CacheGeometry) -> Result<Self, Mips4CacheConfigError> {
        let set_count = geometry.validate()?;
        Ok(Self {
            geometry,
            sets: vec![vec![Mips4CacheLine::INVALID; geometry.ways as usize]; set_count],
            next_victim: vec![0; set_count],
        })
    }

    fn set_index(&self, virtual_address: u64) -> usize {
        ((virtual_address / u64::from(self.geometry.line_size_bytes)) as usize) % self.sets.len()
    }

    fn lookup(&self, virtual_address: u64, physical_address: u64) -> Option<(usize, usize)> {
        let set = self.set_index(virtual_address);
        let base = line_base(physical_address);
        self.sets[set]
            .iter()
            .position(|line| line.valid && line.physical_line_base == base)
            .map(|way| (set, way))
    }

    fn victim(&mut self, virtual_address: u64) -> (usize, usize) {
        let set = self.set_index(virtual_address);
        if let Some(way) = self.sets[set].iter().position(|line| !line.valid) {
            return (set, way);
        }
        let way = self.next_victim[set] as usize;
        self.next_victim[set] = (self.next_victim[set] + 1) % self.geometry.ways;
        (set, way)
    }

    fn index_location(&self, virtual_address: u64) -> (usize, usize) {
        (
            self.set_index(virtual_address),
            ((virtual_address >> 14) as usize) & (self.geometry.ways as usize - 1),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct Mips4SecondaryCache {
    geometry: Mips4CacheGeometry,
    lines: Vec<Mips4CacheLine>,
}

impl Mips4SecondaryCache {
    fn new(geometry: Mips4CacheGeometry) -> Result<Self, Mips4CacheConfigError> {
        if geometry.ways != 1 {
            return Err(Mips4CacheConfigError::SecondaryNotDirectMapped);
        }
        let line_count = geometry.validate()?;
        Ok(Self {
            geometry,
            lines: vec![Mips4CacheLine::INVALID; line_count],
        })
    }

    fn index(&self, physical_address: u64) -> usize {
        ((line_base(physical_address) / u64::from(self.geometry.line_size_bytes)) as usize)
            % self.lines.len()
    }
}

/// Mutable functional cache hierarchy owned by one processor.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct Mips4CacheHierarchy {
    instruction: Option<Mips4SetAssociativeCache>,
    data: Option<Mips4SetAssociativeCache>,
    secondary: Option<Mips4SecondaryCache>,
}

impl Mips4CacheHierarchy {
    pub(crate) fn new(config: Mips4CacheHierarchyConfig) -> Result<Self, Mips4CacheConfigError> {
        Ok(Self {
            instruction: config
                .instruction
                .map(Mips4SetAssociativeCache::new)
                .transpose()?,
            data: config.data.map(Mips4SetAssociativeCache::new).transpose()?,
            secondary: config.secondary.map(Mips4SecondaryCache::new).transpose()?,
        })
    }

    pub(crate) fn instruction_lookup(
        &self,
        virtual_address: u64,
        physical_address: u64,
    ) -> Option<Mips4CacheLine> {
        let cache = self.instruction.as_ref()?;
        let (set, way) = cache.lookup(virtual_address, physical_address)?;
        Some(cache.sets[set][way])
    }

    pub(crate) fn data_lookup(
        &self,
        virtual_address: u64,
        physical_address: u64,
    ) -> Option<Mips4CacheLine> {
        let cache = self.data.as_ref()?;
        let (set, way) = cache.lookup(virtual_address, physical_address)?;
        Some(cache.sets[set][way])
    }

    pub(crate) fn data_write(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        bytes: usize,
        lanes: u64,
        byte_enable: u8,
        dirty: bool,
    ) -> bool {
        let Some(cache) = self.data.as_mut() else {
            return false;
        };
        let Some((set, way)) = cache.lookup(virtual_address, physical_address) else {
            return false;
        };
        let line = &mut cache.sets[set][way];
        line.write_lanes(physical_address, bytes, lanes, byte_enable);
        line.dirty |= dirty;
        true
    }

    pub(crate) fn choose_instruction_victim(
        &mut self,
        virtual_address: u64,
    ) -> Option<(usize, usize, Mips4CacheLine)> {
        let cache = self.instruction.as_mut()?;
        let (set, way) = cache.victim(virtual_address);
        Some((set, way, cache.sets[set][way]))
    }

    pub(crate) fn choose_data_victim(
        &mut self,
        virtual_address: u64,
    ) -> Option<(usize, usize, Mips4CacheLine)> {
        let cache = self.data.as_mut()?;
        let (set, way) = cache.victim(virtual_address);
        Some((set, way, cache.sets[set][way]))
    }

    pub(crate) fn install_instruction(&mut self, set: usize, way: usize, line: Mips4CacheLine) {
        self.instruction.as_mut().unwrap().sets[set][way] = line;
    }

    pub(crate) fn install_data(&mut self, set: usize, way: usize, line: Mips4CacheLine) {
        self.data.as_mut().unwrap().sets[set][way] = line;
    }

    pub(crate) fn secondary_lookup(&self, physical_address: u64) -> Option<Mips4CacheLine> {
        let cache = self.secondary.as_ref()?;
        let line = cache.lines[cache.index(physical_address)];
        (line.valid && line.physical_line_base == line_base(physical_address)).then_some(line)
    }

    pub(crate) fn secondary_install(&mut self, line: Mips4CacheLine) {
        let Some(cache) = self.secondary.as_mut() else {
            return;
        };
        let index = cache.index(line.physical_line_base);
        cache.lines[index] = Mips4CacheLine {
            dirty: false,
            ..line
        };
    }

    pub(crate) const fn has_instruction(&self) -> bool {
        self.instruction.is_some()
    }

    pub(crate) const fn has_data(&self) -> bool {
        self.data.is_some()
    }

    pub(crate) const fn has_secondary(&self) -> bool {
        self.secondary.is_some()
    }

    pub(crate) fn primary_index_line(
        &self,
        instruction: bool,
        virtual_address: u64,
    ) -> Option<Mips4CacheLine> {
        let cache = if instruction {
            self.instruction.as_ref()?
        } else {
            self.data.as_ref()?
        };
        let (set, way) = cache.index_location(virtual_address);
        Some(cache.sets[set][way])
    }

    pub(crate) fn primary_index_line_mut(
        &mut self,
        instruction: bool,
        virtual_address: u64,
    ) -> Option<&mut Mips4CacheLine> {
        let cache = if instruction {
            self.instruction.as_mut()?
        } else {
            self.data.as_mut()?
        };
        let (set, way) = cache.index_location(virtual_address);
        Some(&mut cache.sets[set][way])
    }

    pub(crate) fn primary_hit_line_mut(
        &mut self,
        instruction: bool,
        virtual_address: u64,
        physical_address: u64,
    ) -> Option<&mut Mips4CacheLine> {
        let cache = if instruction {
            self.instruction.as_mut()?
        } else {
            self.data.as_mut()?
        };
        let (set, way) = cache.lookup(virtual_address, physical_address)?;
        Some(&mut cache.sets[set][way])
    }

    pub(crate) fn secondary_index_line(&self, physical_address: u64) -> Option<Mips4CacheLine> {
        let cache = self.secondary.as_ref()?;
        Some(cache.lines[cache.index(physical_address)])
    }

    pub(crate) fn secondary_index_line_mut(
        &mut self,
        physical_address: u64,
    ) -> Option<&mut Mips4CacheLine> {
        let cache = self.secondary.as_mut()?;
        let index = cache.index(physical_address);
        Some(&mut cache.lines[index])
    }

    pub(crate) fn secondary_flash_invalidate(&mut self) {
        if let Some(cache) = self.secondary.as_mut() {
            for line in &mut cache.lines {
                line.valid = false;
                line.dirty = false;
            }
        }
    }

    pub(crate) fn secondary_page_invalidate(&mut self, physical_page: u64) {
        if let Some(cache) = self.secondary.as_mut() {
            for offset in (0..4096).step_by(MIPS4_FUNCTIONAL_CACHE_LINE_BYTES) {
                let address = physical_page + offset as u64;
                let index = cache.index(address);
                if cache.lines[index].physical_line_base == address {
                    cache.lines[index].valid = false;
                    cache.lines[index].dirty = false;
                }
            }
        }
    }
}

/// Returns the aligned physical base of a functional cache line.
pub(crate) const fn line_base(address: u64) -> u64 {
    address & !((MIPS4_FUNCTIONAL_CACHE_LINE_BYTES as u64) - 1)
}

fn data_check_bits(data: &[u8; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES]) -> [u8; 4] {
    let mut result = [0; 4];
    for doubleword in 0..4 {
        let mut bits = 0;
        for byte in 0..8 {
            if !data[doubleword * 8 + byte].count_ones().is_multiple_of(2) {
                bits |= 1 << byte;
            }
        }
        result[doubleword] = bits;
    }
    result
}

fn tag_check_bit(physical_line_base: u64) -> bool {
    !((physical_line_base >> 12) & 0x00ff_ffff)
        .count_ones()
        .is_multiple_of(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: Mips4CacheGeometry = Mips4CacheGeometry::new(32 * 1024, 32, 2);

    fn hierarchy(secondary: Option<Mips4CacheGeometry>) -> Mips4CacheHierarchy {
        Mips4CacheHierarchy::new(Mips4CacheHierarchyConfig::new(
            Some(PRIMARY),
            Some(PRIMARY),
            secondary,
        ))
        .unwrap()
    }

    #[test]
    fn geometry_rejects_invalid_line_way_and_secondary_shapes() {
        assert!(matches!(
            Mips4CacheHierarchy::new(Mips4CacheHierarchyConfig::new(
                Some(Mips4CacheGeometry::new(32 * 1024, 16, 2)),
                None,
                None,
            )),
            Err(Mips4CacheConfigError::UnsupportedLineSize { .. })
        ));
        assert!(matches!(
            Mips4CacheHierarchy::new(Mips4CacheHierarchyConfig::new(
                None,
                None,
                Some(Mips4CacheGeometry::new(512 * 1024, 32, 2)),
            )),
            Err(Mips4CacheConfigError::SecondaryNotDirectMapped)
        ));
    }

    #[test]
    fn primary_lookup_uses_virtual_set_and_physical_tag() {
        let mut cache = hierarchy(None);
        let virtual_address = 0xffff_ffff_8000_2460;
        let physical_address = 0x0123_4460;
        let (set, way, _) = cache.choose_data_victim(virtual_address).unwrap();
        let line = Mips4CacheLine::from_data(
            line_base(physical_address),
            virtual_address,
            [0x5a; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
        );
        cache.install_data(set, way, line);
        assert_eq!(
            cache.data_lookup(virtual_address, physical_address),
            Some(line)
        );
        assert!(
            cache
                .data_lookup(virtual_address ^ (1 << 13), physical_address)
                .is_none()
        );
        assert!(
            cache
                .data_lookup(virtual_address, physical_address ^ (1 << 12))
                .is_none()
        );
    }

    #[test]
    fn two_way_replacement_prefers_invalid_then_round_robins() {
        let mut cache = hierarchy(None);
        let virtual_address = 0xffff_ffff_8000_0040;
        for way in 0..2 {
            let (set, selected, _) = cache.choose_data_victim(virtual_address).unwrap();
            assert_eq!(selected, way);
            cache.install_data(
                set,
                selected,
                Mips4CacheLine::from_data(
                    (u64::from(way as u32) + 1) << 12,
                    virtual_address,
                    [way as u8; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
                ),
            );
        }
        assert_eq!(cache.choose_data_victim(virtual_address).unwrap().1, 0);
        assert_eq!(cache.choose_data_victim(virtual_address).unwrap().1, 1);
    }

    #[test]
    fn cache_lines_preserve_physical_byte_lanes_dirty_state_and_even_parity() {
        let mut line = Mips4CacheLine::from_data(0x1000, 0x8000_1000, [0; 32]);
        line.write_lanes(0x1003, 4, 0x8877_6655, 0x0f);
        assert_eq!(line.read_lanes(0x1003, 4), 0x8877_6655);
        assert_eq!(&line.data[3..7], &[0x55, 0x66, 0x77, 0x88]);
        assert_eq!(line.check_bits, data_check_bits(&line.data));
        assert_eq!(line.check_errors(0x1003, 4), (false, false));
        line.check_bits[0] ^= 1;
        assert_eq!(line.check_errors(0x1003, 4), (true, false));
        line.check_bits[0] ^= 1;
        line.tag_check_bit = !line.tag_check_bit;
        assert_eq!(line.check_errors(0x1003, 4), (false, true));
        assert!(!line.dirty);
    }

    #[test]
    fn secondary_cache_is_direct_mapped_by_physical_line() {
        let mut cache = hierarchy(Some(Mips4CacheGeometry::new(512 * 1024, 32, 1)));
        let first = Mips4CacheLine::from_data(0x2000, 0x8000_2000, [1; 32]);
        cache.secondary_install(first);
        assert_eq!(cache.secondary_lookup(0x2004), Some(first));
        let collision = Mips4CacheLine::from_data(0x8_2000, 0x8008_2000, [2; 32]);
        cache.secondary_install(collision);
        assert!(cache.secondary_lookup(0x2004).is_none());
        assert_eq!(cache.secondary_lookup(0x8_2004), Some(collision));
    }
}
