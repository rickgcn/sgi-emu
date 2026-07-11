use crate::cpu::execution::protocol::ExecutionAction;
use crate::cpu::mips4::cache::hierarchy::{MIPS4_FUNCTIONAL_CACHE_LINE_BYTES, Mips4CacheLine};
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::Mips4Cp0Register;

use super::super::target::Mips4ExecutionBoundary;
use super::{ConformanceMachine, assert_retired, i_type};

const CP0_INSTRUCTIONS: [u32; 10] = [
    cp0_transfer(0x00, 1, 9),
    cp0_transfer(0x01, 1, 14),
    cp0_transfer(0x04, 1, 9),
    cp0_transfer(0x05, 1, 14),
    cp0_co(0x01),
    cp0_co(0x02),
    cp0_co(0x06),
    cp0_co(0x08),
    cp0_co(0x18),
    cp0_co(0x20),
];

const CACHE_OPERATIONS: [(u8, u8); 17] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (0, 4),
    (0, 5),
    (0, 6),
    (1, 0),
    (1, 1),
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
    (3, 0),
    (3, 1),
    (3, 2),
    (3, 5),
];

#[test]
fn every_r5000_cp0_instruction_reaches_an_architectural_boundary() {
    for bits in CP0_INSTRUCTIONS {
        let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
        machine.write_gpr(1, 0);
        let boundary = machine.execute_with_zero_bus(bits);
        assert!(
            matches!(
                boundary,
                Mips4ExecutionBoundary::Retired { .. } | Mips4ExecutionBoundary::Exception { .. }
            ),
            "CP0 instruction {bits:#010x}"
        );
    }
}

#[test]
fn cp0_word_and_doubleword_transfers_preserve_documented_widths() {
    let mut word = ConformanceMachine::new(Mips4Endianness::Big);
    word.write_gpr(1, 0xffff_ffff_89ab_cdef);
    let mtc0 = cp0_transfer(0x04, 1, 9);
    assert_retired(word.execute(mtc0), mtc0);
    assert_eq!(word.state().cp0().count().bits(), 0x89ab_cdef);
    word.write_gpr(2, 0);
    let mfc0 = cp0_transfer(0x00, 2, 9);
    assert!(matches!(
        word.execute(mfc0),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert_eq!(word.read_gpr(2), 0xffff_ffff_89ab_cdef);

    let mut doubleword = ConformanceMachine::new(Mips4Endianness::Big);
    doubleword.write_gpr(1, 0x0123_4567_89ab_cdef);
    let dmtc0 = cp0_transfer(0x05, 1, 14);
    assert_retired(doubleword.execute(dmtc0), dmtc0);
    assert_eq!(doubleword.state().cp0().epc().bits(), 0x0123_4567_89ab_cdef);
    let dmfc0 = cp0_transfer(0x01, 2, 14);
    assert!(matches!(
        doubleword.execute(dmfc0),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert_eq!(doubleword.read_gpr(2), 0x0123_4567_89ab_cdef);
}

#[test]
fn tlb_write_read_probe_and_random_paths_use_cp0_selected_entries() {
    let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::Index, 3)
        .unwrap();
    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::EntryHi, 0x0000_0000_0040_005a)
        .unwrap();
    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::EntryLo0, 0x0000_0000_0004_001f)
        .unwrap();
    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::EntryLo1, 0x0000_0000_0008_001f)
        .unwrap();
    let tlbwi = cp0_co(0x02);
    assert_retired(machine.execute(tlbwi), tlbwi);
    assert!(machine.state().tlb_entries()[3].even_page().valid());

    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::EntryHi, 0)
        .unwrap();
    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::EntryLo0, 0)
        .unwrap();
    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::EntryLo1, 0)
        .unwrap();
    let tlbr = cp0_co(0x01);
    assert!(matches!(
        machine.execute(tlbr),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert_eq!(
        machine.state().cp0().entry_hi().address_space_identifier(),
        0x5a
    );

    let tlbp = cp0_co(0x08);
    assert!(matches!(
        machine.execute(tlbp),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert_eq!(machine.state().cp0().index().index(), 3);

    let tlbwr = cp0_co(0x06);
    assert!(matches!(
        machine.execute(tlbwr),
        Mips4ExecutionBoundary::Retired { .. }
    ));
}

#[test]
fn eret_selects_epc_or_error_epc_and_wait_enters_standby() {
    let mut ordinary = ConformanceMachine::new(Mips4Endianness::Big);
    ordinary
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::Status, (1 << 22) | (1 << 1))
        .unwrap();
    ordinary
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::Epc, 0xffff_ffff_8000_4000)
        .unwrap();
    let eret = cp0_co(0x18);
    assert_retired(ordinary.execute(eret), eret);
    assert_eq!(ordinary.state().pc(), 0xffff_ffff_8000_4000);
    assert!(!ordinary.state().cp0().status().exception_level());

    let mut error = ConformanceMachine::new(Mips4Endianness::Big);
    error
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::ErrorEpc, 0xffff_ffff_8000_5000)
        .unwrap();
    assert_retired(error.execute(eret), eret);
    assert_eq!(error.state().pc(), 0xffff_ffff_8000_5000);
    assert!(!error.state().cp0().status().error_level());

    let mut wait = ConformanceMachine::new(Mips4Endianness::Big);
    let wait_bits = cp0_co(0x20);
    assert_retired(wait.execute(wait_bits), wait_bits);
    assert_eq!(wait.executor.poll().unwrap(), ExecutionAction::Idle);
}

