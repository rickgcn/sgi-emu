use super::{
    ExecutionError,
    control::branch_resume_pc,
    decode::Cp0Instruction,
    state::{InstructionEffect, State},
};

const INDEX_PROBE_FAILURE: u32 = 1 << 31;
const INDEX_INDEX_MASK: u32 = 0x0000_3f00;
const ENTRY_LO_VISIBLE_MASK: u32 = 0xffff_ff00;
const CONTEXT_VISIBLE_MASK: u32 = 0xffff_fffc;
const CONTEXT_PTE_BASE_MASK: u32 = 0xffe0_0000;
const CONTEXT_BAD_VPN_MASK: u32 = 0x001f_fffc;
const ENTRY_HI_VISIBLE_MASK: u32 = 0xffff_ffc0;
const ENTRY_HI_VPN_MASK: u32 = 0xffff_f000;
const ENTRY_HI_ASID_MASK: u32 = 0x0000_0fc0;

const PRID: u32 = 0x0000_0230;
const RANDOM_RESET: u32 = 63 << 8;

const STATUS_BEV: u32 = 1 << 22;
const STATUS_TS: u32 = 1 << 21;
const STATUS_PE: u32 = 1 << 20;
const STATUS_CM: u32 = 1 << 19;
const STATUS_SWC: u32 = 1 << 17;
const STATUS_ISC: u32 = 1 << 16;
const STATUS_KUC: u32 = 1 << 1;
const STATUS_IEC: u32 = 1;
const STATUS_MODE_STACK_MASK: u32 = 0x3f;
const STATUS_CU0: u32 = 1 << 28;
const STATUS_CU_MASK: u32 = 0xf000_0000;
const STATUS_INTERRUPT_CONTROL_MASK: u32 = 0x0000_ff01;
const STATUS_VISIBLE_MASK: u32 = 0xf27f_ff3f;
const STATUS_WRITABLE_MASK: u32 = 0xf247_ff3f;

const CAUSE_BD: u32 = 1 << 31;
const CAUSE_IP_MASK: u32 = 0x0000_ff00;
const CAUSE_SOFTWARE_IP_MASK: u32 = 0x0000_0300;
const CAUSE_VISIBLE_MASK: u32 = 0xb000_ff7c;

