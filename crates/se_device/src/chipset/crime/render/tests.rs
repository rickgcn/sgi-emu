use super::*;
use crate::chipset::crime::protocol::CrimeTransferView;

const PROM_CLEAR_MODE: u32 = 0x0000_0011;
const PIXEL_PIPE_FLUSH: u64 = PIXEL_PIPE_BASE + 0x1f8;

fn write_completion() -> Result<CrimeMemoryOutcome, CrimeBusError> {
    Ok(CrimeMemoryOutcome::new(
        CrimeCompletionPayload::WriteComplete,
        None,
        None,
    ))
}

fn retire(render: &mut CrimeRender) -> RenderProgress {
    render.step().unwrap()
}

fn queue_and_retire(render: &mut CrimeRender, address: u64, size: u8, value: u64) {
    render.write(address, size, value).unwrap();
    retire(render);
}

fn configure_prom_clear(render: &mut CrimeRender, linear_a: u64, start: u32, end: u32) {
    queue_and_retire(render, LINEAR_A_BASE, 8, linear_a);
    configure_zero_clear_destination(render, start, end);
}

fn configure_zero_clear_destination(render: &mut CrimeRender, start: u32, end: u32) {
    queue_and_retire(render, MTE_BASE + 0x08, 4, u32::MAX.into());
    queue_and_retire(render, MTE_BASE + 0x18, 4, 0);
    queue_and_retire(render, MTE_BASE + 0x30, 4, start.into());
    queue_and_retire(render, MTE_BASE + 0x38, 4, end.into());
}

fn configure_x_line(render: &mut CrimeRender, color: u32, x0: u16, y0: u16, x1: u16, y1: u16) {
    queue_and_retire(render, PIXEL_PIPE_BASE, 4, RGBA32_FRAMEBUFFER_B_MODE.into());
    queue_and_retire(
        render,
        PIXEL_PIPE_BASE + 0x008,
        4,
        RGBA32_FRAMEBUFFER_B_MODE.into(),
    );
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x018, 4, X_LINE_DRAW_MODE.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x060, 4, X_LINE_PRIMITIVE.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x0d0, 4, color.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x1b0, 4, LOGIC_COPY.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x1b8, 4, u32::MAX.into());
    queue_and_retire(
        render,
        PIXEL_PIPE_BASE + 0x070,
        4,
        (u32::from(x0) << 16 | u32::from(y0)).into(),
    );
    queue_and_retire(
        render,
        PIXEL_PIPE_BASE + 0x074,
        4,
        (u32::from(x1) << 16 | u32::from(y1)).into(),
    );
}

fn configure_prom_ci8_zero_rectangle(render: &mut CrimeRender, x1: u16, y1: u16) {
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x0d0, 4, 0);
    queue_and_retire(
        render,
        PIXEL_PIPE_BASE + 0x018,
        4,
        PROM_CI8_ZERO_RECTANGLE_DRAW_MODE.into(),
    );
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x1b8, 4, u32::MAX.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x070, 4, 0);
    queue_and_retire(
        render,
        PIXEL_PIPE_BASE + 0x074,
        4,
        (u32::from(x1) << 16 | u32::from(y1)).into(),
    );
    queue_and_retire(
        render,
        PIXEL_PIPE_BASE + 0x060,
        4,
        PROM_CI8_RECTANGLE_PRIMITIVE.into(),
    );
}

#[test]
fn interface_buffer_enforces_sixty_four_entry_capacity() {
    let mut render = CrimeRender::new();
    for _ in 0..64 {
        render.write(PIXEL_PIPE_BASE, 4, 0).unwrap();
    }
    assert_eq!(render.interface_level(), 64);
    assert_eq!(
        render.write(PIXEL_PIPE_BASE, 4, 0),
        Err(RenderWriteError::InterfaceFull)
    );

    retire(&mut render);
    assert!(render.write(PIXEL_PIPE_BASE, 4, 0).is_ok());
}

#[test]
fn interface_ram_preserves_big_endian_data_lanes_and_address_metadata() {
    let mut render = CrimeRender::new();
    render
        .write(PIXEL_PIPE_BASE + 0x074, 4, 0x1122_3344)
        .unwrap();
    render.write(MTE_BASE + 0x030, 4, 0x5566_7788).unwrap();
    render.write(TLB_BASE, 8, 0x99aa_bbcc_ddee_ff00).unwrap();

    assert_eq!(render.interface.data[0], 0x0000_0000_1122_3344);
    assert_eq!(render.interface.data[1], 0x5566_7788_0000_0000);
    assert_eq!(render.interface.data[2], 0x99aa_bbcc_ddee_ff00);
    assert_eq!(
        (render.interface.address[0] >> ADDRESS_WMASK_SHIFT) & 3,
        ADDRESS_WMASK_UPPER
    );
    assert_eq!(
        (render.interface.address[1] >> ADDRESS_WMASK_SHIFT) & 3,
        ADDRESS_WMASK_LOWER
    );
    assert_eq!(
        (render.interface.address[2] >> ADDRESS_WMASK_SHIFT) & 3,
        ADDRESS_WMASK_DOUBLE
    );
    assert_eq!(
        render.read(INTERFACE_DATA_BASE, 8),
        Ok(render.interface.data[0])
    );
    assert_eq!(
        render.read(INTERFACE_ADDRESS_BASE + 8, 8),
        Ok(render.interface.address[1])
    );
}

