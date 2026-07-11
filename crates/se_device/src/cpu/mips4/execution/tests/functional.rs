use se_float::backend::native::NativeFloatBackend;

use crate::cpu::execution::functional::FunctionalExecutor;
use crate::cpu::execution::protocol::{ExecutionAction, ExecutionCompletion};
use crate::cpu::mips4::cache::hierarchy::{
    Mips4CacheAccessPolicy, Mips4CacheGeometry, Mips4CacheHierarchyConfig,
};
use crate::cpu::mips4::cache::{Mips4CacheCoherenceAlgorithm, Mips4MemoryAccessType};
use crate::cpu::mips4::config::{
    Mips4AddressConfig, Mips4CacheConfig, Mips4Config, Mips4CoprocessorConfig, Mips4Endianness,
};
use crate::cpu::mips4::cp0::{Mips4Cp0CacheErr, Mips4Cp0Config, Mips4Cp0Register};
use crate::cpu::mips4::exception::{Mips4ErrorException, Mips4Exception, Mips4ExceptionImage};
use crate::cpu::mips4::gpr::Mips4GprIndex;
use crate::cpu::mips4::mmu::{Mips4MmuCacheAttribute, Mips4MmuConfig};
use crate::cpu::mips4::tlb::Mips4TlbAddressMode;

use super::super::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransaction,
    Mips4ExecutionTransferSize,
};
use super::super::policy::{
    Mips4Cp0DoublewordTransferDirection, Mips4Cp0DoublewordTransferPolicy, Mips4Cp0WaitPolicy,
    Mips4ExecutionPolicy,
};
use super::super::target::{Mips4ExecutionBoundary, Mips4ExecutionSignal, Mips4ExecutionTarget};

const RESET_PC: u64 = 0xffff_ffff_8000_0000;
const EXCEPTION_VECTOR: u64 = 0xffff_ffff_8000_0180;

struct TestPolicy;

struct CachedTestPolicy;

fn test_architecture_config(primary_cache: Mips4CacheConfig) -> Mips4Config {
    Mips4Config::new(
        Mips4Endianness::Big,
        0x2300,
        Mips4AddressConfig::new(36, 40),
        primary_cache,
        primary_cache,
        Mips4CacheConfig::disabled(),
        Mips4CoprocessorConfig::new(true, false),
    )
}

impl Mips4ExecutionPolicy for TestPolicy {
    fn reset_pc(&self) -> u64 {
        RESET_PC
    }

    fn architecture_config(&self) -> Mips4Config {
        test_architecture_config(Mips4CacheConfig::disabled())
    }

    fn cp0_config(&self) -> u32 {
        0
    }

    fn fcr0(&self) -> u32 {
        0x2300
    }

    fn tlb_entry_count(&self) -> usize {
        48
    }

    fn tlb_random_upper_bound(&self) -> u8 {
        47
    }

    fn mmu_config(&self, _config: Mips4Cp0Config) -> Mips4MmuConfig {
        Mips4MmuConfig::new(
            self.architecture_config().address,
            Mips4CacheCoherenceAlgorithm::from_bits(3).unwrap(),
        )
    }

    fn cp0_write_value(&self, _register: Mips4Cp0Register, _current: u64, requested: u64) -> u64 {
        requested
    }

    fn cp0_wait_policy(&self) -> Mips4Cp0WaitPolicy {
        Mips4Cp0WaitPolicy::Standby
    }

    fn cp0_doubleword_transfer_policy(
        &self,
        _direction: Mips4Cp0DoublewordTransferDirection,
        _status: crate::cpu::mips4::cp0::Mips4Cp0Status,
        _register: Mips4Cp0Register,
    ) -> Mips4Cp0DoublewordTransferPolicy {
        Mips4Cp0DoublewordTransferPolicy::Execute
    }

    fn resolve_access_type(
        &self,
        cache_attribute: Mips4MmuCacheAttribute,
    ) -> Mips4MemoryAccessType {
        if cache_attribute.is_uncached() {
            Mips4MemoryAccessType::Uncached
        } else {
            Mips4MemoryAccessType::CachedNoncoherent
        }
    }

