//! Read-only architectural state for debuggers.

use se_core::bus::PhysAddr;
use se_float::backend::Backend;

use super::R3000;
use super::cache::CacheBank;
use super::decode::{
    AluInstruction, ControlInstruction, Cp0Instruction, Cp1BinaryOperation, Cp1Conversion,
    Cp1FloatFormat, Cp1Instruction, Cp1UnaryOperation, DecodeResult, Instruction,
    MemoryInstruction, decode,
};
use super::mmu::AccessType;
use super::state::PendingCp1Write;

const GPR_NAMES: [&str; 32] = [
    "$zero", "$at", "$v0", "$v1", "$a0", "$a1", "$a2", "$a3", "$t0", "$t1", "$t2", "$t3", "$t4",
    "$t5", "$t6", "$t7", "$s0", "$s1", "$s2", "$s3", "$s4", "$s5", "$s6", "$s7", "$t8", "$t9",
    "$k0", "$k1", "$gp", "$sp", "$fp", "$ra",
];

/// A pending branch or jump delay slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DelaySlotDebugSnapshot {
    /// Address of the branch or jump instruction.
    pub origin_pc: u32,
    /// Address selected after the delay-slot instruction completes.
    pub resume_pc: u32,
}

/// A pending general-register transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingGprDebugSnapshot {
    /// Destination register index.
    pub index: usize,
    /// Value awaiting visibility.
    pub value: u32,
    /// Whether a merge load can bypass this transfer.
    pub load_merge_bypass: bool,
}

/// A pending general CP0-register transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingCp0DebugSnapshot {
    /// Destination register index.
    pub index: usize,
    /// Value awaiting visibility.
    pub value: u32,
}

/// A pending CP1-visible transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingCp1DebugSnapshot {
    /// A floating-point general-register write.
    General {
        /// Destination register index.
        index: usize,
        /// Value awaiting visibility.
        value: u32,
    },
    /// A floating-point control-register write.
    Control {
        /// Destination register index.
        index: usize,
        /// Value awaiting visibility.
        value: u32,
    },
    /// A floating-point condition write.
    Condition {
        /// Condition value awaiting visibility.
        value: bool,
    },
}

/// CP0 state used for execution before delayed control changes become visible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cp0FunctionalDebugSnapshot {
    /// Effective coprocessor usability bits.
    pub coprocessor_usable: u32,
    /// Effective interrupt mask, mode, and enable bits.
    pub interrupt_control: u32,
    /// Effective software interrupt bits.
    pub software_interrupts: u32,
}

/// Committed and execution-visible CP0 state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cp0DebugSnapshot {
    /// Values returned by reads of all 32 CP0 register numbers.
    pub registers: [u32; 32],
    /// State currently used by instruction execution.
    pub effective: Cp0FunctionalDebugSnapshot,
    /// Functional state awaiting visibility, when present.
    pub pending_functional: Option<Cp0FunctionalDebugSnapshot>,
}

/// Committed R3010 state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cp1DebugSnapshot {
    /// Raw floating-point general registers.
    pub registers: [u32; 32],
    /// FCR0 implementation and revision value.
    pub fcr0: u32,
    /// FCR30 exception instruction register.
    pub fcr30: u32,
    /// FCR31 control/status register.
    pub fcr31: u32,
    /// Selected arithmetic backend.
    pub backend: Backend,
    /// Current external interrupt output.
    pub interrupt_asserted: bool,
}

/// Small R3000/R3010 state sampled at one instruction boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R3000DebugSnapshot {
    /// Current program counter.
    pub pc: u32,
    /// HI register.
    pub hi: u32,
    /// LO register.
    pub lo: u32,
    /// Committed general-purpose registers.
    pub gpr: [u32; 32],
    /// Pending delay slot, when present.
    pub delay_slot: Option<DelaySlotDebugSnapshot>,
    /// Pending general-register transfer, when present.
    pub pending_gpr: Option<PendingGprDebugSnapshot>,
    /// Pending CP0 transfer, when present.
    pub pending_cp0: Option<PendingCp0DebugSnapshot>,
    /// Pending CP1 transfer, when present.
    pub pending_cp1: Option<PendingCp1DebugSnapshot>,
    /// CP0 state.
    pub cp0: Cp0DebugSnapshot,
    /// CP1 state.
    pub cp1: Cp1DebugSnapshot,
}

/// Selects one software-visible TLB view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlbView {
    /// Main view used by data translation and TLB instructions.
    Main,
    /// Delayed view used by instruction translation.
    Instruction,
}

