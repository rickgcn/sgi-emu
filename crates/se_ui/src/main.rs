fn main() {
    let arguments = std::env::args_os()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let exit_code = se_ui::application::run(env!("CARGO_PKG_VERSION"), arguments);

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}
