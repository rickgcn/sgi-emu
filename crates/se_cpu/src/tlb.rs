//! Implements the CPU-local authoritative translation lookaside buffer.
//!
//! The buffer owns 64 fixed slots of paired 4 KiB pages. Mapped-address lookup
//! matches `VPN2` plus `ASID` or global state and returns either a physical byte
//! address or an architectural refill, invalid, or modified fault. CP0 remains
//! responsible for encoding the fault and updating diagnostic registers. Indexed
//! writes capture replacement and conflict information from immutable state so
//! the TLB and `Status.TS` effects can retire in one CPU commit.

use std::error::Error;
use std::fmt;

use se_core::address::PhysAddr;

use crate::memory::AccessKind;

const ENTRY_COUNT: usize = 64;
const VPN2_MASK: u32 = 0x0007_ffff;
const PFN_MASK: u32 = 0x0fff_ffff;
const PAGE_OFFSET_MASK: u64 = 0x0fff;

/// Identifies the architectural reason a mapped translation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TlbFaultReason {
    /// No occupied entry matched the virtual page and current address space.
    Refill,
    /// The selected even or odd page was not valid.
    Invalid,
    /// A store selected a valid page that was not writable.
    Modified,
}

/// Preserves one TLB fault's reason, access type, and original guest virtual
/// byte address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TlbFault {
    reason: TlbFaultReason,
    access: AccessKind,
    virtual_address: u64,
}

impl TlbFault {
    const fn new(reason: TlbFaultReason, access: AccessKind, virtual_address: u64) -> Self {
        Self {
            reason,
            access,
            virtual_address,
        }
    }

    pub(crate) const fn reason(self) -> TlbFaultReason {
        self.reason
    }

    pub(crate) const fn access(self) -> AccessKind {
        self.access
    }

    pub(crate) const fn virtual_address(self) -> u64 {
        self.virtual_address
    }

    #[cfg(test)]
    pub(crate) const fn new_for_test(
        reason: TlbFaultReason,
        access: AccessKind,
        virtual_address: u64,
    ) -> Self {
        Self::new(reason, access, virtual_address)
    }
}

/// Reports authoritative TLB state that would make a lookup ambiguous.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TlbInvariantError {
    /// Two occupied entries matched the same lookup.
    MultipleMatches {
        /// Virtual address presented for translation.
        virtual_address: u64,
        /// First matching TLB slot.
        first_index: u8,
        /// Second matching TLB slot.
        second_index: u8,
    },
}

impl fmt::Display for TlbInvariantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleMatches {
                virtual_address,
                first_index,
                second_index,
            } => write!(
                formatter,
                "virtual address {virtual_address:#018x} matches TLB slots {first_index} and {second_index}"
            ),
        }
    }
}

impl Error for TlbInvariantError {}

/// Separates a successful mapped translation from an architectural TLB fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TlbTranslation {
    /// Physical byte address selected by the matching page.
    Translated(PhysAddr),
    /// Architectural fault raised before any physical transaction.
    Fault(TlbFault),
}

/// Represents one 4 KiB page's physical-frame and permission fields.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TlbPage {
    pfn: u32,
    valid: bool,
    dirty: bool,
}

impl TlbPage {
    /// Constructs page fields, truncating `pfn` to the architectural 28-bit width.
    pub(crate) const fn new(pfn: u32, valid: bool, dirty: bool) -> Self {
        Self {
            pfn: pfn & PFN_MASK,
            valid,
            dirty,
        }
    }

    #[cfg(test)]
    const fn pfn(self) -> u32 {
        self.pfn
    }

    #[cfg(test)]
    const fn valid(self) -> bool {
        self.valid
    }

    #[cfg(test)]
    const fn dirty(self) -> bool {
        self.dirty
    }
}

/// Stores one occupied TLB slot's shared tag and even/odd page mappings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TlbEntry {
    vpn2: u32,
    asid: u8,
    global: bool,
    even: TlbPage,
    odd: TlbPage,
}