    fn cache_config(&self) -> Mips4CacheHierarchyConfig {
        Mips4CacheHierarchyConfig::disabled()
    }

    fn resolve_cache_policy(
        &self,
        _cache_attribute: Mips4MmuCacheAttribute,
    ) -> Mips4CacheAccessPolicy {
        Mips4CacheAccessPolicy::Uncached
    }

    fn exception_vector(
        &self,
        _status_before_exception: crate::cpu::mips4::cp0::Mips4Cp0Status,
        _image: Mips4ExceptionImage,
        _refill_address_mode: Option<Mips4TlbAddressMode>,
    ) -> u64 {
        EXCEPTION_VECTOR
    }

    fn error_exception_vector(
        &self,
        status_before_exception: crate::cpu::mips4::cp0::Mips4Cp0Status,
        reason: Mips4ErrorException,
    ) -> u64 {
        match reason {
            Mips4ErrorException::SoftReset | Mips4ErrorException::NonMaskableInterrupt => {
                0xffff_ffff_bfc0_0000
            }
            Mips4ErrorException::CacheError => {
                if status_before_exception.boot_exception_vectors() {
                    0xffff_ffff_bfc0_0300
                } else {
                    0xffff_ffff_a000_0100
                }
            }
        }
    }
}

impl Mips4ExecutionPolicy for CachedTestPolicy {
    fn reset_pc(&self) -> u64 {
        RESET_PC
    }

    fn architecture_config(&self) -> Mips4Config {
        test_architecture_config(Mips4CacheConfig::present(32 * 1024, 32))
    }

    fn cp0_config(&self) -> u32 {
        3
    }

    fn fcr0(&self) -> u32 {
        0x2300
    }

    fn tlb_entry_count(&self) -> usize {
        48
    }

    fn tlb_random_upper_bound(&self) -> u8 {
        47
    }

    fn mmu_config(&self, _config: Mips4Cp0Config) -> Mips4MmuConfig {
        Mips4MmuConfig::new(
            self.architecture_config().address,
            Mips4CacheCoherenceAlgorithm::from_bits(3).unwrap(),
        )
    }

    fn cp0_write_value(&self, _register: Mips4Cp0Register, _current: u64, requested: u64) -> u64 {
        requested
    }

    fn cp0_wait_policy(&self) -> Mips4Cp0WaitPolicy {
        Mips4Cp0WaitPolicy::Standby
    }

    fn cp0_doubleword_transfer_policy(
        &self,
        _direction: Mips4Cp0DoublewordTransferDirection,
        _status: crate::cpu::mips4::cp0::Mips4Cp0Status,
        _register: Mips4Cp0Register,
    ) -> Mips4Cp0DoublewordTransferPolicy {
        Mips4Cp0DoublewordTransferPolicy::Execute
    }

    fn resolve_access_type(
        &self,
        cache_attribute: Mips4MmuCacheAttribute,
    ) -> Mips4MemoryAccessType {
        if cache_attribute.is_uncached() {
            Mips4MemoryAccessType::Uncached
        } else {
            Mips4MemoryAccessType::CachedNoncoherent
        }
    }

    fn cache_config(&self) -> Mips4CacheHierarchyConfig {
        let primary = Mips4CacheGeometry::new(32 * 1024, 32, 2);
        Mips4CacheHierarchyConfig::new(Some(primary), Some(primary), None)
    }

    fn resolve_cache_policy(
        &self,
        cache_attribute: Mips4MmuCacheAttribute,
    ) -> Mips4CacheAccessPolicy {
        if cache_attribute.is_uncached() {
            Mips4CacheAccessPolicy::Uncached
        } else {
            Mips4CacheAccessPolicy::WriteBackWriteAllocate
        }
    }

    fn exception_vector(
        &self,
        _status_before_exception: crate::cpu::mips4::cp0::Mips4Cp0Status,
        _image: Mips4ExceptionImage,
        _refill_address_mode: Option<Mips4TlbAddressMode>,
    ) -> u64 {
        EXCEPTION_VECTOR
    }

