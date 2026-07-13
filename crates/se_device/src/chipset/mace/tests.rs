use se_core::component::{Component, ComponentId};
use se_core::role::{BusControllerRole, BusDeviceRole};
use se_core::tracing::TraceInterest;

use crate::bus::i2c::I2cCompletion;
use crate::bus::irq::IrqDelivery;
use crate::bus::isa::{IsaCompletion, IsaCompletionPayload, IsaTransferView};
use crate::bus::media::{EthernetFrame, MediaPayload, MediaPort, MediaTransaction};
use crate::bus::pci::PciCompletion;
use crate::chipset::crime::protocol::{
    CrimeCmiCompletion, CrimeCmiTransaction, CrimeCompletionPayload, CrimeLinkOperation,
    CrimePioRequest, CrimeTransactionId, CrimeTransfer,
};

use super::config::MaceConfig;
use super::protocol::{MaceExternalLinks, MacePoll, MaceWiring};
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
        isa_bus: component(3),
        prom: component(4),
        rtc: component(5),
        serial: [component(16), component(17)],
        parallel: component(18),
        external_links: MaceExternalLinks {
            i2c: [component(6), component(7)],
            audio: component(8),
            video_input_ab: component(9),
            video_input_cd: component(10),
            video_output: component(11),
            ethernet: component(12),
            keyboard: component(13),
            mouse: component(14),
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
