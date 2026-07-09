//! `SYNC` memory-ordering operation marker.
//!
//! This module models the manual `SyncOperation` pseudocode (MIPS IV manual
//! section A.2, Table A-19) as an immutable typed marker. It carries the
//! synchronization type field without performing any architectural side effect;
//! concrete ordering over the cache hierarchy or system bus is implementation-
//! and system-specific behavior that stays outside this base layer.
//!
//! Manual semantics: a `SYNC` orders synchronizable loads and stores (those to
//! shared memory using an uncached or cached coherent access type) so that all
//! such accesses before the `SYNC` are globally performed before any after it
//! may be performed. The `stype` field value `0` is the defined full
//! synchronization; values `1` through `31` are reserved but produce the same
//! result as `0`.

/// 5-bit `stype` field of the `SYNC` instruction.
///
/// The manual defines only `stype = 0` (full synchronization). Values `1` through
/// `31` are reserved but produce the same result as `0`; they are preserved as
/// [`Self::Reserved`] so the raw field round-trips.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4SyncStype {
    /// `stype = 0`: full synchronization of prior and subsequent loads and stores.
    Full,

    /// A reserved `stype` value (`1` through `31`), which behaves the same as `Full`.
    Reserved(u8),
}

impl Mips4SyncStype {
    /// Creates a `stype` from the raw 5-bit field value.
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 0x1f {
            0 => Self::Full,
            other => Self::Reserved(other),
        }
    }

    /// Returns the raw 5-bit `stype` field value.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Full => 0,
            Self::Reserved(bits) => bits,
        }
    }

    /// Returns whether the architecture defines this `stype` value.
    pub const fn is_defined(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Manual `SyncOperation` typed marker.
///
/// The marker records the `stype` field for fidelity. Every `stype` value —
/// defined or reserved — produces the full synchronization effect described by
/// the manual, so this type carries no further behavioral distinction. It has
/// no architectural side effect: actual cache drain or bus ordering is
/// implementation- and system-specific and stays outside the base ISA layer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Mips4SyncOperation {
    /// Synchronization type field selected by the instruction.
    pub stype: Mips4SyncStype,
}

impl Mips4SyncOperation {
    /// Creates a `SyncOperation` marker from its synchronization type.
    pub const fn new(stype: Mips4SyncStype) -> Self {
        Self { stype }
    }

    /// Returns the synchronization type field.
    pub const fn stype(self) -> Mips4SyncStype {
        self.stype
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stype_round_trips_defined_and_reserved_values() {
        for bits in 0..=31u8 {
            let stype = Mips4SyncStype::from_bits(bits);
            assert_eq!(stype.bits(), bits, "stype {bits}");
            assert_eq!(stype.is_defined(), bits == 0, "stype {bits} defined flag");
        }
    }

    #[test]
    fn stype_masks_to_five_bits() {
        assert_eq!(Mips4SyncStype::from_bits(0x20), Mips4SyncStype::Full);
        assert_eq!(Mips4SyncStype::from_bits(0x21), Mips4SyncStype::Reserved(1));
    }

    #[test]
    fn sync_operation_carries_stype() {
        let operation = Mips4SyncOperation::new(Mips4SyncStype::Reserved(7));
        assert_eq!(operation.stype(), Mips4SyncStype::Reserved(7));
        assert!(!operation.stype().is_defined());

        assert_eq!(
            Mips4SyncOperation::new(Mips4SyncStype::Full).stype(),
            Mips4SyncStype::Full
        );
    }
}