    fn error_exception_vector(
        &self,
        status_before_exception: crate::cpu::mips4::cp0::Mips4Cp0Status,
        reason: Mips4ErrorException,
    ) -> u64 {
        match reason {
            Mips4ErrorException::SoftReset | Mips4ErrorException::NonMaskableInterrupt => {
                0xffff_ffff_bfc0_0000
            }
            Mips4ErrorException::CacheError => {
                if status_before_exception.boot_exception_vectors() {
                    0xffff_ffff_bfc0_0300
                } else {
                    0xffff_ffff_a000_0100
                }
            }
        }
    }
}

fn executor() -> FunctionalExecutor<Mips4ExecutionTarget<TestPolicy, NativeFloatBackend>> {
    FunctionalExecutor::new(
        Mips4ExecutionTarget::new(TestPolicy, NativeFloatBackend::new()).unwrap(),
    )
}

fn big_endian_word(bits: u32) -> u64 {
    let bytes = bits.to_be_bytes();
    u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0])
}

#[test]
fn instruction_cache_fills_four_doublewords_then_hits_without_a_bus_transaction() {
    let target = Mips4ExecutionTarget::new(CachedTestPolicy, NativeFloatBackend::new()).unwrap();
    let mut executor = FunctionalExecutor::new(target);
    for doubleword in 0..4_u64 {
        let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
            panic!("expected cache-line fill transaction");
        };
        assert_eq!(
            transaction.payload,
            Mips4ExecutionTransaction::Read {
                physical_address: doubleword * 8,
                size: Mips4ExecutionTransferSize::Doubleword,
                kind: Mips4ExecutionAccessKind::InstructionFetch,
                access_type: Mips4MemoryAccessType::CachedNoncoherent,
            }
        );
        executor
            .complete(ExecutionCompletion {
                id: transaction.id,
                payload: Mips4ExecutionCompletion::ReadData(0),
            })
            .unwrap();
    }
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. })
    ));
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. })
    ));
}

#[test]
fn failed_instruction_fill_does_not_install_a_partial_cache_line() {
    let target = Mips4ExecutionTarget::new(CachedTestPolicy, NativeFloatBackend::new()).unwrap();
    let mut executor = FunctionalExecutor::new(target);
    for doubleword in 0..3 {
        let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
            panic!("expected cache-line fill transaction");
        };
        let completion = if doubleword == 2 {
            Mips4ExecutionCompletion::BusError
        } else {
            Mips4ExecutionCompletion::ReadData(u64::MAX)
        };
        executor
            .complete(ExecutionCompletion {
                id: transaction.id,
                payload: completion,
            })
            .unwrap();
    }
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception {
            image: Mips4ExceptionImage {
                reason: crate::cpu::mips4::exception::Mips4Exception::InstructionBusError,
                ..
            },
            ..
        })
    ));
    assert!(
        executor
            .target()
            .state()
            .cache
            .instruction_lookup(RESET_PC, 0)
            .is_none()
    );
}

#[test]
fn corrupted_instruction_cache_data_enters_cache_error_vector() {
    let target = Mips4ExecutionTarget::new(CachedTestPolicy, NativeFloatBackend::new()).unwrap();
    let mut executor = FunctionalExecutor::new(target);
    for _ in 0..4 {
        let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
            panic!("expected cache-line fill transaction");
        };
        executor
            .complete(ExecutionCompletion {
                id: transaction.id,
                payload: Mips4ExecutionCompletion::ReadData(0),
            })
            .unwrap();
    }
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. })
    ));
    executor
        .target_mut()
        .state_mut()
        .cache
        .primary_hit_line_mut(true, RESET_PC + 4, 4)
        .unwrap()
        .check_bits[0] ^= 1;

    let ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException { image, vector, .. }) =
        executor.poll().unwrap()
    else {
        panic!("expected cache-error boundary");
    };
    assert_eq!(image.reason, Mips4ErrorException::CacheError);
    let cache_error = Mips4Cp0CacheErr::from_bits(image.cache_error.unwrap());
    assert!(!cache_error.data_reference());
    assert!(cache_error.data_field_error());
    assert!(!cache_error.tag_field_error());
    assert_eq!(vector, 0xffff_ffff_bfc0_0300);
}

