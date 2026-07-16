use super::*;
use se_core::component::ComponentId;
use se_core::role::BusDeviceRole;
use se_core::scheduler::SimTime;

use crate::chipset::crime::config::CrimeMemoryConfig;
use crate::chipset::crime::memory::CrimeSdram;
use crate::chipset::crime::protocol::{
    CrimeMemoryClient, CrimeMemoryTransaction, CrimeTransactionId, CrimeTransfer, CrimeTransferView,
};
use crate::chipset::gbe::display::{PlaneDepth, decode_raw_pixels};

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
    queue_and_retire(render, PIXEL_PIPE_BASE, 4, 0x0000_0628_u32.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x008, 4, 0x0000_0628_u32.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x018, 4, 0x0000_02f8_u32.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x060, 4, 0x0100_0020_u32.into());
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
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x018, 4, 0x0000_00f8_u32.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x1b8, 4, u32::MAX.into());
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x070, 4, 0);
    queue_and_retire(
        render,
        PIXEL_PIPE_BASE + 0x074,
        4,
        (u32::from(x1) << 16 | u32::from(y1)).into(),
    );
    queue_and_retire(render, PIXEL_PIPE_BASE + 0x060, 4, 0x0302_0000_u32.into());
}

#[derive(Clone, Copy)]
struct StippledLineConfig {
    buffer_mode: u32,
    color: u32,
    x0: u16,
    y: u16,
    x1: u16,
    stipple_mode: u32,
    pattern: u32,
}

fn configure_stippled_line(render: &mut CrimeRender, config: StippledLineConfig) {
    for (offset, value) in [
        (0x000, config.buffer_mode),
        (0x008, config.buffer_mode),
        (0x018, 0x0008_02f8),
        (0x060, 0x0100_0020),
        (0x0c0, config.stipple_mode),
        (0x0c4, config.pattern),
        (0x0d0, config.color),
        (0x1b0, LOGIC_COPY),
        (0x1b8, u32::MAX),
        (0x070, u32::from(config.x0) << 16 | u32::from(config.y)),
        (0x074, u32::from(config.x1) << 16 | u32::from(config.y)),
    ] {
        queue_and_retire(render, PIXEL_PIPE_BASE + offset, 4, value.into());
    }
}

fn collect_pixel_writes(render: &mut CrimeRender) -> Vec<(u64, Vec<u8>, Vec<bool>)> {
    let mut writes = Vec::new();
    loop {
        let progress = retire(render);
        if let Some(request) = progress.memory_request {
            let alias_address = request.alias_address;
            let CrimeTransferView::Write { data, byte_enable } = request.transfer.view() else {
                panic!("PixelPipe emitted a read request")
            };
            writes.push((alias_address, data.to_vec(), byte_enable.iter().collect()));
            render.complete_memory(write_completion()).unwrap();
        }
        if render.active_pixel_command.is_none() {
            break;
        }
    }
    writes
}

fn complete_through_sdram(
    render: &mut CrimeRender,
    sdram: &mut CrimeSdram,
    request: RenderMemoryRequest,
) {
    let completion = sdram.accept(CrimeMemoryTransaction {
        id: CrimeTransactionId::new(1),
        time: SimTime::ZERO,
        controller: ComponentId::new(1),
        client: CrimeMemoryClient::Render,
        address: request.physical_address,
        bank_select: request.bank_select,
        no_ecc: request.no_ecc,
        transfer: request.transfer,
    });
    render.complete_memory(completion.result).unwrap();
}

fn read_gbe_word(sdram: &mut CrimeSdram, address: u64) -> Vec<u8> {
    let completion = sdram.accept(CrimeMemoryTransaction {
        id: CrimeTransactionId::new(2),
        time: SimTime::ZERO,
        controller: ComponentId::new(1),
        client: CrimeMemoryClient::Gbe,
        address,
        bank_select: CrimeMemoryBankSelect::Decode,
        no_ecc: false,
        transfer: CrimeTransfer::read(RENDER_MEMORY_WORD_BYTES as u16),
    });
    let CrimeCompletionPayload::ReadData(data) = completion.result.unwrap().payload else {
        panic!("GBE memory read returned the wrong payload")
    };
    data.to_vec()
}

