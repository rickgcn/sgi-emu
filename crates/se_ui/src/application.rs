#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("se_ui/include/application.h");

        #[namespace = "se::ui"]
        fn run_application(version: &str, arguments: Vec<String>) -> i32;
    }
}

/// Runs the native Qt application.
pub fn run(version: &str, arguments: Vec<String>) -> i32 {
    ffi::run_application(version, arguments)
}
