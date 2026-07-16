use std::os::fd::AsFd;
use std::path::Path;

use libpff_sys::{
    CatalogEvent, CatalogSink, PffError, PffFile, PropertyOwner, RawCatalog, RawPffMetadata,
};
use serde::Serialize;
use thiserror::Error;
use tracing::info;

use crate::{SourceError, SourceFile, SourceIdentity, VERSION};

pub const INSPECTION_SCHEMA_VERSION: &str = "1.1.0";

#[derive(Debug, Error)]
pub enum InspectionError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Pff(#[from] PffError),
    #[error("libpff reported {native} bytes, but the open source has {source_size} bytes")]
    SizeMismatch { native: u64, source_size: u64 },
    #[error("source is not a PST (libpff content type: {raw:?})")]
    UnsupportedContentType { raw: Option<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    Ansi32,
    Unicode64,
    Unicode64With4kPages,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionMode {
    None,
    Compressible,
    High,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Producer {
    pub pstforge_version: String,
    pub libpff_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PstMetadata {
    pub content_type: String,
    pub format: FileFormat,
    pub page_size_bytes: Option<u16>,
    pub encryption: EncryptionMode,
    pub corruption_observed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InfoReport {
    pub schema_version: String,
    pub command: String,
    pub producer: Producer,
    pub source: SourceIdentity,
    pub pst: PstMetadata,
    pub source_unchanged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InventoryIssueReport {
    pub node_id: Option<u32>,
    pub operation: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InventoryReport {
    pub scope: String,
    pub folders: u64,
    pub normal_items: u64,
    pub recipients: u64,
    pub attachments: u64,
    pub embedded_messages: u64,
    pub unsupported_messages: u64,
    pub raw_properties: u64,
    pub property_bytes: u64,
    pub body_bytes: u64,
    pub attachment_bytes: u64,
    pub peak_stream_chunk_bytes: u64,
    pub recovered_items: Option<u64>,
    pub orphan_items: Option<u64>,
    pub issues: Vec<InventoryIssueReport>,
    pub issues_dropped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyReport {
    pub schema_version: String,
    pub command: String,
    pub mode: String,
    pub producer: Producer,
    pub source: SourceIdentity,
    pub pst: PstMetadata,
    pub inventory: InventoryReport,
    pub source_unchanged: bool,
}

pub trait InspectionBackend {
    fn library_version(&self) -> String;
    fn metadata(&self) -> Result<RawPffMetadata, PffError>;
    fn catalog(&self, sink: &mut dyn CatalogSink) -> Result<RawCatalog, PffError>;
}

pub struct LibpffBackend {
    file: PffFile,
}

impl LibpffBackend {
    pub fn open(source: &SourceFile) -> Result<Self, PffError> {
        Ok(Self {
            file: PffFile::open_fd(source.file().as_fd())?,
        })
    }
}

impl InspectionBackend for LibpffBackend {
    fn library_version(&self) -> String {
        libpff_sys::library_version()
    }

    fn metadata(&self) -> Result<RawPffMetadata, PffError> {
        self.file.metadata()
    }

    fn catalog(&self, sink: &mut dyn CatalogSink) -> Result<RawCatalog, PffError> {
        self.file.catalog(sink)
    }
}

pub fn info(path: &Path) -> Result<InfoReport, InspectionError> {
    let source = SourceFile::open(path)?;
    let backend = LibpffBackend::open(&source)?;
    info_with_backend(&source, &backend)
}

pub fn verify(path: &Path) -> Result<VerifyReport, InspectionError> {
    let source = SourceFile::open(path)?;
    let backend = LibpffBackend::open(&source)?;
    verify_with_backend(&source, &backend)
}

pub fn info_with_backend(
    source: &SourceFile,
    backend: &dyn InspectionBackend,
) -> Result<InfoReport, InspectionError> {
    let raw = backend.metadata()?;
    validate_metadata(source, raw)?;
    source.verify_unchanged()?;
    info!(
        operation = "info",
        source_size = source.identity().size_bytes,
        "inspection complete"
    );
    Ok(InfoReport {
        schema_version: INSPECTION_SCHEMA_VERSION.to_owned(),
        command: "info".to_owned(),
        producer: producer(backend),
        source: source.identity().clone(),
        pst: map_metadata(raw),
        source_unchanged: true,
    })
}

pub fn verify_with_backend(
    source: &SourceFile,
    backend: &dyn InspectionBackend,
) -> Result<VerifyReport, InspectionError> {
    let raw = backend.metadata()?;
    validate_metadata(source, raw)?;
    let mut sink = InventorySink::default();
    let raw_inventory = backend.catalog(&mut sink)?;
    source.verify_unchanged()?;
    let issues = raw_inventory
        .issues
        .into_iter()
        .map(|issue| InventoryIssueReport {
            node_id: issue.node_id,
            operation: issue.operation.to_owned(),
            message: issue.message,
        })
        .collect();
    info!(
        operation = "verify",
        folders = raw_inventory.folders,
        normal_items = raw_inventory.messages,
        "reachable inventory complete"
    );
    Ok(VerifyReport {
        schema_version: INSPECTION_SCHEMA_VERSION.to_owned(),
        command: "verify".to_owned(),
        mode: "full".to_owned(),
        producer: producer(backend),
        source: source.identity().clone(),
        pst: map_metadata(raw),
        inventory: InventoryReport {
            scope: "reachable_mail_catalog".to_owned(),
            folders: raw_inventory.folders,
            normal_items: raw_inventory.messages,
            recipients: raw_inventory.recipients,
            attachments: raw_inventory.attachments,
            embedded_messages: raw_inventory.embedded_messages,
            unsupported_messages: raw_inventory.unsupported_messages,
            raw_properties: raw_inventory.properties,
            property_bytes: raw_inventory.property_bytes,
            body_bytes: sink.body_bytes,
            attachment_bytes: raw_inventory.attachment_bytes,
            peak_stream_chunk_bytes: sink.peak_chunk_bytes,
            recovered_items: None,
            orphan_items: None,
            issues,
            issues_dropped: raw_inventory.issues_dropped,
        },
        source_unchanged: true,
    })
}

#[derive(Default)]
struct InventorySink {
    body_bytes: u64,
    peak_chunk_bytes: u64,
}

impl CatalogSink for InventorySink {
    fn event(&mut self, event: CatalogEvent<'_>) -> Result<(), String> {
        match event {
            CatalogEvent::PropertyData { descriptor, bytes } => {
                let bytes = u64::try_from(bytes.len())
                    .map_err(|_| "property chunk length does not fit u64".to_owned())?;
                self.peak_chunk_bytes = self.peak_chunk_bytes.max(bytes);
                if matches!(descriptor.owner, PropertyOwner::Message(_))
                    && matches!(descriptor.entry_type, Some(0x1000 | 0x1009 | 0x1013))
                {
                    self.body_bytes = self
                        .body_bytes
                        .checked_add(bytes)
                        .ok_or_else(|| "body byte count overflowed".to_owned())?;
                }
            }
            CatalogEvent::AttachmentData { bytes, .. } => {
                let bytes = u64::try_from(bytes.len())
                    .map_err(|_| "attachment chunk length does not fit u64".to_owned())?;
                self.peak_chunk_bytes = self.peak_chunk_bytes.max(bytes);
            }
            _ => {}
        }
        Ok(())
    }
}

fn producer(backend: &dyn InspectionBackend) -> Producer {
    Producer {
        pstforge_version: VERSION.to_owned(),
        libpff_version: backend.library_version(),
    }
}

fn validate_metadata(source: &SourceFile, raw: RawPffMetadata) -> Result<(), InspectionError> {
    if raw.content_type != Some(b'p') {
        return Err(InspectionError::UnsupportedContentType {
            raw: raw.content_type,
        });
    }
    if raw.size != source.identity().size_bytes {
        return Err(InspectionError::SizeMismatch {
            native: raw.size,
            source_size: source.identity().size_bytes,
        });
    }
    Ok(())
}

fn map_metadata(raw: RawPffMetadata) -> PstMetadata {
    let (format, page_size_bytes) = match raw.file_type {
        Some(32) => (FileFormat::Ansi32, Some(512)),
        Some(64) => (FileFormat::Unicode64, Some(512)),
        Some(65) => (FileFormat::Unicode64With4kPages, Some(4096)),
        _ => (FileFormat::Unknown, None),
    };
    let content_type = match raw.content_type {
        Some(b'p') => "pst",
        Some(b'o') => "ost",
        Some(b'a') => "pab",
        _ => "unknown",
    };
    let encryption = match raw.encryption_type {
        Some(0) => EncryptionMode::None,
        Some(1) => EncryptionMode::Compressible,
        Some(2) => EncryptionMode::High,
        _ => EncryptionMode::Unknown,
    };
    PstMetadata {
        content_type: content_type.to_owned(),
        format,
        page_size_bytes,
        encryption,
        corruption_observed: raw.corrupted,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use libpff_sys::{CatalogEvent, CatalogSink, PffError, RawCatalog, RawPffMetadata};
    use tempfile::tempdir;

    use super::{FileFormat, InspectionBackend, verify_with_backend};
    use crate::SourceFile;

    struct FakeBackend {
        size: u64,
    }

    impl InspectionBackend for FakeBackend {
        fn library_version(&self) -> String {
            "test-libpff".to_owned()
        }

        fn metadata(&self) -> Result<RawPffMetadata, PffError> {
            Ok(RawPffMetadata {
                size: self.size,
                content_type: Some(b'p'),
                file_type: Some(64),
                encryption_type: Some(1),
                corrupted: false,
            })
        }

        fn catalog(&self, sink: &mut dyn CatalogSink) -> Result<RawCatalog, PffError> {
            sink.event(CatalogEvent::AttachmentData {
                message_id: 10,
                index: 0,
                bytes: &[0_u8; 17],
            })
            .map_err(|detail| PffError::Sink {
                operation: "fake catalog",
                detail,
            })?;
            Ok(RawCatalog {
                folders: 3,
                messages: 12,
                recovered_messages: 0,
                orphan_messages: 0,
                recipients: 24,
                attachments: 4,
                embedded_messages: 1,
                unsupported_messages: 2,
                properties: 60,
                property_bytes: 4096,
                attachment_bytes: 17,
                issues: Vec::new(),
                issues_dropped: 0,
            })
        }
    }

    #[test]
    fn fake_backend_builds_uniform_verify_report() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let path = directory.path().join("fake.pst");
        fs::write(&path, b"fake pst")?;
        let source = SourceFile::open(&path)?;
        let report = verify_with_backend(
            &source,
            &FakeBackend {
                size: source.identity().size_bytes,
            },
        )?;
        assert_eq!(report.pst.format, FileFormat::Unicode64);
        assert_eq!(report.inventory.folders, 3);
        assert_eq!(report.inventory.normal_items, 12);
        assert_eq!(report.inventory.recipients, 24);
        assert_eq!(report.inventory.peak_stream_chunk_bytes, 17);
        assert!(report.source_unchanged);
        Ok(())
    }
}
