//! The app and the player are their own plugin hosts: started with
//! [`FLAG`], the executable serves plugins instead of opening as usual
//! (`docs/design/vst3-hosting.md`).

use daw_plugins::client::HostCommand;
use std::process::ExitCode;

/// The first argument that starts this executable as a plugin host.
pub const FLAG: &str = "--plugin-host";

/// How to start a plugin host: this executable, in plugin-host mode.
pub fn host_command() -> Option<HostCommand> {
    Some(HostCommand {
        program: std::env::current_exe().ok()?,
        args: vec![FLAG.to_owned()],
    })
}

/// Serves plugins if this process was started as a plugin host, returning
/// its exit code; `None` otherwise. Call first thing in `main`.
pub fn run_if_host() -> Option<ExitCode> {
    (std::env::args().nth(1).as_deref() == Some(FLAG)).then(daw_plugins::host::run)
}
