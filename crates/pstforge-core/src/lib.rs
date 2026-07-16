#![deny(unsafe_code)]

mod catalog;
mod inspection;
mod recovery;
mod source;
mod worker;

pub use inspection::{
    EncryptionMode, FileFormat, InfoReport, InspectionBackend, InspectionError, InventoryReport,
    LibpffBackend, PstMetadata, VerifyReport, info, info_with_backend, verify, verify_with_backend,
};
pub use recovery::{RecoveryError, RecoveryReport, recover};
pub use source::{SourceError, SourceFile, SourceIdentity, validate_output_relationship};
pub use worker::{WorkerProtocolError, run_recovery_worker};
pub(crate) use worker::{receive_worker_catalog_body_with_progress, receive_worker_hello};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub use catalog::{
    CandidateSink, ContentCompleteness, ItemKey, ProcessingStatus, RecoveryProvenance,
};
pub use libpff_sys::PffError;
pub use libpff_sys::RecoveryMode;