fn write_sdram(sdram: &mut CrimeSdram, address: u64, data: &[u8]) {
    let completion = sdram.accept(CrimeMemoryTransaction {
        id: CrimeTransactionId::new(3),
        time: SimTime::ZERO,
        controller: ComponentId::new(1),
        client: CrimeMemoryClient::Render,
        address,
        bank_select: CrimeMemoryBankSelect::Decode,
        no_ecc: false,
        transfer: CrimeTransfer::write(data.to_vec().into(), CrimeByteEnable::enabled(data.len())),
    });
    assert!(matches!(
        completion.result.unwrap().payload,
        CrimeCompletionPayload::WriteComplete
    ));
}

fn run_mte_through_sdram(render: &mut CrimeRender, sdram: &mut CrimeSdram) {
    while render.active_job.is_some() || render.interface_level() != 0 {
        let progress = retire(render);
        if let Some(request) = progress.memory_request {
            complete_through_sdram(render, sdram, request);
        }
    }
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
    let error = render.step().unwrap_err();
    assert!(matches!(
        error,
        CrimeRenderError::UnsupportedPixelCommand {
            trigger_address: PIXEL_PIPE_NULL,
            primitive: 0,
            draw_mode: 0,
            ref blockers,
            ..
        } if !blockers.is_empty()
    ));
    assert!(matches!(
        render.active_pixel_command,
        Some(PixelExecution::Blocked(_))
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
    assert!(matches!(
        &error,
        CrimeRenderError::UnsupportedPixelCommand {
            trigger_address,
            primitive: captured_primitive,
            draw_mode,
            blockers,
            ..
        } if *trigger_address == PIXEL_PIPE_BASE + 0x060
            && *captured_primitive == primitive
            && *draw_mode == initial_draw_mode
            && !blockers.is_empty()
    ));
    let Some(PixelExecution::Blocked(blocked)) = render.active_pixel_command.as_ref() else {
        panic!("unsupported command snapshot was not retained")
    };
    assert_eq!(blocked.command.snapshot.primitive(), primitive);
    assert_eq!(blocked.command.snapshot.draw_mode(), initial_draw_mode);
    assert_eq!(blocked.error, error);
    assert_eq!(render.interface_level(), 1);
    assert_eq!(render.status() & STATUS_PIXEL_PIPE_IDLE, 0);
    assert_eq!(render.step(), Err(error));
}

#[test]
fn invalid_pixel_start_reports_every_violation_before_capabilities() {
    let mut render = CrimeRender::new();
    queue_and_retire(&mut render, PIXEL_PIPE_BASE, 4, u32::MAX.into());
    queue_and_retire(&mut render, PIXEL_PIPE_BASE + 0x008, 4, u32::MAX.into());
    queue_and_retire(&mut render, PIXEL_PIPE_BASE + 0x018, 4, u32::MAX.into());
    render
        .write(PIXEL_PIPE_BASE + 0x060 + START_OFFSET, 4, 0x05f8_0000)
        .unwrap();

    let error = render.step().unwrap_err();
    let CrimeRenderError::InvalidPixelCommand {
        primitive,
        draw_mode,
        violations,
        ..
    } = &error
    else {
        panic!("invalid command was not distinguished from unsupported behavior")
    };
    assert_eq!(*primitive, 0x05f8_0000);
    assert_eq!(*draw_mode, u32::MAX);
    assert!(violations.len() >= 8);
    assert!(
        violations
            .windows(2)
            .all(|pair| pair[0].kind <= pair[1].kind)
    );
    assert_eq!(render.step(), Err(error));
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
                primitive: 0x0100_0020,
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
                primitive: 0x0100_0020,
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
fn ci8_stipple_round_trips_from_crime_through_sdram_to_gbe() {
    let mut render = CrimeRender::new();
    let mut sdram = CrimeSdram::new(ComponentId::new(2), "SDRAM", CrimeMemoryConfig::default());
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 1) << 48,
    );
    configure_stippled_line(
        &mut render,
        StippledLineConfig {
            buffer_mode: 0,
            color: 0x5a,
            x0: 0,
            y: 0,
            x1: 31,
            stipple_mode: 0x001f_0000,
            pattern: 0x8000_0001,
        },
    );
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let request = retire(&mut render).memory_request.unwrap();
    let address = request.physical_address;
    complete_through_sdram(&mut render, &mut sdram, request);
    retire(&mut render);
    let pixels = decode_raw_pixels(&read_gbe_word(&mut sdram, address), PlaneDepth::Eight);

    let mut expected = vec![0_u32; 32];
    expected[0] = 0x5a;
    expected[31] = 0x5a;
    assert_eq!(pixels, expected);
}

#[test]
fn rgba32_pixels_round_trip_from_crime_through_sdram_to_gbe() {
    let mut render = CrimeRender::new();
    let mut sdram = CrimeSdram::new(ComponentId::new(2), "SDRAM", CrimeMemoryConfig::default());
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_B_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 1) << 48,
    );
    let colors = [
        0x1020_3040,
        0x1121_3141,
        0x1222_3242,
        0x1323_3343,
        0x1424_3444,
        0x1525_3545,
        0x1626_3646,
        0x1727_3747,
    ];
    let address = FRAMEBUFFER_TILE_BYTES;
    for (x, color) in colors.into_iter().enumerate() {
        configure_x_line(&mut render, color, x as u16, 0, x as u16, 0);
        render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();
        let request = retire(&mut render).memory_request.unwrap();
        assert_eq!(request.physical_address, address);
        complete_through_sdram(&mut render, &mut sdram, request);
        retire(&mut render);
    }

    let pixels = decode_raw_pixels(&read_gbe_word(&mut sdram, address), PlaneDepth::ThirtyTwo);
    assert_eq!(pixels, colors);
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
    assert_eq!(&data[24..28], &[0xaa, 0xbb, 0xcc, 0xdd]);
    assert_eq!(byte_enable.iter().filter(|enabled| *enabled).count(), 4);
    assert!((24..28).all(|lane| byte_enable.is_enabled(lane) == Some(true)));

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
fn prom_short_stipple_patterns_generate_exact_ci8_byte_enables() {
    for (width, pattern, expected_positions) in [
        (1_u16, 0x0000_8000_u32, vec![0_usize]),
        (8, 0x0000_a500, vec![0, 2, 5, 7]),
        (16, 0x0000_8001, vec![0, 15]),
    ] {
        let mut render = CrimeRender::new();
        queue_and_retire(
            &mut render,
            FRAMEBUFFER_A_BASE,
            8,
            u64::from(FRAMEBUFFER_TLB_VALID | 1) << 48,
        );
        configure_stippled_line(
            &mut render,
            StippledLineConfig {
                buffer_mode: 0,
                color: 0xa5,
                x0: 0,
                y: 0,
                x1: width - 1,
                stipple_mode: 0x101f_0000,
                pattern,
            },
        );
        render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

        let writes = collect_pixel_writes(&mut render);
        assert_eq!(writes.len(), 1, "width {width}");
        let (_, data, byte_enable) = &writes[0];
        let mut expected_enable = [false; RENDER_MEMORY_WORD_BYTES];
        let mut expected_data = [0; RENDER_MEMORY_WORD_BYTES];
        for candidate in 0..usize::from(width) {
            let expected = expected_positions.contains(&candidate);
            let lane = framebuffer::physical_pixel_lane(candidate, 1).unwrap();
            expected_enable[lane] = expected;
            expected_data[lane] = if expected { 0xa5 } else { 0 };
        }
        assert_eq!(byte_enable.as_slice(), expected_enable, "width {width}");
        assert_eq!(data.as_slice(), expected_data, "width {width}");
    }
}

