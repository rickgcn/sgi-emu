//! Stable component identifiers for the SGI O2 IP32 topology.
//!
//! These identifiers describe board-level O2 blocks: machine orchestration,
//! CPU and I/O communication domains, CPU core blocks, CRIME, MACE, GBE, VICE,
//! memory and firmware blocks, and external I/O attachment points.
//!
//! The range is sparse and grouped by subsystem so related blocks can keep
//! stable identities. Identifiers are assigned to stable topological blocks.
//! Chip-private FIFOs, register banks, state machines, and other internal
//! details are represented by their parent blocks.

use se_core::component::ComponentId;

const IP32_COMPONENT_BASE: u64 = 0x3200_0000;

const fn ip32_id(offset: u64) -> ComponentId {
    ComponentId::new(IP32_COMPONENT_BASE + offset)
}

/// Whole-machine coordination component.
pub const MACHINE: ComponentId = ip32_id(0x0000);

/// CPU SysAD communication domain.
pub const CPU_SYSAD_BUS: ComponentId = ip32_id(0x0100);

/// CRIME-managed memory communication domain.
pub const CRIME_MEMORY_DOMAIN: ComponentId = ip32_id(0x0101);

/// CRIME-to-MACE interconnect.
pub const CRIME_MACE_LINK: ComponentId = ip32_id(0x0102);

/// CRIME-to-GBE interconnect.
pub const CRIME_GBE_LINK: ComponentId = ip32_id(0x0103);

/// MACE-managed PCI bus.
pub const PCI_BUS: ComponentId = ip32_id(0x0104);

/// MACE-managed ISA island.
pub const ISA_BUS: ComponentId = ip32_id(0x0105);

/// Primary MIPS CPU core.
pub const CPU0: ComponentId = ip32_id(0x0200);

/// Primary floating-point unit.
pub const FPU0: ComponentId = ip32_id(0x0201);

/// CPU instruction cache.
pub const ICACHE0: ComponentId = ip32_id(0x0202);

/// CPU data cache.
pub const DCACHE0: ComponentId = ip32_id(0x0203);

/// CPU secondary cache.
pub const SCACHE0: ComponentId = ip32_id(0x0204);

/// CRIME CPU interface, memory controller, rendering, and interrupt ASIC.
pub const CRIME: ComponentId = ip32_id(0x0300);

/// MACE I/O ASIC.
pub const MACE: ComponentId = ip32_id(0x0301);

/// GBE display ASIC.
pub const GBE: ComponentId = ip32_id(0x0302);

/// VICE image and compression ASIC.
pub const VICE: ComponentId = ip32_id(0x0303);

/// Unified system RAM.
pub const RAM: ComponentId = ip32_id(0x0400);

/// System boot PROM.
pub const PROM: ComponentId = ip32_id(0x0401);

/// Non-volatile RAM storage.
pub const NVRAM: ComponentId = ip32_id(0x0402);

/// Real-time clock.
pub const RTC: ComponentId = ip32_id(0x0403);

/// Board-level PCI SCSI controller.
pub const SCSI_CONTROLLER: ComponentId = ip32_id(0x0500);

/// External SCSI bus.
pub const SCSI_BUS: ComponentId = ip32_id(0x0501);

/// MACE Ethernet controller block.
pub const ETHERNET_CONTROLLER: ComponentId = ip32_id(0x0502);

/// First 16550-compatible serial port.
pub const SERIAL0: ComponentId = ip32_id(0x0503);

/// Second 16550-compatible serial port.
pub const SERIAL1: ComponentId = ip32_id(0x0504);

/// Parallel-port interface.
pub const PARALLEL_PORT: ComponentId = ip32_id(0x0505);

/// Keyboard controller endpoint.
pub const KEYBOARD: ComponentId = ip32_id(0x0506);

/// Mouse controller endpoint.
pub const MOUSE: ComponentId = ip32_id(0x0507);

/// Board-level audio subsystem.
pub const AUDIO_SUBSYSTEM: ComponentId = ip32_id(0x0508);

/// First video input channel.
pub const VIDEO_INPUT0: ComponentId = ip32_id(0x0509);

/// Second video input channel.
pub const VIDEO_INPUT1: ComponentId = ip32_id(0x050a);

/// Video output channel.
pub const VIDEO_OUTPUT: ComponentId = ip32_id(0x050b);

/// External PCI slot.
pub const PCI_SLOT0: ComponentId = ip32_id(0x050c);

/// All IP32 component identifiers in stable definition order.
pub const ALL_COMPONENT_IDS: [ComponentId; 33] = [
    MACHINE,
    CPU_SYSAD_BUS,
    CRIME_MEMORY_DOMAIN,
    CRIME_MACE_LINK,
    CRIME_GBE_LINK,
    PCI_BUS,
    ISA_BUS,
    CPU0,
    FPU0,
    ICACHE0,
    DCACHE0,
    SCACHE0,
    CRIME,
    MACE,
    GBE,
    VICE,
    RAM,
    PROM,
    NVRAM,
    RTC,
    SCSI_CONTROLLER,
    SCSI_BUS,
    ETHERNET_CONTROLLER,
    SERIAL0,
    SERIAL1,
    PARALLEL_PORT,
    KEYBOARD,
    MOUSE,
    AUDIO_SUBSYSTEM,
    VIDEO_INPUT0,
    VIDEO_INPUT1,
    VIDEO_OUTPUT,
    PCI_SLOT0,
];

/// IP32 bus and communication-domain component identifiers.
pub const BUS_COMPONENT_IDS: [ComponentId; 6] = [
    CPU_SYSAD_BUS,
    CRIME_MEMORY_DOMAIN,
    CRIME_MACE_LINK,
    CRIME_GBE_LINK,
    PCI_BUS,
    ISA_BUS,
];

#[cfg(test)]
mod tests;