impl TlbEntry {
    /// Constructs an entry, truncating `vpn2` to the architectural 19-bit width.
    pub(crate) const fn new(
        vpn2: u32,
        asid: u8,
        global: bool,
        even: TlbPage,
        odd: TlbPage,
    ) -> Self {
        Self {
            vpn2: vpn2 & VPN2_MASK,
            asid,
            global,
            even,
            odd,
        }
    }

    const fn matches(self, vpn2: u32, current_asid: u8) -> bool {
        self.vpn2 == vpn2 && (self.global || self.asid == current_asid)
    }

    const fn conflicts_with(self, other: Self) -> bool {
        self.vpn2 == other.vpn2 && (self.global || other.global || self.asid == other.asid)
    }

    const fn selected_page(self, virtual_address: u64) -> TlbPage {
        if virtual_address & 0x1000 == 0 {
            self.even
        } else {
            self.odd
        }
    }

    #[cfg(test)]
    const fn vpn2(self) -> u32 {
        self.vpn2
    }

    #[cfg(test)]
    const fn asid(self) -> u8 {
        self.asid
    }

    #[cfg(test)]
    const fn global(self) -> bool {
        self.global
    }

    #[cfg(test)]
    const fn even(self) -> TlbPage {
        self.even
    }

    #[cfg(test)]
    const fn odd(self) -> TlbPage {
        self.odd
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TlbSlot {
    Vacant,
    Occupied(TlbEntry),
}

/// Holds one atomic indexed-write decision computed from immutable TLB state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TlbWriteDecision {
    target_index: u8,
    replacement: TlbEntry,
    invalidation_mask: u64,
}

impl TlbWriteDecision {
    /// Reports whether the decision invalidates a conflicting non-target slot.
    pub(crate) const fn conflict_detected(self) -> bool {
        self.invalidation_mask != 0
    }
}

/// Owns all 64 authoritative software-managed TLB entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Tlb {
    slots: [TlbSlot; ENTRY_COUNT],
}

impl Tlb {
    /// Constructs an empty TLB with all 64 slots vacant.
    pub(crate) const fn new() -> Self {
        Self {
            slots: [TlbSlot::Vacant; ENTRY_COUNT],
        }
    }

    /// Translates one canonical mapped 32-bit guest virtual byte address.
    ///
    /// The lookup uses the address's low 32 bits after the caller has classified
    /// its complete representation. A second tag match is an emulator invariant
    /// failure rather than an arbitrary selection. Page validity and writability
    /// are checked only after the unique shared tag has been identified. Address
    /// bit 12 selects the even or odd page, and bits 11:0 become the physical page
    /// offset. The method does not consult or mutate CP0 or machine state.
    ///
    /// # Errors
    ///
    /// Returns [`TlbInvariantError::MultipleMatches`] if authoritative state has
    /// more than one matching slot.
    pub(crate) fn translate(
        &self,
        virtual_address: u64,
        current_asid: u8,
        access: AccessKind,
    ) -> Result<TlbTranslation, TlbInvariantError> {
        let vpn2 = ((virtual_address as u32) >> 13) & VPN2_MASK;
        let mut matched: Option<(u8, TlbEntry)> = None;

        for (index, slot) in self.slots.iter().copied().enumerate() {
            let TlbSlot::Occupied(entry) = slot else {
                continue;
            };
            if !entry.matches(vpn2, current_asid) {
                continue;
            }
            let index = u8::try_from(index).expect("a 64-entry TLB index fits in u8");
            if let Some((first_index, _)) = matched {
                return Err(TlbInvariantError::MultipleMatches {
                    virtual_address,
                    first_index,
                    second_index: index,
                });
            }
            matched = Some((index, entry));
        }

        let Some((_, entry)) = matched else {
            return Ok(TlbTranslation::Fault(TlbFault::new(
                TlbFaultReason::Refill,
                access,
                virtual_address,
            )));
        };
        let page = entry.selected_page(virtual_address);
        if !page.valid {
            return Ok(TlbTranslation::Fault(TlbFault::new(
                TlbFaultReason::Invalid,
                access,
                virtual_address,
            )));
        }
        if matches!(access, AccessKind::Store) && !page.dirty {
            return Ok(TlbTranslation::Fault(TlbFault::new(
                TlbFaultReason::Modified,
                access,
                virtual_address,
            )));
        }

        let physical_address = (u64::from(page.pfn) << 12) | (virtual_address & PAGE_OFFSET_MASK);
        Ok(TlbTranslation::Translated(PhysAddr::new(physical_address)))
    }

