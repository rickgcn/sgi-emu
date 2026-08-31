//! Read-only debugger access to an Indigo IP12.

use se_core::bus::PhysAddr;
use se_cpu::mips1::r3000::debug::{
    CacheDebugSnapshot, CacheView, R3000DebugSnapshot, TlbDebugSnapshot, TlbView,
    VirtualAddressView, disassemble,
};

use super::Ip12;

/// Small machine state sampled at one instruction boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip12DebugSnapshot {
    /// Processor state.
    pub cpu: R3000DebugSnapshot,
}

/// Selects the address space used by a memory request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryAddressSpace {
    /// IP12 physical addresses.
    Physical,
    /// R3000 data virtual addresses.
    Virtual,
}

/// One disassembled instruction row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisassemblyLine {
    /// Virtual instruction address.
    pub address: u32,
    /// Instruction word, or `None` when the address is unreadable.
    pub word: Option<u32>,
    /// Decoded instruction text, or an unreadable marker.
    pub text: String,
}

/// A side-effect-free memory range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryDebugSnapshot {
    /// Requested address space.
    pub address_space: MemoryAddressSpace,
    /// First requested address.
    pub start: u64,
    /// One optional byte for each requested address.
    pub bytes: Vec<Option<u8>>,
}

/// Selects one read-only debugger query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugRequest {
    /// Samples processor registers and pending architectural effects.
    Registers,
    /// Samples one TLB view.
    Tlb(TlbView),
    /// Samples one cache bank.
    Cache(CacheView),
    /// Reads and disassembles virtual instructions.
    Disassembly {
        /// First virtual address.
        start: u32,
        /// Number of instruction rows.
        row_count: usize,
    },
    /// Reads a byte range.
    Memory {
        /// Address space to use.
        address_space: MemoryAddressSpace,
        /// First address.
        start: u64,
        /// Number of bytes.
        length: usize,
    },
}

/// Result of one debugger query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugResponse {
    /// Processor registers and pending architectural effects.
    Registers(Box<Ip12DebugSnapshot>),
    /// One TLB view.
    Tlb(TlbDebugSnapshot),
    /// One cache bank.
    Cache(CacheDebugSnapshot),
    /// Disassembled instruction rows.
    Disassembly(Vec<DisassemblyLine>),
    /// One memory range.
    Memory(MemoryDebugSnapshot),
}

impl Ip12 {
    /// Performs one side-effect-free debugger query.
    #[must_use]
    pub fn debug(&self, request: DebugRequest) -> DebugResponse {
        match request {
            DebugRequest::Registers => DebugResponse::Registers(Box::new(Ip12DebugSnapshot {
                cpu: self.cpu.debug_snapshot(),
            })),
            DebugRequest::Tlb(view) => DebugResponse::Tlb(self.cpu.tlb_debug_snapshot(view)),
            DebugRequest::Cache(view) => DebugResponse::Cache(self.cpu.cache_debug_snapshot(view)),
            DebugRequest::Disassembly { start, row_count } => {
                DebugResponse::Disassembly(self.debug_disassembly(start, row_count))
            }
            DebugRequest::Memory {
                address_space,
                start,
                length,
            } => DebugResponse::Memory(self.debug_memory(address_space, start, length)),
        }
    }

    fn debug_disassembly(&self, start: u32, row_count: usize) -> Vec<DisassemblyLine> {
        (0..row_count)
            .map(|row| {
                let byte_offset = u32::try_from(row).unwrap_or(u32::MAX).wrapping_mul(4);
                let address = start.wrapping_add(byte_offset);
                let word = self
                    .cpu
                    .debug_translate_address(address, VirtualAddressView::Instruction)
                    .and_then(|physical| self.debug_read_word(physical));
                let text = word.map_or_else(
                    || String::from("<unreadable>"),
                    |instruction| disassemble(instruction, address),
                );
                DisassemblyLine {
                    address,
                    word,
                    text,
                }
            })
            .collect()
    }

    fn debug_memory(
        &self,
        address_space: MemoryAddressSpace,
        start: u64,
        length: usize,
    ) -> MemoryDebugSnapshot {
        let bytes = (0..length)
            .map(|offset| {
                let address = start.checked_add(offset as u64)?;
                let physical = match address_space {
                    MemoryAddressSpace::Physical => Some(PhysAddr::new(address)),
                    MemoryAddressSpace::Virtual => {
                        u32::try_from(address).ok().and_then(|virtual_address| {
                            self.cpu
                                .debug_translate_address(virtual_address, VirtualAddressView::Data)
                        })
                    }
                }?;
                self.debug_read_byte(physical)
            })
            .collect();

        MemoryDebugSnapshot {
            address_space,
            start,
            bytes,
        }
    }

    fn debug_read_word(&self, address: PhysAddr) -> Option<u32> {
        let mut bytes = [0; 4];
        self.bus.debug_read(address, &mut bytes).ok()?;
        Some(u32::from_be_bytes(bytes))
    }

    fn debug_read_byte(&self, address: PhysAddr) -> Option<u8> {
        let mut byte = [0];
        self.bus.debug_read(address, &mut byte).ok()?;
        Some(byte[0])
    }
}

#[cfg(test)]
mod tests {
    use se_cpu::mips1::r3000::debug::TlbView;
    use se_float::backend::Backend;

    use super::{DebugRequest, DebugResponse, Ip12, MemoryAddressSpace};
    use crate::indigo::ip12::PROM_BYTES;

    #[test]
    fn debug_queries_do_not_advance_the_machine() {
        let machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat).unwrap();
        let pc = machine.execution_address();

        assert!(matches!(
            machine.debug(DebugRequest::Registers),
            DebugResponse::Registers(_)
        ));
        assert!(matches!(
            machine.debug(DebugRequest::Tlb(TlbView::Main)),
            DebugResponse::Tlb(_)
        ));
        assert_eq!(machine.execution_address(), pc);
    }

    #[test]
    fn physical_memory_reports_mapped_and_unmapped_bytes() {
        let machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat).unwrap();
        let response = machine.debug(DebugRequest::Memory {
            address_space: MemoryAddressSpace::Physical,
            start: 0x1fbf_ffff,
            length: 2,
        });

        let DebugResponse::Memory(snapshot) = response else {
            panic!("memory request returned the wrong response")
        };
        assert_eq!(snapshot.bytes, vec![None, Some(0)]);
    }
}
