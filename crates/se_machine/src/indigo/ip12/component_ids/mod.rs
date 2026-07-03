//! Stable component identifiers for the SGI Indigo IP12 topology.
//!
//! These identifiers describe the board-level blocks visible in the IP12
//! technical-report diagrams: CPU bus, GIO32 bus, peripheral bus, processor
//! core blocks, PIC1, INT2, HPC1.5, memory and firmware blocks, I/O blocks,
//! audio blocks, and GIO/graphics attachment points.
//!
//! The range is sparse and grouped by subsystem so related blocks can keep
//! stable identities. Identifiers are assigned to stable topological blocks.
//! Internal state machines, FIFO buffers, audio converters, and similar
//! chip-private implementation details are represented by their parent blocks.

use se_core::component::ComponentId;

const IP12_COMPONENT_BASE: u64 = 0x1200_0000;

const fn ip12_id(offset: u64) -> ComponentId {
    ComponentId::new(IP12_COMPONENT_BASE + offset)
}

/// Whole-machine coordination component.
pub const MACHINE: ComponentId = ip12_id(0x0000);

/// CPU local bus.
pub const CPU_BUS: ComponentId = ip12_id(0x0100);

/// GIO32 system bus.
pub const GIO32_BUS: ComponentId = ip12_id(0x0101);

/// Peripheral bus controlled through HPC1.5.
pub const PBUS: ComponentId = ip12_id(0x0102);

/// R3000A CPU core.
pub const CPU0: ComponentId = ip12_id(0x0200);

/// R3010 floating-point unit.
pub const FPU0: ComponentId = ip12_id(0x0201);

/// CPU instruction cache.
pub const ICACHE0: ComponentId = ip12_id(0x0202);

/// CPU data cache.
pub const DCACHE0: ComponentId = ip12_id(0x0203);

/// PIC1 CPU-bus, memory-controller, and GIO interface ASIC.
pub const PIC1: ComponentId = ip12_id(0x0300);

/// INT2 interrupt and timer ASIC.
pub const INT2: ComponentId = ip12_id(0x0301);

/// HPC1.5 GIO32, peripheral-bus, and I/O interface ASIC.
pub const HPC15: ComponentId = ip12_id(0x0302);

/// Main system RAM.
pub const RAM: ComponentId = ip12_id(0x0400);

/// Boot PROM.
pub const PROM: ComponentId = ip12_id(0x0401);

/// Non-volatile serial memory.
pub const NVRAM: ComponentId = ip12_id(0x0402);

/// Real-time clock.
pub const RTC: ComponentId = ip12_id(0x0403);

/// WD33C93B SCSI controller.
pub const SCSI_CONTROLLER: ComponentId = ip12_id(0x0500);

/// External SCSI bus.
pub const SCSI_BUS: ComponentId = ip12_id(0x0501);

/// Ethernet controller block.
pub const ETHERNET_CONTROLLER: ComponentId = ip12_id(0x0502);

/// Parallel-port interface.
pub const PARALLEL_PORT: ComponentId = ip12_id(0x0503);

/// First Z85230 DUART.
pub const DUART0: ComponentId = ip12_id(0x0504);

/// Second Z85230 DUART.
pub const DUART1: ComponentId = ip12_id(0x0505);

/// Board-level audio subsystem.
pub const AUDIO_SUBSYSTEM: ComponentId = ip12_id(0x0600);

/// Motorola DSP56001 audio processor.
pub const AUDIO_DSP: ComponentId = ip12_id(0x0601);

/// Audio SRAM attached to the DSP and peripheral bus.
pub const AUDIO_SRAM: ComponentId = ip12_id(0x0602);

/// First GIO32 expansion slot.
pub const GIO_SLOT0: ComponentId = ip12_id(0x0700);

/// Second GIO32 expansion slot.
pub const GIO_SLOT1: ComponentId = ip12_id(0x0701);

/// Graphics backplane attachment point.
pub const GRAPHICS_BACKPLANE: ComponentId = ip12_id(0x0702);

/// Primary graphics device placeholder.
pub const GRAPHICS0: ComponentId = ip12_id(0x0703);

/// All IP12 component identifiers in stable definition order.
pub const ALL_COMPONENT_IDS: [ComponentId; 28] = [
    MACHINE,
    CPU_BUS,
    GIO32_BUS,
    PBUS,
    CPU0,
    FPU0,
    ICACHE0,
    DCACHE0,
    PIC1,
    INT2,
    HPC15,
    RAM,
    PROM,
    NVRAM,
    RTC,
    SCSI_CONTROLLER,
    SCSI_BUS,
    ETHERNET_CONTROLLER,
    PARALLEL_PORT,
    DUART0,
    DUART1,
    AUDIO_SUBSYSTEM,
    AUDIO_DSP,
    AUDIO_SRAM,
    GIO_SLOT0,
    GIO_SLOT1,
    GRAPHICS_BACKPLANE,
    GRAPHICS0,
];

/// IP12 bus component identifiers.
pub const BUS_COMPONENT_IDS: [ComponentId; 3] = [CPU_BUS, GIO32_BUS, PBUS];

#[cfg(test)]
mod tests;
