use std::path::Path;

use pstforge_job::{
    JobError, PublishedPartRecord, ReportLedgerEvidence, read_validated_report_snapshot,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{SplitReport, VERSION};

pub const REPORT_SCHEMA_VERSION: &str = "1.0.0";

#[derive(Debug, Error)]
pub enum ReportError {
    #[error(transparent)]
    Job(#[from] JobError),
    #[error("job report snapshot is invalid: {0}")]
    InvalidSnapshot(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobReport {
    pub schema_version: String,
    pub command: String,
    pub producer_version: String,
    pub split: SplitReport,
}

pub fn report(job_directory: &Path) -> Result<JobReport, ReportError> {
    let validated = read_validated_report_snapshot(job_directory)?;
    let split: SplitReport = serde_json::from_str(&validated.json)
        .map_err(|error| ReportError::InvalidSnapshot(error.to_string()))?;
    validate_split_snapshot(&split, &validated.parts, &validated.evidence)?;
    Ok(JobReport {
        schema_version: REPORT_SCHEMA_VERSION.to_owned(),
        command: "report".to_owned(),
        producer_version: VERSION.to_owned(),
        split,
    })
}

fn validate_split_snapshot(
    split: &SplitReport,
    records: &[PublishedPartRecord],
    evidence: &ReportLedgerEvidence,
) -> Result<(), ReportError> {
    if split.schema_version != crate::split::SPLIT_SCHEMA_VERSION || split.command != "split" {
        return Err(ReportError::InvalidSnapshot(
            "unsupported split report schema or command".to_owned(),
        ));
    }
    if split.parts.len() != records.len() {
        return Err(ReportError::InvalidSnapshot(
            "part count disagrees with the durable ledger".to_owned(),
        ));
    }
    let mut written_candidates = 0_u64;
    for (part, record) in split.parts.iter().zip(records) {
        let expected = &record.sidecar;
        if part.index != record.part.index
            || part.filename != record.part.filename
            || part.byte_len != record.part.byte_len
            || part.sha256 != record.part.sha256
            || part.folder_count != expected.folder_count
            || part.message_count != expected.message_count
            || part.oversize != record.part.oversize
            || part.partial != expected.partial
            || part.omitted_folders != expected.omitted_folders
            || part.omitted_properties != expected.omitted_properties
            || part.omitted_attachments != expected.omitted_attachments
        {
            return Err(ReportError::InvalidSnapshot(format!(
                "part {} disagrees with its durable manifest",
                record.part.index
            )));
        }
        written_candidates = written_candidates
            .checked_add(record.item_count)
            .ok_or_else(|| {
                ReportError::InvalidSnapshot("written candidate count overflow".to_owned())
            })?;
    }
    if split.written_candidates != written_candidates {
        return Err(ReportError::InvalidSnapshot(
            "written candidate count disagrees with the durable ledger".to_owned(),
        ));
    }
    let recovery = &split.recovery;
    let source = &evidence.source;
    let summary = evidence.summary;
    let normal_items = summary
        .committed_candidates
        .saturating_sub(summary.recovered_candidates)
        .saturating_sub(summary.orphan_candidates)
        .saturating_sub(summary.fragment_candidates);
    let source_matches = recovery.source.canonical_path == source.canonical_path
        && recovery.source.device == source.device
        && recovery.source.inode == source.inode
        && recovery.source.size_bytes == source.size_bytes
        && recovery.source.modified_at == source.modified_at
        && recovery.source.sha256 == source.sha256;
    if !source_matches
        || split.execution_mode != evidence.configuration.execution_mode
        || split.maximum_pst_bytes != evidence.configuration.maximum_pst_bytes
        || recovery.mode != evidence.configuration.recovery_mode
        || recovery.normal_items != normal_items
        || recovery.recovered_items != summary.recovered_candidates
        || recovery.orphan_items != summary.orphan_candidates
        || recovery.fragment_items != summary.fragment_candidates
        || recovery.committed_candidates != summary.committed_candidates
        || recovery.complete_candidates != summary.complete_candidates
        || recovery.partial_candidates != summary.partial_candidates
        || recovery.unsupported_candidates != summary.unsupported_candidates
        || recovery.blob_count != summary.blob_count
        || recovery.blob_bytes != summary.blob_bytes
        || recovery.worker_attempts != evidence.worker_attempts
        || recovery.worker_failures != evidence.worker_failures
        || recovery.isolated_units != evidence.isolated_units
        || recovery.interrupted != evidence.interrupted
        || split.rejection_counts != evidence.rejection_counts
        || split
            .terminal_failure
            .map(|category| category.metadata_name())
            != evidence.direct_terminal_failure.as_deref()
    {
        return Err(ReportError::InvalidSnapshot(
            "recovery summary disagrees with the durable ledger".to_owned(),
        ));
    }
    if let Some(completion) = &evidence.recovery_completion
        && (recovery.normal_items != completion.normal_items
            || recovery.recovered_items != completion.recovered_items
            || recovery.orphan_items != completion.orphan_items
            || recovery.fragment_items != completion.fragment_items
            || recovery.issues != completion.issues
            || recovery.issues_dropped != completion.issues_dropped
            || recovery.peak_worker_rss_bytes != completion.peak_worker_rss_bytes)
    {
        return Err(ReportError::InvalidSnapshot(
            "recovery completion disagrees with the durable ledger".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pstforge_job::{DurableCatalogSink, JobConfiguration, JobSourceIdentity};
    use serde_json::json;
    use tempfile::tempdir;

    use super::{REPORT_SCHEMA_VERSION, ReportError, report};

    #[test]
    fn report_schema_is_stable_for_0_5() {
        assert_eq!(REPORT_SCHEMA_VERSION, "1.0.0");
    }

    #[test]
    fn report_rejects_a_digest_valid_snapshot_that_disagrees_with_the_ledger()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let job_directory = directory.path().join("job");
        let sink = DurableCatalogSink::create(&job_directory)?;
        let source = JobSourceIdentity {
            canonical_path: "/external/mail.pst".to_owned(),
            device: 1,
            inode: 2,
            size_bytes: 3,
            modified_at: "2026-07-20T00:00:00Z".to_owned(),
            sha256: None,
        };
        sink.bind_source(&source)?;
        sink.bind_configuration(&JobConfiguration {
            tool_compatibility_major: 0,
            split_schema_version: "0.5.0".to_owned(),
            execution_mode: "direct".to_owned(),
            recovery_mode: "balanced".to_owned(),
            maximum_pst_bytes: 4_294_967_296,
            part_size_policy: "hard-maximum-v1".to_owned(),
            writer_format: "unicode-pst-v23".to_owned(),
        })?;
        sink.publish_report_snapshot(&json!({
            "schema_version": "0.5.0",
            "command": "split",
            "execution_mode": "direct",
            "maximum_pst_bytes": 4_294_967_296_u64,
            "resumed": false,
            "keep_work": false,
            "disk_preflight": {
                "required_bytes": 0,
                "available_bytes": 0,
                "existing_job_bytes": 0
            },
            "metrics": {
                "elapsed_millis": 0,
                "source_bytes": 3,
                "output_bytes": 0,
                "average_source_bytes_per_second": 0,
                "peak_process_rss_bytes": 0,
                "payload_pack_bytes_written": 0,
                "peak_payload_pack_bytes": 0,
                "active_pst_bytes_written": 0,
                "finalized_output_bytes": 0,
                "validator_input_bytes": 0,
                "peak_payload_and_active_pst_bytes": 0,
                "supervisor_filesystem_read_bytes": null,
                "supervisor_filesystem_write_bytes": null
            },
            "recovery": {
                "schema_version": "0.5.0",
                "command": "recover",
                "mode": "balanced",
                "source": source,
                "job_directory": job_directory.to_string_lossy(),
                "normal_items": 1,
                "recovered_items": 0,
                "orphan_items": 0,
                "fragment_items": 0,
                "committed_candidates": 0,
                "complete_candidates": 0,
                "partial_candidates": 0,
                "unsupported_candidates": 0,
                "blob_count": 0,
                "blob_bytes": 0,
                "issues": 0,
                "issues_dropped": 0,
                "worker_attempts": 0,
                "worker_failures": 0,
                "isolated_units": 0,
                "peak_worker_rss_bytes": 0,
                "interrupted": false,
                "source_unchanged": true
            },
            "rejection_counts": {},
            "parts": [],
            "written_candidates": 0,
            "partial": false,
            "terminal_failure": null
        }))?;
        sink.checkpoint()?;
        drop(sink);

        assert!(matches!(
            report(&job_directory),
            Err(ReportError::InvalidSnapshot(_))
        ));
        Ok(())
    }
}
