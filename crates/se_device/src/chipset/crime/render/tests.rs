use super::*;

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
    queue_and_retire(render, MTE_BASE + 0x08, 4, u32::MAX.into());
    queue_and_retire(render, MTE_BASE + 0x18, 4, 0);
    queue_and_retire(render, MTE_BASE + 0x30, 4, start.into());
    queue_and_retire(render, MTE_BASE + 0x38, 4, end.into());
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
fn tagged_mte_write_is_canonicalized_and_tagged_reads_are_rejected() {
    let mut render = CrimeRender::new();
    render
        .write(MTE_BASE + START_OFFSET, 4, PROM_CLEAR_MODE.into())
        .unwrap();
    assert_eq!(
        render.interface.front(),
        Some(&RenderRegisterWrite {
            address: MTE_BASE,
            value: PROM_CLEAR_MODE.into(),
            size: 4,
            commit: true,
        })
    );
    assert_eq!(render.read(MTE_BASE + START_OFFSET, 4), None);
    assert_eq!(
        render.write(PIXEL_PIPE_BASE + START_OFFSET, 4, 0),
        Err(RenderWriteError::UndefinedRegister)
    );
}

#[test]
fn status_tracks_fifo_pointers_and_idle_blocks() {
    let mut render = CrimeRender::new();
    assert_eq!(
        render.status(),
        STATUS_IDLE
            | STATUS_SETUP_IDLE
            | STATUS_PIXEL_PIPE_IDLE
            | STATUS_MTE_IDLE
            | STATUS_BUFFER_START
    );

    render.write(PIXEL_PIPE_BASE, 4, 0).unwrap();
    assert_eq!(
        render.status(),
        STATUS_PIXEL_PIPE_IDLE
            | STATUS_MTE_IDLE
            | (1 << STATUS_LEVEL_SHIFT)
            | (1 << STATUS_WRITE_POINTER_SHIFT)
            | STATUS_BUFFER_START
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
            | STATUS_BUFFER_START
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
    let first = retire(&mut render).memory_write.unwrap();
    assert_eq!(first.virtual_address, 0x0ff0);
    assert_eq!(first.physical_address, 0x2ff0);
    assert_eq!(first.data, vec![0; 16]);
    assert!(first.byte_enable.iter().all(|enabled| enabled));

    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_write.unwrap();
    assert_eq!(second.virtual_address, 0x1000);
    assert_eq!(second.physical_address, 0x5000);
    assert_eq!(second.data, vec![0; 17]);
}

#[test]
fn mte_chunks_are_bounded_to_five_hundred_twelve_bytes() {
    let mut render = CrimeRender::new();
    configure_prom_clear(&mut render, u64::from(0x8000_0001_u32) << 32, 0, 1023);
    render
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();

    let first = retire(&mut render).memory_write.unwrap();
    assert_eq!(first.physical_address, 0x1000);
    assert_eq!(first.data.len(), 512);
    render.complete_memory(write_completion()).unwrap();
    let second = retire(&mut render).memory_write.unwrap();
    assert_eq!(second.physical_address, 0x1200);
    assert_eq!(second.data.len(), 512);
}

#[test]
fn linear_a_uses_the_valid_bit_and_nineteen_page_bits() {
    let mut invalid = CrimeRender::new();
    configure_prom_clear(&mut invalid, 0, 0, 0);
    invalid
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    let write = invalid.step().unwrap().memory_write.unwrap();
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
    let write = linear_alias.step().unwrap().memory_write.unwrap();
    assert_eq!(write.alias_address, 0x4000_1000);
    assert_eq!(write.physical_address, 0x1000);
    assert_eq!(write.bank_select, CrimeMemoryBankSelect::Decode);

    let mut reserved_bits = CrimeRender::new();
    configure_prom_clear(&mut reserved_bits, u64::from(u32::MAX) << 32, 0, 0);
    reserved_bits
        .write(MTE_BASE + START_OFFSET, 4, u64::from(PROM_CLEAR_MODE))
        .unwrap();
    let write = reserved_bits.step().unwrap().memory_write.unwrap();
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

    let write = retire(&mut render).memory_write.unwrap();
    assert_eq!(write.virtual_address, 0x0002_0000);
    assert_eq!(write.physical_address, 0x3000);
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

    let write = retire(&mut render).memory_write.unwrap();
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
    assert_eq!(render.linear_a[0], 0x8000_0002);
}

#[test]
fn full_empty_and_idle_transitions_emit_edge_and_level_effects() {
    let mut render = CrimeRender::new();
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

    assert_eq!(render.read(PIXEL_PIPE_BASE + 0x068, 4), None);
    assert_eq!(render.read(PIXEL_PIPE_BASE + 0x070, 4), None);
    assert_eq!(
        render.write(PIXEL_PIPE_BASE + 0x068, 4, 0),
        Err(RenderWriteError::UndefinedRegister)
    );
    assert_eq!(
        render.write(STATUS_BASE, 4, 0),
        Err(RenderWriteError::UndefinedRegister)
    );
}