#[test]
fn prom_long_stipple_patterns_generate_exact_ci8_byte_enables() {
    for (width, pattern, expected_positions) in [
        (17_u16, 0x8001_0000_u32, vec![0_usize, 15]),
        (31, 0x8000_0002, vec![0, 30]),
        (32, 0x8000_0001, vec![0, 31]),
    ] {
        let mut render = CrimeRender::new();
        queue_and_retire(
            &mut render,
            FRAMEBUFFER_A_BASE,
            8,
            u64::from(FRAMEBUFFER_TLB_VALID | 2) << 48,
        );
        configure_stippled_line(
            &mut render,
            StippledLineConfig {
                buffer_mode: 0,
                color: 0x5a,
                x0: 0,
                y: 0,
                x1: width - 1,
                stipple_mode: 0x001f_0000,
                pattern,
            },
        );
        render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

        let writes = collect_pixel_writes(&mut render);
        assert_eq!(writes.len(), 1, "width {width}");
        let (_, data, byte_enable) = &writes[0];
        let mut expected_enable = [false; RENDER_MEMORY_WORD_BYTES];
        let mut expected_data = [0; RENDER_MEMORY_WORD_BYTES];
        for candidate in 0..usize::from(width) {
            let expected = expected_positions.contains(&candidate);
            let lane = framebuffer::physical_pixel_lane(candidate, 1).unwrap();
            expected_enable[lane] = expected;
            expected_data[lane] = if expected { 0x5a } else { 0 };
        }
        assert_eq!(byte_enable.as_slice(), expected_enable, "width {width}");
        assert_eq!(data.as_slice(), expected_data, "width {width}");
    }
}

