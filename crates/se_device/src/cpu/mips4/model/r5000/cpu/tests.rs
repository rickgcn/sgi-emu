use std::collections::BTreeMap;

use se_core::role::{BusControllerRole, BusDeviceRole, BusRole};
use se_float::backend::native::NativeFloatBackend;

use crate::cpu::execution::protocol::{ExecutionAction, ExecutionTransaction};
use crate::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
use crate::cpu::mips4::execution::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};
use crate::cpu::mips4::model::r5000::revision::R5000Revision;

use super::*;

fn profile() -> R5000Profile {
    R5000Profile::new(
        Mips4Endianness::Big,
        R5000Revision::from_bits(0x21),
        200_000_000,
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::disabled(),
    )
}

fn cpu() -> R5000Cpu<NativeFloatBackend> {
    R5000Cpu::with_float_backend(
        ComponentId::new(7),
        "cpu0",
        profile(),
        R5000BootMode::from_low_bits(0).unwrap(),
        NativeFloatBackend::new(),
    )
    .unwrap()
}

fn big_endian_word(bits: u32) -> u64 {
    let bytes = bits.to_be_bytes();
    u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0])
}

fn retire_nop(cpu: &mut R5000Cpu<NativeFloatBackend>) {
    let ExecutionAction::Transaction(fetch) = cpu.poll().unwrap() else {
        panic!("expected fetch");
    };
    assert_eq!(
        fetch.payload,
        Mips4ExecutionTransaction::Read {
            physical_address: 0x1fc0_0000 + (cpu.state().pc() & 0x0fff),
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::InstructionFetch,
            access_type: crate::cpu::mips4::cache::Mips4MemoryAccessType::Uncached,
        }
    );
    BusControllerRole::complete(
        cpu,
        ExecutionCompletion {
            id: fetch.id,
            payload: Mips4ExecutionCompletion::ReadData(big_endian_word(0)),
        },
    );
    let ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. }) = cpu.poll().unwrap()
    else {
        panic!("expected retired boundary");
    };
}

#[test]
fn component_identity_and_reset_image_are_visible() {
    let mut cpu = cpu();
    assert_eq!(cpu.id(), ComponentId::new(7));
    assert_eq!(cpu.name(), "cpu0");
    assert_eq!(cpu.state().pc(), 0xffff_ffff_bfc0_0000);
    assert_eq!(cpu.state().cp0().processor_id().bits(), 0x2321);
    assert_eq!(cpu.state().cp1().fcr0().bits(), 0x2321);

    retire_nop(&mut cpu);
    cpu.reset();
    assert_eq!(cpu.state().pc(), 0xffff_ffff_bfc0_0000);
}

#[test]
fn count_uses_half_pclock_remainder_without_drift() {
    let mut cpu = cpu();
    assert_eq!(cpu.state().cp0().count().bits(), 0);
    retire_nop(&mut cpu);
    assert_eq!(cpu.state().cp0().count().bits(), 0);
    retire_nop(&mut cpu);
    assert_eq!(cpu.state().cp0().count().bits(), 1);

    cpu.advance_pclocks(5);
    assert_eq!(cpu.state().cp0().count().bits(), 3);
    cpu.advance_pclocks(1);
    assert_eq!(cpu.state().cp0().count().bits(), 4);
}

#[test]
fn bus_device_signals_update_interrupt_lines() {
    let mut cpu = cpu();
    BusDeviceRole::accept(&mut cpu, R5000CpuSignal::ExternalInterrupts(0x24));
    assert_eq!(cpu.state().external_interrupts(), 0x24);
    assert_eq!(cpu.state().cp0().cause().interrupt_pending() & 0x24, 0x24);
}

struct FakeRam {
    bytes: BTreeMap<u64, u8>,
}

impl FakeRam {
    fn new() -> Self {
        Self {
            bytes: BTreeMap::new(),
        }
    }

    fn load_word_be(&mut self, address: u64, word: u32) {
        for (offset, byte) in word.to_be_bytes().into_iter().enumerate() {
            self.bytes.insert(address + offset as u64, byte);
        }
    }

    fn read_word_be(&self, address: u64) -> u32 {
        u32::from_be_bytes([
            *self.bytes.get(&address).unwrap_or(&0),
            *self.bytes.get(&(address + 1)).unwrap_or(&0),
            *self.bytes.get(&(address + 2)).unwrap_or(&0),
            *self.bytes.get(&(address + 3)).unwrap_or(&0),
        ])
    }
}

impl BusDeviceRole<Mips4ExecutionTransaction> for FakeRam {
    type Response = Mips4ExecutionCompletion;