fn complete_instruction(
    executor: &mut FunctionalExecutor<Mips4ExecutionTarget<TestPolicy, NativeFloatBackend>>,
    bits: u32,
) -> Mips4ExecutionBoundary {
    let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
        panic!("expected instruction fetch");
    };
    assert_eq!(
        transaction.payload,
        Mips4ExecutionTransaction::Read {
            physical_address: executor.target().state().pc() & 0x1fff_ffff,
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::InstructionFetch,
            access_type: Mips4MemoryAccessType::CachedNoncoherent,
        }
    );
    executor
        .complete(ExecutionCompletion {
            id: transaction.id,
            payload: Mips4ExecutionCompletion::ReadData(big_endian_word(bits)),
        })
        .unwrap();
    let ExecutionAction::Boundary(boundary) = executor.poll().unwrap() else {
        panic!("expected instruction boundary");
    };
    boundary
}

fn complete_fetch_for_data(
    executor: &mut FunctionalExecutor<Mips4ExecutionTarget<TestPolicy, NativeFloatBackend>>,
    bits: u32,
) -> crate::cpu::execution::protocol::ExecutionTransaction<Mips4ExecutionTransaction> {
    let ExecutionAction::Transaction(fetch) = executor.poll().unwrap() else {
        panic!("expected instruction fetch");
    };
    executor
        .complete(ExecutionCompletion {
            id: fetch.id,
            payload: Mips4ExecutionCompletion::ReadData(big_endian_word(bits)),
        })
        .unwrap();
    let ExecutionAction::Transaction(data) = executor.poll().unwrap() else {
        panic!("expected data transaction");
    };
    data
}

fn complete_data_boundary(
    executor: &mut FunctionalExecutor<Mips4ExecutionTarget<TestPolicy, NativeFloatBackend>>,
    id: crate::cpu::execution::protocol::ExecutionTransactionId,
    completion: Mips4ExecutionCompletion,
) -> Mips4ExecutionBoundary {
    executor
        .complete(ExecutionCompletion {
            id,
            payload: completion,
        })
        .unwrap();
    let ExecutionAction::Boundary(boundary) = executor.poll().unwrap() else {
        panic!("expected instruction boundary");
    };
    boundary
}

#[test]
fn addiu_fetches_through_bus_and_commits_register() {
    let mut executor = executor();
    let boundary = complete_instruction(&mut executor, 0x2401_0005);

    assert_eq!(
        boundary,
        Mips4ExecutionBoundary::Retired {
            pc: RESET_PC,
            instruction: 0x2401_0005,
        }
    );
    assert_eq!(
        executor
            .target()
            .state()
            .gpr()
            .read(Mips4GprIndex::from_u8(1).unwrap()),
        5
    );
    assert_eq!(executor.target().state().pc(), RESET_PC + 4);
}

#[test]
fn taken_branch_enters_delay_slot_with_target_as_next_pc() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x1000_0002);

    assert_eq!(executor.target().state().pc(), RESET_PC + 4);
    assert_eq!(executor.target().state().next_pc(), RESET_PC + 12);
    assert_eq!(
        executor.target().state().delay_slot_branch_pc(),
        Some(RESET_PC)
    );
}

#[test]
fn overflow_enters_exception_without_writing_destination() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x3c01_7fff);
    complete_instruction(&mut executor, 0x3421_ffff);
    let boundary = complete_instruction(&mut executor, 0x2022_0001);

    let Mips4ExecutionBoundary::Exception { vector, .. } = boundary else {
        panic!("expected overflow exception");
    };
    assert_eq!(vector, EXCEPTION_VECTOR);
    assert_eq!(executor.target().state().pc(), EXCEPTION_VECTOR);
    assert_eq!(
        executor
            .target()
            .state()
            .gpr()
            .read(Mips4GprIndex::from_u8(2).unwrap()),
        0
    );
}

