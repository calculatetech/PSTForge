#![deny(unsafe_code)]

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use pstforge_core::{
    EncryptionMode, FileFormat, InfoReport, InspectionError, JobReport, RecoveryError,
    RecoveryMode, RecoveryReport, ReportError, SourceError, SourceFile, SplitError,
    SplitFailureKind, SplitReport, VerifyReport, WorkerProtocolError,
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
    /// Recreate a validated summary from an existing PSTForge job.
    Report {
        job_directory: PathBuf,
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
    #[command(name = "__verify-worker", hide = true)]
    VerifyWorker {
        source: PathBuf,
        expected_identity: String,
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
    Recovery,
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
    Report(#[from] ReportError),
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
    #[error("cannot start or supervise recovery verification: {0}")]
    VerifyWorkerIo(std::io::Error),
    #[error("recovery verification worker failed: {0}")]
    VerifyWorkerExit(String),
    #[error("recovery verification worker protocol failed: {0}")]
    VerifyWorkerProtocol(String),
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
            | Self::Report(ReportError::Job(_))
            | Self::Report(ReportError::InvalidSnapshot(_))
            | Self::Recovery(RecoveryError::WorkerProtocol(WorkerProtocolError::Sink(_)))
            | Self::Recovery(RecoveryError::Source(SourceError::UnsafeOutput(_)))
            | Self::Output(_) => 4,
            Self::Recovery(RecoveryError::Source(_))
            | Self::VerifyWorkerExit(_)
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
            | Self::Validator(_)
            | Self::VerifyWorkerIo(_)
            | Self::VerifyWorkerProtocol(_) => 6,
        }
    }

    pub fn installation_hint(&self) -> Option<&'static str> {
        match self {
            Self::Split(error) => error.installation_hint(),
            _ => None,
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
        Command::Verify { source, mode, json } => {
            let report = match mode {
                VerifyMode::Full => pstforge_core::verify(source)?,
                VerifyMode::Recovery => {
                    let executable = std::env::current_exe().map_err(CliError::Executable)?;
                    supervised_recovery_verify(source, &executable)?
                }
            };
            if *json {
                write_json(output, &report)?;
            } else {
                write_verify(output, &report)?;
            }
            if report.inventory.unsupported_messages == 0
                && report.inventory.issues.is_empty()
                && report.inventory.issues_dropped == 0
            {
                Ok(CommandStatus::Complete)
            } else {
                Ok(CommandStatus::Partial)
            }
        }
        Command::Report {
            job_directory,
            json,
        } => {
            let report = pstforge_core::report(job_directory)?;
            if *json {
                write_json(output, &report)?;
            } else {
                write_job_report(output, &report)?;
            }
            if report.split.recovery.interrupted {
                Ok(CommandStatus::Interrupted)
            } else if report.split.partial {
                Ok(CommandStatus::Partial)
            } else {
                Ok(CommandStatus::Complete)
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
        Command::VerifyWorker {
            source,
            expected_identity,
        } => {
            arm_verify_worker_parent_death_signal()?;
            if std::env::var_os("PSTFORGE_TEST_VERIFY_WORKER_ABORT").is_some() {
                std::process::abort();
            }
            if std::env::var_os("PSTFORGE_TEST_VERIFY_WORKER_STALL").is_some() {
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            let expected: pstforge_core::SourceIdentity = serde_json::from_str(expected_identity)?;
            let source =
                SourceFile::open(source).map_err(pstforge_core::InspectionError::Source)?;
            if source.identity() != &expected {
                return Err(pstforge_core::InspectionError::Source(SourceError::Changed(
                    PathBuf::from(&expected.canonical_path),
                ))
                .into());
            }
            let report = pstforge_core::verify_recovery_source(&source)?;
            write_json(output, &report)?;
            if report.inventory.unsupported_messages == 0
                && report.inventory.issues.is_empty()
                && report.inventory.issues_dropped == 0
            {
                Ok(CommandStatus::Complete)
            } else {
                Ok(CommandStatus::Partial)
            }
        }
    }
}

fn supervised_recovery_verify(
    source_path: &std::path::Path,
    executable: &std::path::Path,
) -> Result<VerifyReport, CliError> {
    let source =
        SourceFile::open_protected(source_path).map_err(pstforge_core::InspectionError::Source)?;
    let result = run_recovery_verify_worker(source_path, executable, source.identity());
    let unchanged = source
        .verify_unchanged()
        .map_err(pstforge_core::InspectionError::Source);
    match (result, unchanged) {
        (_, Err(error)) => Err(error.into()),
        (Ok(report), Ok(())) => Ok(report),
        (Err(error), Ok(())) => Err(error),
    }
}

fn run_recovery_verify_worker(
    source_path: &std::path::Path,
    executable: &std::path::Path,
    source_identity: &pstforge_core::SourceIdentity,
) -> Result<VerifyReport, CliError> {
    const MAX_WORKER_REPORT_BYTES: u64 = 16 * 1024 * 1024;

    let expected_identity = serde_json::to_string(source_identity)?;
    let mut child = ProcessCommand::new(executable)
        .arg("__verify-worker")
        .arg(source_path)
        .arg(expected_identity)
        .arg("--quiet")
        .arg("--color")
        .arg("never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env(
            "PSTFORGE_INTERNAL_SUPERVISOR_PID",
            std::process::id().to_string(),
        )
        .spawn()
        .map_err(CliError::VerifyWorkerIo)?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        CliError::VerifyWorkerProtocol("worker stdout was unavailable".to_owned())
    })?;
    let mut bytes = Vec::new();
    if let Err(error) = stdout
        .by_ref()
        .take(MAX_WORKER_REPORT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(CliError::VerifyWorkerIo(error));
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_WORKER_REPORT_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        return Err(CliError::VerifyWorkerProtocol(
            "worker report exceeded the bounded protocol limit".to_owned(),
        ));
    }
    let status = child.wait().map_err(CliError::VerifyWorkerIo)?;
    if !status.success() && status.code() != Some(1) {
        return Err(CliError::VerifyWorkerExit(status.to_string()));
    }
    let report: VerifyReport = serde_json::from_slice(&bytes).map_err(|error| {
        CliError::VerifyWorkerProtocol(format!("worker returned invalid JSON: {error}"))
    })?;
    if report.mode != "recovery"
        || report.command != "verify"
        || report.source != *source_identity
        || !report.source_unchanged
    {
        return Err(CliError::VerifyWorkerProtocol(
            "worker report identity or scope did not match the request".to_owned(),
        ));
    }
    Ok(report)
}

fn arm_verify_worker_parent_death_signal() -> Result<(), CliError> {
    let expected_parent = std::env::var("PSTFORGE_INTERNAL_SUPERVISOR_PID")
        .map_err(|_| {
            CliError::VerifyWorkerProtocol("worker supervisor identity is absent".to_owned())
        })?
        .parse::<i32>()
        .map_err(|_| {
            CliError::VerifyWorkerProtocol("worker supervisor identity is invalid".to_owned())
        })?;
    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))
        .map_err(|source| CliError::VerifyWorkerIo(source.into()))?;
    let actual_parent = rustix::process::Pid::as_raw(rustix::process::getppid());
    if actual_parent != expected_parent {
        return Err(CliError::VerifyWorkerProtocol(
            "worker supervisor exited during startup".to_owned(),
        ));
    }
    Ok(())
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
    write_source_hash(output, report.source.sha256.as_deref())?;
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
    write_source_hash(output, report.source.sha256.as_deref())?;
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
    write_split_details(output, report)
}