/// One decoded R3000 TLB entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TlbEntryDebugSnapshot {
    /// Entry index.
    pub index: usize,
    /// Raw EntryHi value.
    pub entry_hi: u32,
    /// Raw EntryLo value.
    pub entry_lo: u32,
    /// Virtual page number.
    pub vpn: u32,
    /// Address-space identifier.
    pub asid: u8,
    /// Physical page frame number.
    pub pfn: u32,
    /// Whether the entry selects uncached accesses.
    pub noncacheable: bool,
    /// Dirty/write permission bit.
    pub dirty: bool,
    /// Valid bit.
    pub valid: bool,
    /// Global bit.
    pub global: bool,
}

/// One complete R3000 TLB view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlbDebugSnapshot {
    /// Selected view.
    pub view: TlbView,
    /// Whether CP0 is in TLB shutdown state.
    pub shutdown: bool,
    /// Current indexed-operation entry.
    pub index: usize,
    /// Current random-write entry.
    pub random: usize,
    /// All 64 entries in index order.
    pub entries: Vec<TlbEntryDebugSnapshot>,
}

/// Selects one physical R3000 cache bank.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheView {
    /// Instruction cache bank.
    Instruction,
    /// Data cache bank.
    Data,
}

/// One R3000 cache word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheEntryDebugSnapshot {
    /// Direct-mapped word index.
    pub index: usize,
    /// Stored physical page frame tag.
    pub page_frame: u32,
    /// Raw word in guest address order.
    pub word: u32,
    /// Entry validity.
    pub valid: bool,
}

/// One physical R3000 cache bank.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheDebugSnapshot {
    /// Selected bank.
    pub view: CacheView,
    /// Configured refill size in bytes.
    pub refill_bytes: usize,
    /// Cache words in direct-mapped index order.
    pub entries: Vec<CacheEntryDebugSnapshot>,
}

/// Selects a non-mutating virtual-address translation view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VirtualAddressView {
    /// Instruction translation using the delayed instruction TLB view.
    Instruction,
    /// Data-load translation using the main TLB view.
    Data,
}

impl R3000 {
    /// Returns the current program counter.
    #[must_use]
    pub fn program_counter(&self) -> u32 {
        self.state.pc()
    }

    /// Samples small architectural state without changing the processor.
    #[must_use]
    pub fn debug_snapshot(&self) -> R3000DebugSnapshot {
        let delay_slot = self
            .state
            .debug_delay_slot()
            .map(|slot| DelaySlotDebugSnapshot {
                origin_pc: slot.origin_pc,
                resume_pc: slot.resume_pc,
            });
        let pending_gpr =
            self.state
                .debug_pending_gpr_write()
                .map(|write| PendingGprDebugSnapshot {
                    index: write.index,
                    value: write.value,
                    load_merge_bypass: write.load_merge_bypass,
                });
        let pending_cp0 =
            self.state
                .debug_pending_cp0_write()
                .map(|write| PendingCp0DebugSnapshot {
                    index: write.index,
                    value: write.value,
                });
        let pending_cp1 = self
            .state
            .debug_pending_cp1_write()
            .map(|write| match write {
                PendingCp1Write::General { index, value } => {
                    PendingCp1DebugSnapshot::General { index, value }
                }
                PendingCp1Write::Control { index, value } => {
                    PendingCp1DebugSnapshot::Control { index, value }
                }
                PendingCp1Write::Condition { value } => {
                    PendingCp1DebugSnapshot::Condition { value }
                }
            });
        let (effective, pending_functional) = self.state.debug_cp0_functional_state();
        let cp1 = self.state.cp1();

        R3000DebugSnapshot {
            pc: self.state.pc(),
            hi: self.state.read_hi(),
            lo: self.state.read_lo(),
            gpr: std::array::from_fn(|index| self.state.read_gpr(index)),
            delay_slot,
            pending_gpr,
            pending_cp0,
            pending_cp1,
            cp0: Cp0DebugSnapshot {
                registers: std::array::from_fn(|index| self.state.read_cp0(index)),
                effective: functional_snapshot(effective),
                pending_functional: pending_functional.map(functional_snapshot),
            },
            cp1: Cp1DebugSnapshot {
                registers: std::array::from_fn(|index| self.state.read_cp1_general(index)),
                fcr0: self.state.read_cp1_control(0),
                fcr30: self.state.read_cp1_control(30),
                fcr31: self.state.read_cp1_control(31),
                backend: cp1.backend(),
                interrupt_asserted: cp1.interrupt_asserted(),
            },
        }
    }