    /// Prepares an indexed write from immutable TLB state.
    ///
    /// The caller supplies a six-bit target index. The returned decision captures
    /// the replacement and every conflicting non-target slot; it does not mutate
    /// the TLB until passed to [`Self::apply_indexed_write`]. Entries conflict
    /// when they share a `VPN2` and either entry is global or their ASIDs match.
    pub(crate) fn prepare_indexed_write(
        &self,
        target_index: u8,
        replacement: TlbEntry,
    ) -> TlbWriteDecision {
        debug_assert!(usize::from(target_index) < ENTRY_COUNT);
        let mut invalidation_mask = 0_u64;
        for (index, slot) in self.slots.iter().copied().enumerate() {
            if index == usize::from(target_index) {
                continue;
            }
            if let TlbSlot::Occupied(entry) = slot
                && entry.conflicts_with(replacement)
            {
                invalidation_mask |= 1_u64 << index;
            }
        }
        TlbWriteDecision {
            target_index,
            replacement,
            invalidation_mask,
        }
    }

    /// Applies a prepared replacement and its captured conflict invalidations.
    pub(crate) fn apply_indexed_write(&mut self, decision: TlbWriteDecision) {
        for index in 0..ENTRY_COUNT {
            if decision.invalidation_mask & (1_u64 << index) != 0 {
                self.slots[index] = TlbSlot::Vacant;
            }
        }
        self.slots[usize::from(decision.target_index)] = TlbSlot::Occupied(decision.replacement);
    }

    #[cfg(test)]
    pub(crate) fn entry_for_test(&self, index: u8) -> Option<TlbEntry> {
        match self.slots[usize::from(index)] {
            TlbSlot::Vacant => None,
            TlbSlot::Occupied(entry) => Some(entry),
        }
    }

    #[cfg(test)]
    fn install_for_test(&mut self, index: u8, entry: TlbEntry) {
        self.slots[usize::from(index)] = TlbSlot::Occupied(entry);
    }
}

#[cfg(test)]
mod tests {
    use se_core::address::PhysAddr;

    use super::{
        Tlb, TlbEntry, TlbFault, TlbFaultReason, TlbInvariantError, TlbPage, TlbTranslation,
    };
    use crate::memory::AccessKind;

    const VA: u64 = 0x0040_0123;
    const VPN2: u32 = 0x200;

    fn page(pfn: u32, valid: bool, dirty: bool) -> TlbPage {
        TlbPage::new(pfn, valid, dirty)
    }

    fn entry(asid: u8, global: bool, even: TlbPage, odd: TlbPage) -> TlbEntry {
        TlbEntry::new(VPN2, asid, global, even, odd)
    }

    fn fault(reason: TlbFaultReason, access: AccessKind, virtual_address: u64) -> TlbTranslation {
        TlbTranslation::Fault(TlbFault::new(reason, access, virtual_address))
    }

    #[test]
    fn no_match_and_asid_mismatch_request_refill() {
        let mut tlb = Tlb::new();
        assert_eq!(
            tlb.translate(VA, 7, AccessKind::Load),
            Ok(fault(TlbFaultReason::Refill, AccessKind::Load, VA))
        );

        tlb.install_for_test(3, entry(8, false, page(4, true, true), page(5, true, true)));
        assert_eq!(
            tlb.translate(VA, 7, AccessKind::Fetch),
            Ok(fault(TlbFaultReason::Refill, AccessKind::Fetch, VA))
        );
    }

    #[test]
    fn global_entry_ignores_the_current_asid() {
        let mut tlb = Tlb::new();
        tlb.install_for_test(3, entry(8, true, page(4, true, true), page(5, true, true)));

        assert_eq!(
            tlb.translate(VA, 99, AccessKind::Load),
            Ok(TlbTranslation::Translated(PhysAddr::new(0x4123)))
        );
    }