#[test]
fn set_start_pointer_records_the_current_uncommitted_fifo_boundary() {
    let mut render = CrimeRender::new();
    render.write(PIXEL_PIPE_BASE, 4, 1).unwrap();
    render.write(PIXEL_PIPE_BASE + 0x008, 4, 2).unwrap();
    assert_eq!(render.interface.write_pointer, 2);
    assert_eq!(render.interface.start_pointer, 0);

    render.write(SET_START_POINTER, 4, 0).unwrap();
    assert_eq!(render.interface.start_pointer, 2);
    assert_eq!(render.status() & 0x3f, 2);

    render
        .write(MTE_BASE + START_OFFSET, 4, PROM_CLEAR_MODE.into())
        .unwrap();
    assert_eq!(render.interface.write_pointer, 3);
    assert_eq!(render.interface.start_pointer, 3);
    assert_ne!(render.interface.address[2] & ADDRESS_START, 0);
}

#[test]
fn programmed_fifo_thresholds_and_stall_cycles_drive_acceptance() {
    let mut render = CrimeRender::new();
    let control = (48_u64 << INTERFACE_FULL_SHIFT)
        | (24_u64 << INTERFACE_EMPTY_SHIFT)
        | (47_u64 << INTERFACE_STALL_LEVEL_SHIFT)
        | 10;
    render.write(INTERFACE_CONTROL, 4, control).unwrap();

    let mut empty_deasserted = false;
    let mut full_asserted = false;
    for index in 0..48 {
        let progress = render.write(PIXEL_PIPE_BASE, 4, index).unwrap();
        empty_deasserted |= progress.interrupts.contains(&RenderInterruptEffect {
            mask: registers::INTERRUPT_RE_EMPTY_LEVEL,
            asserted: false,
        });
        full_asserted |= progress.interrupts.contains(&RenderInterruptEffect {
            mask: registers::INTERRUPT_RE_FULL_LEVEL,
            asserted: true,
        });
    }
    assert!(empty_deasserted);
    assert!(full_asserted);
    assert_eq!(render.interface.stall_cycles, 10);
    assert!(!render.has_interface_space());

    for _ in 0..9 {
        retire(&mut render);
        assert!(!render.has_interface_space());
    }
    retire(&mut render);
    assert!(render.has_interface_space());
    assert_eq!(render.interface_level(), 38);
}

#[test]
fn interface_control_is_immediate_and_interface_reset_clears_pending_entries() {
    let mut render = CrimeRender::new();
    assert_eq!(render.read(INTERFACE_CONTROL, 4), Ok(0));
    assert_eq!(
        render.read(INTERFACE_CONTROL, 8),
        Err(RenderAccessError::Access)
    );

    render.write(INTERFACE_CONTROL, 4, 1).unwrap();
    assert_eq!(render.interface_level(), 0);
    assert_eq!(render.interface.control, 1);
    assert_eq!(render.read(INTERFACE_CONTROL, 4), Ok(1));

    render.write(PIXEL_PIPE_BASE, 4, 0x1234_5678).unwrap();
    assert_eq!(render.interface_level(), 1);
    assert_ne!(render.interface.data[0], 0);

    render.write(INTERFACE_RESET, 4, 0).unwrap();
    assert_eq!(render.interface_level(), 0);
    assert_eq!(render.interface.read_pointer, 0);
    assert_eq!(render.interface.write_pointer, 0);
    assert_eq!(render.interface.start_pointer, 0);
    assert_eq!(render.interface.data, [0; INTERFACE_CAPACITY]);
    assert_eq!(
        render.read(INTERFACE_RESET, 4),
        Err(RenderAccessError::Unsupported)
    );
}

#[test]
fn window_offsets_accept_the_prom_doubleword_initialization() {
    let mut render = CrimeRender::new();
    for offset in [0x50, 0x58] {
        queue_and_retire(
            &mut render,
            PIXEL_PIPE_BASE + offset,
            8,
            0x1234_5678_9abc_def0,
        );
        assert_eq!(
            render.read(PIXEL_PIPE_BASE + offset, 8),
            Err(RenderAccessError::Access)
        );
        assert_eq!(render.read(PIXEL_PIPE_BASE + offset, 4), Ok(0x1234_5678));
    }
}

#[test]
fn pixel_dma_pair_writes_use_only_the_traced_doubleword_whitelist() {
    let mut render = CrimeRender::new();
    for (offset, value) in [
        (0x070, 0x0001_0002_0003_0004),
        (0x080, 0x0005_0006_0007_0008),
        (0x088, 0x0009_000a_000b_000c),
    ] {
        queue_and_retire(&mut render, PIXEL_PIPE_BASE + offset, 8, value);
        assert_eq!(
            render.pixel.slots[(offset / 8) as usize],
            value,
            "doubleword write at offset {offset:#x}"
        );
    }
    assert_eq!(
        render.write(PIXEL_PIPE_BASE + 0x090, 8, 0),
        Err(RenderWriteError::Access(RenderAccessError::Access))
    );
}

#[test]
fn tagged_mte_write_is_canonicalized_and_tagged_reads_are_rejected() {
    let mut render = CrimeRender::new();
    render
        .write(MTE_BASE + START_OFFSET, 4, PROM_CLEAR_MODE.into())
        .unwrap();
    assert_eq!(
        decode_interface_entry(render.interface.data[0], render.interface.address[0]),
        RenderRegisterWrite {
            address: MTE_BASE,
            value: PROM_CLEAR_MODE.into(),
            size: 4,
            commit: true,
        }
    );
    assert_eq!(
        render.read(MTE_BASE + START_OFFSET, 4),
        Err(RenderAccessError::Unsupported)
    );
    assert!(render.write(PIXEL_PIPE_BASE + START_OFFSET, 4, 0).is_ok());
}