    /// Samples one software-visible TLB view without changing the processor.
    #[must_use]
    pub fn tlb_debug_snapshot(&self, view: TlbView) -> TlbDebugSnapshot {
        let instruction = view == TlbView::Instruction;
        let entries = self
            .state
            .debug_tlb_entries(instruction)
            .into_iter()
            .enumerate()
            .map(|(index, (entry_hi, entry_lo))| TlbEntryDebugSnapshot {
                index,
                entry_hi,
                entry_lo,
                vpn: entry_hi >> 12,
                asid: ((entry_hi >> 6) & 0x3f) as u8,
                pfn: entry_lo >> 12,
                noncacheable: entry_lo & (1 << 11) != 0,
                dirty: entry_lo & (1 << 10) != 0,
                valid: entry_lo & (1 << 9) != 0,
                global: entry_lo & (1 << 8) != 0,
            })
            .collect();

        TlbDebugSnapshot {
            view,
            shutdown: self.state.is_tlb_shutdown(),
            index: ((self.state.read_cp0(0) >> 8) & 0x3f) as usize,
            random: ((self.state.read_cp0(1) >> 8) & 0x3f) as usize,
            entries,
        }
    }

    /// Samples one physical cache bank without changing the processor.
    #[must_use]
    pub fn cache_debug_snapshot(&self, view: CacheView) -> CacheDebugSnapshot {
        let bank = match view {
            CacheView::Instruction => CacheBank::Instruction,
            CacheView::Data => CacheBank::Data,
        };
        let (refill_bytes, entries) = self.state.debug_cache_entries(bank);
        let entries = entries
            .into_iter()
            .enumerate()
            .map(
                |(index, (page_frame, data, valid))| CacheEntryDebugSnapshot {
                    index,
                    page_frame,
                    word: u32::from_be_bytes(data),
                    valid,
                },
            )
            .collect();

        CacheDebugSnapshot {
            view,
            refill_bytes,
            entries,
        }
    }

    /// Translates one virtual address for a debugger without changing CP0.
    #[must_use]
    pub fn debug_translate_address(
        &self,
        virtual_address: u32,
        view: VirtualAddressView,
    ) -> Option<PhysAddr> {
        let access = match view {
            VirtualAddressView::Instruction => AccessType::Instruction,
            VirtualAddressView::Data => AccessType::Load,
        };
        self.state
            .debug_translate_address(virtual_address, access)
            .ok()
            .map(|translation| translation.address)
    }
}

fn functional_snapshot(
    (coprocessor_usable, interrupt_control, software_interrupts): (u32, u32, u32),
) -> Cp0FunctionalDebugSnapshot {
    Cp0FunctionalDebugSnapshot {
        coprocessor_usable,
        interrupt_control,
        software_interrupts,
    }
}

/// Decodes one MIPS I instruction for presentation by a debugger.
#[must_use]
pub fn disassemble(word: u32, pc: u32) -> String {
    match decode(word) {
        DecodeResult::Implemented(instruction) => format_instruction(instruction, pc),
        DecodeResult::UnsupportedCoprocessor { unit } => {
            format!("cop{unit} 0x{word:08x}")
        }
        DecodeResult::Reserved => format!(".word 0x{word:08x}"),
    }
}

fn format_instruction(instruction: Instruction, pc: u32) -> String {
    match instruction {
        Instruction::Alu(instruction) => format_alu(instruction),
        Instruction::Control(instruction) => format_control(instruction, pc),
        Instruction::Cp0(instruction) => format_cp0(instruction, pc),
        Instruction::Cp1(instruction) => format_cp1(instruction, pc),
        Instruction::Memory(instruction) => format_memory(instruction),
        Instruction::Syscall => String::from("syscall"),
        Instruction::Breakpoint => String::from("break"),
    }
}

