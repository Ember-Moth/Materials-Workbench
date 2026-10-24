//! `mw`, the Materials Workbench command line (`docs/design/scripting.md`
//! §4): structure files converted, made into supercells, described in
//! another cell, checked for symmetry and symmetrized, a batch at a time.
//!
//! Every command reads its files as the application does and works on
//! each structure through the capability registry (`mw-scripting`), so
//! the results are the application's. Data goes to stdout: file text, the
//! reports, or with `--json` one JSON document. Messages go to stderr.
//! The exit status is 0 when everything worked, 1 when a file or a
//! structure failed (the others are still done), and 2 for a usage error.

mod args;
mod commands;
mod report;

use std::process::ExitCode;

use clap::Parser;

use crate::args::{Cli, Command};
use crate::report::Report;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut report = Report::new(cli.json);
    match cli.command {
        Command::Info(inputs) => commands::info(&inputs, &mut report),
        Command::Diagnostics(inputs) => commands::diagnostics(&inputs, &mut report),
        Command::Convert { inputs, output } => commands::convert(&inputs, &output, &mut report),
        Command::Supercell {
            inputs,
            size,
            output,
        } => commands::supercell(&inputs, &size, &output, &mut report),
        Command::Transform {
            inputs,
            cell,
            output,
        } => commands::transform(&inputs, &cell, &output, &mut report),
        Command::Symmetry { inputs, options } => commands::symmetry(&inputs, &options, &mut report),
        Command::Symmetrize {
            inputs,
            options,
            output,
        } => commands::symmetrize(&inputs, &options, &output, &mut report),
        Command::Call {
            capability,
            inputs,
            args,
        } => commands::call(&capability, &inputs, &args, &mut report),
        Command::Capabilities => commands::capabilities(&mut report),
    }
    report.finish()
}
