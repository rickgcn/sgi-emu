use se_core::bus::{BusFault, PhysicalBus};

use super::{
    R3000Config, StepError,
    cache::{CacheBank, Caches},
    cp0::{Cp0, Cp0FunctionalState, Exception, TlbFaultKind},
    cp1::Cp1,
    mmu::{AccessType, Cacheability, Mmu, ProbeResult, Translation, TranslationFault},
};

const RESET_PC: u32 = 0xbfc0_0000;
const TLB_PROBE_FAILURE: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DelaySlot {
    pub(super) origin_pc: u32,
    pub(super) resume_pc: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PendingGprWrite {
    pub(super) index: usize,
    pub(super) value: u32,
    pub(super) load_merge_bypass: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingCp0Write {
    Register { index: usize, value: u32 },
    TlbRead { entry_hi: u32, entry_lo: u32 },
    TlbProbe { index: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingCp1Write {
    General { index: usize, value: u32 },
    Control { index: usize, value: u32 },
    Condition { value: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InstructionEffect {
    DelayedGprWrite {
        index: usize,
        value: u32,
        load_merge_bypass: bool,
    },
    DelayedCp0Write {
        index: usize,
        value: u32,
    },
    DelayedCp1GeneralWrite {
        index: usize,
        value: u32,
    },
    DelayedCp1ControlWrite {
        index: usize,
        value: u32,
    },
    DelayedCp1ConditionWrite {
        value: bool,
    },
    DelayedTlbRead {
        entry_hi: u32,
        entry_lo: u32,
    },
    DelayedTlbProbe {
        index: u32,
    },
    TlbWrite {
        index: usize,
        entry_hi: u32,
        entry_lo: u32,
    },
    RestoreStatus {
        value: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranslationError {
    Exception(Exception),
    TlbShutdown,
}

#[derive(Clone, Copy)]
enum LoadKind {
    Instruction,
    Data,
}

pub(super) struct State {
    gpr: [u32; 32],
    hi: u32,
    lo: u32,
    pc: u32,
    delay_slot: Option<DelaySlot>,
    cp0: Cp0,
    cp1: Cp1,
    mmu: Mmu,
    caches: Caches,
    pending_gpr_write: Option<PendingGprWrite>,
    pending_cp0_write: Option<PendingCp0Write>,
    pending_cp1_write: Option<PendingCp1Write>,
}

impl State {
    pub(super) fn new(config: R3000Config) -> Self {
        Self {
            gpr: [0; 32],
            hi: 0,
            lo: 0,
            pc: RESET_PC,
            delay_slot: None,
            cp0: Cp0::new(),
            cp1: Cp1::new(config.floating_point_backend()),
            mmu: Mmu::new(),
            caches: Caches::new(config),
            pending_gpr_write: None,
            pending_cp0_write: None,
            pending_cp1_write: None,
        }
    }

    pub(super) fn reset(&mut self) {
        let interrupted_pc = self.pc;
        self.cp0.reset(interrupted_pc);
        self.mmu.reset();
        self.gpr[0] = 0;
        self.pc = RESET_PC;
        self.delay_slot = None;
        self.pending_gpr_write = None;
        self.pending_cp0_write = None;
        self.pending_cp1_write = None;
    }

    pub(super) fn pc(&self) -> u32 {
        self.pc
    }

    pub(super) fn debug_delay_slot(&self) -> Option<DelaySlot> {
        self.delay_slot
    }

    pub(super) fn debug_pending_gpr_write(&self) -> Option<PendingGprWrite> {
        self.pending_gpr_write
    }

    pub(super) fn debug_pending_cp0_write(&self) -> Option<PendingCp0Write> {
        self.pending_cp0_write
    }

    pub(super) fn debug_pending_cp1_write(&self) -> Option<PendingCp1Write> {
        self.pending_cp1_write
    }

    pub(super) fn debug_cp0_functional_state(
        &self,
    ) -> (Cp0FunctionalState, Option<Cp0FunctionalState>) {
        self.cp0.debug_functional_state()
    }

    pub(super) fn debug_tlb_entries(&self, instruction: bool) -> [(u32, u32); 64] {
        self.mmu.debug_entries(instruction)
    }

    pub(super) fn debug_cache_entries(
        &self,
        bank: CacheBank,
    ) -> (usize, Vec<(u32, [u8; 4], bool)>) {
        self.caches.debug_entries(bank)
    }

    pub(super) fn debug_translate_address(
        &self,
        virtual_address: u32,
        access: AccessType,
    ) -> Result<Translation, TranslationFault> {
        self.mmu.translate(
            virtual_address,
            self.cp0.current_asid(),
            self.cp0.is_kernel_mode(),
            access,
        )
    }

    pub(super) fn read_gpr(&self, index: usize) -> u32 {
        if index == 0 { 0 } else { self.gpr[index] }
    }

    pub(super) fn read_gpr_for_load_merge(&self, index: usize) -> u32 {
        if index == 0 {
            return 0;
        }

        match self.pending_gpr_write {
            Some(write) if write.index == index && write.load_merge_bypass => write.value,
            _ => self.read_gpr(index),
        }
    }

    pub(super) fn write_gpr(&mut self, index: usize, value: u32) {
        self.commit_pending_gpr_write();
        self.write_gpr_direct(index, value);
    }

    fn write_gpr_direct(&mut self, index: usize, value: u32) {
        if index != 0 {
            self.gpr[index] = value;
        }
    }

    pub(super) fn read_hi(&self) -> u32 {
        self.hi
    }

    pub(super) fn write_hi(&mut self, value: u32) {
        self.hi = value;
    }

    pub(super) fn read_lo(&self) -> u32 {
        self.lo
    }

    pub(super) fn write_lo(&mut self, value: u32) {
        self.lo = value;
    }

    pub(super) fn read_cp0(&self, index: usize) -> u32 {
        self.cp0.read_register(index)
    }

    pub(super) fn cp0_status(&self) -> u32 {
        self.cp0.status()
    }

    pub(super) fn cp0_usable(&self) -> bool {
        self.cp0.is_usable()
    }

    pub(super) fn coprocessor_usable(&self, unit: usize) -> bool {
        self.cp0.coprocessor_usable(unit)
    }

    pub(super) fn read_cp1_general(&self, index: usize) -> u32 {
        self.cp1.read_general_register(index)
    }

    pub(super) fn cp1(&self) -> &Cp1 {
        &self.cp1
    }

    pub(super) fn cp1_mut(&mut self) -> &mut Cp1 {
        &mut self.cp1
    }

    pub(super) fn read_cp1_control(&self, index: usize) -> u32 {
        self.cp1.read_control_register(index)
    }

    pub(super) fn cp1_condition(&self) -> bool {
        self.cp1.condition()
    }

    pub(super) fn cp1_interrupt_asserted(&self) -> bool {
        self.cp1.interrupt_asserted()
    }

    pub(super) fn set_hardware_interrupt_lines(&mut self, lines: u8) {
        self.cp0.set_hardware_interrupt_lines(lines);
    }

    pub(super) fn interrupt_requested(&self) -> bool {
        self.cp0.interrupt_requested()
    }

    pub(super) fn is_tlb_shutdown(&self) -> bool {
        self.cp0.is_tlb_shutdown()
    }

    pub(super) fn translate_address(
        &mut self,
        virtual_address: u32,
        access: AccessType,
    ) -> Result<Translation, TranslationError> {
        let result = self.mmu.translate(
            virtual_address,
            self.cp0.current_asid(),
            self.cp0.is_kernel_mode(),
            access,
        );

        match result {
            Ok(translation) => Ok(translation),
            Err(TranslationFault::Shutdown) => {
                self.cp0.enter_tlb_shutdown();
                Err(TranslationError::TlbShutdown)
            }
            Err(fault) => Err(TranslationError::Exception(Self::translation_exception(
                virtual_address,
                access,
                fault,
            ))),
        }
    }

    pub(super) fn read_instruction(
        &mut self,
        translation: Translation,
        bus: &mut dyn PhysicalBus,
    ) -> Result<[u8; 4], BusFault> {
        validate_memory_access(translation, 4);

        let mut data = [0; 4];
        match translation.cacheability {
            Cacheability::Cached => {
                let bank = self.cache_bank(LoadKind::Instruction);
                self.caches
                    .read(bank, translation.address, &mut data, bus)?;
            }
            Cacheability::Uncached => bus.read(translation.address, &mut data)?,
        }

        Ok(data)
    }

    pub(super) fn load_data(
        &mut self,
        translation: Translation,
        data: &mut [u8],
        bus: &mut dyn PhysicalBus,
    ) -> Result<(), BusFault> {
        validate_memory_access(translation, data.len());

        if self.cp0.is_cache_isolated() {
            let bank = self.cache_bank(LoadKind::Data);
            let miss = self.caches.read_isolated(bank, translation.address, data);
            self.cp0.set_cache_miss(miss);
            return Ok(());
        }

        match translation.cacheability {
            Cacheability::Cached => {
                let bank = self.cache_bank(LoadKind::Data);
                self.caches.read(bank, translation.address, data, bus)
            }
            Cacheability::Uncached => {
                let mut staged = [0; 4];
                bus.read(translation.address, &mut staged[..data.len()])?;
                commit_load_data(data, staged);
                Ok(())
            }
        }
    }

    pub(super) fn store_memory(
        &mut self,
        translation: Translation,
        data: &[u8],
        bus: &mut dyn PhysicalBus,
    ) -> Result<(), BusFault> {
        validate_memory_access(translation, data.len());

        if self.cp0.is_cache_isolated() {
            let bank = self.cache_bank(LoadKind::Data);
            self.caches.write_isolated(bank, translation.address, data);
            return Ok(());
        }

        match translation.cacheability {
            Cacheability::Cached => {
                let bank = self.cache_bank(LoadKind::Data);
                self.caches.write(bank, translation.address, data, bus)
            }
            Cacheability::Uncached => bus.write(translation.address, data),
        }
    }

    fn cache_bank(&self, kind: LoadKind) -> CacheBank {
        match (kind, self.cp0.caches_swapped()) {
            (LoadKind::Instruction, false) | (LoadKind::Data, true) => CacheBank::Instruction,
            (LoadKind::Instruction, true) | (LoadKind::Data, false) => CacheBank::Data,
        }
    }

    pub(super) fn tlbr_effect(&self) -> InstructionEffect {
        let (entry_hi, entry_lo) = self.mmu.read_indexed(self.cp0.tlb_index());
        InstructionEffect::DelayedTlbRead { entry_hi, entry_lo }
    }

    pub(super) fn tlbwi_effect(&self) -> InstructionEffect {
        let (entry_hi, entry_lo) = self.cp0.tlb_staging();
        InstructionEffect::TlbWrite {
            index: self.cp0.tlb_index(),
            entry_hi,
            entry_lo,
        }
    }

    pub(super) fn tlbwr_effect(&self) -> InstructionEffect {
        let (entry_hi, entry_lo) = self.cp0.tlb_staging();
        InstructionEffect::TlbWrite {
            index: self.cp0.random_tlb_index(),
            entry_hi,
            entry_lo,
        }
    }

    pub(super) fn tlbp_effect(&mut self) -> Result<InstructionEffect, StepError> {
        let (entry_hi, _) = self.cp0.tlb_staging();
        let index = match self.mmu.probe(entry_hi) {
            ProbeResult::Miss => TLB_PROBE_FAILURE,
            ProbeResult::Match(index) => (index as u32) << 8,
            ProbeResult::Shutdown => {
                self.cp0.enter_tlb_shutdown();
                return Err(StepError::TlbShutdown);
            }
        };

        Ok(InstructionEffect::DelayedTlbProbe { index })
    }

    pub(super) fn complete_instruction(
        &mut self,
        delayed_resume_pc: Option<u32>,
        effect: Option<InstructionEffect>,
    ) {
        self.cp0.commit_pending_functional();
        self.commit_pending_gpr_write();
        self.commit_pending_cp0_write();
        self.commit_pending_cp1_write();

        let tlb_write = match effect {
            Some(InstructionEffect::DelayedGprWrite {
                index,
                value,
                load_merge_bypass,
            }) => {
                self.pending_gpr_write = Some(PendingGprWrite {
                    index,
                    value,
                    load_merge_bypass,
                });
                None
            }
            Some(InstructionEffect::DelayedCp0Write { index, value }) => {
                self.pending_cp0_write = Some(PendingCp0Write::Register { index, value });
                None
            }
            Some(InstructionEffect::DelayedCp1GeneralWrite { index, value }) => {
                self.pending_cp1_write = Some(PendingCp1Write::General { index, value });
                None
            }
            Some(InstructionEffect::DelayedCp1ControlWrite { index, value }) => {
                self.pending_cp1_write = Some(PendingCp1Write::Control { index, value });
                None
            }
            Some(InstructionEffect::DelayedCp1ConditionWrite { value }) => {
                self.pending_cp1_write = Some(PendingCp1Write::Condition { value });
                None
            }
            Some(InstructionEffect::DelayedTlbRead { entry_hi, entry_lo }) => {
                self.pending_cp0_write = Some(PendingCp0Write::TlbRead { entry_hi, entry_lo });
                None
            }
            Some(InstructionEffect::DelayedTlbProbe { index }) => {
                self.pending_cp0_write = Some(PendingCp0Write::TlbProbe { index });
                None
            }
            Some(InstructionEffect::TlbWrite {
                index,
                entry_hi,
                entry_lo,
            }) => Some((index, entry_hi, entry_lo)),
            Some(InstructionEffect::RestoreStatus { value }) => {
                self.cp0.restore_status(value);
                None
            }
            None => None,
        };

        match tlb_write {
            Some((index, entry_hi, entry_lo)) => {
                self.mmu.complete_write(index, entry_hi, entry_lo);
            }
            None => self.mmu.advance_instruction_view(),
        }

        let origin_pc = self.pc;

        self.pc = match self.delay_slot.take() {
            Some(delay_slot) => delay_slot.resume_pc,
            None => self.pc.wrapping_add(4),
        };

        self.delay_slot = delayed_resume_pc.map(|resume_pc| DelaySlot {
            origin_pc,
            resume_pc,
        });
        self.cp0.advance_random();
    }

    pub(super) fn take_exception(&mut self, exception: Exception) {
        self.cp0.commit_pending_functional();
        self.commit_pending_gpr_write();
        self.commit_pending_cp0_write();
        self.commit_pending_cp1_write();

        let (epc, in_delay_slot) = match self.delay_slot.take() {
            Some(delay_slot) => (delay_slot.origin_pc, true),
            None => (self.pc, false),
        };

        self.pc = self.cp0.take_exception(exception, epc, in_delay_slot);
        self.mmu.advance_instruction_view();
        self.cp0.advance_random();
    }

    fn commit_pending_gpr_write(&mut self) {
        if let Some(write) = self.pending_gpr_write.take() {
            self.write_gpr_direct(write.index, write.value);
        }
    }

    fn commit_pending_cp0_write(&mut self) {
        if let Some(write) = self.pending_cp0_write.take() {
            match write {
                PendingCp0Write::Register { index, value } => {
                    self.cp0.write_register(index, value);
                }
                PendingCp0Write::TlbRead { entry_hi, entry_lo } => {
                    self.cp0.write_tlb_read_result(entry_hi, entry_lo)
                }
                PendingCp0Write::TlbProbe { index } => {
                    self.cp0.write_tlb_probe_result(index);
                }
            }
        }
    }

    pub(super) fn commit_pending_cp1_write(&mut self) {
        if let Some(write) = self.pending_cp1_write.take() {
            match write {
                PendingCp1Write::General { index, value } => {
                    self.cp1.write_general_register(index, value);
                }
                PendingCp1Write::Control { index, value } => {
                    self.cp1.write_control_register(index, value);
                }
                PendingCp1Write::Condition { value } => {
                    self.cp1.write_condition(value);
                }
            }
        }
    }

    fn translation_exception(
        virtual_address: u32,
        access: AccessType,
        fault: TranslationFault,
    ) -> Exception {
        match (access, fault) {
            (AccessType::Instruction, TranslationFault::AddressError) => {
                Exception::InstructionAddressError {
                    address: virtual_address,
                }
            }
            (AccessType::Load, TranslationFault::AddressError) => Exception::LoadAddressError {
                address: virtual_address,
            },
            (AccessType::Store, TranslationFault::AddressError) => Exception::StoreAddressError {
                address: virtual_address,
            },
            (AccessType::Instruction | AccessType::Load, TranslationFault::Miss) => {
                Exception::TlbLoad {
                    address: virtual_address,
                    fault: TlbFaultKind::Miss,
                }
            }
            (AccessType::Instruction | AccessType::Load, TranslationFault::Invalid) => {
                Exception::TlbLoad {
                    address: virtual_address,
                    fault: TlbFaultKind::Invalid,
                }
            }
            (AccessType::Store, TranslationFault::Miss) => Exception::TlbStore {
                address: virtual_address,
                fault: TlbFaultKind::Miss,
            },
            (AccessType::Store, TranslationFault::Invalid) => Exception::TlbStore {
                address: virtual_address,
                fault: TlbFaultKind::Invalid,
            },
            (AccessType::Store, TranslationFault::Modified) => Exception::TlbModified {
                address: virtual_address,
            },
            (AccessType::Instruction | AccessType::Load, TranslationFault::Modified) => {
                unreachable!("the MMU reports modified only for store translations")
            }
            (_, TranslationFault::Shutdown) => {
                unreachable!("TLB shutdown is handled before exception mapping")
            }
        }
    }
}

fn commit_load_data(data: &mut [u8], staged: [u8; 4]) {
    match data {
        [byte0] => {
            *byte0 = staged[0];
        }
        [byte0, byte1] => {
            *byte0 = staged[0];
            *byte1 = staged[1];
        }
        [byte0, byte1, byte2] => {
            *byte0 = staged[0];
            *byte1 = staged[1];
            *byte2 = staged[2];
        }
        [byte0, byte1, byte2, byte3] => {
            *byte0 = staged[0];
            *byte1 = staged[1];
            *byte2 = staged[2];
            *byte3 = staged[3];
        }
        _ => unreachable!("R3000 memory transactions contain one through four bytes"),
    }
}

fn validate_memory_access(translation: Translation, length: usize) {
    assert!((1..=4).contains(&length));
    let offset = (translation.address.get() & 3) as usize;
    assert!(offset + length <= 4);
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

    use super::{
        AccessType, Cacheability, Cp0, DelaySlot, Exception, InstructionEffect, PendingCp0Write,
        PendingCp1Write, PendingGprWrite, RESET_PC, State, StepError, TlbFaultKind, Translation,
        TranslationError, TranslationFault,
    };

    const ENTRY_LO_DIRTY: u32 = 1 << 10;
    const ENTRY_LO_VALID: u32 = 1 << 9;
    const STATUS_BEV: u32 = 1 << 22;
    const STATUS_TS: u32 = 1 << 21;
    const STATUS_CM: u32 = 1 << 19;
    const STATUS_SWC: u32 = 1 << 17;
    const STATUS_ISC: u32 = 1 << 16;
    const STATUS_KUC: u32 = 1 << 1;
    const STATUS_IEC: u32 = 1;

    struct TestBus {
        read_data: [u8; 4],
        reads: Vec<(PhysAddr, usize)>,
        writes: Vec<(PhysAddr, Vec<u8>)>,
        read_fault: Option<BusFault>,
        write_fault: Option<BusFault>,
    }

    impl TestBus {
        fn new(read_data: [u8; 4]) -> Self {
            Self {
                read_data,
                reads: Vec::new(),
                writes: Vec::new(),
                read_fault: None,
                write_fault: None,
            }
        }
    }

    impl PhysicalBus for TestBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
            self.reads.push((address, data.len()));
            if let Some(fault) = self.read_fault {
                return Err(fault);
            }
            let source = self
                .read_data
                .get(..data.len())
                .ok_or(BusFault::UnsupportedAccess)?;
            data.copy_from_slice(source);
            Ok(())
        }

        fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
            if let Some(fault) = self.write_fault {
                return Err(fault);
            }

            self.writes.push((address, data.to_vec()));
            Ok(())
        }
    }

    fn translation(address: u64, cacheability: Cacheability) -> Translation {
        Translation {
            address: PhysAddr::new(address),
            cacheability,
        }
    }

    #[test]
    fn new_initializes_deterministic_state() {
        let state = State::new(crate::mips1::r3000::TEST_CONFIG);

        assert_eq!(state.gpr, [0; 32]);
        assert_eq!(state.hi, 0);
        assert_eq!(state.lo, 0);
        assert_eq!(state.pc, RESET_PC);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.cp0, Cp0::new());
        assert_eq!(state.pending_gpr_write, None);
        assert_eq!(state.pending_cp0_write, None);
        assert_eq!(state.pending_cp1_write, None);
    }

    #[test]
    fn reset_restores_defined_state_only() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        for (index, register) in state.gpr.iter_mut().enumerate() {
            *register = index as u32 + 1;
        }
        state.write_hi(0x1234_5678);
        state.write_lo(0x89ab_cdef);
        state.pc = 0;
        state.delay_slot = Some(DelaySlot {
            origin_pc: 0xffff_fff8,
            resume_pc: 0x1234_5678,
        });
        state.pending_gpr_write = Some(PendingGprWrite {
            index: 1,
            value: 0xaaaa_aaaa,
            load_merge_bypass: false,
        });
        state.pending_cp0_write = Some(PendingCp0Write::Register {
            index: 14,
            value: 0xbbbb_bbbb,
        });
        state.cp1.write_general_register(5, 0x1357_9bdf);
        state.cp1.write_control_register(30, 0x2468_ace0);
        state.cp1.write_control_register(31, 1 << 17);
        state.pending_cp1_write = Some(PendingCp1Write::General {
            index: 5,
            value: 0xcccc_cccc,
        });
        let preserved_gpr = state.gpr;
        let preserved_hi = state.read_hi();
        let preserved_lo = state.read_lo();
        let preserved_cp1_general = state.read_cp1_general(5);
        let preserved_cp1_eir = state.read_cp1_control(30);
        let preserved_cp1_csr = state.read_cp1_control(31);
        let mut expected_cp0 = Cp0::new();
        expected_cp0.reset(state.pc);

        state.reset();

        assert_eq!(state.gpr[0], 0);
        assert_eq!(state.gpr[1..], preserved_gpr[1..]);
        assert_eq!(state.read_hi(), preserved_hi);
        assert_eq!(state.read_lo(), preserved_lo);
        assert_eq!(state.pc, RESET_PC);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.cp0, expected_cp0);
        assert_eq!(state.pending_gpr_write, None);
        assert_eq!(state.pending_cp0_write, None);
        assert_eq!(state.pending_cp1_write, None);
        assert_eq!(state.read_cp1_general(5), preserved_cp1_general);
        assert_eq!(state.read_cp1_control(30), preserved_cp1_eir);
        assert_eq!(state.read_cp1_control(31), preserved_cp1_csr);
        assert!(state.cp1_interrupt_asserted());
    }

    #[test]
    fn general_register_access_preserves_register_zero() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);

        state.write_gpr(1, 0x1234_5678);
        state.write_gpr(31, 0x89ab_cdef);
        state.write_gpr(0, u32::MAX);

        assert_eq!(state.read_gpr(0), 0);
        assert_eq!(state.read_gpr(1), 0x1234_5678);
        assert_eq!(state.read_gpr(31), 0x89ab_cdef);
        assert_eq!(state.gpr[0], 0);
    }

    #[test]
    fn load_merge_read_bypasses_only_a_matching_memory_load() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.write_gpr(1, 0x1111_1111);
        state.write_gpr(2, 0x2222_2222);

        assert_eq!(state.read_gpr_for_load_merge(1), 0x1111_1111);

        let load = PendingGprWrite {
            index: 1,
            value: 0xaaaa_aaaa,
            load_merge_bypass: true,
        };
        state.pending_gpr_write = Some(load);

        assert_eq!(state.read_gpr_for_load_merge(1), 0xaaaa_aaaa);
        assert_eq!(state.read_gpr_for_load_merge(2), 0x2222_2222);
        assert_eq!(state.read_gpr_for_load_merge(0), 0);
        assert_eq!(state.pending_gpr_write, Some(load));

        let transfer = PendingGprWrite {
            index: 1,
            value: 0xbbbb_bbbb,
            load_merge_bypass: false,
        };
        state.pending_gpr_write = Some(transfer);

        assert_eq!(state.read_gpr_for_load_merge(1), 0x1111_1111);
        assert_eq!(state.pending_gpr_write, Some(transfer));
    }

    #[test]
    fn hi_and_lo_accessors_are_independent() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);

        state.write_hi(0x1234_5678);
        assert_eq!(state.read_hi(), 0x1234_5678);
        assert_eq!(state.read_lo(), 0);

        state.write_lo(0x89ab_cdef);
        assert_eq!(state.read_hi(), 0x1234_5678);
        assert_eq!(state.read_lo(), 0x89ab_cdef);
    }

    #[test]
    fn sequential_completion_advances_with_wrapping_arithmetic() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);

        assert_eq!(state.pc(), RESET_PC);
        state.complete_instruction(None, None);
        assert_eq!(state.pc(), RESET_PC + 4);

        state.pc = 0xffff_fffc;
        state.complete_instruction(None, None);
        assert_eq!(state.pc(), 0);
    }

    #[test]
    fn control_flow_completion_enters_and_leaves_delay_slot() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let resume_pc = 0xbfc0_0040;

        state.complete_instruction(Some(resume_pc), None);

        assert_eq!(state.pc(), RESET_PC + 4);
        assert_eq!(
            state.delay_slot,
            Some(DelaySlot {
                origin_pc: RESET_PC,
                resume_pc,
            })
        );

        state.complete_instruction(None, None);

        assert_eq!(state.pc(), resume_pc);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn not_taken_branch_still_records_delay_slot_origin() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let fallthrough = RESET_PC + 8;

        state.complete_instruction(Some(fallthrough), None);

        assert_eq!(
            state.delay_slot,
            Some(DelaySlot {
                origin_pc: RESET_PC,
                resume_pc: fallthrough,
            })
        );

        state.complete_instruction(None, None);

        assert_eq!(state.pc(), fallthrough);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn delay_slot_resume_address_can_wrap_to_zero() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.pc = 0xffff_fffc;

        state.complete_instruction(Some(0), None);
        assert_eq!(state.pc(), 0);

        state.complete_instruction(None, None);
        assert_eq!(state.pc(), 0);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn exception_outside_delay_slot_uses_current_pc() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.pc = 0xbfc0_0040;
        state.write_gpr(1, 0x1234_5678);
        let mut expected_cp0 = Cp0::new();
        let expected_pc = expected_cp0.take_exception(Exception::Syscall, state.pc, false);
        expected_cp0.advance_random();

        state.take_exception(Exception::Syscall);

        assert_eq!(state.cp0, expected_cp0);
        assert_eq!(state.pc, expected_pc);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.read_gpr(1), 0x1234_5678);
    }

    #[test]
    fn exception_in_delay_slot_uses_origin_and_cancels_resume() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let origin_pc = state.pc;
        let resume_pc = 0xbfc0_0040;
        state.write_gpr(31, origin_pc.wrapping_add(8));
        state.complete_instruction(Some(resume_pc), None);
        let mut expected_cp0 = Cp0::new();
        let expected_pc = expected_cp0.take_exception(Exception::Overflow, origin_pc, true);
        expected_cp0.advance_random();
        expected_cp0.advance_random();

        state.take_exception(Exception::Overflow);

        assert_eq!(state.cp0, expected_cp0);
        assert_eq!(state.pc, expected_pc);
        assert_eq!(state.delay_slot, None);
        assert_eq!(state.read_gpr(31), origin_pc.wrapping_add(8));
    }

    #[test]
    fn interrupt_input_reuses_exception_boundaries_and_survives_reset() {
        const CAUSE_BD: u32 = 1 << 31;
        const CAUSE_HARDWARE_IP_MASK: u32 = 0x0000_fc00;
        const STATUS_IM2: u32 = 1 << 10;

        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.set_hardware_interrupt_lines(1);

        assert_eq!(state.read_cp0(13) & CAUSE_HARDWARE_IP_MASK, STATUS_IM2);
        assert!(!state.interrupt_requested());

        state
            .cp0
            .write_register(12, STATUS_BEV | STATUS_IM2 | STATUS_IEC);
        state.cp0.commit_pending_functional();
        assert!(state.interrupt_requested());

        let branch_pc = state.pc();
        state.complete_instruction(Some(0xbfc0_0040), None);
        state.take_exception(Exception::Interrupt);

        assert_eq!(state.read_cp0(14), branch_pc);
        assert_eq!(state.read_cp0(13) & CAUSE_BD, CAUSE_BD);
        assert_eq!((state.read_cp0(13) >> 2) & 0x1f, 0);
        assert_eq!(state.read_cp0(13) & CAUSE_HARDWARE_IP_MASK, STATUS_IM2);
        assert!(!state.interrupt_requested());

        state.reset();

        assert_eq!(state.read_cp0(13) & CAUSE_HARDWARE_IP_MASK, STATUS_IM2);
        assert!(!state.interrupt_requested());
    }

    #[test]
    fn delayed_gpr_write_becomes_visible_after_one_instruction() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.write_gpr(1, 0x1111_1111);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x2222_2222,
                load_merge_bypass: false,
            }),
        );

        assert_eq!(state.read_gpr(1), 0x1111_1111);
        let dependent_value = state.read_gpr(1);
        state.complete_instruction(None, None);

        assert_eq!(dependent_value, 0x1111_1111);
        assert_eq!(state.read_gpr(1), 0x2222_2222);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 0,
                value: u32::MAX,
                load_merge_bypass: false,
            }),
        );
        state.complete_instruction(None, None);
        assert_eq!(state.read_gpr(0), 0);
    }

    #[test]
    fn direct_gpr_write_overrides_a_pending_transfer() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.write_gpr(1, 0x1111_1111);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x2222_2222,
                load_merge_bypass: false,
            }),
        );

        let source = state.read_gpr(1);
        state.write_gpr(1, source.wrapping_add(1));
        state.complete_instruction(None, None);

        assert_eq!(source, 0x1111_1111);
        assert_eq!(state.read_gpr(1), 0x1111_1112);
        assert_eq!(state.pending_gpr_write, None);
    }

    #[test]
    fn consecutive_transfers_commit_in_order() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.write_gpr(1, 0x1111_1111);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x2222_2222,
                load_merge_bypass: false,
            }),
        );
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x3333_3333,
                load_merge_bypass: false,
            }),
        );

        assert_eq!(state.read_gpr(1), 0x2222_2222);
        state.complete_instruction(None, None);
        assert_eq!(state.read_gpr(1), 0x3333_3333);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 14,
                value: 0x4444_4444,
            }),
        );
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 14,
                value: 0x5555_5555,
            }),
        );

        assert_eq!(state.read_cp0(14), 0x4444_4444);
        state.complete_instruction(None, None);
        assert_eq!(state.read_cp0(14), 0x5555_5555);
    }

    #[test]
    fn cp1_writes_become_visible_after_one_instruction() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.cp1.write_general_register(5, 0x1111_1111);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1GeneralWrite {
                index: 5,
                value: 0x2222_2222,
            }),
        );
        assert_eq!(state.read_cp1_general(5), 0x1111_1111);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1ControlWrite {
                index: 31,
                value: (1 << 23) | (1 << 17),
            }),
        );
        assert_eq!(state.read_cp1_general(5), 0x2222_2222);
        assert!(!state.cp1_condition());
        assert!(!state.cp1_interrupt_asserted());

        state.complete_instruction(None, None);
        assert!(state.cp1_condition());
        assert!(state.cp1_interrupt_asserted());
    }

    #[test]
    fn cp1_condition_write_commits_on_success_and_guest_exception() {
        let mut successful = State::new(crate::mips1::r3000::TEST_CONFIG);
        successful.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1ConditionWrite { value: true }),
        );
        assert!(!successful.cp1_condition());
        successful.complete_instruction(None, None);
        assert!(successful.cp1_condition());

        let mut exceptional = State::new(crate::mips1::r3000::TEST_CONFIG);
        exceptional.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1ConditionWrite { value: true }),
        );
        assert!(!exceptional.cp1_condition());
        exceptional.take_exception(Exception::Syscall);
        assert!(exceptional.cp1_condition());
        assert_eq!(exceptional.pending_cp1_write, None);
    }

    #[test]
    fn exception_commits_an_older_cp1_write() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1GeneralWrite {
                index: 7,
                value: 0x1234_5678,
            }),
        );
        assert_eq!(state.read_cp1_general(7), 0);

        state.take_exception(Exception::Syscall);

        assert_eq!(state.read_cp1_general(7), 0x1234_5678);
        assert_eq!(state.pending_cp1_write, None);
    }

    #[test]
    fn exception_commits_old_transfers_before_hardware_state() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let exception_pc = state.pc().wrapping_add(8);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x1234_5678,
                load_merge_bypass: false,
            }),
        );
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 14,
                value: 0xdead_beef,
            }),
        );
        assert_eq!(state.pc(), exception_pc);

        state.take_exception(Exception::Syscall);

        assert_eq!(state.read_gpr(1), 0x1234_5678);
        assert_eq!(state.read_cp0(14), exception_pc);
        assert_eq!(state.pending_gpr_write, None);
        assert_eq!(state.pending_cp0_write, None);
    }

    #[test]
    fn transfer_delay_coexists_with_a_branch_delay_slot() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let resume_pc = 0xbfc0_0040;
        state.write_gpr(1, 0x1111_1111);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x2222_2222,
                load_merge_bypass: false,
            }),
        );
        let branch_source = state.read_gpr(1);
        state.complete_instruction(Some(resume_pc), None);

        assert_eq!(branch_source, 0x1111_1111);
        assert_eq!(state.read_gpr(1), 0x2222_2222);
        assert!(state.delay_slot.is_some());

        state.complete_instruction(None, None);

        assert_eq!(state.pc(), resume_pc);
        assert_eq!(state.delay_slot, None);
    }

    #[test]
    fn status_functional_state_uses_the_full_hazard_window() {
        const STATUS_BEV: u32 = 1 << 22;
        const STATUS_KUC: u32 = 1 << 1;
        const STATUS_CU0: u32 = 1 << 28;

        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let user_with_cu0 = STATUS_BEV | STATUS_KUC | STATUS_CU0;

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: user_with_cu0,
            }),
        );
        assert_eq!(state.read_cp0(12), STATUS_BEV);
        assert!(state.cp0_usable());

        state.complete_instruction(None, None);
        assert_eq!(state.read_cp0(12), user_with_cu0);
        assert!(!state.cp0_usable());

        state.complete_instruction(None, None);
        assert!(state.cp0_usable());
    }

    #[test]
    fn rfe_restore_overrides_pending_status_stack_only() {
        const STATUS_BEV: u32 = 1 << 22;
        const STATUS_CU0: u32 = 1 << 28;

        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: STATUS_BEV | STATUS_CU0 | 0x0c,
            }),
        );

        state.complete_instruction(
            None,
            Some(InstructionEffect::RestoreStatus { value: STATUS_BEV }),
        );

        assert_eq!(state.read_cp0(12), STATUS_BEV | STATUS_CU0);
    }

    #[test]
    fn reset_clears_functional_and_transfer_pending_state() {
        const STATUS_BEV: u32 = 1 << 22;
        const STATUS_KUC: u32 = 1 << 1;
        const STATUS_CU0: u32 = 1 << 28;

        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: STATUS_BEV | STATUS_KUC | STATUS_CU0,
            }),
        );
        state.complete_instruction(None, None);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: 1,
                value: 0x1234_5678,
                load_merge_bypass: false,
            }),
        );
        state.pending_cp1_write = Some(PendingCp1Write::Control {
            index: 31,
            value: u32::MAX,
        });

        state.reset();

        assert_eq!(state.pending_gpr_write, None);
        assert_eq!(state.pending_cp0_write, None);
        assert_eq!(state.pending_cp1_write, None);
        assert_eq!(state.read_cp0(12) & STATUS_KUC, 0);
        assert!(state.cp0_usable());
    }

    #[test]
    fn random_advances_for_normal_and_exception_completion() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);

        assert_eq!(state.read_cp0(1), 63 << 8);
        state.complete_instruction(None, None);
        assert_eq!(state.read_cp0(1), 62 << 8);
        state.take_exception(Exception::Syscall);
        assert_eq!(state.read_cp0(1), 61 << 8);
    }

    #[test]
    fn translation_faults_map_to_precise_exceptions() {
        let address = 0x1234_5678;
        let cases = [
            (
                AccessType::Instruction,
                TranslationFault::AddressError,
                Exception::InstructionAddressError { address },
            ),
            (
                AccessType::Load,
                TranslationFault::AddressError,
                Exception::LoadAddressError { address },
            ),
            (
                AccessType::Store,
                TranslationFault::AddressError,
                Exception::StoreAddressError { address },
            ),
            (
                AccessType::Instruction,
                TranslationFault::Miss,
                Exception::TlbLoad {
                    address,
                    fault: TlbFaultKind::Miss,
                },
            ),
            (
                AccessType::Load,
                TranslationFault::Invalid,
                Exception::TlbLoad {
                    address,
                    fault: TlbFaultKind::Invalid,
                },
            ),
            (
                AccessType::Store,
                TranslationFault::Miss,
                Exception::TlbStore {
                    address,
                    fault: TlbFaultKind::Miss,
                },
            ),
            (
                AccessType::Store,
                TranslationFault::Invalid,
                Exception::TlbStore {
                    address,
                    fault: TlbFaultKind::Invalid,
                },
            ),
            (
                AccessType::Store,
                TranslationFault::Modified,
                Exception::TlbModified { address },
            ),
        ];

        for (access, fault, expected) in cases {
            assert_eq!(
                State::translation_exception(address, access, fault),
                expected
            );
        }
    }

    #[test]
    fn translation_uses_current_cp0_asid_and_mode() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let virtual_address = 0x1234_5000;
        let asid = 0x15_u32;
        let entry_hi = virtual_address | (asid << 6);
        state.cp0.write_register(10, entry_hi);
        state
            .mmu
            .complete_write(5, entry_hi, 0x4321_0000 | ENTRY_LO_VALID | ENTRY_LO_DIRTY);

        assert_eq!(
            state.translate_address(virtual_address, AccessType::Load),
            Ok(translation(0x4321_0000, Cacheability::Cached))
        );

        state
            .cp0
            .write_register(10, virtual_address | ((asid ^ 1) << 6));
        assert_eq!(
            state.translate_address(virtual_address, AccessType::Load),
            Err(TranslationError::Exception(Exception::TlbLoad {
                address: virtual_address,
                fault: TlbFaultKind::Miss,
            }))
        );

        state.cp0.write_register(12, STATUS_BEV | STATUS_KUC);
        assert_eq!(
            state.translate_address(0x8000_0000, AccessType::Instruction),
            Err(TranslationError::Exception(
                Exception::InstructionAddressError {
                    address: 0x8000_0000,
                }
            ))
        );
    }

    #[test]
    fn translation_shutdown_changes_only_status_ts() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.mmu.complete_write(24, 0, 0);
        state.mmu.complete_write(25, 0, 0);
        state.pending_gpr_write = Some(PendingGprWrite {
            index: 1,
            value: 0x1111_1111,
            load_merge_bypass: false,
        });
        state.pending_cp0_write = Some(PendingCp0Write::Register {
            index: 14,
            value: 0x2222_2222,
        });
        let pc = state.pc;
        let random = state.read_cp0(1);
        let status = state.read_cp0(12);

        assert_eq!(
            state.translate_address(0, AccessType::Load),
            Err(TranslationError::TlbShutdown)
        );

        assert_eq!(state.pc, pc);
        assert_eq!(state.read_cp0(1), random);
        assert_eq!(state.read_cp0(12), status | STATUS_TS);
        assert_eq!(
            state.pending_gpr_write,
            Some(PendingGprWrite {
                index: 1,
                value: 0x1111_1111,
                load_merge_bypass: false,
            })
        );
        assert_eq!(
            state.pending_cp0_write,
            Some(PendingCp0Write::Register {
                index: 14,
                value: 0x2222_2222,
            })
        );
    }

    #[test]
    fn tlb_instruction_effects_capture_current_staging_and_indices() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.cp0.write_register(0, 7 << 8);
        state.cp0.write_register(10, 0x1234_5a80);
        state.cp0.write_register(2, 0x9876_5f00);
        state.mmu.complete_write(7, 0x4567_8b40, 0x3456_7e00);

        assert_eq!(
            state.tlbr_effect(),
            InstructionEffect::DelayedTlbRead {
                entry_hi: 0x4567_8b40,
                entry_lo: 0x3456_7e00,
            }
        );
        assert_eq!(
            state.tlbwi_effect(),
            InstructionEffect::TlbWrite {
                index: 7,
                entry_hi: 0x1234_5a80,
                entry_lo: 0x9876_5f00,
            }
        );
        assert_eq!(
            state.tlbwr_effect(),
            InstructionEffect::TlbWrite {
                index: 63,
                entry_hi: 0x1234_5a80,
                entry_lo: 0x9876_5f00,
            }
        );
    }

    #[test]
    fn tlb_probe_produces_delayed_index_results_and_shutdown() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let entry_hi = 0x2345_6a80;
        state.cp0.write_register(10, entry_hi);
        state.mmu.complete_write(23, entry_hi, 0);

        assert_eq!(
            state.tlbp_effect(),
            Ok(InstructionEffect::DelayedTlbProbe { index: 23 << 8 })
        );

        state.cp0.write_register(10, 0x3456_7a80);
        assert_eq!(
            state.tlbp_effect(),
            Ok(InstructionEffect::DelayedTlbProbe { index: 0x8000_0000 })
        );

        state.mmu.complete_write(24, entry_hi, 0);
        state.mmu.complete_write(25, entry_hi, 0);
        state.cp0.write_register(10, entry_hi);
        state.pending_cp0_write = Some(PendingCp0Write::Register {
            index: 14,
            value: 0x1234_5678,
        });
        let pc = state.pc;
        let random = state.read_cp0(1);

        assert_eq!(state.tlbp_effect(), Err(StepError::TlbShutdown));
        assert!(state.is_tlb_shutdown());
        assert_eq!(state.pc, pc);
        assert_eq!(state.read_cp0(1), random);
        assert_eq!(
            state.pending_cp0_write,
            Some(PendingCp0Write::Register {
                index: 14,
                value: 0x1234_5678,
            })
        );
    }

    #[test]
    fn tlb_read_and_probe_results_observe_cp0_delay() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.cp0.write_register(10, 0x1111_1a80);
        state.cp0.write_register(2, 0x2222_2f00);
        state.cp0.write_register(0, 4 << 8);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedTlbRead {
                entry_hi: 0x3333_3b40,
                entry_lo: 0x4444_4e00,
            }),
        );
        assert_eq!(
            state.tlbwi_effect(),
            InstructionEffect::TlbWrite {
                index: 4,
                entry_hi: 0x1111_1a80,
                entry_lo: 0x2222_2f00,
            }
        );

        state.complete_instruction(None, None);
        assert_eq!(state.read_cp0(10), 0x3333_3b40);
        assert_eq!(state.read_cp0(2), 0x4444_4e00);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedTlbProbe { index: 0x8000_0000 }),
        );
        assert_eq!(state.read_cp0(0), 4 << 8);
        state.complete_instruction(None, None);
        assert_eq!(state.read_cp0(0), 0x8000_0000);
    }

    #[test]
    fn tlb_operations_immediately_after_mtc0_read_old_cp0_values() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.cp0.write_register(0, 4 << 8);
        state.cp0.write_register(10, 0x1111_1a80);
        state.cp0.write_register(2, 0x2222_2f00);
        state.mmu.complete_write(4, 0x3333_3b40, 0x4444_4e00);
        state.mmu.complete_write(5, 0x5555_5c00, 0x6666_6d00);

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 0,
                value: 5 << 8,
            }),
        );
        let immediate_tlbr = state.tlbr_effect();
        assert_eq!(
            immediate_tlbr,
            InstructionEffect::DelayedTlbRead {
                entry_hi: 0x3333_3b40,
                entry_lo: 0x4444_4e00,
            }
        );
        state.complete_instruction(None, Some(immediate_tlbr));
        assert_eq!(
            state.tlbr_effect(),
            InstructionEffect::DelayedTlbRead {
                entry_hi: 0x5555_5c00,
                entry_lo: 0x6666_6d00,
            }
        );

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 10,
                value: 0x7777_7d40,
            }),
        );
        assert_eq!(
            state.tlbwi_effect(),
            InstructionEffect::TlbWrite {
                index: 5,
                entry_hi: 0x3333_3b40,
                entry_lo: 0x4444_4e00,
            }
        );
    }

    #[test]
    fn tlb_write_is_immediate_for_main_view_and_delayed_for_instructions() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let virtual_address = 0x4567_8000;
        let old_entry_lo = 0x1000_0000 | ENTRY_LO_VALID | ENTRY_LO_DIRTY;
        let new_entry_lo = 0x2000_0000 | ENTRY_LO_VALID | ENTRY_LO_DIRTY;
        state.mmu.complete_write(8, virtual_address, old_entry_lo);
        state.mmu.advance_instruction_view();
        state.mmu.advance_instruction_view();

        state.complete_instruction(
            None,
            Some(InstructionEffect::TlbWrite {
                index: 8,
                entry_hi: virtual_address,
                entry_lo: new_entry_lo,
            }),
        );

        assert_eq!(
            state.translate_address(virtual_address, AccessType::Load),
            Ok(translation(0x2000_0000, Cacheability::Cached))
        );
        assert_eq!(
            state.translate_address(virtual_address, AccessType::Instruction),
            Ok(translation(0x1000_0000, Cacheability::Cached))
        );

        state.complete_instruction(None, None);
        assert_eq!(
            state.translate_address(virtual_address, AccessType::Instruction),
            Ok(translation(0x1000_0000, Cacheability::Cached))
        );

        state.take_exception(Exception::Syscall);
        assert_eq!(
            state.translate_address(virtual_address, AccessType::Instruction),
            Ok(translation(0x2000_0000, Cacheability::Cached))
        );
    }

    #[test]
    fn tlb_exception_overrides_pending_tlbr_vpn_and_preserves_asid() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let fault_address = 0xf234_5678;
        let asid = 0x0000_0a80;

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedTlbRead {
                entry_hi: 0x1234_5000 | asid,
                entry_lo: 0x3456_7e00,
            }),
        );
        state.take_exception(Exception::TlbLoad {
            address: fault_address,
            fault: TlbFaultKind::Invalid,
        });

        assert_eq!(state.read_cp0(10), (fault_address & 0xffff_f000) | asid);
        assert_eq!(state.pending_cp0_write, None);
    }

    #[test]
    fn reset_preserves_main_tlb_and_clears_translation_pending_state() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let virtual_address = 0x5678_9000;
        let entry_lo = 0x3000_0000 | ENTRY_LO_VALID | ENTRY_LO_DIRTY;

        state.complete_instruction(
            None,
            Some(InstructionEffect::TlbWrite {
                index: 9,
                entry_hi: virtual_address,
                entry_lo,
            }),
        );
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedTlbProbe { index: 9 << 8 }),
        );
        state.cp0.enter_tlb_shutdown();

        state.reset();

        assert_eq!(state.mmu.read_indexed(9), (virtual_address, entry_lo));
        assert_eq!(
            state.translate_address(virtual_address, AccessType::Instruction),
            Ok(translation(0x3000_0000, Cacheability::Cached))
        );
        assert_eq!(state.pending_cp0_write, None);
        assert!(!state.is_tlb_shutdown());
    }

    #[test]
    fn memory_loads_route_cached_and_uncached_translations() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let mut bus = TestBus::new([1, 2, 3, 4]);
        let cached = translation(0x100, Cacheability::Cached);
        let uncached = translation(0x1100, Cacheability::Uncached);
        let mut data = [0; 4];

        state.cp0.set_cache_miss(true);
        state
            .load_data(cached, &mut data, &mut bus)
            .expect("cached load should refill");
        assert_eq!(data, [1, 2, 3, 4]);
        assert_eq!(bus.reads.len(), 1);
        assert_eq!(state.read_cp0(12) & STATUS_CM, STATUS_CM);

        bus.read_data = [5, 6, 7, 8];
        state
            .load_data(cached, &mut data, &mut bus)
            .expect("cached load should hit");
        assert_eq!(data, [1, 2, 3, 4]);
        assert_eq!(bus.reads.len(), 1);

        state
            .load_data(uncached, &mut data, &mut bus)
            .expect("uncached load should reach the bus");
        assert_eq!(data, [5, 6, 7, 8]);
        assert_eq!(bus.reads.len(), 2);
        assert_eq!(state.read_cp0(12) & STATUS_CM, STATUS_CM);

        bus.read_fault = Some(BusFault::Unmapped);
        data = [0xaa; 4];
        let pc = state.pc();
        let status = state.read_cp0(12);
        let random = state.read_cp0(1);
        assert_eq!(
            state.load_data(uncached, &mut data, &mut bus),
            Err(BusFault::Unmapped)
        );
        assert_eq!(data, [0xaa; 4]);
        assert_eq!(state.pc(), pc);
        assert_eq!(state.read_cp0(12), status);
        assert_eq!(state.read_cp0(1), random);

        bus.read_fault = None;
        state
            .load_data(cached, &mut data, &mut bus)
            .expect("uncached alias should not modify the resident cache entry");
        assert_eq!(data, [1, 2, 3, 4]);
        assert_eq!(bus.reads.len(), 3);
    }

    #[test]
    fn cache_controls_follow_the_cp0_write_hazard() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: STATUS_BEV | STATUS_ISC | STATUS_SWC,
            }),
        );

        assert!(!state.cp0.is_cache_isolated());
        assert!(!state.cp0.caches_swapped());

        state.complete_instruction(None, None);

        assert!(state.cp0.is_cache_isolated());
        assert!(state.cp0.caches_swapped());
    }

    #[test]
    fn isolation_and_swap_select_physical_cache_banks() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let address = translation(0x100, Cacheability::Cached);
        let mut bus = TestBus::new([9, 8, 7, 6]);

        state.cp0.write_register(12, STATUS_BEV | STATUS_ISC);
        state
            .store_memory(
                translation(0x100, Cacheability::Uncached),
                &[1, 2, 3, 4],
                &mut bus,
            )
            .expect("isolated data-cache write should succeed");
        state
            .cp0
            .write_register(12, STATUS_BEV | STATUS_ISC | STATUS_SWC);
        state
            .store_memory(address, &[5, 6, 7, 8], &mut bus)
            .expect("isolated instruction-cache write should succeed");
        assert!(bus.writes.is_empty());

        state.cp0.write_register(12, STATUS_BEV | STATUS_ISC);
        state.cp0.set_cache_miss(true);
        let mut data = state
            .read_instruction(address, &mut bus)
            .expect("unswapped instruction load should select the instruction cache");
        assert_eq!(data, [5, 6, 7, 8]);
        assert_eq!(state.read_cp0(12) & STATUS_CM, STATUS_CM);

        state
            .load_data(address, &mut data, &mut bus)
            .expect("isolated data-cache read should succeed");
        assert_eq!(data, [1, 2, 3, 4]);
        assert_eq!(state.read_cp0(12) & STATUS_CM, 0);

        let alias = translation(0x1100, Cacheability::Uncached);
        state
            .load_data(alias, &mut data, &mut bus)
            .expect("isolated miss should still return indexed data");
        assert_eq!(data, [1, 2, 3, 4]);
        assert_eq!(state.read_cp0(12) & STATUS_CM, STATUS_CM);

        state
            .cp0
            .write_register(12, STATUS_BEV | STATUS_ISC | STATUS_SWC);
        state
            .load_data(address, &mut data, &mut bus)
            .expect("swapped isolated read should succeed");
        assert_eq!(data, [5, 6, 7, 8]);
        assert_eq!(state.read_cp0(12) & STATUS_CM, 0);

        state.cp0.set_cache_miss(true);
        let instruction_address = translation(0x204, Cacheability::Cached);
        data = state
            .read_instruction(instruction_address, &mut bus)
            .expect("instruction load should ignore cache isolation");
        assert_eq!(data, [9, 8, 7, 6]);
        assert_eq!(bus.reads, vec![(PhysAddr::new(0x204), 4)]);
        assert_eq!(state.read_cp0(12) & STATUS_CM, STATUS_CM);

        state.cp0.write_register(12, STATUS_BEV | STATUS_ISC);
        state
            .load_data(instruction_address, &mut data, &mut bus)
            .expect("swapped instruction refill should reside in the data cache");
        assert_eq!(data, [9, 8, 7, 6]);
        assert_eq!(state.read_cp0(12) & STATUS_CM, 0);
        assert_eq!(bus.reads.len(), 1);
    }

    #[test]
    fn stores_route_through_cacheability_and_isolation() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let mut bus = TestBus::new([0; 4]);
        state.cp0.set_cache_miss(true);

        state
            .store_memory(
                translation(0x301, Cacheability::Uncached),
                &[1, 2, 3],
                &mut bus,
            )
            .expect("uncached partial store should reach the bus");
        state
            .store_memory(
                translation(0x400, Cacheability::Cached),
                &[4, 5, 6, 7],
                &mut bus,
            )
            .expect("cached full store should write through");

        assert_eq!(
            bus.writes,
            vec![
                (PhysAddr::new(0x301), vec![1, 2, 3]),
                (PhysAddr::new(0x400), vec![4, 5, 6, 7]),
            ]
        );
        assert_eq!(state.read_cp0(12) & STATUS_CM, STATUS_CM);

        bus.write_fault = Some(BusFault::Unmapped);
        let pc = state.pc();
        let status = state.read_cp0(12);
        let random = state.read_cp0(1);
        assert_eq!(
            state.store_memory(translation(0x401, Cacheability::Cached), &[9, 8], &mut bus,),
            Err(BusFault::Unmapped)
        );
        assert_eq!(state.pc(), pc);
        assert_eq!(state.read_cp0(12), status);
        assert_eq!(state.read_cp0(1), random);

        state.cp0.write_register(12, STATUS_BEV | STATUS_ISC);
        let mut data = [0; 4];
        state
            .load_data(
                translation(0x400, Cacheability::Uncached),
                &mut data,
                &mut bus,
            )
            .expect("isolated read should ignore uncached translation");
        assert_eq!(data, [4, 5, 6, 7]);
        assert_eq!(state.read_cp0(12) & STATUS_CM, 0);
    }

    #[test]
    fn uncached_store_does_not_modify_a_resident_cache_entry() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let mut bus = TestBus::new([0; 4]);
        state
            .store_memory(
                translation(0x300, Cacheability::Cached),
                &[1, 2, 3, 4],
                &mut bus,
            )
            .expect("cached store should establish a resident word");
        state
            .store_memory(
                translation(0x1301, Cacheability::Uncached),
                &[9, 8],
                &mut bus,
            )
            .expect("uncached alias should bypass the cache");

        state.cp0.write_register(12, STATUS_BEV | STATUS_ISC);
        let mut data = [0; 4];
        state
            .load_data(
                translation(0x300, Cacheability::Uncached),
                &mut data,
                &mut bus,
            )
            .expect("isolated read should observe the original resident word");

        assert_eq!(data, [1, 2, 3, 4]);
        assert_eq!(state.read_cp0(12) & STATUS_CM, 0);
        assert_eq!(
            bus.writes,
            vec![
                (PhysAddr::new(0x300), vec![1, 2, 3, 4]),
                (PhysAddr::new(0x1301), vec![9, 8]),
            ]
        );
    }

    #[test]
    fn reset_preserves_cache_contents() {
        let mut state = State::new(crate::mips1::r3000::TEST_CONFIG);
        let translation = translation(0x500, Cacheability::Cached);
        let mut bus = TestBus::new([1, 2, 3, 4]);
        let data = state
            .read_instruction(translation, &mut bus)
            .expect("initial fetch should refill");

        state.reset();
        bus.read_data = [5, 6, 7, 8];
        let reset_data = state
            .read_instruction(translation, &mut bus)
            .expect("reset cache entry should remain readable");

        assert_eq!(data, [1, 2, 3, 4]);
        assert_eq!(reset_data, [1, 2, 3, 4]);
        assert_eq!(bus.reads.len(), 1);
    }
}