fn format_alu(instruction: AluInstruction) -> String {
    use AluInstruction as I;
    match instruction {
        I::Sll {
            rd: 0,
            rt: 0,
            shift_amount: 0,
        } => String::from("nop"),
        I::Sll {
            rd,
            rt,
            shift_amount,
        } => format!("sll {}, {}, {shift_amount}", reg(rd), reg(rt)),
        I::Srl {
            rd,
            rt,
            shift_amount,
        } => format!("srl {}, {}, {shift_amount}", reg(rd), reg(rt)),
        I::Sra {
            rd,
            rt,
            shift_amount,
        } => format!("sra {}, {}, {shift_amount}", reg(rd), reg(rt)),
        I::Sllv { rd, rt, rs } => format!("sllv {}, {}, {}", reg(rd), reg(rt), reg(rs)),
        I::Srlv { rd, rt, rs } => format!("srlv {}, {}, {}", reg(rd), reg(rt), reg(rs)),
        I::Srav { rd, rt, rs } => format!("srav {}, {}, {}", reg(rd), reg(rt), reg(rs)),
        I::Mfhi { rd } => format!("mfhi {}", reg(rd)),
        I::Mthi { rs } => format!("mthi {}", reg(rs)),
        I::Mflo { rd } => format!("mflo {}", reg(rd)),
        I::Mtlo { rs } => format!("mtlo {}", reg(rs)),
        I::Mult { rs, rt } => format!("mult {}, {}", reg(rs), reg(rt)),
        I::Multu { rs, rt } => format!("multu {}, {}", reg(rs), reg(rt)),
        I::Div { rs, rt } => format!("div {}, {}", reg(rs), reg(rt)),
        I::Divu { rs, rt } => format!("divu {}, {}", reg(rs), reg(rt)),
        I::Add { rd, rs, rt } => format_three_registers("add", rd, rs, rt),
        I::Addu { rd, rs, rt } => format_three_registers("addu", rd, rs, rt),
        I::Sub { rd, rs, rt } => format_three_registers("sub", rd, rs, rt),
        I::Subu { rd, rs, rt } => format_three_registers("subu", rd, rs, rt),
        I::And { rd, rs, rt } => format_three_registers("and", rd, rs, rt),
        I::Or { rd, rs, rt } => format_three_registers("or", rd, rs, rt),
        I::Xor { rd, rs, rt } => format_three_registers("xor", rd, rs, rt),
        I::Nor { rd, rs, rt } => format_three_registers("nor", rd, rs, rt),
        I::Slt { rd, rs, rt } => format_three_registers("slt", rd, rs, rt),
        I::Sltu { rd, rs, rt } => format_three_registers("sltu", rd, rs, rt),
        I::Addiu { rt, rs, immediate } => format_signed_immediate("addiu", rt, rs, immediate),
        I::Addi { rt, rs, immediate } => format_signed_immediate("addi", rt, rs, immediate),
        I::Slti { rt, rs, immediate } => format_signed_immediate("slti", rt, rs, immediate),
        I::Sltiu { rt, rs, immediate } => format_signed_immediate("sltiu", rt, rs, immediate),
        I::Andi { rt, rs, immediate } => {
            format!("andi {}, {}, 0x{immediate:04x}", reg(rt), reg(rs))
        }
        I::Ori { rt, rs, immediate } => format!("ori {}, {}, 0x{immediate:04x}", reg(rt), reg(rs)),
        I::Xori { rt, rs, immediate } => {
            format!("xori {}, {}, 0x{immediate:04x}", reg(rt), reg(rs))
        }
        I::Lui { rt, immediate } => format!("lui {}, 0x{immediate:04x}", reg(rt)),
    }
}

fn format_control(instruction: ControlInstruction, pc: u32) -> String {
    use ControlInstruction as I;
    match instruction {
        I::J { target } => format!("j 0x{:08x}", jump_target(pc, target)),
        I::Jal { target } => format!("jal 0x{:08x}", jump_target(pc, target)),
        I::Jr { rs } => format!("jr {}", reg(rs)),
        I::Jalr { rd, rs } => format!("jalr {}, {}", reg(rd), reg(rs)),
        I::Beq { rs, rt, offset } => format_branch_two("beq", rs, rt, pc, offset),
        I::Bne { rs, rt, offset } => format_branch_two("bne", rs, rt, pc, offset),
        I::Blez { rs, offset } => format_branch_one("blez", rs, pc, offset),
        I::Bgtz { rs, offset } => format_branch_one("bgtz", rs, pc, offset),
        I::Bltz { rs, offset } => format_branch_one("bltz", rs, pc, offset),
        I::Bgez { rs, offset } => format_branch_one("bgez", rs, pc, offset),
        I::Bltzal { rs, offset } => format_branch_one("bltzal", rs, pc, offset),
        I::Bgezal { rs, offset } => format_branch_one("bgezal", rs, pc, offset),
    }
}