#[test]
fn word_load_uses_physical_lanes_and_commits_after_completion() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x3c01_8000);

    let data = complete_fetch_for_data(&mut executor, 0x8c22_0000);
    assert_eq!(
        data.payload,
        Mips4ExecutionTransaction::Read {
            physical_address: 0,
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::DataLoad,
            access_type: Mips4MemoryAccessType::CachedNoncoherent,
        }
    );
    assert_eq!(
        executor
            .target()
            .state()
            .gpr()
            .read(Mips4GprIndex::from_u8(2).unwrap()),
        0
    );

    complete_data_boundary(
        &mut executor,
        data.id,
        Mips4ExecutionCompletion::ReadData(big_endian_word(0x89ab_cdef)),
    );
    assert_eq!(
        executor
            .target()
            .state()
            .gpr()
            .read(Mips4GprIndex::from_u8(2).unwrap()),
        0xffff_ffff_89ab_cdef
    );
}

#[test]
fn word_store_emits_byte_enables_without_read_modify_write() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x3c01_8000);
    complete_instruction(&mut executor, 0x3c02_1234);
    complete_instruction(&mut executor, 0x3442_5678);

    let data = complete_fetch_for_data(&mut executor, 0xac22_0004);
    assert_eq!(
        data.payload,
        Mips4ExecutionTransaction::Write {
            physical_address: 4,
            size: Mips4ExecutionTransferSize::Word,
            data: big_endian_word(0x1234_5678),
            byte_enable: 0x0f,
            access_type: Mips4MemoryAccessType::CachedNoncoherent,
        }
    );
    complete_data_boundary(
        &mut executor,
        data.id,
        Mips4ExecutionCompletion::WriteComplete,
    );
}

#[test]
fn soft_reset_aborts_an_outstanding_transaction_and_enters_error_level() {
    let mut executor = executor();
    let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
        panic!("expected instruction fetch");
    };

    executor.signal(Mips4ExecutionSignal::SoftReset);

    let ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException { pc, image, vector }) =
        executor.poll().unwrap()
    else {
        panic!("expected soft-reset boundary");
    };
    assert_eq!(pc, RESET_PC);
    assert_eq!(image.reason, Mips4ErrorException::SoftReset);
    assert_eq!(image.restart.restart_pc, RESET_PC);
    assert_eq!(image.cache_error, None);
    assert_eq!(vector, 0xffff_ffff_bfc0_0000);
    assert_eq!(executor.target().state().pc(), vector);
    assert_eq!(
        executor.target().state().cp0().error_epc().address(),
        RESET_PC
    );
    assert!(executor.target().state().cp0().status().error_level());
    assert!(
        executor
            .target()
            .state()
            .cp0()
            .status()
            .boot_exception_vectors()
    );
    assert!(executor.target().state().cp0().status().soft_reset_or_nmi());
    assert!(matches!(
        executor.complete(ExecutionCompletion {
            id: transaction.id,
            payload: Mips4ExecutionCompletion::ReadData(0),
        }),
        Err(crate::cpu::execution::protocol::FunctionalExecutorError::UnexpectedCompletion { .. })
    ));
}

#[test]
fn nmi_waits_for_the_current_instruction_boundary() {
    let mut executor = executor();
    let ExecutionAction::Transaction(transaction) = executor.poll().unwrap() else {
        panic!("expected instruction fetch");
    };

    executor.signal(Mips4ExecutionSignal::NonMaskableInterrupt);
    assert_eq!(
        executor.poll().unwrap(),
        ExecutionAction::Waiting {
            transaction_id: transaction.id,
        }
    );
    executor
        .complete(ExecutionCompletion {
            id: transaction.id,
            payload: Mips4ExecutionCompletion::ReadData(big_endian_word(0)),
        })
        .unwrap();
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { pc: RESET_PC, .. })
    ));

    let ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException { pc, image, vector }) =
        executor.poll().unwrap()
    else {
        panic!("expected NMI boundary");
    };
    assert_eq!(pc, RESET_PC + 4);
    assert_eq!(image.reason, Mips4ErrorException::NonMaskableInterrupt);
    assert_eq!(image.restart.restart_pc, RESET_PC + 4);
    assert_eq!(vector, 0xffff_ffff_bfc0_0000);
}

