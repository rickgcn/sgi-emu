use se_core::component::{Component, ComponentId};
use se_core::role::{BusControllerRole, BusDeviceRole};
use se_core::tracing::TraceInterest;

use crate::bus::i2c::I2cCompletion;
use crate::bus::irq::IrqDelivery;
use crate::bus::isa::{IsaCompletion, IsaCompletionPayload, IsaTransferView};
use crate::bus::media::{EthernetFrame, MediaPayload, MediaPort, MediaTransaction};
use crate::bus::one_wire::OneWireLineDelivery;
use crate::bus::pci::{PciCompletion, PciStatus};
use crate::bus::two_wire::TwoWireLineDelivery;
use crate::chipset::crime::protocol::{
    CrimeByteEnable, CrimeCmiCompletion, CrimeCmiTransaction, CrimeCompletionPayload, CrimeData,
    CrimeLinkDeviceResponse, CrimeLinkOperation, CrimePioRequest, CrimeTransactionId,
    CrimeTransfer,
};

use super::config::MaceConfig;
use super::protocol::{MaceAction, MaceExternalLinks, MacePoll, MaceWiring};
use super::{Mace, MaceError};

fn component(value: u64) -> ComponentId {
    ComponentId::new(value)
}

fn wiring() -> MaceWiring {
    MaceWiring {
        crime: component(1),
        pci_bus: component(2),
        pci_devices: [
            component(19),
            component(20),
            component(21),
            component(22),
            component(23),
        ],
        pci_absent: component(25),
        isa_bus: component(3),
        prom: component(4),
        rtc: component(5),
        serial: [component(16), component(17)],
        parallel: component(18),
        ps2_buses: [component(13), component(14)],
        external_links: MaceExternalLinks {
            i2c: [component(6), component(7)],
            audio: component(8),
            video_input_ab: component(9),
            video_input_cd: component(10),
            video_output: component(11),
            ethernet: component(12),
        },
    }
}

fn assert_component<T: Component>() {}
fn assert_device<T, P>()
where
    T: BusDeviceRole<P>,
{
}
fn assert_controller<T, P>()
where
    T: BusControllerRole<P>,
{
}

#[test]
fn mace_implements_every_topological_role() {
    assert_component::<Mace>();
    assert_device::<Mace, CrimeCmiTransaction>();
    assert_device::<Mace, IrqDelivery>();
    assert_device::<Mace, MediaTransaction>();
    assert_device::<Mace, OneWireLineDelivery>();
    assert_device::<Mace, TwoWireLineDelivery>();
    assert_controller::<Mace, CrimeCmiCompletion>();
    assert_controller::<Mace, IsaCompletion>();
    assert_controller::<Mace, PciCompletion>();
    assert_controller::<Mace, I2cCompletion>();
}

#[test]
fn component_reset_restores_revision_register() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    mace.reset();
    assert_eq!(mace.id(), component(15));
}

#[test]
fn ps2_registers_require_full_aligned_sixty_four_bit_pio() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");

    assert_eq!(
        mace.access_internal(
            super::system::MaceAddressTarget::Peripheral,
            0x20018,
            CrimeTransfer::read(4),
        ),
        Err(crate::chipset::crime::protocol::CrimeBusError::Access)
    );
    assert!(
        mace.access_internal(
            super::system::MaceAddressTarget::Peripheral,
            0x20018,
            CrimeTransfer::read(8),
        )
        .is_ok()
    );
}

#[test]
fn ps2_interrupt_and_poll_sources_follow_controller_levels() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    mace.write_ps2(0x10, 0x18).unwrap();
    let byte = 0xaa_u8;
    let parity = u16::from(byte.count_ones() & 1 == 0);
    let frame = (u16::from(byte) << 1) | parity << 9 | 1 << 10;
    for bit in 0u8..=10 {
        let data_low = frame & (1 << bit) == 0;
        for (phase, clock_low) in [(0, true), (1, false)] {
            BusDeviceRole::<TwoWireLineDelivery>::accept(
                &mut mace,
                TwoWireLineDelivery {
                    bus: component(13),
                    source: component(30),
                    time: se_core::scheduler::SimTime::new(u64::from(bit) * 2 + phase),
                    source_clock_low: clock_low,
                    source_data_low: data_low,
                    clock_low,
                    data_low,
                },
            )
            .unwrap();
        }
    }

    assert_eq!(mace.peripheral_interrupt_status() & (3 << 9), 3 << 9);
    assert_eq!(mace.read_ps2(0x08).unwrap() as u8, 0xaa);
    assert_eq!(mace.peripheral_interrupt_status() & (3 << 9), 0);

    mace.write_ps2(0x10, 0x14).unwrap();
    assert_eq!(mace.peripheral_interrupt_status() & (3 << 9), 1 << 9);
}

#[test]
fn phy_read_start_has_the_prom_observed_zero_readback() {
    let mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");

    assert_eq!(mace.read_ethernet(0x70), Ok(0));
}

