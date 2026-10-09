//! The `veta` binary: no arguments open the GUI, a subcommand runs the CLI.

use std::process::ExitCode;

use veta_cli::Invocation;

fn main() -> ExitCode {
    match veta_cli::parse() {
        Invocation::Gui { files } => match veta_gui::run(files) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("veta: {e}");
                ExitCode::FAILURE
            }
        },
        Invocation::Cli(command) => veta_cli::run(command),
    }
}
