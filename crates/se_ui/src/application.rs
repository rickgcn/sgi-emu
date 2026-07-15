use crate::emulation::EmulationController;
use crate::terminal::{TerminalModel, new_terminal_model};

#[cxx::bridge(namespace = "se::ui")]
pub(crate) mod ffi {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum EmulationState {
        Unconfigured,
        Building,
        Saving,
        Loading,
        Paused,
        Running,
        Idle,
        Faulted,
        ShuttingDown,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum PersistenceOutcome {
        None,
        Saved,
        Loaded,
        PromRequired,
        Warning,
        Failed,
    }

    struct EmulationSnapshot {
        state: EmulationState,
        session_id: u64,
        sim_time: u64,
        has_machine: bool,
        error_id: u64,
        error_message: String,
        persistence_id: u64,
        persistence_outcome: PersistenceOutcome,
        persistence_message: String,
        prom_path: String,
        rtc_mode: u8,
        jit_enabled: bool,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum UiDisplayField {
        Progressive,
        First,
        Second,
    }

    struct UiDisplayUpdate {
        generation: u64,
        session_id: u64,
        has_frame: bool,
        sequence: u64,
        completed_at: u64,
        width: u32,
        height: u32,
        stride: u32,
        field: UiDisplayField,
        rgba: Vec<u8>,
        machine_dropped: u64,
        transport_dropped: u64,
        invalid_frames: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum UiSerialPort {
        Serial1,
        Serial2,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TerminalInputStatus {
        Accepted,
        Unavailable,
        QueueFull,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum UiTerminalColorKind {
        Default,
        Indexed,
        Rgb,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum UiTerminalKey {
        Text,
        Enter,
        Backspace,
        Tab,
        Escape,
        Up,
        Down,
        Right,
        Left,
        Home,
        End,
        Insert,
        Delete,
        PageUp,
        PageDown,
    }

    struct UiTerminalChunk {
        session_id: u64,
        port: UiSerialPort,
        bytes: Vec<u8>,
    }

    struct UiTerminalIoStats {
        sent: u64,
        received: u64,
        dropped: u64,
    }

    struct UiTerminalCell {
        text: String,
        foreground_kind: UiTerminalColorKind,
        foreground_index: u8,
        foreground_red: u8,
        foreground_green: u8,
        foreground_blue: u8,
        background_kind: UiTerminalColorKind,
        background_index: u8,
        background_red: u8,
        background_green: u8,
        background_blue: u8,
        bold: bool,
        dim: bool,
        italic: bool,
        underline: bool,
        inverse: bool,
        wide: bool,
        wide_continuation: bool,
    }

    struct UiTerminalSnapshot {
        rows: u16,
        columns: u16,
        cells: Vec<UiTerminalCell>,
        cursor_row: u16,
        cursor_column: u16,
        cursor_visible: bool,
        scrollback: usize,
        maximum_scrollback: usize,
        bell_count: u64,
    }

    extern "Rust" {
        type EmulationController;

        fn request_run(self: &EmulationController) -> bool;
        fn request_pause(self: &EmulationController) -> bool;
        fn request_hard_reset(self: &EmulationController) -> bool;
        fn configure_prom(self: &EmulationController, prom: &[u8]) -> bool;
        fn configure_machine(
            self: &EmulationController,
            prom_path: &str,
            prom: &[u8],
            rtc_mode: u8,
            jit_enabled: bool,
        ) -> bool;
        fn request_save_state(self: &EmulationController, path: &str) -> bool;
        fn request_load_state(self: &EmulationController, path: &str, prom_override: &str) -> bool;
        fn snapshot(self: &EmulationController) -> EmulationSnapshot;
        fn take_display_update(self: &EmulationController) -> UiDisplayUpdate;
        fn submit_terminal_input(
            self: &EmulationController,
            port: UiSerialPort,
            bytes: &[u8],
        ) -> TerminalInputStatus;
        fn drain_terminal_output(
            self: &EmulationController,
            max_bytes: usize,
        ) -> Vec<UiTerminalChunk>;
        fn terminal_io_stats(self: &EmulationController, port: UiSerialPort) -> UiTerminalIoStats;

        type TerminalModel;

        fn new_terminal_model() -> Box<TerminalModel>;
        fn process_output(self: &mut TerminalModel, port: UiSerialPort, bytes: &[u8]);
        fn clear(self: &mut TerminalModel, port: UiSerialPort);
        fn clear_all(self: &mut TerminalModel);
        fn set_scrollback(self: &mut TerminalModel, port: UiSerialPort, rows: usize);
        fn snapshot(self: &mut TerminalModel, port: UiSerialPort) -> UiTerminalSnapshot;
        fn selected_text(
            self: &TerminalModel,
            port: UiSerialPort,
            start_row: u16,
            start_column: u16,
            end_row: u16,
            end_column: u16,
        ) -> String;
        fn encode_key(
            self: &TerminalModel,
            port: UiSerialPort,
            key: UiTerminalKey,
            text: &str,
            control: bool,
            alt: bool,
        ) -> Vec<u8>;
        fn encode_paste(self: &TerminalModel, port: UiSerialPort, text: &str) -> Vec<u8>;
    }

    unsafe extern "C++" {
        include!("se_ui/include/application.h");

        fn run_application(
            version: &str,
            arguments: Vec<String>,
            controller: &EmulationController,
        ) -> i32;
    }
}

/// Runs the native Qt application.
pub fn run(version: &str, arguments: Vec<String>) -> i32 {
    let controller = EmulationController::new_with_version(version);
    let exit_code = ffi::run_application(version, arguments, &controller);
    controller.shutdown();
    exit_code
}
