use crate::cpu::execution::protocol::ExecutionAction;
use crate::cpu::mips4::cache::Mips4MemoryAccessType;
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::exception::Mips4Exception;

use super::super::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransaction,
    Mips4ExecutionTransferSize,
};
use super::super::target::Mips4ExecutionBoundary;
use super::{ConformanceMachine, assert_retired, i_type};

const KSEG1_BASE: u64 = 0xffff_ffff_a000_1000;

#[test]
fn normal_integer_loads_match_size_extension_and_byte_order_rules() {
    struct Case {
        opcode: u8,
        offset: u16,
        size: Mips4ExecutionTransferSize,
        lanes_big: u64,
        lanes_little: u64,
        expected: u64,
    }

    let cases = [
        Case {
            opcode: 0x20,
            offset: 1,
            size: Mips4ExecutionTransferSize::Byte,
            lanes_big: 0x80,
            lanes_little: 0x80,
            expected: 0xffff_ffff_ffff_ff80,
        },
        Case {
            opcode: 0x24,
            offset: 1,
            size: Mips4ExecutionTransferSize::Byte,
            lanes_big: 0x80,
            lanes_little: 0x80,
            expected: 0x80,
        },
        Case {
            opcode: 0x21,
            offset: 2,
            size: Mips4ExecutionTransferSize::Halfword,
            lanes_big: 0x0080,
            lanes_little: 0x8000,
            expected: 0xffff_ffff_ffff_8000,
        },
        Case {
            opcode: 0x25,
            offset: 2,
            size: Mips4ExecutionTransferSize::Halfword,
            lanes_big: 0x0080,
            lanes_little: 0x8000,
            expected: 0x8000,
        },
        Case {
            opcode: 0x23,
            offset: 4,
            size: Mips4ExecutionTransferSize::Word,
            lanes_big: 0x7856_3412,
            lanes_little: 0x1234_5678,
            expected: 0x1234_5678,
        },
        Case {
            opcode: 0x27,
            offset: 4,
            size: Mips4ExecutionTransferSize::Word,
            lanes_big: 0x7856_34f2,
            lanes_little: 0xf234_5678,
            expected: 0xf234_5678,
        },
        Case {
            opcode: 0x37,
            offset: 8,
            size: Mips4ExecutionTransferSize::Doubleword,
            lanes_big: 0x0807_0605_0403_0201,
            lanes_little: 0x0102_0304_0506_0708,
            expected: 0x0102_0304_0506_0708,
        },
    ];

    for endianness in [Mips4Endianness::Big, Mips4Endianness::Little] {
        for case in &cases {
            let mut machine = ConformanceMachine::new(endianness);
            machine.write_gpr(1, KSEG1_BASE);
            machine.write_gpr(2, 0xfeed_face_dead_beef);
            let bits = i_type(case.opcode, 1, 2, case.offset);
            let ExecutionAction::Transaction(transaction) = machine.begin(bits) else {
                panic!("expected load transaction for {bits:#010x}");
            };
            assert_eq!(
                transaction.payload,
                Mips4ExecutionTransaction::Read {
                    physical_address: 0x1000 + u64::from(case.offset),
                    size: case.size,
                    kind: Mips4ExecutionAccessKind::DataLoad,
                    access_type: Mips4MemoryAccessType::Uncached,
                }
            );
            let lanes = match endianness {
                Mips4Endianness::Big => case.lanes_big,
                Mips4Endianness::Little => case.lanes_little,
            };
            let ExecutionAction::Boundary(boundary) =
                machine.complete(transaction, Mips4ExecutionCompletion::ReadData(lanes))
            else {
                panic!("expected load boundary for {bits:#010x}");
            };
            assert_retired(boundary, bits);
            assert_eq!(machine.read_gpr(2), case.expected, "load {bits:#010x}");
        }
    }
}