#[test]
fn cache_error_records_cacheerr_and_uses_the_boot_vector() {
    let mut executor = executor();
    let cache_error = Mips4Cp0CacheErr::from_bits(0xb300_1231);

    executor.signal(Mips4ExecutionSignal::CacheError(cache_error));

    let ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException { pc, image, vector }) =
        executor.poll().unwrap()
    else {
        panic!("expected cache-error boundary");
    };
    assert_eq!(pc, RESET_PC);
    assert_eq!(image.reason, Mips4ErrorException::CacheError);
    assert_eq!(image.cache_error, Some(cache_error.bits()));
    assert_eq!(vector, 0xffff_ffff_bfc0_0300);
    assert_eq!(executor.target().state().cp0().cache_err(), cache_error);
    assert!(executor.target().state().cp0().status().error_level());
    assert!(!executor.target().state().cp0().status().soft_reset_or_nmi());
}

#[test]
fn cache_error_cancels_a_latched_nmi() {
    let mut executor = executor();
    executor.signal(Mips4ExecutionSignal::NonMaskableInterrupt);
    executor.signal(Mips4ExecutionSignal::CacheError(
        Mips4Cp0CacheErr::from_bits(0xb300_1231),
    ));

    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException {
            image: crate::cpu::mips4::exception::Mips4ErrorExceptionImage {
                reason: Mips4ErrorException::CacheError,
                ..
            },
            ..
        })
    ));
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Transaction(_)
    ));
}

#[test]
fn eret_returns_from_cache_error_using_error_epc() {
    let mut executor = executor();
    executor.signal(Mips4ExecutionSignal::CacheError(
        Mips4Cp0CacheErr::from_bits(0xb300_1231),
    ));
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException { .. })
    ));

    let ExecutionAction::Transaction(fetch) = executor.poll().unwrap() else {
        panic!("expected uncached cache-error-vector fetch");
    };
    assert_eq!(
        fetch.payload,
        Mips4ExecutionTransaction::Read {
            physical_address: 0x1fc0_0300,
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::InstructionFetch,
            access_type: Mips4MemoryAccessType::Uncached,
        }
    );
    executor
        .complete(ExecutionCompletion {
            id: fetch.id,
            payload: Mips4ExecutionCompletion::ReadData(big_endian_word(0x4200_0018)),
        })
        .unwrap();
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. })
    ));

    assert_eq!(executor.target().state().pc(), RESET_PC);
    assert!(!executor.target().state().cp0().status().error_level());
}

#[test]
fn status_de_masks_cache_error_signals() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x3c01_0001);
    complete_instruction(&mut executor, 0x4081_6000);
    assert!(
        executor
            .target()
            .state()
            .cp0()
            .status()
            .cache_error_disabled()
    );

    executor.signal(Mips4ExecutionSignal::CacheError(
        Mips4Cp0CacheErr::from_bits(0xb300_1231),
    ));

    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Transaction(_)
    ));
    assert_eq!(executor.target().state().cp0().cache_err().bits(), 0);
}

#[test]
fn linked_load_and_store_conditional_follow_completion_state() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x3c01_8000);

    let linked = complete_fetch_for_data(&mut executor, 0xc022_0000);
    complete_data_boundary(
        &mut executor,
        linked.id,
        Mips4ExecutionCompletion::ReadData(big_endian_word(7)),
    );
    assert_eq!(
        executor.target().state().llbit(),
        crate::cpu::mips4::memory::ll_sc::Mips4LlBit::Set
    );

    let conditional = complete_fetch_for_data(&mut executor, 0xe022_0000);
    complete_data_boundary(
        &mut executor,
        conditional.id,
        Mips4ExecutionCompletion::WriteComplete,
    );
    assert_eq!(
        executor
            .target()
            .state()
            .gpr()
            .read(Mips4GprIndex::from_u8(2).unwrap()),
        1
    );
    assert_eq!(
        executor.target().state().llbit(),
        crate::cpu::mips4::memory::ll_sc::Mips4LlBit::Clear
    );
}

