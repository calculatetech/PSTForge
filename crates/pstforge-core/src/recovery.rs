use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use libpff_sys::{PffError, PffFile};
use pstforge_job::{DurableCatalogSink, JobError, JobSourceIdentity};
use serde::Serialize;
use thiserror::Error;

use crate::{SourceError, SourceFile, SourceIdentity, validate_output_relationship};

pub const RECOVERY_SCHEMA_VERSION: &str = "0.3.0";

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Pff(#[from] PffError),
    #[error(transparent)]
    Job(#[from] JobError),
    #[error("libpff reported {native} bytes, but the open source has {source_size} bytes")]
    SizeMismatch { native: u64, source_size: u64 },
    #[error("source is not a PST (libpff content type: {raw:?})")]
    UnsupportedContentType { raw: Option<u8> },
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
    pub committed_candidates: u64,
    pub complete_candidates: u64,
    pub partial_candidates: u64,
    pub unsupported_candidates: u64,
    pub blob_count: u64,
    pub blob_bytes: u64,
    pub issues: u64,
    pub issues_dropped: u64,
    pub source_unchanged: bool,
}

pub fn recover(source_path: &Path, job_directory: &Path) -> Result<RecoveryReport, RecoveryError> {
    validate_output_relationship(source_path, job_directory)?;
    let source = SourceFile::open(source_path)?;
    let mut file = PffFile::open_fd(source.file().as_fd())?;
    let metadata = file.metadata()?;
    if metadata.content_type != Some(b'p') {
        return Err(RecoveryError::UnsupportedContentType {
            raw: metadata.content_type,
        });
    }
    if metadata.size != source.identity().size_bytes {
        return Err(RecoveryError::SizeMismatch {
            native: metadata.size,
            source_size: source.identity().size_bytes,
        });
    }

    let mut sink = DurableCatalogSink::create(job_directory)?;
    sink.bind_source(&JobSourceIdentity {
        canonical_path: source.identity().canonical_path.clone(),
        device: source.identity().device,
        inode: source.identity().inode,
        size_bytes: source.identity().size_bytes,
        modified_at: source.identity().modified_at.clone(),
        sha256: source.identity().sha256.clone(),
    })?;
    let catalog = file.recovery_catalog(&mut sink)?;
    sink.checkpoint()?;
    let summary = sink.summary()?;
    source.verify_unchanged()?;
    let normal_items = catalog
        .messages
        .checked_sub(catalog.recovered_messages)
        .and_then(|value| value.checked_sub(catalog.orphan_messages))
        .ok_or(RecoveryError::InconsistentCounters)?;
    let job_directory = job_directory
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(job_directory))
        .to_string_lossy()
        .into_owned();
    Ok(RecoveryReport {
        schema_version: RECOVERY_SCHEMA_VERSION.to_owned(),
        command: "recover".to_owned(),
        mode: "balanced".to_owned(),
        source: source.identity().clone(),
        job_directory,
        normal_items,
        recovered_items: catalog.recovered_messages,
        orphan_items: catalog.orphan_messages,
        committed_candidates: summary.committed_candidates,
        complete_candidates: summary.complete_candidates,
        partial_candidates: summary.partial_candidates,
        unsupported_candidates: summary.unsupported_candidates,
        blob_count: summary.blob_count,
        blob_bytes: summary.blob_bytes,
        issues: u64::try_from(catalog.issues.len()).unwrap_or(u64::MAX),
        issues_dropped: catalog.issues_dropped,
        source_unchanged: true,
    })
}