#[test]
fn all_documented_cache_operation_and_selector_pairs_execute() {
    let mut seen = [[false; 7]; 4];
    for (selector, operation) in CACHE_OPERATIONS {
        assert!(!seen[selector as usize][operation as usize]);
        seen[selector as usize][operation as usize] = true;

        let mut machine = ConformanceMachine::with_secondary(Mips4Endianness::Big, true);
        machine.write_gpr(1, 0xffff_ffff_8000_1000);
        let op = (operation << 2) | selector;
        let bits = i_type(0x2f, 1, op, 0);
        let boundary = machine.execute_with_zero_bus(bits);
        assert!(
            matches!(boundary, Mips4ExecutionBoundary::Retired { instruction, .. } if instruction == bits)
        );
    }
    assert_eq!(CACHE_OPERATIONS.len(), 17);
}

#[test]
fn cache_instructions_change_primary_and_secondary_state() {
    let mut primary = ConformanceMachine::new(Mips4Endianness::Big);
    select_writeback_kseg0(&mut primary);
    let address = 0xffff_ffff_8000_1000;
    primary.write_gpr(1, address);
    let lw = i_type(0x23, 1, 2, 0);
    assert_retired(primary.execute_with_zero_bus(lw), lw);
    assert!(primary.state().cache.data_lookup(address, 0x1000).is_some());

    let hit_invalidate = i_type(0x2f, 1, (4 << 2) | 1, 0);
    assert!(matches!(
        primary.execute(hit_invalidate),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert!(primary.state().cache.data_lookup(address, 0x1000).is_none());

    let create_dirty = i_type(0x2f, 1, (3 << 2) | 1, 0);
    assert!(matches!(
        primary.execute(create_dirty),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert!(
        primary
            .state()
            .cache
            .data_lookup(address, 0x1000)
            .unwrap()
            .dirty
    );

    let hit_writeback = i_type(0x2f, 1, (6 << 2) | 1, 0);
    assert!(matches!(
        primary.execute_with_zero_bus(hit_writeback),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert!(
        !primary
            .state()
            .cache
            .data_lookup(address, 0x1000)
            .unwrap()
            .dirty
    );

    let mut secondary = ConformanceMachine::with_secondary(Mips4Endianness::Big, true);
    select_writeback_kseg0(&mut secondary);
    secondary.write_gpr(1, address);
    secondary
        .state_mut()
        .cache
        .secondary_install(Mips4CacheLine::from_data(
            0x1000,
            address,
            [0x5a; MIPS4_FUNCTIONAL_CACHE_LINE_BYTES],
        ));
    assert!(secondary.state().cache.secondary_lookup(0x1000).is_some());
    let flash_invalidate = i_type(0x2f, 1, 3, 0);
    assert!(matches!(
        secondary.execute(flash_invalidate),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert!(secondary.state().cache.secondary_lookup(0x1000).is_none());
}

#[test]
fn r5000_prefetch_instructions_retire_without_translation_or_bus_access() {
    let mut pref = ConformanceMachine::new(Mips4Endianness::Big);
    pref.write_gpr(1, 0x4000_0000_0000_0000);
    let bits = i_type(0x33, 1, 0, 1);
    assert_retired(pref.execute(bits), bits);

    let mut prefx = ConformanceMachine::new(Mips4Endianness::Big);
    prefx
        .state_mut()
        .cp0
        .write(
            Mips4Cp0Register::Status,
            (1 << 29) | (1 << 26) | (1 << 22) | (1 << 2),
        )
        .unwrap();
    prefx.write_gpr(1, 0x4000_0000_0000_0000);
    prefx.write_gpr(2, 1);
    let bits = ((0x13_u32) << 26) | (1 << 21) | (2 << 16) | 0x0f;
    assert_retired(prefx.execute(bits), bits);
}

const fn cp0_transfer(rs: u8, rt: u8, rd: u8) -> u32 {
    ((0x10_u32) << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | ((rd as u32) << 11)
}

const fn cp0_co(function: u8) -> u32 {
    ((0x10_u32) << 26) | (1 << 25) | function as u32
}

fn select_writeback_kseg0(machine: &mut ConformanceMachine) {
    let config = machine.state().cp0().config().bits();
    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::Config, u64::from((config & !0x07) | 3))
        .unwrap();
}
