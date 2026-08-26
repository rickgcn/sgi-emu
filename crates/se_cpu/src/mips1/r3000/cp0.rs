const STATUS_BEV: u32 = 1 << 22;
const STATUS_TS: u32 = 1 << 21;
const STATUS_KUC: u32 = 1 << 1;
const STATUS_IEC: u32 = 1;
const STATUS_MODE_STACK_MASK: u32 = 0x3f;

const CAUSE_BD: u32 = 1 << 31;
const CAUSE_IP_MASK: u32 = 0x0000_ff00;

const GENERAL_EXCEPTION_VECTOR: u32 = 0x8000_0080;
const BOOT_GENERAL_EXCEPTION_VECTOR: u32 = 0xbfc0_0180;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Exception {
    InstructionAddressError { address: u32 },
    Syscall,
    Breakpoint,
    ReservedInstruction,
    Overflow,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Cp0 {
    status: u32,
    cause: u32,
    epc: u32,
    bad_vaddr: u32,
}

impl Cp0 {
    pub(super) const fn new() -> Self {
        Self {
            status: STATUS_BEV,
            cause: 0,
            epc: 0,
            bad_vaddr: 0,
        }
    }

    pub(super) fn reset(&mut self, interrupted_pc: u32) {
        self.status = (self.status | STATUS_BEV) & !(STATUS_TS | STATUS_KUC | STATUS_IEC);
        self.epc = interrupted_pc;
    }

    pub(super) fn take_exception(
        &mut self,
        exception: Exception,
        epc: u32,
        in_delay_slot: bool,
    ) -> u32 {
        let (exception_code, bad_address) = match exception {
            Exception::InstructionAddressError { address } => (4, Some(address)),
            Exception::Syscall => (8, None),
            Exception::Breakpoint => (9, None),
            Exception::ReservedInstruction => (10, None),
            Exception::Overflow => (12, None),
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

        if self.status & STATUS_BEV == 0 {
            GENERAL_EXCEPTION_VECTOR
        } else {
            BOOT_GENERAL_EXCEPTION_VECTOR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BOOT_GENERAL_EXCEPTION_VECTOR, CAUSE_BD, CAUSE_IP_MASK, Cp0, Exception,
        GENERAL_EXCEPTION_VECTOR, STATUS_BEV, STATUS_IEC, STATUS_KUC, STATUS_MODE_STACK_MASK,
        STATUS_TS,
    };

    #[test]
    fn new_initializes_deterministic_state() {
        let cp0 = Cp0::new();

        assert_eq!(cp0.status, STATUS_BEV);
        assert_eq!(cp0.cause, 0);
        assert_eq!(cp0.epc, 0);
        assert_eq!(cp0.bad_vaddr, 0);
    }

    #[test]
    fn reset_updates_only_defined_state() {
        let original_status = !STATUS_BEV;
        let original_cause = 0x8123_4500;
        let original_bad_vaddr = 0x1234_5678;
        let interrupted_pc = 0x89ab_cdef;
        let mut cp0 = Cp0 {
            status: original_status,
            cause: original_cause,
            epc: 0x7654_3210,
            bad_vaddr: original_bad_vaddr,
        };

        cp0.reset(interrupted_pc);

        assert_eq!(
            cp0.status,
            (original_status | STATUS_BEV) & !(STATUS_TS | STATUS_KUC | STATUS_IEC)
        );
        assert_eq!(cp0.cause, original_cause);
        assert_eq!(cp0.epc, interrupted_pc);
        assert_eq!(cp0.bad_vaddr, original_bad_vaddr);
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
                Exception::InstructionAddressError {
                    address: 0x1111_1111,
                },
                4,
                false,
            ),
            (Exception::Syscall, 8, true),
            (Exception::Breakpoint, 9, false),
            (Exception::ReservedInstruction, 10, true),
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
    fn address_error_is_the_only_exception_that_updates_bad_vaddr() {
        let original_bad_vaddr = 0x1234_5678;
        let fault_address = 0x8765_4321;
        let mut cp0 = Cp0::new();
        cp0.bad_vaddr = original_bad_vaddr;

        cp0.take_exception(
            Exception::InstructionAddressError {
                address: fault_address,
            },
            0,
            false,
        );
        assert_eq!(cp0.bad_vaddr, fault_address);

        for exception in [
            Exception::Syscall,
            Exception::Breakpoint,
            Exception::ReservedInstruction,
            Exception::Overflow,
        ] {
            cp0.bad_vaddr = original_bad_vaddr;

            cp0.take_exception(exception, 0, false);

            assert_eq!(cp0.bad_vaddr, original_bad_vaddr);
        }
    }
}
