//! CRIME Rendering Engine register front end and fixed-point helpers.

use std::collections::{BTreeMap, VecDeque};

use super::registers;

const PIXEL_PIPE_BASE: u64 = registers::CRIME_RENDER_BASE + 0x2000;
const MTE_BASE: u64 = registers::CRIME_RENDER_BASE + 0x3000;
const STATUS_BASE: u64 = registers::CRIME_RENDER_BASE + 0x4000;
const INTERFACE_CAPACITY: usize = 128;

/// One host register write retained by the Rendering Engine interface buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderRegisterWrite {
    /// Register address without commit tagging.
    pub address: u64,

    /// Register value.
    pub value: u64,

    /// Access width in bytes.
    pub size: u8,
}

/// CRIME Rendering Engine front-end state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeRender {
    interface: VecDeque<RenderRegisterWrite>,
    registers: BTreeMap<u64, u64>,
    epoch: u64,
}

impl CrimeRender {
    /// Creates reset Rendering Engine state.
    pub const fn new() -> Self {
        Self {
            interface: VecDeque::new(),
            registers: BTreeMap::new(),
            epoch: 0,
        }
    }

    /// Resets the front end and invalidates old render events.
    pub fn reset(&mut self) {
        self.interface.clear();
        self.registers.clear();
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Returns the active Rendering Engine epoch.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Returns the number of host writes waiting in the 128-entry interface buffer.
    pub fn interface_level(&self) -> usize {
        self.interface.len()
    }

    /// Reads a software-visible Rendering Engine register.
    pub fn read(&self, address: u64, size: u8) -> Option<u64> {
        let access = register_access(address, size)?;
        if !access.readable {
            return None;
        }
        if address == STATUS_BASE {
            return Some(self.interface.len() as u64);
        }
        Some(self.registers.get(&address).copied().unwrap_or(0))
    }

    /// Queues one software-visible Rendering Engine register write.
    pub fn write(&mut self, address: u64, size: u8, value: u64) -> Result<(), RenderWriteError> {
        let Some(access) = register_access(address, size) else {
            return Err(RenderWriteError::UndefinedRegister);
        };
        if !access.writable {
            return Err(RenderWriteError::UndefinedRegister);
        }
        if self.interface.len() == INTERFACE_CAPACITY {
            return Err(RenderWriteError::InterfaceFull);
        }
        self.interface.push_back(RenderRegisterWrite {
            address,
            value,
            size,
        });
        Ok(())
    }

    /// Retires at most one host write into the active register file.
    pub fn retire_one(&mut self) -> Option<RenderRegisterWrite> {
        let write = self.interface.pop_front()?;
        self.registers.insert(write.address, write.value);
        Some(write)
    }
}

impl Default for CrimeRender {
    fn default() -> Self {
        Self::new()
    }
}

/// Rendering Engine host-write failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderWriteError {
    /// Address or access width is not defined by the register map.
    UndefinedRegister,

    /// The 128-entry host interface buffer has no free entry.
    InterfaceFull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegisterAccess {
    readable: bool,
    writable: bool,
}

const READ_WRITE: RegisterAccess = RegisterAccess {
    readable: true,
    writable: true,
};
const WRITE_ONLY: RegisterAccess = RegisterAccess {
    readable: false,
    writable: true,
};
const READ_ONLY: RegisterAccess = RegisterAccess {
    readable: true,
    writable: false,
};

const fn register_access(address: u64, size: u8) -> Option<RegisterAccess> {
    if address >= registers::CRIME_RENDER_BASE + 0x1000
        && address <= registers::CRIME_RENDER_BASE + 0x17f8
    {
        return if size == 8 && address & 7 == 0 {
            Some(READ_WRITE)
        } else {
            None
        };
    }
    if address >= PIXEL_PIPE_BASE && address <= PIXEL_PIPE_BASE + 0x1f8 {
        let offset = address - PIXEL_PIPE_BASE;
        let access = match offset {
            0x000 | 0x008 | 0x010 | 0x018 | 0x050 | 0x058 | 0x0c0 | 0x0c4 | 0x0d8 | 0x110
            | 0x160 | 0x168 | 0x170 | 0x198 | 0x1a0 | 0x1a8 | 0x1b0 | 0x1b8 | 0x1c0 | 0x1e0
            | 0x1e8
                if size == 4 =>
            {
                READ_WRITE
            }
            0x020 | 0x028 | 0x030 | 0x038 | 0x040 | 0x048 | 0x118 if size == 8 => READ_WRITE,
            0x060 | 0x070 | 0x074 | 0x078 | 0x080 | 0x084 | 0x088 | 0x08c | 0x090 | 0x094
            | 0x0a0 | 0x0a8 | 0x0ac | 0x0b0 | 0x0b4 | 0x0d0 | 0x0e0 | 0x0e4 | 0x0e8 | 0x0ec
            | 0x0f0 | 0x0f4 | 0x0f8 | 0x0fc | 0x100 | 0x104 | 0x108 | 0x10c | 0x130 | 0x158
            | 0x15c | 0x178 | 0x180 | 0x188 | 0x190 | 0x194 | 0x1f0 | 0x1f8
                if size == 4 =>
            {
                WRITE_ONLY
            }
            0x120 | 0x128 | 0x138 | 0x140 | 0x148 | 0x150 | 0x1c8 | 0x1d0 | 0x1d8 if size == 8 => {
                WRITE_ONLY
            }
            _ => return None,
        };
        return Some(access);
    }
    if address >= MTE_BASE && address <= MTE_BASE + 0x78 {
        if size != 4 {
            return None;
        }
        return match address - MTE_BASE {
            0x00 | 0x08 | 0x18 | 0x20 | 0x28 | 0x40 | 0x48 => Some(READ_WRITE),
            0x10 | 0x30 | 0x38 | 0x70 | 0x78 => Some(WRITE_ONLY),
            _ => None,
        };
    }
    if address == STATUS_BASE && size == 4 {
        return Some(READ_ONLY);
    }
    None
}

/// Applies one CRIME bitwise logic operation.
pub const fn logic_operation(operation: u8, source: u32, destination: u32) -> u32 {
    match operation & 0xf {
        0 => 0,
        1 => source & destination,
        2 => source & !destination,
        3 => source,
        4 => !source & destination,
        5 => destination,
        6 => source ^ destination,
        7 => source | destination,
        8 => !(source | destination),
        9 => !(source ^ destination),
        10 => !destination,
        11 => source | !destination,
        12 => !source,
        13 => !source | destination,
        14 => !(source & destination),
        _ => u32::MAX,
    }
}

/// Evaluates one eight-function comparison used by alpha, depth, and stencil tests.
pub const fn compare(function: u8, source: u32, reference: u32) -> bool {
    match function & 7 {
        0 => false,
        1 => source < reference,
        2 => source == reference,
        3 => source <= reference,
        4 => source > reference,
        5 => source != reference,
        6 => source >= reference,
        _ => true,
    }
}

#[cfg(test)]
mod tests;