#[test]
fn all_zero_stipple_advances_without_issuing_memory() {
    let mut render = CrimeRender::new();
    configure_stippled_line(
        &mut render,
        StippledLineConfig {
            buffer_mode: 0,
            color: 0xff,
            x0: 0,
            y: 0,
            x1: 15,
            stipple_mode: 0x101f_0000,
            pattern: 0,
        },
    );
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let committed = retire(&mut render);
    assert!(committed.memory_request.is_none());
    assert!(committed.notices.contains(&RenderNotice::RasterBatch {
        x: 0,
        y: 0,
        candidates: 16,
        enabled: 0,
    }));
    let completed = retire(&mut render);
    assert!(completed.memory_request.is_none());
    assert!(render.active_pixel_command.is_none());
}

#[test]
fn full_stipple_preserves_big_endian_rgba32_packing() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_C_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 3) << 48,
    );
    configure_stippled_line(
        &mut render,
        StippledLineConfig {
            buffer_mode: 0x0000_0a28,
            color: 0x1122_3344,
            x0: 0,
            y: 0,
            x1: 7,
            stipple_mode: 0x101f_0000,
            pattern: 0x0000_ffff,
        },
    );
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let writes = collect_pixel_writes(&mut render);
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].1, [0x11, 0x22, 0x33, 0x44].repeat(8));
    assert!(writes[0].2.iter().all(|enabled| *enabled));
}

#[test]
fn stippled_line_switches_tlb_entries_without_losing_pattern_phase() {
    let mut render = CrimeRender::new();
    let first_entry = FRAMEBUFFER_TLB_VALID | 4;
    let second_entry = FRAMEBUFFER_TLB_VALID | 5;
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(first_entry) << 48 | u64::from(second_entry) << 32,
    );
    configure_stippled_line(
        &mut render,
        StippledLineConfig {
            buffer_mode: 0,
            color: 0x7e,
            x0: 508,
            y: 0,
            x1: 523,
            stipple_mode: 0x101f_0000,
            pattern: 0x0000_a5a5,
        },
    );
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let writes = collect_pixel_writes(&mut render);
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0].0, 4 * FRAMEBUFFER_TILE_BYTES + 480);
    assert_eq!(writes[1].0, 5 * FRAMEBUFFER_TILE_BYTES);
    let enabled = writes
        .iter()
        .flat_map(|(_, _, mask)| mask.iter().copied())
        .filter(|enabled| *enabled)
        .count();
    assert_eq!(enabled, 8);
}