#[test]
fn cp0_word_transfers_apply_register_width_and_masks() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x2401_0000);
    complete_instruction(&mut executor, 0x4081_6000);
    complete_instruction(&mut executor, 0x4002_6000);

    assert_eq!(executor.target().state().cp0().status().bits(), 0);
    assert_eq!(
        executor
            .target()
            .state()
            .gpr()
            .read(Mips4GprIndex::from_u8(2).unwrap()),
        0
    );
}

#[test]
fn wait_retires_once_then_remains_idle_until_an_interrupt_is_pending() {
    let mut executor = executor();
    let boundary = complete_instruction(&mut executor, 0x4200_0020);
    assert!(matches!(
        boundary,
        Mips4ExecutionBoundary::Retired {
            instruction: 0x4200_0020,
            ..
        }
    ));
    assert_eq!(executor.poll().unwrap(), ExecutionAction::Idle);
    assert_eq!(executor.poll().unwrap(), ExecutionAction::Idle);

    executor.signal(Mips4ExecutionSignal::ExternalInterrupts(0x04));
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Transaction(_)
    ));
}

#[test]
fn enabled_interrupt_wakes_wait_and_enters_the_interrupt_vector() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x2401_0401);
    complete_instruction(&mut executor, 0x4081_6000);
    complete_instruction(&mut executor, 0x4200_0020);
    assert_eq!(executor.poll().unwrap(), ExecutionAction::Idle);

    executor.signal(Mips4ExecutionSignal::ExternalInterrupts(0x04));
    let ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception { image, vector, .. }) =
        executor.poll().unwrap()
    else {
        panic!("expected interrupt exception after wake");
    };
    assert_eq!(image.reason, Mips4Exception::Interrupt);
    assert_eq!(vector, EXCEPTION_VECTOR);
}

#[test]
fn pending_software_interrupt_prevents_wait_from_remaining_idle() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x2401_0100);
    complete_instruction(&mut executor, 0x4081_6800);
    complete_instruction(&mut executor, 0x4200_0020);

    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Transaction(_)
    ));
}

#[test]
fn timer_interrupt_and_reset_wake_wait() {
    {
        let mut executor = executor();
        complete_instruction(&mut executor, 0x2401_0001);
        complete_instruction(&mut executor, 0x4081_5800);
        complete_instruction(&mut executor, 0x4200_0020);
        assert_eq!(executor.poll().unwrap(), ExecutionAction::Idle);

        executor.target_mut().advance_count(1, true);
        assert!(matches!(
            executor.poll().unwrap(),
            ExecutionAction::Transaction(_)
        ));
    }

    let mut executor = executor();
    complete_instruction(&mut executor, 0x4200_0020);
    assert_eq!(executor.poll().unwrap(), ExecutionAction::Idle);
    executor.reset();
    assert!(matches!(
        executor.poll().unwrap(),
        ExecutionAction::Transaction(_)
    ));
}

#[test]
fn eret_restores_epc_and_clears_exception_level() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x2401_0100);
    complete_instruction(&mut executor, 0x4081_7000);
    complete_instruction(&mut executor, 0x2401_0000);
    complete_instruction(&mut executor, 0x4081_6000);
    complete_instruction(&mut executor, 0x4200_0018);

    assert_eq!(executor.target().state().pc(), 0x100);
    assert!(!executor.target().state().cp0().status().exception_level());
    assert!(!executor.target().state().cp0().status().error_level());
}

#[test]
fn pending_unmasked_interrupt_enters_before_the_next_fetch() {
    let mut executor = executor();
    complete_instruction(&mut executor, 0x2401_0401);
    complete_instruction(&mut executor, 0x4081_6000);
    executor.signal(Mips4ExecutionSignal::ExternalInterrupts(0x04));

    let ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception { image, vector, .. }) =
        executor.poll().unwrap()
    else {
        panic!("expected interrupt boundary");
    };
    assert_eq!(
        image.reason,
        crate::cpu::mips4::exception::Mips4Exception::Interrupt
    );
    assert_eq!(vector, EXCEPTION_VECTOR);
    assert_eq!(executor.target().state().cp0().cause().exception_code(), 0);
}