    fn accept(&mut self, transaction: Mips4ExecutionTransaction) -> Self::Response {
        match transaction {
            Mips4ExecutionTransaction::Read {
                physical_address,
                size,
                ..
            } => {
                let mut data = 0;
                for offset in 0..size.bytes() {
                    data |= u64::from(
                        *self
                            .bytes
                            .get(&(physical_address + u64::from(offset)))
                            .unwrap_or(&0),
                    ) << (offset * 8);
                }
                Mips4ExecutionCompletion::ReadData(data)
            }
            Mips4ExecutionTransaction::Write {
                physical_address,
                size,
                data,
                byte_enable,
                ..
            } => {
                for offset in 0..size.bytes() {
                    if byte_enable & (1 << offset) != 0 {
                        self.bytes.insert(
                            physical_address + u64::from(offset),
                            (data >> (offset * 8)) as u8,
                        );
                    }
                }
                Mips4ExecutionCompletion::WriteComplete
            }
        }
    }
}

struct FakeBus {
    ram: FakeRam,
}

impl BusRole<ExecutionTransaction<Mips4ExecutionTransaction>> for FakeBus {
    type Response = ExecutionCompletion<Mips4ExecutionCompletion>;

    fn route(
        &mut self,
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
    ) -> Self::Response {
        ExecutionCompletion {
            id: transaction.id,
            payload: self.ram.accept(transaction.payload),
        }
    }
}

#[test]
fn delayed_bus_completion_keeps_the_cpu_waiting_on_the_same_id() {
    let mut cpu = cpu();
    let mut bus = FakeBus {
        ram: FakeRam::new(),
    };
    bus.ram.load_word_be(0x1fc0_0000, 0);

    let ExecutionAction::Transaction(fetch) = cpu.poll().unwrap() else {
        panic!("expected fetch");
    };
    let ExecutionAction::Waiting { transaction_id } = cpu.poll().unwrap() else {
        panic!("expected wait state");
    };
    assert_eq!(transaction_id, fetch.id);

    let completion = bus.route(fetch);
    BusControllerRole::complete(&mut cpu, completion);
    assert!(matches!(
        cpu.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. })
    ));
}

#[test]
fn hand_written_rom_runs_through_cpu_bus_and_ram_roles() {
    const ROM: [u32; 16] = [
        0x2401_0005,
        0x2402_0007,
        0x0022_1821,
        0x3c04_8000,
        0xac83_0000,
        0x8c85_0000,
        0x10a3_0001,
        0x2406_0001,
        0x3c07_2440,
        0x34e7_0004,
        0x4087_6000,
        0x3c08_3f80,
        0x4488_1000,
        0x4602_1100,
        0x4409_2000,
        0x0000_000d,
    ];

    let mut cpu = cpu();
    let mut bus = FakeBus {
        ram: FakeRam::new(),
    };
    for (index, instruction) in ROM.into_iter().enumerate() {
        bus.ram
            .load_word_be(0x1fc0_0000 + (index as u64 * 4), instruction);
    }

    let mut boundaries = 0;
    loop {
        match cpu.poll().unwrap() {
            ExecutionAction::Transaction(transaction) => {
                let completion = bus.route(transaction);
                BusControllerRole::complete(&mut cpu, completion);
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. }) => {
                boundaries += 1;
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception { image, .. }) => {
                assert_eq!(
                    image.reason,
                    crate::cpu::mips4::exception::Mips4Exception::Breakpoint
                );
                break;
            }
            ExecutionAction::Waiting { .. } => panic!("immediate bus must not remain waiting"),
        }
    }

    assert_eq!(boundaries, 15);
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(3).unwrap()),
        12
    );
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(5).unwrap()),
        12
    );
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(6).unwrap()),
        1
    );
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(9).unwrap()),
        2.0f32.to_bits() as u64
    );
    assert_eq!(bus.ram.read_word_be(0), 12);
}

#[test]
fn instruction_tlb_miss_selects_the_boot_refill_vector() {
    const ROM: [u32; 5] = [
        0x2401_0000,
        0x4081_7000,
        0x3c01_0040,
        0x4081_6000,
        0x4200_0018,
    ];

    let mut cpu = cpu();
    let mut bus = FakeBus {
        ram: FakeRam::new(),
    };
    for (index, instruction) in ROM.into_iter().enumerate() {
        bus.ram
            .load_word_be(0x1fc0_0000 + (index as u64 * 4), instruction);
    }

    let mut retired = 0;
    while retired != ROM.len() {
        match cpu.poll().unwrap() {
            ExecutionAction::Transaction(transaction) => {
                let completion = bus.route(transaction);
                BusControllerRole::complete(&mut cpu, completion);
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. }) => retired += 1,
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception { .. }) => {
                panic!("setup instruction unexpectedly trapped");
            }
            ExecutionAction::Waiting { .. } => unreachable!(),
        }
    }

    let ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception { image, vector, .. }) =
        cpu.poll().unwrap()
    else {
        panic!("expected instruction TLB miss");
    };
    assert_eq!(
        image.reason,
        crate::cpu::mips4::exception::Mips4Exception::TlbLoad
    );
    assert_eq!(image.bad_virtual_address, Some(0));
    assert_eq!(vector, 0xffff_ffff_bfc0_0200);
}