#[test]
fn in_flight_stipple_batch_round_trips_before_cursor_commit() {
    let mut reference = CrimeRender::new();
    queue_and_retire(
        &mut reference,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 6) << 48,
    );
    configure_stippled_line(
        &mut reference,
        StippledLineConfig {
            buffer_mode: 0,
            color: 0x3c,
            x0: 0,
            y: 0,
            x1: 31,
            stipple_mode: 0x001f_0000,
            pattern: 0x8000_0001,
        },
    );
    reference
        .write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0)
        .unwrap();
    let request = retire(&mut reference).memory_request.unwrap();
    assert_eq!(request.transfer.length(), RENDER_MEMORY_WORD_BYTES);
    let Some(PixelExecution::Running(job)) = reference.active_pixel_command.as_ref() else {
        panic!("stippled line was not running")
    };
    assert_eq!(job.stipple.map(PixelStippleCursor::index), Some(0));
    assert_eq!(job.pending_batch.unwrap().candidate_count, 32);

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
                primitive: 0x0302_0000,
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
                primitive: 0x0302_0000,
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
        (0x018, 0x0000_02f8),
        (0x1b0, LOGIC_COPY),
        (0x1b8, u32::MAX),
        (0x070, 3 << 16 | 2),
        (0x074, 35 << 16 | 2),
        (0x060, 0x0302_0000),
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
    assert!(data[..28].iter().all(|value| *value == 0xa5));
    assert!(data[28..31].iter().all(|value| *value == 0));
    assert_eq!(data[31], 0xa5);
    assert!(byte_enable.iter().take(28).all(|enabled| enabled));
    assert!(byte_enable.iter().skip(28).take(3).all(|enabled| !enabled));
    assert_eq!(byte_enable.is_enabled(31), Some(true));

    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_request.unwrap();
    assert_eq!(second.virtual_address, 2 << 16 | 32);
    let CrimeTransferView::Write { data, byte_enable } = second.transfer.view() else {
        panic!("PROM flat rectangle emitted a read request")
    };
    assert!(data[..28].iter().all(|value| *value == 0));
    assert!(data[28..].iter().all(|value| *value == 0xa5));
    assert!(byte_enable.iter().take(28).all(|enabled| !enabled));
    assert!(byte_enable.iter().skip(28).all(|enabled| enabled));
}

#[test]
fn prom_ci8_flat_rectangle_supports_evidence_backed_descending_rows() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 3) << 48,
    );
    for (offset, value) in [
        (0x0d0, 0xa5),
        (0x018, 0x0000_02f8),
        (0x1b0, LOGIC_COPY),
        (0x1b8, u32::MAX),
        (0x070, 3 << 16 | 2),
        (0x074, 5 << 16 | 1),
        (0x060, 0x0300_0000),
    ] {
        queue_and_retire(&mut render, PIXEL_PIPE_BASE + offset, 4, value.into());
    }
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let first = retire(&mut render).memory_request.unwrap();
    assert_eq!(first.virtual_address, 2 << 16 | 3);
    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_request.unwrap();
    assert_eq!(second.virtual_address, 1 << 16 | 3);
    render.complete_memory(write_completion()).unwrap();
    let completed = retire(&mut render);
    assert!(
        completed
            .notices
            .contains(&RenderNotice::PixelCommandCompleted {
                primitive: 0x0300_0000,
                x0: 3,
                y0: 2,
                x1: 5,
                y1: 1,
            })
    );
}