fn write_job_report(output: &mut dyn Write, report: &JobReport) -> Result<(), std::io::Error> {
    writeln!(output, "PSTForge job report")?;
    write_split_details(output, &report.split)
}

fn write_split_details(output: &mut dyn Write, report: &SplitReport) -> Result<(), std::io::Error> {
    writeln!(output, "Source: {}", report.recovery.source.canonical_path)?;
    write_source_hash(output, report.recovery.source.sha256.as_deref())?;
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
        "Payload pack: {} bytes written, {} bytes peak",
        report.metrics.payload_pack_bytes_written, report.metrics.peak_payload_pack_bytes
    )?;
    writeln!(
        output,
        "Active PST bytes written: {}",
        report.metrics.active_pst_bytes_written
    )?;
    writeln!(
        output,
        "Finalized output bytes: {}",
        report.metrics.finalized_output_bytes
    )?;
    writeln!(
        output,
        "Validator input bytes: {}",
        report.metrics.validator_input_bytes
    )?;
    writeln!(
        output,
        "Peak payload plus active PST bytes: {}",
        report.metrics.peak_payload_and_active_pst_bytes
    )?;
    write_optional_metric(
        output,
        "Supervisor filesystem read bytes",
        report.metrics.supervisor_filesystem_read_bytes,
    )?;
    write_optional_metric(
        output,
        "Supervisor filesystem write bytes",
        report.metrics.supervisor_filesystem_write_bytes,
    )?;
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
            part.sha256.as_deref().unwrap_or("not calculated"),
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

fn write_optional_metric(
    output: &mut dyn Write,
    name: &str,
    value: Option<u64>,
) -> Result<(), std::io::Error> {
    match value {
        Some(value) => writeln!(output, "{name}: {value}"),
        None => writeln!(output, "{name}: unavailable"),
    }
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
    write_source_hash(output, report.source.sha256.as_deref())?;
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

fn write_source_hash(output: &mut dyn Write, sha256: Option<&str>) -> Result<(), std::io::Error> {
    match sha256 {
        Some(value) => writeln!(output, "SHA-256: {value}"),
        None => writeln!(output, "SHA-256: not calculated (direct mode)"),
    }
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
    fn verify_accepts_recovery_mode() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["pstforge", "verify", "mail.pst", "--mode", "recovery"])?;
        assert!(matches!(
            cli.command,
            Command::Verify {
                mode: VerifyMode::Recovery,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn parses_read_only_job_report() -> Result<(), clap::Error> {
        let cli = Cli::try_parse_from(["pstforge", "report", "recovery-job", "--json"])?;
        assert!(matches!(
            cli.command,
            Command::Report {
                job_directory,
                json: true,
            } if job_directory.as_path() == std::path::Path::new("recovery-job")
        ));
        Ok(())
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
