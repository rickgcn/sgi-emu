//! Applies the CP0 state transitions required for synchronous precise exceptions.
//!
//! [`Cp0`] owns only `Status.EXL`, `Status.BEV`, `Cause.ExcCode`, `Cause.BD`,
//! `EPC`, and `BadVAddr`. Exception requests supply the cause and optional fault
//! address, while exception locations supply the precise `EPC` and `BD` context.
//! The module exposes neither packed CP0 registers nor architectural reset values.

use crate::exception::{ExceptionCode, ExceptionLocation, ExceptionRequest};

const NORMAL_GENERAL_EXCEPTION_VECTOR: u64 = 0xffff_ffff_8000_0180;
const BOOTSTRAP_GENERAL_EXCEPTION_VECTOR: u64 = 0xffff_ffff_bfc0_0380;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Status {
    exl: bool,
    bev: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Cause {
    exception_code: ExceptionCode,
    branch_delay: bool,
}

/// Stores the CP0 subset consumed and produced by synchronous exception entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Cp0 {
    status: Status,
    cause: Cause,
    epc: u64,
    bad_vaddr: u64,
}

impl Cp0 {
    // Constructs explicit semantic-test pre-state. The selected `Cause`, `EPC`,
    // and `BadVAddr` values are test inputs, not R10000 reset semantics.
    #[cfg(test)]
    pub(crate) const fn synthetic_test_state(bev: bool) -> Self {
        Self {
            status: Status { exl: false, bev },
            cause: Cause {
                exception_code: ExceptionCode::ReservedInstruction,
                branch_delay: false,
            },
            epc: 0,
            bad_vaddr: 0,
        }
    }

    pub(crate) const fn exl(&self) -> bool {
        self.status.exl
    }

    pub(crate) const fn bev(&self) -> bool {
        self.status.bev
    }

    pub(crate) const fn exception_code(&self) -> ExceptionCode {
        self.cause.exception_code
    }

    pub(crate) const fn branch_delay(&self) -> bool {
        self.cause.branch_delay
    }

    pub(crate) const fn epc(&self) -> u64 {
        self.epc
    }

    pub(crate) const fn bad_vaddr(&self) -> u64 {
        self.bad_vaddr
    }

    /// Applies one synchronous exception and returns the selected general exception vector.
    ///
    /// When `Status.EXL` is clear, this captures `EPC` and `Cause.BD` from
    /// `location`; when it is set, both fields remain protected. Every request
    /// updates `Cause.ExcCode` and sets `Status.EXL`. A request carrying a fault
    /// address writes `BadVAddr`. For other requests this implementation leaves the
    /// stored value untouched, but the architecture does not define that value.
    ///
    /// `Status.BEV = 0` selects `0xffff_ffff_8000_0180`; `Status.BEV = 1`
    /// selects `0xffff_ffff_bfc0_0380`.
    pub(crate) fn apply_exception(
        &mut self,
        request: ExceptionRequest,
        location: ExceptionLocation,
    ) -> u64 {
        let vector = self.general_exception_vector();

        self.cause.exception_code = request.exception_code();
        if !self.status.exl {
            let (epc, branch_delay) = location.exception_program_counter();
            self.epc = epc;
            self.cause.branch_delay = branch_delay;
        }
        // The R10000 architecture leaves BadVAddr undefined for non-address
        // exceptions. Preserve its previous value as the deterministic policy.
        if let Some(bad_vaddr) = request.bad_vaddr() {
            self.bad_vaddr = bad_vaddr;
        }
        self.status.exl = true;

        vector
    }

    const fn general_exception_vector(&self) -> u64 {
        if self.status.bev {
            BOOTSTRAP_GENERAL_EXCEPTION_VECTOR
        } else {
            NORMAL_GENERAL_EXCEPTION_VECTOR
        }
    }
}