#[test]
fn pixel_start_aliases_submit_the_frozen_primitive_state() {
    let mut render = CrimeRender::new();
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();
    assert_eq!(
        render.step(),
        Err(CrimeRenderError::UnsupportedPixelCommand {
            trigger_address: PIXEL_PIPE_NULL,
            primitive: 0,
            draw_mode: 0,
        })
    );
    assert!(matches!(
        render.active_pixel_command,
        Some(PixelExecution::Unsupported(_))
    ));

    let mut flush = CrimeRender::new();
    flush.write(PIXEL_PIPE_FLUSH, 4, 0).unwrap();
    let progress = retire(&mut flush);
    assert!(progress.memory_request.is_none());
    assert!(flush.active_pixel_command.is_none());
}

#[test]
fn tagged_mte_null_and_flush_retire_without_starting_a_transfer() {
    let mut render = CrimeRender::new();
    for offset in [0x70, 0x78] {
        render
            .write(MTE_BASE + offset + START_OFFSET, 4, 0)
            .unwrap();
        let progress = retire(&mut render);
        assert!(progress.memory_request.is_none());
        assert!(render.active_job.is_none());
    }
}

#[test]
fn unsupported_pixel_start_preserves_an_immutable_command_snapshot() {
    let mut render = CrimeRender::new();
    let initial_draw_mode = 0x0012_3400_u32;
    let primitive = 0x0200_0020_u32;
    render
        .write(PIXEL_PIPE_BASE + 0x018, 4, initial_draw_mode.into())
        .unwrap();
    render
        .write(PIXEL_PIPE_BASE + 0x060 + START_OFFSET, 4, primitive.into())
        .unwrap();
    render
        .write(PIXEL_PIPE_BASE + 0x018, 4, 0x00ff_ffff)
        .unwrap();

    retire(&mut render);
    let error = render.step().unwrap_err();
    assert_eq!(
        error,
        CrimeRenderError::UnsupportedPixelCommand {
            trigger_address: PIXEL_PIPE_BASE + 0x060,
            primitive,
            draw_mode: initial_draw_mode,
        }
    );
    let Some(PixelExecution::Unsupported(snapshot)) = render.active_pixel_command.as_ref() else {
        panic!("unsupported command snapshot was not retained")
    };
    assert_eq!(snapshot.primitive(), primitive);
    assert_eq!(snapshot.draw_mode(), initial_draw_mode);
    assert_eq!(render.interface_level(), 1);
    assert_eq!(render.status() & STATUS_PIXEL_PIPE_IDLE, 0);
}

#[test]
fn diagnostic_x_line_stream_writes_big_endian_rgba_through_framebuffer_b() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_B_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 1) << 48,
    );
    configure_x_line(&mut render, 0x1122_3344, 0, 0, 7, 0);
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let progress = retire(&mut render);
    assert!(
        progress
            .notices
            .contains(&RenderNotice::PixelCommandCommitted {
                primitive: X_LINE_PRIMITIVE,
                x0: 0,
                y0: 0,
                x1: 7,
                y1: 0,
            })
    );
    let request = progress.memory_request.unwrap();
    assert_eq!(request.destination, RenderMemoryDestination::Pixel);
    assert_eq!(request.virtual_address, 0);
    assert_eq!(request.raw_entry, u32::from(FRAMEBUFFER_TLB_VALID | 1));
    assert_eq!(request.alias_address, FRAMEBUFFER_TILE_BYTES);
    assert_eq!(request.physical_address, FRAMEBUFFER_TILE_BYTES);
    assert!(request.valid);
    assert_eq!(request.transfer.length(), RENDER_MEMORY_WORD_BYTES);
    let CrimeTransferView::Write { data, byte_enable } = request.transfer.view() else {
        panic!("X line emitted a read request")
    };
    assert_eq!(data, [0x11, 0x22, 0x33, 0x44].repeat(8));
    assert!(byte_enable.iter().all(|enabled| enabled));

    render.complete_memory(write_completion()).unwrap();
    let completed = retire(&mut render);
    assert!(
        completed
            .notices
            .contains(&RenderNotice::PixelCommandCompleted {
                primitive: X_LINE_PRIMITIVE,
                x0: 0,
                y0: 0,
                x1: 7,
                y1: 0,
            })
    );
    assert!(render.active_pixel_command.is_none());
    assert_ne!(render.status() & STATUS_PIXEL_PIPE_IDLE, 0);
}

#[test]
fn vertical_x_line_uses_one_enabled_pixel_per_memory_word() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_B_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 2) << 48,
    );
    configure_x_line(&mut render, 0xaabb_ccdd, 1, 0, 1, 1);
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let first = retire(&mut render).memory_request.unwrap();
    assert_eq!(first.alias_address, 2 * FRAMEBUFFER_TILE_BYTES);
    let CrimeTransferView::Write { data, byte_enable } = first.transfer.view() else {
        panic!("vertical X line emitted a read request")
    };
    assert_eq!(&data[4..8], &[0xaa, 0xbb, 0xcc, 0xdd]);
    assert_eq!(byte_enable.iter().filter(|enabled| *enabled).count(), 4);
    assert!((4..8).all(|lane| byte_enable.is_enabled(lane) == Some(true)));

    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_request.unwrap();
    assert_eq!(second.virtual_address, 1_u32 << 16 | 1);
    assert_eq!(
        second.alias_address,
        2 * FRAMEBUFFER_TILE_BYTES + FRAMEBUFFER_TILE_ROW_BYTES
    );
    render.complete_memory(write_completion()).unwrap();
    retire(&mut render);
    assert!(render.active_pixel_command.is_none());
}

