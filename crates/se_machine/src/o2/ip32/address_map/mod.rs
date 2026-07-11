//! SGI O2 IP32 CPU physical address classification.
//!
//! This module owns the board-level address ABI. Device models receive offsets
//! only after this classifier has selected a target and resolved memory aliases.

use se_core::component::ComponentId;

use super::component_ids;

const LOW_MEMORY_START: u64 = 0x0000_0000;
const LOW_MEMORY_END: u64 = 0x1000_0000;
const FRAME_BUFFER_START: u64 = 0x1000_0000;
const FRAME_BUFFER_END: u64 = 0x1200_0000;
const DEPTH_BUFFER_START: u64 = 0x1200_0000;
const DEPTH_BUFFER_END: u64 = 0x1400_0000;
const CRIME_REGISTERS_START: u64 = 0x1400_0000;
const CRIME_REGISTERS_END: u64 = 0x1600_0000;
const GBE_START: u64 = 0x1600_0000;
const GBE_END: u64 = 0x1700_0000;
const VICE_START: u64 = 0x1700_0000;
const VICE_END: u64 = 0x1800_0000;
const PCI_LOW_IO_START: u64 = 0x1800_0000;
const PCI_LOW_IO_END: u64 = 0x1a00_0000;
const PCI_LOW_MEMORY_START: u64 = 0x1a00_0000;
const PCI_LOW_MEMORY_END: u64 = 0x1c00_0000;
const PCI_CONFIG_START: u64 = 0x1c00_0000;
const PCI_CONFIG_END: u64 = 0x1c40_0000;
const MACE_RESERVED_START: u64 = 0x1c40_0000;
const MACE_RESERVED_END: u64 = 0x1f00_0000;
const MACE_IO_START: u64 = 0x1f00_0000;
const MACE_IO_END: u64 = 0x1f40_0000;
const MACE_AV_START: u64 = 0x1f40_0000;
const MACE_AV_END: u64 = 0x1f80_0000;
const MACE_SUPER_IO_START: u64 = 0x1f80_0000;
const MACE_SUPER_IO_END: u64 = 0x1fc0_0000;
const PROM_START: u64 = 0x1fc0_0000;
const PROM_END: u64 = 0x2000_0000;
const HIGH_MEMORY_START: u64 = 0x2000_0000;
const HIGH_MEMORY_END: u64 = 0x4000_0000;
const LINEAR_MEMORY_START: u64 = 0x4000_0000;
const LINEAR_MEMORY_END: u64 = 0x8000_0000;
const NO_ECC_MEMORY_START: u64 = 0x8000_0000;
const NO_ECC_MEMORY_END: u64 = 0xc000_0000;
const PCI_HIGH_IO_START: u64 = 0x1_0000_0000;
const PCI_HIGH_IO_END: u64 = 0x2_0000_0000;
const PCI_HIGH_MEMORY_START: u64 = 0x2_0000_0000;
const PCI_HIGH_MEMORY_END: u64 = 0x3_0000_0000;

/// Size of the physical System ROM window.
pub const IP32_PROM_WINDOW_SIZE_BYTES: u64 = PROM_END - PROM_START;

/// Size of the flash image mirrored through the System ROM window.
pub const IP32_PROM_IMAGE_SIZE_BYTES: usize = 512 * 1024;

/// Largest installed RAM size supported by the IP32 address map.
pub const IP32_MAX_RAM_SIZE_BYTES: u64 = LINEAR_MEMORY_END - LINEAR_MEMORY_START;

/// Named physical region in the IP32 CPU address map.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ip32PhysicalRegion {
    /// Low 256 MiB RAM alias.
    LowMemory,
    /// Rendering frame-buffer alias.
    FrameBuffer,
    /// Rendering depth-buffer alias.
    DepthBuffer,
    /// CRIME and rendering registers.
    CrimeRegisters,
    /// GBE registers.
    GbeRegisters,
    /// VICE registers and RAM.
    Vice,
    /// Low PCI I/O window.
    PciLowIo,
    /// Low PCI memory window.
    PciLowMemory,
    /// PCI configuration window.
    PciConfiguration,
    /// Reserved MACE-owned window.
    MaceReserved,
    /// MACE primary I/O registers.
    MaceIo,
    /// MACE audio and video registers.
    MaceAv,
    /// MACE Super I/O registers.
    MaceSuperIo,
    /// MACE System ROM window.
    SystemRom,
    /// Unconfirmed high-memory alias.
    HighMemoryUnconfirmed,
    /// Linear RAM alias.
    LinearMemory,
    /// RAM alias that bypasses ECC checking.
    NoEccMemory,
    /// High PCI I/O window.
    PciHighIo,
    /// High PCI memory window.
    PciHighMemory,
}

