//! Application entry point.

mod config;

use std::error::Error;

use se_runtime::runtime::Runtime;
use se_ui::session::UiSession;

fn main() -> Result<(), Box<dyn Error>> {
    let config_path = config::config_path()?;
    let mut application_config = config::load(&config_path)?;
    application_config.apply_environment();

    let startup = application_config.ui_startup_state();
    let runtime = Runtime::new_unconfigured()?;
    let mut session = UiSession::new(runtime);
    let exit = session.run(&startup);
    session.shutdown()?;

    application_config.apply_ui_exit_state(exit);
    config::save(&config_path, &application_config)?;

    Ok(())
}