#[test]
fn in_flight_x_line_round_trips_with_pixel_progress() {
    let mut reference = CrimeRender::new();
    queue_and_retire(
        &mut reference,
        FRAMEBUFFER_B_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 3) << 48,
    );
    configure_x_line(&mut reference, 0x1020_3040, 0, 0, 15, 0);
    reference
        .write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0)
        .unwrap();
    let first = retire(&mut reference).memory_request.unwrap();
    assert_eq!(first.transfer.length(), RENDER_MEMORY_WORD_BYTES);

    let encoded = postcard::to_stdvec(&reference).unwrap();
    let mut restored: CrimeRender = postcard::from_bytes(&encoded).unwrap();
    assert_eq!(restored, reference);

    assert_eq!(
        restored.complete_memory(write_completion()),
        reference.complete_memory(write_completion())
    );
    assert_eq!(restored, reference);
    assert_eq!(retire(&mut restored), retire(&mut reference));
    assert_eq!(restored, reference);
}

#[test]
fn prom_ci8_zero_rectangle_walks_inclusive_rows_through_framebuffer_a() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 4) << 48,
    );
    configure_prom_ci8_zero_rectangle(&mut render, 63, 1);
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let committed = retire(&mut render);
    assert!(
        committed
            .notices
            .contains(&RenderNotice::PixelCommandCommitted {
                primitive: PROM_CI8_RECTANGLE_PRIMITIVE,
                x0: 0,
                y0: 0,
                x1: 63,
                y1: 1,
            })
    );
    let first = committed.memory_request.unwrap();
    assert_eq!(first.destination, RenderMemoryDestination::Pixel);
    assert_eq!(first.virtual_address, 0);
    assert_eq!(first.raw_entry, u32::from(FRAMEBUFFER_TLB_VALID | 4));
    assert_eq!(first.alias_address, 4 * FRAMEBUFFER_TILE_BYTES);
    assert_eq!(first.physical_address, 4 * FRAMEBUFFER_TILE_BYTES);
    assert_eq!(first.transfer.length(), RENDER_MEMORY_WORD_BYTES);
    let CrimeTransferView::Write { data, byte_enable } = first.transfer.view() else {
        panic!("PROM rectangle emitted a read request")
    };
    assert_eq!(data, vec![0; RENDER_MEMORY_WORD_BYTES]);
    assert!(byte_enable.iter().all(|enabled| enabled));

    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_request.unwrap();
    assert_eq!(second.virtual_address, 32);
    assert_eq!(second.alias_address, 4 * FRAMEBUFFER_TILE_BYTES + 32);

    render.complete_memory(write_completion()).unwrap();
    let third = retire(&mut render).memory_request.unwrap();
    assert_eq!(third.virtual_address, 1 << 16);
    assert_eq!(
        third.alias_address,
        4 * FRAMEBUFFER_TILE_BYTES + FRAMEBUFFER_TILE_ROW_BYTES
    );

    render.complete_memory(write_completion()).unwrap();
    let fourth = retire(&mut render).memory_request.unwrap();
    assert_eq!(fourth.virtual_address, 1 << 16 | 32);
    assert_eq!(
        fourth.alias_address,
        4 * FRAMEBUFFER_TILE_BYTES + FRAMEBUFFER_TILE_ROW_BYTES + 32
    );

    render.complete_memory(write_completion()).unwrap();
    let completed = retire(&mut render);
    assert!(
        completed
            .notices
            .contains(&RenderNotice::PixelCommandCompleted {
                primitive: PROM_CI8_RECTANGLE_PRIMITIVE,
                x0: 0,
                y0: 0,
                x1: 63,
                y1: 1,
            })
    );
    assert!(render.active_pixel_command.is_none());
}

#[test]
fn prom_ci8_zero_rectangle_switches_tlb_entries_at_the_tile_column() {
    let mut render = CrimeRender::new();
    let first_entry = FRAMEBUFFER_TLB_VALID | 1;
    let second_entry = FRAMEBUFFER_TLB_VALID | 7;
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(first_entry) << 48 | u64::from(second_entry) << 32,
    );
    configure_prom_ci8_zero_rectangle(&mut render, 543, 1);
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let mut request = retire(&mut render).memory_request.unwrap();
    for _ in 0..16 {
        render.complete_memory(write_completion()).unwrap();
        request = retire(&mut render).memory_request.unwrap();
    }
    assert_eq!(request.virtual_address, 512);
    assert_eq!(request.raw_entry, u32::from(second_entry));
    assert_eq!(request.alias_address, 7 * FRAMEBUFFER_TILE_BYTES);
}

#[test]
fn prom_ci8_flat_rectangle_uses_the_low_foreground_byte() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 3) << 48,
    );
    for (offset, value) in [
        (0x0d0, 0x1234_56a5),
        (0x018, PROM_CI8_FLAT_RECTANGLE_DRAW_MODE),
        (0x1b0, LOGIC_COPY),
        (0x1b8, u32::MAX),
        (0x070, 3 << 16 | 2),
        (0x074, 35 << 16 | 2),
        (0x060, PROM_CI8_RECTANGLE_PRIMITIVE),
    ] {
        queue_and_retire(&mut render, PIXEL_PIPE_BASE + offset, 4, value.into());
    }
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let first = retire(&mut render).memory_request.unwrap();
    assert_eq!(first.virtual_address, 2 << 16 | 3);
    assert_eq!(
        first.alias_address,
        3 * FRAMEBUFFER_TILE_BYTES + 2 * FRAMEBUFFER_TILE_ROW_BYTES
    );
    let CrimeTransferView::Write { data, byte_enable } = first.transfer.view() else {
        panic!("PROM flat rectangle emitted a read request")
    };
    assert!(data[..3].iter().all(|value| *value == 0));
    assert!(data[3..].iter().all(|value| *value == 0xa5));
    assert!(byte_enable.iter().take(3).all(|enabled| !enabled));
    assert!(byte_enable.iter().skip(3).all(|enabled| enabled));

    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_request.unwrap();
    assert_eq!(second.virtual_address, 2 << 16 | 32);
    let CrimeTransferView::Write { data, byte_enable } = second.transfer.view() else {
        panic!("PROM flat rectangle emitted a read request")
    };
    assert!(data[..4].iter().all(|value| *value == 0xa5));
    assert!(data[4..].iter().all(|value| *value == 0));
    assert!(byte_enable.iter().take(4).all(|enabled| enabled));
    assert!(byte_enable.iter().skip(4).all(|enabled| !enabled));
}