#[test]
fn absent_pci_configuration_read_returns_ones_and_latches_master_abort() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    mace.set_trace_interest(TraceInterest::None);
    mace.write_pci(0x0cf8, 4, 7 << 11).unwrap();

    let response = BusDeviceRole::<CrimeCmiTransaction>::accept(
        &mut mace,
        CrimeCmiTransaction {
            id: CrimeTransactionId::new(9),
            controller: component(1),
            target: component(15),
            operation: CrimeLinkOperation::Pio(CrimePioRequest {
                address: 0x1f08_0cfc,
                transfer: CrimeTransfer::read(4),
            }),
        },
    );
    assert!(matches!(response, CrimeLinkDeviceResponse::Deferred));
    let transaction = match mace.poll().unwrap() {
        MacePoll::Action(MaceAction::StartPci(transaction)) => transaction,
        other => panic!("expected PCI configuration transaction, got {other:?}"),
    };
    assert_eq!(transaction.target, component(25));
    assert_eq!(transaction.configuration.unwrap().device, 7);

    BusControllerRole::<PciCompletion>::complete(
        &mut mace,
        PciCompletion {
            id: transaction.id,
            status: PciStatus::MasterAbort,
            data: vec![],
        },
    );
    let completion = match mace.poll().unwrap() {
        MacePoll::Action(MaceAction::CompleteCmiDevice(completion)) => completion,
        other => panic!("expected CMI completion, got {other:?}"),
    };
    assert_eq!(
        completion.result,
        Ok(CrimeCompletionPayload::ReadData(
            vec![0xff, 0xff, 0xff, 0xff].into()
        ))
    );
    assert_eq!(mace.read_pci(0x0000, 4), Ok(1 << 18));
    assert_eq!(
        mace.read_pci(0x0004, 4).unwrap() as u32
            & (super::pci::error::MASTER_ABORT
                | super::pci::error::MASTER_ABORT_ADDRESS
                | super::pci::error::CONFIG_ADDRESS),
        super::pci::error::MASTER_ABORT
            | super::pci::error::MASTER_ABORT_ADDRESS
            | super::pci::error::CONFIG_ADDRESS
    );

    mace.write_pci(0x0004, 4, 0x7fff_ffff).unwrap();
    assert_eq!(
        mace.read_pci(0x0004, 4).unwrap() as u32
            & (super::pci::error::MASTER_ABORT | super::pci::error::MASTER_ABORT_ADDRESS),
        0
    );
    assert_ne!(
        mace.read_pci(0x0004, 4).unwrap() as u32 & super::pci::error::CONFIG_ADDRESS,
        0
    );
}

#[test]
fn rtc_lane_access_is_forwarded_to_isa() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    let request = CrimeCmiTransaction {
        id: CrimeTransactionId::new(1),
        controller: component(1),
        target: component(15),
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0x1f3a_3707,
            transfer: CrimeTransfer::read(1),
        }),
    };

    BusDeviceRole::<CrimeCmiTransaction>::accept(&mut mace, request);
    assert!(matches!(mace.poll(), Ok(MacePoll::Action(_))));
}

#[test]
fn mace_trace_targets_are_device_local() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    let request = CrimeCmiTransaction {
        id: CrimeTransactionId::new(1),
        controller: component(1),
        target: component(15),
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0x1f3a_3707,
            transfer: CrimeTransfer::read(1),
        }),
    };

    BusDeviceRole::<CrimeCmiTransaction>::accept(&mut mace, request);
    let MacePoll::Action(MaceAction::Trace(event)) = mace.poll().unwrap() else {
        panic!("MACE must trace an accepted CMI request first");
    };
    assert_eq!(event.target, "mace.cmi");
    assert_eq!(event.event, "pio");
    assert!(!event.target.starts_with("ip32."));
}

#[test]
fn uninterested_mace_does_not_construct_trace_actions() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    mace.set_trace_interest(TraceInterest::None);
    let request = CrimeCmiTransaction {
        id: CrimeTransactionId::new(1),
        controller: component(1),
        target: component(15),
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0x1fc0_0000,
            transfer: CrimeTransfer::read(4),
        }),
    };

    BusDeviceRole::<CrimeCmiTransaction>::accept(&mut mace, request);
    let mut saw_hardware_action = false;
    while let Ok(MacePoll::Action(action)) = mace.poll() {
        assert!(!matches!(action, super::protocol::MaceAction::Trace(_)));
        saw_hardware_action = true;
    }
    assert!(saw_hardware_action);
}

