use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use libpff_sys::RecoveryMode;
use pstforge_job::{DurableCatalogSink, JobError, PartSidecar, PublishedPart};
use pstforge_pst::writer::{
    AttachmentContent, MessageSpec, WriterError, create_mail_store, validate_mail_store_input,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    CanonicalError, CanonicalWriteError, PackCandidate, PackingError, PartSizeEstimator,
    RecoveryError, RecoveryReport, SourceError, SourceFile, build_part_writer_input,
    load_canonical_mail, pack_candidates,
};

pub const SPLIT_SCHEMA_VERSION: &str = "0.4.0";
const MAX_SAFETY_RESERVE: u64 = 64 * 1024 * 1024;
const ESTIMATED_STORE_BYTES: u64 = 1024 * 1024;
const ESTIMATED_MESSAGE_BYTES: u64 = 64 * 1024;
const ESTIMATED_FOLDER_BYTES: u64 = 16 * 1024;

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
            | Self::StagedIo { .. }
            | Self::Writer(
                WriterError::OutputExists(_)
                | WriterError::PublishedDurability { .. }
                | WriterError::PublishedDestinationChanged(_)
                | WriterError::Io(_),
            ) => SplitFailureKind::Output,
            Self::Recovery(RecoveryError::Source(_)) => SplitFailureKind::Source,
            Self::Recovery(RecoveryError::Interrupted) => SplitFailureKind::Interrupted,
            Self::Writer(
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
            | Self::Writer(WriterError::IndependentValidatorIo { .. })
            | Self::ZeroMaximumSize
            | Self::UnknownAssignment
            | Self::TooManyParts => SplitFailureKind::Internal,
        }
    }
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
    pub omitted_properties: u64,
    pub omitted_attachments: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SplitReport {
    pub schema_version: String,
    pub command: String,
    pub maximum_pst_bytes: u64,
    pub recovery: RecoveryReport,
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
) -> Result<SplitReport, SplitError> {
    if maximum_pst_bytes == 0 {
        return Err(SplitError::ZeroMaximumSize);
    }
    let mut recovery = crate::recover(source_path, job_directory, worker_executable, mode)?;
    if recovery.interrupted {
        return Ok(SplitReport {
            schema_version: SPLIT_SCHEMA_VERSION.to_owned(),
            command: "split".to_owned(),
            maximum_pst_bytes,
            recovery,
            parts: Vec::new(),
            written_candidates: 0,
            partial: true,
        });
    }
    let source = SourceFile::open(source_path).map_err(RecoveryError::Source)?;
    if source.identity() != &recovery.source {
        return Err(RecoveryError::Source(SourceError::Changed(source_path.to_path_buf())).into());
    }
    let (parts, written_candidates, output_partial) = split_recovered_job(
        job_directory,
        &recovery.source.sha256,
        mode,
        maximum_pst_bytes,
    )?;
    recovery.unsupported_candidates = DurableCatalogSink::open(job_directory)?
        .summary()?
        .unsupported_candidates;
    source.verify_unchanged().map_err(RecoveryError::Source)?;
    let partial = output_partial
        || recovery.partial_candidates != 0
        || recovery.unsupported_candidates != 0
        || recovery.issues != 0
        || recovery.issues_dropped != 0;
    Ok(SplitReport {
        schema_version: SPLIT_SCHEMA_VERSION.to_owned(),
        command: "split".to_owned(),
        maximum_pst_bytes,
        recovery,
        parts,
        written_candidates,
        partial,
    })
}

