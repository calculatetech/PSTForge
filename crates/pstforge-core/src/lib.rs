#![deny(unsafe_code)]

mod catalog;
mod inspection;
mod recovery;
mod source;

pub use inspection::{
    EncryptionMode, FileFormat, InfoReport, InspectionBackend, InspectionError, InventoryReport,
    LibpffBackend, PstMetadata, VerifyReport, info, info_with_backend, verify, verify_with_backend,
};
pub use recovery::{RecoveryError, RecoveryReport, recover};
pub use source::{SourceError, SourceFile, SourceIdentity, validate_output_relationship};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub use catalog::{
    CandidateSink, ContentCompleteness, ItemKey, ProcessingStatus, RecoveryProvenance,
};
pub use libpff_sys::PffError;
