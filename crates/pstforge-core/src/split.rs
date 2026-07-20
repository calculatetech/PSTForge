use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use libpff_sys::RecoveryMode;
use pstforge_job::{
    CandidateRejectionCategory, CandidateRejectionCounts, DurableCatalogSink, JobConfiguration,
    JobError, JobSourceIdentity, PartSidecar, PublishedPart, PublishedPartRecord,
    ReconstructedField, ReconstructionCounts,
};
use pstforge_pst::writer::{
    AttachmentContent, MailFolderLocation, MessageSpec, NamedPropertyCatalog, NamedPropertyName,
    TransactionAppend, TransactionalMailStoreWriter, WriterError, create_mail_store_interruptible,
    create_mail_store_supervised, validate_mail_store_input, validate_mail_store_layout,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::canonical::load_canonical_mail_tree_from_folders_interruptible;
use crate::recovery::{InterruptHandler, PreparedSplitRecovery, recover_for_split};
use crate::writer_input::{
    PartBuildOptions, PartWriterInput, build_part_writer_input_with_folders_interruptible,
    build_part_writer_layout,
};
use crate::{
    CanonicalError, CanonicalWriteError, PackCandidate, PackingError, PartSizeEstimator,
    RecoveryError, RecoveryReport, SourceError, SourceFile, load_canonical_folders_interruptible,
    load_canonical_mail_interruptible,
};

pub const SPLIT_SCHEMA_VERSION: &str = "0.4.4";
const TOOL_COMPATIBILITY_MAJOR: u64 = 0;
const PART_SIZE_POLICY: &str = "hard-maximum-v1";
const WRITER_FORMAT: &str = "unicode-pst-v23";
const ESTIMATED_STORE_BYTES: u64 = 1024 * 1024;
const ESTIMATED_MESSAGE_BYTES: u64 = 64 * 1024;
const ESTIMATED_FOLDER_BYTES: u64 = 16 * 1024;
const MAX_RECOVERY_LOG_DETAIL_LINES: usize = 10_000;
const MAX_BATCH_MESSAGES: usize = 2_048;

#[derive(Debug, Error)]
pub enum SplitError {
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
    #[error(transparent)]
    Job(#[from] JobError),
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    #[error(transparent)]
    Translation(#[from] CanonicalWriteError),
    #[error(transparent)]
    Packing(#[from] PackingError),
    #[error(transparent)]
    Writer(#[from] WriterError),
    #[error("maximum PST size must be greater than zero")]
    ZeroMaximumSize,
    #[error("packing assignment references an unknown canonical item")]
    UnknownAssignment,
    #[error("cannot access staged PST {path}: {source}")]
    StagedIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("part counter exceeds the supported range")]
    TooManyParts,
    #[error(
        "part {part_index} serialized to {byte_len} bytes, exceeding the configured maximum of {maximum_bytes} bytes"
    )]
    PartExceedsMaximum {
        part_index: u32,
        byte_len: u64,
        maximum_bytes: u64,
    },
    #[error(
        "insufficient output space: {available_bytes} bytes available, {required_bytes} bytes required"
    )]
    InsufficientDiskSpace {
        required_bytes: u64,
        available_bytes: u64,
    },
    #[error("cannot inspect available space for {path}: {source}")]
    DiskSpaceIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitFailureKind {
    Source,
    Output,
    Conformance,
    Interrupted,
    Internal,
}

impl SplitError {
    pub fn failure_kind(&self) -> SplitFailureKind {
        match self {
            Self::Recovery(RecoveryError::Source(SourceError::UnsafeOutput(_)))
            | Self::Recovery(RecoveryError::Job(_))
            | Self::Recovery(RecoveryError::WorkerProtocol(crate::WorkerProtocolError::Sink(_)))
            | Self::Job(_)
            | Self::InsufficientDiskSpace { .. }
            | Self::DiskSpaceIo { .. }
            | Self::StagedIo { .. }
            | Self::Writer(
                WriterError::OutputExists(_)
                | WriterError::PublishedDurability { .. }
                | WriterError::PublishedDestinationChanged(_)
                | WriterError::Io(_),
            ) => SplitFailureKind::Output,
            Self::Recovery(RecoveryError::Source(_)) => SplitFailureKind::Source,
            Self::Recovery(RecoveryError::Interrupted) | Self::Writer(WriterError::Interrupted) => {
                SplitFailureKind::Interrupted
            }
            Self::PartExceedsMaximum { .. }
            | Self::Writer(
                WriterError::IndependentValidation { .. }
                | WriterError::InputRejected(_)
                | WriterError::CompletedValidation(_)
                | WriterError::ValueTooLarge(_)
                | WriterError::InvalidStructure(_),
            ) => SplitFailureKind::Conformance,
            Self::Recovery(_)
            | Self::Canonical(_)
            | Self::Translation(_)
            | Self::Packing(_)
            | Self::Writer(
                WriterError::IndependentValidatorIo { .. } | WriterError::ExecutionTerminated,
            )
            | Self::ZeroMaximumSize
            | Self::UnknownAssignment
            | Self::TooManyParts => SplitFailureKind::Internal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiskPreflight {
    pub required_bytes: u64,
    pub available_bytes: u64,
    pub existing_job_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionMetrics {
    pub elapsed_millis: u64,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub average_source_bytes_per_second: u64,
    pub peak_process_rss_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartReport {
    pub index: u32,
    pub filename: String,
    pub byte_len: u64,
    pub sha256: String,
    pub folder_count: u64,
    pub message_count: u64,
    pub oversize: bool,
    pub partial: bool,
    pub omitted_folders: u64,
    pub omitted_properties: u64,
    pub omitted_attachments: u64,
    #[serde(skip_serializing)]
    pub reconstructions: ReconstructionCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SplitReport {
    pub schema_version: String,
    pub command: String,
    pub maximum_pst_bytes: u64,
    pub resumed: bool,
    pub keep_work: bool,
    pub disk_preflight: DiskPreflight,
    pub metrics: ExecutionMetrics,
    pub recovery: RecoveryReport,
    pub rejection_counts: CandidateRejectionCounts,
    pub parts: Vec<PartReport>,
    pub written_candidates: u64,
    pub partial: bool,
}

pub fn split(
    source_path: &Path,
    job_directory: &Path,
    worker_executable: &Path,
    mode: RecoveryMode,
    maximum_pst_bytes: u64,
    resume: bool,
    keep_work: bool,
) -> Result<SplitReport, SplitError> {
    let started = Instant::now();
    if maximum_pst_bytes == 0 {
        return Err(SplitError::ZeroMaximumSize);
    }
    let interrupt = InterruptHandler::install()?;
    let interrupt_flag = interrupt.flag();
    crate::validate_output_relationship(source_path, job_directory)
        .map_err(RecoveryError::Source)?;
    let source = SourceFile::open_interruptible(source_path, &interrupt_flag).map_err(|error| {
        if matches!(error, SourceError::Interrupted) {
            RecoveryError::Interrupted
        } else {
            RecoveryError::Source(error)
        }
    })?;
    let mut configuration = JobConfiguration {
        tool_compatibility_major: TOOL_COMPATIBILITY_MAJOR,
        split_schema_version: SPLIT_SCHEMA_VERSION.to_owned(),
        recovery_mode: recovery_mode_name(mode).to_owned(),
        maximum_pst_bytes,
        part_size_policy: PART_SIZE_POLICY.to_owned(),
        writer_format: WRITER_FORMAT.to_owned(),
    };
    let (existing_job_bytes, resumed_job) = if resume {
        let source_identity = JobSourceIdentity {
            canonical_path: source.identity().canonical_path.clone(),
            device: source.identity().device,
            inode: source.identity().inode,
            size_bytes: source.identity().size_bytes,
            modified_at: source.identity().modified_at.clone(),
            sha256: source.identity().sha256.clone(),
        };
        let open = DurableCatalogSink::open_resume_interruptible(
            job_directory,
            &source_identity,
            &configuration,
            &interrupt_flag,
        );
        let open = match open {
            Err(JobError::ResumeMismatch("split report schema version"))
                if SPLIT_SCHEMA_VERSION == "0.4.4" =>
            {
                let mut legacy_configuration = configuration.clone();
                legacy_configuration.split_schema_version = "0.4.2".to_owned();
                let legacy = DurableCatalogSink::open_resume_interruptible(
                    job_directory,
                    &source_identity,
                    &legacy_configuration,
                    &interrupt_flag,
                );
                if legacy.is_ok() {
                    tracing::info!(
                        stored_split_schema = "0.4.2",
                        producer_version = SPLIT_SCHEMA_VERSION,
                        "resuming compatible pre-performance durable recovery state"
                    );
                    configuration = legacy_configuration;
                }
                legacy
            }
            result => result,
        };
        match open {
            Ok(job) => {
                let allocated = job.allocated_bytes()?;
                (allocated, Some(job))
            }
            Err(JobError::Interrupted) => return Err(RecoveryError::Interrupted.into()),
            Err(error) => return Err(error.into()),
        }
    } else {
        (0, None)
    };
    let disk_preflight = disk_preflight(
        job_directory,
        source.identity().size_bytes,
        existing_job_bytes,
    )?;
    tracing::info!(
        required_bytes = disk_preflight.required_bytes,
        available_bytes = disk_preflight.available_bytes,
        existing_job_bytes = disk_preflight.existing_job_bytes,
        "output space preflight passed"
    );
    let (mut recovery, mut job) = recover_for_split(
        &source,
        job_directory,
        worker_executable,
        mode,
        std::sync::Arc::clone(&interrupt_flag),
        PreparedSplitRecovery {
            resume,
            configuration: &configuration,
            job: resumed_job,
        },
    )?;
    if recovery.interrupted {
        let (parts, written_candidates, unsupported_candidates) = durable_output_snapshot(&job)?;
        recovery.unsupported_candidates = unsupported_candidates;
        let metrics = execution_metrics(
            started,
            source.identity().size_bytes,
            &parts,
            recovery.peak_worker_rss_bytes,
        );
        let report = SplitReport {
            schema_version: SPLIT_SCHEMA_VERSION.to_owned(),
            command: "split".to_owned(),
            maximum_pst_bytes,
            resumed: resume,
            keep_work,
            disk_preflight,
            metrics,
            recovery,
            rejection_counts: job.candidate_rejection_counts()?,
            parts,
            written_candidates,
            partial: true,
        };
        job.publish_recovery_log(&render_recovery_log(&report))?;
        return Ok(report);
    }
    if source.identity() != &recovery.source {
        return Err(RecoveryError::Source(SourceError::Changed(source_path.to_path_buf())).into());
    }
    let (parts, written_candidates, output_partial, split_interrupted) =
        split_recovered_job_with_interrupt(
            &mut job,
            &recovery.source.sha256,
            mode,
            maximum_pst_bytes,
            &interrupt_flag,
            Some(worker_executable),
        )?;
    recovery.interrupted |= split_interrupted;
    if split_interrupted {
        let (parts, written_candidates, unsupported_candidates) = durable_output_snapshot(&job)?;
        recovery.unsupported_candidates = unsupported_candidates;
        let metrics = execution_metrics(
            started,
            source.identity().size_bytes,
            &parts,
            recovery.peak_worker_rss_bytes,
        );
        let report = SplitReport {
            schema_version: SPLIT_SCHEMA_VERSION.to_owned(),
            command: "split".to_owned(),
            maximum_pst_bytes,
            resumed: resume,
            keep_work,
            disk_preflight,
            metrics,
            recovery,
            rejection_counts: job.candidate_rejection_counts()?,
            parts,
            written_candidates,
            partial: true,
        };
        job.publish_recovery_log(&render_recovery_log(&report))?;
        return Ok(report);
    }
    recovery.unsupported_candidates = job.summary()?.unsupported_candidates;
    match source.verify_unchanged_interruptible(&interrupt_flag) {
        Ok(()) => {}
        Err(SourceError::Interrupted) => {
            recovery.interrupted = true;
            recovery.source_unchanged = false;
        }
        Err(error) => return Err(RecoveryError::Source(error).into()),
    }
    if interrupt_flag.load(Ordering::Relaxed) && !recovery.interrupted {
        recovery.interrupted = true;
    }
    if !recovery.interrupted {
        match job.finalize_private_work_interruptible(keep_work, &interrupt_flag) {
            Ok(()) => {}
            Err(JobError::Interrupted) => recovery.interrupted = true,
            Err(error) => return Err(error.into()),
        }
    }
    if interrupt_flag.load(Ordering::Relaxed) && !recovery.interrupted {
        recovery.interrupted = true;
    }
    let partial = output_partial
        || recovery.partial_candidates != 0
        || recovery.unsupported_candidates != 0
        || recovery.issues != 0
        || recovery.issues_dropped != 0
        || recovery.interrupted;
    let metrics = execution_metrics(
        started,
        source.identity().size_bytes,
        &parts,
        recovery.peak_worker_rss_bytes,
    );
    tracing::info!(
        parts = parts.len(),
        written_candidates,
        output_bytes = metrics.output_bytes,
        elapsed_millis = metrics.elapsed_millis,
        interrupted = recovery.interrupted,
        "split invocation complete"
    );
    let report = SplitReport {
        schema_version: SPLIT_SCHEMA_VERSION.to_owned(),
        command: "split".to_owned(),
        maximum_pst_bytes,
        resumed: resume,
        keep_work,
        disk_preflight,
        metrics,
        recovery,
        rejection_counts: job.candidate_rejection_counts()?,
        parts,
        written_candidates,
        partial,
    };
    job.publish_recovery_log(&render_recovery_log(&report))?;
    Ok(report)
}

fn durable_output_snapshot(
    job: &DurableCatalogSink,
) -> Result<(Vec<PartReport>, u64, u64), SplitError> {
    let records = job.published_parts()?;
    let parts = records
        .iter()
        .map(part_report)
        .collect::<Result<Vec<_>, _>>()?;
    let written = records
        .iter()
        .map(|record| record.item_count)
        .fold(0_u64, u64::saturating_add);
    let unsupported = job.summary()?.unsupported_candidates;
    Ok((parts, written, unsupported))
}

fn render_recovery_log(report: &SplitReport) -> String {
    let omitted_folders = report
        .parts
        .iter()
        .map(|part| part.omitted_folders)
        .fold(0_u64, u64::saturating_add);
    let omitted_properties = report
        .parts
        .iter()
        .map(|part| part.omitted_properties)
        .fold(0_u64, u64::saturating_add);
    let omitted_attachments = report
        .parts
        .iter()
        .map(|part| part.omitted_attachments)
        .fold(0_u64, u64::saturating_add);
    let unfinished_items = report
        .recovery
        .committed_candidates
        .saturating_sub(report.written_candidates)
        .saturating_sub(report.recovery.unsupported_candidates);
    let mut reconstructions = ReconstructionCounts::default();
    for part in &report.parts {
        reconstructions.merge(part.reconstructions.clone());
    }
    let clean_data = report.recovery.partial_candidates == 0
        && report.recovery.unsupported_candidates == 0
        && report.recovery.issues == 0
        && report.recovery.issues_dropped == 0
        && omitted_folders == 0
        && omitted_properties == 0
        && omitted_attachments == 0
        && unfinished_items == 0
        && report.recovery.source_unchanged
        && !report.recovery.interrupted;
    let result = if report.recovery.interrupted {
        "interrupted"
    } else if report.partial {
        "partial"
    } else {
        "complete"
    };
    let mut output = String::new();
    output.push_str("PSTForge recovery log\n");
    output.push_str(&format!("Version: {}\n", crate::VERSION));
    output.push_str(&format!("Result: {result}\n"));
    output.push_str(&format!(
        "Source SHA-256: {}\n",
        report.recovery.source.sha256
    ));
    output.push_str(&format!(
        "Source size: {} bytes\n",
        report.recovery.source.size_bytes
    ));
    output.push_str(&format!(
        "Source unchanged: {}\n",
        if report.recovery.source_unchanged {
            "yes"
        } else {
            "no"
        }
    ));
    output.push_str(&format!(
        "Maximum part size: {} bytes\n\n",
        report.maximum_pst_bytes
    ));
    output.push_str("Recovery summary\n");
    output.push_str(&format!(
        "Items found: {}\n",
        report.recovery.committed_candidates
    ));
    output.push_str(&format!("Items written: {}\n", report.written_candidates));
    output.push_str(&format!(
        "Items recovered from damaged structures: {}\n",
        report.recovery.recovered_items
    ));
    output.push_str(&format!(
        "Detached items recovered: {}\n",
        report.recovery.orphan_items
    ));
    output.push_str(&format!(
        "Fragmentary items found: {}\n",
        report.recovery.fragment_items
    ));
    output.push_str(&format!("Output files: {}\n\n", report.parts.len()));

    output.push_str("Data not copied\n");
    if clean_data {
        output.push_str("No readable data was skipped.\n");
    } else {
        output.push_str(&format!(
            "Items only partly readable: {}\n",
            report.recovery.partial_candidates
        ));
        output.push_str(&format!(
            "Items not copied because their type or contents could not yet be preserved safely: {}\n",
            report.recovery.unsupported_candidates
        ));
        for (category, count) in &report.rejection_counts {
            output.push_str(&format!(
                "  {}: {count}\n",
                rejection_category_label(*category)
            ));
        }
        output.push_str(&format!("Attachments not copied: {omitted_attachments}\n"));
        output.push_str(&format!("Folders not copied: {omitted_folders}\n"));
        output.push_str(&format!("Item details not copied: {omitted_properties}\n"));
        output.push_str(&format!("Items left unfinished: {unfinished_items}\n"));
        output.push_str(&format!(
            "Read problems encountered: {}\n",
            report.recovery.issues
        ));
        output.push_str(&format!(
            "Additional read problems not listed individually: {}\n",
            report.recovery.issues_dropped
        ));
    }
    output.push_str("\nMetadata recovery\n");
    if reconstructions.is_empty() {
        output.push_str("No source metadata required recovery handling.\n");
    } else {
        append_reconstruction_group(
            &mut output,
            "Derived from other readable source values",
            &reconstructions.derived,
        );
        append_reconstruction_group(
            &mut output,
            "Source metadata absent or unusable; defaults generated or fields left absent",
            &reconstructions.generated,
        );
    }
    output.push_str("\nOutput files\n");
    for part in report.parts.iter().take(MAX_RECOVERY_LOG_DETAIL_LINES) {
        output.push_str(&format!(
            "{}: {} bytes, SHA-256 {}, {} messages, {} folders\n",
            part.filename, part.byte_len, part.sha256, part.message_count, part.folder_count
        ));
    }
    let unlisted = report
        .parts
        .len()
        .saturating_sub(MAX_RECOVERY_LOG_DETAIL_LINES);
    if unlisted != 0 {
        output.push_str(&format!(
            "Additional output files omitted from detail: {unlisted}\n"
        ));
    }
    output
}

fn rejection_category_label(category: CandidateRejectionCategory) -> &'static str {
    match category {
        CandidateRejectionCategory::SourceItemUnsupported => {
            "Source reader reported the item type as unsupported"
        }
        CandidateRejectionCategory::MalformedCandidate => {
            "Item structure could not be translated safely"
        }
        CandidateRejectionCategory::MalformedProperty => "A required item property was malformed",
        CandidateRejectionCategory::WriterInputRejected => {
            "Translated item failed writer validation"
        }
        CandidateRejectionCategory::ItemGraphDependencyRejected => {
            "Item depended on another item that could not be written"
        }
        CandidateRejectionCategory::UnsupportedEmbeddedItem => {
            "Embedded item type or linkage could not be preserved"
        }
        CandidateRejectionCategory::StrandedEmbeddedItem => {
            "Embedded item was stranded beneath a finalized parent"
        }
    }
}

fn append_reconstruction_group(
    output: &mut String,
    heading: &str,
    counts: &BTreeMap<ReconstructedField, u64>,
) {
    if counts.is_empty() {
        return;
    }
    output.push_str(heading);
    output.push_str(":\n");
    for (field, count) in counts {
        output.push_str(&format!(
            "  {}: {count}\n",
            reconstructed_field_label(*field)
        ));
    }
}

fn reconstructed_field_label(field: ReconstructedField) -> &'static str {
    match field {
        ReconstructedField::FolderClass => "Folder class",
        ReconstructedField::MessageClass => "Message class",
        ReconstructedField::Subject => "Subject",
        ReconstructedField::SenderName => "Sender name",
        ReconstructedField::SenderAddress => "Sender address",
        ReconstructedField::MessageFlags => "Message flags",
        ReconstructedField::InternetCodepage => "Internet code page",
        ReconstructedField::SubmitTime => "Submit time",
        ReconstructedField::DeliveryTime => "Delivery time",
        ReconstructedField::CreationTime => "Creation time",
        ReconstructedField::ModificationTime => "Modification time",
        ReconstructedField::AssociatedDisplayName => "Associated-item display name",
        ReconstructedField::RecipientDisplayName => "Recipient display name",
        ReconstructedField::RecipientAddress => "Recipient address",
        ReconstructedField::AttachmentFilename => "Attachment filename",
        ReconstructedField::AttachmentMimeType => "Attachment MIME type",
        ReconstructedField::AttachmentRenderingPosition => "Attachment rendering position",
        ReconstructedField::AttachmentFlags => "Attachment flags",
        ReconstructedField::DocumentAttachment => "Required Document attachment",
    }
}

pub fn split_recovered_job(
    job_directory: &Path,
    source_sha256: &str,
    recovery_mode: RecoveryMode,
    maximum_pst_bytes: u64,
) -> Result<(Vec<PartReport>, u64, bool), SplitError> {
    let interrupted = AtomicBool::new(false);
    let mut job = DurableCatalogSink::open(job_directory)?;
    let (parts, written, partial, _) = split_recovered_job_with_interrupt(
        &mut job,
        source_sha256,
        recovery_mode,
        maximum_pst_bytes,
        &interrupted,
        None,
    )?;
    Ok((parts, written, partial))
}

#[derive(Clone)]
struct IncrementalCandidate {
    durable_key: String,
    folder_location: crate::CanonicalFolderLocation,
    folder_path: Vec<String>,
    folder_role: crate::CanonicalFolderRole,
    container_class: String,
    placement: crate::CanonicalMessagePlacement,
    key: crate::ItemKey,
}

#[derive(Clone, Default)]
struct IncrementalPartStats {
    item_keys: Vec<String>,
    unsupported_item_keys: Vec<String>,
    folder_keys: BTreeSet<(MailFolderLocation, Vec<String>)>,
    message_count: u64,
    partial: bool,
    omitted_folders: u64,
    omitted_properties: u64,
    omitted_attachments: u64,
    reconstructions: ReconstructionCounts,
}

fn projection_batch_byte_target(maximum_pst_bytes: u64, private_file_eof: u64) -> u64 {
    let finalization_reserve = (maximum_pst_bytes / 16).max(1);
    if private_file_eof >= maximum_pst_bytes.saturating_sub(finalization_reserve) {
        (maximum_pst_bytes / 256).max(1)
    } else {
        (maximum_pst_bytes / 16).max(1)
    }
}

fn split_recovered_job_with_interrupt(
    job: &mut DurableCatalogSink,
    source_sha256: &str,
    recovery_mode: RecoveryMode,
    maximum_pst_bytes: u64,
    interrupted: &AtomicBool,
    validator_supervisor: Option<&Path>,
) -> Result<(Vec<PartReport>, u64, bool, bool), SplitError> {
    if maximum_pst_bytes == 0 {
        return Err(SplitError::ZeroMaximumSize);
    }
    let existing = job.published_parts()?;
    let mut reports = existing
        .iter()
        .map(part_report)
        .collect::<Result<Vec<_>, _>>()?;
    let mut written_candidates = existing
        .iter()
        .map(|record| record.item_count)
        .fold(0_u64, u64::saturating_add);
    let mut any_partial = reports
        .iter()
        .any(|report| report.partial || report.oversize);
    let mut part_index = existing.last().map_or(Ok(1_u32), |record| {
        record
            .part
            .index
            .checked_add(1)
            .ok_or(SplitError::TooManyParts)
    })?;
    let source_folders = match load_canonical_folders_interruptible(job, interrupted) {
        Ok(folders) => folders,
        Err(CanonicalError::Job(JobError::Interrupted)) => {
            return finish_incremental_interruption(job, reports, written_candidates);
        }
        Err(error) => return Err(error.into()),
    };
    let spooled_folders = job.spooled_folders()?;
    let mode_name = match recovery_mode {
        RecoveryMode::Balanced => "balanced",
        RecoveryMode::Aggressive => "aggressive",
    };
    let metadata_started = Instant::now();
    let mut candidates = Vec::new();
    let header_count = match crate::canonical::for_each_canonical_mail_header_interruptible(
        job,
        &spooled_folders,
        interrupted,
        |mail| {
            candidates.push(IncrementalCandidate {
                durable_key: mail.durable_item_key,
                folder_location: mail.folder_location,
                folder_path: mail.folder_path,
                folder_role: mail.folder_role,
                container_class: crate::writer_input::default_container_class(
                    mail.message_class.as_deref().unwrap_or("IPM.Note"),
                )
                .to_owned(),
                placement: mail.placement,
                key: mail.key,
            });
            Ok(())
        },
    ) {
        Ok(count) => count,
        Err(CanonicalError::Job(JobError::Interrupted)) => {
            return finish_incremental_interruption(job, reports, written_candidates);
        }
        Err(error) => return Err(error.into()),
    };
    if header_count == 0 {
        return finish_incremental_completion(job, reports, written_candidates, any_partial);
    }
    let identities = match job.candidate_named_property_identities_interruptible(interrupted) {
        Ok(identities) => identities,
        Err(JobError::Interrupted) => {
            return finish_incremental_interruption(job, reports, written_candidates);
        }
        Err(error) => return Err(error.into()),
    };
    let mut named_properties = NamedPropertyCatalog::default();
    for identity in identities {
        let name = match identity.name {
            libpff_sys::NamedPropertyName::Numeric(value) => NamedPropertyName::Numeric(value),
            libpff_sys::NamedPropertyName::String(value) => NamedPropertyName::String(value),
        };
        named_properties.observe(crate::writer_input::named_property_set(identity.guid), name);
    }
    // These three identities are a bounded conservative superset. They avoid
    // decoding attachment properties during the metadata-only catalog pass.
    named_properties.observe_reference_attachment();
    candidates.sort_by(|left, right| {
        (
            left.folder_location,
            &left.folder_path,
            left.placement,
            left.key,
        )
            .cmp(&(
                right.folder_location,
                &right.folder_path,
                right.placement,
                right.key,
            ))
    });
    if candidates.is_empty() {
        return finish_incremental_completion(job, reports, written_candidates, true);
    }
    tracing::info!(
        candidate_count = candidates.len(),
        elapsed_ms = metadata_started.elapsed().as_millis(),
        "incremental metadata catalog completed"
    );
    if test_pause_at_prefilter(job, interrupted)? {
        return finish_incremental_interruption(job, reports, written_candidates);
    }
    let mut accepted_folders = Vec::with_capacity(source_folders.folders.len());
    let mut omitted_folders = source_folders.omitted_folders;
    let mut rejected_folder_keys = BTreeSet::new();
    for folder in source_folders.folders {
        let key = (folder.location, folder.path.clone());
        let trial = [folder.clone()];
        let layout = build_part_writer_layout(
            &trial,
            PartBuildOptions {
                source_sha256,
                recovery_mode: mode_name,
                maximum_pst_bytes,
                part_index,
                omitted_folders: 0,
            },
        )?;
        match validate_mail_store_layout(&layout) {
            Ok(()) => accepted_folders.push(folder),
            Err(WriterError::InputRejected(_)) => {
                rejected_folder_keys.insert(key);
                omitted_folders = omitted_folders.saturating_add(1);
                any_partial = true;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let mut recovered_folder_keys = accepted_folders
        .iter()
        .map(|folder| (folder.location, folder.path.clone()))
        .collect::<BTreeSet<_>>();
    let mut invalid_recovered_folder_keys = BTreeSet::new();
    let mut recovered_from_messages = 0_u64;
    for candidate in &candidates {
        let key = (candidate.folder_location, candidate.folder_path.clone());
        if !recovered_folder_keys.contains(&key) {
            let recovered = crate::CanonicalFolder {
                path: candidate.folder_path.clone(),
                location: candidate.folder_location,
                role: candidate.folder_role,
                container_class: Some(candidate.container_class.clone()),
            };
            let layout = build_part_writer_layout(
                std::slice::from_ref(&recovered),
                PartBuildOptions {
                    source_sha256,
                    recovery_mode: mode_name,
                    maximum_pst_bytes,
                    part_index,
                    omitted_folders: 0,
                },
            )?;
            match validate_mail_store_layout(&layout) {
                Ok(()) => {
                    recovered_folder_keys.insert(key);
                    accepted_folders.push(recovered);
                    recovered_from_messages = recovered_from_messages.saturating_add(1);
                }
                Err(WriterError::InputRejected(_)) => {
                    if !rejected_folder_keys.contains(&key)
                        && invalid_recovered_folder_keys.insert(key)
                    {
                        omitted_folders = omitted_folders.saturating_add(1);
                    }
                    any_partial = true;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    accepted_folders
        .sort_by(|left, right| (left.location, &left.path).cmp(&(right.location, &right.path)));
    tracing::info!(
        recovered_from_messages,
        folder_count = accepted_folders.len(),
        "incremental folder layout completed"
    );
    let source_folders = crate::CanonicalFolderSet {
        folders: accepted_folders,
        omitted_folders,
    };

    let mut terminal_candidate_keys = BTreeSet::new();
    let mut candidate_index = 0_usize;
    while candidate_index < candidates.len() {
        if interrupted.load(Ordering::Relaxed) {
            return finish_incremental_interruption(job, reports, written_candidates);
        }
        let filename = job.available_part_filename(part_index)?;
        let layout = build_part_writer_layout(
            &source_folders.folders,
            PartBuildOptions {
                source_sha256,
                recovery_mode: mode_name,
                maximum_pst_bytes,
                part_index,
                omitted_folders: 0,
            },
        )?;
        let store_record_key = layout.record_key;
        let layout_folder_count = saturating_len(layout.folders.len());
        let staged_filename = format!("part-{part_index:04}-transaction.pst.partial");
        let staged_path = job.staged_part_path(&staged_filename)?;
        remove_staged_if_present(&staged_path)?;
        let mut writer = TransactionalMailStoreWriter::begin(
            &staged_path,
            layout,
            &named_properties,
            part_index == 1,
            validator_supervisor,
        )?;
        let mut stats = IncrementalPartStats {
            omitted_folders: if part_index == 1 {
                source_folders.omitted_folders
            } else {
                0
            },
            ..IncrementalPartStats::default()
        };
        let part_started = Instant::now();
        let mut appended_top_level = 0_usize;
        let mut last_progress = Instant::now();
        let mut batch_checkpoint = Some(writer.begin_batch());
        let mut batch_candidate_index = candidate_index;
        let mut batch_stats = stats.clone();
        let mut batch_appended_top_level = appended_top_level;
        let mut batch_attempts = 0_usize;
        let mut batch_private_file_eof = writer.private_file_eof();
        let mut exact_replay = false;

        loop {
            if interrupted.load(Ordering::Relaxed) {
                drop(writer);
                return finish_incremental_interruption(job, reports, written_candidates);
            }
            let private_file_eof = writer.private_file_eof();
            let batch_byte_target =
                projection_batch_byte_target(maximum_pst_bytes, private_file_eof);
            let batch_bytes = private_file_eof.saturating_sub(batch_private_file_eof);
            if !exact_replay
                && batch_attempts != 0
                && writer.message_count() != 0
                && (batch_bytes >= batch_byte_target
                    || batch_attempts >= MAX_BATCH_MESSAGES
                    || candidate_index == candidates.len())
            {
                let projected_file_eof = writer.projected_file_eof(interrupted)?;
                if projected_file_eof > maximum_pst_bytes && writer.message_count() > 1 {
                    let checkpoint = batch_checkpoint.take().ok_or_else(|| {
                        WriterError::InvalidStructure(
                            "transactional batch checkpoint is unavailable".to_owned(),
                        )
                    })?;
                    writer.rollback_batch(checkpoint)?;
                    candidate_index = batch_candidate_index;
                    stats = batch_stats.clone();
                    appended_top_level = batch_appended_top_level;
                    batch_attempts = 0;
                    exact_replay = true;
                    tracing::debug!(
                        part_index,
                        projected_file_eof,
                        maximum_pst_bytes,
                        "transactional batch crossed the part boundary and will be replayed"
                    );
                    continue;
                }
                tracing::info!(
                    part_index,
                    appended_top_level,
                    private_file_eof = writer.private_file_eof(),
                    projected_file_eof,
                    maximum_pst_bytes,
                    elapsed_ms = part_started.elapsed().as_millis(),
                    "transactional PST batch accepted"
                );
                batch_checkpoint = Some(writer.begin_batch());
                batch_candidate_index = candidate_index;
                batch_stats = stats.clone();
                batch_appended_top_level = appended_top_level;
                batch_attempts = 0;
                batch_private_file_eof = writer.private_file_eof();
                last_progress = Instant::now();
            }
            if candidate_index == candidates.len() {
                break;
            }
            if terminal_candidate_keys.contains(&candidates[candidate_index].durable_key) {
                candidate_index = candidate_index.saturating_add(1);
                continue;
            }
            let candidate = &candidates[candidate_index];
            let mail = match load_canonical_mail_tree_from_folders_interruptible(
                job,
                &spooled_folders,
                &candidate.durable_key,
                interrupted,
            ) {
                Ok(mail) => mail,
                Err(CanonicalError::Job(JobError::Interrupted)) => {
                    drop(writer);
                    return finish_incremental_interruption(job, reports, written_candidates);
                }
                Err(error) => return Err(error.into()),
            };
            let relevant_folders = source_folders_for_mail(&source_folders.folders, &mail);
            let mut input = match build_part_writer_input_with_folders_interruptible(
                job,
                &[&mail],
                &relevant_folders,
                PartBuildOptions {
                    source_sha256,
                    recovery_mode: mode_name,
                    maximum_pst_bytes,
                    part_index,
                    omitted_folders: 0,
                },
                interrupted,
            ) {
                Ok(input) => input,
                Err(CanonicalWriteError::Job(JobError::Interrupted)) => {
                    drop(writer);
                    return finish_incremental_interruption(job, reports, written_candidates);
                }
                Err(error) if candidate_rejection_category(&error).is_some() => {
                    tracing::debug!(
                        error = %error,
                        durable_key = %candidate.durable_key,
                        "candidate rejected during incremental translation"
                    );
                    let rejections = translation_rejections(&mail, &error)
                        .ok_or_else(|| SplitError::Translation(error))?;
                    job.mark_candidate_rejections(&rejections)?;
                    any_partial = true;
                    terminal_candidate_keys.insert(candidate.durable_key.clone());
                    candidate_index = candidate_index.saturating_add(1);
                    batch_attempts = batch_attempts.saturating_add(1);
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            match validate_mail_store_input(&input.store) {
                Ok(()) => {}
                Err(WriterError::InputRejected(detail)) => {
                    tracing::debug!(
                        %detail,
                        durable_key = %candidate.durable_key,
                        "writer rejected candidate during incremental translation"
                    );
                    let mut item_keys = Vec::new();
                    collect_item_keys(&mail, &mut item_keys);
                    job.mark_candidates_unsupported(
                        &item_keys,
                        CandidateRejectionCategory::WriterInputRejected,
                    )?;
                    any_partial = true;
                    terminal_candidate_keys.insert(candidate.durable_key.clone());
                    candidate_index = candidate_index.saturating_add(1);
                    batch_attempts = batch_attempts.saturating_add(1);
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            let (location, path, associated, message) = take_incremental_message(&mut input)?;
            let message_count = count_messages(&message);
            let append_result = if exact_replay {
                writer.append_message(
                    location,
                    &path,
                    associated,
                    message,
                    maximum_pst_bytes,
                    interrupted,
                )
            } else {
                writer
                    .append_message_deferred(location, &path, associated, message, interrupted)
                    .map(|()| TransactionAppend::Appended {
                        projected_file_eof: 0,
                    })
            };
            match append_result {
                Ok(TransactionAppend::Appended { projected_file_eof }) => {
                    stats.item_keys.append(&mut input.item_keys);
                    stats
                        .unsupported_item_keys
                        .append(&mut input.unsupported_item_keys);
                    for folder in &input.store.folders {
                        stats
                            .folder_keys
                            .insert((folder.location, folder.path.clone()));
                    }
                    stats.message_count = stats.message_count.saturating_add(message_count);
                    stats.partial |= input.partial;
                    stats.omitted_properties = stats
                        .omitted_properties
                        .saturating_add(input.omitted_properties);
                    stats.omitted_attachments = stats
                        .omitted_attachments
                        .saturating_add(input.omitted_attachments);
                    stats.reconstructions.merge(input.reconstructions);
                    candidate_index = candidate_index.saturating_add(1);
                    appended_top_level = appended_top_level.saturating_add(1);
                    batch_attempts = batch_attempts.saturating_add(1);
                    if exact_replay
                        && (appended_top_level % 250 == 0
                            || last_progress.elapsed() >= Duration::from_secs(5))
                    {
                        tracing::info!(
                            part_index,
                            appended_top_level,
                            projected_file_eof,
                            maximum_pst_bytes,
                            elapsed_ms = part_started.elapsed().as_millis(),
                            "transactional PST append progress"
                        );
                        last_progress = Instant::now();
                    }
                }
                Ok(TransactionAppend::PartFull { rejected_file_eof }) => {
                    tracing::debug!(
                        part_index,
                        rejected_file_eof,
                        maximum_pst_bytes,
                        "transactional append established exact part boundary"
                    );
                    break;
                }
                Err(WriterError::Interrupted) => {
                    drop(writer);
                    return finish_incremental_interruption(job, reports, written_candidates);
                }
                Err(error) => return Err(error.into()),
            }
        }
        let top_level_count = writer.message_count();
        if top_level_count == 0 {
            drop(writer);
            any_partial = true;
            continue;
        }
        writer.finalize(interrupted)?;
        let byte_len = staged_path
            .metadata()
            .map_err(|source| staged_io(&staged_path, source))?
            .len();
        let oversize = byte_len > maximum_pst_bytes;
        if oversize && top_level_count != 1 {
            return Err(SplitError::PartExceedsMaximum {
                part_index,
                byte_len,
                maximum_bytes: maximum_pst_bytes,
            });
        }
        let Some(sha256) = hash_file(&staged_path, interrupted)? else {
            remove_staged_if_present(&staged_path)?;
            return finish_incremental_interruption(job, reports, written_candidates);
        };
        if !stats.unsupported_item_keys.is_empty() {
            job.mark_candidates_unsupported(
                &stats.unsupported_item_keys,
                CandidateRejectionCategory::UnsupportedEmbeddedItem,
            )?;
            any_partial = true;
        }
        let folder_count = if part_index == 1 {
            layout_folder_count
        } else {
            saturating_len(stats.folder_keys.len())
        };
        let partial = stats.partial
            || stats.omitted_folders != 0
            || stats.omitted_properties != 0
            || stats.omitted_attachments != 0;
        let published = PublishedPart {
            index: part_index,
            filename: filename.clone(),
            byte_len,
            sha256: sha256.clone(),
            oversize,
        };
        let sidecar = PartSidecar {
            schema_version: "1.1.0".to_owned(),
            producer_version: crate::VERSION.to_owned(),
            index: part_index,
            filename: filename.clone(),
            byte_len,
            sha256: sha256.clone(),
            store_record_key: hex(&store_record_key),
            folder_count,
            message_count: stats.message_count,
            oversize,
            partial,
            omitted_folders: stats.omitted_folders,
            omitted_properties: stats.omitted_properties,
            omitted_attachments: stats.omitted_attachments,
            reconstructions: stats.reconstructions.clone(),
        };
        job.publish_validated_part_interruptible(
            &staged_filename,
            &published,
            &sidecar,
            &stats.item_keys,
            interrupted,
        )?;
        tracing::info!(
            part_index,
            byte_len,
            message_count = stats.message_count,
            top_level_count,
            oversize,
            partial,
            serialization_count = 1,
            elapsed_ms = part_started.elapsed().as_millis(),
            "transactional PST part published"
        );
        reports.push(PartReport {
            index: part_index,
            filename,
            byte_len,
            sha256,
            folder_count,
            message_count: stats.message_count,
            oversize,
            partial,
            omitted_folders: stats.omitted_folders,
            omitted_properties: stats.omitted_properties,
            omitted_attachments: stats.omitted_attachments,
            reconstructions: stats.reconstructions,
        });
        written_candidates =
            written_candidates.saturating_add(saturating_len(stats.item_keys.len()));
        any_partial |= partial || oversize;
        part_index = part_index.checked_add(1).ok_or(SplitError::TooManyParts)?;
        if let Some(milliseconds) = test_pause_after_part() {
            std::thread::sleep(std::time::Duration::from_millis(milliseconds));
        }
    }
    finish_incremental_completion(job, reports, written_candidates, any_partial)
}

fn finish_incremental_completion(
    job: &mut DurableCatalogSink,
    reports: Vec<PartReport>,
    written_candidates: u64,
    mut any_partial: bool,
) -> Result<(Vec<PartReport>, u64, bool, bool), SplitError> {
    let stranded_embedded = job.mark_stranded_embedded_candidates_unsupported()?;
    if stranded_embedded != 0 {
        tracing::warn!(
            stranded_embedded,
            "embedded candidates stranded by finalized parents were marked unsupported"
        );
        any_partial = true;
    }
    job.clear_interrupted()?;
    job.checkpoint()?;
    Ok((reports, written_candidates, any_partial, false))
}

fn finish_incremental_interruption(
    job: &mut DurableCatalogSink,
    reports: Vec<PartReport>,
    written_candidates: u64,
) -> Result<(Vec<PartReport>, u64, bool, bool), SplitError> {
    job.record_interrupted()?;
    job.checkpoint()?;
    Ok((reports, written_candidates, true, true))
}

fn source_folders_for_mail(
    source_folders: &[crate::CanonicalFolder],
    mail: &crate::CanonicalMail,
) -> Vec<crate::CanonicalFolder> {
    source_folders
        .iter()
        .filter(|folder| {
            folder.location == mail.folder_location && mail.folder_path.starts_with(&folder.path)
        })
        .cloned()
        .collect()
}

fn take_incremental_message(
    input: &mut PartWriterInput,
) -> Result<(MailFolderLocation, Vec<String>, bool, MessageSpec), SplitError> {
    let mut found = None;
    for folder in &mut input.store.folders {
        if folder
            .messages
            .len()
            .saturating_add(folder.associated_messages.len())
            > 1
        {
            return Err(WriterError::InvalidStructure(
                "bounded translation produced multiple top-level messages".to_owned(),
            )
            .into());
        }
        let message = folder
            .messages
            .pop()
            .map(|message| (false, message))
            .or_else(|| {
                folder
                    .associated_messages
                    .pop()
                    .map(|message| (true, message))
            });
        if let Some((associated, message)) = message {
            if found.is_some() {
                return Err(WriterError::InvalidStructure(
                    "bounded translation assigned a message to multiple folders".to_owned(),
                )
                .into());
            }
            found = Some((folder.location, folder.path.clone(), associated, message));
        }
    }
    found.ok_or_else(|| {
        WriterError::InvalidStructure(
            "bounded translation produced no top-level message".to_owned(),
        )
        .into()
    })
}

#[allow(dead_code)]
fn split_recovered_job_with_interrupt_legacy(
    job_directory: &Path,
    source_sha256: &str,
    recovery_mode: RecoveryMode,
    maximum_pst_bytes: u64,
    interrupted: &AtomicBool,
    validator_supervisor: Option<&Path>,
) -> Result<(Vec<PartReport>, u64, bool, bool), SplitError> {
    if maximum_pst_bytes == 0 {
        return Err(SplitError::ZeroMaximumSize);
    }
    let mut job = match DurableCatalogSink::open_interruptible(job_directory, interrupted) {
        Ok(job) => job,
        Err(JobError::Interrupted) => return Ok((Vec::new(), 0, true, true)),
        Err(error) => return Err(error.into()),
    };
    let existing = job.published_parts()?;
    let mut reports = existing
        .iter()
        .map(part_report)
        .collect::<Result<Vec<_>, _>>()?;
    let mut written_candidates = existing
        .iter()
        .map(|record| record.item_count)
        .fold(0_u64, u64::saturating_add);
    let mut any_partial = reports
        .iter()
        .any(|report| report.partial || report.oversize);
    let mut part_index = existing.last().map_or(Ok(1_u32), |record| {
        record
            .part
            .index
            .checked_add(1)
            .ok_or(SplitError::TooManyParts)
    })?;
    let mail = match load_canonical_mail_interruptible(&job, interrupted) {
        Ok(mail) => mail,
        Err(CanonicalError::Job(JobError::Interrupted)) => {
            job.record_interrupted()?;
            job.checkpoint()?;
            return Ok((reports, written_candidates, true, true));
        }
        Err(error) => return Err(error.into()),
    };
    let source_folders = match load_canonical_folders_interruptible(&job, interrupted) {
        Ok(folders) => folders,
        Err(CanonicalError::Job(JobError::Interrupted)) => {
            job.record_interrupted()?;
            job.checkpoint()?;
            return Ok((reports, written_candidates, true, true));
        }
        Err(error) => return Err(error.into()),
    };
    if mail.is_empty() {
        job.checkpoint()?;
        return Ok((reports, written_candidates, any_partial, false));
    }
    if test_pause_at_prefilter(&job, interrupted)? {
        job.record_interrupted()?;
        job.checkpoint()?;
        return Ok((reports, written_candidates, true, true));
    }
    let mode_name = match recovery_mode {
        RecoveryMode::Balanced => "balanced",
        RecoveryMode::Aggressive => "aggressive",
    };
    let mut writable_mail = Vec::with_capacity(mail.len());
    for message in &mail {
        if interrupted.load(Ordering::Relaxed) {
            job.record_interrupted()?;
            job.checkpoint()?;
            return Ok((reports, written_candidates, true, true));
        }
        let messages = [message];
        let input = match build_part_writer_input_with_folders_interruptible(
            &job,
            &messages,
            &[],
            PartBuildOptions {
                source_sha256,
                recovery_mode: mode_name,
                maximum_pst_bytes,
                part_index,
                omitted_folders: 0,
            },
            interrupted,
        ) {
            Err(CanonicalWriteError::Job(JobError::Interrupted)) => {
                job.record_interrupted()?;
                job.checkpoint()?;
                return Ok((reports, written_candidates, true, true));
            }
            Ok(input) => input,
            Err(error) if candidate_rejection_category(&error).is_some() => {
                tracing::debug!(error = %error, "candidate translation rejected during prefilter");
                let rejections = translation_rejections(message, &error)
                    .ok_or_else(|| SplitError::Translation(error))?;
                job.mark_candidate_rejections(&rejections)?;
                any_partial = true;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        match validate_mail_store_input(&input.store) {
            Ok(()) => writable_mail.push(message),
            Err(WriterError::InputRejected(detail)) => {
                tracing::debug!(%detail, "writer input rejected candidate during prefilter");
                let mut item_keys = Vec::new();
                collect_item_keys(message, &mut item_keys);
                job.mark_candidates_unsupported(
                    &item_keys,
                    CandidateRejectionCategory::WriterInputRejected,
                )?;
                any_partial = true;
            }
            Err(error) => return Err(error.into()),
        }
    }
    if writable_mail.is_empty() {
        job.checkpoint()?;
        return Ok((reports, written_candidates, true, false));
    }
    let source_folders = {
        let mut accepted = Vec::with_capacity(source_folders.folders.len());
        let mut omitted_folders = if part_index == 1 {
            source_folders.omitted_folders
        } else {
            0
        };
        let probe = [writable_mail[0]];
        for folder in source_folders.folders {
            let trial = [folder.clone()];
            let input = match build_part_writer_input_with_folders_interruptible(
                &job,
                &probe,
                &trial,
                PartBuildOptions {
                    source_sha256,
                    recovery_mode: mode_name,
                    maximum_pst_bytes,
                    part_index,
                    omitted_folders: 0,
                },
                interrupted,
            ) {
                Ok(input) => input,
                Err(CanonicalWriteError::Job(JobError::Interrupted)) => {
                    job.record_interrupted()?;
                    job.checkpoint()?;
                    return Ok((reports, written_candidates, true, true));
                }
                Err(error) => return Err(error.into()),
            };
            match validate_mail_store_input(&input.store) {
                Ok(()) => accepted.push(folder),
                Err(WriterError::InputRejected(_)) => {
                    if part_index == 1 {
                        omitted_folders = omitted_folders.saturating_add(1);
                    }
                    any_partial = true;
                }
                Err(error) => return Err(error.into()),
            }
        }
        crate::CanonicalFolderSet {
            folders: accepted,
            omitted_folders,
        }
    };
    let by_key = writable_mail
        .iter()
        .map(|message| (message.key, *message))
        .collect::<BTreeMap<_, _>>();
    let candidates = writable_mail
        .iter()
        .map(|message| PackCandidate {
            key: message.key,
            folder_location: message.folder_location,
            folder_path: message.folder_path.clone(),
            payload_bytes: message.spooled_bytes,
        })
        .collect::<Vec<_>>();
    let ordered = canonical_candidate_order(candidates)?;
    let mut remaining = VecDeque::from(ordered);
    let mut size_model = AdaptiveSizeModel::default();
    let mut attempt = 0_u64;

    while !remaining.is_empty() {
        let mut candidate_group =
            take_fitting_prefix(&mut remaining, maximum_pst_bytes, &size_model)?;
        let mut rejected_prefix_len = None;
        let (input, staged_filename, staged_path, byte_len) = loop {
            if interrupted.load(Ordering::Relaxed) {
                job.record_interrupted()?;
                job.checkpoint()?;
                return Ok((reports, written_candidates, true, true));
            }
            attempt = attempt.saturating_add(1);
            let messages = candidate_group
                .iter()
                .map(|candidate| {
                    by_key
                        .get(&candidate.key)
                        .copied()
                        .ok_or(SplitError::UnknownAssignment)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let part_folders =
                source_folders_for_part(part_index, &source_folders.folders, &messages);
            let input = match build_part_writer_input_with_folders_interruptible(
                &job,
                &messages,
                &part_folders,
                PartBuildOptions {
                    source_sha256,
                    recovery_mode: mode_name,
                    maximum_pst_bytes,
                    part_index,
                    omitted_folders: if part_index == 1 {
                        source_folders.omitted_folders
                    } else {
                        0
                    },
                },
                interrupted,
            ) {
                Err(CanonicalWriteError::Job(JobError::Interrupted)) => {
                    job.record_interrupted()?;
                    job.checkpoint()?;
                    return Ok((reports, written_candidates, true, true));
                }
                Ok(input) => input,
                Err(error) => return Err(error.into()),
            };
            let staged_filename = format!("part-{part_index:04}-attempt-{attempt}.pst.partial");
            let staged_path = job.staged_part_path(&staged_filename)?;
            match validate_mail_store_input(&input.store) {
                Ok(()) => {}
                Err(WriterError::InputRejected(detail)) => {
                    return Err(WriterError::InputRejected(detail).into());
                }
                Err(error) => return Err(error.into()),
            }
            let write_result = write_staged_part(
                &staged_path,
                &input.store,
                interrupted,
                validator_supervisor,
            );
            match write_result {
                Ok(_) => {}
                Err(WriterError::Interrupted) => {
                    job.record_interrupted()?;
                    job.checkpoint()?;
                    return Ok((reports, written_candidates, true, true));
                }
                Err(error) => return Err(error.into()),
            }
            if interrupted.load(Ordering::Relaxed) {
                fs::remove_file(&staged_path).map_err(|source| staged_io(&staged_path, source))?;
                job.record_interrupted()?;
                job.checkpoint()?;
                return Ok((reports, written_candidates, true, true));
            }
            let byte_len = staged_path
                .metadata()
                .map_err(|source| staged_io(&staged_path, source))?
                .len();
            if byte_len > maximum_pst_bytes
                && candidate_group.len() == 1
                && part_index == 1
                && !source_folders.folders.is_empty()
            {
                let baseline_input = build_part_writer_input_with_folders_interruptible(
                    &job,
                    &messages,
                    &[],
                    PartBuildOptions {
                        source_sha256,
                        recovery_mode: mode_name,
                        maximum_pst_bytes,
                        part_index,
                        omitted_folders: 0,
                    },
                    interrupted,
                )?;
                validate_mail_store_input(&baseline_input.store)?;
                let baseline_filename =
                    format!("part-{part_index:04}-attempt-{attempt}-baseline.pst.partial");
                let baseline_path = job.staged_part_path(&baseline_filename)?;
                match write_staged_part(
                    &baseline_path,
                    &baseline_input.store,
                    interrupted,
                    validator_supervisor,
                ) {
                    Ok(()) => {}
                    Err(WriterError::Interrupted) => {
                        remove_staged_if_present(&staged_path)?;
                        job.record_interrupted()?;
                        job.checkpoint()?;
                        return Ok((reports, written_candidates, true, true));
                    }
                    Err(error) => {
                        remove_staged_if_present(&staged_path)?;
                        return Err(error.into());
                    }
                }
                let baseline_len = baseline_path
                    .metadata()
                    .map_err(|source| staged_io(&baseline_path, source))?
                    .len();
                remove_staged_if_present(&baseline_path)?;
                if baseline_len <= maximum_pst_bytes {
                    remove_staged_if_present(&staged_path)?;
                    return Err(SplitError::PartExceedsMaximum {
                        part_index,
                        byte_len,
                        maximum_bytes: maximum_pst_bytes,
                    });
                }
            }
            let estimated_bytes = LayoutEstimator.estimate_part_bytes(&candidate_group)?;
            size_model.observe(estimated_bytes, byte_len)?;
            if byte_len > maximum_pst_bytes && candidate_group.len() > 1 {
                fs::remove_file(&staged_path).map_err(|source| staged_io(&staged_path, source))?;
                rejected_prefix_len = Some(
                    rejected_prefix_len.map_or(candidate_group.len(), |known: usize| {
                        known.min(candidate_group.len())
                    }),
                );
                shrink_to_fitting_prefix(
                    &mut candidate_group,
                    &mut remaining,
                    maximum_pst_bytes,
                    &size_model,
                )?;
                continue;
            }
            if byte_len <= maximum_pst_bytes
                && extend_to_fitting_prefix(
                    &mut candidate_group,
                    &mut remaining,
                    maximum_pst_bytes,
                    &size_model,
                    rejected_prefix_len,
                )?
            {
                fs::remove_file(&staged_path).map_err(|source| staged_io(&staged_path, source))?;
                continue;
            }
            break (input, staged_filename, staged_path, byte_len);
        };
        if !input.unsupported_item_keys.is_empty() {
            job.mark_candidates_unsupported(
                &input.unsupported_item_keys,
                CandidateRejectionCategory::UnsupportedEmbeddedItem,
            )?;
            any_partial = true;
        }
        let oversize = byte_len > maximum_pst_bytes;
        let Some(sha256) = hash_file(&staged_path, interrupted)? else {
            fs::remove_file(&staged_path).map_err(|source| staged_io(&staged_path, source))?;
            job.record_interrupted()?;
            job.checkpoint()?;
            return Ok((reports, written_candidates, true, true));
        };
        if interrupted.load(Ordering::Relaxed) {
            fs::remove_file(&staged_path).map_err(|source| staged_io(&staged_path, source))?;
            job.record_interrupted()?;
            job.checkpoint()?;
            return Ok((reports, written_candidates, true, true));
        }
        let filename = format!("part-{part_index:04}.pst");
        let published = PublishedPart {
            index: part_index,
            filename: filename.clone(),
            byte_len,
            sha256: sha256.clone(),
            oversize,
        };
        let folder_count = saturating_len(input.store.folders.len());
        let message_count = input
            .store
            .folders
            .iter()
            .flat_map(|folder| {
                folder
                    .messages
                    .iter()
                    .chain(folder.associated_messages.iter())
            })
            .map(count_messages)
            .fold(0_u64, u64::saturating_add);
        // Translator omissions already include every diagnostic record returned
        // by the writer; the report list must not increment the same occurrence.
        let omitted_properties = input.omitted_properties;
        let partial = input.partial
            || input.omitted_folders != 0
            || omitted_properties != 0
            || input.omitted_attachments != 0;
        let sidecar = PartSidecar {
            schema_version: "1.1.0".to_owned(),
            producer_version: crate::VERSION.to_owned(),
            index: part_index,
            filename: filename.clone(),
            byte_len,
            sha256: sha256.clone(),
            store_record_key: hex(&input.store.record_key),
            folder_count,
            message_count,
            oversize,
            partial,
            omitted_folders: input.omitted_folders,
            omitted_properties,
            omitted_attachments: input.omitted_attachments,
            reconstructions: input.reconstructions.clone(),
        };
        match job.publish_validated_part_interruptible(
            &staged_filename,
            &published,
            &sidecar,
            &input.item_keys,
            interrupted,
        ) {
            Ok(()) => {}
            Err(JobError::Interrupted) => {
                job.record_interrupted()?;
                job.checkpoint()?;
                return Ok((reports, written_candidates, true, true));
            }
            Err(error) => return Err(error.into()),
        }
        tracing::info!(
            part_index,
            byte_len,
            message_count,
            oversize,
            partial,
            "validated PST part published"
        );
        reports.push(PartReport {
            index: part_index,
            filename,
            byte_len,
            sha256,
            folder_count,
            message_count,
            oversize,
            partial,
            omitted_folders: input.omitted_folders,
            omitted_properties,
            omitted_attachments: input.omitted_attachments,
            reconstructions: input.reconstructions,
        });
        written_candidates =
            written_candidates.saturating_add(saturating_len(input.item_keys.len()));
        any_partial |= partial || oversize;
        part_index = part_index.checked_add(1).ok_or(SplitError::TooManyParts)?;
        if let Some(milliseconds) = test_pause_after_part() {
            std::thread::sleep(std::time::Duration::from_millis(milliseconds));
        }
        if interrupted.load(Ordering::Relaxed) {
            job.record_interrupted()?;
            job.checkpoint()?;
            return Ok((reports, written_candidates, true, true));
        }
    }
    if interrupted.load(Ordering::Relaxed) {
        job.record_interrupted()?;
        job.checkpoint()?;
        return Ok((reports, written_candidates, true, true));
    }
    job.clear_interrupted()?;
    job.checkpoint()?;
    Ok((reports, written_candidates, any_partial, false))
}

fn source_folders_for_part(
    part_index: u32,
    source_folders: &[crate::CanonicalFolder],
    messages: &[&crate::CanonicalMail],
) -> Vec<crate::CanonicalFolder> {
    if part_index == 1 {
        return source_folders.to_vec();
    }
    source_folders
        .iter()
        .filter(|folder| {
            messages.iter().any(|message| {
                message.folder_location == folder.location
                    && message.folder_path.starts_with(&folder.path)
            })
        })
        .cloned()
        .collect()
}

fn write_staged_part(
    staged_path: &Path,
    store: &pstforge_pst::writer::MailStoreSpec,
    interrupted: &AtomicBool,
    validator_supervisor: Option<&Path>,
) -> Result<(), WriterError> {
    match validator_supervisor {
        Some(supervisor) => {
            create_mail_store_supervised(staged_path, store, interrupted, supervisor).map(|_| ())
        }
        None => create_mail_store_interruptible(staged_path, store, interrupted).map(|_| ()),
    }
}

fn remove_staged_if_present(path: &Path) -> Result<(), SplitError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(staged_io(path, source)),
    }
}

fn test_pause_after_part() -> Option<u64> {
    std::env::var("PSTFORGE_TEST_PAUSE_AFTER_PART_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
}

fn test_pause_at_prefilter(
    job: &DurableCatalogSink,
    interrupted: &AtomicBool,
) -> Result<bool, SplitError> {
    let Some(milliseconds) = std::env::var("PSTFORGE_TEST_PAUSE_AT_PREFILTER_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return Ok(false);
    };
    let marker = job.staged_part_path("prefilter-test-marker.partial")?;
    fs::write(&marker, b"candidate prefilter active").map_err(|source| SplitError::StagedIo {
        path: marker.display().to_string(),
        source,
    })?;
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).map_err(|source| {
        SplitError::StagedIo {
            path: marker.display().to_string(),
            source,
        }
    })?;
    let deadline = Instant::now() + Duration::from_millis(milliseconds);
    while Instant::now() < deadline && !interrupted.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !interrupted.load(Ordering::Relaxed) {
        fs::remove_file(&marker).map_err(|source| SplitError::StagedIo {
            path: marker.display().to_string(),
            source,
        })?;
    }
    Ok(interrupted.load(Ordering::Relaxed))
}

fn part_report(record: &PublishedPartRecord) -> Result<PartReport, SplitError> {
    Ok(PartReport {
        index: record.part.index,
        filename: record.part.filename.clone(),
        byte_len: record.part.byte_len,
        sha256: record.part.sha256.clone(),
        folder_count: record.sidecar.folder_count,
        message_count: record.sidecar.message_count,
        oversize: record.part.oversize,
        partial: record.sidecar.partial,
        omitted_folders: record.sidecar.omitted_folders,
        omitted_properties: record.sidecar.omitted_properties,
        omitted_attachments: record.sidecar.omitted_attachments,
        reconstructions: record.sidecar.reconstructions.clone(),
    })
}

fn recovery_mode_name(mode: RecoveryMode) -> &'static str {
    match mode {
        RecoveryMode::Balanced => "balanced",
        RecoveryMode::Aggressive => "aggressive",
    }
}

fn disk_preflight(
    job_directory: &Path,
    source_bytes: u64,
    existing_job_bytes: u64,
) -> Result<DiskPreflight, SplitError> {
    let required_bytes = source_bytes
        .saturating_mul(3)
        .saturating_sub(existing_job_bytes);
    let path = preflight_filesystem_path(job_directory);
    let stats = rustix::fs::statvfs(path).map_err(|source| SplitError::DiskSpaceIo {
        path: path.display().to_string(),
        source: source.into(),
    })?;
    let available_bytes = stats.f_bavail.saturating_mul(stats.f_frsize);
    if available_bytes < required_bytes {
        return Err(SplitError::InsufficientDiskSpace {
            required_bytes,
            available_bytes,
        });
    }
    Ok(DiskPreflight {
        required_bytes,
        available_bytes,
        existing_job_bytes,
    })
}

fn preflight_filesystem_path(job_directory: &Path) -> &Path {
    let mut path = job_directory;
    while !path.exists() {
        path = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
    }
    path
}

fn execution_metrics(
    started: Instant,
    source_bytes: u64,
    parts: &[PartReport],
    peak_worker_rss_bytes: u64,
) -> ExecutionMetrics {
    let elapsed_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let output_bytes = parts
        .iter()
        .map(|part| part.byte_len)
        .fold(0_u64, u64::saturating_add);
    let average_source_bytes_per_second = if elapsed_millis == 0 {
        0
    } else {
        source_bytes
            .saturating_mul(1000)
            .checked_div(elapsed_millis)
            .unwrap_or(0)
    };
    ExecutionMetrics {
        elapsed_millis,
        source_bytes,
        output_bytes,
        average_source_bytes_per_second,
        peak_process_rss_bytes: peak_worker_rss_bytes.max(self_peak_rss_bytes()),
    }
}

fn self_peak_rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmHWM:")?
                    .split_ascii_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
        })
        .and_then(|kibibytes| kibibytes.checked_mul(1024))
        .unwrap_or(0)
}

fn candidate_rejection_category(error: &CanonicalWriteError) -> Option<CandidateRejectionCategory> {
    match error {
        CanonicalWriteError::InvalidCandidate { .. } => {
            Some(CandidateRejectionCategory::MalformedCandidate)
        }
        CanonicalWriteError::InvalidProperty { .. } => {
            Some(CandidateRejectionCategory::MalformedProperty)
        }
        CanonicalWriteError::Job(_) | CanonicalWriteError::BlobRead { .. } => None,
    }
}

fn translation_rejections(
    message: &crate::CanonicalMail,
    error: &CanonicalWriteError,
) -> Option<Vec<(String, CandidateRejectionCategory)>> {
    let category = candidate_rejection_category(error)?;
    let target = match error {
        CanonicalWriteError::InvalidCandidate { item_key, .. }
        | CanonicalWriteError::InvalidProperty { item_key, .. } => item_key,
        CanonicalWriteError::Job(_) | CanonicalWriteError::BlobRead { .. } => return None,
    };
    let mut path = Vec::new();
    if !find_item_path(message, target, &mut path) {
        return None;
    }
    let mut rejections = path
        .iter()
        .take(path.len().saturating_sub(1))
        .cloned()
        .map(|item_key| {
            (
                item_key,
                CandidateRejectionCategory::ItemGraphDependencyRejected,
            )
        })
        .collect::<Vec<_>>();
    rejections.push((target.clone(), category));
    Some(rejections)
}

fn find_item_path(message: &crate::CanonicalMail, target: &str, path: &mut Vec<String>) -> bool {
    path.push(message.durable_item_key.clone());
    if message.durable_item_key == target {
        return true;
    }
    for attachment in &message.attachments {
        if let Some(embedded) = &attachment.embedded
            && find_item_path(embedded, target, path)
        {
            return true;
        }
    }
    path.pop();
    false
}

fn collect_item_keys(message: &crate::CanonicalMail, item_keys: &mut Vec<String>) {
    item_keys.push(message.durable_item_key.clone());
    for attachment in &message.attachments {
        if let Some(embedded) = &attachment.embedded {
            collect_item_keys(embedded, item_keys);
        }
    }
}

fn canonical_candidate_order(
    mut candidates: Vec<PackCandidate>,
) -> Result<Vec<PackCandidate>, PackingError> {
    for candidate in &candidates {
        if candidate.folder_path.iter().any(String::is_empty) {
            return Err(PackingError::InvalidFolderPath);
        }
    }
    candidates.sort_by(|left, right| {
        left.folder_location
            .cmp(&right.folder_location)
            .then_with(|| left.folder_path.cmp(&right.folder_path))
            .then_with(|| left.key.cmp(&right.key))
    });
    let mut keys = BTreeSet::new();
    for candidate in &candidates {
        if !keys.insert(candidate.key) {
            return Err(PackingError::DuplicateCandidate(candidate.key));
        }
    }
    Ok(candidates)
}

#[derive(Default)]
struct AdaptiveSizeModel {
    observed: Option<(u64, u64)>,
}

impl AdaptiveSizeModel {
    fn predict(&self, estimated_bytes: u64) -> Result<u64, PackingError> {
        let Some((observed_estimate, observed_actual)) = self.observed else {
            return Ok(estimated_bytes);
        };
        let numerator = u128::from(estimated_bytes)
            .checked_mul(u128::from(observed_actual))
            .ok_or(PackingError::SizeOverflow)?;
        let predicted = numerator
            .checked_add(u128::from(observed_estimate.saturating_sub(1)))
            .ok_or(PackingError::SizeOverflow)?
            / u128::from(observed_estimate);
        u64::try_from(predicted).map_err(|_| PackingError::SizeOverflow)
    }

    fn observe(&mut self, estimated_bytes: u64, actual_bytes: u64) -> Result<(), PackingError> {
        if estimated_bytes == 0 {
            return Err(PackingError::Estimator(
                "adaptive size observation has a zero estimate".to_owned(),
            ));
        }
        self.observed = Some((estimated_bytes, actual_bytes));
        Ok(())
    }
}

fn fitting_prefix_len(
    candidates: &[PackCandidate],
    maximum_pst_bytes: u64,
    model: &AdaptiveSizeModel,
) -> Result<usize, PackingError> {
    if candidates.is_empty() {
        return Ok(0);
    }
    let fits = |length: usize| -> Result<bool, PackingError> {
        let estimated = LayoutEstimator.estimate_part_bytes(&candidates[..length])?;
        Ok(model.predict(estimated)? <= maximum_pst_bytes)
    };
    if !fits(1)? {
        return Ok(1);
    }
    let mut low = 1_usize;
    let mut high = candidates.len().saturating_add(1);
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if fits(middle)? {
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

fn take_fitting_prefix(
    remaining: &mut VecDeque<PackCandidate>,
    maximum_pst_bytes: u64,
    model: &AdaptiveSizeModel,
) -> Result<Vec<PackCandidate>, PackingError> {
    let count = fitting_prefix_len(remaining.make_contiguous(), maximum_pst_bytes, model)?;
    let mut selected = Vec::with_capacity(count);
    for _ in 0..count {
        selected.push(remaining.pop_front().ok_or_else(|| {
            PackingError::Estimator("candidate queue changed during packing".to_owned())
        })?);
    }
    Ok(selected)
}

fn extend_to_fitting_prefix(
    selected: &mut Vec<PackCandidate>,
    remaining: &mut VecDeque<PackCandidate>,
    maximum_pst_bytes: u64,
    model: &AdaptiveSizeModel,
    rejected_prefix_len: Option<usize>,
) -> Result<bool, PackingError> {
    if remaining.is_empty() {
        return Ok(false);
    }
    let selected_len = selected.len();
    let maximum_count = rejected_prefix_len
        .map(|count| count.saturating_sub(1))
        .unwrap_or_else(|| selected_len.saturating_add(remaining.len()));
    if maximum_count <= selected_len {
        return Ok(false);
    }
    let maximum_additional = maximum_count
        .saturating_sub(selected_len)
        .min(remaining.len());
    let fits = |additional: usize| -> Result<bool, PackingError> {
        let estimated =
            estimate_candidate_iter(selected.iter().chain(remaining.iter().take(additional)))?;
        Ok(model.predict(estimated)? <= maximum_pst_bytes)
    };
    if maximum_additional == 0 || !fits(1)? {
        return Ok(false);
    }
    let mut low = 1_usize;
    let mut high = 2_usize.min(maximum_additional);
    while high < maximum_additional && fits(high)? {
        low = high;
        high = high.saturating_mul(2).min(maximum_additional);
    }
    if fits(high)? {
        low = high;
    } else {
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if fits(middle)? {
                low = middle;
            } else {
                high = middle;
            }
        }
    }
    for _ in 0..low {
        selected.push(remaining.pop_front().ok_or_else(|| {
            PackingError::Estimator("candidate queue changed during expansion".to_owned())
        })?);
    }
    Ok(true)
}

fn shrink_to_fitting_prefix(
    selected: &mut Vec<PackCandidate>,
    remaining: &mut VecDeque<PackCandidate>,
    maximum_pst_bytes: u64,
    model: &AdaptiveSizeModel,
) -> Result<(), PackingError> {
    let mut count = fitting_prefix_len(selected, maximum_pst_bytes, model)?;
    if count >= selected.len() {
        count = selected.len().saturating_sub(1).max(1);
    }
    let returned = selected.split_off(count);
    for candidate in returned.into_iter().rev() {
        remaining.push_front(candidate);
    }
    Ok(())
}

struct LayoutEstimator;

impl PartSizeEstimator for LayoutEstimator {
    fn estimate_part_bytes(&self, candidates: &[PackCandidate]) -> Result<u64, PackingError> {
        estimate_candidate_iter(candidates.iter())
    }
}

fn estimate_candidate_iter<'a>(
    candidates: impl Iterator<Item = &'a PackCandidate>,
) -> Result<u64, PackingError> {
    let mut folders = BTreeSet::new();
    let mut estimated = ESTIMATED_STORE_BYTES;
    for candidate in candidates {
        for length in 1..=candidate.folder_path.len() {
            folders.insert(candidate.folder_path[..length].to_vec());
        }
        estimated = estimated
            .checked_add(candidate.payload_bytes)
            .and_then(|value| value.checked_add(ESTIMATED_MESSAGE_BYTES))
            .ok_or(PackingError::SizeOverflow)?;
    }
    estimated
        .checked_add(saturating_len(folders.len()).saturating_mul(ESTIMATED_FOLDER_BYTES))
        .ok_or(PackingError::SizeOverflow)
}

fn count_messages(message: &MessageSpec) -> u64 {
    message.attachments.iter().fold(1_u64, |count, attachment| {
        if let AttachmentContent::Embedded(embedded) = &attachment.content {
            count.saturating_add(count_messages(embedded))
        } else {
            count
        }
    })
}

fn hash_file(path: &Path, interrupted: &AtomicBool) -> Result<Option<String>, SplitError> {
    let mut file = File::open(path).map_err(|source| staged_io(path, source))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        if interrupted.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let read = file
            .read(&mut buffer)
            .map_err(|source| staged_io(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Some(hex(&hasher.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn saturating_len(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn staged_io(path: &Path, source: std::io::Error) -> SplitError {
    SplitError::StagedIo {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    use pstforge_job::{
        CandidateRejectionCategory, CandidateRejectionCounts, JobError, ReconstructedField,
    };
    use pstforge_pst::writer::WriterError;

    use tempfile::tempdir;

    use super::{
        AdaptiveSizeModel, DiskPreflight, ExecutionMetrics, LayoutEstimator,
        MAX_RECOVERY_LOG_DETAIL_LINES, PartReport, SplitError, SplitFailureKind, SplitReport,
        disk_preflight, extend_to_fitting_prefix, hash_file, preflight_filesystem_path,
        projection_batch_byte_target, render_recovery_log, shrink_to_fitting_prefix,
        source_folders_for_part, take_fitting_prefix, translation_rejections,
    };
    use crate::{
        CanonicalAttachment, CanonicalFolder, CanonicalFolderRole, CanonicalMail,
        CanonicalWriteError, ContentCompleteness, ItemKey, PackCandidate, PartSizeEstimator,
        RecoveryError, RecoveryProvenance, RecoveryReport, SourceError, SourceIdentity,
    };

    fn packing_candidate(index: u32, payload_bytes: u64) -> PackCandidate {
        PackCandidate {
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(index),
                recovery_index: None,
                occurrence: 0,
            },
            folder_location: crate::CanonicalFolderLocation::IpmSubtree,
            folder_path: vec!["Inbox".to_owned()],
            payload_bytes,
        }
    }

    fn bare_mail(id: u32) -> CanonicalMail {
        CanonicalMail {
            durable_item_key: format!("normal:{id}:-:0"),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(id),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Inbox".to_owned()],
            folder_location: crate::CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: crate::CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Note".to_owned()),
            subject: None,
            sender_name: None,
            sender_email: None,
            submit_filetime: None,
            delivery_filetime: None,
            recipients: Vec::new(),
            attachments: Vec::new(),
            properties: Vec::new(),
            completeness: ContentCompleteness::Complete,
            spooled_bytes: 0,
        }
    }

    fn embed(parent: &mut CanonicalMail, child: CanonicalMail) {
        let index = u32::try_from(parent.attachments.len()).expect("fixture attachment count");
        parent.attachments.push(CanonicalAttachment {
            index,
            attachment_type: Some(i32::from(b'i')),
            filename: None,
            declared_size: None,
            data: None,
            data_complete: true,
            reference_complete: true,
            properties: Vec::new(),
            embedded: Some(Box::new(child)),
        });
    }

    #[test]
    fn nested_translation_rejection_distinguishes_direct_and_dependent_items() {
        let grandchild = bare_mail(3);
        let mut child = bare_mail(2);
        embed(&mut child, grandchild);
        let sibling = bare_mail(4);
        let mut root = bare_mail(1);
        embed(&mut root, child);
        embed(&mut root, sibling);

        let error = CanonicalWriteError::InvalidProperty {
            item_key: "normal:2:-:0".to_owned(),
            property_id: 0x1000,
            detail: "private diagnostic".to_owned(),
        };
        assert_eq!(
            translation_rejections(&root, &error),
            Some(vec![
                (
                    "normal:1:-:0".to_owned(),
                    CandidateRejectionCategory::ItemGraphDependencyRejected,
                ),
                (
                    "normal:2:-:0".to_owned(),
                    CandidateRejectionCategory::MalformedProperty,
                ),
            ])
        );
    }

    #[test]
    fn projection_batches_scale_with_the_requested_part_size() {
        const MIB: u64 = 1024 * 1024;
        const GIB: u64 = 1024 * MIB;
        assert_eq!(projection_batch_byte_target(64 * MIB, 0), 4 * MIB);
        assert_eq!(projection_batch_byte_target(64 * MIB, 60 * MIB), 256 * 1024);
        assert_eq!(projection_batch_byte_target(4 * GIB, 0), 256 * MIB);
        assert_eq!(
            projection_batch_byte_target(4 * GIB, 4 * GIB - 256 * MIB),
            16 * MIB
        );
    }

    #[test]
    fn later_parts_retain_only_used_source_folder_metadata() {
        let folders = [
            CanonicalFolder {
                path: vec!["Contacts".to_owned()],
                location: crate::CanonicalFolderLocation::IpmSubtree,
                role: CanonicalFolderRole::Ordinary,
                container_class: Some("IPF.Contact".to_owned()),
            },
            CanonicalFolder {
                path: vec!["Contacts".to_owned(), "Child".to_owned()],
                location: crate::CanonicalFolderLocation::IpmSubtree,
                role: CanonicalFolderRole::Ordinary,
                container_class: Some("IPF.Contact".to_owned()),
            },
            CanonicalFolder {
                path: vec!["Contacts".to_owned(), "Empty Child".to_owned()],
                location: crate::CanonicalFolderLocation::IpmSubtree,
                role: CanonicalFolderRole::Ordinary,
                container_class: Some("IPF.Contact".to_owned()),
            },
        ];
        let mail = CanonicalMail {
            durable_item_key: "normal:1:-:0".to_owned(),
            key: ItemKey {
                provenance: RecoveryProvenance::Normal,
                source_node_id: Some(1),
                recovery_index: None,
                occurrence: 0,
            },
            folder_path: vec!["Contacts".to_owned(), "Child".to_owned()],
            folder_location: crate::CanonicalFolderLocation::IpmSubtree,
            folder_role: CanonicalFolderRole::Ordinary,
            placement: crate::CanonicalMessagePlacement::Normal,
            message_class: Some("IPM.Note".to_owned()),
            subject: Some("ordinary item in contacts folder".to_owned()),
            sender_name: Some("Sender".to_owned()),
            sender_email: Some("sender@example.com".to_owned()),
            submit_filetime: None,
            delivery_filetime: None,
            recipients: Vec::new(),
            attachments: Vec::new(),
            properties: Vec::new(),
            completeness: ContentCompleteness::Complete,
            spooled_bytes: 0,
        };

        assert_eq!(source_folders_for_part(1, &folders, &[&mail]), folders);
        assert_eq!(
            source_folders_for_part(2, &folders, &[&mail]),
            [folders[0].clone(), folders[1].clone()]
        );
    }

    fn split_report() -> SplitReport {
        SplitReport {
            schema_version: "0.4.4".to_owned(),
            command: "split".to_owned(),
            maximum_pst_bytes: 4_294_967_296,
            resumed: false,
            keep_work: false,
            disk_preflight: DiskPreflight {
                required_bytes: 0,
                available_bytes: 0,
                existing_job_bytes: 0,
            },
            metrics: ExecutionMetrics {
                elapsed_millis: 1,
                source_bytes: 100,
                output_bytes: 80,
                average_source_bytes_per_second: 100_000,
                peak_process_rss_bytes: 1,
            },
            recovery: RecoveryReport {
                schema_version: "0.4.4".to_owned(),
                command: "recover".to_owned(),
                mode: "balanced".to_owned(),
                source: SourceIdentity {
                    canonical_path: "/private/source.pst".to_owned(),
                    device: 1,
                    inode: 2,
                    size_bytes: 100,
                    modified_at: "2026-01-01T00:00:00Z".to_owned(),
                    sha256: "a".repeat(64),
                },
                job_directory: "/private/job".to_owned(),
                normal_items: 1,
                recovered_items: 0,
                orphan_items: 0,
                fragment_items: 0,
                committed_candidates: 1,
                complete_candidates: 1,
                partial_candidates: 0,
                unsupported_candidates: 0,
                blob_count: 0,
                blob_bytes: 0,
                issues: 0,
                issues_dropped: 0,
                worker_attempts: 1,
                worker_failures: 0,
                isolated_units: 0,
                peak_worker_rss_bytes: 1,
                interrupted: false,
                source_unchanged: true,
            },
            rejection_counts: CandidateRejectionCounts::new(),
            parts: vec![PartReport {
                index: 1,
                filename: "part-0001.pst".to_owned(),
                byte_len: 80,
                sha256: "b".repeat(64),
                folder_count: 1,
                message_count: 1,
                oversize: false,
                partial: false,
                omitted_folders: 0,
                omitted_properties: 0,
                omitted_attachments: 0,
                reconstructions: Default::default(),
            }],
            written_candidates: 1,
            partial: false,
        }
    }

    #[test]
    fn recovery_log_is_human_readable_bounded_and_excludes_private_paths() {
        let mut report = split_report();
        let clean = render_recovery_log(&report);
        assert!(clean.contains("Result: complete"));
        assert!(clean.contains("No readable data was skipped."));
        assert!(clean.contains("No source metadata required recovery handling."));
        assert!(clean.contains("part-0001.pst: 80 bytes"));
        assert!(!clean.contains("/private/source.pst"));
        assert!(!clean.contains("/private/job"));

        report.parts[0]
            .reconstructions
            .record_derived(ReconstructedField::RecipientDisplayName);
        report.parts[0]
            .reconstructions
            .record_generated(ReconstructedField::AttachmentFilename);
        report.parts[0]
            .reconstructions
            .record_generated(ReconstructedField::Subject);
        report.parts[0]
            .reconstructions
            .record_generated(ReconstructedField::SenderName);
        report.parts[0]
            .reconstructions
            .record_generated(ReconstructedField::SenderAddress);
        let reconstructed = render_recovery_log(&report);
        assert!(reconstructed.contains("Result: complete"));
        assert!(reconstructed.contains("Derived from other readable source values:"));
        assert!(reconstructed.contains("Recipient display name: 1"));
        assert!(reconstructed.contains(
            "Source metadata absent or unusable; defaults generated or fields left absent:"
        ));
        assert!(reconstructed.contains("Attachment filename: 1"));
        assert!(reconstructed.contains("Subject: 1"));
        assert!(reconstructed.contains("Sender name: 1"));
        assert!(reconstructed.contains("Sender address: 1"));

        report.partial = true;
        report.recovery.unsupported_candidates = 2;
        report
            .rejection_counts
            .insert(CandidateRejectionCategory::MalformedProperty, 1);
        report
            .rejection_counts
            .insert(CandidateRejectionCategory::UnsupportedEmbeddedItem, 1);
        report.parts[0].omitted_attachments = 3;
        let partial = render_recovery_log(&report);
        assert!(partial.contains("Result: partial"));
        assert!(partial.contains("Attachments not copied: 3"));
        assert!(partial.contains("could not yet be preserved safely: 2"));
        assert!(partial.contains("A required item property was malformed: 1"));
        assert!(partial.contains("Embedded item type or linkage could not be preserved: 1"));
        assert!(!partial.contains("/private/"));
        let json = serde_json::to_value(&report).expect("split report serializes");
        assert_eq!(
            json["rejection_counts"]["malformed_property"],
            serde_json::json!(1)
        );
        assert!(partial.len() < 4 * 1024 * 1024);

        let part = report.parts[0].clone();
        report.parts = vec![part; MAX_RECOVERY_LOG_DETAIL_LINES + 1];
        let bounded = render_recovery_log(&report);
        assert!(bounded.contains("Additional output files omitted from detail: 1"));
        assert_eq!(
            bounded.matches("part-0001.pst: 80 bytes").count(),
            MAX_RECOVERY_LOG_DETAIL_LINES
        );
        assert!(bounded.len() < 4 * 1024 * 1024);
    }

    #[test]
    fn failure_kinds_separate_source_output_and_conformance() {
        let unsafe_output = SplitError::Recovery(RecoveryError::Source(SourceError::UnsafeOutput(
            PathBuf::from("job"),
        )));
        assert_eq!(unsafe_output.failure_kind(), SplitFailureKind::Output);
        let changed_source = SplitError::Recovery(RecoveryError::Source(SourceError::Changed(
            PathBuf::from("source.pst"),
        )));
        assert_eq!(changed_source.failure_kind(), SplitFailureKind::Source);
        let disk_full =
            SplitError::Writer(WriterError::Io(io::Error::from(io::ErrorKind::StorageFull)));
        assert_eq!(disk_full.failure_kind(), SplitFailureKind::Output);
        let job_failure = SplitError::Job(JobError::ExistingJob(PathBuf::from("job")));
        assert_eq!(job_failure.failure_kind(), SplitFailureKind::Output);
        let rejected = SplitError::Writer(WriterError::InvalidStructure("bad PST".to_owned()));
        assert_eq!(rejected.failure_kind(), SplitFailureKind::Conformance);
        let completed_validation = WriterError::CompletedValidation("bad output".to_owned());
        let invalid_output = SplitError::Writer(completed_validation);
        assert_eq!(invalid_output.failure_kind(), SplitFailureKind::Conformance);
        let interrupted = SplitError::Writer(WriterError::Interrupted);
        assert_eq!(interrupted.failure_kind(), SplitFailureKind::Interrupted);
    }

    #[test]
    fn disk_preflight_refuses_impossible_capacity_without_creating_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let error = disk_preflight(&job, u64::MAX, 0).expect_err("capacity must be insufficient");
        assert!(matches!(
            error,
            SplitError::InsufficientDiskSpace {
                required_bytes: u64::MAX,
                ..
            }
        ));
        assert!(!job.exists());
        Ok(())
    }

    #[test]
    fn resume_preflight_credits_validated_existing_job_allocation()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        let preflight = disk_preflight(&job, 1024, 2048)?;
        assert_eq!(preflight.required_bytes, 1024);
        assert_eq!(preflight.existing_job_bytes, 2048);
        Ok(())
    }

    #[test]
    fn preflight_measures_an_existing_job_directory_for_mountpoint_support()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job = directory.path().join("job");
        std::fs::create_dir(&job)?;
        assert_eq!(preflight_filesystem_path(&job), job);
        let missing = directory.path().join("missing/job");
        assert_eq!(preflight_filesystem_path(&missing), directory.path());
        Ok(())
    }

    #[test]
    fn staged_part_hashing_honors_interruption() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let part = directory.path().join("part.pst.partial");
        std::fs::write(&part, vec![0_u8; 2 * 1024 * 1024])?;
        assert_eq!(hash_file(&part, &AtomicBool::new(true))?, None);
        Ok(())
    }

    #[test]
    fn observed_underfill_extends_the_same_part_without_reordering()
    -> Result<(), Box<dyn std::error::Error>> {
        let candidates = (1..=10)
            .map(|index| packing_candidate(index, 256 * 1024))
            .collect::<Vec<_>>();
        let maximum = LayoutEstimator.estimate_part_bytes(&candidates[..4])?;
        let mut remaining = candidates.clone().into();
        let mut model = AdaptiveSizeModel::default();
        let mut selected = take_fitting_prefix(&mut remaining, maximum, &model)?;
        assert_eq!(selected.len(), 4);

        let estimate = LayoutEstimator.estimate_part_bytes(&selected)?;
        model.observe(estimate, estimate / 2)?;
        assert!(extend_to_fitting_prefix(
            &mut selected,
            &mut remaining,
            maximum,
            &model,
            None,
        )?);
        assert!(selected.len() > 4);
        assert_eq!(
            selected
                .iter()
                .map(|candidate| candidate.key.source_node_id)
                .collect::<Vec<_>>(),
            (1..=u32::try_from(selected.len())?)
                .map(Some)
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn observed_overrun_returns_only_the_ordered_tail() -> Result<(), Box<dyn std::error::Error>> {
        let mut selected = (1..=10)
            .map(|index| packing_candidate(index, 256 * 1024))
            .collect::<Vec<_>>();
        let maximum = LayoutEstimator.estimate_part_bytes(&selected[..4])?;
        let full_estimate = LayoutEstimator.estimate_part_bytes(&selected)?;
        let mut model = AdaptiveSizeModel::default();
        model.observe(full_estimate, maximum.saturating_mul(2))?;
        let mut remaining = VecDeque::new();
        shrink_to_fitting_prefix(&mut selected, &mut remaining, maximum, &model)?;

        assert!(!selected.is_empty());
        assert!(selected.len() < 10);
        let observed = selected
            .iter()
            .chain(remaining.iter())
            .map(|candidate| candidate.key.source_node_id)
            .collect::<Vec<_>>();
        assert_eq!(observed, (1..=10).map(Some).collect::<Vec<_>>());
        Ok(())
    }

    #[test]
    fn rejected_prefix_is_never_reexpanded_after_nonlinear_overrun()
    -> Result<(), Box<dyn std::error::Error>> {
        let candidates = (1..=12)
            .map(|index| packing_candidate(index, 256 * 1024))
            .collect::<Vec<_>>();
        let maximum = LayoutEstimator.estimate_part_bytes(&candidates[..4])?;
        let mut remaining = candidates.clone().into();
        let mut model = AdaptiveSizeModel::default();
        let mut selected = take_fitting_prefix(&mut remaining, maximum, &model)?;
        let initial_estimate = LayoutEstimator.estimate_part_bytes(&selected)?;
        model.observe(initial_estimate, initial_estimate / 2)?;
        assert!(extend_to_fitting_prefix(
            &mut selected,
            &mut remaining,
            maximum,
            &model,
            None,
        )?);

        let rejected = selected.len();
        let rejected_estimate = LayoutEstimator.estimate_part_bytes(&selected)?;
        model.observe(rejected_estimate, maximum.saturating_add(1))?;
        shrink_to_fitting_prefix(&mut selected, &mut remaining, maximum, &model)?;
        assert!(selected.len() < rejected);

        let fitting_estimate = LayoutEstimator.estimate_part_bytes(&selected)?;
        model.observe(fitting_estimate, fitting_estimate / 2)?;
        let _ = extend_to_fitting_prefix(
            &mut selected,
            &mut remaining,
            maximum,
            &model,
            Some(rejected),
        )?;
        assert!(selected.len() < rejected);
        Ok(())
    }
}