#[test]
fn prom_zero_rectangle_rejects_nonzero_foreground() {
    let mut render = CrimeRender::new();
    configure_prom_ci8_zero_rectangle(&mut render, 31, 1);
    queue_and_retire(&mut render, PIXEL_PIPE_BASE + 0x0d0, 4, 1);
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let error = render.step().unwrap_err();
    assert_eq!(
        error,
        CrimeRenderError::UnsupportedPixelCommand {
            trigger_address: PIXEL_PIPE_NULL,
            primitive: PROM_CI8_RECTANGLE_PRIMITIVE,
            draw_mode: PROM_CI8_ZERO_RECTANGLE_DRAW_MODE,
        }
    );
}

#[test]
fn in_flight_prom_zero_rectangle_round_trips_with_row_progress() {
    let mut reference = CrimeRender::new();
    queue_and_retire(
        &mut reference,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 5) << 48,
    );
    configure_prom_ci8_zero_rectangle(&mut reference, 63, 1);
    reference
        .write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0)
        .unwrap();
    retire(&mut reference);

    let encoded = postcard::to_stdvec(&reference).unwrap();
    let mut restored: CrimeRender = postcard::from_bytes(&encoded).unwrap();
    assert_eq!(restored, reference);
    assert_eq!(
        restored.complete_memory(write_completion()),
        reference.complete_memory(write_completion())
    );
    assert_eq!(restored, reference);
    assert_eq!(retire(&mut restored), retire(&mut reference));
    assert_eq!(restored, reference);
}

#[test]
fn status_tracks_fifo_pointers_and_idle_blocks() {
    let mut render = CrimeRender::new();
    assert_eq!(
        render.status(),
        STATUS_IDLE | STATUS_SETUP_IDLE | STATUS_PIXEL_PIPE_IDLE | STATUS_MTE_IDLE
    );

    render.write(PIXEL_PIPE_BASE, 4, 0).unwrap();
    assert_eq!(
        render.status(),
        STATUS_PIXEL_PIPE_IDLE
            | STATUS_MTE_IDLE
            | (1 << STATUS_LEVEL_SHIFT)
            | (1 << STATUS_WRITE_POINTER_SHIFT)
    );

    retire(&mut render);
    assert_eq!(
        render.status(),
        STATUS_IDLE
            | STATUS_SETUP_IDLE
            | STATUS_PIXEL_PIPE_IDLE
            | STATUS_MTE_IDLE
            | (1 << STATUS_READ_POINTER_SHIFT)
            | (1 << STATUS_WRITE_POINTER_SHIFT)
    );
}

#[test]
fn linear_a_uses_high_then_low_entries_and_splits_at_page_boundaries() {
    let mut render = CrimeRender::new();
    let even_entry = 0x8000_0002_u32;
    let odd_entry = 0x8000_0005_u32;
    configure_prom_clear(
        &mut render,
        u64::from(even_entry) << 32 | u64::from(odd_entry),
        0x0ff0,
        0x1010,
    );

    render
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    let first = retire(&mut render).memory_request.unwrap();
    assert_eq!(first.virtual_address, 0x0ff0);
    assert_eq!(first.physical_address, 0x2ff0);
    let CrimeTransferView::Write { data, byte_enable } = first.transfer.view() else {
        panic!("MTE clear emitted a read request")
    };
    assert_eq!(data, vec![0; 16]);
    assert!(byte_enable.iter().all(|enabled| enabled));

    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_request.unwrap();
    assert_eq!(second.virtual_address, 0x1000);
    assert_eq!(second.physical_address, 0x5000);
    assert_eq!(second.transfer.length(), 17);
}

#[test]
fn mte_chunks_are_bounded_to_five_hundred_twelve_bytes() {
    let mut render = CrimeRender::new();
    configure_prom_clear(&mut render, u64::from(0x8000_0001_u32) << 32, 0, 1023);
    render
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();

    let first = retire(&mut render).memory_request.unwrap();
    assert_eq!(first.physical_address, 0x1000);
    assert_eq!(first.transfer.length(), 512);
    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_request.unwrap();
    assert_eq!(second.physical_address, 0x1200);
    assert_eq!(second.transfer.length(), 512);
}

#[test]
fn mte_flush_waits_for_the_outstanding_memory_completion() {
    let mut render = CrimeRender::new();
    configure_prom_clear(&mut render, u64::from(0x8000_0001_u32) << 32, 0, 0);
    render
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    let request = retire(&mut render).memory_request.unwrap();
    assert_eq!(request.transfer.length(), 1);

    render.write(MTE_BASE + 0x078 + START_OFFSET, 4, 0).unwrap();
    let blocked = retire(&mut render);
    assert!(blocked.memory_request.is_none());
    assert_eq!(render.interface_level(), 1);

    render.complete_memory(write_completion()).unwrap();
    let completed = retire(&mut render);
    assert!(
        completed
            .notices
            .contains(&RenderNotice::JobCompleted { start: 0, end: 0 })
    );
    assert_eq!(render.interface_level(), 1);

    let flushed = retire(&mut render);
    assert!(flushed.memory_request.is_none());
    assert_eq!(render.interface_level(), 0);
}

