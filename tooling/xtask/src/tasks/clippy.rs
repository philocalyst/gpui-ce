#![allow(clippy::disallowed_methods, reason = "tooling is exempt")]
use std::process::Command;

use crate::error::{Result, XtaskError};
use clap::Parser;

#[derive(Parser)]
pub struct ClippyArgs {
    /// Automatically apply lint suggestions (`clippy --fix`).
    #[arg(long)]
    fix: bool,

    /// The package to run Clippy against (`cargo -p <PACKAGE> clippy`).
    #[arg(long, short)]
    package: Option<String>,
}

pub fn run_clippy(args: ClippyArgs) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    let mut clippy_command = Command::new(&cargo);
    clippy_command.arg("clippy");

    if let Some(package) = args.package.as_ref() {
        clippy_command.args(["--package", package]);
    } else {
        clippy_command.arg("--workspace");
    }

    clippy_command
        .arg("--release")
        .arg("--all-targets")
        .arg("--all-features");

    if args.fix {
        clippy_command.arg("--fix");
    }

    clippy_command.arg("--");

    // Deny all warnings.
    clippy_command.args(["--deny", "warnings"]);

    eprintln!(
        "running: {cargo} {}",
        clippy_command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ")
    );

    let exit_status = clippy_command.spawn()?.wait()?;

    if !exit_status.success() {
        return Err(XtaskError::CommandFailed {
            command: format!(
                "{} {}",
                cargo,
                clippy_command
                    .get_args()
                    .map(|arg| arg.to_str().unwrap())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            exit_code: exit_status.code(),
        });
    }

    Ok(())
}