impl Ip32PhysicalRegion {
    /// Returns a stable trace name for the region.
    pub const fn trace_name(self) -> &'static str {
        match self {
            Self::LowMemory => "low_memory",
            Self::FrameBuffer => "frame_buffer",
            Self::DepthBuffer => "depth_buffer",
            Self::CrimeRegisters => "crime_registers",
            Self::GbeRegisters => "gbe_registers",
            Self::Vice => "vice",
            Self::PciLowIo => "pci_low_io",
            Self::PciLowMemory => "pci_low_memory",
            Self::PciConfiguration => "pci_configuration",
            Self::MaceReserved => "mace_reserved",
            Self::MaceIo => "mace_io",
            Self::MaceAv => "mace_av",
            Self::MaceSuperIo => "mace_super_io",
            Self::SystemRom => "system_rom",
            Self::HighMemoryUnconfirmed => "high_memory_unconfirmed",
            Self::LinearMemory => "linear_memory",
            Self::NoEccMemory => "no_ecc_memory",
            Self::PciHighIo => "pci_high_io",
            Self::PciHighMemory => "pci_high_memory",
        }
    }
}

/// Result of classifying one complete CPU transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip32AddressResolution {
    /// The transfer resolves to byte-addressed memory.
    Memory {
        /// Named address-map region.
        region: Ip32PhysicalRegion,
        /// Target component.
        target: ComponentId,
        /// Device-local byte offset.
        offset: u64,
        /// Whether the alias bypasses ECC checking.
        no_ecc: bool,
    },

    /// The transfer resolves to an unimplemented MMIO target.
    Stub {
        /// Named address-map region.
        region: Ip32PhysicalRegion,
        /// Target ASIC component.
        target: ComponentId,
    },

    /// The transfer is invalid or maps to a deliberately unimplemented range.
    Unmapped {
        /// Known region containing the first byte, when applicable.
        region: Option<Ip32PhysicalRegion>,
    },
}

/// Classifies one complete CPU physical transfer.
pub fn resolve(address: u64, size: u8, ram_size_bytes: u64) -> Ip32AddressResolution {
    if !(1..=8).contains(&size) {
        return Ip32AddressResolution::Unmapped { region: None };
    }
    let Some(end) = address.checked_add(u64::from(size)) else {
        return Ip32AddressResolution::Unmapped { region: None };
    };

    if in_region(address, end, LOW_MEMORY_START, LOW_MEMORY_END) {
        return memory_alias(
            Ip32PhysicalRegion::LowMemory,
            address,
            LOW_MEMORY_START,
            end,
            ram_size_bytes.min(LOW_MEMORY_END - LOW_MEMORY_START),
            false,
        );
    }
    if in_region(address, end, FRAME_BUFFER_START, FRAME_BUFFER_END) {
        return stub(Ip32PhysicalRegion::FrameBuffer, component_ids::CRIME);
    }
    if in_region(address, end, DEPTH_BUFFER_START, DEPTH_BUFFER_END) {
        return stub(Ip32PhysicalRegion::DepthBuffer, component_ids::CRIME);
    }
    if in_region(address, end, CRIME_REGISTERS_START, CRIME_REGISTERS_END) {
        return stub(Ip32PhysicalRegion::CrimeRegisters, component_ids::CRIME);
    }
    if in_region(address, end, GBE_START, GBE_END) {
        return stub(Ip32PhysicalRegion::GbeRegisters, component_ids::GBE);
    }
    if in_region(address, end, VICE_START, VICE_END) {
        return stub(Ip32PhysicalRegion::Vice, component_ids::VICE);
    }
    if in_region(address, end, PCI_LOW_IO_START, PCI_LOW_IO_END) {
        return stub(Ip32PhysicalRegion::PciLowIo, component_ids::MACE);
    }
    if in_region(address, end, PCI_LOW_MEMORY_START, PCI_LOW_MEMORY_END) {
        return stub(Ip32PhysicalRegion::PciLowMemory, component_ids::MACE);
    }
    if in_region(address, end, PCI_CONFIG_START, PCI_CONFIG_END) {
        return stub(Ip32PhysicalRegion::PciConfiguration, component_ids::MACE);
    }
    if in_region(address, end, MACE_RESERVED_START, MACE_RESERVED_END) {
        return stub(Ip32PhysicalRegion::MaceReserved, component_ids::MACE);
    }
    if in_region(address, end, MACE_IO_START, MACE_IO_END) {
        return stub(Ip32PhysicalRegion::MaceIo, component_ids::MACE);
    }
    if in_region(address, end, MACE_AV_START, MACE_AV_END) {
        return stub(Ip32PhysicalRegion::MaceAv, component_ids::MACE);
    }
    if in_region(address, end, MACE_SUPER_IO_START, MACE_SUPER_IO_END) {
        return stub(Ip32PhysicalRegion::MaceSuperIo, component_ids::MACE);
    }
    if in_region(address, end, PROM_START, PROM_END) {
        let offset = (address - PROM_START) % IP32_PROM_IMAGE_SIZE_BYTES as u64;
        if offset + u64::from(size) <= IP32_PROM_IMAGE_SIZE_BYTES as u64 {
            return Ip32AddressResolution::Memory {
                region: Ip32PhysicalRegion::SystemRom,
                target: component_ids::PROM,
                offset,
                no_ecc: false,
            };
        }
        return unmapped(Some(Ip32PhysicalRegion::SystemRom));
    }
    if in_region(address, end, HIGH_MEMORY_START, HIGH_MEMORY_END) {
        return unmapped(Some(Ip32PhysicalRegion::HighMemoryUnconfirmed));
    }
    if in_region(address, end, LINEAR_MEMORY_START, LINEAR_MEMORY_END) {
        return memory_alias(
            Ip32PhysicalRegion::LinearMemory,
            address,
            LINEAR_MEMORY_START,
            end,
            ram_size_bytes,
            false,
        );
    }
    if in_region(address, end, NO_ECC_MEMORY_START, NO_ECC_MEMORY_END) {
        return memory_alias(
            Ip32PhysicalRegion::NoEccMemory,
            address,
            NO_ECC_MEMORY_START,
            end,
            ram_size_bytes,
            true,
        );
    }
    if in_region(address, end, PCI_HIGH_IO_START, PCI_HIGH_IO_END) {
        return stub(Ip32PhysicalRegion::PciHighIo, component_ids::MACE);
    }
    if in_region(address, end, PCI_HIGH_MEMORY_START, PCI_HIGH_MEMORY_END) {
        return stub(Ip32PhysicalRegion::PciHighMemory, component_ids::MACE);
    }

    let region = region_containing(address);
    unmapped(region)
}