#[test]
fn linear_a_uses_the_valid_bit_and_nineteen_page_bits() {
    let mut invalid = CrimeRender::new();
    configure_prom_clear(&mut invalid, 0, 0, 0);
    invalid
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    let write = invalid.step().unwrap().memory_request.unwrap();
    assert_eq!(write.alias_address, 0);
    assert_eq!(write.physical_address, 0);
    assert_eq!(
        write.bank_select,
        CrimeMemoryBankSelect::Inhibited {
            reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
        }
    );

    let mut linear_alias = CrimeRender::new();
    configure_prom_clear(&mut linear_alias, u64::from(0x8004_0001_u32) << 32, 0, 0);
    linear_alias
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    let write = linear_alias.step().unwrap().memory_request.unwrap();
    assert_eq!(write.alias_address, 0x4000_1000);
    assert_eq!(write.physical_address, 0x1000);
    assert_eq!(write.bank_select, CrimeMemoryBankSelect::Decode);

    let mut reserved_bits = CrimeRender::new();
    configure_prom_clear(&mut reserved_bits, u64::from(u32::MAX) << 32, 0, 0);
    reserved_bits
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    let write = reserved_bits.step().unwrap().memory_request.unwrap();
    assert_eq!(write.alias_address, 0x7fff_f000);
    assert_eq!(write.physical_address, 0x3fff_f000);
    assert_eq!(write.bank_select, CrimeMemoryBankSelect::Decode);
}

#[test]
fn linear_a_virtual_page_index_wraps_after_thirty_two_pages() {
    let mut render = CrimeRender::new();
    configure_prom_clear(
        &mut render,
        u64::from(0x8000_0003_u32) << 32,
        0x0002_0000,
        0x0002_0000,
    );
    render
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();

    let write = retire(&mut render).memory_request.unwrap();
    assert_eq!(write.virtual_address, 0x0002_0000);
    assert_eq!(write.physical_address, 0x3000);
}

#[test]
fn linear_b_clear_uses_its_own_tlb_and_snapshots_ecc_control() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        LINEAR_B_BASE,
        8,
        u64::from(0x8000_0007_u32) << 32,
    );
    configure_zero_clear_destination(&mut render, 0x20, 0x2f);
    render.write(MTE_BASE + START_OFFSET, 4, 5 << 2).unwrap();

    let request = retire(&mut render).memory_request.unwrap();
    assert_eq!(request.virtual_address, 0x20);
    assert_eq!(request.raw_entry, 0x8000_0007);
    assert_eq!(request.physical_address, 0x7020);
    assert_eq!(request.transfer.length(), 16);
    assert!(request.no_ecc);
}

#[test]
fn framebuffer_clear_translates_all_three_tile_depths() {
    let cases = [
        (FRAMEBUFFER_A_BASE, 0_u32, 1_u16, 3_u16, 4_u16),
        (FRAMEBUFFER_B_BASE, 1_u32, 2_u16, 5_u16, 6_u16),
        (FRAMEBUFFER_C_BASE, 2_u32, 4_u16, 7_u16, 8_u16),
    ];
    for (tlb_base, depth, bytes_per_pixel, x, y) in cases {
        let mut render = CrimeRender::new();
        let tile = 3_u16 + depth as u16;
        let entry = FRAMEBUFFER_TLB_VALID | tile;
        queue_and_retire(&mut render, tlb_base, 8, u64::from(entry) << 48);
        let y_byte = u32::from(y) * u32::from(bytes_per_pixel);
        let start = y_byte << 16 | u32::from(x);
        let end = (y_byte + u32::from(bytes_per_pixel) - 1) << 16 | u32::from(x);
        configure_zero_clear_destination(&mut render, start, end);
        let framebuffer = depth;
        let mode = depth << 8 | framebuffer << 2;
        render
            .write(MTE_BASE + START_OFFSET, 4, u64::from(mode))
            .unwrap();

        let request = retire(&mut render).memory_request.unwrap();
        let expected_offset =
            u64::from(y) * FRAMEBUFFER_TILE_ROW_BYTES + u64::from(x) * u64::from(bytes_per_pixel);
        assert_eq!(request.virtual_address, start);
        assert_eq!(request.raw_entry, u32::from(entry));
        assert_eq!(
            request.alias_address,
            u64::from(tile) * FRAMEBUFFER_TILE_BYTES + expected_offset
        );
        assert_eq!(request.transfer.length(), usize::from(bytes_per_pixel));
        assert!(request.no_ecc);
    }
}

#[test]
fn framebuffer_clear_splits_at_a_tile_column_boundary() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 1) << 48 | u64::from(FRAMEBUFFER_TLB_VALID | 2) << 32,
    );
    let start = 2_u32 << 16 | 511;
    let end = 2_u32 << 16 | 512;
    configure_zero_clear_destination(&mut render, start, end);
    render.write(MTE_BASE + START_OFFSET, 4, 1).unwrap();

    let first = retire(&mut render).memory_request.unwrap();
    assert_eq!(first.raw_entry, u32::from(FRAMEBUFFER_TLB_VALID | 1));
    assert_eq!(first.alias_address, 0x1_05ff);
    assert_eq!(first.transfer.length(), 1);

    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_request.unwrap();
    assert_eq!(second.virtual_address, 2_u32 << 16 | 512);
    assert_eq!(second.raw_entry, u32::from(FRAMEBUFFER_TLB_VALID | 2));
    assert_eq!(second.alias_address, 0x2_0400);
    assert_eq!(second.transfer.length(), 1);
}

