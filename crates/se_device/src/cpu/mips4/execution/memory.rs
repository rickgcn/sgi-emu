//! Integer load and store execution.

use crate::cpu::mips4::cache::hierarchy::Mips4CacheAccessPolicy;
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::gpr::{Mips4GprIndex, sign_extend_word};
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::decode::Mips4CpuInstruction;
use crate::cpu::mips4::memory::ll_sc::Mips4LlBit;
use crate::cpu::mips4::memory::operation::{Mips4MemoryAccess, Mips4MemoryAccessError};
use crate::cpu::mips4::memory::{Mips4Memory, Mips4MemoryAccessKind, Mips4MemoryAccessSize};
use crate::cpu::mips4::mmu::Mips4MmuCacheAttribute;
use crate::cpu::mips4::tlb::Mips4TlbAsid;

use super::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransaction, Mips4ExecutionTransferSize};
use super::policy::Mips4ExecutionPolicy;
use super::state::Mips4ExecutionState;

pub(super) enum Mips4MemoryPlan {
    Read {
        pending: Mips4PendingRead,
        transaction: Mips4ExecutionTransaction,
        virtual_address: u64,
        cache_policy: Mips4CacheAccessPolicy,
    },
    Write {
        pending: Mips4PendingWrite,
        transaction: Mips4ExecutionTransaction,
        virtual_address: u64,
        cache_policy: Mips4CacheAccessPolicy,
    },
    Retire {
        register_write: Option<(u8, u64)>,
        clear_llbit: bool,
    },
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub(super) struct Mips4PendingRead {
    operation: Mips4LoadOperation,
    target: u8,
    virtual_address: u64,
    physical_address: u64,
    register_value: u64,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub(super) struct Mips4PendingWrite {
    conditional_target: Option<u8>,
}

pub(super) struct Mips4ResolvedCacheAddress {
    pub(super) virtual_address: u64,
    pub(super) physical_address: u64,
    pub(super) cache_attribute: Mips4MmuCacheAttribute,
}

pub(super) fn prepare_cache_address(
    state: &Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    raw: Mips4Instruction,
) -> Result<Mips4ResolvedCacheAddress, Mips4MemoryAccessError> {
    let virtual_address = read(state, raw.rs()).wrapping_add(raw.signed_immediate() as i64 as u64);
    let tlb_entries = state.deterministic_tlb_entries(policy, virtual_address);
    let access = Mips4MemoryAccess::prepare(
        virtual_address,
        Mips4MemoryAccessKind::Load {
            size: Mips4MemoryAccessSize::Byte,
            signed: false,
        },
        state.config.endianness,
        policy.mmu_config(state.cp0.config()),
        state.cp0.status(),
        Mips4TlbAsid::new(state.cp0.entry_hi().address_space_identifier()),
        tlb_entries,
    )?;
    Ok(Mips4ResolvedCacheAddress {
        virtual_address,
        physical_address: access.physical_address(),
        cache_attribute: access.cache_attribute(),
    })
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
enum Mips4LoadOperation {
    Byte { signed: bool },
    Halfword { signed: bool },
    Word { signed: bool },
    Doubleword,
    WordLeft,
    WordRight,
    DoublewordLeft,
    DoublewordRight,
    LinkedWord,
    LinkedDoubleword,
}

pub(super) fn prepare_memory(
    state: &Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    raw: Mips4Instruction,
    instruction: Mips4CpuInstruction,
    endianness: Mips4Endianness,
) -> Result<Mips4MemoryPlan, Mips4MemoryAccessError> {
    let base = read(state, raw.rs());
    let register_value = read(state, raw.rt());
    let virtual_address = Mips4Memory::effective_address(base, raw.signed_immediate());
    let kind = access_kind(instruction);
    let tlb_entries = state.deterministic_tlb_entries(policy, virtual_address);
    let access = Mips4MemoryAccess::prepare(
        virtual_address,
        kind,
        endianness,
        policy.mmu_config(state.cp0.config()),
        state.cp0.status(),
        Mips4TlbAsid::new(state.cp0.entry_hi().address_space_identifier()),
        tlb_entries,
    )?;
    let access_type = policy.resolve_access_type(access.cache_attribute());
    let cache_policy = policy.resolve_cache_policy(access.cache_attribute());

    if let Some(operation) = load_operation(instruction) {
        let (physical_address, size) = load_transfer(access.physical_address(), &operation);
        return Ok(Mips4MemoryPlan::Read {
            pending: Mips4PendingRead {
                operation,
                target: raw.rt(),
                virtual_address,
                physical_address: access.physical_address(),
                register_value,
            },
            transaction: Mips4ExecutionTransaction::Read {
                physical_address,
                size,
                kind: Mips4ExecutionAccessKind::DataLoad,
                access_type,
            },
            virtual_address,
            cache_policy,
        });
    }

    let conditional = matches!(
        instruction,
        Mips4CpuInstruction::Sc | Mips4CpuInstruction::Scd
    );
    if conditional && (!matches!(state.llbit, Mips4LlBit::Set) || !access_type.is_ll_sc_eligible())
    {
        return Ok(Mips4MemoryPlan::Retire {
            register_write: Some((raw.rt(), 0)),
            clear_llbit: true,
        });
    }

    let (physical_address, size, data, byte_enable) = store_transfer(
        access.physical_address(),
        virtual_address,
        register_value,
        instruction,
        endianness,
    );
    Ok(Mips4MemoryPlan::Write {
        pending: Mips4PendingWrite {
            conditional_target: conditional.then_some(raw.rt()),
        },
        transaction: Mips4ExecutionTransaction::Write {
            physical_address,
            size,
            data,
            byte_enable,
            access_type,
        },
        virtual_address,
        cache_policy,
    })
}

pub(super) fn complete_read(
    state: &mut Mips4ExecutionState,
    pending: Mips4PendingRead,
    physical_lanes: u64,
    endianness: Mips4Endianness,
) {
    let value = match pending.operation {
        Mips4LoadOperation::Byte { signed } => {
            let value = physical_lanes as u8;
            if signed {
                Mips4Memory::sign_extend_byte(value)
            } else {
                Mips4Memory::zero_extend_byte(value)
            }
        }
        Mips4LoadOperation::Halfword { signed } => {
            let value = decode_u16(physical_lanes, endianness);
            if signed {
                Mips4Memory::sign_extend_halfword(value)
            } else {
                Mips4Memory::zero_extend_halfword(value)
            }
        }
        Mips4LoadOperation::Word { signed } => {
            let value = decode_u32(physical_lanes, endianness);
            if signed {
                Mips4Memory::sign_extend_loaded_word(value)
            } else {
                Mips4Memory::zero_extend_word(value)
            }
        }
        Mips4LoadOperation::Doubleword => decode_u64(physical_lanes, endianness),
        Mips4LoadOperation::WordLeft => Mips4Memory::lwl_merge(
            endianness,
            pending.virtual_address,
            pending.register_value,
            decode_u32(physical_lanes, endianness),
        ),
        Mips4LoadOperation::WordRight => Mips4Memory::lwr_merge(
            endianness,
            pending.virtual_address,
            pending.register_value,
            decode_u32(physical_lanes, endianness),
        ),
        Mips4LoadOperation::DoublewordLeft => Mips4Memory::ldl_merge(
            endianness,
            pending.virtual_address,
            pending.register_value,
            decode_u64(physical_lanes, endianness),
        ),
        Mips4LoadOperation::DoublewordRight => Mips4Memory::ldr_merge(
            endianness,
            pending.virtual_address,
            pending.register_value,
            decode_u64(physical_lanes, endianness),
        ),
        Mips4LoadOperation::LinkedWord => {
            state.llbit = Mips4LlBit::Set;
            let _ = state.cp0.write(
                crate::cpu::mips4::cp0::Mips4Cp0Register::LlAddr,
                crate::cpu::mips4::cp0::Mips4Cp0LlAddr::from_physical_address(
                    pending.physical_address,
                )
                .bits() as u64,
            );
            sign_extend_word(decode_u32(physical_lanes, endianness))
        }
        Mips4LoadOperation::LinkedDoubleword => {
            state.llbit = Mips4LlBit::Set;
            let _ = state.cp0.write(
                crate::cpu::mips4::cp0::Mips4Cp0Register::LlAddr,
                crate::cpu::mips4::cp0::Mips4Cp0LlAddr::from_physical_address(
                    pending.physical_address,
                )
                .bits() as u64,
            );
            decode_u64(physical_lanes, endianness)
        }
    };
    write(state, pending.target, value);
}

pub(super) fn complete_write(state: &mut Mips4ExecutionState, pending: Mips4PendingWrite) {
    if let Some(target) = pending.conditional_target {
        write(state, target, 1);
        state.llbit = Mips4LlBit::Clear;
    }
}

fn access_kind(instruction: Mips4CpuInstruction) -> Mips4MemoryAccessKind {
    match instruction {
        Mips4CpuInstruction::Lb => load(Mips4MemoryAccessSize::Byte, true),
        Mips4CpuInstruction::Lbu => load(Mips4MemoryAccessSize::Byte, false),
        Mips4CpuInstruction::Lh => load(Mips4MemoryAccessSize::Halfword, true),
        Mips4CpuInstruction::Lhu => load(Mips4MemoryAccessSize::Halfword, false),
        Mips4CpuInstruction::Lw | Mips4CpuInstruction::Ll => {
            load(Mips4MemoryAccessSize::Word, true)
        }
        Mips4CpuInstruction::Lwu => load(Mips4MemoryAccessSize::Word, false),
        Mips4CpuInstruction::Ld | Mips4CpuInstruction::Lld => {
            load(Mips4MemoryAccessSize::Doubleword, false)
        }
        Mips4CpuInstruction::Lwl => Mips4MemoryAccessKind::LoadWordLeft,
        Mips4CpuInstruction::Lwr => Mips4MemoryAccessKind::LoadWordRight,
        Mips4CpuInstruction::Ldl => Mips4MemoryAccessKind::LoadDoublewordLeft,
        Mips4CpuInstruction::Ldr => Mips4MemoryAccessKind::LoadDoublewordRight,
        Mips4CpuInstruction::Sb => store(Mips4MemoryAccessSize::Byte),
        Mips4CpuInstruction::Sh => store(Mips4MemoryAccessSize::Halfword),
        Mips4CpuInstruction::Sw | Mips4CpuInstruction::Sc => store(Mips4MemoryAccessSize::Word),
        Mips4CpuInstruction::Sd | Mips4CpuInstruction::Scd => {
            store(Mips4MemoryAccessSize::Doubleword)
        }
        Mips4CpuInstruction::Swl => Mips4MemoryAccessKind::StoreWordLeft,
        Mips4CpuInstruction::Swr => Mips4MemoryAccessKind::StoreWordRight,
        Mips4CpuInstruction::Sdl => Mips4MemoryAccessKind::StoreDoublewordLeft,
        Mips4CpuInstruction::Sdr => Mips4MemoryAccessKind::StoreDoublewordRight,
        _ => unreachable!(),
    }
}

fn load(size: Mips4MemoryAccessSize, signed: bool) -> Mips4MemoryAccessKind {
    Mips4MemoryAccessKind::Load { size, signed }
}

fn store(size: Mips4MemoryAccessSize) -> Mips4MemoryAccessKind {
    Mips4MemoryAccessKind::Store { size }
}

fn load_operation(instruction: Mips4CpuInstruction) -> Option<Mips4LoadOperation> {
    match instruction {
        Mips4CpuInstruction::Lb => Some(Mips4LoadOperation::Byte { signed: true }),
        Mips4CpuInstruction::Lbu => Some(Mips4LoadOperation::Byte { signed: false }),
        Mips4CpuInstruction::Lh => Some(Mips4LoadOperation::Halfword { signed: true }),
        Mips4CpuInstruction::Lhu => Some(Mips4LoadOperation::Halfword { signed: false }),
        Mips4CpuInstruction::Lw => Some(Mips4LoadOperation::Word { signed: true }),
        Mips4CpuInstruction::Lwu => Some(Mips4LoadOperation::Word { signed: false }),
        Mips4CpuInstruction::Ld => Some(Mips4LoadOperation::Doubleword),
        Mips4CpuInstruction::Lwl => Some(Mips4LoadOperation::WordLeft),
        Mips4CpuInstruction::Lwr => Some(Mips4LoadOperation::WordRight),
        Mips4CpuInstruction::Ldl => Some(Mips4LoadOperation::DoublewordLeft),
        Mips4CpuInstruction::Ldr => Some(Mips4LoadOperation::DoublewordRight),
        Mips4CpuInstruction::Ll => Some(Mips4LoadOperation::LinkedWord),
        Mips4CpuInstruction::Lld => Some(Mips4LoadOperation::LinkedDoubleword),
        _ => None,
    }
}

fn load_transfer(
    physical_address: u64,
    operation: &Mips4LoadOperation,
) -> (u64, Mips4ExecutionTransferSize) {
    match operation {
        Mips4LoadOperation::Byte { .. } => (physical_address, Mips4ExecutionTransferSize::Byte),
        Mips4LoadOperation::Halfword { .. } => {
            (physical_address, Mips4ExecutionTransferSize::Halfword)
        }
        Mips4LoadOperation::Word { .. } | Mips4LoadOperation::LinkedWord => {
            (physical_address, Mips4ExecutionTransferSize::Word)
        }
        Mips4LoadOperation::Doubleword | Mips4LoadOperation::LinkedDoubleword => {
            (physical_address, Mips4ExecutionTransferSize::Doubleword)
        }
        Mips4LoadOperation::WordLeft | Mips4LoadOperation::WordRight => {
            (physical_address & !3, Mips4ExecutionTransferSize::Word)
        }
        Mips4LoadOperation::DoublewordLeft | Mips4LoadOperation::DoublewordRight => (
            physical_address & !7,
            Mips4ExecutionTransferSize::Doubleword,
        ),
    }
}

fn store_transfer(
    physical_address: u64,
    virtual_address: u64,
    register_value: u64,
    instruction: Mips4CpuInstruction,
    endianness: Mips4Endianness,
) -> (u64, Mips4ExecutionTransferSize, u64, u8) {
    match instruction {
        Mips4CpuInstruction::Sb => (
            physical_address,
            Mips4ExecutionTransferSize::Byte,
            register_value & 0xff,
            0x01,
        ),
        Mips4CpuInstruction::Sh => (
            physical_address,
            Mips4ExecutionTransferSize::Halfword,
            encode_lanes(
                register_value,
                Mips4ExecutionTransferSize::Halfword,
                endianness,
            ),
            0x03,
        ),
        Mips4CpuInstruction::Sw | Mips4CpuInstruction::Sc => (
            physical_address,
            Mips4ExecutionTransferSize::Word,
            encode_lanes(register_value, Mips4ExecutionTransferSize::Word, endianness),
            0x0f,
        ),
        Mips4CpuInstruction::Sd | Mips4CpuInstruction::Scd => (
            physical_address,
            Mips4ExecutionTransferSize::Doubleword,
            encode_lanes(
                register_value,
                Mips4ExecutionTransferSize::Doubleword,
                endianness,
            ),
            0xff,
        ),
        Mips4CpuInstruction::Swl | Mips4CpuInstruction::Swr => {
            let masked = if matches!(instruction, Mips4CpuInstruction::Swl) {
                Mips4Memory::swl_masked_word(endianness, virtual_address, register_value, 0)
            } else {
                Mips4Memory::swr_masked_word(endianness, virtual_address, register_value, 0)
            };
            let data = encode_lanes(
                masked.value as u64,
                Mips4ExecutionTransferSize::Word,
                endianness,
            );
            let lane_mask = encode_lanes(
                masked.write_mask as u64,
                Mips4ExecutionTransferSize::Word,
                endianness,
            );
            (
                physical_address & !3,
                Mips4ExecutionTransferSize::Word,
                data,
                byte_enable(lane_mask, 4),
            )
        }
        Mips4CpuInstruction::Sdl | Mips4CpuInstruction::Sdr => {
            let masked = if matches!(instruction, Mips4CpuInstruction::Sdl) {
                Mips4Memory::sdl_masked_doubleword(endianness, virtual_address, register_value, 0)
            } else {
                Mips4Memory::sdr_masked_doubleword(endianness, virtual_address, register_value, 0)
            };
            let data = encode_lanes(
                masked.value,
                Mips4ExecutionTransferSize::Doubleword,
                endianness,
            );
            let lane_mask = encode_lanes(
                masked.write_mask,
                Mips4ExecutionTransferSize::Doubleword,
                endianness,
            );
            (
                physical_address & !7,
                Mips4ExecutionTransferSize::Doubleword,
                data,
                byte_enable(lane_mask, 8),
            )
        }
        _ => unreachable!(),
    }
}

pub(super) fn encode_lanes(
    value: u64,
    size: Mips4ExecutionTransferSize,
    endianness: Mips4Endianness,
) -> u64 {
    let count = size.bytes() as usize;
    let source = match endianness {
        Mips4Endianness::Little => value.to_le_bytes(),
        Mips4Endianness::Big => value.to_be_bytes(),
    };
    let offset = if matches!(endianness, Mips4Endianness::Big) {
        8 - count
    } else {
        0
    };
    let mut lanes = [0; 8];
    lanes[..count].copy_from_slice(&source[offset..offset + count]);
    u64::from_le_bytes(lanes)
}

fn byte_enable(mask: u64, count: usize) -> u8 {
    let bytes = mask.to_le_bytes();
    let mut enabled = 0;
    for (index, byte) in bytes[..count].iter().enumerate() {
        if *byte != 0 {
            enabled |= 1 << index;
        }
    }
    enabled
}

fn decode_u16(lanes: u64, endianness: Mips4Endianness) -> u16 {
    let bytes = lanes.to_le_bytes();
    match endianness {
        Mips4Endianness::Big => u16::from_be_bytes([bytes[0], bytes[1]]),
        Mips4Endianness::Little => u16::from_le_bytes([bytes[0], bytes[1]]),
    }
}

pub(super) fn decode_u32(lanes: u64, endianness: Mips4Endianness) -> u32 {
    let bytes = lanes.to_le_bytes();
    match endianness {
        Mips4Endianness::Big => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        Mips4Endianness::Little => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    }
}

pub(super) fn decode_u64(lanes: u64, endianness: Mips4Endianness) -> u64 {
    match endianness {
        Mips4Endianness::Big => u64::from_be_bytes(lanes.to_le_bytes()),
        Mips4Endianness::Little => lanes,
    }
}

fn read(state: &Mips4ExecutionState, register: u8) -> u64 {
    state.gpr.read(Mips4GprIndex::from_u8(register).unwrap())
}

fn write(state: &mut Mips4ExecutionState, register: u8, value: u64) {
    state
        .gpr
        .write(Mips4GprIndex::from_u8(register).unwrap(), value);
}