#[test]
fn prom_zero_rectangle_rejects_nonzero_foreground() {
    let mut render = CrimeRender::new();
    configure_prom_ci8_zero_rectangle(&mut render, 31, 1);
    queue_and_retire(&mut render, PIXEL_PIPE_BASE + 0x0d0, 4, 1);
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();

    let error = render.step().unwrap_err();
    assert!(matches!(
        error,
        CrimeRenderError::UnsupportedPixelCommand {
            trigger_address: PIXEL_PIPE_NULL,
            primitive: 0x0302_0000,
            draw_mode: 0x0000_00f8,
            ref blockers,
            ..
        } if blockers.iter().any(|blocker| blocker.capability == PixelCapability::ZeroRectangleColor)
    ));
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
fn mte_accepts_foreground_and_byte_mask_but_rejects_reserved_mode_bits() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        LINEAR_A_BASE,
        8,
        u64::from(0x8000_0001_u32) << 32,
    );
    queue_and_retire(&mut render, MTE_BASE + 0x08, 4, 0x7fff_ffff);
    queue_and_retire(&mut render, MTE_BASE + 0x18, 4, 0x1122_3344);
    queue_and_retire(&mut render, MTE_BASE + 0x30, 4, 0);
    queue_and_retire(&mut render, MTE_BASE + 0x38, 4, 0);
    render
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();

    let request = render.step().unwrap().memory_request.unwrap();
    let CrimeTransferView::Write { data, byte_enable } = request.transfer.view() else {
        panic!("MTE clear emitted a read")
    };
    assert_eq!(data, [0x44]);
    assert_eq!(byte_enable.iter().collect::<Vec<_>>(), [false]);

    let mut invalid = CrimeRender::new();
    configure_zero_clear_destination(&mut invalid, 0, 0);
    let mode = 1_u32 << 12 | PROM_CLEAR_MODE;
    invalid
        .write(MTE_BASE + START_OFFSET, 4, mode.into())
        .unwrap();
    assert_eq!(
        invalid.step(),
        Err(CrimeRenderError::InvalidMteJob {
            mode,
            field: MteInvalidField::ReservedModeBits,
        })
    );
}

#[test]
fn mte_copy_reads_then_writes_across_distinct_linear_tlbs() {
    let mut render = CrimeRender::new();
    let mut sdram = CrimeSdram::new(ComponentId::new(2), "SDRAM", CrimeMemoryConfig::default());
    queue_and_retire(
        &mut render,
        LINEAR_A_BASE,
        8,
        u64::from(0x8000_0001_u32) << 32,
    );
    queue_and_retire(
        &mut render,
        LINEAR_B_BASE,
        8,
        u64::from(0x8000_0002_u32) << 32,
    );
    write_sdram(&mut sdram, 0x1000, &[0x11, 0x22, 0x33, 0x44]);
    for (offset, value) in [(0x20, 0_u32), (0x28, 3), (0x30, 0), (0x38, 3)] {
        queue_and_retire(&mut render, MTE_BASE + offset, 4, value.into());
    }
    let mode = 1_u32 << 11 | 4 << 5 | 5 << 2 | 3;
    render
        .write(MTE_BASE + START_OFFSET, 4, mode.into())
        .unwrap();
    run_mte_through_sdram(&mut render, &mut sdram);

    assert_eq!(
        &read_gbe_word(&mut sdram, 0x2000)[..4],
        [0x11, 0x22, 0x33, 0x44]
    );
}

