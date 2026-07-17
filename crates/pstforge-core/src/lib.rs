#![deny(unsafe_code)]

mod canonical;
mod catalog;
mod inspection;
mod packing;
mod recovery;
mod source;
mod split;
mod worker;
mod writer_input;

pub use canonical::{
    CanonicalAttachment, CanonicalError, CanonicalFolderRole, CanonicalMail, CanonicalProperty,
    CanonicalRecipient, load_canonical_mail, load_canonical_mail_interruptible,
};
pub use inspection::{
    EncryptionMode, FileFormat, InfoReport, InspectionBackend, InspectionError, InventoryReport,
    LibpffBackend, PstMetadata, VerifyReport, info, info_with_backend, verify, verify_with_backend,
};
pub use packing::{
    PackCandidate, PackingError, PartAssignment, PartSizeEstimator, pack_candidates,
};
pub use recovery::{RecoveryError, RecoveryReport, recover};
pub use source::{SourceError, SourceFile, SourceIdentity, validate_output_relationship};
pub use split::{
    DiskPreflight, ExecutionMetrics, PartReport, SplitError, SplitFailureKind, SplitReport, split,
    split_recovered_job,
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
