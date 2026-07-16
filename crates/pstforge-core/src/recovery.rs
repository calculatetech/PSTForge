use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pstforge_job::{DurableCatalogSink, JobError, JobSourceIdentity, JobSummary};
use serde::Serialize;
use thiserror::Error;

use crate::worker::WorkerCatalog;
use crate::{
    SourceError, SourceFile, SourceIdentity, WorkerProtocolError,
    receive_worker_catalog_body_with_progress, receive_worker_hello, validate_output_relationship,
};
use libpff_sys::{RecoveryMode, RecoveryUnit};

pub const RECOVERY_SCHEMA_VERSION: &str = "0.3.1";
const MAX_WORKER_RETRIES: u32 = 3;

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Job(#[from] JobError),
    #[error(transparent)]
    WorkerProtocol(#[from] WorkerProtocolError),
    #[error("cannot locate the pstforge worker executable: {0}")]
    WorkerExecutable(std::io::Error),
    #[error("cannot start the recovery worker: {0}")]
    WorkerSpawn(std::io::Error),
    #[error("recovery worker stdout was not available")]
    MissingWorkerOutput,
    #[error("recovery worker exited unsuccessfully ({status})")]
    WorkerExit { status: ExitStatus },
    #[error("recovery worker stopped making progress")]
    WorkerStalled,
    #[error("recovery was interrupted")]
    Interrupted,
    #[error("recovery catalog counters are inconsistent")]
    InconsistentCounters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryReport {
    pub schema_version: String,
    pub command: String,
    pub mode: String,
    pub source: SourceIdentity,
    pub job_directory: String,
    pub normal_items: u64,
    pub recovered_items: u64,
    pub orphan_items: u64,
    pub fragment_items: u64,
    pub committed_candidates: u64,
    pub complete_candidates: u64,
    pub partial_candidates: u64,
    pub unsupported_candidates: u64,
    pub blob_count: u64,
    pub blob_bytes: u64,
    pub issues: u64,
    pub issues_dropped: u64,
    pub worker_attempts: u32,
    pub worker_failures: u32,
    pub isolated_units: u64,
    pub interrupted: bool,
    pub source_unchanged: bool,
}

pub fn recover(
    source_path: &Path,
    job_directory: &Path,
    worker_executable: &Path,
    mode: RecoveryMode,
) -> Result<RecoveryReport, RecoveryError> {
    validate_output_relationship(source_path, job_directory)?;
    let source = SourceFile::open(source_path)?;
    let interrupt = InterruptHandler::install()?;
    let worker_controls = WorkerControls {
        mode,
        interrupted: interrupt.flag(),
    };
    let expected_identity =
        serde_json::to_string(source.identity()).map_err(WorkerProtocolError::Json)?;
    let mut worker_attempts = 0_u32;
    let mut worker_failures = 0_u32;
    let mut skipped_units = HashSet::new();
    let mut repeat_fault_unit = None;
    let mut worker = loop {
        worker_attempts += 1;
        match spawn_worker(
            worker_executable,
            source.identity(),
            &expected_identity,
            &skipped_units,
            repeat_fault_unit,
            worker_attempts,
            &worker_controls,
        ) {
            Ok(worker) => break worker,
            Err(error) if retryable_worker_failure(&error) => {
                worker_failures += 1;
                if worker_failures > MAX_WORKER_RETRIES {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    };
    let mut sink = match DurableCatalogSink::create(job_directory) {
        Ok(sink) => sink,
        Err(error) => {
            worker.stop();
            return Err(error.into());
        }
    };
    if let Err(error) = sink.bind_source(&JobSourceIdentity {
        canonical_path: source.identity().canonical_path.clone(),
        device: source.identity().device,
        inode: source.identity().inode,
        size_bytes: source.identity().size_bytes,
        modified_at: source.identity().modified_at.clone(),
        sha256: source.identity().sha256.clone(),
    }) {
        worker.stop();
        return Err(error.into());
    }
    sink.bind_recovery_mode(recovery_mode_name(mode))?;
    sink.record_worker_event("started", worker_attempts, "parser")?;
    let mut retries_exhausted = false;
    let mut interrupted = false;
    let mut global_failures = worker_failures;
    let mut unit_failures = HashMap::<RecoveryUnit, u32>::new();
    let mut catalog = 'attempts: loop {
        let (mut replay_candidates, isolated_candidates): (Vec<_>, Vec<_>) = sink
            .replay_candidates()?
            .into_iter()
            .partition(|candidate| {
                candidate
                    .unit
                    .is_none_or(|unit| !skipped_units.contains(&unit))
            });
        normalize_replay_occurrences(&mut replay_candidates);
        match finish_worker_attempt(worker, &mut sink, &replay_candidates) {
            Ok(mut catalog) => {
                account_isolated_candidates(&mut catalog, &isolated_candidates)?;
                break catalog;
            }
            Err(failure) if matches!(failure.error, RecoveryError::Interrupted) => {
                sink.abort_worker_attempt()?;
                sink.record_interrupted()?;
                let summary = sink.summary()?;
                interrupted = true;
                break WorkerCatalog {
                    messages: summary.committed_candidates,
                    recovered_messages: summary.recovered_candidates,
                    orphan_messages: summary.orphan_candidates,
                    fragment_messages: summary.fragment_candidates,
                    unsupported_messages: summary.unsupported_candidates,
                    issues: 1,
                    ..WorkerCatalog::default()
                };
            }
            Err(failure) if retryable_worker_failure(&failure.error) => {
                worker_failures += 1;
                drop(sink);
                sink = DurableCatalogSink::open(job_directory)?;
                let failed_unit = failure.unit.as_deref().copied();
                if (std::env::var_os("PSTFORGE_TEST_ABORT_ON_UNIT_ORDINAL").is_some()
                    || std::env::var_os("PSTFORGE_TEST_ABORT_INSIDE_UNIT_AFTER_CANDIDATES")
                        .is_some()
                    || std::env::var_os("PSTFORGE_TEST_SEGV_ON_UNIT_ORDINAL").is_some())
                    && repeat_fault_unit.is_none()
                {
                    repeat_fault_unit = failed_unit;
                }
                sink.record_worker_event(
                    "failure",
                    worker_attempts,
                    worker_failure_category(&failure.error),
                )?;
                let isolated = failed_unit.and_then(|unit| {
                    let failures = unit_failures.entry(unit).or_default();
                    *failures = failures.saturating_add(1);
                    (*failures > MAX_WORKER_RETRIES).then_some(unit)
                });
                if let Some(unit) = isolated {
                    skipped_units.insert(unit);
                    sink.record_isolated_unit(unit, MAX_WORKER_RETRIES + 1)?;
                } else if failed_unit.is_none() {
                    global_failures = global_failures.saturating_add(1);
                }
                if global_failures > MAX_WORKER_RETRIES {
                    let summary = sink.summary()?;
                    retries_exhausted = true;
                    break WorkerCatalog {
                        messages: summary.committed_candidates,
                        recovered_messages: summary.recovered_candidates,
                        orphan_messages: summary.orphan_candidates,
                        fragment_messages: summary.fragment_candidates,
                        unsupported_messages: summary.unsupported_candidates,
                        issues: 1,
                        ..WorkerCatalog::default()
                    };
                }
                loop {
                    worker_attempts += 1;
                    match spawn_worker(
                        worker_executable,
                        source.identity(),
                        &expected_identity,
                        &skipped_units,
                        repeat_fault_unit,
                        worker_attempts,
                        &worker_controls,
                    ) {
                        Ok(next_worker) => {
                            worker = next_worker;
                            sink.record_worker_event("started", worker_attempts, "parser")?;
                            break;
                        }
                        Err(RecoveryError::Interrupted) => {
                            sink.abort_worker_attempt()?;
                            sink.record_interrupted()?;
                            let summary = sink.summary()?;
                            interrupted = true;
                            break 'attempts WorkerCatalog {
                                messages: summary.committed_candidates,
                                recovered_messages: summary.recovered_candidates,
                                orphan_messages: summary.orphan_candidates,
                                fragment_messages: summary.fragment_candidates,
                                unsupported_messages: summary.unsupported_candidates,
                                issues: 1,
                                ..WorkerCatalog::default()
                            };
                        }
                        Err(spawn_error) if retryable_worker_failure(&spawn_error) => {
                            worker_failures += 1;
                            global_failures = global_failures.saturating_add(1);
                            sink.record_worker_event(
                                "failure",
                                worker_attempts,
                                worker_failure_category(&spawn_error),
                            )?;
                            if global_failures > MAX_WORKER_RETRIES {
                                let summary = sink.summary()?;
                                retries_exhausted = true;
                                break 'attempts WorkerCatalog {
                                    messages: summary.committed_candidates,
                                    recovered_messages: summary.recovered_candidates,
                                    orphan_messages: summary.orphan_candidates,
                                    fragment_messages: summary.fragment_candidates,
                                    unsupported_messages: summary.unsupported_candidates,
                                    issues: 1,
                                    ..WorkerCatalog::default()
                                };
                            }
                        }
                        Err(spawn_error) => return Err(spawn_error),
                    }
                }
            }
            Err(failure) => return Err(failure.error),
        }
    };
    if interrupt.is_set() && !interrupted {
        sink.abort_worker_attempt()?;
        sink.record_interrupted()?;
        interrupted = true;
        catalog.issues = catalog.issues.saturating_add(1);
    }
    sink.record_worker_supervision(worker_attempts, worker_failures, retries_exhausted)?;
    sink.checkpoint()?;
    let summary = sink.summary()?;
    source.verify_unchanged()?;
    if interrupt.is_set() && !interrupted {
        sink.record_interrupted()?;
        sink.checkpoint()?;
        interrupted = true;
        catalog.issues = catalog.issues.saturating_add(1);
    }
    validate_catalog(&catalog, &summary)?;
    let normal_items = catalog
        .messages
        .checked_sub(catalog.recovered_messages)
        .and_then(|value| value.checked_sub(catalog.orphan_messages))
        .and_then(|value| value.checked_sub(catalog.fragment_messages))
        .ok_or(RecoveryError::InconsistentCounters)?;
    let job_directory = job_directory
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(job_directory))
        .to_string_lossy()
        .into_owned();
    Ok(RecoveryReport {
        schema_version: RECOVERY_SCHEMA_VERSION.to_owned(),
        command: "recover".to_owned(),
        mode: recovery_mode_name(mode).to_owned(),
        source: source.identity().clone(),
        job_directory,
        normal_items,
        recovered_items: catalog.recovered_messages,
        orphan_items: catalog.orphan_messages,
        fragment_items: catalog.fragment_messages,
        committed_candidates: summary.committed_candidates,
        complete_candidates: summary.complete_candidates,
        partial_candidates: summary.partial_candidates,
        unsupported_candidates: summary.unsupported_candidates,
        blob_count: summary.blob_count,
        blob_bytes: summary.blob_bytes,
        issues: catalog.issues,
        issues_dropped: catalog.issues_dropped,
        worker_attempts,
        worker_failures,
        isolated_units: u64::try_from(skipped_units.len()).unwrap_or(u64::MAX),
        interrupted,
        source_unchanged: true,
    })
}

struct WorkerProcess {
    child: Child,
    output: ChildStdout,
    reaped: bool,
    watchdog: AttemptWatchdog,
}

impl WorkerProcess {
    fn stop(&mut self) {
        if self.reaped {
            return;
        }
        let _ = self.watchdog.stop();
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reaped = true;
    }
}

enum WatchdogSignal {
    Progress,
    Stop,
}

struct AttemptWatchdog {
    sender: SyncSender<WatchdogSignal>,
    thread: Option<JoinHandle<WatchdogOutcome>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogOutcome {
    Stopped,
    Stalled,
    Interrupted,
}

impl AttemptWatchdog {
    fn start(process_id: u32, interrupted: Arc<AtomicBool>) -> Result<Self, RecoveryError> {
        let pid = i32::try_from(process_id)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| {
                RecoveryError::WorkerSpawn(std::io::Error::other(
                    "worker process ID is out of range",
                ))
            })?;
        let timeout = worker_stall_timeout();
        let poll_interval = Duration::from_millis(100);
        let (sender, receiver) = sync_channel(1);
        let thread = thread::Builder::new()
            .name("pstforge-worker-watchdog".to_owned())
            .spawn(move || {
                let mut last_progress = Instant::now();
                loop {
                    match receiver.recv_timeout(poll_interval) {
                        Ok(WatchdogSignal::Progress) => last_progress = Instant::now(),
                        Ok(WatchdogSignal::Stop)
                        | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            return WatchdogOutcome::Stopped;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if interrupted.load(Ordering::Relaxed) {
                                let _ = rustix::process::kill_process(
                                    pid,
                                    rustix::process::Signal::KILL,
                                );
                                return WatchdogOutcome::Interrupted;
                            }
                            if last_progress.elapsed() >= timeout {
                                let _ = rustix::process::kill_process(
                                    pid,
                                    rustix::process::Signal::KILL,
                                );
                                return WatchdogOutcome::Stalled;
                            }
                        }
                    }
                }
            })
            .map_err(RecoveryError::WorkerSpawn)?;
        Ok(Self {
            sender,
            thread: Some(thread),
        })
    }

    fn progress(&self) {
        match self.sender.try_send(WatchdogSignal::Progress) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn stop(&mut self) -> WatchdogOutcome {
        let _ = self.sender.send(WatchdogSignal::Stop);
        self.thread
            .take()
            .map_or(WatchdogOutcome::Stopped, |thread| {
                thread.join().unwrap_or(WatchdogOutcome::Stalled)
            })
    }
}

fn worker_stall_timeout() -> Duration {
    std::env::var("PSTFORGE_TEST_STALL_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(15 * 60))
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_worker(
    worker_executable: &Path,
    source: &SourceIdentity,
    expected_identity: &str,
    skipped_units: &HashSet<RecoveryUnit>,
    repeat_fault_unit: Option<RecoveryUnit>,
    attempt: u32,
    controls: &WorkerControls,
) -> Result<WorkerProcess, RecoveryError> {
    let mut command = Command::new(worker_executable);
    command
        .arg("__worker")
        .arg(&source.canonical_path)
        .arg(expected_identity)
        .arg(serde_json::to_string(skipped_units).map_err(WorkerProtocolError::Json)?)
        .arg(recovery_mode_name(controls.mode))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let abort_after = std::env::var_os("PSTFORGE_TEST_ABORT_EVERY_ATTEMPT_AFTER_CANDIDATES")
        .or_else(|| {
            (attempt == 1)
                .then(|| std::env::var_os("PSTFORGE_TEST_ABORT_AFTER_CANDIDATES"))
                .flatten()
        });
    if let Some(candidate_count) = abort_after {
        command.env("PSTFORGE_INTERNAL_ABORT_AFTER_CANDIDATES", candidate_count);
    }
    let stall_after = std::env::var_os("PSTFORGE_TEST_STALL_EVERY_ATTEMPT_AFTER_CANDIDATES")
        .or_else(|| {
            (attempt == 1)
                .then(|| std::env::var_os("PSTFORGE_TEST_STALL_AFTER_CANDIDATES"))
                .flatten()
        });
    if let Some(candidate_count) = stall_after {
        command.env("PSTFORGE_INTERNAL_STALL_AFTER_CANDIDATES", candidate_count);
    }
    if attempt == 1
        && let Some(unit_ordinal) = std::env::var_os("PSTFORGE_TEST_ABORT_ON_UNIT_ORDINAL")
    {
        command.env("PSTFORGE_INTERNAL_ABORT_ON_UNIT", unit_ordinal);
    }
    if attempt == 1
        && let Some(unit_ordinal) = std::env::var_os("PSTFORGE_TEST_SEGV_ON_UNIT_ORDINAL")
    {
        command.env("PSTFORGE_INTERNAL_SEGV_ON_UNIT", unit_ordinal);
    }
    if attempt == 1
        && let Some(candidate_count) =
            std::env::var_os("PSTFORGE_TEST_ABORT_INSIDE_UNIT_AFTER_CANDIDATES")
    {
        command.env(
            "PSTFORGE_INTERNAL_ABORT_INSIDE_AFTER_CANDIDATES",
            candidate_count,
        );
    }
    if let Some(unit) = repeat_fault_unit {
        let variable =
            if std::env::var_os("PSTFORGE_TEST_ABORT_INSIDE_UNIT_AFTER_CANDIDATES").is_some() {
                "PSTFORGE_INTERNAL_ABORT_INSIDE_UNIT"
            } else if std::env::var_os("PSTFORGE_TEST_SEGV_ON_UNIT_ORDINAL").is_some() {
                "PSTFORGE_INTERNAL_SEGV_UNIT"
            } else {
                "PSTFORGE_INTERNAL_ABORT_UNIT"
            };
        command.env(
            variable,
            serde_json::to_string(&unit).map_err(WorkerProtocolError::Json)?,
        );
    }
    if attempt == 1
        && let Some(candidate_count) =
            std::env::var_os("PSTFORGE_TEST_PARSER_ERROR_AFTER_CANDIDATES")
    {
        command.env(
            "PSTFORGE_INTERNAL_PARSER_ERROR_AFTER_CANDIDATES",
            candidate_count,
        );
    }
    let mut child = command.spawn().map_err(RecoveryError::WorkerSpawn)?;
    let mut watchdog = match AttemptWatchdog::start(child.id(), Arc::clone(&controls.interrupted)) {
        Ok(watchdog) => watchdog,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let mut output = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        RecoveryError::MissingWorkerOutput
    })?;
    if let Err(error) = receive_worker_hello(&mut output) {
        let outcome = watchdog.stop();
        let _ = child.kill();
        let _ = child.wait();
        match outcome {
            WatchdogOutcome::Stalled => return Err(RecoveryError::WorkerStalled),
            WatchdogOutcome::Interrupted => return Err(RecoveryError::Interrupted),
            WatchdogOutcome::Stopped => {}
        }
        return Err(error.into());
    }
    watchdog.progress();
    Ok(WorkerProcess {
        child,
        output,
        reaped: false,
        watchdog,
    })
}

fn finish_worker_attempt(
    mut worker: WorkerProcess,
    sink: &mut DurableCatalogSink,
    replay_candidates: &[pstforge_job::ReplayCandidate],
) -> Result<WorkerCatalog, AttemptFailure> {
    let mut active_unit = None;
    let mut active_unit_replayed = false;
    let mut active_unit_committed = false;
    let catalog = match receive_worker_catalog_body_with_progress(
        &mut worker.output,
        sink,
        replay_candidates,
        &mut active_unit,
        &mut active_unit_replayed,
        &mut active_unit_committed,
        &mut || worker.watchdog.progress(),
    ) {
        Ok(catalog) => catalog,
        Err(error) => {
            let outcome = worker.watchdog.stop();
            worker.stop();
            if outcome == WatchdogOutcome::Stalled {
                return Err(AttemptFailure {
                    error: RecoveryError::WorkerStalled,
                    unit: active_unit.map(Box::new),
                });
            }
            if outcome == WatchdogOutcome::Interrupted {
                return Err(AttemptFailure {
                    error: RecoveryError::Interrupted,
                    unit: active_unit.map(Box::new),
                });
            }
            return Err(AttemptFailure {
                error: error.into(),
                unit: active_unit.map(Box::new),
            });
        }
    };
    match worker.watchdog.stop() {
        WatchdogOutcome::Stalled => {
            worker.stop();
            return Err(AttemptFailure {
                error: RecoveryError::WorkerStalled,
                unit: active_unit.map(Box::new),
            });
        }
        WatchdogOutcome::Interrupted => {
            worker.stop();
            return Err(AttemptFailure {
                error: RecoveryError::Interrupted,
                unit: active_unit.map(Box::new),
            });
        }
        WatchdogOutcome::Stopped => {}
    }
    let status = worker.child.wait().map_err(|error| AttemptFailure {
        error: RecoveryError::WorkerSpawn(error),
        unit: active_unit.map(Box::new),
    })?;
    worker.reaped = true;
    if !status.success() {
        return Err(AttemptFailure {
            error: RecoveryError::WorkerExit { status },
            unit: active_unit.map(Box::new),
        });
    }
    Ok(catalog)
}

struct AttemptFailure {
    error: RecoveryError,
    unit: Option<Box<RecoveryUnit>>,
}

struct WorkerControls {
    mode: RecoveryMode,
    interrupted: Arc<AtomicBool>,
}

fn retryable_worker_failure(error: &RecoveryError) -> bool {
    matches!(
        error,
        RecoveryError::WorkerSpawn(_)
            | RecoveryError::MissingWorkerOutput
            | RecoveryError::WorkerExit { .. }
            | RecoveryError::WorkerStalled
            | RecoveryError::WorkerProtocol(
                WorkerProtocolError::Io(_)
                    | WorkerProtocolError::Json(_)
                    | WorkerProtocolError::Invalid(_)
                    | WorkerProtocolError::ReportedParser(_)
            )
    )
}

struct InterruptHandler {
    interrupted: Arc<AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

impl InterruptHandler {
    fn install() -> Result<Self, RecoveryError> {
        let interrupted = Arc::new(AtomicBool::new(false));
        let mut registrations = Vec::new();
        for signal in [
            signal_hook::consts::signal::SIGINT,
            signal_hook::consts::signal::SIGTERM,
        ] {
            match signal_hook::flag::register(signal, Arc::clone(&interrupted)) {
                Ok(registration) => registrations.push(registration),
                Err(error) => {
                    for registration in registrations {
                        signal_hook::low_level::unregister(registration);
                    }
                    return Err(RecoveryError::WorkerSpawn(error));
                }
            }
        }
        Ok(Self {
            interrupted,
            registrations,
        })
    }

    fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.interrupted)
    }

    fn is_set(&self) -> bool {
        self.interrupted.load(Ordering::Relaxed)
    }
}

impl Drop for InterruptHandler {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

fn worker_failure_category(error: &RecoveryError) -> &'static str {
    match error {
        RecoveryError::WorkerSpawn(_) | RecoveryError::MissingWorkerOutput => "spawn",
        RecoveryError::WorkerExit { .. } => "exit",
        RecoveryError::WorkerStalled => "stall",
        RecoveryError::WorkerProtocol(WorkerProtocolError::ReportedParser(_)) => "parser",
        RecoveryError::WorkerProtocol(_) => "protocol",
        _ => "supervisor",
    }
}

fn validate_catalog(catalog: &WorkerCatalog, summary: &JobSummary) -> Result<(), RecoveryError> {
    if catalog.messages != summary.committed_candidates
        || catalog.recovered_messages != summary.recovered_candidates
        || catalog.orphan_messages != summary.orphan_candidates
        || catalog.fragment_messages != summary.fragment_candidates
        || catalog.unsupported_messages != summary.unsupported_candidates
    {
        return Err(RecoveryError::InconsistentCounters);
    }
    Ok(())
}

fn account_isolated_candidates(
    catalog: &mut WorkerCatalog,
    candidates: &[pstforge_job::ReplayCandidate],
) -> Result<(), RecoveryError> {
    for candidate in candidates {
        catalog.messages = catalog
            .messages
            .checked_add(1)
            .ok_or(RecoveryError::InconsistentCounters)?;
        match candidate.provenance {
            libpff_sys::CatalogProvenance::Normal => {}
            libpff_sys::CatalogProvenance::Recovered => {
                catalog.recovered_messages = catalog
                    .recovered_messages
                    .checked_add(1)
                    .ok_or(RecoveryError::InconsistentCounters)?;
            }
            libpff_sys::CatalogProvenance::Orphan => {
                catalog.orphan_messages = catalog
                    .orphan_messages
                    .checked_add(1)
                    .ok_or(RecoveryError::InconsistentCounters)?;
            }
            libpff_sys::CatalogProvenance::Fragment => {
                catalog.fragment_messages = catalog
                    .fragment_messages
                    .checked_add(1)
                    .ok_or(RecoveryError::InconsistentCounters)?;
            }
        }
        if candidate.metadata["supported"] == false {
            catalog.unsupported_messages = catalog
                .unsupported_messages
                .checked_add(1)
                .ok_or(RecoveryError::InconsistentCounters)?;
        }
    }
    Ok(())
}

fn normalize_replay_occurrences(candidates: &mut [pstforge_job::ReplayCandidate]) {
    let mut occurrences = HashMap::new();
    for candidate in candidates {
        let occurrence = occurrences
            .entry((candidate.provenance, candidate.id, candidate.recovery_index))
            .or_insert(0_u32);
        candidate.occurrence = *occurrence;
        *occurrence = occurrence.saturating_add(1);
    }
}

fn recovery_mode_name(mode: RecoveryMode) -> &'static str {
    match mode {
        RecoveryMode::Balanced => "balanced",
        RecoveryMode::Aggressive => "aggressive",
    }
}