fn format_cp0(instruction: Cp0Instruction, pc: u32) -> String {
    use Cp0Instruction as I;
    match instruction {
        I::Mfc0 { rt, rd } => format!("mfc0 {}, ${rd}", reg(rt)),
        I::Cfc0 { rt, rd } => format!("cfc0 {}, ${rd}", reg(rt)),
        I::Mtc0 { rt, rd } => format!("mtc0 {}, ${rd}", reg(rt)),
        I::Ctc0 { rt, rd } => format!("ctc0 {}, ${rd}", reg(rt)),
        I::Bc0f { offset } => format!("bc0f 0x{:08x}", branch_target(pc, offset)),
        I::Bc0t { offset } => format!("bc0t 0x{:08x}", branch_target(pc, offset)),
        I::Tlbr => String::from("tlbr"),
        I::Tlbwi => String::from("tlbwi"),
        I::Tlbwr => String::from("tlbwr"),
        I::Tlbp => String::from("tlbp"),
        I::Rfe => String::from("rfe"),
    }
}

fn format_cp1(instruction: Cp1Instruction, pc: u32) -> String {
    use Cp1Instruction as I;
    match instruction {
        I::Mfc1 { rt, rd } => format!("mfc1 {}, $f{rd}", reg(rt)),
        I::Cfc1 { rt, rd } => format!("cfc1 {}, ${rd}", reg(rt)),
        I::Mtc1 { rt, rd } => format!("mtc1 {}, $f{rd}", reg(rt)),
        I::Ctc1 { rt, rd } => format!("ctc1 {}, ${rd}", reg(rt)),
        I::Bc1f { offset } => format!("bc1f 0x{:08x}", branch_target(pc, offset)),
        I::Bc1t { offset } => format!("bc1t 0x{:08x}", branch_target(pc, offset)),
        I::Lwc1 { base, ft, offset } => format!("lwc1 $f{ft}, {}({})", signed(offset), reg(base)),
        I::Swc1 { base, ft, offset } => format!("swc1 $f{ft}, {}({})", signed(offset), reg(base)),
        I::Binary {
            operation,
            format,
            ft,
            fs,
            fd,
        } => format!(
            "{}.{} $f{fd}, $f{fs}, $f{ft}",
            binary_name(operation),
            format_name(format)
        ),
        I::Unary {
            operation,
            format,
            fs,
            fd,
        } => format!(
            "{}.{} $f{fd}, $f{fs}",
            unary_name(operation),
            format_name(format)
        ),
        I::Convert { operation, fs, fd } => {
            let (destination, source) = conversion_formats(operation);
            format!("cvt.{destination}.{source} $f{fd}, $f{fs}")
        }
        I::Compare {
            format,
            condition,
            fs,
            ft,
        } => format!(
            "c.{}.{} $f{fs}, $f{ft}",
            comparison_name(condition),
            format_name(format)
        ),
        I::UnimplementedOperation => String::from("cop1.unimplemented"),
    }
}

fn format_memory(instruction: MemoryInstruction) -> String {
    use MemoryInstruction as I;
    match instruction {
        I::Lb { base, rt, offset } => format_load_store("lb", base, rt, offset),
        I::Lbu { base, rt, offset } => format_load_store("lbu", base, rt, offset),
        I::Lh { base, rt, offset } => format_load_store("lh", base, rt, offset),
        I::Lhu { base, rt, offset } => format_load_store("lhu", base, rt, offset),
        I::Lwl { base, rt, offset } => format_load_store("lwl", base, rt, offset),
        I::Lw { base, rt, offset } => format_load_store("lw", base, rt, offset),
        I::Lwr { base, rt, offset } => format_load_store("lwr", base, rt, offset),
        I::Sb { base, rt, offset } => format_load_store("sb", base, rt, offset),
        I::Sh { base, rt, offset } => format_load_store("sh", base, rt, offset),
        I::Swl { base, rt, offset } => format_load_store("swl", base, rt, offset),
        I::Sw { base, rt, offset } => format_load_store("sw", base, rt, offset),
        I::Swr { base, rt, offset } => format_load_store("swr", base, rt, offset),
    }
}

fn reg(index: usize) -> &'static str {
    GPR_NAMES[index]
}

fn signed(immediate: u16) -> i16 {
    immediate as i16
}

fn format_three_registers(name: &str, rd: usize, rs: usize, rt: usize) -> String {
    format!("{name} {}, {}, {}", reg(rd), reg(rs), reg(rt))
}