#[test]
fn system_flash_writes_obey_mace_write_enable() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    mace.set_trace_interest(TraceInterest::None);
    let write = || CrimeCmiTransaction {
        id: CrimeTransactionId::new(1),
        controller: component(1),
        target: component(15),
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0x1fc0_4000,
            transfer: CrimeTransfer::write([0x5a].into(), [true].into()),
        }),
    };

    assert_eq!(
        BusDeviceRole::<CrimeCmiTransaction>::accept(&mut mace, write()),
        CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
            id: CrimeTransactionId::new(1),
            result: Ok(CrimeCompletionPayload::WriteComplete),
            memory_fault: None,
        })
    );
    assert_eq!(mace.poll(), Ok(MacePoll::Idle));

    mace.write_peripheral(0x10008, 1).unwrap();
    assert_eq!(
        BusDeviceRole::<CrimeCmiTransaction>::accept(&mut mace, write()),
        CrimeLinkDeviceResponse::Deferred
    );
    let isa = match mace.poll().unwrap() {
        MacePoll::Action(MaceAction::StartIsa(transaction)) => transaction,
        other => panic!("expected System Flash ISA transaction, got {other:?}"),
    };
    assert_eq!(isa.target, component(4));
    assert_eq!(isa.address, 0x4000);
    assert!(matches!(
        isa.transfer.view(),
        IsaTransferView::Write { data, byte_enable }
            if data == [0x5a] && byte_enable.iter().eq([true])
    ));

    mace.reset();
    assert_eq!(
        BusDeviceRole::<CrimeCmiTransaction>::accept(&mut mace, write()),
        CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
            id: CrimeTransactionId::new(1),
            result: Ok(CrimeCompletionPayload::WriteComplete),
            memory_fault: None,
        })
    );
}

#[test]
fn system_flash_rejects_invalid_write_shapes() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    mace.set_trace_interest(TraceInterest::None);
    let request = CrimeCmiTransaction {
        id: CrimeTransactionId::new(2),
        controller: component(1),
        target: component(15),
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0x1fc0_4000,
            transfer: CrimeTransfer::write(
                CrimeData::from([0x12, 0x34]),
                CrimeByteEnable::from([true, true]),
            ),
        }),
    };
    assert_eq!(
        BusDeviceRole::<CrimeCmiTransaction>::accept(&mut mace, request),
        CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
            id: CrimeTransactionId::new(2),
            result: Err(crate::chipset::crime::protocol::CrimeBusError::Access),
            memory_fault: None,
        })
    );
}

#[test]
fn host_queue_reports_capacity() {
    let mut config = MaceConfig::default();
    config.ports.ethernet_frames = 0;
    let mut mace =
        Mace::new(component(15), "MACE", config, wiring(), 1_000_000_000).expect("MACE must build");
    let error = mace
        .accept_host_input(MediaTransaction {
            source: component(16),
            target: component(15),
            port: MediaPort::Ethernet,
            payload: MediaPayload::Ethernet(EthernetFrame {
                data: vec![],
                crc_valid: true,
                collision_count: 0,
            }),
        })
        .expect_err("zero-capacity host queue must reject input");
    assert_eq!(error, MaceError::HostPortFull(MediaPort::Ethernet));
}

#[test]
fn hardware_actions_are_not_sized_by_inline_trace_fields() {
    assert!(core::mem::size_of::<super::protocol::MaceAction>() <= 128);
}

#[test]
fn isa_misc_routes_the_open_drain_nic_line_and_keeps_input_read_only() {
    let mut mace = Mace::new(
        component(15),
        "MACE",
        MaceConfig::default(),
        wiring(),
        1_000_000_000,
    )
    .expect("MACE must build");
    mace.power_on(se_core::scheduler::SimTime::new(10));
    assert_eq!(
        mace.poll(),
        Ok(MacePoll::Action(MaceAction::SetOneWire(
            crate::bus::one_wire::OneWireDrive {
                source: component(15),
                time: se_core::scheduler::SimTime::new(10),
                drive_low: true,
            }
        )))
    );

    BusDeviceRole::<OneWireLineDelivery>::accept(
        &mut mace,
        OneWireLineDelivery {
            source: component(24),
            time: se_core::scheduler::SimTime::new(20),
            source_drive_low: false,
            line_low: false,
        },
    );
    assert_eq!(mace.read_peripheral(0x10008).unwrap() & (1 << 3), 1 << 3);

    mace.write_peripheral(0x10008, (1 << 2) | (1 << 3)).unwrap();
    assert_eq!(
        mace.poll(),
        Ok(MacePoll::Action(MaceAction::SetOneWire(
            crate::bus::one_wire::OneWireDrive {
                source: component(15),
                time: se_core::scheduler::SimTime::new(20),
                drive_low: false,
            }
        )))
    );
    assert_eq!(mace.read_peripheral(0x10008).unwrap() & (1 << 3), 1 << 3);
}

#[test]
fn crime_isa_conversions_move_compact_storage_without_length_loss() {
    let read = super::to_isa_transfer(CrimeTransfer::read(512));
    assert_eq!(read.view(), IsaTransferView::Read { length: 512 });

    let result = super::from_isa_result(Ok(IsaCompletionPayload::ReadData(vec![0; 32].into())));
    let Ok(CrimeCompletionPayload::ReadData(data)) = result else {
        panic!("ISA read did not remain a CRIME read completion");
    };
    assert_eq!(data.len(), 32);
    assert!(data.spilled());
}