#[cfg(test)]
mod tests {
    use pstforge_job::JobSummary;

    use super::{RecoveryError, validate_catalog, worker_failure_category};
    use crate::worker::WorkerCatalog;

    #[test]
    fn forged_completion_counts_are_rejected() {
        let matching_catalog = || WorkerCatalog {
            messages: 3,
            recovered_messages: 1,
            orphan_messages: 1,
            fragment_messages: 0,
            unsupported_messages: 1,
            ..WorkerCatalog::default()
        };
        let matching_summary = || JobSummary {
            committed_candidates: 3,
            recovered_candidates: 1,
            orphan_candidates: 1,
            fragment_candidates: 0,
            complete_candidates: 1,
            partial_candidates: 0,
            unsupported_candidates: 1,
            blob_count: 0,
            blob_bytes: 0,
        };

        let mut cases = Vec::new();
        let mut summary = matching_summary();
        summary.committed_candidates = 2;
        cases.push((matching_catalog(), summary));
        let mut summary = matching_summary();
        summary.recovered_candidates = 0;
        cases.push((matching_catalog(), summary));
        let mut summary = matching_summary();
        summary.orphan_candidates = 0;
        cases.push((matching_catalog(), summary));
        let mut summary = matching_summary();
        summary.fragment_candidates = 1;
        cases.push((matching_catalog(), summary));
        let mut summary = matching_summary();
        summary.unsupported_candidates = 0;
        cases.push((matching_catalog(), summary));

        for (catalog, summary) in cases {
            assert!(matches!(
                validate_catalog(&catalog, &summary),
                Err(RecoveryError::InconsistentCounters)
            ));
        }
    }

    #[test]
    fn stall_failures_have_a_stable_durable_category() {
        assert_eq!(
            worker_failure_category(&RecoveryError::WorkerStalled),
            "stall"
        );
    }
}
