//! Encapsulates general-purpose register storage and index validation.
//!
//! [`Reg`] excludes indices outside `0..32`. [`GprFile`] owns every write so
//! register zero always reads as zero and ignores writes; no mutable view of the
//! backing array is exposed.

/// Identifies a general-purpose register with an index in `0..32`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Reg(u8);

impl Reg {
    /// The architectural zero register.
    pub(crate) const ZERO: Self = Self(0);

    /// Returns a register when `index` is in `0..32`.
    pub(crate) const fn new(index: u8) -> Option<Self> {
        if index < 32 { Some(Self(index)) } else { None }
    }
}

/// Owns general-purpose register storage while enforcing the zero-register rule.
///
/// Register zero always reads as zero and ignores writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GprFile {
    regs: [u64; 32],
}

impl GprFile {
    pub(crate) const fn new() -> Self {
        Self { regs: [0; 32] }
    }

    pub(crate) fn read(&self, reg: Reg) -> u64 {
        if reg == Reg::ZERO {
            0
        } else {
            self.regs[usize::from(reg.0)]
        }
    }

    pub(crate) fn write(&mut self, reg: Reg, value: u64) {
        if reg != Reg::ZERO {
            self.regs[usize::from(reg.0)] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GprFile, Reg};

    fn reg(index: u8) -> Reg {
        Reg::new(index).expect("test register index must be architectural")
    }

    #[test]
    fn rejects_out_of_range_register_indices() {
        assert_eq!(Reg::new(31), Some(reg(31)));
        assert_eq!(Reg::new(32), None);
        assert_eq!(Reg::new(u8::MAX), None);
    }

    #[test]
    fn zero_register_ignores_writes() {
        let mut gpr = GprFile::new();

        gpr.write(Reg::ZERO, u64::MAX);

        assert_eq!(gpr.read(Reg::ZERO), 0);
    }

    #[test]
    fn nonzero_register_round_trips() {
        let mut gpr = GprFile::new();
        let destination = reg(17);

        gpr.write(destination, 0x0123_4567_89ab_cdef);

        assert_eq!(gpr.read(destination), 0x0123_4567_89ab_cdef);
    }
}