#[test]
fn overlapping_mte_copy_uses_memmove_safe_reverse_order() {
    let mut render = CrimeRender::new();
    let mut sdram = CrimeSdram::new(ComponentId::new(2), "SDRAM", CrimeMemoryConfig::default());
    queue_and_retire(
        &mut render,
        LINEAR_A_BASE,
        8,
        u64::from(0x8000_0001_u32) << 32,
    );
    write_sdram(&mut sdram, 0x1000, &[1, 2, 3, 4, 5]);
    for (offset, value) in [(0x20, 0_u32), (0x28, 3), (0x30, 1), (0x38, 4)] {
        queue_and_retire(&mut render, MTE_BASE + offset, 4, value.into());
    }
    let mode = 1_u32 << 11 | 4 << 5 | 4 << 2 | 3;
    render
        .write(MTE_BASE + START_OFFSET, 4, mode.into())
        .unwrap();
    run_mte_through_sdram(&mut render, &mut sdram);

    assert_eq!(&read_gbe_word(&mut sdram, 0x1000)[..5], [1, 1, 2, 3, 4]);
}

#[test]
fn mte_copy_read_stage_round_trips_with_row_buffer_state() {
    let mut render = CrimeRender::new();
    queue_and_retire(
        &mut render,
        LINEAR_A_BASE,
        8,
        u64::from(0x8000_0001_u32) << 32,
    );
    for (offset, value) in [(0x20, 0_u32), (0x28, 1), (0x30, 2), (0x38, 3)] {
        queue_and_retire(&mut render, MTE_BASE + offset, 4, value.into());
    }
    let mode = 1_u32 << 11 | 4 << 5 | 4 << 2;
    render
        .write(MTE_BASE + START_OFFSET, 4, mode.into())
        .unwrap();
    let first = retire(&mut render).memory_request.unwrap();
    assert!(matches!(
        first.transfer.view(),
        CrimeTransferView::Read { length: 1 }
    ));
    render
        .complete_memory(Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::ReadData(vec![0xaa].into()),
            None,
            None,
        )))
        .unwrap();

    let encoded = postcard::to_stdvec(&render).unwrap();
    let restored: CrimeRender = postcard::from_bytes(&encoded).unwrap();
    assert_eq!(restored, render);
    let job = restored.active_job.as_ref().unwrap();
    assert_eq!(job.row_buffer, [0xaa]);
    assert_eq!(job.stage, MteStage::CopyRead);
}

#[test]
fn pixel_dma_converts_linear_ycrcb_into_tiled_rgba() {
    let mut render = CrimeRender::new();
    let mut sdram = CrimeSdram::new(ComponentId::new(2), "SDRAM", CrimeMemoryConfig::default());
    queue_and_retire(
        &mut render,
        LINEAR_A_BASE,
        8,
        u64::from(0x8000_0001_u32) << 32,
    );
    queue_and_retire(
        &mut render,
        FRAMEBUFFER_A_BASE,
        8,
        u64::from(FRAMEBUFFER_TLB_VALID | 2) << 48,
    );
    write_sdram(&mut sdram, 0x1000, &[235, 128, 128, 0]);
    for (offset, value) in [
        (0x000, 0x0000_12f8_u32),
        (0x008, 0x0000_0228),
        (0x018, 0x0020_02f8),
        (0x060, 0x0100_0020),
        (0x070, 0),
        (0x074, 0),
        (0x0a0, 0),
        (0x0a8, 4),
        (0x1b0, LOGIC_COPY),
        (0x1b8, u32::MAX),
    ] {
        queue_and_retire(&mut render, PIXEL_PIPE_BASE + offset, 4, value.into());
    }
    render.write(PIXEL_PIPE_NULL + START_OFFSET, 4, 0).unwrap();
    while render.active_pixel_command.is_some() || render.interface_level() != 0 {
        let progress = retire(&mut render);
        if let Some(request) = progress.memory_request {
            complete_through_sdram(&mut render, &mut sdram, request);
        }
    }

    assert_eq!(
        &decode_raw_pixels(
            &read_gbe_word(&mut sdram, 2 * FRAMEBUFFER_TILE_BYTES),
            PlaneDepth::ThirtyTwo,
        )[..1],
        [u32::MAX]
    );
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
