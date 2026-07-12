use super::*;

#[test]
fn interface_buffer_enforces_128_entry_capacity() {
    let mut render = CrimeRender::new();
    for _ in 0..128 {
        render.write(PIXEL_PIPE_BASE, 4, 0).unwrap();
    }
    assert_eq!(render.interface_level(), 128);
    assert_eq!(
        render.write(PIXEL_PIPE_BASE, 4, 0),
        Err(RenderWriteError::InterfaceFull)
    );
    render.retire_one();
    assert!(render.write(PIXEL_PIPE_BASE, 4, 0).is_ok());
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