#[test]
fn zero_framebuffer_tile_is_an_invalid_translation() {
    let mut render = CrimeRender::new();
    configure_zero_clear_destination(&mut render, 0, 0);
    render.write(MTE_BASE + START_OFFSET, 4, 1).unwrap();

    let request = retire(&mut render).memory_request.unwrap();
    assert!(!request.valid);
    assert_eq!(request.raw_entry, 0);
    assert_eq!(
        request.bank_select,
        CrimeMemoryBankSelect::Inhibited {
            reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
        }
    );
}

#[test]
fn framebuffer_tile_requires_valid_bit_and_masks_it_from_the_address() {
    let mut render = CrimeRender::new();
    queue_and_retire(&mut render, FRAMEBUFFER_A_BASE, 8, 1_u64 << 48);
    configure_zero_clear_destination(&mut render, 0, 0);
    render.write(MTE_BASE + START_OFFSET, 4, 1).unwrap();

    let request = retire(&mut render).memory_request.unwrap();
    assert!(!request.valid);
    assert_eq!(request.raw_entry, 1);
    assert_eq!(request.alias_address, FRAMEBUFFER_TILE_BYTES);
    assert_eq!(
        request.bank_select,
        CrimeMemoryBankSelect::Inhibited {
            reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
        }
    );

    let mut valid = CrimeRender::new();
    queue_and_retire(
        &mut valid,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 1) << 48,
    );
    configure_zero_clear_destination(&mut valid, 0, 0);
    valid.write(MTE_BASE + START_OFFSET, 4, 1).unwrap();
    let request = retire(&mut valid).memory_request.unwrap();
    assert!(request.valid);
    assert_eq!(request.alias_address, FRAMEBUFFER_TILE_BYTES);
    assert_eq!(request.bank_select, CrimeMemoryBankSelect::Decode);
}

#[test]
fn in_flight_mte_state_round_trips_with_the_memory_correlation() {
    let mut reference = CrimeRender::new();
    configure_prom_clear(
        &mut reference,
        u64::from(0x8000_0009_u32) << 32,
        0x40,
        0x23f,
    );
    reference
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    let request = retire(&mut reference).memory_request.unwrap();
    assert_eq!(request.transfer.length(), 512);
    assert!(reference.memory_request_unit.busy());

    let encoded = postcard::to_stdvec(&reference).unwrap();
    let mut restored: CrimeRender = postcard::from_bytes(&encoded).unwrap();
    assert_eq!(restored, reference);

    let reference_progress = reference.complete_memory(write_completion()).unwrap();
    let restored_progress = restored.complete_memory(write_completion()).unwrap();
    assert_eq!(restored_progress, reference_progress);
    assert_eq!(restored, reference);
    assert!(
        reference_progress
            .notices
            .contains(&RenderNotice::MemoryCompleted {
                destination: RenderMemoryDestination::Mte,
                virtual_address: 0x40,
                physical_address: 0x9040,
                length: 512,
            })
    );
}

#[test]
fn unproven_mte_modes_fail_instead_of_falling_back() {
    let cases = [
        (1_u32 << 11 | PROM_CLEAR_MODE, u32::MAX, 0_u32),
        (PROM_CLEAR_MODE, 0xffff_fffe, 0),
        (PROM_CLEAR_MODE, u32::MAX, 0x1122_3344),
    ];
    for (mode, byte_mask, foreground) in cases {
        let mut render = CrimeRender::new();
        queue_and_retire(
            &mut render,
            LINEAR_A_BASE,
            8,
            u64::from(0x8000_0001_u32) << 32,
        );
        queue_and_retire(&mut render, MTE_BASE + 0x08, 4, byte_mask.into());
        queue_and_retire(&mut render, MTE_BASE + 0x18, 4, foreground.into());
        queue_and_retire(&mut render, MTE_BASE + 0x30, 4, 0);
        queue_and_retire(&mut render, MTE_BASE + 0x38, 4, 0);
        render
            .write(MTE_BASE + START_OFFSET, 4, mode.into())
            .unwrap();

        assert_eq!(
            render.step(),
            Err(CrimeRenderError::UnsupportedMteJob {
                mode,
                byte_mask,
                foreground,
            })
        );
        assert!(render.active_job.is_none());
        assert!(!render.memory_request_unit.busy());
    }
}

#[test]
fn all_seven_tlb_groups_preserve_their_first_and_last_raw_slots() {
    let mut render = CrimeRender::new();
    let ranges = [
        (FRAMEBUFFER_A_BASE, 64_usize),
        (FRAMEBUFFER_B_BASE, 64),
        (FRAMEBUFFER_C_BASE, 64),
        (TEXTURE_TLB_BASE, 28),
        (CID_TLB_BASE, 4),
        (LINEAR_A_BASE, 16),
        (LINEAR_B_BASE, 16),
    ];
    let mut expected = Vec::new();
    for (group, (base, count)) in ranges.into_iter().enumerate() {
        for index in [0, count - 1] {
            let address = base + (index as u64) * 8;
            let value = ((group as u64) << 56) | ((index as u64) << 32) | 0x1122_3344;
            queue_and_retire(&mut render, address, 8, value);
            expected.push((address, value));
        }
    }
    for (address, value) in expected {
        assert_eq!(render.read(address, 8), Ok(value));
    }
    assert_eq!(
        render.read(TLB_BASE + 0x800, 8),
        Err(RenderAccessError::Unsupported)
    );
}

