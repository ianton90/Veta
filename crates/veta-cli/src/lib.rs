//! Veta's command-line interface. Parses arguments with clap and runs
//! headless commands through `veta-core`.

use clap::Parser;

/// Parquet viewer and editor.
///
/// Run without arguments to open the desktop app.
#[derive(Debug, Parser)]
#[command(name = "veta", version, about, long_about)]
struct Cli {}

/// What the `veta` binary should do for the given arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    /// Open the desktop app.
    Gui,
}

/// Parses the process arguments. Prints help/version or an argument error
/// and exits the process when clap requires it.
pub fn parse() -> Invocation {
    let Cli {} = Cli::parse();
    Invocation::Gui
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments_parses() {
        assert!(Cli::try_parse_from(["veta"]).is_ok());
    }
}
