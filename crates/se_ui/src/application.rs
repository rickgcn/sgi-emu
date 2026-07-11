use crate::emulation::EmulationController;

#[cxx::bridge(namespace = "se::ui")]
pub(crate) mod ffi {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum EmulationState {
        Unconfigured,
        Building,
        Paused,
        Running,
        Idle,
        Faulted,
        ShuttingDown,
    }

    struct EmulationSnapshot {
        state: EmulationState,
        session_id: u64,
        sim_time: u64,
        has_machine: bool,
        error_id: u64,
        error_message: String,
    }

    extern "Rust" {
        type EmulationController;

        fn request_run(self: &EmulationController) -> bool;
        fn request_pause(self: &EmulationController) -> bool;
        fn request_hard_reset(self: &EmulationController) -> bool;
        fn configure_prom(self: &EmulationController, prom: &[u8]) -> bool;
        fn snapshot(self: &EmulationController) -> EmulationSnapshot;
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
    let controller = EmulationController::new();
    let exit_code = ffi::run_application(version, arguments, &controller);
    controller.shutdown();
    exit_code
}
