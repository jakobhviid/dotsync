//! Shell completions and man page, generated from the same clap definition so
//! they never drift from the actual command surface.

use std::io;

use clap::CommandFactory;
use clap_complete::Shell;

use crate::Cli;

/// Print a completion script for `shell` to stdout.
pub fn print_completions(shell: Shell) {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, &mut io::stdout());
}

/// Print the roff man page to stdout.
pub fn print_man() -> anyhow::Result<()> {
    let cmd = Cli::command();
    let man = clap_mangen::Man::new(cmd);
    man.render(&mut io::stdout())?;
    Ok(())
}
