#![deny(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use pstforge_core::{EncryptionMode, FileFormat, InfoReport, InspectionError, VerifyReport};
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(
    name = "pstforge",
    version,
    about = "Read-only PST recovery inspection"
)]
pub struct Cli {
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto, global = true)]
    pub color: ColorChoice,

    #[arg(long, value_enum, default_value_t = LogFormat::Human, global = true)]
    pub log_format: LogFormat,

    #[arg(long, global = true)]
    pub quiet: bool,

    #[arg(short = 'v', action = ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect PST format and source identity without traversing mail.
    Info {
        source: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Traverse reachable folders and count their messages.
    Verify {
        source: PathBuf,
        #[arg(long, value_enum, default_value_t = VerifyMode::Full)]
        mode: VerifyMode,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum VerifyMode {
    Full,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Inspection(#[from] InspectionError),
    #[error("cannot write command result: {0}")]
    Output(#[from] std::io::Error),
    #[error("cannot serialize command result: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    Complete,
    SourceIncomplete,
}

impl CliError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Inspection(
                InspectionError::Source(_)
                | InspectionError::Pff(_)
                | InspectionError::UnsupportedContentType { .. },
            ) => 3,
            Self::Inspection(InspectionError::SizeMismatch { .. })
            | Self::Output(_)
            | Self::Json(_) => 6,
        }
    }
}

pub fn execute(cli: &Cli, output: &mut dyn Write) -> Result<CommandStatus, CliError> {
    match &cli.command {
        Command::Info { source, json } => {
            let report = pstforge_core::info(source)?;
            if *json {
                write_json(output, &report)?;
            } else {
                write_info(output, &report)?;
            }
            Ok(CommandStatus::Complete)
        }
        Command::Verify { source, json, .. } => {
            let report = pstforge_core::verify(source)?;
            if *json {
                write_json(output, &report)?;
            } else {
                write_verify(output, &report)?;
            }
            if report.inventory.issues.is_empty() {
                Ok(CommandStatus::Complete)
            } else {
                Ok(CommandStatus::SourceIncomplete)
            }
        }
    }
}

fn write_json<T: serde::Serialize + ?Sized>(
    output: &mut dyn Write,
    report: &T,
) -> Result<(), CliError> {
    serde_json::to_writer_pretty(&mut *output, report)?;
    writeln!(output)?;
    Ok(())
}

fn write_info(output: &mut dyn Write, report: &InfoReport) -> Result<(), std::io::Error> {
    writeln!(output, "PSTForge inspection")?;
    write_common(output, report)
}

fn write_verify(output: &mut dyn Write, report: &VerifyReport) -> Result<(), std::io::Error> {
    writeln!(output, "PSTForge verification")?;
    writeln!(output, "Source: {}", report.source.canonical_path)?;
    writeln!(output, "SHA-256: {}", report.source.sha256)?;
    writeln!(output, "Size: {} bytes", report.source.size_bytes)?;
    writeln!(output, "Format: {}", format_name(report.pst.format))?;
    writeln!(output, "Content type: {}", report.pst.content_type)?;
    writeln!(
        output,
        "Corruption observed: {}",
        yes_no(report.pst.corruption_observed)
    )?;
    writeln!(output, "Folders: {}", report.inventory.folders)?;
    writeln!(output, "Normal items: {}", report.inventory.normal_items)?;
    write_optional_count(output, "Recovered items", report.inventory.recovered_items)?;
    write_optional_count(output, "Orphan items", report.inventory.orphan_items)?;
    writeln!(
        output,
        "Traversal issues: {}",
        report.inventory.issues.len()
    )?;
    writeln!(
        output,
        "Source unchanged: {}",
        yes_no(report.source_unchanged)
    )?;
    Ok(())
}

fn write_common(output: &mut dyn Write, report: &InfoReport) -> Result<(), std::io::Error> {
    writeln!(output, "Source: {}", report.source.canonical_path)?;
    writeln!(output, "SHA-256: {}", report.source.sha256)?;
    writeln!(output, "Size: {} bytes", report.source.size_bytes)?;
    writeln!(output, "Modified: {}", report.source.modified_at)?;
    writeln!(output, "Content type: {}", report.pst.content_type)?;
    writeln!(output, "Format: {}", format_name(report.pst.format))?;
    match report.pst.page_size_bytes {
        Some(size) => writeln!(output, "Page size: {size} bytes")?,
        None => writeln!(output, "Page size: unknown")?,
    }
    writeln!(
        output,
        "Encryption: {}",
        encryption_name(report.pst.encryption)
    )?;
    writeln!(
        output,
        "Corruption observed: {}",
        yes_no(report.pst.corruption_observed)
    )?;
    writeln!(output, "libpff: {}", report.producer.libpff_version)?;
    writeln!(
        output,
        "Source unchanged: {}",
        yes_no(report.source_unchanged)
    )?;
    Ok(())
}

fn format_name(format: FileFormat) -> &'static str {
    match format {
        FileFormat::Ansi32 => "ANSI 32-bit",
        FileFormat::Unicode64 => "Unicode 64-bit",
        FileFormat::Unicode64With4kPages => "Unicode 64-bit (4 KiB pages)",
        FileFormat::Unknown => "unknown",
    }
}

fn encryption_name(mode: EncryptionMode) -> &'static str {
    match mode {
        EncryptionMode::None => "none",
        EncryptionMode::Compressible => "compressible",
        EncryptionMode::High => "high",
        EncryptionMode::Unknown => "unknown",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn write_optional_count(
    output: &mut dyn Write,
    label: &str,
    value: Option<u64>,
) -> Result<(), std::io::Error> {
    match value {
        Some(count) => writeln!(output, "{label}: {count}"),
        None => writeln!(output, "{label}: not scanned"),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, VerifyMode};

    #[test]
    fn parses_info_json_with_global_options_after_command() {
        let cli = Cli::try_parse_from([
            "pstforge",
            "info",
            "mail.pst",
            "--json",
            "--log-format",
            "json",
        ]);
        assert!(cli.is_ok());
    }

    #[test]
    fn verify_defaults_to_full() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["pstforge", "verify", "mail.pst"])?;
        assert!(matches!(
            cli.command,
            Command::Verify {
                mode: VerifyMode::Full,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn recovery_mode_is_not_exposed_before_implementation() {
        let cli = Cli::try_parse_from(["pstforge", "verify", "mail.pst", "--mode", "recovery"]);
        assert!(cli.is_err());
    }
}
