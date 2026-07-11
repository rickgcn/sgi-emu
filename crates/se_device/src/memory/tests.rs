use super::*;

const ID: ComponentId = ComponentId::new(1);

#[test]
fn ram_reads_and_writes_physical_byte_lanes() {
    let mut ram = Ram::new(ID, "ram", 16);
    for size in [1_u8, 2, 4, 8] {
        assert_eq!(
            ram.accept(MemoryTransaction::Write {
                offset: 8 - u64::from(size),
                size,
                data: 0x8877_6655_4433_2211,
                byte_enable: u8::MAX,
            }),
            MemoryResponse::WriteComplete
        );
        let mask = if size == 8 {
            u64::MAX
        } else {
            (1_u64 << (size * 8)) - 1
        };
        assert_eq!(
            ram.accept(MemoryTransaction::Read {
                offset: 8 - u64::from(size),
                size,
            }),
            MemoryResponse::ReadData(0x8877_6655_4433_2211 & mask)
        );
    }
}

#[test]
fn ram_honors_byte_enables_and_reset() {
    let mut ram = Ram::new(ID, "ram", 8);
    assert_eq!(
        ram.accept(MemoryTransaction::Write {
            offset: 0,
            size: 4,
            data: 0x4433_2211,
            byte_enable: 0b0101,
        }),
        MemoryResponse::WriteComplete
    );
    assert_eq!(&ram.bytes()[..4], &[0x11, 0, 0x33, 0]);
    ram.reset();
    assert_eq!(ram.bytes(), &[0; 8]);
}

#[test]
fn memory_rejects_invalid_widths_and_out_of_bounds_ranges() {
    let mut ram = Ram::new(ID, "ram", 8);
    for transaction in [
        MemoryTransaction::Read { offset: 0, size: 0 },
        MemoryTransaction::Read { offset: 0, size: 9 },
        MemoryTransaction::Read { offset: 7, size: 2 },
        MemoryTransaction::Write {
            offset: u64::MAX,
            size: 1,
            data: 0,
            byte_enable: 1,
        },
    ] {
        assert_eq!(ram.accept(transaction), MemoryResponse::AccessError);
    }
}

#[test]
fn rom_reads_image_and_rejects_writes_without_changing_reset_state() {
    let image = vec![0x11, 0x22, 0x33, 0x44];
    let mut rom = Rom::new(ID, "rom", image.clone());
    assert_eq!(
        rom.accept(MemoryTransaction::Read { offset: 0, size: 4 }),
        MemoryResponse::ReadData(0x4433_2211)
    );
    assert_eq!(
        rom.accept(MemoryTransaction::Write {
            offset: 0,
            size: 1,
            data: 0xff,
            byte_enable: 1,
        }),
        MemoryResponse::AccessError
    );
    assert_eq!(
        rom.accept(MemoryTransaction::Read { offset: 3, size: 2 }),
        MemoryResponse::AccessError
    );
    rom.reset();
    assert_eq!(rom.bytes(), image);
}
