//! Data transferred across the Rust and Qt boundary.

#[cxx::bridge(namespace = "se_ui")]
pub mod ffi {
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

    unsafe extern "C++" {
        include!("se_ui/main_window.h");

        /// Runs the Qt event loop and returns the final user-interface state.
        fn run_gui(startup: &UiStartupState) -> UiExitState;
    }
}
