use super::*;

const RAM_SIZE: u64 = 64 * 1024 * 1024;

#[test]
fn documented_stub_regions_route_first_and_last_bytes() {
    let cases = [
        (
            FRAME_BUFFER_START,
            FRAME_BUFFER_END,
            Ip32PhysicalRegion::FrameBuffer,
            component_ids::CRIME,
        ),
        (
            DEPTH_BUFFER_START,
            DEPTH_BUFFER_END,
            Ip32PhysicalRegion::DepthBuffer,
            component_ids::CRIME,
        ),
        (
            CRIME_REGISTERS_START,
            CRIME_REGISTERS_END,
            Ip32PhysicalRegion::CrimeRegisters,
            component_ids::CRIME,
        ),
        (
            GBE_START,
            GBE_END,
            Ip32PhysicalRegion::GbeRegisters,
            component_ids::GBE,
        ),
        (
            VICE_START,
            VICE_END,
            Ip32PhysicalRegion::Vice,
            component_ids::VICE,
        ),
        (
            PCI_LOW_IO_START,
            PCI_LOW_IO_END,
            Ip32PhysicalRegion::PciLowIo,
            component_ids::MACE,
        ),
        (
            PCI_LOW_MEMORY_START,
            PCI_LOW_MEMORY_END,
            Ip32PhysicalRegion::PciLowMemory,
            component_ids::MACE,
        ),
        (
            PCI_CONFIG_START,
            PCI_CONFIG_END,
            Ip32PhysicalRegion::PciConfiguration,
            component_ids::MACE,
        ),
        (
            MACE_RESERVED_START,
            MACE_RESERVED_END,
            Ip32PhysicalRegion::MaceReserved,
            component_ids::MACE,
        ),
        (
            MACE_IO_START,
            MACE_IO_END,
            Ip32PhysicalRegion::MaceIo,
            component_ids::MACE,
        ),
        (
            MACE_AV_START,
            MACE_AV_END,
            Ip32PhysicalRegion::MaceAv,
            component_ids::MACE,
        ),
        (
            MACE_SUPER_IO_START,
            MACE_SUPER_IO_END,
            Ip32PhysicalRegion::MaceSuperIo,
            component_ids::MACE,
        ),
        (
            PCI_HIGH_IO_START,
            PCI_HIGH_IO_END,
            Ip32PhysicalRegion::PciHighIo,
            component_ids::MACE,
        ),
        (
            PCI_HIGH_MEMORY_START,
            PCI_HIGH_MEMORY_END,
            Ip32PhysicalRegion::PciHighMemory,
            component_ids::MACE,
        ),
    ];

    for (start, end, region, target) in cases {
        for address in [start, end - 1] {
            assert_eq!(
                resolve(address, 1, RAM_SIZE),
                Ip32AddressResolution::Stub { region, target }
            );
        }
    }
}

#[test]
fn ram_aliases_resolve_to_the_same_offsets() {
    for offset in [0, 0x1234, RAM_SIZE - 8] {
        for (start, region, no_ecc) in [
            (LOW_MEMORY_START, Ip32PhysicalRegion::LowMemory, false),
            (LINEAR_MEMORY_START, Ip32PhysicalRegion::LinearMemory, false),
            (NO_ECC_MEMORY_START, Ip32PhysicalRegion::NoEccMemory, true),
        ] {
            assert_eq!(
                resolve(start + offset, 8, RAM_SIZE),
                Ip32AddressResolution::Memory {
                    region,
                    target: component_ids::RAM,
                    offset,
                    no_ecc,
                }
            );
        }
    }
}

#[test]
fn ram_capacity_and_region_boundaries_cover_the_complete_transfer() {
    for start in [LOW_MEMORY_START, LINEAR_MEMORY_START, NO_ECC_MEMORY_START] {
        assert!(matches!(
            resolve(start + RAM_SIZE - 4, 8, RAM_SIZE),
            Ip32AddressResolution::Unmapped { .. }
        ));
        assert!(matches!(
            resolve(start + RAM_SIZE, 1, RAM_SIZE),
            Ip32AddressResolution::Unmapped { .. }
        ));
    }

    assert_eq!(
        resolve(FRAME_BUFFER_END - 4, 8, RAM_SIZE),
        Ip32AddressResolution::Unmapped {
            region: Some(Ip32PhysicalRegion::FrameBuffer)
        }
    );
}

#[test]
fn prom_window_mirrors_the_fixed_image_eight_times() {
    for mirror in 0..8_u64 {
        let address = PROM_START + mirror * IP32_PROM_IMAGE_SIZE_BYTES as u64 + 0x1234;
        assert_eq!(
            resolve(address, 4, RAM_SIZE),
            Ip32AddressResolution::Memory {
                region: Ip32PhysicalRegion::SystemRom,
                target: component_ids::PROM,
                offset: 0x1234,
                no_ecc: false,
            }
        );
    }

    assert!(matches!(
        resolve(
            PROM_START + IP32_PROM_IMAGE_SIZE_BYTES as u64 - 4,
            8,
            RAM_SIZE
        ),
        Ip32AddressResolution::Unmapped {
            region: Some(Ip32PhysicalRegion::SystemRom)
        }
    ));
}

#[test]
fn uncertain_and_out_of_range_addresses_are_unmapped() {
    assert_eq!(
        resolve(HIGH_MEMORY_START, 4, RAM_SIZE),
        Ip32AddressResolution::Unmapped {
            region: Some(Ip32PhysicalRegion::HighMemoryUnconfirmed)
        }
    );
    for address in [0xc000_0000, PCI_HIGH_MEMORY_END, 1_u64 << 40] {
        assert_eq!(
            resolve(address, 4, RAM_SIZE),
            Ip32AddressResolution::Unmapped { region: None }
        );
    }
    assert_eq!(
        resolve(u64::MAX - 3, 8, RAM_SIZE),
        Ip32AddressResolution::Unmapped { region: None }
    );
}
