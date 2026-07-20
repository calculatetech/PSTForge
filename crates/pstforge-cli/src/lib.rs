#![deny(unsafe_code)]

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use pstforge_core::{
    EncryptionMode, FileFormat, InfoReport, InspectionError, RecoveryError, RecoveryMode,
    RecoveryReport, SourceError, SplitError, SplitFailureKind, SplitReport, VerifyReport,
    WorkerProtocolError,
};
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
    /// Stream and account for reachable mail and its content.
    Verify {
        source: PathBuf,
        #[arg(long, value_enum, default_value_t = VerifyMode::Full)]
        mode: VerifyMode,
        #[arg(long)]
        json: bool,
    },
    /// Recover reachable, deleted, and orphan mail into a durable private job spool.
    Recover {
        source: PathBuf,
        #[arg(long, short = 'o')]
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = RecoveryModeArg::Balanced)]
        recovery: RecoveryModeArg,
        #[arg(long)]
        json: bool,
    },
    /// Recover mail and write independently importable size-limited PST parts.
    Split {
        source: PathBuf,
        #[arg(long, short = 'o')]
        output: PathBuf,
        #[arg(long, value_parser = parse_byte_size, default_value = "4GiB")]
        max_pst_size: u64,
        #[arg(long, value_enum, default_value_t = RecoveryModeArg::Balanced)]
        recovery: RecoveryModeArg,
        /// Retain a durable payload spool so an interrupted job can resume.
        #[arg(long)]
        restartable: bool,
        #[arg(long)]
        resume: bool,
        #[arg(long)]
        keep_work: bool,
        #[arg(long)]
        json: bool,
    },
    #[command(name = "__worker", hide = true)]
    Worker {
        source: PathBuf,
        expected_identity: String,
        skipped_units: String,
        #[arg(value_enum)]
        recovery: RecoveryModeArg,
        #[arg(long, hide = true)]
        metadata_only: bool,
        #[arg(long, hide = true)]
        writer_order: bool,
    },
    #[command(name = "__validator", hide = true)]
    Validator {
        expected_parent: i32,
        #[arg(value_enum)]
        tool: ValidatorToolArg,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<OsString>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RecoveryModeArg {
    Balanced,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ValidatorToolArg {
    Pffinfo,
    Readpst,
}

impl From<RecoveryModeArg> for RecoveryMode {
    fn from(value: RecoveryModeArg) -> Self {
        match value {
            RecoveryModeArg::Balanced => Self::Balanced,
            RecoveryModeArg::Aggressive => Self::Aggressive,
        }
    }
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Inspection(#[from] InspectionError),
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
    #[error(transparent)]
    Split(#[from] SplitError),
    #[error(transparent)]
    Worker(#[from] WorkerProtocolError),
    #[error("cannot write command result: {0}")]
    Output(#[from] std::io::Error),
    #[error("cannot serialize command result: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cannot locate the pstforge executable: {0}")]
    Executable(std::io::Error),
    #[error("cannot supervise the independent PST validator: {0}")]
    Validator(std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    Complete,
    Partial,
    Interrupted,
}

impl CliError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Split(error) => match error.failure_kind() {
                SplitFailureKind::Source => 3,
                SplitFailureKind::Output => 4,
                SplitFailureKind::Conformance => 5,
                SplitFailureKind::Interrupted => 130,
                SplitFailureKind::Internal => 6,
            },
            Self::Inspection(
                InspectionError::Source(_)
                | InspectionError::Pff(_)
                | InspectionError::UnsupportedContentType { .. },
            ) => 3,
            Self::Recovery(RecoveryError::Job(_))
            | Self::Recovery(RecoveryError::WorkerProtocol(WorkerProtocolError::Sink(_)))
            | Self::Recovery(RecoveryError::Source(SourceError::UnsafeOutput(_)))
            | Self::Output(_) => 4,
            Self::Recovery(RecoveryError::Source(_))
            | Self::Recovery(RecoveryError::WorkerProtocol(WorkerProtocolError::ReportedSource(
                _,
            ))) => 3,
            Self::Recovery(RecoveryError::Interrupted) => 130,
            Self::Inspection(InspectionError::SizeMismatch { .. }) => 3,
            Self::Recovery(
                RecoveryError::WorkerProtocol(_)
                | RecoveryError::WorkerExecutable(_)
                | RecoveryError::WorkerSpawn(_)
                | RecoveryError::MissingWorkerOutput
                | RecoveryError::WorkerExit { .. }
                | RecoveryError::WorkerStalled
                | RecoveryError::InconsistentCounters
                | RecoveryError::ResumeConfigurationRequired,
            )
            | Self::Worker(_)
            | Self::Json(_)
            | Self::Executable(_)
            | Self::Validator(_) => 6,
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
            if report.inventory.issues.is_empty() && report.inventory.issues_dropped == 0 {
                Ok(CommandStatus::Complete)
            } else {
                Ok(CommandStatus::Partial)
            }
        }
        Command::Recover {
            source,
            output: job_directory,
            recovery,
            json,
        } => {
            let executable = std::env::current_exe().map_err(CliError::Executable)?;
            let report =
                pstforge_core::recover(source, job_directory, &executable, (*recovery).into())?;
            if *json {
                write_json(output, &report)?;
            } else {
                write_recovery(output, &report)?;
            }
            if report.interrupted {
                Ok(CommandStatus::Interrupted)
            } else if report.partial_candidates == 0
                && report.unsupported_candidates == 0
                && report.issues == 0
                && report.issues_dropped == 0
            {
                Ok(CommandStatus::Complete)
            } else {
                Ok(CommandStatus::Partial)
            }
        }
        Command::Split {
            source,
            output: job_directory,
            max_pst_size,
            recovery,
            restartable,
            resume,
            keep_work,
            json,
        } => {
            let executable = std::env::current_exe().map_err(CliError::Executable)?;
            let report = pstforge_core::split(
                source,
                job_directory,
                &executable,
                (*recovery).into(),
                pstforge_core::SplitOptions {
                    maximum_pst_bytes: *max_pst_size,
                    restartable: *restartable,
                    resume: *resume,
                    keep_work: *keep_work,
                },
            )?;
            if *json {
                write_json(output, &report)?;
            } else {
                write_split(output, &report)?;
            }
            if report.recovery.interrupted {
                Ok(CommandStatus::Interrupted)
            } else if report.partial {
                Ok(CommandStatus::Partial)
            } else {
                Ok(CommandStatus::Complete)
            }
        }
        Command::Worker {
            source,
            expected_identity,
            skipped_units,
            recovery,
            metadata_only,
            writer_order,
        } => {
            let expected_identity = serde_json::from_str(expected_identity)?;
            let skipped_units = serde_json::from_str(skipped_units)?;
            pstforge_core::run_recovery_worker(
                source,
                &expected_identity,
                &skipped_units,
                (*recovery).into(),
                *metadata_only,
                *writer_order,
                output,
            )?;
            Ok(CommandStatus::Complete)
        }
        Command::Validator {
            expected_parent,
            tool,
            arguments,
        } => {
            let parent_died = Arc::new(AtomicBool::new(false));
            signal_hook::flag::register(
                signal_hook::consts::signal::SIGTERM,
                Arc::clone(&parent_died),
            )
            .map_err(CliError::Validator)?;
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))
                .map_err(|source| CliError::Validator(source.into()))?;
            let actual_parent = rustix::process::Pid::as_raw(rustix::process::getppid());
            if actual_parent != *expected_parent {
                return Err(CliError::Validator(std::io::Error::other(
                    "validator supervisor exited during startup",
                )));
            }
            let mut command = if std::env::var_os("PSTFORGE_TEST_STALL_VALIDATOR").is_some() {
                let mut command = std::process::Command::new("sh");
                command.args(["-c", "(sleep 30) >&1 2>&2 & wait"]);
                command
            } else {
                let program = match tool {
                    ValidatorToolArg::Pffinfo => "pffinfo",
                    ValidatorToolArg::Readpst => "readpst",
                };
                let mut command = std::process::Command::new(program);
                command.args(arguments);
                command
            };
            let mut child = command.spawn().map_err(CliError::Validator)?;
            loop {
                if parent_died.load(Ordering::Relaxed) {
                    let _ = rustix::process::kill_process_group(
                        rustix::process::getpid(),
                        rustix::process::Signal::KILL,
                    );
                    return Err(CliError::Validator(std::io::Error::other(
                        "validator supervisor exited",
                    )));
                }
                if let Some(status) = child.try_wait().map_err(CliError::Validator)? {
                    return if status.success() {
                        Ok(CommandStatus::Complete)
                    } else {
                        Err(CliError::Validator(std::io::Error::other(format!(
                            "validator exited with {status}"
                        ))))
                    };
                }
                std::thread::sleep(Duration::from_millis(10));
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
    writeln!(output, "Recipients: {}", report.inventory.recipients)?;
    writeln!(output, "Attachments: {}", report.inventory.attachments)?;
    writeln!(
        output,
        "Embedded messages: {}",
        report.inventory.embedded_messages
    )?;
    writeln!(
        output,
        "Unsupported messages: {}",
        report.inventory.unsupported_messages
    )?;
    writeln!(
        output,
        "Raw properties: {}",
        report.inventory.raw_properties
    )?;
    writeln!(
        output,
        "Property bytes: {}",
        report.inventory.property_bytes
    )?;
    writeln!(output, "Body bytes: {}", report.inventory.body_bytes)?;
    writeln!(
        output,
        "Attachment bytes: {}",
        report.inventory.attachment_bytes
    )?;
    writeln!(
        output,
        "Peak stream chunk: {} bytes",
        report.inventory.peak_stream_chunk_bytes
    )?;
    write_optional_count(output, "Recovered items", report.inventory.recovered_items)?;
    write_optional_count(output, "Orphan items", report.inventory.orphan_items)?;
    writeln!(
        output,
        "Traversal issues: {}",
        report.inventory.issues.len()
    )?;
    writeln!(
        output,
        "Additional issues omitted: {}",
        report.inventory.issues_dropped
    )?;
    writeln!(
        output,
        "Source unchanged: {}",
        yes_no(report.source_unchanged)
    )?;
    Ok(())
}

fn write_recovery(output: &mut dyn Write, report: &RecoveryReport) -> Result<(), std::io::Error> {
    writeln!(output, "PSTForge recovery")?;
    writeln!(output, "Source: {}", report.source.canonical_path)?;
    writeln!(output, "SHA-256: {}", report.source.sha256)?;
    writeln!(output, "Job: {}", report.job_directory)?;
    writeln!(output, "Mode: {}", report.mode)?;
    writeln!(output, "Normal items: {}", report.normal_items)?;
    writeln!(output, "Recovered items: {}", report.recovered_items)?;
    writeln!(output, "Orphan items: {}", report.orphan_items)?;
    writeln!(output, "Fragment candidates: {}", report.fragment_items)?;
    writeln!(
        output,
        "Committed candidates: {}",
        report.committed_candidates
    )?;
    writeln!(
        output,
        "Complete candidates: {}",
        report.complete_candidates
    )?;
    writeln!(output, "Partial candidates: {}", report.partial_candidates)?;
    writeln!(
        output,
        "Unsupported candidates: {}",
        report.unsupported_candidates
    )?;
    writeln!(output, "Spool blobs: {}", report.blob_count)?;
    writeln!(output, "Spool bytes: {}", report.blob_bytes)?;
    writeln!(output, "Recovery issues: {}", report.issues)?;
    writeln!(output, "Worker attempts: {}", report.worker_attempts)?;
    writeln!(output, "Worker failures: {}", report.worker_failures)?;
    writeln!(output, "Isolated units: {}", report.isolated_units)?;
    writeln!(
        output,
        "Peak worker RSS: {} bytes",
        report.peak_worker_rss_bytes
    )?;
    writeln!(output, "Interrupted: {}", yes_no(report.interrupted))?;
    writeln!(
        output,
        "Additional issues omitted: {}",
        report.issues_dropped
    )?;
    writeln!(
        output,
        "Source unchanged: {}",
        yes_no(report.source_unchanged)
    )?;
    Ok(())
}

fn write_split(output: &mut dyn Write, report: &SplitReport) -> Result<(), std::io::Error> {
    writeln!(output, "PSTForge split")?;
    writeln!(output, "Source: {}", report.recovery.source.canonical_path)?;
    writeln!(output, "SHA-256: {}", report.recovery.source.sha256)?;
    writeln!(output, "Job: {}", report.recovery.job_directory)?;
    writeln!(
        output,
        "Maximum part size: {} bytes",
        report.maximum_pst_bytes
    )?;
    writeln!(output, "Execution mode: {}", report.execution_mode)?;
    writeln!(output, "Resumed: {}", yes_no(report.resumed))?;
    writeln!(output, "Keep private work: {}", yes_no(report.keep_work))?;
    writeln!(
        output,
        "Disk preflight: {} bytes available, {} bytes remaining required, {} bytes in existing job",
        report.disk_preflight.available_bytes,
        report.disk_preflight.required_bytes,
        report.disk_preflight.existing_job_bytes
    )?;
    writeln!(output, "Elapsed: {} ms", report.metrics.elapsed_millis)?;
    writeln!(
        output,
        "Average source throughput: {} bytes/s",
        report.metrics.average_source_bytes_per_second
    )?;
    writeln!(output, "Output bytes: {}", report.metrics.output_bytes)?;
    writeln!(
        output,
        "Peak process RSS: {} bytes",
        report.metrics.peak_process_rss_bytes
    )?;
    writeln!(output, "Written candidates: {}", report.written_candidates)?;
    if let Some(category) = report.terminal_failure {
        writeln!(output, "Terminal failure: {category}")?;
    }
    writeln!(output, "Parts: {}", report.parts.len())?;
    for part in &report.parts {
        writeln!(
            output,
            "  {}: {} bytes, SHA-256 {}, messages {}, folders {}, oversize {}, partial {}",
            part.filename,
            part.byte_len,
            part.sha256,
            part.message_count,
            part.folder_count,
            yes_no(part.oversize),
            yes_no(part.partial)
        )?;
    }
    writeln!(
        output,
        "Source unchanged: {}",
        yes_no(report.recovery.source_unchanged)
    )?;
    Ok(())
}

fn parse_byte_size(value: &str) -> Result<u64, String> {
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split);
    if number.is_empty() {
        return Err("size must start with a decimal integer".to_owned());
    }
    let number = number
        .parse::<u64>()
        .map_err(|_| "size integer is out of range".to_owned())?;
    let multiplier = match suffix.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        "tib" => 1_u64 << 40,
        _ => return Err("size suffix must be B, KB, MB, GB, TB, KiB, MiB, GiB, or TiB".to_owned()),
    };
    let bytes = number
        .checked_mul(multiplier)
        .ok_or_else(|| "size is out of range".to_owned())?;
    if bytes == 0 {
        return Err("size must be greater than zero".to_owned());
    }
    Ok(bytes)
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
    use std::path::PathBuf;

    use clap::Parser;
    use pstforge_core::{RecoveryError, SourceError};
    use pstforge_job::JobError;

    use super::{Cli, CliError, Command, RecoveryModeArg, VerifyMode, parse_byte_size};

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

    #[test]
    fn parses_balanced_recover_job() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from([
            "pstforge", "recover", "mail.pst", "--output", "job", "--json",
        ])?;
        assert!(matches!(
            cli.command,
            Command::Recover {
                recovery: RecoveryModeArg::Balanced,
                json: true,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn parses_explicit_aggressive_recovery() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from([
            "pstforge",
            "recover",
            "mail.pst",
            "--output",
            "job",
            "--recovery",
            "aggressive",
        ])?;
        assert!(matches!(
            cli.command,
            Command::Recover {
                recovery: RecoveryModeArg::Aggressive,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn split_defaults_to_four_gib_and_accepts_si_sizes() -> Result<(), clap::Error> {
        let default = Cli::try_parse_from(["pstforge", "split", "mail.pst", "--output", "job"])?;
        assert!(matches!(
            default.command,
            Command::Split {
                max_pst_size: 4_294_967_296,
                recovery: RecoveryModeArg::Balanced,
                restartable: false,
                ..
            }
        ));
        assert_eq!(parse_byte_size("4GB"), Ok(4_000_000_000));
        assert_eq!(parse_byte_size("4GiB"), Ok(4_294_967_296));
        assert!(parse_byte_size("0").is_err());
        assert!(parse_byte_size("1.5GiB").is_err());
        let resume = Cli::try_parse_from([
            "pstforge",
            "split",
            "mail.pst",
            "--output",
            "job",
            "--restartable",
            "--resume",
            "--keep-work",
        ])?;
        assert!(matches!(
            resume.command,
            Command::Split {
                restartable: true,
                resume: true,
                keep_work: true,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn recovery_exit_codes_follow_the_product_contract() {
        let partial_output = CliError::Recovery(RecoveryError::Job(JobError::ExistingJob(
            PathBuf::from("job"),
        )));
        assert_eq!(partial_output.exit_code(), 4);
        let unsafe_output = CliError::Recovery(RecoveryError::Source(SourceError::UnsafeOutput(
            PathBuf::from("job"),
        )));
        assert_eq!(unsafe_output.exit_code(), 4);
        assert_eq!(
            CliError::Recovery(RecoveryError::InconsistentCounters).exit_code(),
            6
        );
        assert_eq!(
            CliError::Recovery(RecoveryError::Interrupted).exit_code(),
            130
        );
    }

    #[test]
    fn worker_command_is_hidden_but_parseable() -> Result<(), clap::Error> {
        let help = Cli::try_parse_from(["pstforge", "--help"])
            .expect_err("help exits through clap")
            .to_string();
        assert!(!help.contains("__worker"));
        let cli = Cli::try_parse_from([
            "pstforge",
            "__worker",
            "mail.pst",
            r#"{"canonical_path":"/mail.pst","device":1,"inode":2,"size_bytes":3,"modified_at":"now","sha256":"abc"}"#,
            "[]",
            "balanced",
        ])?;
        assert!(matches!(cli.command, Command::Worker { .. }));
        Ok(())
    }
}
