#![cfg_attr(
    all(windows, not(debug_assertions), not(test)),
    windows_subsystem = "windows"
)]

//! Application entry point and runtime lifetime owner.

mod config;

use std::error::Error;
use std::io;

use se_runtime::runtime::Runtime;
use se_session::machine::{StartupReadiness, machine_definition, startup_readiness};
use se_ui::bridge::ffi::NetworkConfiguration;
use se_ui::session::UiSession;

fn main() -> Result<(), Box<dyn Error>> {
    let config_path = config::config_path()?;
    let mut application_config = config::load(&config_path)?;
    let machine_draft = application_config.machine_draft();
    let network = application_config.network_configuration();
    let runtime = Runtime::new_unconfigured()?;
    let handle = runtime.handle();
    let (startup_error, active_frontend) =
        if startup_readiness(&machine_draft) == StartupReadiness::Unconfigured {
            (String::new(), se_session::frontend::FrontendPlan::default())
        } else {
            match build_normal_configuration(machine_draft.clone(), &network) {
                Ok(build) => {
                    let (configuration, frontend) = build.into_parts();
                    match handle.configure_with(configuration) {
                        Ok(_) => (String::new(), frontend),
                        Err(error) => (
                            error.to_string(),
                            se_session::frontend::FrontendPlan::default(),
                        ),
                    }
                }
                Err(error) => (error, se_session::frontend::FrontendPlan::default()),
            }
        };

    let startup = application_config.ui_startup_state(startup_error);
    let session = UiSession::new(
        handle,
        machine_draft,
        machine_definition(),
        active_frontend,
        Box::new(|draft| Ok(se_session::normal::preflight_configuration(&draft))),
        Box::new(build_normal_configuration),
        Box::new(|draft, network, path| {
            let network = config::parse_network_configuration(network)?;
            se_session::recording::build_configuration(draft, network, path)
                .map_err(|error| error.to_string())
        }),
        Box::new(|draft, path, snapshot_id| {
            se_session::replay::build_configuration(draft, path, snapshot_id)
                .map_err(|error| error.to_string())
        }),
        Box::new(|network| config::parse_network_configuration(network).map(|_| ())),
    );
    let exit = session.run(&startup);
    let committed = session.machine_draft_snapshot();
    drop(session);
    if let Some(state) = runtime.shutdown()? {
        se_session::persistence::save(&state)?;
    }

    application_config
        .apply_ui_exit_state(exit)
        .map_err(io::Error::other)?;
    application_config.set_machine_draft(committed);
    config::save(&config_path, &application_config)?;
    Ok(())
}

fn build_normal_configuration(
    draft: se_config::draft::MachineDraft,
    network: &NetworkConfiguration,
) -> Result<se_session::frontend::SessionBuild, String> {
    let network = config::parse_network_configuration(network)?;
    se_session::normal::build_configuration(draft, network).map_err(|error| error.to_string())
}