#[test]
fn normal_integer_stores_emit_manual_lane_order_and_byte_enables() {
    struct Case {
        opcode: u8,
        offset: u16,
        size: Mips4ExecutionTransferSize,
        value: u64,
        data_big: u64,
        data_little: u64,
        byte_enable: u8,
    }

    let cases = [
        Case {
            opcode: 0x28,
            offset: 1,
            size: Mips4ExecutionTransferSize::Byte,
            value: 0x89ab_cdef,
            data_big: 0xef,
            data_little: 0xef,
            byte_enable: 0x01,
        },
        Case {
            opcode: 0x29,
            offset: 2,
            size: Mips4ExecutionTransferSize::Halfword,
            value: 0x89ab_cdef,
            data_big: 0xefcd,
            data_little: 0xcdef,
            byte_enable: 0x03,
        },
        Case {
            opcode: 0x2b,
            offset: 4,
            size: Mips4ExecutionTransferSize::Word,
            value: 0x89ab_cdef,
            data_big: 0xefcd_ab89,
            data_little: 0x89ab_cdef,
            byte_enable: 0x0f,
        },
        Case {
            opcode: 0x3f,
            offset: 8,
            size: Mips4ExecutionTransferSize::Doubleword,
            value: 0x0102_0304_0506_0708,
            data_big: 0x0807_0605_0403_0201,
            data_little: 0x0102_0304_0506_0708,
            byte_enable: 0xff,
        },
    ];

    for endianness in [Mips4Endianness::Big, Mips4Endianness::Little] {
        for case in &cases {
            let mut machine = ConformanceMachine::new(endianness);
            machine.write_gpr(1, KSEG1_BASE);
            machine.write_gpr(2, case.value);
            let bits = i_type(case.opcode, 1, 2, case.offset);
            let ExecutionAction::Transaction(transaction) = machine.begin(bits) else {
                panic!("expected store transaction for {bits:#010x}");
            };
            let data = match endianness {
                Mips4Endianness::Big => case.data_big,
                Mips4Endianness::Little => case.data_little,
            };
            assert_eq!(
                transaction.payload,
                Mips4ExecutionTransaction::Write {
                    physical_address: 0x1000 + u64::from(case.offset),
                    size: case.size,
                    data,
                    byte_enable: case.byte_enable,
                    access_type: Mips4MemoryAccessType::Uncached,
                }
            );
            let ExecutionAction::Boundary(boundary) =
                machine.complete(transaction, Mips4ExecutionCompletion::WriteComplete)
            else {
                panic!("expected store boundary for {bits:#010x}");
            };
            assert_retired(boundary, bits);
        }
    }
}

#[test]
fn unaligned_load_pairs_merge_through_the_instruction_path() {
    for (endianness, left_lanes, right_lanes) in [
        (Mips4Endianness::Big, 0x4433_2211, 0x8877_6655),
        (Mips4Endianness::Little, 0x1122_3344, 0x5566_7788),
    ] {
        let mut machine = ConformanceMachine::new(endianness);
        machine.write_gpr(1, KSEG1_BASE + 1);
        machine.write_gpr(2, 0xffff_ffff_aabb_ccdd);
        for (opcode, lanes) in [(0x22, left_lanes), (0x26, right_lanes)] {
            let bits = i_type(opcode, 1, 2, 0);
            let ExecutionAction::Transaction(transaction) = machine.begin(bits) else {
                panic!("expected partial load transaction");
            };
            let ExecutionAction::Boundary(boundary) =
                machine.complete(transaction, Mips4ExecutionCompletion::ReadData(lanes))
            else {
                panic!("expected partial load boundary");
            };
            assert!(
                matches!(boundary, Mips4ExecutionBoundary::Retired { instruction, .. } if instruction == bits)
            );
        }
        assert_ne!(machine.read_gpr(2), 0xffff_ffff_aabb_ccdd);
    }
}

#[test]
fn linked_and_conditional_stores_cover_success_failure_and_reservation_clear() {
    let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
    let config = machine.state().cp0().config().bits();
    machine
        .state_mut()
        .cp0
        .write(
            crate::cpu::mips4::cp0::Mips4Cp0Register::Config,
            u64::from((config & !0x07) | 3),
        )
        .unwrap();
    machine.write_gpr(1, 0xffff_ffff_8000_1000);
    let ll = i_type(0x30, 1, 2, 0);
    assert_retired(machine.execute_with_zero_bus(ll), ll);
    assert_eq!(machine.read_gpr(2), 0);
    assert!(matches!(
        machine.state().llbit(),
        crate::cpu::mips4::memory::ll_sc::Mips4LlBit::Set
    ));

    machine.write_gpr(2, 0x89ab_cdef);
    let sc = i_type(0x38, 1, 2, 0);
    let ExecutionAction::Boundary(boundary) = machine.begin(sc) else {
        panic!("expected cached SC boundary");
    };
    assert!(
        matches!(boundary, Mips4ExecutionBoundary::Retired { instruction, .. } if instruction == sc)
    );
    assert_eq!(machine.read_gpr(2), 1);
    assert!(matches!(
        machine.state().llbit(),
        crate::cpu::mips4::memory::ll_sc::Mips4LlBit::Clear
    ));

    machine.write_gpr(2, 0xfeed_face);
    assert!(matches!(machine.begin(sc), ExecutionAction::Boundary(_)));
    assert_eq!(machine.read_gpr(2), 0);
}

#[test]
fn misaligned_accesses_raise_the_architectural_address_exception_without_a_bus_request() {
    for (opcode, reason) in [
        (0x23, Mips4Exception::AddressErrorLoad),
        (0x2b, Mips4Exception::AddressErrorStore),
    ] {
        let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
        machine.write_gpr(1, KSEG1_BASE + 1);
        let bits = i_type(opcode, 1, 2, 0);
        assert!(matches!(
            machine.begin(bits),
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception {
                image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                    reason: actual,
                    bad_virtual_address: Some(KSEG1_BASE_PLUS_ONE),
                    ..
                },
                ..
            }) if actual == reason
        ));
    }
}

const KSEG1_BASE_PLUS_ONE: u64 = KSEG1_BASE + 1;