#[test]
fn job_snapshot_is_not_changed_by_later_linear_a_writes() {
    let mut render = CrimeRender::new();
    configure_prom_clear(&mut render, u64::from(0x8000_0001_u32) << 32, 0, 0);
    render
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    render
        .write(LINEAR_A_BASE, 8, u64::from(0x8000_0002_u32) << 32)
        .unwrap();

    let write = retire(&mut render).memory_request.unwrap();
    assert_eq!(write.physical_address, 0x1000);
    assert_eq!(render.interface_level(), 1);
    render.complete_memory(write_completion()).unwrap();
    let completion = retire(&mut render);
    assert!(
        completion
            .notices
            .contains(&RenderNotice::JobCompleted { start: 0, end: 0 })
    );
    assert!(completion.schedule_step);
    retire(&mut render);
    assert_eq!(render.tlbs.linear_a[0] >> 32, 0x8000_0002);
}

#[test]
fn full_empty_and_idle_transitions_emit_edge_and_level_effects() {
    let mut render = CrimeRender::new();
    render
        .write(
            INTERFACE_CONTROL,
            4,
            (64_u64 << INTERFACE_FULL_SHIFT) | (127_u64 << INTERFACE_STALL_LEVEL_SHIFT),
        )
        .unwrap();
    let mut last = RenderProgress::default();
    for _ in 0..64 {
        last = render.write(PIXEL_PIPE_BASE, 4, 0).unwrap();
    }
    assert!(last.interrupts.contains(&RenderInterruptEffect {
        mask: registers::INTERRUPT_RE_FULL_LEVEL,
        asserted: true,
    }));
    assert!(last.interrupts.contains(&RenderInterruptEffect {
        mask: registers::INTERRUPT_RE_FULL_EDGE,
        asserted: true,
    }));

    let first_retire = retire(&mut render);
    assert!(first_retire.interrupts.contains(&RenderInterruptEffect {
        mask: registers::INTERRUPT_RE_FULL_LEVEL,
        asserted: false,
    }));
    for _ in 1..64 {
        last = retire(&mut render);
    }
    assert!(last.interrupts.contains(&RenderInterruptEffect {
        mask: registers::INTERRUPT_RE_EMPTY_LEVEL,
        asserted: true,
    }));
    assert!(last.interrupts.contains(&RenderInterruptEffect {
        mask: registers::INTERRUPT_RE_EMPTY_EDGE,
        asserted: true,
    }));
    assert!(last.interrupts.contains(&RenderInterruptEffect {
        mask: registers::INTERRUPT_RE_IDLE_EDGE,
        asserted: true,
    }));
}

#[test]
fn unexpected_memory_results_are_explicit_errors() {
    let mut render = CrimeRender::new();
    assert_eq!(
        render.complete_memory(write_completion()),
        Err(CrimeRenderError::UnexpectedMemoryCompletion)
    );

    configure_prom_clear(&mut render, u64::from(0x8000_0001_u32) << 32, 0, 0);
    render
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    retire(&mut render);
    assert_eq!(
        render.complete_memory(Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::ReadData(vec![0].into()),
            None,
            None,
        ))),
        Err(CrimeRenderError::UnexpectedMemoryPayload)
    );
}

#[test]
fn logic_operations_cover_all_sixteen_functions() {
    let source = 0x0f0f_55aa;
    let destination = 0x3333_cccc;
    assert_eq!(logic_operation(0, source, destination), 0);
    assert_eq!(logic_operation(3, source, destination), source);
    assert_eq!(logic_operation(5, source, destination), destination);
    assert_eq!(
        logic_operation(6, source, destination),
        source ^ destination
    );
    assert_eq!(logic_operation(15, source, destination), u32::MAX);
}

#[test]
fn comparison_functions_match_documented_order() {
    assert!(!compare(0, 1, 2));
    assert!(compare(1, 1, 2));
    assert!(compare(2, 2, 2));
    assert!(compare(3, 2, 2));
    assert!(compare(4, 3, 2));
    assert!(compare(5, 3, 2));
    assert!(compare(6, 2, 2));
    assert!(compare(7, 0, u32::MAX));
}

#[test]
fn undefined_holes_and_write_only_register_reads_are_rejected() {
    let mut render = CrimeRender::new();

    assert_eq!(
        render.read(PIXEL_PIPE_BASE + 0x068, 4),
        Err(RenderAccessError::Unsupported)
    );
    assert_eq!(
        render.read(PIXEL_PIPE_BASE + 0x070, 4),
        Err(RenderAccessError::Unsupported)
    );
    assert_eq!(
        render.write(PIXEL_PIPE_BASE + 0x068, 4, 0),
        Err(RenderWriteError::Access(RenderAccessError::Unsupported))
    );
    assert_eq!(
        render.write(STATUS_BASE, 4, 0),
        Err(RenderWriteError::Access(RenderAccessError::Unsupported))
    );
}

#[test]
fn alignment_width_and_mapping_failures_remain_distinct() {
    let mut render = CrimeRender::new();
    assert_eq!(
        render.read(PIXEL_PIPE_BASE + 2, 4),
        Err(RenderAccessError::Access)
    );
    assert_eq!(
        render.read(PIXEL_PIPE_BASE, 8),
        Err(RenderAccessError::Access)
    );
    assert_eq!(
        render.read(PIXEL_PIPE_BASE + 0x068, 4),
        Err(RenderAccessError::Unsupported)
    );
    assert_eq!(
        render.write(MTE_BASE, 8, 0),
        Err(RenderWriteError::Access(RenderAccessError::Access))
    );
    assert_eq!(
        render.write(MTE_BASE + 0x050, 4, 0),
        Err(RenderWriteError::Access(RenderAccessError::Unsupported))
    );
}
