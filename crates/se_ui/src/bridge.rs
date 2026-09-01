//! Data transferred across the Rust and Qt boundary.

use crate::session::UiSession;
use crate::terminal::{TerminalModel, new_terminal_model, normalize_terminal_paste};

#[cxx::bridge(namespace = "se_ui")]
pub mod ffi {
    /// A frontend-neutral external serial port.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum SerialPortDto {
        A,
        B,
    }

    /// A semantic terminal key interpreted by the Rust terminal model.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum TerminalKeyDto {
        Text,
        Control,
        Up,
        Down,
        Right,
        Left,
        Keypad,
        Pf1,
        Pf2,
        Pf3,
        Pf4,
        Enter,
        Backspace,
        Tab,
        Escape,
        Delete,
    }

    /// A terminal color encoded for Qt rendering.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct TerminalColorDto {
        pub kind: u8,
        pub value: u32,
    }

    /// One visible terminal cell.
    #[derive(Debug, Eq, PartialEq)]
    pub struct TerminalCellDto {
        pub text: String,
        pub foreground: TerminalColorDto,
        pub background: TerminalColorDto,
        pub attributes: u8,
    }

    /// A complete visible terminal grid.
    #[derive(Debug, Eq, PartialEq)]
    pub struct TerminalSnapshotDto {
        pub columns: u16,
        pub rows: u16,
        pub cells: Vec<TerminalCellDto>,
        pub cursor_row: u16,
        pub cursor_column: u16,
        pub cursor_visible: bool,
        pub scrollback_rows: u32,
        pub scrollback_offset: u32,
    }

    /// Values used to initialize the Qt user interface.
    #[derive(Debug)]
    pub struct UiStartupState {
        /// Stable machine identifier.
        pub machine_model: String,
        /// Path to the selected PROM image.
        pub prom_path: String,
        /// Stable floating-point backend identifier.
        pub float_backend: String,
        /// Base64-encoded `QWidget::saveGeometry()` bytes.
        pub window_geometry: String,
        /// Base64-encoded `QMainWindow::saveState()` bytes.
        pub window_state: String,
        /// Machine construction error detected before the UI starts.
        pub startup_error: String,
    }

    /// Values returned after the Qt event loop exits normally.
    #[derive(Debug)]
    pub struct UiExitState {
        /// Stable machine identifier.
        pub machine_model: String,
        /// Path to the selected PROM image.
        pub prom_path: String,
        /// Stable floating-point backend identifier.
        pub float_backend: String,
        /// Base64-encoded `QWidget::saveGeometry()` bytes.
        pub window_geometry: String,
        /// Base64-encoded `QMainWindow::saveState()` bytes.
        pub window_state: String,
    }

    /// Runtime status returned by a control command.
    #[derive(Debug)]
    pub struct RuntimeStatusDto {
        /// Whether the command succeeded.
        pub success: bool,
        /// Runtime state identifier.
        pub state: u8,
        /// Debugger-visible revision.
        pub revision: u64,
        /// Most recent execution error.
        pub execution_error: String,
        /// Command error when `success` is false.
        pub command_error: String,
    }

    /// Complete register debugger payload.
    #[derive(Debug)]
    pub struct RegistersDto {
        /// Whether the request succeeded.
        pub success: bool,
        /// Command error when `success` is false.
        pub error: String,
        /// Runtime revision.
        pub revision: u64,
        /// Program counter.
        pub pc: u32,
        /// HI register.
        pub hi: u32,
        /// LO register.
        pub lo: u32,
        /// General-purpose registers.
        pub gpr: Vec<u32>,
        /// Human-readable pending delay-slot state.
        pub delay_slot: String,
        /// Human-readable pending GPR state.
        pub pending_gpr: String,
        /// Human-readable pending CP0 state.
        pub pending_cp0: String,
        /// Human-readable pending CP1 state.
        pub pending_cp1: String,
        /// CP0 register values.
        pub cp0: Vec<u32>,
        /// Effective CP0 functional state.
        pub cp0_effective: Vec<u32>,
        /// Pending CP0 functional state, if present.
        pub cp0_pending_effective: Vec<u32>,
        /// CP1 general registers.
        pub cp1: Vec<u32>,
        /// FCR0 implementation and revision.
        pub fcr0: u32,
        /// FCR30 exception instruction register.
        pub fcr30: u32,
        /// FCR31 control/status register.
        pub fcr31: u32,
        /// Selected floating-point backend.
        pub float_backend: String,
        /// CP1 interrupt output.
        pub cp1_interrupt: bool,
    }

    /// One decoded TLB entry.
    #[derive(Debug)]
    pub struct TlbEntryDto {
        pub index: u32,
        pub entry_hi: u32,
        pub entry_lo: u32,
        pub vpn: u32,
        pub asid: u8,
        pub pfn: u32,
        pub noncacheable: bool,
        pub dirty: bool,
        pub valid: bool,
        pub global: bool,
    }

    /// Complete TLB debugger payload.
    #[derive(Debug)]
    pub struct TlbDto {
        pub success: bool,
        pub error: String,
        pub revision: u64,
        pub instruction_view: bool,
        pub shutdown: bool,
        pub index: u32,
        pub random: u32,
        pub entries: Vec<TlbEntryDto>,
    }

    /// One physical cache entry.
    #[derive(Debug)]
    pub struct CacheEntryDto {
        pub index: u32,
        pub page_frame: u32,
        pub word: u32,
        pub valid: bool,
    }

    /// Complete cache debugger payload.
    #[derive(Debug)]
    pub struct CacheDto {
        pub success: bool,
        pub error: String,
        pub revision: u64,
        pub instruction_cache: bool,
        pub refill_bytes: u32,
        pub entries: Vec<CacheEntryDto>,
    }

    /// One disassembled instruction row.
    #[derive(Debug)]
    pub struct DisassemblyLineDto {
        pub address: u32,
        pub readable: bool,
        pub word: u32,
        pub text: String,
        pub current: bool,
        pub breakpoint: bool,
    }

    /// Complete disassembly debugger payload.
    #[derive(Debug)]
    pub struct DisassemblyDto {
        pub success: bool,
        pub error: String,
        pub revision: u64,
        pub lines: Vec<DisassemblyLineDto>,
    }

    /// Complete memory debugger payload.
    #[derive(Debug)]
    pub struct MemoryDto {
        pub success: bool,
        pub error: String,
        pub revision: u64,
        pub virtual_address_space: bool,
        pub start: u64,
        pub values: Vec<u8>,
        pub readable: Vec<u8>,
    }

    extern "Rust" {
        type UiSession;
        type TerminalModel;

        fn new_terminal_model() -> Box<TerminalModel>;
        fn terminal_feed(self: Pin<&mut TerminalModel>, bytes: &[u8]) -> TerminalSnapshotDto;
        fn terminal_snapshot(
            self: Pin<&mut TerminalModel>,
            scrollback_offset: u32,
        ) -> TerminalSnapshotDto;
        fn terminal_clear(self: Pin<&mut TerminalModel>) -> TerminalSnapshotDto;
        fn terminal_encode_key(self: &TerminalModel, key: TerminalKeyDto, value: u8) -> Vec<u8>;
        fn terminal_selection(
            self: Pin<&mut TerminalModel>,
            start_row: u32,
            start_column: u16,
            end_row: u32,
            end_column: u16,
        ) -> String;
        fn normalize_terminal_paste(text: &str) -> Vec<u8>;

        fn runtime_status(self: &UiSession) -> RuntimeStatusDto;
        fn configure_machine(
            self: &UiSession,
            model: &str,
            prom_path: &str,
            float_backend: &str,
        ) -> RuntimeStatusDto;
        fn run_machine(self: &UiSession) -> RuntimeStatusDto;
        fn reset_machine(self: &UiSession) -> RuntimeStatusDto;
        fn pause_machine(self: &UiSession) -> RuntimeStatusDto;
        fn step_machine(self: &UiSession) -> RuntimeStatusDto;
        fn registers(self: &UiSession) -> RegistersDto;
        fn tlb(self: &UiSession, instruction_view: bool) -> TlbDto;
        fn cache(self: &UiSession, instruction_cache: bool) -> CacheDto;
        fn disassembly(self: &UiSession, start: u32, row_count: u32) -> DisassemblyDto;
        fn memory(
            self: &UiSession,
            virtual_address_space: bool,
            start: u64,
            length: u32,
        ) -> MemoryDto;
        fn toggle_breakpoint(self: &UiSession, address: u32) -> RuntimeStatusDto;
        fn send_serial(self: &UiSession, port: SerialPortDto, bytes: &[u8]) -> RuntimeStatusDto;
        fn attach_machine_output(
            self: &UiSession,
            sink: SharedPtr<MachineOutputSink>,
        ) -> RuntimeStatusDto;
        fn detach_machine_output(self: &UiSession);
    }

    unsafe extern "C++" {
        include!("se_ui/main_window.h");
        include!("se_ui/serial_console_dock.h");

        type MachineOutputSink;

        fn publish_serial(self: &MachineOutputSink, serial_a: &[u8], serial_b: &[u8]);

        /// Runs the Qt event loop and returns the final user-interface state.
        fn run_gui(session: &UiSession, startup: &UiStartupState) -> UiExitState;
    }
}

// SAFETY: `MachineOutputSink::publish_serial` only mutates mutex-protected
// buffers and schedules GUI work through a queued Qt invocation.
unsafe impl Send for ffi::MachineOutputSink {}

// SAFETY: all shared state in `MachineOutputSink` is protected by its mutex;
// the referenced Qt widgets are accessed only by the queued GUI-thread drain.
unsafe impl Sync for ffi::MachineOutputSink {}
