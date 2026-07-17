#![deny(unsafe_code)]

use std::io;
use std::process::ExitCode;

use clap::Parser;
use pstforge_cli::{Cli, ColorChoice, CommandStatus, LogFormat};
use tracing::error;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(&cli);
    let stdout = io::stdout();
    let mut output = stdout.lock();
    match pstforge_cli::execute(&cli, &mut output) {
        Ok(CommandStatus::Complete) => ExitCode::SUCCESS,
        Ok(CommandStatus::Partial) => ExitCode::from(1),
        Ok(CommandStatus::Interrupted) => ExitCode::from(130),
        Err(run_error) => {
            error!(error = %run_error, "command failed");
            eprintln!("pstforge: {run_error}");
            ExitCode::from(run_error.exit_code())
        }
    }
}

fn init_logging(cli: &Cli) {
    let directive = if cli.quiet {
        "off"
    } else {
        match cli.verbose {
            0 => "info",
            1 => "debug",
            2 => "trace",
            _ => "trace",
        }
    };
    let filter = EnvFilter::builder()
        .with_default_directive(tracing::level_filters::LevelFilter::WARN.into())
        .parse_lossy(directive);
    let ansi = match cli.color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => std::io::IsTerminal::is_terminal(&std::io::stderr()),
    };

    match cli.log_format {
        LogFormat::Human => {
            if let Err(init_error) = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(ansi)
                .with_writer(io::stderr)
                .try_init()
            {
                eprintln!("pstforge: cannot initialize logging: {init_error}");
            }
        }
        LogFormat::Json => {
            if let Err(init_error) = tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(io::stderr)
                .try_init()
            {
                eprintln!("pstforge: cannot initialize logging: {init_error}");
            }
        }
    }
}