fn memory_alias(
    region: Ip32PhysicalRegion,
    address: u64,
    start: u64,
    transfer_end: u64,
    available_bytes: u64,
    no_ecc: bool,
) -> Ip32AddressResolution {
    let offset = address - start;
    let size = transfer_end - address;
    if offset + size <= available_bytes {
        Ip32AddressResolution::Memory {
            region,
            target: component_ids::RAM,
            offset,
            no_ecc,
        }
    } else {
        unmapped(Some(region))
    }
}

const fn stub(region: Ip32PhysicalRegion, target: ComponentId) -> Ip32AddressResolution {
    Ip32AddressResolution::Stub { region, target }
}

const fn unmapped(region: Option<Ip32PhysicalRegion>) -> Ip32AddressResolution {
    Ip32AddressResolution::Unmapped { region }
}

const fn in_region(address: u64, end: u64, start: u64, region_end: u64) -> bool {
    address >= start && address < region_end && end <= region_end
}

const fn region_containing(address: u64) -> Option<Ip32PhysicalRegion> {
    match address {
        FRAME_BUFFER_START..FRAME_BUFFER_END => Some(Ip32PhysicalRegion::FrameBuffer),
        DEPTH_BUFFER_START..DEPTH_BUFFER_END => Some(Ip32PhysicalRegion::DepthBuffer),
        CRIME_REGISTERS_START..CRIME_REGISTERS_END => Some(Ip32PhysicalRegion::CrimeRegisters),
        GBE_START..GBE_END => Some(Ip32PhysicalRegion::GbeRegisters),
        VICE_START..VICE_END => Some(Ip32PhysicalRegion::Vice),
        PCI_LOW_IO_START..PCI_LOW_IO_END => Some(Ip32PhysicalRegion::PciLowIo),
        PCI_LOW_MEMORY_START..PCI_LOW_MEMORY_END => Some(Ip32PhysicalRegion::PciLowMemory),
        PCI_CONFIG_START..PCI_CONFIG_END => Some(Ip32PhysicalRegion::PciConfiguration),
        MACE_RESERVED_START..MACE_RESERVED_END => Some(Ip32PhysicalRegion::MaceReserved),
        MACE_IO_START..MACE_IO_END => Some(Ip32PhysicalRegion::MaceIo),
        MACE_AV_START..MACE_AV_END => Some(Ip32PhysicalRegion::MaceAv),
        MACE_SUPER_IO_START..MACE_SUPER_IO_END => Some(Ip32PhysicalRegion::MaceSuperIo),
        PROM_START..PROM_END => Some(Ip32PhysicalRegion::SystemRom),
        HIGH_MEMORY_START..HIGH_MEMORY_END => Some(Ip32PhysicalRegion::HighMemoryUnconfirmed),
        LINEAR_MEMORY_START..LINEAR_MEMORY_END => Some(Ip32PhysicalRegion::LinearMemory),
        NO_ECC_MEMORY_START..NO_ECC_MEMORY_END => Some(Ip32PhysicalRegion::NoEccMemory),
        PCI_HIGH_IO_START..PCI_HIGH_IO_END => Some(Ip32PhysicalRegion::PciHighIo),
        PCI_HIGH_MEMORY_START..PCI_HIGH_MEMORY_END => Some(Ip32PhysicalRegion::PciHighMemory),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