pub fn split_recovered_job(
    job_directory: &Path,
    source_sha256: &str,
    recovery_mode: RecoveryMode,
    maximum_pst_bytes: u64,
) -> Result<(Vec<PartReport>, u64, bool), SplitError> {
    if maximum_pst_bytes == 0 {
        return Err(SplitError::ZeroMaximumSize);
    }
    let mut job = DurableCatalogSink::open(job_directory)?;
    let mail = load_canonical_mail(&job)?;
    if mail.is_empty() {
        return Ok((Vec::new(), 0, false));
    }
    let by_key = mail
        .iter()
        .map(|message| (message.key, message))
        .collect::<BTreeMap<_, _>>();
    let candidates = mail
        .iter()
        .map(|message| PackCandidate {
            key: message.key,
            folder_path: message.folder_path.clone(),
            payload_bytes: message.spooled_bytes,
        })
        .collect::<Vec<_>>();
    let reserve = (maximum_pst_bytes / 20).min(MAX_SAFETY_RESERVE);
    let assignments = pack_candidates(candidates, maximum_pst_bytes, reserve, &LayoutEstimator)?;
    let mut queue = assignments
        .into_iter()
        .map(|assignment| assignment.candidates)
        .collect::<VecDeque<_>>();
    let mut reports = Vec::new();
    let mut part_index = 1_u32;
    let mut attempt = 0_u64;
    let mut written_candidates = 0_u64;
    let mut any_partial = false;

    while let Some(candidate_group) = queue.pop_front() {
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
        let mode_name = match recovery_mode {
            RecoveryMode::Balanced => "balanced",
            RecoveryMode::Aggressive => "aggressive",
        };
        let input = match build_part_writer_input(
            &job,
            &messages,
            source_sha256,
            mode_name,
            maximum_pst_bytes,
            part_index,
        ) {
            Ok(input) => input,
            Err(error) if candidate_local_translation_error(&error) => {
                if isolate_or_reject(&mut job, &mut queue, &candidate_group, &messages)? {
                    any_partial = true;
                }
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let staged_filename = format!("part-{part_index:04}-attempt-{attempt}.pst.partial");
        let staged_path = job.staged_part_path(&staged_filename)?;
        match validate_mail_store_input(&input.store) {
            Ok(()) => {}
            Err(WriterError::InputRejected(_)) => {
                if isolate_or_reject(&mut job, &mut queue, &candidate_group, &messages)? {
                    any_partial = true;
                }
                continue;
            }
            Err(error) => return Err(error.into()),
        }
        create_mail_store(&staged_path, &input.store)?;
        let byte_len = staged_path
            .metadata()
            .map_err(|source| staged_io(&staged_path, source))?
            .len();
        if byte_len > maximum_pst_bytes && candidate_group.len() > 1 {
            fs::remove_file(&staged_path).map_err(|source| staged_io(&staged_path, source))?;
            let midpoint = candidate_group.len() / 2;
            queue.push_front(candidate_group[midpoint..].to_vec());
            queue.push_front(candidate_group[..midpoint].to_vec());
            continue;
        }
        if !input.unsupported_item_keys.is_empty() {
            job.mark_candidates_unsupported(&input.unsupported_item_keys)?;
            any_partial = true;
        }
        let oversize = byte_len > maximum_pst_bytes;
        let sha256 = hash_file(&staged_path)?;
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
            .flat_map(|folder| &folder.messages)
            .map(count_messages)
            .fold(0_u64, u64::saturating_add);
        // Translator omissions already include every diagnostic record returned
        // by the writer; the report list must not increment the same occurrence.
        let omitted_properties = input.omitted_properties;
        let partial = input.partial || omitted_properties != 0 || input.omitted_attachments != 0;
        let sidecar = PartSidecar {
            schema_version: "1.0.0".to_owned(),
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
            omitted_properties,
            omitted_attachments: input.omitted_attachments,
        };
        job.publish_validated_part(&staged_filename, &published, &sidecar, &input.item_keys)?;
        reports.push(PartReport {
            index: part_index,
            filename,
            byte_len,
            sha256,
            folder_count,
            message_count,
            oversize,
            partial,
            omitted_properties,
            omitted_attachments: input.omitted_attachments,
        });
        written_candidates =
            written_candidates.saturating_add(saturating_len(input.item_keys.len()));
        any_partial |= partial || oversize;
        part_index = part_index.checked_add(1).ok_or(SplitError::TooManyParts)?;
    }
    job.checkpoint()?;
    Ok((reports, written_candidates, any_partial))
}

fn candidate_local_translation_error(error: &CanonicalWriteError) -> bool {
    matches!(
        error,
        CanonicalWriteError::InvalidCandidate { .. } | CanonicalWriteError::InvalidProperty { .. }
    )
}

fn isolate_or_reject(
    job: &mut DurableCatalogSink,
    queue: &mut VecDeque<Vec<PackCandidate>>,
    candidate_group: &[PackCandidate],
    messages: &[&crate::CanonicalMail],
) -> Result<bool, SplitError> {
    if candidate_group.len() > 1 {
        let midpoint = candidate_group.len() / 2;
        queue.push_front(candidate_group[midpoint..].to_vec());
        queue.push_front(candidate_group[..midpoint].to_vec());
        return Ok(false);
    }
    let mut item_keys = Vec::new();
    for message in messages {
        collect_item_keys(message, &mut item_keys);
    }
    job.mark_candidates_unsupported(&item_keys)?;
    Ok(true)
}

fn collect_item_keys(message: &crate::CanonicalMail, item_keys: &mut Vec<String>) {
    item_keys.push(message.durable_item_key.clone());
    for attachment in &message.attachments {
        if let Some(embedded) = &attachment.embedded {
            collect_item_keys(embedded, item_keys);
        }
    }
}

struct LayoutEstimator;

impl PartSizeEstimator for LayoutEstimator {
    fn estimate_part_bytes(&self, candidates: &[PackCandidate]) -> Result<u64, PackingError> {
        let folders = candidates
            .iter()
            .flat_map(|candidate| {
                (1..=candidate.folder_path.len())
                    .map(|length| candidate.folder_path[..length].to_vec())
            })
            .collect::<BTreeSet<_>>();
        let estimated = candidates
            .iter()
            .try_fold(ESTIMATED_STORE_BYTES, |total, candidate| {
                total
                    .checked_add(candidate.payload_bytes)
                    .and_then(|value| value.checked_add(ESTIMATED_MESSAGE_BYTES))
                    .ok_or(PackingError::SizeOverflow)
            })?;
        estimated
            .checked_add(saturating_len(folders.len()).saturating_mul(ESTIMATED_FOLDER_BYTES))
            .ok_or(PackingError::SizeOverflow)
    }
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

fn hash_file(path: &Path) -> Result<String, SplitError> {
    let mut file = File::open(path).map_err(|source| staged_io(path, source))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| staged_io(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
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
    use std::io;
    use std::path::PathBuf;

    use pstforge_job::JobError;
    use pstforge_pst::writer::WriterError;

    use super::{SplitError, SplitFailureKind};
    use crate::{RecoveryError, SourceError};

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
    }
}