fn enable_cp1(
    executor: &mut FunctionalExecutor<Mips4ExecutionTarget<TestPolicy, NativeFloatBackend>>,
) {
    complete_instruction(executor, 0x3c01_2440);
    complete_instruction(executor, 0x3421_0004);
    complete_instruction(executor, 0x4081_6000);
}

#[test]
fn unusable_cp1_traps_before_reading_floating_point_state() {
    let mut executor = executor();
    let boundary = complete_instruction(&mut executor, 0x4402_1000);

    let Mips4ExecutionBoundary::Exception { image, .. } = boundary else {
        panic!("expected coprocessor unusable exception");
    };
    assert_eq!(
        image.reason,
        crate::cpu::mips4::exception::Mips4Exception::CoprocessorUnusable {
            coprocessor: crate::cpu::mips4::exception::Mips4CoprocessorNumber::Cp1,
        }
    );
}

#[test]
fn cp1_single_add_uses_transfers_and_commits_result() {
    let mut executor = executor();
    enable_cp1(&mut executor);
    complete_instruction(&mut executor, 0x3c01_3fc0);
    complete_instruction(&mut executor, 0x4481_1000);
    complete_instruction(&mut executor, 0x3c01_4010);
    complete_instruction(&mut executor, 0x4481_2000);
    complete_instruction(&mut executor, 0x4604_1180);
    complete_instruction(&mut executor, 0x4402_3000);

    assert_eq!(
        executor
            .target()
            .state()
            .gpr()
            .read(Mips4GprIndex::from_u8(2).unwrap()),
        3.75f32.to_bits() as u64
    );
    assert!(
        executor
            .target()
            .state()
            .cp1()
            .fcsr()
            .cause_flags()
            .is_empty()
    );
}

#[test]
fn cp1_word_load_uses_the_same_functional_bus_protocol() {
    let mut executor = executor();
    enable_cp1(&mut executor);
    complete_instruction(&mut executor, 0x3c01_8000);

    let data = complete_fetch_for_data(&mut executor, 0xc422_0000);
    assert_eq!(
        data.payload,
        Mips4ExecutionTransaction::Read {
            physical_address: 0,
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::DataLoad,
            access_type: Mips4MemoryAccessType::CachedNoncoherent,
        }
    );
    complete_data_boundary(
        &mut executor,
        data.id,
        Mips4ExecutionCompletion::ReadData(big_endian_word(1.25f32.to_bits())),
    );
    complete_instruction(&mut executor, 0x4402_1000);
    assert_eq!(
        executor
            .target()
            .state()
            .gpr()
            .read(Mips4GprIndex::from_u8(2).unwrap()),
        1.25f32.to_bits() as u64
    );
}

#[test]
fn enabled_cp1_exception_does_not_write_destination() {
    let mut executor = executor();
    enable_cp1(&mut executor);
    complete_instruction(&mut executor, 0x2401_0400);
    complete_instruction(&mut executor, 0x44c1_f800);
    complete_instruction(&mut executor, 0x3c01_3f80);
    complete_instruction(&mut executor, 0x4481_1000);
    complete_instruction(&mut executor, 0x4480_2000);
    let boundary = complete_instruction(&mut executor, 0x4604_1183);

    let Mips4ExecutionBoundary::Exception { image, .. } = boundary else {
        panic!("expected floating-point exception");
    };
    assert_eq!(
        image.reason,
        crate::cpu::mips4::exception::Mips4Exception::FloatingPoint
    );
    assert_eq!(
        executor
            .target()
            .state()
            .cp1()
            .fgr()
            .read_word(crate::cpu::mips4::cp1::Mips4Cp1FgrIndex::from_u8(6).unwrap()),
        0
    );
    assert!(
        executor
            .target()
            .state()
            .cp1()
            .fcsr()
            .cause_flags()
            .contains(se_float::control::FloatExceptionFlags::DIVIDE_BY_ZERO)
    );
}
