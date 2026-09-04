//! Read-only debugger access to an Indigo IP12.

use se_core::bus::PhysAddr;
use se_cpu::mips1::r3000::debug::{
    CacheDebugSnapshot, CacheView, PendingCp1DebugSnapshot, R3000DebugSnapshot, TlbDebugSnapshot,
    TlbView, VirtualAddressView, disassemble,
};
use sha2::{Digest, Sha256};

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
    /// Computes the machine-defined fingerprint used to detect execution
    /// divergence without exposing processor-specific debug state.
    pub(crate) fn machine_state_fingerprint(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"sgi-emu-machine-state-fingerprint-v1");
        let cpu = self.cpu.debug_snapshot();
        hash_u32(&mut hasher, cpu.pc);
        hash_u32(&mut hasher, cpu.hi);
        hash_u32(&mut hasher, cpu.lo);
        for value in cpu.gpr {
            hash_u32(&mut hasher, value);
        }
        match cpu.delay_slot {
            None => hasher.update([0]),
            Some(slot) => {
                hasher.update([1]);
                hash_u32(&mut hasher, slot.origin_pc);
                hash_u32(&mut hasher, slot.resume_pc);
            }
        }
        match cpu.pending_gpr {
            None => hasher.update([0]),
            Some(write) => {
                hasher.update([1]);
                hash_usize(&mut hasher, write.index);
                hash_u32(&mut hasher, write.value);
                hash_bool(&mut hasher, write.load_merge_bypass);
            }
        }
        match cpu.pending_cp0 {
            None => hasher.update([0]),
            Some(write) => {
                hasher.update([1]);
                hash_usize(&mut hasher, write.index);
                hash_u32(&mut hasher, write.value);
            }
        }
        match cpu.pending_cp1 {
            None => hasher.update([0]),
            Some(PendingCp1DebugSnapshot::General { index, value }) => {
                hasher.update([1]);
                hash_usize(&mut hasher, index);
                hash_u32(&mut hasher, value);
            }
            Some(PendingCp1DebugSnapshot::Control { index, value }) => {
                hasher.update([2]);
                hash_usize(&mut hasher, index);
                hash_u32(&mut hasher, value);
            }
            Some(PendingCp1DebugSnapshot::Condition { value }) => {
                hasher.update([3]);
                hash_bool(&mut hasher, value);
            }
        }
        hasher.update([cpu.interrupt_inputs.asserted]);
        hasher.update([cpu.interrupt_inputs.sampled]);
        for value in cpu.cp0.registers {
            hash_u32(&mut hasher, value);
        }
        hash_u32(&mut hasher, cpu.cp0.effective.coprocessor_usable);
        hash_u32(&mut hasher, cpu.cp0.effective.interrupt_control);
        hash_u32(&mut hasher, cpu.cp0.effective.software_interrupts);
        match cpu.cp0.pending_functional {
            None => hasher.update([0]),
            Some(state) => {
                hasher.update([1]);
                hash_u32(&mut hasher, state.coprocessor_usable);
                hash_u32(&mut hasher, state.interrupt_control);
                hash_u32(&mut hasher, state.software_interrupts);
            }
        }
        for value in cpu.cp1.registers {
            hash_u32(&mut hasher, value);
        }
        hash_u32(&mut hasher, cpu.cp1.fcr0);
        hash_u32(&mut hasher, cpu.cp1.fcr30);
        hash_u32(&mut hasher, cpu.cp1.fcr31);
        hash_bool(&mut hasher, cpu.cp1.interrupt_asserted);

        hasher.finalize().into()
    }

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

fn hash_u32(hasher: &mut Sha256, value: u32) {
    hasher.update(value.to_le_bytes());
}

fn hash_usize(hasher: &mut Sha256, value: usize) {
    hasher.update((value as u64).to_le_bytes());
}

fn hash_bool(hasher: &mut Sha256, value: bool) {
    hasher.update([u8::from(value)]);
}

#[cfg(test)]
mod tests {
    use se_cpu::mips1::r3000::debug::TlbView;
    use se_float::backend::Backend;

    use super::{DebugRequest, DebugResponse, Ip12, MemoryAddressSpace};
    use crate::indigo::ip12::PROM_BYTES;

    #[test]
    fn debug_queries_do_not_advance_the_machine() {
        let machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat, None, None).unwrap();
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
    fn machine_state_fingerprint_is_stable_and_tracks_processor_state() {
        let mut machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat, None, None).unwrap();
        let baseline = machine.machine_state_fingerprint();

        assert_eq!(baseline, machine.machine_state_fingerprint());
        machine.execute_instruction().unwrap();
        assert_ne!(baseline, machine.machine_state_fingerprint());
    }

    #[test]
    fn machine_state_fingerprint_tracks_uncommitted_interrupt_input() {
        let mut machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat, None, None).unwrap();
        let baseline = machine.machine_state_fingerprint();

        machine.cpu.set_hardware_interrupt_lines(1 << 3);

        assert_ne!(baseline, machine.machine_state_fingerprint());
        let snapshot = machine.cpu.debug_snapshot();
        assert_eq!(snapshot.interrupt_inputs.asserted, 1 << 3);
        assert_eq!(snapshot.interrupt_inputs.sampled, 0);
        assert_eq!(snapshot.cp0.registers[13] & 0x0000_fc00, 0);
    }

    #[test]
    fn physical_memory_reports_mapped_and_unmapped_bytes() {
        let machine = Ip12::new(vec![0; PROM_BYTES], Backend::SoftFloat, None, None).unwrap();
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
