#![deny(unsafe_code)]

mod attachment_mime;
mod canonical;
mod catalog;
mod inspection;
mod packing;
mod recovery;
mod report;
mod source;
mod split;
mod worker;
mod writer_input;

pub use canonical::{
    CanonicalAttachment, CanonicalError, CanonicalFolder, CanonicalFolderLocation,
    CanonicalFolderRole, CanonicalFolderSet, CanonicalMail, CanonicalMessagePlacement,
    CanonicalProperty, CanonicalRecipient, load_canonical_folders,
    load_canonical_folders_interruptible, load_canonical_mail, load_canonical_mail_interruptible,
    load_canonical_mail_tree_interruptible,
};
pub use inspection::{
    EncryptionMode, FileFormat, InfoReport, InspectionBackend, InspectionError, InventoryReport,
    LibpffBackend, PstMetadata, VerifyReport, info, info_with_backend, verify, verify_recovery,
    verify_recovery_source, verify_recovery_with_backend, verify_with_backend,
};
pub use packing::{
    PackCandidate, PackingError, PartAssignment, PartSizeEstimator, pack_candidates,
};
pub use recovery::{RecoveryError, RecoveryReport, recover};
pub use report::{JobReport, REPORT_SCHEMA_VERSION, ReportError, report};
pub use source::{SourceError, SourceFile, SourceIdentity, validate_output_relationship};
pub use split::{
    DiskPreflight, ExecutionMetrics, PartReport, SplitError, SplitFailureKind, SplitOptions,
    SplitReport, split, split_recovered_job,
};
pub use worker::{WorkerProtocolError, run_recovery_worker};
pub(crate) use worker::{receive_worker_catalog_body_with_progress, receive_worker_hello};
pub use writer_input::{
    CanonicalWriteError, PartWriterInput, build_part_writer_input,
    build_part_writer_input_interruptible,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub use catalog::{
    CandidateSink, ContentCompleteness, ItemKey, ProcessingStatus, RecoveryProvenance,
};
pub use libpff_sys::PffError;
pub use libpff_sys::RecoveryMode;

#[cfg(test)]
mod public_json_fixture_tests {
    use super::{InfoReport, JobReport, RecoveryReport, SplitReport, VerifyReport};

    #[test]
    fn tracked_public_command_fixtures_match_the_rust_contracts() -> Result<(), serde_json::Error> {
        let info: InfoReport =
            serde_json::from_str(include_str!("../../../tests/fixtures/json/info.json"))?;
        let verify: VerifyReport =
            serde_json::from_str(include_str!("../../../tests/fixtures/json/verify.json"))?;
        let recovery: RecoveryReport =
            serde_json::from_str(include_str!("../../../tests/fixtures/json/recover.json"))?;
        let split: SplitReport =
            serde_json::from_str(include_str!("../../../tests/fixtures/json/split.json"))?;
        let report: JobReport =
            serde_json::from_str(include_str!("../../../tests/fixtures/json/report.json"))?;

        assert_eq!(info.command, "info");
        assert_eq!(verify.command, "verify");
        assert_eq!(recovery.command, "recover");
        assert_eq!(split.command, "split");
        assert_eq!(report.command, "report");
        Ok(())
    }
}