    #[test]
    fn address_bit_twelve_selects_even_and_odd_pages() {
        let mut tlb = Tlb::new();
        tlb.install_for_test(3, entry(7, false, page(4, true, true), page(9, true, true)));

        assert_eq!(
            tlb.translate(VA, 7, AccessKind::Load),
            Ok(TlbTranslation::Translated(PhysAddr::new(0x4123)))
        );
        assert_eq!(
            tlb.translate(VA | 0x1000, 7, AccessKind::Load),
            Ok(TlbTranslation::Translated(PhysAddr::new(0x9123)))
        );
    }

    #[test]
    fn invalid_and_clean_pages_preserve_distinct_fault_reasons() {
        let mut invalid = Tlb::new();
        invalid.install_for_test(
            1,
            entry(7, false, page(4, false, true), page(5, true, true)),
        );
        assert_eq!(
            invalid.translate(VA, 7, AccessKind::Load),
            Ok(fault(TlbFaultReason::Invalid, AccessKind::Load, VA))
        );

        let mut clean = Tlb::new();
        clean.install_for_test(
            1,
            entry(7, false, page(4, true, false), page(5, true, true)),
        );
        assert_eq!(
            clean.translate(VA, 7, AccessKind::Store),
            Ok(fault(TlbFaultReason::Modified, AccessKind::Store, VA))
        );
        assert_eq!(
            clean.translate(VA, 7, AccessKind::Fetch),
            Ok(TlbTranslation::Translated(PhysAddr::new(0x4123)))
        );
    }

    #[test]
    fn lookup_rejects_multiple_authoritative_matches() {
        let duplicate = entry(7, false, page(4, true, true), page(5, true, true));
        let mut tlb = Tlb::new();
        tlb.install_for_test(1, duplicate);
        tlb.install_for_test(9, duplicate);

        assert_eq!(
            tlb.translate(VA, 7, AccessKind::Load),
            Err(TlbInvariantError::MultipleMatches {
                virtual_address: VA,
                first_index: 1,
                second_index: 9,
            })
        );
    }

    #[test]
    fn indexed_write_invalidates_conflicts_before_installing_replacement() {
        let old = entry(7, false, page(4, true, true), page(5, true, true));
        let replacement = entry(7, false, page(8, true, true), page(9, true, true));
        let mut tlb = Tlb::new();
        tlb.install_for_test(1, old);

        let decision = tlb.prepare_indexed_write(9, replacement);
        assert!(decision.conflict_detected());
        tlb.apply_indexed_write(decision);

        assert_eq!(tlb.entry_for_test(1), None);
        assert_eq!(tlb.entry_for_test(9), Some(replacement));
        assert_eq!(
            tlb.translate(VA, 7, AccessKind::Load),
            Ok(TlbTranslation::Translated(PhysAddr::new(0x8123)))
        );
    }

    #[test]
    fn indexed_write_conflict_accounts_for_global_and_asid_domains() {
        let mut tlb = Tlb::new();
        tlb.install_for_test(1, entry(1, false, page(1, true, true), page(2, true, true)));

        let disjoint =
            tlb.prepare_indexed_write(2, entry(2, false, page(3, true, true), page(4, true, true)));
        assert!(!disjoint.conflict_detected());

        let global =
            tlb.prepare_indexed_write(2, entry(2, true, page(3, true, true), page(4, true, true)));
        assert!(global.conflict_detected());
    }

    #[test]
    fn entry_fields_use_r10000_widths() {
        let entry = TlbEntry::new(
            u32::MAX,
            0xff,
            true,
            TlbPage::new(u32::MAX, true, false),
            TlbPage::new(7, false, true),
        );

        assert_eq!(entry.vpn2(), 0x7ffff);
        assert_eq!(entry.asid(), 0xff);
        assert!(entry.global());
        assert_eq!(entry.even().pfn(), 0x0fff_ffff);
        assert!(entry.even().valid());
        assert!(!entry.even().dirty());
        assert_eq!(entry.odd().pfn(), 7);
        assert!(!entry.odd().valid());
        assert!(entry.odd().dirty());
    }
}