const GENERAL_EXCEPTION_VECTOR: u32 = 0x8000_0080;
const BOOT_GENERAL_EXCEPTION_VECTOR: u32 = 0xbfc0_0180;
const TLB_REFILL_EXCEPTION_VECTOR: u32 = 0x8000_0000;
const BOOT_TLB_REFILL_EXCEPTION_VECTOR: u32 = 0xbfc0_0100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TlbFaultKind {
    Miss,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Exception {
    InstructionAddressError { address: u32 },
    LoadAddressError { address: u32 },
    StoreAddressError { address: u32 },
    TlbLoad { address: u32, fault: TlbFaultKind },
    TlbStore { address: u32, fault: TlbFaultKind },
    TlbModified { address: u32 },
    InstructionBusError,
    DataBusError,
    Syscall,
    Breakpoint,
    ReservedInstruction,
    CoprocessorUnusable,
    Overflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FunctionalState {
    coprocessor_usable: u32,
    interrupt_control: u32,
    software_interrupts: u32,
}

impl FunctionalState {
    const fn from_registers(status: u32, cause: u32) -> Self {
        Self {
            coprocessor_usable: status & STATUS_CU_MASK,
            interrupt_control: status & STATUS_INTERRUPT_CONTROL_MASK,
            software_interrupts: cause & CAUSE_SOFTWARE_IP_MASK,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Cp0 {
    index: u32,
    random: u32,
    entry_lo: u32,
    context: u32,
    bad_vaddr: u32,
    entry_hi: u32,
    status: u32,
    cause: u32,
    epc: u32,
    effective: FunctionalState,
    pending_functional: Option<FunctionalState>,
}

impl Cp0 {
    pub(super) const fn new() -> Self {
        Self {
            index: 0,
            random: RANDOM_RESET,
            entry_lo: 0,
            context: 0,
            bad_vaddr: 0,
            entry_hi: 0,
            status: STATUS_BEV,
            cause: 0,
            epc: 0,
            effective: FunctionalState::from_registers(STATUS_BEV, 0),
            pending_functional: None,
        }
    }

    pub(super) fn reset(&mut self, interrupted_pc: u32) {
        self.status = (self.status | STATUS_BEV) & !(STATUS_TS | STATUS_KUC | STATUS_IEC);
        self.epc = interrupted_pc;
        self.random = RANDOM_RESET;
        self.effective = FunctionalState::from_registers(self.status, self.cause);
        self.pending_functional = None;
    }

    pub(super) fn read_register(&self, index: usize) -> u32 {
        match index {
            0 => self.index & (INDEX_PROBE_FAILURE | INDEX_INDEX_MASK),
            1 => self.random & INDEX_INDEX_MASK,
            2 => self.entry_lo & ENTRY_LO_VISIBLE_MASK,
            4 => self.context & CONTEXT_VISIBLE_MASK,
            8 => self.bad_vaddr,
            10 => self.entry_hi & ENTRY_HI_VISIBLE_MASK,
            12 => self.status & STATUS_VISIBLE_MASK,
            13 => self.cause & CAUSE_VISIBLE_MASK,
            14 => self.epc,
            15 => PRID,
            _ => 0,
        }
    }

    pub(super) fn write_register(&mut self, index: usize, value: u32) {
        match index {
            0 => {
                self.index = (self.index & INDEX_PROBE_FAILURE) | (value & INDEX_INDEX_MASK);
            }
            1 | 8 | 15 => {}
            2 => self.entry_lo = value & ENTRY_LO_VISIBLE_MASK,
            4 => {
                self.context =
                    (self.context & !CONTEXT_PTE_BASE_MASK) | (value & CONTEXT_PTE_BASE_MASK);
            }
            10 => self.entry_hi = value & ENTRY_HI_VISIBLE_MASK,
            12 => self.write_status(value),
            13 => self.write_cause(value),
            14 => self.epc = value,
            _ => {}
        }
    }

    pub(super) fn status(&self) -> u32 {
        self.status
    }

    pub(super) fn tlb_index(&self) -> usize {
        ((self.index & INDEX_INDEX_MASK) >> 8) as usize
    }

    pub(super) fn random_tlb_index(&self) -> usize {
        ((self.random & INDEX_INDEX_MASK) >> 8) as usize
    }

    pub(super) fn tlb_staging(&self) -> (u32, u32) {
        (
            self.entry_hi & ENTRY_HI_VISIBLE_MASK,
            self.entry_lo & ENTRY_LO_VISIBLE_MASK,
        )
    }

    pub(super) fn current_asid(&self) -> u8 {
        ((self.entry_hi & ENTRY_HI_ASID_MASK) >> 6) as u8
    }

    pub(super) fn is_kernel_mode(&self) -> bool {
        self.status & STATUS_KUC == 0
    }

    pub(super) fn is_tlb_shutdown(&self) -> bool {
        self.status & STATUS_TS != 0
    }

    pub(super) fn is_cache_isolated(&self) -> bool {
        self.status & STATUS_ISC != 0
    }

    pub(super) fn caches_swapped(&self) -> bool {
        self.status & STATUS_SWC != 0
    }

    pub(super) fn set_cache_miss(&mut self, miss: bool) {
        if miss {
            self.status |= STATUS_CM;
        } else {
            self.status &= !STATUS_CM;
        }
    }

    pub(super) fn enter_tlb_shutdown(&mut self) {
        self.status |= STATUS_TS;
    }

    pub(super) fn write_tlb_read_result(&mut self, entry_hi: u32, entry_lo: u32) {
        self.entry_hi = entry_hi & ENTRY_HI_VISIBLE_MASK;
        self.entry_lo = entry_lo & ENTRY_LO_VISIBLE_MASK;
    }

    pub(super) fn write_tlb_probe_result(&mut self, index: u32) {
        self.index = index & (INDEX_PROBE_FAILURE | INDEX_INDEX_MASK);
    }

    pub(super) fn is_usable(&self) -> bool {
        self.status & STATUS_KUC == 0 || self.effective.coprocessor_usable & STATUS_CU0 != 0
    }

    pub(super) fn commit_pending_functional(&mut self) {
        if let Some(functional) = self.pending_functional.take() {
            self.effective = functional;
        }
    }

    pub(super) fn restore_status(&mut self, value: u32) {
        self.status = (self.status & !0x0f) | (value & 0x0f);

        let interrupt_enable = self.status & STATUS_IEC;
        self.effective.interrupt_control =
            (self.effective.interrupt_control & !STATUS_IEC) | interrupt_enable;
        if let Some(functional) = &mut self.pending_functional {
            functional.interrupt_control =
                (functional.interrupt_control & !STATUS_IEC) | interrupt_enable;
        }
    }

    pub(super) fn advance_random(&mut self) {
        let current = (self.random & INDEX_INDEX_MASK) >> 8;
        let next = if current <= 8 { 63 } else { current - 1 };
        self.random = next << 8;
    }

    pub(super) fn take_exception(
        &mut self,
        exception: Exception,
        epc: u32,
        in_delay_slot: bool,
    ) -> u32 {
        let (exception_code, bad_address, tlb_address, use_refill_vector) = match exception {
            Exception::InstructionAddressError { address }
            | Exception::LoadAddressError { address } => (4, Some(address), None, false),
            Exception::StoreAddressError { address } => (5, Some(address), None, false),
            Exception::TlbLoad { address, fault } => (
                2,
                Some(address),
                Some(address),
                fault == TlbFaultKind::Miss && address < 0x8000_0000,
            ),
            Exception::TlbStore { address, fault } => (
                3,
                Some(address),
                Some(address),
                fault == TlbFaultKind::Miss && address < 0x8000_0000,
            ),
            Exception::TlbModified { address } => (1, Some(address), Some(address), false),
            Exception::InstructionBusError => (6, None, None, false),
            Exception::DataBusError => (7, None, None, false),
            Exception::Syscall => (8, None, None, false),
            Exception::Breakpoint => (9, None, None, false),
            Exception::ReservedInstruction => (10, None, None, false),
            Exception::CoprocessorUnusable => (11, None, None, false),
            Exception::Overflow => (12, None, None, false),
        };

        self.status =
            (self.status & !STATUS_MODE_STACK_MASK) | ((self.status << 2) & STATUS_MODE_STACK_MASK);
        self.cause = (self.cause & CAUSE_IP_MASK)
            | if in_delay_slot { CAUSE_BD } else { 0 }
            | (exception_code << 2);
        self.epc = epc;

        if let Some(address) = bad_address {
            self.bad_vaddr = address;
        }
        if let Some(address) = tlb_address {
            self.context =
                (self.context & CONTEXT_PTE_BASE_MASK) | ((address >> 10) & CONTEXT_BAD_VPN_MASK);
            self.entry_hi = (address & ENTRY_HI_VPN_MASK) | (self.entry_hi & ENTRY_HI_ASID_MASK);
        }

        self.effective.interrupt_control &= !STATUS_IEC;
        if let Some(functional) = &mut self.pending_functional {
            functional.interrupt_control &= !STATUS_IEC;
        }

        match (self.status & STATUS_BEV != 0, use_refill_vector) {
            (false, false) => GENERAL_EXCEPTION_VECTOR,
            (false, true) => TLB_REFILL_EXCEPTION_VECTOR,
            (true, false) => BOOT_GENERAL_EXCEPTION_VECTOR,
            (true, true) => BOOT_TLB_REFILL_EXCEPTION_VECTOR,
        }
    }

    fn write_status(&mut self, value: u32) {
        let preserved = self.status & !(STATUS_WRITABLE_MASK | STATUS_PE);
        let parity_error = if value & STATUS_PE == 0 {
            self.status & STATUS_PE
        } else {
            0
        };
        self.status = preserved | (value & STATUS_WRITABLE_MASK) | parity_error;

        let mut functional = self.effective;
        functional.coprocessor_usable = self.status & STATUS_CU_MASK;
        functional.interrupt_control = self.status & STATUS_INTERRUPT_CONTROL_MASK;
        self.pending_functional = Some(functional);
    }

    fn write_cause(&mut self, value: u32) {
        self.cause = (self.cause & !CAUSE_SOFTWARE_IP_MASK) | (value & CAUSE_SOFTWARE_IP_MASK);

        let mut functional = self.effective;
        functional.software_interrupts = self.cause & CAUSE_SOFTWARE_IP_MASK;
        self.pending_functional = Some(functional);
    }
}

pub(super) fn execute(
    state: &mut State,
    instruction: Cp0Instruction,
    condition: bool,
) -> Result<(Option<u32>, Option<InstructionEffect>), ExecutionError> {
    if !state.cp0_usable() {
        return Err(ExecutionError::Exception(Exception::CoprocessorUnusable));
    }

    let outcome = match instruction {
        Cp0Instruction::Mfc0 { rt, rd } | Cp0Instruction::Cfc0 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: rt,
                value: state.read_cp0(rd),
                load_merge_bypass: false,
            }),
        ),
        Cp0Instruction::Mtc0 { rt, rd } | Cp0Instruction::Ctc0 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: rd,
                value: state.read_gpr(rt),
            }),
        ),
        Cp0Instruction::Bc0f { offset } => {
            (Some(branch_resume_pc(state.pc(), offset, !condition)), None)
        }
        Cp0Instruction::Bc0t { offset } => {
            (Some(branch_resume_pc(state.pc(), offset, condition)), None)
        }
        Cp0Instruction::Tlbr => (None, Some(state.tlbr_effect())),
        Cp0Instruction::Tlbwi => (None, Some(state.tlbwi_effect())),
        Cp0Instruction::Tlbwr => (None, Some(state.tlbwr_effect())),
        Cp0Instruction::Tlbp => (None, Some(state.tlbp_effect()?)),
        Cp0Instruction::Rfe => {
            let status = state.cp0_status();
            let restored = (status & !0x0f) | ((status >> 2) & 0x0f);
            (
                None,
                Some(InstructionEffect::RestoreStatus { value: restored }),
            )
        }
    };

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::{
        BOOT_GENERAL_EXCEPTION_VECTOR, BOOT_TLB_REFILL_EXCEPTION_VECTOR, CAUSE_BD, CAUSE_IP_MASK,
        CAUSE_SOFTWARE_IP_MASK, CAUSE_VISIBLE_MASK, CONTEXT_BAD_VPN_MASK, CONTEXT_PTE_BASE_MASK,
        Cp0, Cp0Instruction, ENTRY_HI_VISIBLE_MASK, ENTRY_HI_VPN_MASK, ENTRY_LO_VISIBLE_MASK,
        Exception, ExecutionError, FunctionalState, GENERAL_EXCEPTION_VECTOR, INDEX_INDEX_MASK,
        INDEX_PROBE_FAILURE, InstructionEffect, PRID, RANDOM_RESET, STATUS_BEV, STATUS_CM,
        STATUS_CU_MASK, STATUS_CU0, STATUS_IEC, STATUS_INTERRUPT_CONTROL_MASK, STATUS_ISC,
        STATUS_KUC, STATUS_MODE_STACK_MASK, STATUS_PE, STATUS_SWC, STATUS_TS, STATUS_VISIBLE_MASK,
        STATUS_WRITABLE_MASK, State, TLB_REFILL_EXCEPTION_VECTOR, TlbFaultKind, execute,
    };

    #[test]
    fn new_initializes_deterministic_state() {
        let cp0 = Cp0::new();

        assert_eq!(cp0.index, 0);
        assert_eq!(cp0.random, RANDOM_RESET);
        assert_eq!(cp0.entry_lo, 0);
        assert_eq!(cp0.context, 0);
        assert_eq!(cp0.entry_hi, 0);
        assert_eq!(cp0.status, STATUS_BEV);
        assert_eq!(cp0.cause, 0);
        assert_eq!(cp0.epc, 0);
        assert_eq!(cp0.bad_vaddr, 0);
        assert_eq!(
            cp0.effective,
            FunctionalState::from_registers(STATUS_BEV, 0)
        );
        assert_eq!(cp0.pending_functional, None);
    }

    #[test]
    fn reset_updates_only_defined_state() {
        let original_status = !STATUS_BEV;
        let original_cause = 0x8123_4500;
        let original_bad_vaddr = 0x1234_5678;
        let interrupted_pc = 0x89ab_cdef;
        let mut cp0 = Cp0::new();
        cp0.index = INDEX_PROBE_FAILURE | INDEX_INDEX_MASK;
        cp0.random = 8 << 8;
        cp0.entry_lo = ENTRY_LO_VISIBLE_MASK;
        cp0.context = 0xaaaa_aaa8;
        cp0.entry_hi = ENTRY_HI_VISIBLE_MASK;
        cp0.status = original_status;
        cp0.cause = original_cause;
        cp0.epc = 0x7654_3210;
        cp0.bad_vaddr = original_bad_vaddr;
        cp0.effective = FunctionalState::from_registers(0, 0);
        cp0.pending_functional = Some(FunctionalState::from_registers(u32::MAX, u32::MAX));

        cp0.reset(interrupted_pc);

        assert_eq!(
            cp0.status,
            (original_status | STATUS_BEV) & !(STATUS_TS | STATUS_KUC | STATUS_IEC)
        );
        assert_eq!(cp0.cause, original_cause);
        assert_eq!(cp0.epc, interrupted_pc);
        assert_eq!(cp0.bad_vaddr, original_bad_vaddr);
        assert_eq!(cp0.index, INDEX_PROBE_FAILURE | INDEX_INDEX_MASK);
        assert_eq!(cp0.random, RANDOM_RESET);
        assert_eq!(cp0.entry_lo, ENTRY_LO_VISIBLE_MASK);
        assert_eq!(cp0.context, 0xaaaa_aaa8);
        assert_eq!(cp0.entry_hi, ENTRY_HI_VISIBLE_MASK);
        assert_eq!(
            cp0.effective,
            FunctionalState::from_registers(cp0.status, cp0.cause)
        );
        assert_eq!(cp0.pending_functional, None);
    }

    #[test]
    fn exception_entry_stacks_status_and_selects_vector() {
        for (bev, expected_vector) in [
            (false, GENERAL_EXCEPTION_VECTOR),
            (true, BOOT_GENERAL_EXCEPTION_VECTOR),
        ] {
            let mut cp0 = Cp0::new();
            cp0.status = 0xa518_0039 | if bev { STATUS_BEV } else { 0 };
            let original_status = cp0.status;

            let vector = cp0.take_exception(Exception::Syscall, 0x1234_5678, false);

            assert_eq!(vector, expected_vector);
            assert_eq!(
                cp0.status,
                (original_status & !STATUS_MODE_STACK_MASK)
                    | ((original_status << 2) & STATUS_MODE_STACK_MASK)
            );
        }
    }

    #[test]
    fn exception_entry_records_epc_bd_and_exception_code() {
        let cases = [
            (
                Exception::TlbModified {
                    address: 0x1111_1111,
                },
                1,
                false,
            ),
            (
                Exception::TlbLoad {
                    address: 0x2222_2222,
                    fault: TlbFaultKind::Miss,
                },
                2,
                true,
            ),
            (
                Exception::TlbStore {
                    address: 0x3333_3333,
                    fault: TlbFaultKind::Invalid,
                },
                3,
                false,
            ),
            (
                Exception::InstructionAddressError {
                    address: 0x4444_4444,
                },
                4,
                false,
            ),
            (
                Exception::LoadAddressError {
                    address: 0x5555_5555,
                },
                4,
                false,
            ),
            (
                Exception::StoreAddressError {
                    address: 0x6666_6666,
                },
                5,
                true,
            ),
            (Exception::InstructionBusError, 6, false),
            (Exception::DataBusError, 7, true),
            (Exception::Syscall, 8, true),
            (Exception::Breakpoint, 9, false),
            (Exception::ReservedInstruction, 10, true),
            (Exception::CoprocessorUnusable, 11, false),
            (Exception::Overflow, 12, false),
        ];

        for (exception, exception_code, in_delay_slot) in cases {
            let original_cause = 0x7f00_a5ff;
            let epc = 0x8765_4321;
            let mut cp0 = Cp0::new();
            cp0.cause = original_cause;

            cp0.take_exception(exception, epc, in_delay_slot);

            assert_eq!(cp0.epc, epc);
            assert_eq!(
                cp0.cause,
                (original_cause & CAUSE_IP_MASK)
                    | if in_delay_slot { CAUSE_BD } else { 0 }
                    | (exception_code << 2)
            );
        }
    }

    #[test]
    fn address_errors_update_only_bad_vaddr_and_use_general_vector() {
        let context = 0xabc5_4320;
        let entry_hi = 0x7654_3a80;

        for exception in [
            Exception::InstructionAddressError {
                address: 0x1111_1111,
            },
            Exception::LoadAddressError {
                address: 0x2222_2222,
            },
            Exception::StoreAddressError {
                address: 0x3333_3333,
            },
        ] {
            let mut cp0 = Cp0::new();
            cp0.context = context;
            cp0.entry_hi = entry_hi;

            let vector = cp0.take_exception(exception, 0, false);

            let expected_address = match exception {
                Exception::InstructionAddressError { address }
                | Exception::LoadAddressError { address }
                | Exception::StoreAddressError { address } => address,
                _ => unreachable!(),
            };
            assert_eq!(vector, BOOT_GENERAL_EXCEPTION_VECTOR);
            assert_eq!(cp0.bad_vaddr, expected_address);
            assert_eq!(cp0.context, context);
            assert_eq!(cp0.entry_hi, entry_hi);
        }
    }

    #[test]
    fn non_address_exceptions_preserve_translation_registers() {
        let original_bad_vaddr = 0x1234_5678;
        let original_context = 0xabc0_0000;
        let original_entry_hi = 0x5678_9a80;
        let mut cp0 = Cp0::new();

        for exception in [
            Exception::InstructionBusError,
            Exception::DataBusError,
            Exception::Syscall,
            Exception::Breakpoint,
            Exception::ReservedInstruction,
            Exception::CoprocessorUnusable,
            Exception::Overflow,
        ] {
            cp0.bad_vaddr = original_bad_vaddr;
            cp0.context = original_context;
            cp0.entry_hi = original_entry_hi;

            cp0.take_exception(exception, 0, false);

            assert_eq!(cp0.bad_vaddr, original_bad_vaddr);
            assert_eq!(cp0.context, original_context);
            assert_eq!(cp0.entry_hi, original_entry_hi);
        }
    }

    #[test]
    fn tlb_exceptions_reconstruct_address_registers() {
        let fault_address = 0xf123_4567;
        let pte_base = 0xabc0_0000;
        let asid = 0x0000_0a40;
        let mut cp0 = Cp0::new();
        cp0.context = pte_base | 0x0012_3454;
        cp0.entry_hi = 0x4567_8000 | asid;

        cp0.take_exception(
            Exception::TlbLoad {
                address: fault_address,
                fault: TlbFaultKind::Invalid,
            },
            0,
            false,
        );

        assert_eq!(cp0.bad_vaddr, fault_address);
        assert_eq!(
            cp0.context,
            pte_base | ((fault_address >> 10) & CONTEXT_BAD_VPN_MASK)
        );
        assert_eq!(cp0.entry_hi, (fault_address & ENTRY_HI_VPN_MASK) | asid);
    }

    #[test]
    fn tlb_exception_vectors_distinguish_refill_and_general_cases() {
        let cases = [
            (
                false,
                Exception::TlbLoad {
                    address: 0x1234_5000,
                    fault: TlbFaultKind::Miss,
                },
                TLB_REFILL_EXCEPTION_VECTOR,
            ),
            (
                true,
                Exception::TlbStore {
                    address: 0x1234_5000,
                    fault: TlbFaultKind::Miss,
                },
                BOOT_TLB_REFILL_EXCEPTION_VECTOR,
            ),
            (
                false,
                Exception::TlbLoad {
                    address: 0xc123_4000,
                    fault: TlbFaultKind::Miss,
                },
                GENERAL_EXCEPTION_VECTOR,
            ),
            (
                true,
                Exception::TlbLoad {
                    address: 0x1234_5000,
                    fault: TlbFaultKind::Invalid,
                },
                BOOT_GENERAL_EXCEPTION_VECTOR,
            ),
            (
                false,
                Exception::TlbModified {
                    address: 0x1234_5000,
                },
                GENERAL_EXCEPTION_VECTOR,
            ),
        ];

        for (bev, exception, expected_vector) in cases {
            let mut cp0 = Cp0::new();
            cp0.status = if bev { STATUS_BEV } else { 0 };

            assert_eq!(cp0.take_exception(exception, 0, false), expected_vector);
        }
    }

    #[test]
    fn tlb_helpers_apply_register_masks_and_shutdown_rules() {
        let mut cp0 = Cp0::new();
        cp0.index = INDEX_PROBE_FAILURE | (37 << 8);
        cp0.random = 8 << 8;
        cp0.entry_hi = 0x1234_5a80;
        cp0.entry_lo = 0x9876_5f00;

        assert_eq!(cp0.tlb_index(), 37);
        assert_eq!(cp0.random_tlb_index(), 8);
        assert_eq!(cp0.tlb_staging(), (0x1234_5a80, 0x9876_5f00));
        assert_eq!(cp0.current_asid(), 0x2a);
        assert!(cp0.is_kernel_mode());

        cp0.status |= STATUS_KUC;
        assert!(!cp0.is_kernel_mode());

        cp0.write_tlb_read_result(u32::MAX, u32::MAX);
        assert_eq!(
            cp0.tlb_staging(),
            (ENTRY_HI_VISIBLE_MASK, ENTRY_LO_VISIBLE_MASK)
        );

        cp0.write_tlb_probe_result(INDEX_PROBE_FAILURE);
        assert_eq!(cp0.read_register(0), INDEX_PROBE_FAILURE);
        cp0.write_register(0, 12 << 8);
        assert_eq!(cp0.read_register(0), INDEX_PROBE_FAILURE | (12 << 8));
        cp0.write_tlb_probe_result(25 << 8);
        assert_eq!(cp0.read_register(0), 25 << 8);

        assert!(!cp0.is_tlb_shutdown());
        cp0.enter_tlb_shutdown();
        assert!(cp0.is_tlb_shutdown());
        cp0.write_register(12, 0);
        assert!(cp0.is_tlb_shutdown());
        cp0.reset(0);
        assert!(!cp0.is_tlb_shutdown());
    }

    #[test]
    fn register_access_applies_masks_and_read_only_rules() {
        let mut cp0 = Cp0::new();

        cp0.index = INDEX_PROBE_FAILURE;
        cp0.write_register(0, u32::MAX);
        assert_eq!(cp0.read_register(0), INDEX_PROBE_FAILURE | INDEX_INDEX_MASK);

        cp0.write_register(1, 0);
        assert_eq!(cp0.read_register(1), RANDOM_RESET);

        cp0.write_register(2, u32::MAX);
        assert_eq!(cp0.read_register(2), ENTRY_LO_VISIBLE_MASK);

        let bad_vpn = 0x001f_fffc;
        cp0.context = bad_vpn;
        cp0.write_register(4, u32::MAX);
        assert_eq!(cp0.read_register(4), CONTEXT_PTE_BASE_MASK | bad_vpn);

        cp0.bad_vaddr = 0x1234_5678;
        cp0.write_register(8, u32::MAX);
        assert_eq!(cp0.read_register(8), 0x1234_5678);

        cp0.write_register(10, u32::MAX);
        assert_eq!(cp0.read_register(10), ENTRY_HI_VISIBLE_MASK);

        cp0.write_register(14, 0x89ab_cdef);
        assert_eq!(cp0.read_register(14), 0x89ab_cdef);

        cp0.write_register(15, 0);
        assert_eq!(cp0.read_register(15), PRID);
        cp0.write_register(3, u32::MAX);
        assert_eq!(cp0.read_register(3), 0);
        assert_eq!(cp0.read_register(31), 0);
    }

    #[test]
    fn status_and_cause_writes_preserve_hardware_fields() {
        let mut cp0 = Cp0::new();
        cp0.status = STATUS_TS | STATUS_PE | STATUS_CM;

        cp0.write_register(12, u32::MAX);

        assert_eq!(
            cp0.read_register(12),
            (STATUS_TS | STATUS_CM | STATUS_WRITABLE_MASK) & STATUS_VISIBLE_MASK
        );
        assert_eq!(cp0.status & STATUS_PE, 0);

        cp0.status = STATUS_TS | STATUS_PE | STATUS_CM;
        cp0.write_register(12, 0);
        assert_eq!(cp0.read_register(12), STATUS_TS | STATUS_PE | STATUS_CM);

        let original_cause = CAUSE_BD | (3 << 28) | 0x0000_fc7c;
        cp0.cause = original_cause;
        cp0.write_register(13, 0x0000_0100);
        assert_eq!(
            cp0.cause,
            (original_cause & !CAUSE_SOFTWARE_IP_MASK) | 0x0000_0100
        );
        assert_eq!(cp0.read_register(13), cp0.cause & CAUSE_VISIBLE_MASK);
    }

    #[test]
    fn cache_status_helpers_use_raw_status_and_preserve_hardware_miss() {
        const STATUS_PZ: u32 = 1 << 18;

        let mut cp0 = Cp0::new();
        cp0.write_register(12, STATUS_BEV | STATUS_PZ | STATUS_ISC | STATUS_SWC);

        assert!(cp0.is_cache_isolated());
        assert!(cp0.caches_swapped());
        assert_eq!(cp0.read_register(12) & STATUS_PZ, STATUS_PZ);
        assert_eq!(cp0.read_register(12) & STATUS_CM, 0);

        cp0.set_cache_miss(true);
        assert_eq!(cp0.read_register(12) & STATUS_PZ, STATUS_PZ);
        assert_eq!(cp0.read_register(12) & STATUS_CM, STATUS_CM);

        cp0.write_register(12, 0);
        assert!(!cp0.is_cache_isolated());
        assert!(!cp0.caches_swapped());
        assert_eq!(cp0.read_register(12) & STATUS_CM, STATUS_CM);

        cp0.set_cache_miss(false);
        assert_eq!(cp0.read_register(12) & STATUS_CM, 0);
    }

    #[test]
    fn random_decrements_and_wraps_at_the_wired_range() {
        let mut cp0 = Cp0::new();

        assert_eq!(cp0.read_register(1), 63 << 8);
        cp0.advance_random();
        assert_eq!(cp0.read_register(1), 62 << 8);

        cp0.random = 9 << 8;
        cp0.advance_random();
        assert_eq!(cp0.read_register(1), 8 << 8);
        cp0.advance_random();
        assert_eq!(cp0.read_register(1), 63 << 8);
    }

    #[test]
    fn functional_control_changes_only_when_committed() {
        let mut cp0 = Cp0::new();
        let status = STATUS_BEV | STATUS_KUC | STATUS_CU0 | 0x0000_5500 | STATUS_IEC;

        cp0.write_register(12, status);

        assert_eq!(cp0.read_register(12), status);
        assert!(!cp0.is_usable());
        assert_eq!(cp0.effective.coprocessor_usable, 0);
        assert_eq!(cp0.effective.interrupt_control, 0);

        cp0.commit_pending_functional();

        assert!(cp0.is_usable());
        assert_eq!(cp0.effective.coprocessor_usable, status & STATUS_CU_MASK);
        assert_eq!(
            cp0.effective.interrupt_control,
            status & STATUS_INTERRUPT_CONTROL_MASK
        );

        cp0.write_register(13, CAUSE_SOFTWARE_IP_MASK);
        assert_eq!(cp0.effective.software_interrupts, 0);
        cp0.commit_pending_functional();
        assert_eq!(cp0.effective.software_interrupts, CAUSE_SOFTWARE_IP_MASK);
    }

    #[test]
    fn consecutive_functional_writes_preserve_staggered_groups() {
        let mut cp0 = Cp0::new();
        let status = STATUS_BEV | STATUS_CU0 | 0x0000_5500 | STATUS_IEC;

        cp0.write_register(12, status);
        assert_eq!(
            cp0.effective,
            FunctionalState::from_registers(STATUS_BEV, 0)
        );

        cp0.commit_pending_functional();
        cp0.write_register(13, 0x0000_0200);

        assert_eq!(cp0.effective.coprocessor_usable, STATUS_CU0);
        assert_eq!(cp0.effective.interrupt_control, 0x0000_5501);
        assert_eq!(cp0.effective.software_interrupts, 0);
        assert_eq!(
            cp0.pending_functional,
            Some(FunctionalState {
                coprocessor_usable: STATUS_CU0,
                interrupt_control: 0x0000_5501,
                software_interrupts: 0x0000_0200,
            })
        );

        cp0.commit_pending_functional();

        assert_eq!(
            cp0.effective,
            FunctionalState::from_registers(status, 0x0000_0200)
        );
    }

    #[test]
    fn restore_status_synchronizes_interrupt_enable_only() {
        let mut cp0 = Cp0::new();
        cp0.status = STATUS_BEV | STATUS_CU0 | 0x0000_aa00 | 0x3d;
        cp0.effective = FunctionalState {
            coprocessor_usable: STATUS_CU0,
            interrupt_control: 0x0000_aa00,
            software_interrupts: 0x0000_0100,
        };
        cp0.pending_functional = Some(FunctionalState {
            coprocessor_usable: 0,
            interrupt_control: 0x0000_5500,
            software_interrupts: 0x0000_0200,
        });
        let restored = (cp0.status & !0x0f) | ((cp0.status >> 2) & 0x0f);

        cp0.restore_status(restored);

        assert_eq!(cp0.status, (STATUS_BEV | STATUS_CU0 | 0x0000_aa00 | 0x3f));
        assert_eq!(cp0.effective.coprocessor_usable, STATUS_CU0);
        assert_eq!(cp0.effective.interrupt_control, 0x0000_aa01);
        assert_eq!(cp0.effective.software_interrupts, 0x0000_0100);
        assert_eq!(
            cp0.pending_functional,
            Some(FunctionalState {
                coprocessor_usable: 0,
                interrupt_control: 0x0000_5501,
                software_interrupts: 0x0000_0200,
            })
        );
    }

    #[test]
    fn exception_entry_cancels_delayed_interrupt_enable() {
        let mut cp0 = Cp0::new();
        cp0.status |= STATUS_IEC;
        cp0.effective.interrupt_control |= STATUS_IEC;
        cp0.pending_functional = Some(FunctionalState {
            coprocessor_usable: STATUS_CU0,
            interrupt_control: 0x0000_ff01,
            software_interrupts: CAUSE_SOFTWARE_IP_MASK,
        });

        cp0.take_exception(Exception::CoprocessorUnusable, 0, false);

        assert_eq!(cp0.status & STATUS_IEC, 0);
        assert_eq!(cp0.effective.interrupt_control & STATUS_IEC, 0);
        assert_eq!(
            cp0.pending_functional
                .expect("functional state should remain pending")
                .interrupt_control
                & STATUS_IEC,
            0
        );
        assert_eq!(cp0.cause & (3 << 28), 0);
        assert_eq!((cp0.cause >> 2) & 0x1f, 11);
    }

    #[test]
    fn instruction_execution_captures_values_and_branches() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.write_gpr(2, 0x1234_5678);

        assert_eq!(
            execute(&mut state, Cp0Instruction::Mfc0 { rt: 1, rd: 15 }, false),
            Ok((
                None,
                Some(InstructionEffect::DelayedGprWrite {
                    index: 1,
                    value: PRID,
                    load_merge_bypass: false,
                })
            ))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Cfc0 { rt: 1, rd: 15 }, false),
            Ok((
                None,
                Some(InstructionEffect::DelayedGprWrite {
                    index: 1,
                    value: PRID,
                    load_merge_bypass: false,
                })
            ))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Mfc0 { rt: 5, rd: 1 }, false),
            Ok((
                None,
                Some(InstructionEffect::DelayedGprWrite {
                    index: 5,
                    value: RANDOM_RESET,
                    load_merge_bypass: false,
                })
            ))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Mtc0 { rt: 2, rd: 14 }, false),
            Ok((
                None,
                Some(InstructionEffect::DelayedCp0Write {
                    index: 14,
                    value: 0x1234_5678,
                })
            ))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Ctc0 { rt: 2, rd: 14 }, false),
            Ok((
                None,
                Some(InstructionEffect::DelayedCp0Write {
                    index: 14,
                    value: 0x1234_5678,
                })
            ))
        );

        let pc = state.pc();
        assert_eq!(
            execute(&mut state, Cp0Instruction::Bc0f { offset: 2 }, false),
            Ok((Some(pc + 12), None))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Bc0t { offset: 2 }, false),
            Ok((Some(pc + 8), None))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Rfe, false),
            Ok((
                None,
                Some(InstructionEffect::RestoreStatus { value: STATUS_BEV })
            ))
        );
    }

    #[test]
    fn tlb_instruction_execution_returns_state_coordinated_effects() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);

        assert_eq!(
            execute(&mut state, Cp0Instruction::Tlbr, false),
            Ok((
                None,
                Some(InstructionEffect::DelayedTlbRead {
                    entry_hi: 0,
                    entry_lo: 0,
                })
            ))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Tlbwi, false),
            Ok((
                None,
                Some(InstructionEffect::TlbWrite {
                    index: 0,
                    entry_hi: 0,
                    entry_lo: 0,
                })
            ))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Tlbwr, false),
            Ok((
                None,
                Some(InstructionEffect::TlbWrite {
                    index: 63,
                    entry_hi: 0,
                    entry_lo: 0,
                })
            ))
        );
        assert_eq!(
            execute(&mut state, Cp0Instruction::Tlbp, false),
            Err(ExecutionError::TlbShutdown)
        );
        assert!(state.is_tlb_shutdown());
    }

    #[test]
    fn cp0_usability_uses_raw_mode_and_effective_cu0() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let user_with_cu0 = STATUS_BEV | STATUS_KUC | STATUS_CU0;

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: user_with_cu0,
            }),
        );
        state.complete_instruction(None, None);

        assert_eq!(
            execute(&mut state, Cp0Instruction::Mfc0 { rt: 1, rd: 15 }, false),
            Err(ExecutionError::Exception(Exception::CoprocessorUnusable))
        );

        state.complete_instruction(None, None);

        assert!(execute(&mut state, Cp0Instruction::Mfc0 { rt: 1, rd: 15 }, false).is_ok());
    }

    #[test]
    fn every_cp0_instruction_checks_usability_before_execution() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: STATUS_BEV | STATUS_KUC,
            }),
        );
        state.complete_instruction(None, None);

        for instruction in [
            Cp0Instruction::Mfc0 { rt: 1, rd: 15 },
            Cp0Instruction::Cfc0 { rt: 1, rd: 15 },
            Cp0Instruction::Mtc0 { rt: 1, rd: 14 },
            Cp0Instruction::Ctc0 { rt: 1, rd: 14 },
            Cp0Instruction::Bc0f { offset: 0 },
            Cp0Instruction::Bc0t { offset: 0 },
            Cp0Instruction::Tlbr,
            Cp0Instruction::Tlbwi,
            Cp0Instruction::Tlbwr,
            Cp0Instruction::Tlbp,
            Cp0Instruction::Rfe,
        ] {
            assert_eq!(
                execute(&mut state, instruction, false),
                Err(ExecutionError::Exception(Exception::CoprocessorUnusable))
            );
        }
    }
}