fn format_signed_immediate(name: &str, rt: usize, rs: usize, immediate: u16) -> String {
    format!("{name} {}, {}, {}", reg(rt), reg(rs), signed(immediate))
}

fn format_branch_two(name: &str, rs: usize, rt: usize, pc: u32, offset: u16) -> String {
    format!(
        "{name} {}, {}, 0x{:08x}",
        reg(rs),
        reg(rt),
        branch_target(pc, offset)
    )
}

fn format_branch_one(name: &str, rs: usize, pc: u32, offset: u16) -> String {
    format!("{name} {}, 0x{:08x}", reg(rs), branch_target(pc, offset))
}

fn format_load_store(name: &str, base: usize, rt: usize, offset: u16) -> String {
    format!("{name} {}, {}({})", reg(rt), signed(offset), reg(base))
}

fn branch_target(pc: u32, offset: u16) -> u32 {
    pc.wrapping_add(4)
        .wrapping_add((i32::from(offset as i16) as u32).wrapping_shl(2))
}

fn jump_target(pc: u32, target: u32) -> u32 {
    (pc.wrapping_add(4) & 0xf000_0000) | target << 2
}

const fn format_name(format: Cp1FloatFormat) -> &'static str {
    match format {
        Cp1FloatFormat::Single => "s",
        Cp1FloatFormat::Double => "d",
    }
}

const fn binary_name(operation: Cp1BinaryOperation) -> &'static str {
    match operation {
        Cp1BinaryOperation::Add => "add",
        Cp1BinaryOperation::Subtract => "sub",
        Cp1BinaryOperation::Multiply => "mul",
        Cp1BinaryOperation::Divide => "div",
    }
}

const fn unary_name(operation: Cp1UnaryOperation) -> &'static str {
    match operation {
        Cp1UnaryOperation::Absolute => "abs",
        Cp1UnaryOperation::Move => "mov",
        Cp1UnaryOperation::Negate => "neg",
    }
}

const fn conversion_formats(operation: Cp1Conversion) -> (&'static str, &'static str) {
    match operation {
        Cp1Conversion::SingleToDouble => ("d", "s"),
        Cp1Conversion::WordToDouble => ("d", "w"),
        Cp1Conversion::DoubleToSingle => ("s", "d"),
        Cp1Conversion::WordToSingle => ("s", "w"),
        Cp1Conversion::SingleToWord => ("w", "s"),
        Cp1Conversion::DoubleToWord => ("w", "d"),
    }
}

const fn comparison_name(condition: u8) -> &'static str {
    const NAMES: [&str; 16] = [
        "f", "un", "eq", "ueq", "olt", "ult", "ole", "ule", "sf", "ngle", "seq", "ngl", "lt",
        "nge", "le", "ngt",
    ];
    NAMES[(condition & 0x0f) as usize]
}

#[cfg(test)]
mod tests {
    use se_float::backend::Backend;

    use super::{CacheView, R3000, TlbView, disassemble};
    use crate::mips1::r3000::R3000Config;

    const CONFIG: R3000Config =
        R3000Config::new(1, 4 * 1024, 4 * 1024, 4, 4, true, Backend::SoftFloat);

    #[test]
    fn reset_snapshot_contains_architectural_state() {
        let cpu = R3000::new(CONFIG);
        let snapshot = cpu.debug_snapshot();

        assert_eq!(snapshot.pc, 0xbfc0_0000);
        assert_eq!(snapshot.gpr, [0; 32]);
        assert_eq!(snapshot.cp0.registers[1], 63 << 8);
        assert_eq!(snapshot.cp1.fcr0, 0x0000_0300);
    }

    #[test]
    fn tlb_and_cache_snapshots_cover_both_views() {
        let cpu = R3000::new(CONFIG);

        assert_eq!(cpu.tlb_debug_snapshot(TlbView::Main).entries.len(), 64);
        assert_eq!(
            cpu.tlb_debug_snapshot(TlbView::Instruction).entries.len(),
            64
        );
        assert_eq!(
            cpu.cache_debug_snapshot(CacheView::Instruction)
                .entries
                .len(),
            1024
        );
        assert_eq!(
            cpu.cache_debug_snapshot(CacheView::Data).entries.len(),
            1024
        );
    }

    #[test]
    fn disassembler_formats_reset_jump_and_nop() {
        assert_eq!(disassemble(0x0bf0_0080, 0xbfc0_0000), "j 0xbfc00200");
        assert_eq!(disassemble(0, 0xbfc0_0004), "nop");
    }
}
